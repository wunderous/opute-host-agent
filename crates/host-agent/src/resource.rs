//! Resource identity and the host resource service's read-only projection.
//!
//! `Uri` is Go's `resourceid.URI`: an opaque, tenant-scoped identity
//! `type:tenant:id`. The registry upsert is `state.Store.UpsertResource`.
//! `Coordinator` reports the admission snapshot `get_host_info` embeds
//! (`resource.Coordinator.Metadata`); reservation and admission decisions
//! themselves arrive with M4.

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

    /// Durable reservations (`reservations.json`); this build holds none of
    /// its own, but another co-resident agent's are counted.
    fn reservation_totals(&self) -> J {
        let path = self.lock_dir.join("reservations.json");
        let records: Map<String, J> = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<J>(&b).ok())
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default();
        let now = std::time::SystemTime::now();
        let (mut count, mut cpu, mut memory, mut disk, mut tasks) = (0i64, 0f64, 0i64, 0i64, 0i64);
        for record in records.values() {
            let expired = record
                .get("expiresAt")
                .and_then(J::as_str)
                .and_then(crate::hostobs::parse_rfc3339)
                .is_some_and(|t| t <= now);
            if expired {
                continue;
            }
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
    /// outer field, as Go's encoder does). This build holds no reservations,
    /// so the in-flight and queue counters are zero.
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
