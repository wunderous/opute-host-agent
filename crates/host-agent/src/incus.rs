//! Incus inventory: the read side of Go's `internal/domain/incus`.
//!
//! Every read goes through the provider runner (`Runtime.RunProvider`), which
//! reports a missing binary as `virtualization_stack_absent` before running
//! anything. `incus list --format json` is decoded with Go's typed semantics:
//! any field of the wrong type makes the whole list invalid.

use crate::config::Env;
use crate::gojson::{self, Node, Value};
use crate::hostobs::{look_path, run_command, CommandResult};
use serde_json::{json, Map, Value as J};
use std::collections::BTreeMap;
use std::time::Duration;

pub const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(45);
const OWNER_LABEL: &str = "user.opute.host_agent_instance";

/// What the incus domain needs from the agent's configuration.
#[derive(Clone, Debug)]
pub struct Incus {
    pub env: Env,
    pub binary: String,
    pub instance_id: String,
    pub ownership_mode: String,
    pub agent_id: String,
    pub tenant_id: String,
}

/// `Runtime.ProviderBinary`.
pub fn provider_binary(env: &Env) -> String {
    for key in ["OPUTE_INCUS_BINARY_PATH", "OPUTE_VM_BINARY_PATH"] {
        let v = env.get(key).trim().to_string();
        if !v.is_empty() {
            return v;
        }
    }
    for path in ["/usr/bin/incus", "/snap/bin/incus"] {
        if std::path::Path::new(path).exists() {
            return path.into();
        }
    }
    "incus".into()
}

/// `textutil.FirstNonEmpty` over trimmed values.
pub fn first_non_empty<'a>(values: &[&'a str]) -> &'a str {
    values
        .iter()
        .map(|v| v.trim())
        .find(|v| !v.is_empty())
        .unwrap_or("")
}

/// One `incusListItem`.
#[derive(Clone, Debug, Default)]
pub struct ListItem {
    pub name: String,
    pub status: String,
    pub kind: String,
    pub config: BTreeMap<String, String>,
    pub expanded_config: BTreeMap<String, String>,
    pub devices: Option<Map<String, J>>,
    pub expanded_devices: Option<Map<String, J>>,
    pub state: Option<Map<String, J>>,
}

fn string_map(node: Option<&Node>) -> Result<BTreeMap<String, String>, ()> {
    match node.map(|n| &n.value) {
        None | Some(Value::Null) => Ok(BTreeMap::new()),
        Some(Value::Object(members)) => {
            let mut out = BTreeMap::new();
            for (k, v) in members {
                let value = match &v.value {
                    Value::Null => out.get(k).cloned().unwrap_or_default(),
                    _ => gojson::go_string(Some(v))?,
                };
                out.insert(k.clone(), value);
            }
            Ok(out)
        }
        _ => Err(()),
    }
}

fn object_of_objects(node: Option<&Node>) -> Result<Option<Map<String, J>>, ()> {
    match node.map(|n| &n.value) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Object(members)) => {
            for (_, v) in members {
                if !matches!(v.value, Value::Object(_) | Value::Null) {
                    return Err(());
                }
            }
            match crate::transport::go_any(node.expect("present")) {
                J::Object(m) => Ok(Some(m)),
                _ => Ok(None),
            }
        }
        _ => Err(()),
    }
}

/// `json.Unmarshal(stdout, &[]incusListItem{})`.
pub fn decode_list(stdout: &str) -> Result<Vec<ListItem>, ()> {
    let doc = gojson::parse(stdout.as_bytes()).map_err(|_| ())?;
    let items = match &doc.value {
        Value::Null => return Ok(Vec::new()),
        Value::Array(items) => items,
        _ => return Err(()),
    };
    let mut out = Vec::new();
    for item in items {
        match &item.value {
            Value::Null => {
                out.push(ListItem::default());
                continue;
            }
            Value::Object(_) => {}
            _ => return Err(()),
        }
        let state = match item.field("state").map(|n| &n.value) {
            None | Some(Value::Null) => None,
            Some(Value::Object(_)) => {
                match crate::transport::go_any(item.field("state").unwrap()) {
                    J::Object(m) => Some(m),
                    _ => None,
                }
            }
            _ => return Err(()),
        };
        out.push(ListItem {
            name: gojson::go_string(item.field("name"))?,
            status: gojson::go_string(item.field("status"))?,
            kind: gojson::go_string(item.field("type"))?,
            config: string_map(item.field("config"))?,
            expanded_config: string_map(item.field("expanded_config"))?,
            devices: object_of_objects(item.field("devices"))?,
            expanded_devices: object_of_objects(item.field("expanded_devices"))?,
            state,
        });
    }
    Ok(out)
}

