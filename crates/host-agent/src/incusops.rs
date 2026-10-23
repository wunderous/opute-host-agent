//! Incus instance operations: the mutating side of Go's
//! `internal/domain/incus` (`vm_lifecycle.go`, `incus_launch.go`,
//! `incus_container.go`, `incus_root_disk.go`, `incus_stack.go`).
//!
//! This module is the Incus provider of the neutral instance services; it is
//! the only place, with `incus.rs`, that names Incus. Every command goes
//! through the provider runner, so a non-zero exit is a result and only a
//! missing binary is an error. Every operation on an existing instance checks
//! ownership first and fails closed.

use crate::config::Env;
use crate::gojson::{self, Node, Value};
use crate::hostobs::CommandResult;
use crate::incus::{first_non_empty, Incus, DISCOVERY_TIMEOUT, OWNER_LABEL};
use crate::resource::{AdmitError, Coordinator};
use crate::store::StateStore;
use serde_json::{json, Map, Value as J};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const AGENT_LABEL: &str = "user.opute.host_agent_id";
const DEFAULT_CPUS: i64 = 2;
const DEFAULT_MEMORY: &str = "2GiB";
const DEFAULT_VM_ROOT_DISK: &str = "10GiB";
const DEFAULT_CONTAINER_ROOT_DISK: &str = "20GiB";
const DEFAULT_VM_IMAGE: &str = "images:ubuntu/22.04";
const PROVISION_VM_TIMEOUT: Duration = Duration::from_secs(600);
const CONTAINER_ADDRESS_TIMEOUT: Duration = Duration::from_secs(180);
const CONTAINER_ADDRESS_POLL: Duration = Duration::from_secs(3);
const MINUTE: Duration = Duration::from_secs(60);
const WSL_GPU_LIB_PATH: &str = "/usr/lib/wsl/lib";
const NETPLAN_PATH: &str = "/etc/netplan/99-opute-static.yaml";

/// A failed operation: a plain error, or a typed admission refusal that
/// `tools.ErrorResult` renders with `owner: "admission"`.
#[derive(Debug)]
pub enum Fail {
    Text(String),
    Admission(AdmitError),
}

impl From<String> for Fail {
    fn from(text: String) -> Fail {
        Fail::Text(text)
    }
}

impl From<&str> for Fail {
    fn from(text: &str) -> Fail {
        Fail::Text(text.to_string())
    }
}

impl Fail {
    /// `fmt.Errorf(prefix + "%w", err)`.
    fn wrap(self, prefix: &str) -> Fail {
        match self {
            Fail::Text(text) => Fail::Text(format!("{prefix}{text}")),
            other => other,
        }
    }

    /// The error's text, as Go's `err.Error()`.
    pub(crate) fn text(self) -> String {
        match self {
            Fail::Text(text) => text,
            Fail::Admission(e) => e.to_string(),
        }
    }

    /// `tools.ErrorResult`.
    pub fn render(self) -> J {
        match self {
            Fail::Text(text) => crate::tools::error_result(&text),
            Fail::Admission(e) => crate::admission::Refusal::Admit(e).render(),
        }
    }
}

type R<T> = Result<T, Fail>;

/// What the instance services need: the provider, the registry, the
/// admission snapshot, and the stack-reset seams.
#[derive(Clone)]
pub struct Instances {
    pub incus: Incus,
    pub env: Env,
    pub state: Arc<Mutex<StateStore>>,
    pub tenant_id: String,
    pub coordinator: Coordinator,
    pub shared_host_owner_instance: String,
    /// `resolveResetCheckpointPath`: `<relay dir>/incus-reset-checkpoint.json`.
    pub reset_checkpoint_path: String,
    /// `revokeRelays`: the in-process relays into guests.
    pub relays: Arc<crate::tcprelay::Manager>,
    /// The postgres domain's relays, once that domain exists.
    pub postgres: Arc<std::sync::OnceLock<Arc<crate::postgresops::Live>>>,
    /// The llm domain's relays, once that domain exists.
    pub llm: Arc<std::sync::OnceLock<Arc<crate::llmops::Live>>>,
}

/// `ErrString`-style failure of one provider command: the run error, or
/// the trimmed stderr/stdout/fallback of a non-zero exit.
fn failed(res: &Result<CommandResult, String>, fallback: &str) -> Option<String> {
    match res {
        Err(e) => Some(first_non_empty(&[e]).to_string()),
        Ok(r) if r.exit_code != 0 => {
            Some(first_non_empty(&[&r.stderr, &r.stdout, fallback]).to_string())
        }
        Ok(_) => None,
    }
}

/// The trimmed stderr, else stdout, of a completed command.
fn output(res: &Result<CommandResult, String>) -> String {
    match res {
        Ok(r) => first_non_empty(&[&r.stderr, &r.stdout]).to_string(),
        Err(_) => String::new(),
    }
}

/// `urlPathEscape`.
fn escape(name: &str) -> String {
    name.replace('/', "%2F")
}

fn quote(s: &str) -> String {
    crate::goerr::quote(s)
}

/// `fsutil.Exists`.
fn exists(path: &str) -> bool {
    std::fs::metadata(path).is_ok()
}

/// `json.Unmarshal`'s error for a document that does not fit `target`.
fn decode_error(data: &str, target: &str) -> String {
    if let Some(e) = gojson::syntax_error(data.as_bytes()) {
        return e;
    }
    match gojson::parse(data.as_bytes()) {
        Ok(doc) => format!(
            "json: cannot unmarshal {} into Go value of type {target}",
            gojson::kind_name(&doc.value)
        ),
        Err(_) => "unexpected end of JSON input".into(),
    }
}

/// A struct field decoded as a Go string: absent or null is "", any other
/// kind fails the whole document.
fn string_field(node: &Node, key: &str, data: &str, target: &str) -> Result<String, String> {
    match node.field(key).map(|n| &n.value) {
        None | Some(Value::Null) => Ok(String::new()),
        Some(Value::String(s)) => Ok(s.clone()),
        Some(other) => {
            let _ = data;
            Err(format!(
                "json: cannot unmarshal {} into Go struct field {target}.{key} of type string",
                gojson::kind_name(other)
            ))
        }
    }
}

/// `normalizeIncusLaunchImage`.
pub fn normalize_launch_image(image: &str) -> String {
    let image = image.trim();
    if image.is_empty() {
        return DEFAULT_VM_IMAGE.into();
    }
    if image.starts_with("images:") || image.starts_with("local:") {
        return image.into();
    }
    match image.find(':') {
        Some(colon) if colon > 0 => {
            let (remote, alias) = (&image[..colon], &image[colon + 1..]);
            if remote == "ubuntu" {
                if alias.contains('/') {
                    format!("images:{alias}")
                } else {
                    format!("images:ubuntu/{alias}")
                }
            } else {
                image.into()
            }
        }
        _ => image.into(),
    }
}

/// `resolveIncusImageSource`.
fn image_source(normalized: &str) -> J {
    let image = normalized.trim();
    let pull = |alias: &str| {
        json!({"type": "image", "mode": "pull", "server": "https://images.linuxcontainers.org",
               "protocol": "simplestreams", "alias": alias})
    };
    if let Some(alias) = image.strip_prefix("images:") {
        return pull(alias);
    }
    if let Some(alias) = image.strip_prefix("local:") {
        return json!({"type": "image", "alias": alias});
    }
    if let Some(colon) = image.find(':').filter(|c| *c > 0) {
        let (remote, alias) = (&image[..colon], &image[colon + 1..]);
        if remote == "ubuntu" {
            let alias = if alias.contains('/') {
                alias.to_string()
            } else {
                format!("ubuntu/{alias}")
            };
            return pull(&alias);
        }
    }
    json!({"type": "image", "alias": image})
}

/// `IncusRuntimeKindError.Error`: the struct's JSON, empty optional fields
/// omitted.
fn runtime_kind_error(
    code: &str,
    vm: &str,
    requested: &str,
    observed: &str,
    remediation: &str,
) -> String {
    let mut out = String::from("{\"code\":");
    gojson::encode_string(code, &mut out);
    for (key, value) in [
        ("vmName", vm),
        ("requested", requested),
        ("observed", observed),
    ] {
        if !value.is_empty() {
            out.push_str(&format!(",\"{key}\":"));
            gojson::encode_string(value, &mut out);
        }
    }
    out.push_str(",\"remediation\":");
    gojson::encode_string(remediation, &mut out);
    out.push('}');
    out
}

/// `parseProvisionInstanceType`: the kind and whether it was explicit.
fn parse_instance_type(raw: &str) -> R<(&'static str, bool)> {
    match raw.trim().to_lowercase().as_str() {
        "" => Ok(("container", false)),
        "container" | "system-container" => Ok(("container", true)),
        "vm" | "virtual-machine" | "virtual_machine" | "virtual machine" => {
            Ok(("virtual-machine", true))
        }
        _ => Err(runtime_kind_error(
            "incus_runtime_kind_invalid",
            "",
            raw.trim(),
            "",
            "Use instanceType=container or instanceType=virtual-machine.",
        )
        .into()),
    }
}

/// `normalizeObservedInstanceType`, with the instance named in the error.
fn observed_instance_type(raw: &str, vm: &str) -> R<&'static str> {
    match raw.trim().to_lowercase().as_str() {
        "container" => Ok("container"),
        "virtual-machine" | "virtual machine" => Ok("virtual-machine"),
        _ => Err(runtime_kind_error(
            "incus_runtime_kind_unknown",
            vm,
            "",
            raw.trim(),
            "Inspect the Incus instance runtime kind before provisioning it.",
        )
        .into()),
    }
}

/// `rootDiskQuota`: the admitted size is empty when no enforceable bound
/// exists.
#[derive(Default)]
struct Quota {
    size: String,
    pool: String,
    driver: String,
    enforced: bool,
    reason: String,
}

/// `instanceRootDevice`.
struct RootDevice {
    kind: String,
    path: String,
    pool: String,
    size: String,
}

/// `ProvisionVMArgs`.
pub struct ProvisionArgs {
    pub vm_name: String,
    pub image: String,
    pub cpus: i64,
    pub memory: String,
    pub disk: String,
    pub instance_type: String,
}

/// `ProvisionContainerArgs`.
#[derive(Default)]
pub struct ContainerArgs {
    pub container_name: String,
    pub image: String,
    pub cpus: i64,
    pub memory: String,
    pub disk: String,
    pub gpu: bool,
    pub host_gpu_libs: bool,
    pub nesting: Option<bool>,
    pub port: i64,
    pub model_volume: String,
    pub ipv4_address: String,
}

/// `VMStatusResult`.
pub struct VmStatus {
    pub uri: String,
    pub vm_name: String,
    pub image: String,
    pub status: String,
    pub instance_type: String,
}

