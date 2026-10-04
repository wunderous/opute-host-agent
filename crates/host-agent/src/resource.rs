//! Resource identity and the host resource service's read-only projection.
//!
//! `Uri` is Go's `resourceid.URI`: an opaque, tenant-scoped identity
//! `type:tenant:id`. The registry upsert is `state.Store.UpsertResource`.
//! `Coordinator` reports the admission snapshot `get_host_info` embeds
//! (`resource.Coordinator.Metadata`) and makes the admission decision
//! (`Coordinator.Admit`/`Release`): a reservation is a record in the
//! host-wide `reservations.json`, guarded by an exclusive `flock` on
//! `reservations.lock`, so co-resident agents (Go or Rust) share one budget.

use crate::hostobs::{HostSystemStats, PressureStall};
use serde_json::{json, Map, Value as J};
use std::collections::BTreeMap;
use std::path::PathBuf;

pub const POLICY_REVISION: &str = "opute-host-resource-policy.v2";

const KNOWN_TYPES: &[&str] = &[
    "vm",
    "container",
    "pod",
    "host",
    "cluster",
    "postgres-service",
    "sqlite-database",
    "database",
    "tunnel",
    "llm-runtime",
    "model",
    "host-service",
    "sql-connector",
    "oci-registry",
    "service-domain",
    "service",
    "network",
    "storage",
    "image",
    "profile",
    "cloudflared",
    "language",
    "embedding",
    "operation",
    "plan",
];

#[derive(Clone, Debug, PartialEq)]
pub struct Uri {
    pub resource_type: String,
    pub tenant_id: String,
    pub resource_id: String,
}

/// `^[a-z][a-z0-9-]{0,31}$`
fn segment_ok(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b.len() <= 32
        && b[0].is_ascii_lowercase()
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

impl Uri {
    /// `resourceid.New`.
    pub fn new(resource_type: &str, tenant_id: &str, resource_id: &str) -> Result<Uri, String> {
        let u = Uri {
            resource_type: resource_type.trim().to_string(),
            tenant_id: tenant_id.trim().to_string(),
            resource_id: resource_id.to_string(),
        };
        u.validate()?;
        Ok(u)
    }

    /// `resourceid.Parse`.
    pub fn parse(value: &str) -> Result<Uri, String> {
        let parts: Vec<&str> = value.trim().splitn(3, ':').collect();
        if parts.len() != 3 {
            return Err(
                "invalid resource URI: expected resource-type:tenant-id:resource-id".into(),
            );
        }
        let u = Uri {
            resource_type: parts[0].to_string(),
            tenant_id: parts[1].to_string(),
            resource_id: parts[2].to_string(),
        };
        u.validate()?;
        Ok(u)
    }

    fn validate(&self) -> Result<(), String> {
        if !KNOWN_TYPES.contains(&self.resource_type.as_str()) {
            return Err(format!(
                "unknown resource type: {}",
                crate::goerr::quote(&self.resource_type)
            ));
        }
        if !segment_ok(&self.resource_type) || !segment_ok(&self.tenant_id) {
            return Err(
                "invalid resource URI: type and tenant must match [a-z][a-z0-9-]{0,31}".into(),
            );
        }
        if self.resource_id.trim().is_empty() || self.resource_id.contains(['\r', '\n', '\t', ' '])
        {
            return Err("invalid resource URI: resource id is empty or contains whitespace".into());
        }
        Ok(())
    }
}

impl std::fmt::Display for Uri {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}:{}:{}",
            self.resource_type, self.tenant_id, self.resource_id
        )
    }
}

/// `Shared.RegisterResource`: tenant check, then the registry upsert. Errors
/// are ignored by every read-only caller, as in Go.
pub fn register(
    state: &std::sync::Mutex<crate::store::StateStore>,
    active_tenant: &str,
    uri: &str,
    coordinates: &Map<String, J>,
) -> Result<(), String> {
    let parsed = Uri::parse(uri)?;
    let tenant = active_tenant.trim();
    if !tenant.is_empty() && parsed.tenant_id != tenant {
        return Err(format!(
            "resource URI belongs to a different tenant: active tenant {}",
            crate::goerr::quote(tenant)
        ));
    }
    let mut encoded = String::new();
    crate::gojson::encode(&J::Object(coordinates.clone()), &mut encoded);
    let store = state
        .lock()
        .map_err(|_| "state store unavailable".to_string())?;
    store.upsert_resource(&parsed, &encoded)
}

// --- admission snapshot ---------------------------------------------------------------

/// The resource coordinator's configuration (`resource.Config`).
#[derive(Clone, Debug)]
pub struct Coordinator {
    pub lock_dir: PathBuf,
    pub disk_paths: Vec<String>,
    pub policy_revision: String,
    pub min_available_memory_bytes: i64,
    pub min_available_disk_bytes: i64,
    pub cpu_capacity_cores: f64,
    pub memory_capacity_bytes: i64,
    pub disk_capacity_bytes: i64,
    pub task_capacity: i64,
    /// The environment the host domain's workload-slice probe runs with.
    pub enforcement_env: crate::config::Env,
    pub max_normal: i64,
    pub max_heavy: i64,
    /// `FailClosedOnUnknown`.
    pub fail_closed: bool,
}