impl ListItem {
    /// `pickIncusConfigValue`.
    pub fn config_value(&self, key: &str) -> String {
        for map in [&self.config, &self.expanded_config] {
            if let Some(v) = map.get(key).map(|v| v.trim()).filter(|v| !v.is_empty()) {
                return v.to_string();
            }
        }
        String::new()
    }
}

fn is_virtual_machine(kind: &str) -> bool {
    matches!(
        kind.trim().to_lowercase().as_str(),
        "virtual-machine" | "virtual machine" | "container"
    )
}

/// `mapIncusInstanceType`.
pub fn instance_type(kind: &str) -> String {
    match kind.trim().to_lowercase().as_str() {
        "container" => "container".into(),
        "virtual-machine" | "virtual machine" | "" => "vm".into(),
        other => other.into(),
    }
}

/// `mapIncusStatus`.
pub fn status(s: &str) -> String {
    s.trim().to_lowercase()
}

impl Incus {
    /// `Runtime.RunProviderContext` (without streaming).
    pub fn run(&self, args: &[&str], timeout: Duration) -> Result<CommandResult, String> {
        let binary = self.binary.trim();
        if binary.is_empty() || look_path(&self.env, binary).is_none() {
            return Err(format!(
                "virtualization_stack_absent: the incus virtualization stack is not installed on this host; install it with {}",
                "install_incus_stack"
            ));
        }
        let mut argv = vec![binary.to_string()];
        argv.extend(args.iter().map(|s| s.to_string()));
        Ok(run_command(&self.env, &argv, timeout))
    }

    fn ownership_enforced(&self) -> bool {
        !self.instance_id.trim().is_empty() && self.ownership_mode == "enforce"
    }

    /// `ownedIncusItem`.
    fn owned(&self, item: &ListItem) -> bool {
        !self.ownership_enforced() || item.config_value(OWNER_LABEL) == self.instance_id
    }

    /// `listIncusVirtualMachines`.
    pub fn list(&self) -> Result<Vec<ListItem>, String> {
        let res = self.run(&["list", "--format", "json"], DISCOVERY_TIMEOUT)?;
        if res.exit_code != 0 {
            return Err(
                first_non_empty(&[&res.stderr, &res.stdout, "incus list failed"]).to_string(),
            );
        }
        let items =
            decode_list(&res.stdout).map_err(|_| "incus list returned invalid JSON".to_string())?;
        Ok(items
            .into_iter()
            .filter(|i| !i.name.is_empty() && is_virtual_machine(&i.kind) && self.owned(i))
            .collect())
    }

    /// `VMInventoryCapacity`.
    pub fn inventory_capacity(&self) -> Result<J, String> {
        let items = self.list()?;
        let mut c = [0i64; 12];
        // running/total: vm, cpu, memory, disk, qemu, container
        for item in &items {
            c[1] += 1;
            let running = item.status.eq_ignore_ascii_case("running");
            let qemu = instance_type(&item.kind).eq_ignore_ascii_case("vm");
            if qemu {
                c[9] += 1;
            } else {
                c[11] += 1;
            }
            if running {
                c[0] += 1;
                if qemu {
                    c[8] += 1;
                } else {
                    c[10] += 1;
                }
            }
            let cpus = cpu_count(item);
            if cpus > 0 {
                c[3] += cpus;
                if running {
                    c[2] += cpus;
                }
            }
            let memory = parse_capacity_bytes(&item.config_value("limits.memory"));
            if memory > 0 {
                c[5] += memory;
                if running {
                    c[4] += memory;
                }
            }
            let disk = parse_capacity_bytes(&disk_limit(item));
            if disk > 0 {
                c[7] += disk;
                if running {
                    c[6] += disk;
                }
            }
        }
        Ok(json!({
            "runningVmCount": c[0], "totalVmCount": c[1],
            "runningVmCpuLimitCores": c[2], "totalVmCpuLimitCores": c[3],
            "runningVmMemoryLimitBytes": c[4], "totalVmMemoryLimitBytes": c[5],
            "runningVmDiskLimitBytes": c[6], "totalVmDiskLimitBytes": c[7],
            "runningQemuCount": c[8], "totalQemuCount": c[9],
            "runningContainerCount": c[10], "totalContainerCount": c[11],
        }))
    }