impl VmStatus {
    pub fn to_json(&self) -> J {
        let mut out = Map::new();
        out.insert("uri".into(), J::from(self.uri.clone()));
        out.insert("vmName".into(), J::from(self.vm_name.clone()));
        if !self.image.is_empty() {
            out.insert("image".into(), J::from(self.image.clone()));
        }
        out.insert("status".into(), J::from(self.status.clone()));
        if !self.instance_type.is_empty() {
            out.insert("instanceType".into(), J::from(self.instance_type.clone()));
        }
        J::Object(out)
    }
}

/// `ContainerStatusResult`.
pub struct ContainerStatus {
    pub uri: String,
    pub container_name: String,
    pub image: String,
    pub status: String,
    pub instance_type: String,
}

impl Instances {
    fn run(&self, args: &[&str], timeout: Duration) -> Result<CommandResult, String> {
        self.incus.run(args, timeout)
    }

    /// The command must succeed; its failure is the error.
    fn must(&self, args: &[&str], timeout: Duration, fallback: &str) -> R<CommandResult> {
        let res = self.run(args, timeout);
        match failed(&res, fallback) {
            Some(detail) => match res {
                Err(e) => Err(e.into()),
                Ok(_) => Err(detail.into()),
            },
            None => Ok(res.expect("checked")),
        }
    }

    fn assert_ownership(&self, vm: &str, operation: &str) -> R<()> {
        self.incus
            .assert_ownership(vm, operation)
            .map_err(Fail::from)
    }

    fn register(&self, uri: &str, coordinates: Map<String, J>) {
        let _ = crate::resource::register(&self.state, &self.tenant_id, uri, &coordinates);
    }

    fn uri_for(&self, name: &str) -> String {
        crate::resource::uri_for_provider_name(&self.state, &self.tenant_id, name)
    }

    // --- shared probes (incus_launch.go, incus_inventory.go) -----------------------------

    /// `incusInstanceExists`.
    fn instance_exists(&self, name: &str) -> R<bool> {
        let res = self.run(&["list", name, "--format", "csv"], Duration::from_secs(30))?;
        if res.exit_code != 0 {
            return Err(format!(
                "incus list {}: {}",
                quote(name),
                first_non_empty(&[&res.stderr, &res.stdout, "incus list failed"])
            )
            .into());
        }
        Ok(!res.stdout.trim().is_empty())
    }