/// `ReservationTTL`: how long a crashed holder's record can outlive it.
pub const RESERVATION_TTL: std::time::Duration = std::time::Duration::from_secs(15 * 60);

/// `resource.AdmissionRequest`: an explicit, typed cost and ownership request.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AdmissionRequest {
    pub cpu_cores: f64,
    pub memory_bytes: i64,
    pub disk_bytes: i64,
    pub tasks: i64,
    pub class: String,
    pub operation: String,
    pub agent_id: String,
    pub operation_id: String,
    pub task_id: String,
    pub resource_uri: String,
}

impl AdmissionRequest {
    /// Go's JSON field order; `omitempty` fields dropped when zero.
    fn to_json(&self) -> J {
        let mut m = Map::new();
        if self.cpu_cores != 0.0 {
            m.insert(
                "cpuCores".into(),
                crate::gojson::float_value(self.cpu_cores),
            );
        }
        for (k, v) in [
            ("memoryBytes", self.memory_bytes),
            ("diskBytes", self.disk_bytes),
            ("tasks", self.tasks),
        ] {
            if v != 0 {
                m.insert(k.into(), J::from(v));
            }
        }
        m.insert("class".into(), J::from(self.class.clone()));
        m.insert("operation".into(), J::from(self.operation.clone()));
        m.insert("agentId".into(), J::from(self.agent_id.clone()));
        for (k, v) in [
            ("operationId", &self.operation_id),
            ("taskId", &self.task_id),
            ("resourceUri", &self.resource_uri),
        ] {
            if !v.is_empty() {
                m.insert(k.into(), J::from(v.clone()));
            }
        }
        J::Object(m)
    }

    fn from_json(v: &J) -> AdmissionRequest {
        let s = |k: &str| v.get(k).and_then(J::as_str).unwrap_or("").to_string();
        let i = |k: &str| v.get(k).and_then(J::as_i64).unwrap_or(0);
        AdmissionRequest {
            cpu_cores: v.get("cpuCores").and_then(J::as_f64).unwrap_or(0.0),
            memory_bytes: i("memoryBytes"),
            disk_bytes: i("diskBytes"),
            tasks: i("tasks"),
            class: s("class"),
            operation: s("operation"),
            agent_id: s("agentId"),
            operation_id: s("operationId"),
            task_id: s("taskId"),
            resource_uri: s("resourceUri"),
        }
    }
}

/// `DefaultCostForClass`.
pub fn default_cost_for_class(class: &str) -> AdmissionRequest {
    let (cpu_cores, memory_bytes, tasks) = match class {
        "heavy" => (2.0, 2 << 30, 8),
        "normal" => (0.25, 256 << 20, 1),
        _ => {
            return AdmissionRequest {
                class: "control".into(),
                ..Default::default()
            }
        }
    };
    AdmissionRequest {
        class: class.into(),
        cpu_cores,
        memory_bytes,
        tasks,
        ..Default::default()
    }
}

/// `ResolveArgumentCost`: descriptor-declared argument paths (cpuCores,
/// memoryBytes, diskBytes, tasks) override the static cost. A missing
/// argument keeps the default; a malformed one fails closed.
pub fn resolve_argument_cost(
    mut base: AdmissionRequest,
    args: &Map<String, J>,
    bindings: &[String; 4],
) -> Result<AdmissionRequest, AdmitError> {
    type Parse = fn(&J) -> Result<f64, String>;
    let rows: [(&str, Parse); 4] = [
        ("cpuCores", positive_number),
        ("memoryBytes", positive_capacity),
        ("diskBytes", positive_capacity),
        ("tasks", positive_integer),
    ];
    for (i, (field, parse)) in rows.into_iter().enumerate() {
        let path = bindings[i].trim();
        if path.is_empty() {
            continue;
        }
        let quoted = crate::goerr::quote(path);
        let malformed = path.starts_with('.') || path.ends_with('.');
        if malformed {
            let reason = format!("argument binding path {quoted} is not a non-empty object path");
            return Err(AdmitError::request(
                "host_resource_binding_invalid",
                field,
                &reason,
            ));
        }
        if path
            .split('.')
            .any(|s| s.is_empty() || s.contains(['[', ']', '/']))
        {
            let reason = format!("argument binding path {quoted} is not a supported object path");
            return Err(AdmitError::request(
                "host_resource_binding_invalid",
                field,
                &reason,
            ));
        }
        let Some(value) = argument_at_path(args, path) else {
            continue;
        };
        let parsed = parse(value).map_err(|e| {
            AdmitError::request(
                "host_resource_argument_invalid",
                field,
                &format!("argument {quoted}: {e}"),
            )
        })?;
        match i {
            0 => base.cpu_cores = parsed,
            1 => base.memory_bytes = parsed as i64,
            2 => base.disk_bytes = parsed as i64,
            _ => base.tasks = parsed as i64,
        }
    }
    Ok(base)
}

/// `argumentAtPath`: a dotted path through nested objects.
pub fn argument_at_path<'a>(args: &'a Map<String, J>, path: &str) -> Option<&'a J> {
    let mut segments = path.split('.');
    let mut current = args.get(segments.next()?)?;
    for segment in segments {
        current = current.as_object()?.get(segment)?;
    }
    Some(current)
}