    /// `assertIncusOwnership`.
    fn assert_ownership(&self, vm: &str, operation: &str) -> Result<(), String> {
        let vm = vm.trim();
        if vm.is_empty() || !self.ownership_enforced() {
            return Ok(());
        }
        let res = self.run(&["config", "get", vm, OWNER_LABEL], DISCOVERY_TIMEOUT)?;
        if res.exit_code != 0 {
            return Err(format!(
                "read Incus ownership for {}: {}",
                crate::goerr::quote(vm),
                first_non_empty(&[&res.stderr, &res.stdout, "incus config get failed"])
            ));
        }
        let owner = res.stdout.trim().to_string();
        if owner == self.instance_id {
            return Ok(());
        }
        let actual = if owner.is_empty() {
            "unowned-or-foreign".to_string()
        } else {
            owner
        };
        // IncusOwnershipMismatchError.Error: the JSON encoding of the struct.
        let mut text = String::from("{");
        for (i, (k, v)) in [
            ("code", "incus_ownership_mismatch"),
            ("vmName", vm),
            ("expectedInstance", self.instance_id.as_str()),
            ("actualOwner", actual.as_str()),
            ("operation", operation.trim()),
            (
                "remediation",
                "Select the owning host agent or use the approved adoption workflow.",
            ),
        ]
        .iter()
        .enumerate()
        {
            if i > 0 {
                text.push(',');
            }
            gojson::encode_string(k, &mut text);
            text.push(':');
            gojson::encode_string(v, &mut text);
        }
        text.push('}');
        Err(text)
    }

    /// `probeIncusAgent`.
    fn probe_agent(&self, vm: &str) -> bool {
        self.run(&["exec", vm, "--", "true"], Duration::from_secs(15))
            .is_ok_and(|r| r.exit_code == 0)
    }

    /// `readIncusInstanceIPv4`.
    fn instance_ipv4(&self, vm: &str) -> Result<Vec<String>, String> {
        self.assert_ownership(vm, "read_instance_state")?;
        let path = format!("/1.0/instances/{}/state", vm.replace('/', "%2F"));
        let res = self.run(&["query", &path], DISCOVERY_TIMEOUT)?;
        if res.exit_code != 0 {
            return Err(
                first_non_empty(&[&res.stderr, &res.stdout, "incus query failed"]).to_string(),
            );
        }
        let doc = gojson::parse(res.stdout.as_bytes()).map_err(|_| "invalid JSON".to_string())?;
        // json.Unmarshal into incusInstanceState: every present field must
        // have its declared type.
        let mut ips = Vec::new();
        let typed = |n: Option<&Node>, object: bool| -> Result<(), String> {
            match n.map(|n| &n.value) {
                None | Some(Value::Null) => Ok(()),
                Some(Value::Object(_)) if object => Ok(()),
                Some(Value::Array(_)) if !object => Ok(()),
                _ => Err("invalid JSON".into()),
            }
        };
        match &doc.value {
            Value::Object(_) | Value::Null => {}
            _ => return Err("invalid JSON".into()),
        }
        typed(doc.field("network"), true)?;
        if let Some(Value::Object(ifaces)) = doc.field("network").map(|n| &n.value) {
            for (_, iface) in ifaces {
                match &iface.value {
                    Value::Null => continue,
                    Value::Object(_) => {}
                    _ => return Err("invalid JSON".into()),
                }
                typed(iface.field("addresses"), false)?;
                let Some(Value::Array(addresses)) = iface.field("addresses").map(|n| &n.value)
                else {
                    continue;
                };
                for addr in addresses {
                    match &addr.value {
                        Value::Null => continue,
                        Value::Object(_) => {}
                        _ => return Err("invalid JSON".into()),
                    }
                    let text = |k: &str| {
                        gojson::go_string(addr.field(k)).map_err(|_| "invalid JSON".to_string())
                    };
                    let (address, family, scope) =
                        (text("address")?, text("family")?, text("scope")?);
                    if family == "inet" && scope == "global" && !address.is_empty() {
                        ips.push(address);
                    }
                }
            }
        }
        Ok(ips)
    }