    /// `ReadInstanceType`.
    fn read_instance_type(&self, name: &str) -> R<String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("instance name is required".into());
        }
        self.assert_ownership(name, "read_instance_info")?;
        let path = format!("/1.0/instances/{}", escape(name));
        let res = self.must(
            &["query", &path],
            DISCOVERY_TIMEOUT,
            "incus instance query failed",
        )?;
        const TARGET: &str = "struct { Type string \"json:\\\"type\\\"\" }";
        let doc =
            gojson::parse(res.stdout.as_bytes()).map_err(|_| decode_error(&res.stdout, TARGET))?;
        if !matches!(doc.value, Value::Object(_) | Value::Null) {
            return Err(decode_error(&res.stdout, TARGET).into());
        }
        let kind = string_field(&doc, "type", &res.stdout, "")?;
        let kind = kind.trim();
        if kind.is_empty() {
            return Err("incus instance type is missing".into());
        }
        Ok(kind.to_string())
    }

    /// `readDefaultProfileDevices`, or `None` when it cannot be read (every
    /// caller falls back).
    fn default_profile_devices(&self) -> Option<Vec<(String, [String; 3])>> {
        let res = self
            .run(&["query", "/1.0/profiles/default"], DISCOVERY_TIMEOUT)
            .ok()?;
        if res.exit_code != 0 {
            return None;
        }
        let doc = gojson::parse(res.stdout.as_bytes()).ok()?;
        if !matches!(doc.value, Value::Object(_) | Value::Null) {
            return None;
        }
        let mut out = Vec::new();
        match doc.field("devices").map(|n| &n.value) {
            None | Some(Value::Null) => {}
            Some(Value::Object(devices)) => {
                for (name, device) in devices {
                    let fields = match &device.value {
                        Value::Null => Default::default(),
                        Value::Object(_) => {
                            let get = |k: &str| gojson::go_string(device.field(k));
                            [get("type").ok()?, get("path").ok()?, get("pool").ok()?]
                        }
                        _ => return None,
                    };
                    // A repeated key decodes into the same map entry.
                    out.retain(|(n, _): &(String, [String; 3])| n != name);
                    out.push((name.clone(), fields));
                }
            }
            _ => return None,
        }
        Some(out)
    }

    /// `setIncusInstanceConfig`.
    fn set_config(&self, vm: &str, key: &str, value: &str) -> R<()> {
        if key != OWNER_LABEL {
            self.assert_ownership(vm, "set_instance_config")?;
        }
        self.must(
            &["config", "set", vm, key, value],
            DISCOVERY_TIMEOUT,
            "incus config set failed",
        )?;
        Ok(())
    }

    /// `ensureIncusInstanceAutostart`.
    fn ensure_autostart(&self, name: &str) -> R<()> {
        self.set_config(name, "boot.autostart", "true")
    }

    // --- root disk quota (incus_storage_quota.go, incus_root_disk.go) ---------------------

    /// `admitRootDiskQuotaForPool`; an empty pool resolves the pool a new
    /// instance would land on.
    fn admit_root_disk(&self, requested: &str, explicit: bool, pool: &str) -> R<Quota> {
        let requested = requested.trim();
        if requested.is_empty() {
            return Ok(Quota::default());
        }
        let support = if pool.trim().is_empty() {
            self.incus.root_disk_quota()?
        } else {
            self.incus.pool_quota(pool)?
        };
        let text = |k: &str| support[k].as_str().unwrap_or("").to_string();
        let mut quota = Quota {
            size: String::new(),
            pool: text("pool"),
            driver: text("driver"),
            enforced: support["enforced"].as_bool().unwrap_or(false),
            reason: text("reason"),
        };
        if quota.enforced {
            quota.size = requested.to_string();
            return Ok(quota);
        }
        if explicit {
            return Err(format!(
                "root disk quota {} cannot be enforced: {}; provision on a pool with a quota-capable driver (btrfs, zfs, lvm, ceph) or enable project quotas on the dir pool's filesystem",
                quote(requested),
                quota.reason
            )
            .into());
        }
        Ok(quota)
    }

    /// `readInstanceRootDevice`.
    fn read_root_device(&self, name: &str) -> R<RootDevice> {
        let name = name.trim();
        if name.is_empty() {
            return Err("instance name is required".into());
        }
        let path = format!("/1.0/instances/{}", escape(name));
        let res = self.must(
            &["query", &path],
            DISCOVERY_TIMEOUT,
            "incus instance query failed",
        )?;
        const TARGET: &str = "struct { Devices map[string]map[string]interface {} \"json:\\\"devices\\\"\"; ExpandedDevices map[string]map[string]interface {} \"json:\\\"expanded_devices\\\"\" }";
        let doc =
            gojson::parse(res.stdout.as_bytes()).map_err(|_| decode_error(&res.stdout, TARGET))?;
        if !matches!(doc.value, Value::Object(_) | Value::Null) {
            return Err(decode_error(&res.stdout, TARGET).into());
        }
        let devices = |key: &str| -> R<Map<String, J>> {
            match doc.field(key).map(|n| &n.value) {
                None | Some(Value::Null) => Ok(Map::new()),
                Some(Value::Object(members)) => {
                    let mut out = Map::new();
                    for (name, device) in members {
                        match &device.value {
                            Value::Null => {
                                out.insert(name.clone(), J::Object(Map::new()));
                            }
                            Value::Object(_) => {
                                out.insert(name.clone(), crate::transport::go_any(device));
                            }
                            other => {
                                return Err(format!(
                                    "json: cannot unmarshal {} into Go struct field .{key} of type map[string]interface {{}}",
                                    gojson::kind_name(other)
                                )
                                .into())
                            }
                        }
                    }
                    Ok(out)
                }
                Some(other) => Err(format!(
                    "json: cannot unmarshal {} into Go struct field .{key} of type map[string]map[string]interface {{}}",
                    gojson::kind_name(other)
                )
                .into()),
            }
        };
        let (own, expanded) = (devices("devices")?, devices("expanded_devices")?);
        let non_empty = |m: &Map<String, J>| {
            m.get("root")
                .and_then(J::as_object)
                .filter(|r| !r.is_empty())
                .cloned()
        };
        let Some(root) = non_empty(&own).or_else(|| non_empty(&expanded)) else {
            return Err(format!("instance {} has no root disk device", quote(name)).into());
        };
        let text = |k: &str| {
            root.get(k)
                .and_then(J::as_str)
                .unwrap_or("")
                .trim()
                .to_string()
        };
        let mut wrapped = Map::new();
        wrapped.insert("root".into(), J::Object(root.clone()));
        let mut device = RootDevice {
            kind: text("type"),
            path: text("path"),
            pool: text("pool"),
            size: crate::incus::root_device_size(Some(&wrapped)),
        };
        if device.kind.is_empty() {
            device.kind = "disk".into();
        }
        if device.path.is_empty() {
            device.path = "/".into();
        }
        Ok(device)
    }

    /// `patchInstanceRootDisk`.
    fn patch_root_disk(&self, name: &str, device: &J, timeout: Duration) -> R<()> {
        let payload = gojson::encode_to_string(&json!({"devices": {"root": device}}));
        let timeout = if timeout.is_zero() {
            DISCOVERY_TIMEOUT
        } else {
            timeout
        };
        let path = format!("/1.0/instances/{}", escape(name));
        let res = self.run(
            &["query", "-X", "PATCH", "--wait", &path, "-d", &payload],
            timeout,
        )?;
        if res.exit_code != 0 {
            return Err(format!(
                "incus resize root disk {}: {}",
                quote(name),
                first_non_empty(&[&res.stderr, &res.stdout, "failed to resize root disk"])
            )
            .into());
        }
        Ok(())
    }

    /// `applyInstanceRootDiskQuota`.
    fn apply_root_disk(&self, vm: &str, disk: &str) -> R<Quota> {
        let current = self.read_root_device(vm)?;
        let quota = self.admit_root_disk(disk, true, &current.pool)?;
        if quota.size.is_empty() {
            return Err(format!(
                "root disk quota {} cannot be enforced: {}",
                quote(disk),
                quota.reason
            )
            .into());
        }
        refuse_shrink(&quota.size, &current.size)?;
        if !current.size.is_empty() {
            let want = crate::resource::parse_byte_capacity(&quota.size);
            let have = crate::resource::parse_byte_capacity(&current.size);
            if let (Ok(want), Ok(have)) = (want, have) {
                if want == have {
                    return Ok(quota);
                }
            }
        }
        let mut device = Map::new();
        device.insert(
            "type".into(),
            J::from(first_non_empty(&[&current.kind, "disk"])),
        );
        device.insert(
            "path".into(),
            J::from(first_non_empty(&[&current.path, "/"])),
        );
        device.insert("size".into(), J::from(quota.size.clone()));
        if !current.pool.is_empty() {
            device.insert("pool".into(), J::from(current.pool.clone()));
        }
        self.patch_root_disk(vm, &J::Object(device), 2 * MINUTE)?;
        Ok(quota)
    }

    // --- admission (vm_lifecycle.go) ------------------------------------------------------

    /// `enforceIncusAggregateBudget`: declared Incus limits against the
    /// effective host budget. Existing instances are only observed.
    fn enforce_budget(&self, vm: &str, cpus: i64, memory: &str, disk: &str) -> R<()> {
        let snapshot = self.coordinator.snapshot();
        if snapshot["enforcement"].as_str() != Some("enforced") {
            return Err(Fail::Admission(AdmitError::Admission {
                code: "host_resource_enforcement_unknown".into(),
                class: "heavy".into(),
                pressure: snapshot["pressure"].as_str().unwrap_or("").into(),
                reason: "Incus workload launch requires verified host cgroup enforcement".into(),
                retry_after_ms: 1000,
            }));
        }
        let capacity = self
            .incus
            .inventory_capacity()
            .map_err(|e| format!("observe Incus allocation before launch: {e}"))?;
        let total = |k: &str| capacity[k].as_i64().unwrap_or(0);
        let limits = &snapshot["effectiveLimits"];
        let saturated = |reason: &str| {
            Fail::Admission(AdmitError::Admission {
                code: "host_capacity_saturated".into(),
                class: "heavy".into(),
                pressure: "normal".into(),
                reason: format!("cannot launch Incus instance {}: {reason}", quote(vm)),
                retry_after_ms: 1000,
            })
        };
        let cpu_cores = limits["cpuCores"].as_f64().unwrap_or(0.0);
        if cpu_cores > 0.0 && (total("totalVmCpuLimitCores") + cpus) as f64 > cpu_cores {
            return Err(saturated("CPU allocation exceeds effective host capacity"));
        }
        let memory_limit = limits["memoryBytes"].as_i64().unwrap_or(0);
        let requested = crate::incus::parse_capacity_bytes(memory);
        if memory_limit > 0
            && requested > 0
            && total("totalVmMemoryLimitBytes") + requested > memory_limit
        {
            return Err(saturated(
                "memory allocation exceeds effective host capacity",
            ));
        }
        let disk_limit = limits["diskBytes"].as_i64().unwrap_or(0);
        let requested = crate::incus::parse_capacity_bytes(disk);
        if disk_limit > 0
            && requested > 0
            && total("totalVmDiskLimitBytes") + requested > disk_limit
        {
            return Err(saturated("disk allocation exceeds effective host capacity"));
        }
        Ok(())
    }

    // --- VM lifecycle (vm_lifecycle.go, incus_launch.go) ---------------------------------

    /// `vmStatusResult`: registers the instance under its `vm:` or
    /// `container:` URI.
    fn vm_status(&self, name: &str, image: &str, status: &str, kind: &str) -> VmStatus {
        let resource_type = if kind == "virtual-machine" {
            "vm"
        } else {
            "container"
        };
        let mut result = VmStatus {
            uri: String::new(),
            vm_name: name.into(),
            image: image.into(),
            status: status.into(),
            instance_type: kind.into(),
        };
        if let Ok(uri) = crate::resource::Uri::new(resource_type, &self.tenant_id, name) {
            result.uri = uri.to_string();
            let mut coordinates = Map::new();
            coordinates.insert("providerInstanceName".into(), J::from(name));
            coordinates.insert("instanceType".into(), J::from(kind));
            self.register(&result.uri, coordinates);
        }
        result
    }

    /// `CreateVM` / `ProvisionVM`.
    pub fn provision_vm(&self, args: &ProvisionArgs) -> R<VmStatus> {
        let vm = args.vm_name.trim();
        if vm.is_empty() {
            return Err("vmName is required".into());
        }
        let image = normalize_launch_image(args.image.trim());
        let cpus = if args.cpus <= 0 {
            DEFAULT_CPUS
        } else {
            args.cpus
        };
        let memory = match args.memory.trim() {
            "" => DEFAULT_MEMORY.to_string(),
            m => m.to_string(),
        };
        let (kind, explicit) = parse_instance_type(&args.instance_type)?;
        let existing = self
            .instance_exists(vm)
            .map_err(|e| e.wrap("inspect existing Incus instance: "))?;
        if existing {
            self.assert_ownership(vm, "provision_vm")?;
            let observed = self
                .read_instance_type(vm)
                .map_err(|e| e.wrap("validate existing Incus runtime kind: "))?;
            let actual = observed_instance_type(&observed, vm)?;
            if explicit && actual != kind {
                return Err(runtime_kind_error(
                    "incus_runtime_kind_mismatch",
                    vm,
                    kind,
                    actual,
                    "Use the existing instance runtime kind or delete it through the approved lifecycle operation before reprovisioning.",
                )
                .into());
            }
            // An omitted kind reuses the observed one: a status/start, never a
            // create or resize.
            if actual == "virtual-machine" {
                self.start_existing(vm)?;
                return Ok(self.vm_status(vm, &image, "running", actual));
            }
            let container = self.provision_container(&ContainerArgs {
                container_name: vm.into(),
                image: image.clone(),
                cpus,
                memory: memory.clone(),
                disk: args.disk.clone(),
                nesting: Some(true),
                ..Default::default()
            })?;
            return Ok(self.vm_status(
                &container.container_name,
                &container.image,
                &container.status,
                &container.instance_type,
            ));
        }
        let requested = args.disk.trim();
        let disk = if requested.is_empty() {
            DEFAULT_VM_ROOT_DISK
        } else {
            requested
        };
        let quota = self.admit_root_disk(disk, !requested.is_empty(), "")?;
        let disk = quota.size;
        if kind == "virtual-machine" {
            self.enforce_budget(vm, cpus, &memory, &disk)?;
            self.launch_vm(vm, &image, cpus, &memory, &disk, PROVISION_VM_TIMEOUT)?;
            let image = if image.is_empty() {
                DEFAULT_VM_IMAGE.to_string()
            } else {
                normalize_launch_image(&image)
            };
            return Ok(self.vm_status(vm, &image, "running", "virtual-machine"));
        }
        let container = self.provision_container(&ContainerArgs {
            container_name: vm.into(),
            image,
            cpus,
            memory,
            disk,
            nesting: Some(true),
            ..Default::default()
        })?;
        Ok(self.vm_status(
            &container.container_name,
            &container.image,
            &container.status,
            &container.instance_type,
        ))
    }

    /// `startExistingInstance`.
    fn start_existing(&self, name: &str) -> R<()> {
        let started = self.run(&["start", name], 2 * MINUTE);
        let refused = match &started {
            Err(_) => true,
            Ok(r) => {
                r.exit_code != 0 && !output(&started).to_lowercase().contains("already running")
            }
        };
        if refused {
            return Err(format!(
                "start existing Incus instance: {}",
                failed(&started, "incus start failed").unwrap_or_default()
            )
            .into());
        }
        self.ensure_autostart(name)
    }

    /// `launchIncusVMViaAPI`.
    fn launch_vm(
        &self,
        vm: &str,
        image: &str,
        cpus: i64,
        memory: &str,
        disk: &str,
        timeout: Duration,
    ) -> R<()> {
        let image = normalize_launch_image(image);
        let profile = self.default_profile_devices().unwrap_or_default();
        let cpus = if cpus <= 0 { DEFAULT_CPUS } else { cpus };
        let memory = if memory.is_empty() {
            DEFAULT_MEMORY
        } else {
            memory
        };
        let mut config = Map::new();
        config.insert("limits.cpu".into(), J::from(cpus.to_string()));
        config.insert("limits.memory".into(), J::from(memory));
        config.insert("boot.autostart".into(), J::from("true"));
        let owner = self.incus.instance_id.trim();
        if !owner.is_empty() {
            config.insert(OWNER_LABEL.into(), J::from(owner));
            let agent = self.incus.agent_id.trim();
            if !agent.is_empty() {
                config.insert(AGENT_LABEL.into(), J::from(agent));
            }
        }
        let mut root = Map::new();
        root.insert("type".into(), J::from("disk"));
        root.insert("path".into(), J::from("/"));
        // No admitted quota: omit the size rather than write an unenforced one.
        if !disk.is_empty() {
            root.insert("size".into(), J::from(disk));
        }
        let profile_root = profile.iter().find(|(n, _)| n == "root").map(|(_, d)| d);
        match profile_root {
            Some([kind, path, pool]) if kind == "disk" => {
                if !path.is_empty() {
                    root.insert("path".into(), J::from(path.clone()));
                }
                if !pool.is_empty() {
                    root.insert("pool".into(), J::from(pool.clone()));
                }
            }
            _ => {
                let pool = self.incus.default_storage_pool()?;
                root.insert("pool".into(), J::from(pool));
            }
        }
        let mut devices = Map::new();
        devices.insert("root".into(), J::Object(root.clone()));
        if !profile.iter().any(|(_, [kind, _, _])| kind == "nic") {
            let network = match self.incus.network_name.trim() {
                "" => "incusbr0",
                n => n,
            };
            devices.insert(
                "eth0".into(),
                json!({"type": "nic", "name": "eth0", "nictype": "bridged", "parent": network}),
            );
        }
        let payload = json!({
            "name": vm, "type": "virtual-machine", "profiles": ["default"],
            "source": image_source(&image), "config": config, "devices": devices,
        });
        let data = gojson::encode_to_string(&payload);
        let create = self.run(
            &[
                "query",
                "-X",
                "POST",
                "--wait",
                "/1.0/instances",
                "-d",
                &data,
            ],
            timeout,
        )?;
        if create.exit_code != 0 {
            return Err(format!(
                "incus create {}: {}",
                quote(vm),
                first_non_empty(&[&create.stderr, &create.stdout, "failed to create VM"])
            )
            .into());
        }
        // Profiles may give VMs a small root disk: apply the admitted size.
        if !disk.is_empty() {
            self.patch_root_disk(vm, &J::Object(root), timeout)?;
        }
        let state = format!("/1.0/instances/{}/state", escape(vm));
        let start = self.run(
            &[
                "query",
                "-X",
                "PUT",
                "--wait",
                &state,
                "-d",
                r#"{"action":"start","force":false,"stateful":false}"#,
            ],
            timeout,
        )?;
        if start.exit_code != 0 {
            return Err(format!(
                "incus start {}: {}",
                quote(vm),
                first_non_empty(&[&start.stderr, &start.stdout, "failed to start VM"])
            )
            .into());
        }
        Ok(())
    }

    fn scoped(&self, vm: &str, operation: &str) -> R<String> {
        let vm = vm.trim();
        if vm.is_empty() {
            return Err("vmName is required".into());
        }
        self.assert_ownership(vm, operation)?;
        Ok(vm.to_string())
    }

    /// One unbounded lifecycle command.
    fn lifecycle(&self, args: &[&str], fallback: &str) -> R<()> {
        self.must(args, Duration::ZERO, fallback).map(|_| ())
    }

    fn lifecycle_result(&self, vm: &str, status: &str) -> J {
        let mut out = Map::new();
        out.insert("vmName".into(), J::from(vm));
        out.insert("status".into(), J::from(status));
        let uri = self.uri_for(vm);
        if !uri.is_empty() {
            out.insert("uri".into(), J::from(uri));
        }
        J::Object(out)
    }

    /// `StartVM`.
    pub fn start_vm(&self, vm: &str) -> R<J> {
        let vm = self.scoped(vm, "start_vm")?;
        self.lifecycle(&["start", &vm], "failed to start VM")?;
        Ok(self.lifecycle_result(&vm, "running"))
    }

    /// `StopVM`.
    pub fn stop_vm(&self, vm: &str) -> R<J> {
        let vm = self.scoped(vm, "stop_vm")?;
        self.lifecycle(&["stop", &vm, "--force"], "failed to stop VM")?;
        Ok(self.lifecycle_result(&vm, "stopped"))
    }

    /// `RestartVM`.
    pub fn restart_vm(&self, vm: &str) -> R<J> {
        let vm = self.scoped(vm, "restart_vm")?;
        self.lifecycle(
            &["stop", &vm, "--force"],
            "failed to stop VM during restart",
        )?;
        self.lifecycle(&["start", &vm], "failed to start VM during restart")?;
        Ok(self.lifecycle_result(&vm, "running"))
    }

    /// `DeleteVM`: the registry entry goes with the instance.
    pub fn delete_vm(&self, vm: &str) -> R<J> {
        let vm = self.scoped(vm, "delete_vm")?;
        self.lifecycle(&["delete", &vm, "--force"], "failed to delete VM")?;
        let uri = self.uri_for(&vm);
        if !uri.is_empty() {
            let _ = crate::resource::deregister(&self.state, &self.tenant_id, &uri);
        }
        let mut out = Map::new();
        out.insert("vmName".into(), J::from(vm));
        out.insert("deleted".into(), J::Bool(true));
        if !uri.is_empty() {
            out.insert("uri".into(), J::from(uri));
        }
        Ok(J::Object(out))
    }

    /// `UpdateVMResources`: CPU and memory limits, and a grow-only root disk
    /// admitted on the instance's own pool.
    pub fn update_vm_resources(&self, vm: &str, cpus: i64, memory: &str, disk: &str) -> R<J> {
        let vm = vm.trim();
        if vm.is_empty() {
            return Err("vmName is required".into());
        }
        let (memory, disk) = (memory.trim(), disk.trim());
        if cpus <= 0 && memory.is_empty() && disk.is_empty() {
            return Err("at least one of cpus, memory, or disk is required".into());
        }
        self.assert_ownership(vm, "update_vm_resources")?;
        let mut applied = Map::new();
        applied.insert("vmName".into(), J::from(vm));
        applied.insert("status".into(), J::from("updated"));
        let uri = self.uri_for(vm);
        if !uri.is_empty() {
            applied.insert("uri".into(), J::from(uri));
        }
        if cpus > 0 {
            self.set_config(vm, "limits.cpu", &cpus.to_string())
                .map_err(|e| e.wrap("set CPU limit: "))?;
            applied.insert("cpus".into(), J::from(cpus.to_string()));
        }
        if !memory.is_empty() {
            self.set_config(vm, "limits.memory", memory)
                .map_err(|e| e.wrap("set memory limit: "))?;
            applied.insert("memory".into(), J::from(memory));
        }
        if !disk.is_empty() {
            let quota = self.apply_root_disk(vm, disk)?;
            applied.insert("disk".into(), J::from(quota.size));
            applied.insert("enforced".into(), J::from(quota.enforced.to_string()));
            applied.insert("pool".into(), J::from(quota.pool));
            applied.insert("driver".into(), J::from(quota.driver));
            if !quota.reason.is_empty() {
                applied.insert("reason".into(), J::from(quota.reason));
            }
        }
        Ok(J::Object(applied))
    }
}