const MAX_INT64: f64 = i64::MAX as f64;

fn number_value(value: &J) -> Result<f64, String> {
    let number = value.as_f64().ok_or("must be numeric")?;
    if !number.is_finite() {
        return Err("must be finite".into());
    }
    if number < 0.0 {
        return Err("value cannot be negative".into());
    }
    Ok(number)
}

fn positive_number(value: &J) -> Result<f64, String> {
    let number = number_value(value)?;
    if number <= 0.0 {
        return Err("value must be greater than zero".into());
    }
    Ok(number)
}

fn positive_integer(value: &J) -> Result<f64, String> {
    let number = positive_number(value)?;
    if number.trunc() != number {
        return Err("value must be an integer".into());
    }
    if number > MAX_INT64 {
        return Err("value exceeds the supported integer range".into());
    }
    Ok(number)
}

fn positive_capacity(value: &J) -> Result<f64, String> {
    let bytes = match value {
        J::String(s) => capacity_string_bytes(s)?,
        _ => {
            let parsed = number_value(value)
                .map_err(|_| "must be a capacity string or integer byte count")?;
            if parsed.trunc() != parsed {
                return Err("byte count must be an integer".into());
            }
            parsed
        }
    };
    if bytes <= 0.0 {
        return Err("value must be greater than zero".into());
    }
    if bytes.is_infinite() || bytes > MAX_INT64 || bytes.trunc() != bytes {
        return Err("capacity exceeds the supported byte range".into());
    }
    Ok(bytes)
}

fn capacity_string_bytes(value: &str) -> Result<f64, String> {
    const UNITS: &[(&str, f64)] = &[
        ("TIB", (1u64 << 40) as f64),
        ("TB", 1e12),
        ("TI", (1u64 << 40) as f64),
        ("T", (1u64 << 40) as f64),
        ("GIB", (1u64 << 30) as f64),
        ("GB", 1e9),
        ("GI", (1u64 << 30) as f64),
        ("G", (1u64 << 30) as f64),
        ("MIB", (1u64 << 20) as f64),
        ("MB", 1e6),
        ("MI", (1u64 << 20) as f64),
        ("M", (1u64 << 20) as f64),
        ("KIB", 1024.0),
        ("KB", 1e3),
        ("KI", 1024.0),
        ("K", 1024.0),
        ("B", 1.0),
    ];
    let normalized = value.trim().to_uppercase();
    if normalized.is_empty() {
        return Err("must not be empty".into());
    }
    let (number_text, multiplier) = UNITS
        .iter()
        .find_map(|(suffix, m)| normalized.strip_suffix(suffix).map(|n| (n.trim(), *m)))
        .unwrap_or((normalized.as_str(), 1.0));
    let number = crate::goerr::parse_float(number_text)
        .filter(|n| n.is_finite())
        .ok_or("must be a valid capacity")?;
    if number <= 0.0 {
        return Err("value must be greater than zero".into());
    }
    let bytes = number * multiplier;
    if bytes.is_infinite() || bytes > MAX_INT64 || bytes.trunc() != bytes {
        return Err("capacity must resolve to an integer number of bytes".into());
    }
    Ok(bytes)
}

/// The typed admission failures `tools.ErrorResult` renders with
/// `owner: "admission"`; `Other` is an untyped error (I/O, a corrupt file).
#[derive(Clone, Debug, PartialEq)]
pub enum AdmitError {
    /// `resource.RequestError`.
    Request {
        code: String,
        field: String,
        reason: String,
    },
    /// `resource.AdmissionError`: a retryable capacity refusal.
    Admission {
        code: String,
        class: String,
        pressure: String,
        reason: String,
        retry_after_ms: i64,
    },
    Other(String),
}

impl AdmitError {
    pub fn request(code: &str, field: &str, reason: &str) -> AdmitError {
        AdmitError::Request {
            code: code.into(),
            field: field.into(),
            reason: reason.into(),
        }
    }
}

impl std::fmt::Display for AdmitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AdmitError::Request { code, reason, .. } => write!(f, "{code}: {reason}"),
            AdmitError::Admission {
                code,
                class,
                pressure,
                reason,
                retry_after_ms,
            } => write!(
                f,
                "{code}: class={class} pressure={pressure} reason={reason} retryAfterMs={retry_after_ms}"
            ),
            AdmitError::Other(message) => f.write_str(message),
        }
    }
}

/// A durable ownership handle. `control` is the zero-cost lease that never
/// touches the reservation file.
#[derive(Clone, Debug)]
pub struct Reservation {
    pub id: String,
    pub request: AdmissionRequest,
}

static RESERVATION_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// `sameReservationOwner`: an empty identity on both sides matches.
fn same_owner(left: &AdmissionRequest, right: &AdmissionRequest) -> bool {
    left.agent_id == right.agent_id
        && left.task_id == right.task_id
        && left.operation_id == right.operation_id
}

type Records = Map<String, J>;