    /// `readGuestCpuCount`.
    fn guest_cpus(&self, vm: &str) -> Option<i64> {
        let res = self
            .run(&["exec", vm, "--", "nproc"], Duration::from_secs(15))
            .ok()?;
        if res.exit_code != 0 {
            return None;
        }
        res.stdout.trim().parse::<i64>().ok().filter(|c| *c > 0)
    }

    /// `mapIncusListItem`; `register` records the instance's resource.
    fn map_item(&self, item: &ListItem, fast: bool, register: &dyn Fn(&str, Map<String, J>)) -> J {
        let status = status(&item.status);
        let agent_ready = (status == "running" && !fast).then(|| self.probe_agent(&item.name));
        let kind_type = instance_type(&item.kind);
        let mut info = Map::new();
        let resource_type = if kind_type.eq_ignore_ascii_case("vm") {
            "vm"
        } else {
            "container"
        };
        let uri = crate::resource::Uri::new(resource_type, &self.tenant_id, &item.name)
            .map(|u| u.to_string())
            .unwrap_or_default();
        let mut ipv4 = normalize_ipv4(extract_ipv4(item.state.as_ref()));
        let mut cpus = Some(cpu_count(item)).filter(|c| *c > 0);
        if !uri.is_empty() {
            let mut coordinates = Map::new();
            coordinates.insert("providerInstanceName".into(), J::from(item.name.clone()));
            coordinates.insert("displayName".into(), J::from(item.name.clone()));
            coordinates.insert("instanceType".into(), J::from(kind_type.clone()));
            register(&uri, coordinates);
        }
        if fast && ipv4.is_empty() && status == "running" {
            if let Ok(ips) = self.instance_ipv4(&item.name) {
                ipv4 = normalize_ipv4(ips);
            }
        }
        if !fast {
            if let Ok(ips) = self.instance_ipv4(&item.name) {
                ipv4 = normalize_ipv4(ips);
            }
            if cpus.is_none() && agent_ready == Some(true) {
                cpus = self.guest_cpus(&item.name);
            }
        }
        info.insert("uri".into(), J::from(uri));
        info.insert("kind".into(), J::from(resource_type));
        info.insert("name".into(), J::from(item.name.clone()));
        if !kind_type.is_empty() {
            info.insert("type".into(), J::from(kind_type));
        }
        info.insert("status".into(), J::from(status));
        info.insert("state".into(), json!({"incusStatus": item.status}));
        info.insert("ipv4".into(), json!(ipv4));
        let release = item.config_value("image.release");
        info.insert(
            "release".into(),
            J::from(if release.is_empty() {
                "unknown".into()
            } else {
                release
            }),
        );
        info.insert("providerId".into(), J::from("incus"));
        if let Some(c) = cpus {
            info.insert("cpus".into(), J::from(c));
        }
        let memory = extract_memory(item);
        if !memory.is_empty() {
            info.insert("memory".into(), J::from(memory));
        }
        let disk = extract_disk(item);
        if !disk.is_empty() {
            info.insert("disk".into(), J::from(disk));
        }
        if let Some(ready) = agent_ready {
            info.insert("agentReady".into(), J::Bool(ready));
        }
        let host_id = self.agent_id.trim();
        if !host_id.is_empty() {
            info.insert("hostId".into(), J::from(host_id));
        }
        J::Object(info)
    }

    /// `ListVMs`.
    pub fn list_vms(
        &self,
        fast: bool,
        register: &dyn Fn(&str, Map<String, J>),
    ) -> Result<J, String> {
        let items = self.list()?;
        let vms: Vec<J> = items
            .iter()
            .map(|i| self.map_item(i, fast, register))
            .collect();
        Ok(json!({"vms": vms}))
    }