/// `refuseRootDiskShrink`.
fn refuse_shrink(requested: &str, current: &str) -> R<()> {
    let (requested, current) = (requested.trim(), current.trim());
    if requested.is_empty() || current.is_empty() {
        return Ok(());
    }
    let want = crate::resource::parse_byte_capacity(requested)
        .map_err(|e| format!("root disk size {} is invalid: {e}", quote(requested)))?;
    let have = crate::resource::parse_byte_capacity(current)
        .map_err(|e| format!("observed root disk size {} is invalid: {e}", quote(current)))?;
    if want < have {
        return Err(format!(
            "root disk shrink from {current} to {requested} is refused; Incus root disks are grow-only"
        )
        .into());
    }
    Ok(())
}

// --- system containers (incus_container.go) ---------------------------------------------

const NETPLAN_TEMPLATE: &str = "set -e
umask 022
cat > {path} <<'NETPLAN'
network:
  version: 2
  ethernets:
    eth0:
      dhcp4: false
      addresses: [{address}/{prefix}]
      routes:
        - to: default
          via: {gateway}
      nameservers:
        addresses: [{gateway}]
NETPLAN
chmod 0600 {path}
netplan apply
";

const GPU_PROBE_SCRIPT: &str = r#"set -eu
export LD_LIBRARY_PATH=/usr/lib/wsl/lib:${LD_LIBRARY_PATH:-}
test -e /dev/dxg && echo dxg=present || echo dxg=missing
test -e /usr/lib/wsl/lib/libcuda.so.1 && echo libcuda=present || echo libcuda=missing
test -e /dev/dri/renderD128 && echo dri_render=present || echo dri_render=missing
if command -v nvidia-smi >/dev/null 2>&1; then
  if nvidia-smi --query-gpu=name,memory.total --format=csv,noheader 2>/dev/null; then
    echo nvidia_smi=ok
  else
    echo nvidia_smi=nvml_failed
  fi
elif [ -x /usr/local/bin/nvidia-smi ]; then
  if /usr/local/bin/nvidia-smi --query-gpu=name,memory.total --format=csv,noheader 2>/dev/null; then
    echo nvidia_smi=ok
  else
    echo nvidia_smi=nvml_failed
  fi
else
  echo nvidia_smi=missing
fi
if command -v python3 >/dev/null 2>&1; then
  if python3 - <<'PY'
import ctypes
import os
import sys
if not os.path.exists("/dev/dxg"):
    sys.exit("dxg missing")
lib = ctypes.CDLL("/usr/lib/wsl/lib/libcuda.so.1")
ret = lib.cuInit(0)
if ret != 0:
    print(f"cuda_init_code={ret}")
    sys.exit(f"cuInit returned {ret}")
print("cuda_init=ok")
PY
  then
    echo cuda_init=ok
  else
    echo cuda_init=failed
  fi
else
  echo cuda_init=python_missing
fi
"#;

/// `evaluateSystemContainerGPUProbe`.
fn evaluate_gpu_probe(stdout: &str) -> (bool, &'static str, Vec<&'static str>) {
    let has = |s: &str| stdout.contains(s);
    let (dxg, libcuda) = (has("dxg=present"), has("libcuda=present"));
    if dxg && libcuda && (has("cuda_init=ok") || has("nvidia_smi=ok")) {
        return (true, "ready_for_gpu_container", Vec::new());
    }
    if dxg
        && libcuda
        && has("cuda_init=failed")
        && (has("cuda_init_code=100") || has("cuInit returned 100"))
    {
        return (false, "wsl_gpu_pv_not_nestable_in_lxc", vec![
            "WSL GPU-PV exposes /dev/dxg inside Incus system containers but CUDA init returns no device (cuInit 100). Run GPU-backed workloads on the WSL host namespace instead of nested LXC.",
        ]);
    }
    if dxg && libcuda {
        return (false, "system_container_gpu_blocked", vec![
            "WSL GPU devices are visible but neither CUDA init nor NVML succeeded inside the system container",
        ]);
    }
    (
        false,
        "system_container_gpu_blocked",
        vec!["GPU devices visible in WSL host but system container GPU init did not succeed"],
    )
}

/// `parseGuestIPv4Address`: the address and prefix (default 24).
fn guest_address(output: &str) -> (String, String) {
    let fields: Vec<&str> = output.split_whitespace().collect();
    for (i, field) in fields.iter().enumerate() {
        if *field != "inet" || i + 1 >= fields.len() {
            continue;
        }
        return match fields[i + 1].split_once('/') {
            Some((head, tail)) => (head.into(), tail.into()),
            None => (fields[i + 1].into(), "24".into()),
        };
    }
    (String::new(), "24".into())
}

/// `parseGuestIPv4Gateway`.
fn guest_gateway(output: &str) -> String {
    let fields: Vec<&str> = output.split_whitespace().collect();
    fields
        .iter()
        .position(|f| *f == "via")
        .and_then(|i| fields.get(i + 1))
        .map(|g| g.to_string())
        .unwrap_or_default()
}

impl Instances {
    fn delete_args(name: &str) -> [&str; 3] {
        ["delete", name, "--force"]
    }