/// `removeExpired`: a lease whose `expiresAt` is not after now is gone.
fn remove_expired(records: &mut Records) -> bool {
    let now = std::time::SystemTime::now();
    let before = records.len();
    records.retain(|_, record| {
        record
            .get("expiresAt")
            .and_then(J::as_str)
            .and_then(crate::hostobs::parse_rfc3339)
            .is_none_or(|t| t > now)
    });
    records.len() != before
}

/// `PressureSnapshot`.
struct Pressure {
    pressure: &'static str,
    reason: &'static str,
    memory_available: i64,
    disk_available: i64,
    disk_pressure: String,
    checked_at: String,
    memory_events: Option<BTreeMap<String, i64>>,
    stalls: Option<BTreeMap<String, PressureStall>>,
    controllers: Vec<String>,
    tasks_current: i64,
    tasks_limit: i64,
    enforcement: String,
}

fn normalize_enforcement(value: &str) -> &'static str {
    match value.trim().to_lowercase().as_str() {
        "enforced" => "enforced",
        "unsupported" => "unsupported",
        "unknown" => "unknown",
        _ => "",
    }
}

/// The refusals `Coordinator.Admit` makes before it weighs capacity: heavy
/// work under critical pressure, then (fail closed) any non-control work
/// while workload cgroup enforcement is not verified.
fn pre_capacity_refusal(
    class: &str,
    pressure: &str,
    pressure_reason: &'static str,
    enforcement: &str,
    fail_closed: bool,
) -> Option<(&'static str, &'static str)> {
    if class == "heavy" && pressure == "critical" {
        return Some(("host_resource_pressure", pressure_reason));
    }
    if fail_closed && class != "control" && enforcement != "enforced" {
        return Some((
            "host_resource_enforcement_unknown",
            "workload cgroup enforcement is not verified",
        ));
    }
    None
}

/// The class slots in `fits`: heavy work runs alone, normal work up to
/// `max_normal` at a time and never beside heavy work; control is unlimited.
fn class_slot_free(
    class: &str,
    held: &[AdmissionRequest],
    max_normal: i64,
    max_heavy: i64,
) -> bool {
    let heavy = held.iter().filter(|r| r.class == "heavy").count() as i64;
    let normal = held.iter().filter(|r| r.class == "normal").count() as i64;
    match class {
        "heavy" => heavy < max_heavy && normal == 0,
        "normal" => heavy == 0 && normal < max_normal,
        _ => true,
    }
}

impl Coordinator {
    fn pressure(&self) -> Pressure {
        let stats = HostSystemStats::read(&self.disk_paths);
        let mut enforcement = stats.cgroup_enforcement.clone();
        let probed =
            normalize_enforcement(&crate::hostobs::observe_enforcement(&self.enforcement_env));
        if !probed.is_empty() {
            enforcement = probed.to_string();
        }
        let (mut pressure, mut reason) = ("normal", "");
        if stats.memory_pressure == "critical"
            || (self.min_available_memory_bytes > 0
                && stats.memory_available_bytes > 0
                && stats.memory_available_bytes < self.min_available_memory_bytes)
        {
            pressure = "critical";
            reason = "available memory is below the host admission threshold";
        } else if stats.memory_pressure == "warning" {
            pressure = "warning";
            reason = "available memory is low";
        }
        if stats.disk_pressure == "critical"
            || (self.min_available_disk_bytes > 0
                && stats.disk_available_bytes > 0
                && stats.disk_available_bytes < self.min_available_disk_bytes)
        {
            pressure = "critical";
            reason = "available disk is below the host admission threshold";
        } else if pressure != "critical" && stats.disk_pressure == "warning" {
            pressure = "warning";
            reason = "available disk is low";
        }
        Pressure {
            pressure,
            reason,
            memory_available: stats.memory_available_bytes,
            disk_available: stats.disk_available_bytes,
            disk_pressure: stats.disk_pressure.clone(),
            checked_at: crate::hostobs::rfc3339_now(),
            memory_events: stats.memory_events.clone(),
            stalls: stats.pressure_stalls.clone(),
            controllers: stats.cgroup_controllers.clone(),
            tasks_current: stats.tasks_current,
            tasks_limit: stats.tasks_limit,
            enforcement,
        }
    }

    /// `reservationLock.acquire`: blocks until the host-wide lock is held.
    fn lock(&self, exclusive: bool) -> Result<nix::fcntl::Flock<std::fs::File>, AdmitError> {
        use std::os::unix::fs::OpenOptionsExt;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(self.lock_dir.join("reservations.lock"))
            .map_err(|e| AdmitError::Other(e.to_string()))?;
        let arg = if exclusive {
            nix::fcntl::FlockArg::LockExclusive
        } else {
            nix::fcntl::FlockArg::LockShared
        };
        nix::fcntl::Flock::lock(file, arg)
            .map_err(|(_, errno)| AdmitError::Other(errno.desc().to_lowercase()))
    }