    /// `GetVMInfo`.
    pub fn get_vm_info(
        &self,
        vm: &str,
        fast: bool,
        register: &dyn Fn(&str, Map<String, J>),
    ) -> Result<J, String> {
        let vm = vm.trim();
        if vm.is_empty() {
            return Err("vmName is required".into());
        }
        self.assert_ownership(vm, "get_vm_info")?;
        let items = self.list()?;
        match items.iter().find(|i| i.name == vm) {
            Some(item) => Ok(self.map_item(item, fast, register)),
            None => Err(format!("VM '{vm}' not found")),
        }
    }

    // --- root disk quota (incus_storage_quota.go) -----------------------------------

    fn query(&self, path: &str, fallback: &str) -> Result<String, String> {
        let res = self.run(&["query", path], DISCOVERY_TIMEOUT)?;
        if res.exit_code != 0 {
            return Err(first_non_empty(&[&res.stderr, &res.stdout, fallback]).to_string());
        }
        Ok(res.stdout)
    }

    /// `resolveRootDiskPool`: the default profile's root disk pool, else the
    /// default (or first) storage pool.
    fn root_disk_pool(&self) -> Result<String, String> {
        if let Ok(out) = self.query("/1.0/profiles/default", "incus profile query failed") {
            if let Ok(J::Object(profile)) = serde_json::from_str::<J>(&out) {
                let root = profile.get("devices").and_then(|d| d.get("root"));
                let field = |k: &str| {
                    root.and_then(|r| r.get(k))
                        .and_then(J::as_str)
                        .unwrap_or("")
                };
                if field("type") == "disk" && !field("pool").trim().is_empty() {
                    return Ok(field("pool").trim().to_string());
                }
            }
        }
        let out = self.query("/1.0/storage-pools", "storage pool query failed")?;
        let entries: Vec<String> = serde_json::from_str(&out).map_err(|e| go_json_error(&e))?;
        let names: Vec<String> = entries
            .iter()
            .map(|e| {
                e.strip_prefix("/1.0/storage-pools/")
                    .unwrap_or(e)
                    .trim_matches('/')
                    .to_string()
            })
            .filter(|n| !n.is_empty())
            .collect();
        if names.is_empty() {
            return Err("no storage pools configured".into());
        }
        Ok(if names.iter().any(|n| n == "default") {
            "default".into()
        } else {
            names[0].clone()
        })
    }

    /// `DescribeRootDiskQuotaSupport`.
    pub fn root_disk_quota(&self) -> Result<J, String> {
        let pool_name = self.root_disk_pool()?;
        let out = self.query(
            &format!("/1.0/storage-pools/{}", pool_name.replace('/', "%2F")),
            "storage pool query failed",
        )?;
        let payload: J = serde_json::from_str(&out).map_err(|e| go_json_error(&e))?;
        let text = |k: &str| payload.get(k).and_then(J::as_str).unwrap_or("").to_string();
        let name = first_non_empty(&[&text("name"), &pool_name]).to_string();
        let driver = text("driver");
        let source = payload
            .get("config")
            .and_then(|c| c.get("source"))
            .and_then(J::as_str)
            .unwrap_or("")
            .to_string();
        let mounts = std::fs::read_to_string("/proc/mounts").unwrap_or_default();
        let (enforced, reason) = pool_enforces_quota(&name, &driver, &source, &mounts);
        let mut out = Map::new();
        out.insert("pool".into(), J::from(name));
        out.insert("driver".into(), J::from(driver));
        out.insert("enforced".into(), J::Bool(enforced));
        if !reason.is_empty() {
            out.insert("reason".into(), J::from(reason));
        }
        Ok(J::Object(out))
    }
}

/// The text of a `json.Unmarshal` failure is not part of any result here;
/// callers only need to know it failed.
fn go_json_error(_e: &serde_json::Error) -> String {
    "invalid JSON".into()
}

/// `extractIPv4FromState`.
fn extract_ipv4(state: Option<&Map<String, J>>) -> Vec<String> {
    let Some(network) = state.and_then(|s| s.get("network")).and_then(J::as_object) else {
        return Vec::new();
    };
    let mut ips = Vec::new();
    for iface in network.values() {
        let Some(addresses) = iface.get("addresses").and_then(J::as_array) else {
            continue;
        };
        for addr in addresses {
            let text = |k: &str| addr.get(k).and_then(J::as_str).unwrap_or("");
            if text("family") == "inet" && text("scope") == "global" && !text("address").is_empty()
            {
                ips.push(text("address").to_string());
            }
        }
    }
    ips
}