    /// `ProvisionContainer`: a persistent system container, reused when it
    /// already exists and is owned.
    pub fn provision_container(&self, args: &ContainerArgs) -> R<ContainerStatus> {
        let name = args.container_name.trim();
        if name.is_empty() {
            return Err("containerName is required".into());
        }
        if !cfg!(target_os = "linux") {
            return Err(format!(
                "provision_container is unsupported on {} host agents",
                std::env::consts::OS
            )
            .into());
        }
        if name.contains([' ', '\t', '\r', '\n']) {
            return Err("containerName may not contain whitespace".into());
        }
        let image = normalize_launch_image(args.image.trim());
        let nesting = args.nesting.unwrap_or(true);
        let existing = self
            .instance_exists(name)
            .map_err(|e| e.wrap("inspect existing Incus instance: "))?;
        if existing {
            self.assert_ownership(name, "provision_container")?;
            let observed = self
                .read_instance_type(name)
                .map_err(|e| e.wrap("validate existing Incus runtime kind: "))?;
            let actual = observed_instance_type(&observed, name)?;
            if actual != "container" {
                return Err(runtime_kind_error(
                    "incus_runtime_kind_mismatch",
                    name,
                    "container",
                    actual,
                    "Use the VM provisioning operation for a virtual-machine instance.",
                )
                .into());
            }
            // Pin before the start so this boot claims the address.
            let (deferred, _) = self.ensure_static_ipv4(name, &args.ipv4_address)?;
            let started = self.run(&["start", name], 2 * MINUTE);
            let refused = match &started {
                Err(_) => true,
                Ok(r) => {
                    r.exit_code != 0 && !output(&started).to_lowercase().contains("already running")
                }
            };
            if refused {
                return Err(format!(
                    "start existing container: {}",
                    failed(&started, "incus start failed").unwrap_or_default()
                )
                .into());
            }
            self.ensure_autostart(name)?;
            if nesting {
                self.ensure_runtime_devices(name)?;
            }
            if args.gpu {
                self.attach_gpu_devices(name, args.host_gpu_libs)?;
            }
            if args.port > 0 {
                self.ensure_http_proxy(name, args.port)?;
            }
            if !args.model_volume.is_empty() {
                self.attach_model_volume(name, &args.model_volume)?;
            }
            self.await_ipv4(name)?;
            if deferred {
                self.apply_guest_static_ipv4(name, &args.ipv4_address)?;
            }
            return Ok(self.container_status(name, &image, "running"));
        }
        let requested = args.disk.trim();
        let disk = if requested.is_empty() {
            DEFAULT_CONTAINER_ROOT_DISK
        } else {
            requested
        };
        let disk = self.admit_root_disk(disk, !requested.is_empty(), "")?.size;
        let cpus = if args.cpus <= 0 {
            DEFAULT_CPUS
        } else {
            args.cpus
        };
        let memory = match args.memory.trim() {
            "" => DEFAULT_MEMORY,
            m => m,
        };
        self.enforce_budget(name, cpus, memory, &disk)?;
        self.launch_container(name, &image, &disk, cpus, memory, nesting, 10 * MINUTE)?;
        let cleanup = || {
            let _ = self.run(&Self::delete_args(name), 2 * MINUTE);
        };
        // A fresh container is pinned as deliberately as a reused one.
        let (launched_pin, changed) = match self.ensure_static_ipv4(name, &args.ipv4_address) {
            Ok(pin) => pin,
            Err(e) => {
                cleanup();
                return Err(e);
            }
        };
        if changed {
            // The NIC address is claimed at boot, and this container booted.
            self.restart_if_running(name)?;
        }
        if args.gpu {
            if let Err(e) = self.attach_gpu_devices(name, args.host_gpu_libs) {
                cleanup();
                return Err(e);
            }
        }
        if !args.model_volume.is_empty() {
            if let Err(e) = self.attach_model_volume(name, &args.model_volume) {
                cleanup();
                return Err(e);
            }
        }
        if args.port > 0 {
            if let Err(e) = self.ensure_http_proxy(name, args.port) {
                cleanup();
                return Err(e);
            }
        }
        self.await_ipv4(name)?;
        if launched_pin {
            self.apply_guest_static_ipv4(name, &args.ipv4_address)?;
        }
        Ok(self.container_status(name, &image, "running"))
    }

    /// `containerStatusResult`.
    fn container_status(&self, name: &str, image: &str, status: &str) -> ContainerStatus {
        let mut result = ContainerStatus {
            uri: String::new(),
            container_name: name.into(),
            image: image.into(),
            status: status.into(),
            instance_type: "container".into(),
        };
        if let Ok(uri) = crate::resource::Uri::new("container", &self.tenant_id, name) {
            result.uri = uri.to_string();
            let mut coordinates = Map::new();
            coordinates.insert("providerInstanceName".into(), J::from(name));
            coordinates.insert("displayName".into(), J::from(name));
            coordinates.insert("instanceType".into(), J::from("container"));
            self.register(&result.uri, coordinates);
        }
        result
    }

    /// `awaitContainerIPv4`: a container is provisioned only once it holds
    /// an address.
    fn await_ipv4(&self, name: &str) -> R<()> {
        let deadline = Instant::now() + CONTAINER_ADDRESS_TIMEOUT;
        let last = loop {
            let observed = self.incus.instance_ipv4(name);
            if let Ok(ips) = &observed {
                if !crate::incus::normalize_ipv4(ips.clone()).is_empty() {
                    return Ok(());
                }
            }
            if Instant::now() > deadline {
                break observed.err();
            }
            std::thread::sleep(CONTAINER_ADDRESS_POLL);
        };
        Err(match last {
            Some(e) => format!(
                "container {} did not obtain an IPv4 address within 3m0s: {e}",
                quote(name)
            ),
            None => format!(
                "container {} did not obtain an IPv4 address within 3m0s",
                quote(name)
            ),
        }
        .into())
    }

    /// `launchIncusContainer`.
    #[allow(clippy::too_many_arguments)]
    fn launch_container(
        &self,
        name: &str,
        image: &str,
        disk: &str,
        cpus: i64,
        memory: &str,
        nesting: bool,
        timeout: Duration,
    ) -> R<()> {
        let cpus = if cpus <= 0 { DEFAULT_CPUS } else { cpus };
        let memory = if memory.trim().is_empty() {
            DEFAULT_MEMORY
        } else {
            memory
        };
        let mut launch: Vec<String> = ["launch", image, name, "--config", "boot.autostart=true"]
            .map(String::from)
            .to_vec();
        let owner = self.incus.instance_id.trim();
        if !owner.is_empty() {
            launch.extend(["--config".into(), format!("{OWNER_LABEL}={owner}")]);
            let agent = self.incus.agent_id.trim();
            if !agent.is_empty() {
                launch.extend(["--config".into(), format!("{AGENT_LABEL}={agent}")]);
            }
        }
        if nesting {
            launch.extend(["--config".into(), "security.nesting=true".into()]);
        }
        launch.extend(["--config".into(), format!("limits.cpu={cpus}")]);
        if !memory.is_empty() {
            launch.extend(["--config".into(), format!("limits.memory={memory}")]);
        }
        if !disk.is_empty() {
            launch.extend(["--device".into(), format!("root,size={disk}")]);
        }
        let timeout = if timeout.is_zero() {
            10 * MINUTE
        } else {
            timeout
        };
        let argv: Vec<&str> = launch.iter().map(String::as_str).collect();
        let launched = self.run(&argv, timeout);
        if let Some(detail) = match &launched {
            Err(e) => Some(first_non_empty(&[e]).to_string()),
            Ok(r) if r.exit_code != 0 => Some(
                first_non_empty(&[
                    &r.stderr,
                    &r.stdout,
                    &format!("incus exited {}", r.exit_code),
                ])
                .to_string(),
            ),
            Ok(_) => None,
        } {
            return Err(format!("launch container: {detail}").into());
        }
        if nesting {
            self.ensure_runtime_devices(name)?;
        }
        self.ensure_autostart(name)
    }

    /// `EnsureDevice`: add the device unless it is already configured.
    fn ensure_device(&self, instance: &str, device: &str, add: &[&str]) -> R<()> {
        self.assert_ownership(instance, "configure_instance_device")?;
        if let Ok(show) = self.run(
            &["config", "device", "show", instance],
            Duration::from_secs(30),
        ) {
            if show.exit_code == 0 && show.stdout.contains(&format!("{device}:")) {
                return Ok(());
            }
        }
        let added = self.run(add, 2 * MINUTE);
        if let Some(detail) = failed(&added, "device add failed") {
            let lower = detail.to_lowercase();
            if lower.contains("already exists") || lower.contains("already configured") {
                return Ok(());
            }
            return Err(detail.into());
        }
        Ok(())
    }

    /// `ensureContainerRuntimeDevices`: what nested workloads need.
    fn ensure_runtime_devices(&self, name: &str) -> R<()> {
        if exists("/dev/kmsg") {
            self.ensure_device(
                name,
                "kmsg",
                &[
                    "config",
                    "device",
                    "add",
                    name,
                    "kmsg",
                    "unix-char",
                    "source=/dev/kmsg",
                    "path=/dev/kmsg",
                ],
            )
            .map_err(|e| e.wrap("attach /dev/kmsg: "))?;
        }
        Ok(())
    }

    /// `attachContainerGPUDevices`.
    fn attach_gpu_devices(&self, name: &str, host_libs: bool) -> R<()> {
        if exists("/dev/dxg") {
            self.ensure_device(
                name,
                "dxg",
                &[
                    "config",
                    "device",
                    "add",
                    name,
                    "dxg",
                    "unix-char",
                    "source=/dev/dxg",
                    "path=/dev/dxg",
                ],
            )
            .map_err(|e| e.wrap("attach /dev/dxg: "))?;
        }
        if host_libs && exists(WSL_GPU_LIB_PATH) {
            let source = format!("source={WSL_GPU_LIB_PATH}");
            let path = format!("path={WSL_GPU_LIB_PATH}");
            self.ensure_device(
                name,
                "wsl-gpu-libs",
                &[
                    "config",
                    "device",
                    "add",
                    name,
                    "wsl-gpu-libs",
                    "disk",
                    &source,
                    &path,
                    "readonly=true",
                ],
            )
            .map_err(|e| e.wrap("attach WSL GPU libraries: "))?;
        }
        for (host, guest) in [
            ("/usr/lib/wsl/lib/nvidia-smi", "/usr/local/bin/nvidia-smi"),
            ("/usr/lib/wsl/lib/nvidia-smi", "/usr/bin/nvidia-smi"),
        ] {
            if exists(host) {
                let (source, path) = (format!("source={host}"), format!("path={guest}"));
                let _ = self.ensure_device(
                    name,
                    "nvidia-smi-bind",
                    &[
                        "config",
                        "device",
                        "add",
                        name,
                        "nvidia-smi-bind",
                        "disk",
                        &source,
                        &path,
                        "readonly=true",
                    ],
                );
                break;
            }
        }
        self.ensure_device(
            name,
            "gpu",
            &[
                "config",
                "device",
                "add",
                name,
                "gpu",
                "gpu",
                "gputype=physical",
            ],
        )?;
        self.restart_if_running(name)
    }

    /// `attachContainerModelVolume`.
    fn attach_model_volume(&self, name: &str, volume: &str) -> R<()> {
        let volume = volume.trim();
        if volume.is_empty() {
            return Ok(());
        }
        let source = format!("source={volume}");
        self.ensure_device(
            name,
            "models",
            &[
                "config",
                "device",
                "add",
                name,
                "models",
                "disk",
                "pool=default",
                &source,
                "path=/models",
            ],
        )
    }

    /// `ensureContainerHTTPProxy`.
    fn ensure_http_proxy(&self, name: &str, port: i64) -> R<()> {
        const DEVICE: &str = "http-proxy";
        if let Ok(show) = self.run(&["config", "device", "show", name], Duration::from_secs(30)) {
            if show.exit_code == 0 && show.stdout.contains(&format!("{DEVICE}:")) {
                return Ok(());
            }
        }
        let proxy = format!("listen=tcp:0.0.0.0:{port},connect=tcp:127.0.0.1:{port}");
        let added = self.run(
            &["config", "device", "add", name, DEVICE, "proxy", &proxy],
            2 * MINUTE,
        );
        if let Some(detail) = failed(&added, "proxy add failed") {
            if detail.to_lowercase().contains("already exists") {
                return Ok(());
            }
            return Err(format!("attach HTTP proxy: {detail}").into());
        }
        Ok(())
    }