    fn read_records(&self) -> Result<Records, AdmitError> {
        let raw = match std::fs::read(self.lock_dir.join("reservations.json")) {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Records::new()),
            Err(e) => return Err(AdmitError::Other(e.to_string())),
        };
        if raw.is_empty() {
            return Ok(Records::new());
        }
        match serde_json::from_slice::<J>(&raw) {
            Ok(J::Object(records)) => Ok(records),
            Ok(J::Null) => Ok(Records::new()),
            Ok(_) | Err(_) => Err(AdmitError::Other(
                "read host resource reservations: invalid reservation file".into(),
            )),
        }
    }

    /// Temp file + rename, mode 0600; the file is removed once empty.
    fn write_records(&self, records: &Records) -> Result<(), AdmitError> {
        let path = self.lock_dir.join("reservations.json");
        if records.is_empty() {
            return match std::fs::remove_file(&path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                    Err(AdmitError::Other(e.to_string()))
                }
                _ => Ok(()),
            };
        }
        use std::os::unix::fs::OpenOptionsExt;
        let raw = serde_json::to_vec_pretty(&J::Object(records.clone()))
            .map_err(|e| AdmitError::Other(e.to_string()))?;
        let tmp = self.lock_dir.join("reservations.json.tmp");
        let write = || -> std::io::Result<()> {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .mode(0o600)
                .open(&tmp)?;
            file.write_all(&raw)?;
            std::fs::rename(&tmp, &path)
        };
        write().map_err(|e| AdmitError::Other(e.to_string()))
    }

    /// `effectiveLimits`: observed limits capped by the configured capacity.
    fn effective_limits(&self, stats: &HostSystemStats) -> (f64, i64, i64, i64) {
        let mut cpu = stats.cpu_quota_cores;
        if cpu <= 0.0 {
            cpu = stats.cpu_count as f64;
        }
        let mut memory = stats.memory_limit_bytes;
        if memory <= 0 {
            memory = stats.memory_total_bytes;
        }
        let mut disk = stats.disk_total_bytes;
        let mut tasks = stats.tasks_limit;
        if self.cpu_capacity_cores > 0.0 && (cpu <= 0.0 || self.cpu_capacity_cores < cpu) {
            cpu = self.cpu_capacity_cores;
        }
        if self.memory_capacity_bytes > 0 && (memory <= 0 || self.memory_capacity_bytes < memory) {
            memory = self.memory_capacity_bytes;
        }
        if self.disk_capacity_bytes > 0 && (disk <= 0 || self.disk_capacity_bytes < disk) {
            disk = self.disk_capacity_bytes;
        }
        if self.task_capacity > 0 && (tasks <= 0 || self.task_capacity < tasks) {
            tasks = self.task_capacity;
        }
        (cpu, memory, disk, tasks)
    }

    /// `fits`. The server only ever reserves through `Admit`, so Go's
    /// in-process active counters are always zero there and the persisted
    /// records are the whole picture.
    fn fits(&self, request: &AdmissionRequest, records: &Records, pressure: &Pressure) -> bool {
        let requests: Vec<AdmissionRequest> = records
            .values()
            .map(|r| AdmissionRequest::from_json(r.get("request").unwrap_or(&J::Null)))
            .collect();
        if !class_slot_free(&request.class, &requests, self.max_normal, self.max_heavy) {
            return false;
        }
        let stats = HostSystemStats::read(&self.disk_paths);
        let (cpu, memory, disk, tasks) = self.effective_limits(&stats);
        let total_cpu: f64 = requests.iter().map(|r| r.cpu_cores).sum();
        let total = |f: fn(&AdmissionRequest) -> i64| requests.iter().map(f).sum::<i64>();
        if cpu > 0.0 && total_cpu + request.cpu_cores > cpu {
            return false;
        }
        if memory > 0 {
            let used = stats.memory_used_bytes.max(stats.memory_usage_bytes);
            let available = memory - used;
            if available < 0 || total(|r| r.memory_bytes) + request.memory_bytes > available {
                return false;
            }
        }
        if disk > 0 {
            let available = disk - (stats.disk_total_bytes - stats.disk_available_bytes);
            if available < 0 || total(|r| r.disk_bytes) + request.disk_bytes > available {
                return false;
            }
        }
        if tasks > 0 && total(|r| r.tasks) + request.tasks + stats.tasks_current > tasks {
            return false;
        }
        if self.min_available_memory_bytes > 0
            && pressure.memory_available > 0
            && pressure.memory_available < self.min_available_memory_bytes
        {
            return false;
        }
        if self.min_available_disk_bytes > 0
            && pressure.disk_available > 0
            && pressure.disk_available < self.min_available_disk_bytes
        {
            return false;
        }
        true
    }

    /// `Coordinator.Admit`: validate, then under the host lock drop expired
    /// leases, refuse on pressure, unverified enforcement or capacity, and
    /// persist the new reservation.
    pub fn admit(&self, mut request: AdmissionRequest) -> Result<Reservation, AdmitError> {
        if request.class.is_empty() {
            request.class = "normal".into();
        }
        for (field, negative) in [
            ("cpuCores", request.cpu_cores < 0.0),
            ("memoryBytes", request.memory_bytes < 0),
            ("diskBytes", request.disk_bytes < 0),
            ("tasks", request.tasks < 0),
        ] {
            if negative {
                return Err(AdmitError::request(
                    "host_resource_request_invalid",
                    field,
                    "resource cost cannot be negative",
                ));
            }
        }
        if !matches!(request.class.as_str(), "control" | "normal" | "heavy") {
            return Err(AdmitError::request(
                "host_resource_request_invalid",
                "class",
                "class must be control, normal, or heavy",
            ));
        }
        if request.class == "control"
            && request.cpu_cores == 0.0
            && request.memory_bytes == 0
            && request.disk_bytes == 0
            && request.tasks == 0
        {
            return Ok(Reservation {
                id: "control".into(),
                request,
            });
        }
        let _lock = self.lock(true)?;
        let mut records = self.read_records()?;
        let changed = remove_expired(&mut records);
        let pressure = self.pressure();
        let refuse = |records: &Records, code: &str, reason: &str| {
            if changed {
                let _ = self.write_records(records);
            }
            Err(AdmitError::Admission {
                code: code.into(),
                class: request.class.clone(),
                pressure: pressure.pressure.into(),
                reason: reason.into(),
                retry_after_ms: 1000,
            })
        };
        if let Some((code, reason)) = pre_capacity_refusal(
            &request.class,
            pressure.pressure,
            pressure.reason,
            &pressure.enforcement,
            self.fail_closed,
        ) {
            return refuse(&records, code, reason);
        }
        if !self.fits(&request, &records, &pressure) {
            return refuse(
                &records,
                "host_capacity_saturated",
                "declared resource cost exceeds effective host capacity",
            );
        }
        let now = std::time::SystemTime::now();
        let nanos = now
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let sequence = RESERVATION_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        let id = format!("host-reservation-{nanos}-{sequence}");
        records.insert(
            id.clone(),
            json!({
                "id": id,
                "request": request.to_json(),
                "createdAt": crate::hostobs::rfc3339_nano(now),
                "expiresAt": crate::hostobs::rfc3339_nano(now + RESERVATION_TTL),
            }),
        );
        self.write_records(&records)?;
        Ok(Reservation { id, request })
    }

    /// `Coordinator.Release`: drop the record if this owner still holds it.
    pub fn release(&self, reservation: &Reservation) -> Result<(), AdmitError> {
        if reservation.id.is_empty() || reservation.id == "control" {
            return Ok(());
        }
        let _lock = self.lock(true)?;
        let mut records = self.read_records()?;
        let Some(record) = records.get(&reservation.id) else {
            return Ok(());
        };
        let held = AdmissionRequest::from_json(record.get("request").unwrap_or(&J::Null));
        if !same_owner(&held, &reservation.request) {
            return Err(AdmitError::request(
                "host_reservation_owner_mismatch",
                "",
                "reservation ownership does not match the releasing operation",
            ));
        }
        records.remove(&reservation.id);
        self.write_records(&records)
    }

    /// `reservationTotals` over the live records, read under the shared
    /// lock with expired leases pruned, as `capacitySnapshot` does.
    fn reservation_totals(&self) -> J {
        let records = match self.lock(false) {
            Ok(_lock) => {
                let mut records = self.read_records().unwrap_or_default();
                if remove_expired(&mut records) {
                    let _ = self.write_records(&records);
                }
                records
            }
            Err(_) => Records::new(),
        };
        let (mut count, mut cpu, mut memory, mut disk, mut tasks) = (0i64, 0f64, 0i64, 0i64, 0i64);
        for record in records.values() {
            let request = record.get("request").cloned().unwrap_or(J::Null);
            count += 1;
            cpu += request.get("cpuCores").and_then(J::as_f64).unwrap_or(0.0);
            memory += request.get("memoryBytes").and_then(J::as_i64).unwrap_or(0);
            disk += request.get("diskBytes").and_then(J::as_i64).unwrap_or(0);
            tasks += request.get("tasks").and_then(J::as_i64).unwrap_or(0);
        }
        let mut totals = Map::new();
        totals.insert("count".into(), J::from(count));
        if cpu != 0.0 {
            totals.insert("cpuCores".into(), crate::gojson::float_value(cpu));
        }
        for (k, v) in [
            ("memoryBytes", memory),
            ("diskBytes", disk),
            ("tasks", tasks),
        ] {
            if v != 0 {
                totals.insert(k.into(), J::from(v));
            }
        }
        J::Object(totals)
    }

    /// `Coordinator.Snapshot`: `CapacitySnapshot` with the embedded
    /// `PressureSnapshot` flattened (its `enforcement` is shadowed by the
    /// outer field, as Go's encoder does). The in-flight and queue counters
    /// belong to Go's `Acquire` path, which the server never takes, so they
    /// are zero; held reservations show in `reservations`.
    pub fn snapshot(&self) -> J {
        let pressure = self.pressure();
        let reservations = self.reservation_totals();
        let stats = HostSystemStats::read(&self.disk_paths);
        let limits = |cpu: f64, memory: i64, disk: i64, tasks: i64| {
            let mut m = Map::new();
            if cpu != 0.0 {
                m.insert("cpuCores".into(), crate::gojson::float_value(cpu));
            }
            for (k, v) in [
                ("memoryBytes", memory),
                ("diskBytes", disk),
                ("tasks", tasks),
            ] {
                if v != 0 {
                    m.insert(k.into(), J::from(v));
                }
            }
            J::Object(m)
        };
        let observed = limits(
            stats.cpu_quota_cores,
            stats.memory_limit_bytes,
            stats.disk_total_bytes,
            stats.tasks_limit,
        );
        let (cpu, memory, disk, tasks) = self.effective_limits(&stats);
        let used = if stats.memory_used_bytes > stats.memory_usage_bytes {
            stats.memory_used_bytes
        } else {
            stats.memory_usage_bytes
        };
        let mut usage = Map::new();
        for (k, v) in [
            ("memoryBytes", used),
            ("memoryAvailableBytes", stats.memory_available_bytes),
            (
                "diskBytes",
                stats.disk_total_bytes - stats.disk_available_bytes,
            ),
            ("diskAvailableBytes", stats.disk_available_bytes),
            ("tasks", stats.tasks_current),
        ] {
            if v != 0 {
                usage.insert(k.into(), J::from(v));
            }
        }
        let enforcement = if pressure.enforcement.is_empty() {
            "unknown".to_string()
        } else {
            pressure.enforcement.clone()
        };
        let mut out = Map::new();
        out.insert("pressure".into(), J::from(pressure.pressure));
        if !pressure.reason.is_empty() {
            out.insert("reason".into(), J::from(pressure.reason));
        }
        if pressure.memory_available != 0 {
            out.insert(
                "memoryAvailableBytes".into(),
                J::from(pressure.memory_available),
            );
        }
        if pressure.disk_available != 0 {
            out.insert(
                "diskAvailableBytes".into(),
                J::from(pressure.disk_available),
            );
        }
        if !pressure.disk_pressure.is_empty() {
            out.insert(
                "diskPressure".into(),
                J::from(pressure.disk_pressure.clone()),
            );
        }
        out.insert("normalInFlight".into(), J::from(0));
        out.insert("heavyInFlight".into(), J::from(0));
        out.insert("queued".into(), J::from(0));
        out.insert("checkedAt".into(), J::from(pressure.checked_at.clone()));
        if let Some(events) = pressure.memory_events.as_ref().filter(|e| !e.is_empty()) {
            out.insert("memoryEvents".into(), json!(events));
        }
        if let Some(stalls) = pressure.stalls.as_ref().filter(|s| !s.is_empty()) {
            out.insert("psi".into(), crate::hostobs::stalls_json(stalls));
        }
        if !pressure.controllers.is_empty() {
            out.insert("cgroupControllers".into(), json!(pressure.controllers));
        }
        if pressure.tasks_current != 0 {
            out.insert("tasksCurrent".into(), J::from(pressure.tasks_current));
        }
        if pressure.tasks_limit != 0 {
            out.insert("tasksLimit".into(), J::from(pressure.tasks_limit));
        }
        out.insert(
            "policyRevision".into(),
            J::from(self.policy_revision.clone()),
        );
        out.insert("observedLimits".into(), observed);
        out.insert("effectiveLimits".into(), limits(cpu, memory, disk, tasks));
        out.insert("currentUsage".into(), J::Object(usage));
        out.insert("reservations".into(), reservations);
        out.insert(
            "queue".into(),
            json!({"queued": 0, "heavyQueued": 0, "normalActive": 0, "heavyActive": 0}),
        );
        out.insert("enforcement".into(), J::from(enforcement));
        J::Object(out)
    }

    /// `Coordinator.Metadata`: the subset `get_host_info` and heartbeats carry.
    pub fn metadata(&self) -> J {
        let snapshot = self.snapshot();
        let get = |k: &str| snapshot.get(k).cloned().unwrap_or(J::Null);
        let zero_str = |k: &str| snapshot.get(k).cloned().unwrap_or(J::from(""));
        let zero_int = |k: &str| snapshot.get(k).cloned().unwrap_or(J::from(0));
        json!({
            "policyRevision": get("policyRevision"),
            "enforcement": get("enforcement"),
            "pressure": get("pressure"),
            "reason": zero_str("reason"),
            "memoryAvailableBytes": zero_int("memoryAvailableBytes"),
            "diskAvailableBytes": zero_int("diskAvailableBytes"),
            "diskPressure": zero_str("diskPressure"),
            "normalInFlight": get("normalInFlight"),
            "heavyInFlight": get("heavyInFlight"),
            "queued": get("queued"),
            "checkedAt": get("checkedAt"),
            "effectiveLimits": get("effectiveLimits"),
            "currentUsage": get("currentUsage"),
            "reservations": get("reservations"),
            "queue": get("queue"),
            "psi": get("psi"),
            "memoryEvents": get("memoryEvents"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn held(classes: &[&str]) -> Vec<AdmissionRequest> {
        classes
            .iter()
            .map(|c| AdmissionRequest {
                class: c.to_string(),
                ..Default::default()
            })
            .collect()
    }

    #[test]
    fn class_slots_match_go_fits() {
        // Normal work shares up to max_normal slots, never beside heavy work.
        assert!(class_slot_free("normal", &held(&["normal"]), 2, 1));
        assert!(!class_slot_free(
            "normal",
            &held(&["normal", "normal"]),
            2,
            1
        ));
        assert!(!class_slot_free("normal", &held(&["heavy"]), 2, 1));
        // Heavy work runs alone.
        assert!(class_slot_free("heavy", &held(&[]), 2, 1));
        assert!(!class_slot_free("heavy", &held(&["normal"]), 2, 1));
        assert!(!class_slot_free("heavy", &held(&["heavy"]), 2, 1));
        assert!(class_slot_free("heavy", &held(&["heavy"]), 2, 2));
        // A zero limit refuses the class outright; control is never slotted.
        assert!(!class_slot_free("normal", &held(&[]), 0, 1));
        assert!(class_slot_free(
            "control",
            &held(&["heavy", "normal"]),
            0,
            0
        ));
    }

    #[test]
    fn unverified_enforcement_fails_closed() {
        let reason = "available memory is below the host admission threshold";
        for enforcement in ["unknown", "unsupported", ""] {
            for class in ["normal", "heavy"] {
                assert_eq!(
                    pre_capacity_refusal(class, "normal", "", enforcement, true),
                    Some((
                        "host_resource_enforcement_unknown",
                        "workload cgroup enforcement is not verified"
                    )),
                    "{class} with enforcement {enforcement:?}"
                );
            }
            // Control work and advisory mode are not gated on enforcement.
            assert_eq!(
                pre_capacity_refusal("control", "normal", "", enforcement, true),
                None
            );
            assert_eq!(
                pre_capacity_refusal("normal", "normal", "", enforcement, false),
                None
            );
        }
        assert_eq!(
            pre_capacity_refusal("normal", "normal", "", "enforced", true),
            None
        );
        // Critical pressure refuses heavy work first, even when enforced;
        // normal work under pressure falls through to the capacity check.
        assert_eq!(
            pre_capacity_refusal("heavy", "critical", reason, "unknown", true),
            Some(("host_resource_pressure", reason))
        );
        assert_eq!(
            pre_capacity_refusal("normal", "critical", reason, "enforced", true),
            None
        );
    }

    fn coordinator(dir: &std::path::Path) -> Coordinator {
        Coordinator {
            lock_dir: dir.to_path_buf(),
            disk_paths: vec![],
            policy_revision: String::new(),
            min_available_memory_bytes: 0,
            min_available_disk_bytes: 0,
            cpu_capacity_cores: 0.0,
            memory_capacity_bytes: 0,
            disk_capacity_bytes: 0,
            task_capacity: 0,
            enforcement_env: crate::config::Env::from_pairs([("PATH", "/nonexistent")]),
            max_normal: 2,
            max_heavy: 1,
            fail_closed: false,
        }
    }

    fn lease(id: &str, task: &str, expires: &str) -> (String, J) {
        let request = AdmissionRequest {
            class: "normal".into(),
            operation: "probe_incus_gpu".into(),
            agent_id: "agent".into(),
            task_id: task.into(),
            ..Default::default()
        };
        (
            id.into(),
            json!({"id": id, "request": request.to_json(),
                   "createdAt": "2026-10-02T00:00:00Z", "expiresAt": expires}),
        )
    }

    #[test]
    fn release_checks_owner_and_expired_leases_are_pruned() {
        let dir = std::env::temp_dir().join(format!(
            "opute-coordinator-test-{}-{}",
            std::process::id(),
            RESERVATION_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let c = coordinator(&dir);
        let records: Records = [
            lease("live", "task-a", "2999-01-01T00:00:00Z"),
            lease("stale", "task-b", "2000-01-01T00:00:00Z"),
        ]
        .into_iter()
        .collect();
        c.write_records(&records).unwrap();
        // The snapshot's ledger prunes the crashed holder's expired lease.
        assert_eq!(c.reservation_totals()["count"], json!(1));
        assert!(!c.read_records().unwrap().contains_key("stale"));

        let (_, live) = lease("live", "task-a", "");
        let mut request = AdmissionRequest::from_json(&live["request"]);
        request.task_id = "task-other".into();
        let foreign = Reservation {
            id: "live".into(),
            request: request.clone(),
        };
        assert_eq!(
            c.release(&foreign),
            Err(AdmitError::request(
                "host_reservation_owner_mismatch",
                "",
                "reservation ownership does not match the releasing operation"
            ))
        );
        request.task_id = "task-a".into();
        let owner = Reservation {
            id: "live".into(),
            request,
        };
        c.release(&owner).unwrap();
        // Releasing twice, or the control lease, is a no-op; the empty
        // ledger removes its file.
        c.release(&owner).unwrap();
        c.release(&Reservation {
            id: "control".into(),
            request: AdmissionRequest::default(),
        })
        .unwrap();
        assert!(!dir.join("reservations.json").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn uris_follow_resourceid() {
        let u = Uri::parse("vm:local:my:vm").unwrap();
        assert_eq!(u.resource_id, "my:vm");
        assert_eq!(u.to_string(), "vm:local:my:vm");
        assert!(Uri::parse("vm:local").is_err());
        assert!(Uri::parse("nope:local:x").is_err());
        assert!(Uri::parse("vm:Local:x").is_err());
        assert!(Uri::parse("vm:local:a b").is_err());
        assert_eq!(
            Uri::new(" host ", "local", "h").unwrap().to_string(),
            "host:local:h"
        );
    }
}