/// `vminfo.NormalizeClusterIPv4`: dedupe, then cluster-reachable first.
pub fn normalize_ipv4(ips: Vec<String>) -> Vec<String> {
    let mut unique: Vec<String> = Vec::new();
    for ip in ips {
        let t = ip.trim().to_string();
        if !t.is_empty() && !unique.contains(&t) {
            unique.push(t);
        }
    }
    let score = |ip: &str| {
        let n = ip.trim().to_lowercase();
        match n.as_str() {
            "127.0.0.1" | "::1" | "localhost" => 100,
            _ if n.starts_with("10.42.") || n.starts_with("10.43.") || n.starts_with("fd42:") => 80,
            _ if n.starts_with("10.") || n.starts_with("192.168.") || n.starts_with("172.") => 40,
            _ => 0,
        }
    };
    unique.sort_by(|a, b| score(a).cmp(&score(b)).then_with(|| a.cmp(b)));
    unique
}

/// `normalizeIncusMemory`.
fn normalize_memory(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let lower = trimmed.to_lowercase();
    if lower.ends_with("gib") || lower.ends_with("mib") || lower.ends_with("tib") {
        return trimmed.to_string();
    }
    let gib_or_mib = |num: &str| -> Option<String> {
        let parsed = go_parse_float(num.trim())?;
        Some(if parsed == (parsed as i64) as f64 {
            format!("{}GiB", parsed as i64)
        } else {
            format!("{}MiB", (parsed * 1024.0) as i64)
        })
    };
    if lower.ends_with("gb") {
        if let Some(v) = gib_or_mib(&trimmed[..trimmed.len() - 2]) {
            return v;
        }
    }
    if lower.ends_with("mb") {
        if let Some(parsed) = go_parse_float(trimmed[..trimmed.len() - 2].trim()) {
            return format!("{}MiB", parsed as i64);
        }
    }
    if lower.ends_with('g') {
        if let Some(v) = gib_or_mib(&trimmed[..trimmed.len() - 1]) {
            return v;
        }
    }
    if lower.ends_with('m') {
        if let Some(parsed) = go_parse_float(trimmed[..trimmed.len() - 1].trim()) {
            return format!("{}MiB", parsed as i64);
        }
    }
    trimmed.to_string()
}

/// `formatIncusBytes` for a decoded JSON value.
fn format_value(v: Option<&J>) -> String {
    match v {
        Some(J::Number(n)) => n.as_f64().map(format_bytes).unwrap_or_default(),
        Some(J::String(s)) => go_parse_float(s.trim())
            .map(format_bytes)
            .unwrap_or_default(),
        _ => String::new(),
    }
}

/// `extractIncusMemory`.
fn extract_memory(item: &ListItem) -> String {
    let m = normalize_memory(&item.config_value("limits.memory"));
    if !m.is_empty() {
        return m;
    }
    format_value(
        item.state
            .as_ref()
            .and_then(|s| s.get("memory"))
            .and_then(J::as_object)
            .and_then(|m| m.get("usage")),
    )
}

/// `extractIncusDisk`.
fn extract_disk(item: &ListItem) -> String {
    let disk = item.config_value("limits.disk");
    if !disk.is_empty() && !disk.starts_with('-') && !disk.eq_ignore_ascii_case("0B") {
        return disk;
    }
    for devices in [item.devices.as_ref(), item.expanded_devices.as_ref()] {
        let size = root_device_size(devices);
        if !size.is_empty() {
            return size;
        }
    }
    format_value(
        item.state
            .as_ref()
            .and_then(|s| s.get("disk"))
            .and_then(J::as_object)
            .and_then(|d| d.get("root"))
            .and_then(J::as_object)
            .and_then(|r| r.get("usage")),
    )
}

fn cpu_count(item: &ListItem) -> i64 {
    item.config_value("limits.cpu")
        .trim()
        .parse::<i64>()
        .ok()
        .filter(|c| *c > 0)
        .unwrap_or(0)
}