    /// `ensureContainerStaticIPv4`: (deferred to the guest, NIC changed). A
    /// host that does not own the bridge pins inside the guest instead.
    fn ensure_static_ipv4(&self, name: &str, address: &str) -> R<(bool, bool)> {
        let address = address.trim();
        if address.is_empty() {
            return Ok((false, false));
        }
        if address.parse::<std::net::Ipv4Addr>().is_err() {
            return Err(format!("ipv4Address {} is not an IPv4 address", quote(address)).into());
        }
        if let Ok(current) = self.run(
            &["config", "device", "get", name, "eth0", "ipv4.address"],
            Duration::from_secs(30),
        ) {
            if current.exit_code == 0 && current.stdout.trim() == address {
                return Ok((false, false));
            }
        }
        let unmanaged = |detail: &str| detail.to_lowercase().contains("unmanaged parent bridge");
        let pin = format!("ipv4.address={address}");
        let overridden = self.run(
            &["config", "device", "override", name, "eth0", &pin],
            2 * MINUTE,
        );
        let Some(detail) = failed(&overridden, "incus config device override failed") else {
            return Ok((false, true));
        };
        if unmanaged(&detail) {
            return Ok((true, false));
        }
        if !detail.to_lowercase().contains("already exists") {
            return Err(format!("pin container address: {detail}").into());
        }
        // The override exists from an earlier provision; move the address.
        let updated = self.run(
            &[
                "config",
                "device",
                "set",
                name,
                "eth0",
                "ipv4.address",
                address,
            ],
            2 * MINUTE,
        );
        let Some(detail) = failed(&updated, "incus config device set failed") else {
            return Ok((false, true));
        };
        if unmanaged(&detail) {
            return Ok((true, false));
        }
        Err(format!("pin container address: {detail}").into())
    }

    /// `readGuestIPv4Lease`: (address, prefix, gateway) of the guest's eth0.
    fn guest_lease(&self, name: &str) -> R<(String, String, String)> {
        let addr = self.run(
            &[
                "exec", name, "--", "ip", "-4", "-o", "addr", "show", "dev", "eth0",
            ],
            Duration::from_secs(30),
        );
        if let Some(detail) = failed(&addr, "ip addr show failed") {
            return Err(format!("read guest address of {}: {detail}", quote(name)).into());
        }
        let (address, prefix) = guest_address(&addr.expect("checked").stdout);
        let route = self.run(
            &["exec", name, "--", "ip", "-4", "route", "show", "default"],
            Duration::from_secs(30),
        );
        let gateway = match route {
            Ok(r) if r.exit_code == 0 => guest_gateway(&r.stdout),
            _ => String::new(),
        };
        Ok((address, prefix, gateway))
    }

    /// `guestHoldsStaticIPv4`.
    fn guest_holds_pin(&self, name: &str, address: &str) -> bool {
        match self.run(
            &["exec", name, "--", "cat", NETPLAN_PATH],
            Duration::from_secs(30),
        ) {
            Ok(r) if r.exit_code == 0 => r.stdout.contains(&format!("{address}/")),
            _ => false,
        }
    }

    /// `applyGuestStaticIPv4`: the prefix and gateway come from the lease
    /// the guest holds now.
    fn apply_guest_static_ipv4(&self, name: &str, address: &str) -> R<()> {
        let address = address.trim();
        if address.is_empty() {
            return Ok(());
        }
        let (observed, prefix, gateway) = self.guest_lease(name)?;
        if observed == address && self.guest_holds_pin(name, address) {
            return Ok(());
        }
        if gateway.is_empty() {
            return Err(format!(
                "pin {address} inside {}: the guest holds no default route to copy",
                quote(name)
            )
            .into());
        }
        let script = NETPLAN_TEMPLATE
            .replace("{path}", NETPLAN_PATH)
            .replace("{address}", address)
            .replace("{prefix}", &prefix)
            .replace("{gateway}", &gateway);
        let applied = self.run(&["exec", name, "--", "sh", "-c", &script], 2 * MINUTE);
        if let Some(detail) = failed(&applied, "netplan apply failed") {
            return Err(format!("pin {address} inside {}: {detail}", quote(name)).into());
        }
        let deadline = Instant::now() + CONTAINER_ADDRESS_TIMEOUT;
        loop {
            let held = self.guest_lease(name);
            if let Ok((held, _, _)) = &held {
                if held == address {
                    return Ok(());
                }
            }
            if Instant::now() > deadline {
                let held = held.map(|(h, _, _)| h).unwrap_or_default();
                return Err(format!(
                    "pin {address} inside {}: the guest still reports {}",
                    quote(name),
                    quote(&held)
                )
                .into());
            }
            std::thread::sleep(Duration::from_secs(2));
        }
    }

    /// `restartIncusInstanceIfRunning`: device changes apply on boot.
    fn restart_if_running(&self, name: &str) -> R<()> {
        let info = match self.run(&["info", name, "--format", "json"], Duration::from_secs(30)) {
            Ok(r) if r.exit_code == 0 => r,
            _ => return Ok(()),
        };
        if !info.stdout.contains(r#""status":"Running""#)
            && !info.stdout.contains(r#""status": "Running""#)
        {
            return Ok(());
        }
        let stopped = self.run(&["stop", name, "--force"], 2 * MINUTE);
        if let Some(detail) = failed(&stopped, "incus stop failed") {
            return Err(format!("stop container for device reload: {detail}").into());
        }
        let started = self.run(&["start", name], 2 * MINUTE);
        if let Some(detail) = failed(&started, "incus start failed") {
            return Err(format!("start container after device reload: {detail}").into());
        }
        Ok(())
    }

    /// `waitForIncusAgent`.
    fn wait_for_agent(&self, vm: &str, timeout: Duration) -> R<()> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if let Ok(r) = self.run(&["exec", vm, "--", "true"], Duration::from_secs(30)) {
                if r.exit_code == 0 {
                    return Ok(());
                }
            }
            std::thread::sleep(Duration::from_secs(2));
        }
        Err(format!("timed out waiting for Incus VM agent on {}", quote(vm)).into())
    }

    /// `WaitForVMExecReady`.
    pub(crate) fn wait_for_exec_ready(&self, vm: &str, timeout: Duration) -> Result<(), String> {
        self.wait_for_agent(vm, timeout).map_err(Fail::text)
    }

    /// `ReadInstanceType`, for the cluster agent.
    pub(crate) fn instance_type(&self, name: &str) -> Result<String, String> {
        self.read_instance_type(name).map_err(Fail::text)
    }

    /// `EnsureDevice`, for the cluster agent.
    pub(crate) fn ensure_instance_device(
        &self,
        instance: &str,
        device: &str,
        add: &[&str],
    ) -> Result<(), String> {
        self.ensure_device(instance, device, add)
            .map_err(Fail::text)
    }

    /// `ProbeGPUContainer`: a disposable container, probed and deleted.
    pub fn probe_gpu_container(&self) -> R<J> {
        if !cfg!(target_os = "linux") {
            return Err(format!(
                "probe_gpu_container is unsupported on {} host agents",
                std::env::consts::OS
            )
            .into());
        }
        let mut probe_args = Map::new();
        probe_args.insert("qemuRequired".into(), J::Bool(false));
        let host_tier = crate::tools::probe_incus_gpu(&self.env, &probe_args);
        let mut result = Map::new();
        result.insert("wslHost".into(), host_tier.clone());
        result.insert("status".into(), J::from("blocked"));
        if host_tier["status"].as_str() != Some("ready_for_host_probe") {
            result.insert("status".into(), J::from("blocked_host_gpu"));
            result.insert(
                "blockers".into(),
                json!(["WSL host GPU prerequisites are not ready"]),
            );
            return Ok(J::Object(result));
        }
        let unix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let probe = format!("opute-gpu-probe-{unix}");
        let outcome = self.probe_in(&probe, &mut result);
        let _ = self.run(&Self::delete_args(&probe), 2 * MINUTE);
        outcome.map(|_| J::Object(result))
    }

    fn probe_in(&self, probe: &str, result: &mut Map<String, J>) -> R<()> {
        let mut stop = |status: &str, error: String| {
            result.insert("status".into(), J::from(status));
            result.insert("error".into(), J::from(error));
        };
        let message = |e: Fail| match e {
            Fail::Text(text) => text,
            Fail::Admission(e) => e.to_string(),
        };
        if let Err(e) = self.launch_container(
            probe,
            "images:ubuntu/24.04",
            "4GiB",
            DEFAULT_CPUS,
            DEFAULT_MEMORY,
            false,
            10 * MINUTE,
        ) {
            stop("probe_launch_failed", message(e));
            return Ok(());
        }
        if let Err(e) = self.attach_gpu_devices(probe, true) {
            stop("gpu_attach_failed", message(e));
            return Ok(());
        }
        if let Err(e) = self.wait_for_agent(probe, 3 * MINUTE) {
            stop("probe_exec_unavailable", message(e));
            return Ok(());
        }
        let prep = self.run(
            &[
                "exec",
                probe,
                "--",
                "sh",
                "-lc",
                "command -v python3 >/dev/null 2>&1 || (export DEBIAN_FRONTEND=noninteractive && apt-get update -qq && apt-get install -y -qq python3)",
            ],
            3 * MINUTE,
        );
        if let Some(detail) = failed(&prep, "failed to prepare GPU probe container") {
            stop("probe_prepare_failed", detail);
            return Ok(());
        }
        let run = self.run(
            &["exec", probe, "--", "sh", "-lc", GPU_PROBE_SCRIPT],
            Duration::from_secs(90),
        );
        let (stdout, stderr, exit_code, error) = match &run {
            Ok(r) => (
                r.stdout.trim().to_string(),
                r.stderr.clone(),
                r.exit_code,
                None,
            ),
            Err(e) => (String::new(), String::new(), 0, Some(e.clone())),
        };
        result.insert(
            "systemContainer".into(),
            json!({"instanceName": probe, "guestProbe": stdout, "exitCode": exit_code}),
        );
        if error.is_some() || exit_code != 0 {
            result.insert("status".into(), J::from("guest_probe_failed"));
            let fallback = error.unwrap_or_else(|| "guest probe failed".into());
            result.insert(
                "guestProbeError".into(),
                J::from(first_non_empty(&[&stderr, &stdout, &fallback])),
            );
            return Ok(());
        }
        let (ok, status, blockers) = evaluate_gpu_probe(&stdout);
        result.insert("gpuAvailableInSystemContainer".into(), J::Bool(ok));
        result.insert("status".into(), J::from(status));
        if !blockers.is_empty() {
            result.insert("blockers".into(), json!(blockers));
        }
        Ok(())
    }
}

// --- standalone status (standalone.go) -------------------------------------------------

impl Instances {
    /// `CheckLocalPrerequisites`: the commands a local install needs, the
    /// provider's readiness and, when it is ready, whether a root disk size
    /// on the pool a new instance lands on is enforced.
    pub fn local_prerequisites(&self) -> J {
        let mut commands = Map::new();
        for name in ["incus", "bash", "curl", "base64"] {
            commands.insert(
                name.into(),
                J::Bool(crate::hostobs::look_path(&self.env, name).is_some()),
            );
        }
        let mut checks = Map::new();
        let ready = match self.run(&["version"], Duration::from_secs(10)) {
            Err(e) => {
                checks.insert("incus".into(), J::from(e));
                false
            }
            Ok(r) if r.exit_code == 0 => {
                checks.insert("incus".into(), J::from("ready"));
                true
            }
            Ok(r) => {
                checks.insert(
                    "incus".into(),
                    J::from(first_non_empty(&[
                        &r.stderr,
                        &r.stdout,
                        "incus check failed",
                    ])),
                );
                false
            }
        };
        let mut out = Map::new();
        out.insert("provider".into(), J::from("incus"));
        out.insert("providerBinary".into(), J::from(self.incus.binary.clone()));
        out.insert("providerReady".into(), J::Bool(ready));
        out.insert("commands".into(), J::Object(commands));
        if ready {
            match self.incus.root_disk_quota() {
                Ok(quota) => {
                    out.insert("rootDiskQuota".into(), quota);
                }
                Err(e) => {
                    checks.insert("rootDiskQuota".into(), J::from(e));
                }
            }
        }
        if !checks.is_empty() {
            out.insert("checks".into(), J::Object(checks));
        }
        J::Object(out)
    }

    /// `GetLocalStatus`: the fast inventory, its capacity and the host's
    /// system metadata with the admission snapshot.
    pub fn local_status(&self, register: &dyn Fn(&str, Map<String, J>)) -> R<J> {
        let listed = self.incus.list_vms(true, register)?;
        let capacity = self.incus.inventory_capacity()?;
        let paths = crate::hostobs::default_disk_paths(&self.env);
        let mut system = crate::hostobs::HostSystemStats::read(&paths)
            .metadata()
            .unwrap_or_default();
        system.insert("resourceAdmission".into(), self.coordinator.metadata());
        let vms = listed["vms"].clone();
        Ok(json!({
            "provider": "incus",
            "providerBinary": self.incus.binary,
            "vmCount": vms.as_array().map_or(0, Vec::len),
            "vms": vms,
            "capacity": capacity,
            "system": system,
            "checkedAt": crate::hostobs::rfc3339_now(),
        }))
    }
}

// --- stack reset (incus_stack.go) -------------------------------------------------------

/// `ResetIncusStackArgs`.
#[derive(Default)]
pub struct ResetArgs {
    pub instance_names: Vec<String>,
    pub instance_prefix: String,
    pub confirm: bool,
    pub reinstall: bool,
    pub dry_run: bool,
    pub disposable_host_fingerprint: String,
    pub expected_host_fingerprint: String,
    pub disposable_host_authorization: String,
}

/// `ResetIncusInventoryItem`.
#[derive(Clone, Default)]
struct Target {
    name: String,
    status: String,
    kind: String,
    owner: String,
}

impl Target {
    fn encode(&self, out: &mut String) {
        out.push_str("{\"name\":");
        gojson::encode_string(&self.name, out);
        out.push_str(",\"status\":");
        gojson::encode_string(&self.status, out);
        out.push_str(",\"type\":");
        gojson::encode_string(&self.kind, out);
        out.push_str(",\"owner\":");
        gojson::encode_string(&self.owner, out);
        out.push('}');
    }

    fn to_json(&self) -> J {
        json!({"name": self.name, "status": self.status, "type": self.kind, "owner": self.owner})
    }
}

/// `resetIncusCheckpoint`. Nil and empty slices are kept apart: Go writes
/// `null` for a nil one.
#[derive(Clone, Default)]
struct Checkpoint {
    version: String,
    instance_id: String,
    targets: Option<Vec<Target>>,
    deleted: Option<Vec<String>>,
    phase: String,
    reconcile: Option<Map<String, J>>,
    updated_at: String,
}

impl Checkpoint {
    /// `json.Marshal`, in struct field order.
    fn encode(&self) -> String {
        let mut out = String::from("{\"version\":");
        gojson::encode_string(&self.version, &mut out);
        out.push_str(",\"instanceId\":");
        gojson::encode_string(&self.instance_id, &mut out);
        out.push_str(",\"targets\":");
        match &self.targets {
            None => out.push_str("null"),
            Some(targets) => {
                out.push('[');
                for (i, t) in targets.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    t.encode(&mut out);
                }
                out.push(']');
            }
        }
        out.push_str(",\"deleted\":");
        match &self.deleted {
            None => out.push_str("null"),
            Some(deleted) => gojson::encode(&json!(deleted), &mut out),
        }
        out.push_str(",\"phase\":");
        gojson::encode_string(&self.phase, &mut out);
        if let Some(reconcile) = self.reconcile.as_ref().filter(|r| !r.is_empty()) {
            out.push_str(",\"reconcile\":");
            gojson::encode(&J::Object(reconcile.clone()), &mut out);
        }
        out.push_str(",\"updatedAt\":");
        gojson::encode_string(&self.updated_at, &mut out);
        out.push('}');
        out
    }

    fn to_json(&self) -> J {
        let mut out = Map::new();
        out.insert("version".into(), J::from(self.version.clone()));
        out.insert("instanceId".into(), J::from(self.instance_id.clone()));
        out.insert(
            "targets".into(),
            self.targets.as_ref().map_or(J::Null, |t| {
                J::Array(t.iter().map(Target::to_json).collect())
            }),
        );
        out.insert(
            "deleted".into(),
            self.deleted.as_ref().map_or(J::Null, |d| json!(d)),
        );
        out.insert("phase".into(), J::from(self.phase.clone()));
        if let Some(reconcile) = self.reconcile.as_ref().filter(|r| !r.is_empty()) {
            out.insert("reconcile".into(), J::Object(reconcile.clone()));
        }
        out.insert("updatedAt".into(), J::from(self.updated_at.clone()));
        J::Object(out)
    }

    /// `json.Unmarshal` into the checkpoint struct; `None` when it does not
    /// fit.
    fn decode(data: &[u8]) -> Option<Checkpoint> {
        let doc = gojson::parse(data).ok()?;
        match doc.value {
            Value::Null => return Some(Checkpoint::default()),
            Value::Object(_) => {}
            _ => return None,
        }
        let text = |n: &Node, k: &str| gojson::go_string(n.field(k)).ok();
        let targets = match doc.field("targets").map(|n| &n.value) {
            None | Some(Value::Null) => None,
            Some(Value::Array(items)) => {
                let mut out = Vec::new();
                for item in items {
                    out.push(match &item.value {
                        Value::Null => Target::default(),
                        Value::Object(_) => Target {
                            name: text(item, "name")?,
                            status: text(item, "status")?,
                            kind: text(item, "type")?,
                            owner: text(item, "owner")?,
                        },
                        _ => return None,
                    });
                }
                Some(out)
            }
            _ => return None,
        };
        let deleted = match doc.field("deleted").map(|n| &n.value) {
            None | Some(Value::Null) => None,
            Some(Value::Array(items)) => Some(
                items
                    .iter()
                    .map(|i| gojson::go_string(Some(i)).ok())
                    .collect::<Option<Vec<_>>>()?,
            ),
            _ => return None,
        };
        let reconcile = match doc.field("reconcile") {
            None => None,
            Some(n) if n.is_null() => None,
            Some(n) => match (&n.value, crate::transport::go_any(n)) {
                (Value::Object(_), J::Object(m)) => Some(m),
                _ => return None,
            },
        };
        Some(Checkpoint {
            version: text(&doc, "version")?,
            instance_id: text(&doc, "instanceId")?,
            targets,
            deleted,
            phase: text(&doc, "phase")?,
            reconcile,
            updated_at: text(&doc, "updatedAt")?,
        })
    }
}

impl Instances {
    fn checkpoint_path(&self) -> Option<&Path> {
        let path = self.reset_checkpoint_path.trim();
        (!path.is_empty()).then(|| Path::new(&self.reset_checkpoint_path))
    }

    /// `readResetCheckpoint`.
    fn read_checkpoint(&self) -> R<Option<Checkpoint>> {
        let Some(path) = self.checkpoint_path() else {
            return Ok(None);
        };
        let data = match std::fs::read(path) {
            Ok(data) => data,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(crate::goerr::path_error("open", path, &e).0.into()),
        };
        let checkpoint =
            Checkpoint::decode(&data).ok_or("incus reset checkpoint is invalid".to_string())?;
        if checkpoint.version != "incus-reset.v1"
            || checkpoint.instance_id != self.incus.instance_id
        {
            return Err("incus reset checkpoint owner or version mismatch".into());
        }
        Ok(Some(checkpoint))
    }