/// `extractIncusRootDeviceSize`.
fn root_device_size(devices: Option<&Map<String, J>>) -> String {
    let Some(root) = devices.and_then(|d| d.get("root")).and_then(J::as_object) else {
        return String::new();
    };
    let kind = root.get("type").and_then(J::as_str).unwrap_or("");
    if !kind.trim().eq_ignore_ascii_case("disk") {
        return String::new();
    }
    match root.get("size") {
        Some(J::String(s)) => {
            let s = s.trim();
            if s.is_empty() || s.starts_with('-') || s.eq_ignore_ascii_case("0B") {
                String::new()
            } else {
                s.to_string()
            }
        }
        Some(J::Number(n)) => match n.as_f64() {
            Some(f) if f > 0.0 => format_bytes(f),
            _ => String::new(),
        },
        _ => String::new(),
    }
}

/// `pickIncusDiskLimit`.
fn disk_limit(item: &ListItem) -> String {
    let value = item.config_value("limits.disk");
    if !value.is_empty() && value != "0B" && value != "-1" {
        return value;
    }
    let size = root_device_size(item.devices.as_ref());
    if !size.is_empty() {
        return size;
    }
    root_device_size(item.expanded_devices.as_ref())
}

/// `parseCapacityBytes`.
pub fn parse_capacity_bytes(value: &str) -> i64 {
    let trimmed = value.trim().to_lowercase();
    if trimmed.is_empty() || trimmed == "max" {
        return 0;
    }
    let units: [(&str, f64); 17] = [
        ("tib", (1u64 << 40) as f64),
        ("tb", 1e12),
        ("ti", (1u64 << 40) as f64),
        ("t", (1u64 << 40) as f64),
        ("gib", (1u64 << 30) as f64),
        ("gb", 1e9),
        ("gi", (1u64 << 30) as f64),
        ("g", (1u64 << 30) as f64),
        ("mib", (1u64 << 20) as f64),
        ("mb", 1e6),
        ("mi", (1u64 << 20) as f64),
        ("m", (1u64 << 20) as f64),
        ("kib", 1024.0),
        ("kb", 1e3),
        ("ki", 1024.0),
        ("k", 1024.0),
        ("b", 1.0),
    ];
    let (mut number, mut factor) = (trimmed.as_str(), 1.0);
    for (suffix, f) in units {
        if let Some(n) = trimmed.strip_suffix(suffix) {
            number = n.trim();
            factor = f;
            break;
        }
    }
    match go_parse_float(number) {
        Some(parsed) if parsed > 0.0 && parsed * factor <= i64::MAX as f64 => {
            (parsed * factor) as i64
        }
        _ => 0,
    }
}

/// `strconv.ParseFloat` for decimal input (Rust also accepts "inf"/"nan",
/// which Go accepts too, case-insensitively).
fn go_parse_float(s: &str) -> Option<f64> {
    if s.is_empty() || s.contains('_') {
        return None;
    }
    s.parse::<f64>().ok()
}

/// `formatIncusBytes`.
pub fn format_bytes(bytes: f64) -> String {
    if bytes <= 0.0 {
        return String::new();
    }
    let (kib, mib) = (1024.0, 1024.0 * 1024.0);
    let gib = mib * 1024.0;
    if bytes >= gib {
        let as_gib = bytes / gib;
        if as_gib == (as_gib as i64) as f64 {
            return format!("{}GiB", as_gib as i64);
        }
        return format!("{}MiB", (bytes / mib) as i64);
    }
    if bytes >= mib {
        return format!("{}MiB", (bytes / mib) as i64);
    }
    let _ = kib;
    format!("{}B", bytes as i64)
}

// --- storage pool quota enforcement --------------------------------------------------

fn driver_enforces_quota(driver: &str) -> bool {
    matches!(
        driver.trim().to_lowercase().as_str(),
        "btrfs" | "zfs" | "lvm" | "lvmcluster" | "ceph" | "cephfs"
    )
}

struct MountEntry {
    mount_point: String,
    fs_type: String,
    options: String,
}

fn parse_mounts(data: &str) -> Vec<MountEntry> {
    let unescape = |f: &str| {
        f.replace("\\040", " ")
            .replace("\\011", "\t")
            .replace("\\012", "\n")
            .replace("\\134", "\\")
    };
    data.split('\n')
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            (fields.len() >= 4).then(|| MountEntry {
                mount_point: unescape(fields[1]),
                fs_type: fields[2].to_string(),
                options: fields[3].to_string(),
            })
        })
        .collect()
}