    /// `writeResetCheckpoint`: a copy is stamped and written; the caller's
    /// checkpoint is unchanged.
    fn write_checkpoint(&self, checkpoint: &Checkpoint) -> Result<(), String> {
        use std::os::unix::fs::OpenOptionsExt;
        let Some(path) = self.checkpoint_path() else {
            return Ok(());
        };
        let mut stamped = checkpoint.clone();
        stamped.version = "incus-reset.v1".into();
        stamped.instance_id = self.incus.instance_id.clone();
        stamped.updated_at = crate::hostobs::rfc3339_nano_now();
        let data = gojson::indent(&stamped.encode(), "  ");
        let dir = path.parent().unwrap_or(Path::new("."));
        crate::goerr::mkdir_all(dir, 0o700).map_err(|e| e.0)?;
        let temp = std::path::PathBuf::from(format!("{}.tmp", path.display()));
        let write = || -> std::io::Result<()> {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&temp)?;
            file.write_all(data.as_bytes())
        };
        write().map_err(|e| crate::goerr::path_error("open", &temp, &e).0)?;
        if let Err(e) = std::fs::rename(&temp, path) {
            let _ = std::fs::remove_file(&temp);
            return Err(format!(
                "rename {} {}: {}",
                temp.display(),
                path.display(),
                crate::goerr::errno_text(&e)
            ));
        }
        Ok(())
    }

    /// `verifyResetIncusStack`: the default pool, the owned bridge and the
    /// default-profile root disk.
    fn verify_reset(&self) -> R<J> {
        let storage = self.run(&["storage", "list", "--format", "json"], DISCOVERY_TIMEOUT);
        if let Some(detail) = failed(&storage, "verify Incus storage pools failed") {
            return Err(detail.into());
        }
        let pool_ready = storage
            .expect("checked")
            .stdout
            .contains(r#""name":"default""#);
        let network = self.run(&["network", "list", "--format", "json"], DISCOVERY_TIMEOUT);
        if let Some(detail) = failed(&network, "verify Incus networks failed") {
            return Err(detail.into());
        }
        let network_name = match self.incus.network_name.trim() {
            "" => "incusbr0",
            n => n,
        };
        let bridge_ready = network
            .expect("checked")
            .stdout
            .contains(&format!(r#""name":"{network_name}""#));
        let profile = self.run(&["profile", "device", "show", "default"], DISCOVERY_TIMEOUT);
        if let Some(detail) = failed(&profile, "verify Incus default profile failed") {
            return Err(detail.into());
        }
        let profile_ready = profile.expect("checked").stdout.contains("root:");
        if !(pool_ready && bridge_ready && profile_ready) {
            return Err(format!(
                "incus runtime verification failed after reset: default pool, {network_name}, or default-profile root disk missing"
            )
            .into());
        }
        Ok(json!({"poolReady": pool_ready, "bridgeReady": bridge_ready,
                  "profileReady": profile_ready, "verified": true}))
    }

    /// `validateResetIncusStack`.
    fn validate_reset(&self, args: &ResetArgs) -> R<()> {
        if !args.confirm {
            return Err("reset_incus_stack requires confirm=true".into());
        }
        if !args.reinstall {
            return Err("reset_incus_stack requires reinstall=true".into());
        }
        if self.incus.ownership_mode != "enforce" || self.incus.instance_id.trim().is_empty() {
            return Err(
                "reset_incus_stack requires enforced Incus ownership with a host-agent instance id"
                    .into(),
            );
        }
        if self.shared_host_owner_instance.trim().is_empty() {
            return Err("reset_incus_stack requires a configured shared host owner".into());
        }
        crate::hostfiles::require_shared_host_owner(
            &self.incus.instance_id,
            &self.shared_host_owner_instance,
            "reset_incus_stack",
        )?;
        let fingerprint = args.disposable_host_fingerprint.trim();
        let expected = args.expected_host_fingerprint.trim();
        if fingerprint.is_empty() || expected.is_empty() || fingerprint != expected {
            return Err("reset_incus_stack requires a matching disposable host fingerprint".into());
        }
        if args.disposable_host_authorization.trim() != format!("dispose:{fingerprint}") {
            return Err("reset_incus_stack requires explicit disposable-host authorization".into());
        }
        if args.instance_names.is_empty() && args.instance_prefix.trim().is_empty() {
            return Err(
                "reset_incus_stack requires explicit instanceNames or instancePrefix".into(),
            );
        }
        Ok(())
    }

    /// `ResetIncusStack`: delete the selected owned instances, reinstall the
    /// stack and verify it, resumably through the durable checkpoint.
    /// `reinstall` is the host's stack install with default settings.
    pub fn reset_stack(
        &self,
        args: &ResetArgs,
        ctx: &crate::plan::runner::RunCtx,
        reinstall: &dyn Fn() -> Result<J, String>,
    ) -> R<J> {
        self.validate_reset(args)?;
        let mut checkpoint = self.read_checkpoint()?;
        let res = self.run(&["list", "--format", "json"], DISCOVERY_TIMEOUT)?;
        if res.exit_code != 0 {
            return Err(first_non_empty(&[&res.stderr, &res.stdout, "incus list failed"]).into());
        }
        let items = crate::incus::decode_list(&res.stdout)
            .map_err(|_| "incus list returned invalid JSON".to_string())?;
        let prefix = args.instance_prefix.trim();
        let mut inventory: Vec<Target> = items
            .iter()
            .filter(|item| {
                args.instance_names.iter().any(|n| n.trim() == item.name)
                    || (!prefix.is_empty() && item.name.starts_with(prefix))
            })
            .map(|item| Target {
                name: item.name.clone(),
                status: item.status.clone(),
                kind: item.kind.clone(),
                owner: item.config_value(OWNER_LABEL),
            })
            .collect();
        let mut inventory_nil = false;
        if inventory.is_empty() {
            match &checkpoint {
                Some(done) if done.phase == "reconciled" => {
                    return Ok(json!({
                        "phase": done.phase, "dryRun": args.dry_run, "reinstall": true,
                        "resumable": true, "checkpoint": done.to_json(), "alreadyDone": true,
                    }));
                }
                None => return Err(
                    "reset_incus_stack inventory is empty; refusing to infer or broaden targets"
                        .into(),
                ),
                // Every target may already be deleted: resume from the
                // checkpoint rather than broaden the selection.
                Some(prior) => {
                    inventory_nil = prior.targets.is_none();
                    inventory = prior.targets.clone().unwrap_or_default();
                }
            }
        }
        for candidate in &inventory {
            if candidate.owner != self.incus.instance_id {
                return Err(format!(
                    "reset_incus_stack refuses unowned instance {} (owner {})",
                    quote(&candidate.name),
                    quote(first_non_empty(&[&candidate.owner, "unowned"]))
                )
                .into());
            }
        }
        let mut checkpoint = match checkpoint.take() {
            None => Checkpoint {
                targets: Some(inventory.clone()),
                phase: "validated".into(),
                deleted: Some(Vec::new()),
                ..Default::default()
            },
            Some(mut prior) => {
                if prior.targets.as_ref().is_none_or(|t| t.is_empty()) {
                    prior.targets = Some(inventory.clone());
                }
                prior
            }
        };
        self.write_checkpoint(&checkpoint)
            .map_err(|e| format!("write reset checkpoint: {e}"))?;
        let mut result = Map::new();
        result.insert("phase".into(), J::from("validated"));
        result.insert("dryRun".into(), J::Bool(args.dry_run));
        result.insert("reinstall".into(), J::Bool(true));
        result.insert(
            "inventory".into(),
            if inventory_nil {
                J::Null
            } else {
                J::Array(inventory.iter().map(Target::to_json).collect())
            },
        );
        result.insert("owner".into(), J::from(self.incus.instance_id.clone()));
        result.insert("resumable".into(), J::Bool(true));
        result.insert("nextPhase".into(), J::from("delete"));
        result.insert("evidenceMode".into(), J::from("redacted"));
        if args.dry_run {
            result.insert("checkpoint".into(), checkpoint.to_json());
            return Ok(J::Object(result));
        }
        // `revokeRelays`: relays into the guests stop before they are
        // deleted, those of the llm and postgres domains only if built.
        self.relays.stop_all();
        if let Some(llm) = self.llm.get() {
            llm.relay.stop_all();
        }
        if let Some(postgres) = self.postgres.get() {
            postgres.relay.stop_all();
        }
        let interrupted = |checkpoint: &mut Checkpoint| {
            checkpoint.phase = "interrupted".into();
            let _ = self.write_checkpoint(checkpoint);
        };
        for candidate in &inventory {
            if checkpoint
                .deleted
                .as_ref()
                .is_some_and(|d| d.contains(&candidate.name))
            {
                continue;
            }
            if let Some(e) = ctx.err() {
                interrupted(&mut checkpoint);
                return Err(e.into());
            }
            if let Err(e) = self.assert_ownership(&candidate.name, "reset_incus_stack pre-delete") {
                interrupted(&mut checkpoint);
                return Err(e);
            }
            checkpoint.phase = "deleting".into();
            self.write_checkpoint(&checkpoint)
                .map_err(|e| format!("write reset checkpoint before delete: {e}"))?;
            let deleted =
                self.incus
                    .run_ctx(&["delete", &candidate.name, "--force"], 5 * MINUTE, ctx);
            if let Some(detail) = failed(&deleted, "delete failed") {
                interrupted(&mut checkpoint);
                return Err(
                    format!("delete owned instance {}: {detail}", quote(&candidate.name)).into(),
                );
            }
            checkpoint
                .deleted
                .get_or_insert_with(Vec::new)
                .push(candidate.name.clone());
            self.write_checkpoint(&checkpoint)
                .map_err(|e| format!("write reset checkpoint after delete: {e}"))?;
        }
        if let Some(e) = ctx.err() {
            interrupted(&mut checkpoint);
            return Err(e.into());
        }
        checkpoint.phase = "reconciling".into();
        self.write_checkpoint(&checkpoint)
            .map_err(|e| format!("write reset checkpoint before reconcile: {e}"))?;
        let reconciled = match reinstall() {
            Ok(v) => v,
            Err(e) => {
                interrupted(&mut checkpoint);
                return Err(format!("reconcile Incus runtime after reset: {e}").into());
            }
        };
        let verified = match self.verify_reset() {
            Ok(v) => v,
            Err(e) => {
                interrupted(&mut checkpoint);
                return Err(e.wrap("verify Incus runtime after reset: "));
            }
        };
        checkpoint.phase = "reconciled".into();
        checkpoint.reconcile = reconciled.as_object().cloned();
        self.write_checkpoint(&checkpoint)
            .map_err(|e| format!("write reset checkpoint after reconcile: {e}"))?;
        result.insert("phase".into(), J::from("reconciled"));
        result.insert("nextPhase".into(), J::from("bootstrap"));
        result.insert("reconcileRequired".into(), J::Bool(false));
        result.insert("incus".into(), reconciled);
        result.insert("verify".into(), verified);
        result.insert("checkpoint".into(), checkpoint.to_json());
        Ok(J::Object(result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_image_matches_go() {
        assert_eq!(normalize_launch_image(""), "images:ubuntu/22.04");
        assert_eq!(
            normalize_launch_image("ubuntu:24.04"),
            "images:ubuntu/24.04"
        );
        assert_eq!(
            normalize_launch_image("ubuntu:debian/12"),
            "images:debian/12"
        );
        assert_eq!(normalize_launch_image("local:mine"), "local:mine");
        assert_eq!(normalize_launch_image("other:x"), "other:x");
        assert_eq!(normalize_launch_image(":x"), ":x");
    }

    #[test]
    fn runtime_kind_errors_omit_empty_fields() {
        assert_eq!(
            runtime_kind_error("incus_runtime_kind_invalid", "", "bogus", "", "r"),
            r#"{"code":"incus_runtime_kind_invalid","requested":"bogus","remediation":"r"}"#
        );
        assert!(parse_instance_type(" VM ").is_ok_and(|k| k == ("virtual-machine", true)));
        assert!(parse_instance_type("").is_ok_and(|k| k == ("container", false)));
    }

    #[test]
    fn shrink_is_refused() {
        assert!(refuse_shrink("10GiB", "20GiB").is_err());
        assert!(refuse_shrink("20GiB", "10GiB").is_ok());
        assert!(refuse_shrink("", "10GiB").is_ok());
    }

    #[test]
    fn checkpoint_round_trips_in_go_field_order() {
        let checkpoint = Checkpoint {
            version: "incus-reset.v1".into(),
            instance_id: "a".into(),
            targets: Some(vec![Target {
                name: "n".into(),
                ..Default::default()
            }]),
            deleted: Some(Vec::new()),
            phase: "validated".into(),
            reconcile: None,
            updated_at: "t".into(),
        };
        let text = gojson::indent(&checkpoint.encode(), "  ");
        assert_eq!(
            text,
            "{\n  \"version\": \"incus-reset.v1\",\n  \"instanceId\": \"a\",\n  \"targets\": [\n    {\n      \"name\": \"n\",\n      \"status\": \"\",\n      \"type\": \"\",\n      \"owner\": \"\"\n    }\n  ],\n  \"deleted\": [],\n  \"phase\": \"validated\",\n  \"updatedAt\": \"t\"\n}"
        );
        let back = Checkpoint::decode(text.as_bytes()).expect("decodes");
        assert_eq!(back.encode(), checkpoint.encode());
    }

    #[test]
    fn guest_lease_parsing() {
        assert_eq!(
            guest_address("2: eth0    inet 10.0.0.5/16 brd 10.0.255.255 scope global eth0"),
            ("10.0.0.5".into(), "16".into())
        );
        assert_eq!(guest_gateway("default via 10.0.0.1 dev eth0"), "10.0.0.1");
    }
}