fn mount_for_path<'a>(entries: &'a [MountEntry], path: &str) -> Option<&'a MountEntry> {
    let path = path.trim();
    if path.is_empty() {
        return None;
    }
    let mut best: Option<&MountEntry> = None;
    let mut best_len: i64 = -1;
    for entry in entries {
        let mount = &entry.mount_point;
        if mount.is_empty() {
            continue;
        }
        if path != mount && !path.starts_with(&format!("{}/", mount.trim_end_matches('/'))) {
            continue;
        }
        if mount.len() as i64 >= best_len {
            best = Some(entry);
            best_len = mount.len() as i64;
        }
    }
    best
}

fn project_quota(fs_type: &str, options: &str) -> bool {
    matches!(fs_type.trim().to_lowercase().as_str(), "ext4" | "xfs")
        && options.split(',').any(|o| {
            matches!(
                o.trim().to_lowercase().as_str(),
                "prjquota" | "pquota" | "project"
            )
        })
}

/// `poolEnforcesQuota`.
fn pool_enforces_quota(name: &str, driver: &str, source: &str, mounts: &str) -> (bool, String) {
    let q = crate::goerr::quote;
    if driver_enforces_quota(driver) {
        return (true, String::new());
    }
    if driver.trim().eq_ignore_ascii_case("dir") {
        let source = source.trim();
        if source.is_empty() {
            return (false, format!("storage pool {} uses the dir driver and its source path is unknown, so project-quota support cannot be verified", q(name)));
        }
        let entries = parse_mounts(mounts);
        let Some(mount) = mount_for_path(&entries, source) else {
            return (false, format!("storage pool {} uses the dir driver and no mount was found for {}, so project-quota support cannot be verified", q(name), q(source)));
        };
        if project_quota(&mount.fs_type, &mount.options) {
            return (true, String::new());
        }
        return (false, format!("storage pool {} uses the dir driver on {} mounted at {} without project quotas (prjquota), so Incus accepts a root disk size but does not enforce it", q(name), mount.fs_type, q(&mount.mount_point)));
    }
    (
        false,
        format!(
            "storage pool {} uses the {} driver, which does not enforce root disk quotas",
            q(name),
            q(driver)
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capacity_bytes_match_go() {
        assert_eq!(parse_capacity_bytes("2GiB"), 2 << 30);
        assert_eq!(parse_capacity_bytes("1.5GB"), 1_500_000_000);
        assert_eq!(parse_capacity_bytes("512m"), 512 << 20);
        assert_eq!(parse_capacity_bytes("max"), 0);
        assert_eq!(parse_capacity_bytes("x"), 0);
        assert_eq!(format_bytes(2.0 * 1073741824.0), "2GiB");
        assert_eq!(format_bytes(1.5 * 1073741824.0), "1536MiB");
        assert_eq!(format_bytes(100.0), "100B");
    }

    #[test]
    fn quota_reasons() {
        let mounts = "/dev/sda1 / ext4 rw,relatime 0 0\n/dev/sdb /srv xfs rw,prjquota 0 0\n";
        assert!(pool_enforces_quota("p", "zfs", "", mounts).0);
        assert!(pool_enforces_quota("p", "dir", "/srv/pool", mounts).0);
        let (ok, reason) = pool_enforces_quota("p", "dir", "/var/lib/incus", mounts);
        assert!(!ok);
        assert!(reason.contains("without project quotas"));
    }

    #[test]
    fn list_decoding_is_typed() {
        let items = decode_list(
            r#"[{"name":"a","status":"Running","type":"container","config":{"limits.cpu":"2"}}]"#,
        )
        .unwrap();
        assert_eq!(cpu_count(&items[0]), 2);
        assert!(decode_list(r#"[{"name":1}]"#).is_err());
        assert!(decode_list(r#"[{"name":"a","config":{"k":1}}]"#).is_err());
        assert!(decode_list(r#"[{"name":"a","state":[]}]"#).is_err());
        assert!(decode_list("null").unwrap().is_empty());
    }
}
