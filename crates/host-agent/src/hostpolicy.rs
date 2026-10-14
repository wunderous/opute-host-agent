//! `reconcile_host_resource_policy` (`internal/resource/service.go:Reconcile`
//! and its host backend `internal/domain/host/resource_policy.go`): repair the
//! systemd projection of the installed resource policy for the exact Host
//! Agent unit a `host-service:` URI names. It writes the two managed slice
//! units, never accepts raw systemd arguments, and never restarts a service.

use std::collections::BTreeMap;
use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{json, Value as J};

use crate::config::Env;
use crate::hostobs::{parse_systemd_limit, parse_systemd_properties, run_command_with};
use crate::plan::runner::RunCtx;
use crate::resource::{Coordinator, Uri, POLICY_REVISION};

const PROTECTED_SLICE: &str = "opute-host-agent-protected.slice";
const WORKLOAD_SLICE: &str = "opute-workload.slice";
const AGENT_CPU_WEIGHT: &str = "1000";
const AGENT_TASKS_MAX: &str = "1024";
const SYSTEMCTL: &str = "/usr/bin/systemctl";
const SYSTEM_UNIT_DIR: &str = "/etc/systemd/system";

/// `resource.ReconcileError`, rendered as Go's `ErrorResult` renders it.
fn reconcile_error(code: &str, reason: &str) -> J {
    json!({
        "content": [{"type": "text", "text": format!("Error: {code}: {reason}")}],
        "structuredContent": {"code": code, "reason": reason, "owner": "admission"},
        "isError": true,
    })
}

/// `hostAgentInstancePattern`.
fn instance_id_valid(id: &str) -> bool {
    let b = id.as_bytes();
    !b.is_empty()
        && b.len() <= 63
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

/// `resourcePolicyTarget`: only the agent's own unit vocabulary.
fn policy_target(target: &Uri) -> Result<(String, String), String> {
    if target.resource_type != "host-service" {
        return Err("host resource target must be a host-service URI".into());
    }
    let Some((scope, name)) = target.resource_id.trim().split_once('/') else {
        return Err("host resource target must use <scope>/<exact-host-agent-unit>".into());
    };
    let scope = scope.trim().to_lowercase();
    if scope != "user" && scope != "system" {
        return Err("host resource target scope must be user or system".into());
    }
    let name = name.trim();
    let name = name.strip_suffix(".service").unwrap_or(name);
    if name == "opute-host-agent" {
        return Ok((scope, format!("{name}.service")));
    }
    if let Some(instance) = name.strip_prefix("opute-host-agent@") {
        if !instance_id_valid(instance) {
            return Err("host resource target has invalid Host Agent instance id".into());
        }
        return Ok((scope, format!("{name}.service")));
    }
    Err("host resource target must name the exact opute-host-agent service".into())
}

fn description(name: &str) -> String {
    match name {
        PROTECTED_SLICE => {
            format!("Opute protected host-agent control slice ({POLICY_REVISION})")
        }
        WORKLOAD_SLICE => format!("Opute killable workload slice ({POLICY_REVISION})"),
        _ => String::new(),
    }
}

/// `renderHostResourceSliceUnits`, in name order. (Go ranges over a map, so
/// it writes them in either order; name order is one of those.)
fn slice_units() -> [(&'static str, String); 2] {
    [
        (
            PROTECTED_SLICE,
            format!(
                "[Unit]\nDescription={}\n\n[Slice]\nCPUWeight=1000\nTasksMax=1024\n",
                description(PROTECTED_SLICE)
            ),
        ),
        (
            WORKLOAD_SLICE,
            format!(
                "[Unit]\nDescription={}\n\n[Slice]\nMemoryHigh=10G\nMemoryMax=11G\nMemorySwapMax=1G\nCPUQuota=600%\nCPUWeight=100\nTasksMax=4096\n",
                description(WORKLOAD_SLICE)
            ),
        ),
    ]
}

/// `managedResourceUnitNeedsMigration`: an older Opute-managed description
/// is rewritten; an operator-owned one is not.
fn needs_migration(name: &str, description_now: &str) -> bool {
    let current = description_now.trim();
    if current.is_empty() || current == description(name) {
        return false;
    }
    let prefixes: &[&str] = match name {
        PROTECTED_SLICE => &[
            "Protected Opute Host Agent control slice",
            "Opute protected host-agent control slice",
        ],
        WORKLOAD_SLICE => &[
            "Bounded Opute Host Agent workload slice",
            "Opute killable workload slice",
        ],
        _ => return false,
    };
    prefixes.iter().any(|prefix| {
        current == *prefix || current.starts_with(&format!("{prefix} (opute-host-resource-policy."))
    })
}

/// `parseSystemdUnitProperty`: CPUQuota as a percentage times 10 000.
fn unit_property(property: &str, value: &str) -> Option<i64> {
    if property == "CPUQuota" {
        let value = value.trim();
        let value = value.strip_suffix('%').unwrap_or(value).trim();
        let parsed: f64 = crate::goerr::parse_float(value)?;
        if parsed <= 0.0 || parsed > i64::MAX as f64 / 10_000.0 {
            return None;
        }
        return Some((parsed * 10_000.0) as i64);
    }
    parse_systemd_limit(value)
}

/// `preservesStricterResourceUnit`: an operator's stricter unit is kept.
fn preserves_stricter(name: &str, contents: &str) -> bool {
    let properties = parse_systemd_properties(contents);
    let get = |k: &str| properties.get(k).map(String::as_str).unwrap_or("");
    if needs_migration(name, get("Description")) {
        return false;
    }
    if name == PROTECTED_SLICE {
        if ["MemoryHigh", "MemoryMax", "MemorySwapMax"]
            .iter()
            .any(|p| properties.contains_key(*p))
        {
            return false;
        }
        let cpu = parse_systemd_limit(get("CPUWeight"));
        let tasks = parse_systemd_limit(get("TasksMax"));
        return cpu.is_some_and(|c| c >= 1000) && tasks.is_some_and(|t| t > 0 && t <= 1024);
    }
    if name != WORKLOAD_SLICE {
        return false;
    }
    [
        ("MemoryHigh", 10i64 << 30),
        ("MemoryMax", 11 << 30),
        ("MemorySwapMax", 1 << 30),
        ("CPUQuota", 6_000_000),
        ("CPUWeight", 100),
        ("TasksMax", 4096),
    ]
    .iter()
    .all(|(property, maximum)| {
        unit_property(property, get(property)).is_some_and(|v| v > 0 && v <= *maximum)
    })
}

/// `writeManagedUnit`: an atomic 0600 replacement.
fn write_unit(path: &str, contents: &str) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    let dir = Path::new(path)
        .parent()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| ".".into());
    let base = Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let base = crate::goerr::quote(&base);
    let (mut file, temporary) = crate::goerr::create_temp(&dir, ".opute-host-resource-", ".tmp")
        .map_err(|e| format!("create managed systemd unit {base}: {e}"))?;
    struct Remove<'a>(&'a str);
    impl Drop for Remove<'_> {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(self.0);
        }
    }
    let _cleanup = Remove(&temporary);
    let at = |op: &str, e: &io::Error| crate::goerr::path_error(op, Path::new(&temporary), e).0;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))
        .map_err(|e| format!("protect managed systemd unit {base}: {}", at("chmod", &e)))?;
    file.write_all(contents.as_bytes())
        .map_err(|e| format!("write managed systemd unit {base}: {}", at("write", &e)))?;
    drop(file);
    std::fs::rename(&temporary, path).map_err(|e| {
        format!(
            "install managed systemd unit {base}: rename {temporary} {path}: {}",
            crate::goerr::errno_text(&e)
        )
    })
}

/// `ensureResourceSliceUnits`: whether the workload slice was rewritten.
fn ensure_slice_units(scope: &str) -> Result<bool, String> {
    let dir = match scope {
        "user" => {
            let home = crate::hostread::home_dir()
                .map_err(|e| format!("resolve host resource policy home: {e}"))?;
            crate::hostread::clean(&format!("{home}/.config/systemd/user"))
        }
        _ => SYSTEM_UNIT_DIR.to_string(),
    };
    crate::goerr::mkdir_all(Path::new(&dir), 0o755)
        .map_err(|e| format!("create {scope} systemd unit directory: {}", e.0))?;
    let mut workload_updated = false;
    for (name, contents) in slice_units() {
        let path = format!("{dir}/{name}");
        match std::fs::read(&path) {
            Ok(existing) => {
                if preserves_stricter(name, &String::from_utf8_lossy(&existing)) {
                    continue;
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => {
                let op = if e.raw_os_error() == Some(nix::libc::EISDIR) {
                    "read"
                } else {
                    "open"
                };
                return Err(format!(
                    "inspect managed systemd unit {}: {}",
                    crate::goerr::quote(name),
                    crate::goerr::path_error(op, Path::new(&path), &e).0
                ));
            }
        }
        write_unit(&path, &contents)?;
        if name == WORKLOAD_SLICE {
            workload_updated = true;
        }
    }
    Ok(workload_updated)
}

/// `firstHostResourceError`.
fn first_error(res: &crate::hostobs::CommandResult) -> String {
    crate::incus::first_non_empty(&[&res.stderr, &res.stdout, "systemd command failed"]).to_string()
}

/// `Service.ReconcileHostResourcePolicy`.
fn reconcile_backend(env: &Env, ctx: &RunCtx, target: &Uri) -> Result<(), String> {
    let (scope, unit) = policy_target(target)?;
    let workload_updated = ensure_slice_units(&scope)?;
    let base: Vec<String> = if scope == "user" {
        vec![SYSTEMCTL.into(), "--user".into()]
    } else {
        vec!["sudo".into(), "-n".into(), SYSTEMCTL.into()]
    };
    // `HostCommandRunnerContext`: a done context is the error, reported
    // where a failed command's output would be.
    let run = |extra: &[&str]| -> crate::hostobs::CommandResult {
        if let Some(err) = ctx.err() {
            return crate::hostobs::CommandResult {
                exit_code: -1,
                stdout: String::new(),
                stderr: err,
            };
        }
        let mut argv = base.clone();
        argv.extend(extra.iter().map(|s| s.to_string()));
        let timeout = Duration::from_secs(15);
        run_command_with(
            env,
            &argv,
            None,
            Some(Instant::now() + timeout),
            timeout,
            Some(ctx),
        )
    };
    let res = run(&["daemon-reload"]);
    if res.exit_code != 0 {
        return Err(format!(
            "reload {scope} systemd resource policy: {}",
            first_error(&res)
        ));
    }
    let res = run(&["start", WORKLOAD_SLICE]);
    if res.exit_code != 0 {
        return Err(format!(
            "materialize {scope} systemd workload slice: {}",
            first_error(&res)
        ));
    }
    if workload_updated {
        let res = run(&[
            "set-property",
            WORKLOAD_SLICE,
            "MemoryHigh=10G",
            "MemoryMax=11G",
            "MemorySwapMax=1G",
            "CPUQuota=600%",
            "CPUWeight=100",
            "TasksMax=4096",
        ]);
        if res.exit_code != 0 {
            return Err(format!(
                "apply workload resource policy to {WORKLOAD_SLICE}: {}",
                first_error(&res)
            ));
        }
    }
    let cpu = format!("CPUWeight={AGENT_CPU_WEIGHT}");
    let tasks = format!("TasksMax={AGENT_TASKS_MAX}");
    let res = run(&["set-property", &unit, &cpu, &tasks]);
    if res.exit_code != 0 {
        return Err(format!(
            "apply protected Host Agent resource policy to {unit}: {}",
            first_error(&res)
        ));
    }
    let res = run(&[
        "show",
        &unit,
        "--property=Slice,CPUWeight,TasksMax,KillMode",
    ]);
    if res.exit_code != 0 {
        return Err(format!(
            "verify protected Host Agent resource policy on {unit}: {}",
            first_error(&res)
        ));
    }
    let properties: BTreeMap<String, String> = parse_systemd_properties(&res.stdout);
    // Go ranges over a map here too; the first mismatch reported may differ.
    for (key, expected) in [
        ("CPUWeight", AGENT_CPU_WEIGHT),
        ("KillMode", "control-group"),
        ("Slice", PROTECTED_SLICE),
        ("TasksMax", AGENT_TASKS_MAX),
    ] {
        let actual = properties.get(key).map(String::as_str).unwrap_or("");
        if actual != expected {
            return Err(format!(
                "host resource policy verification failed for {unit}: {key}={}, want {}",
                crate::goerr::quote(actual),
                crate::goerr::quote(expected)
            ));
        }
    }
    Ok(())
}

/// `Coordinator.Reconcile` and the `reconcile_host_resource_policy` result.
pub fn reconcile_host_resource_policy(
    env: &Env,
    coordinator: &Coordinator,
    tenant: &str,
    ctx: &RunCtx,
    policy_revision: &str,
    target: &str,
) -> Result<J, String> {
    if let Some(err) = ctx.err() {
        return Err(err);
    }
    if policy_revision.trim().is_empty() || policy_revision != coordinator.policy_revision() {
        return Ok(reconcile_error(
            "host_resource_policy_revision_mismatch",
            "requested policy revision is not the installed revision",
        ));
    }
    let parsed = match Uri::parse(target) {
        Ok(uri) if uri.resource_type == "host-service" => uri,
        _ => {
            return Ok(reconcile_error(
                "host_resource_target_invalid",
                "target must be an exact host-service URI",
            ))
        }
    };
    if !tenant.is_empty() && parsed.tenant_id != tenant {
        return Ok(reconcile_error(
            "host_resource_target_foreign_tenant",
            "target host-service URI belongs to another tenant",
        ));
    }
    if let Err(reason) = reconcile_backend(env, ctx, &parsed) {
        return Ok(reconcile_error("host_resource_reconcile_failed", &reason));
    }
    let snapshot = coordinator.snapshot();
    Ok(crate::tools::structured_result(
        json!({
            "reconciled": true,
            "policyRevision": snapshot.get("policyRevision").cloned().unwrap_or(J::Null),
            "enforcement": snapshot.get("enforcement").cloned().unwrap_or(J::Null),
            "capacity": snapshot,
        }),
        Some("Host resource policy revision reconciled."),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_follow_go() {
        let uri = |id: &str| Uri::parse(&format!("host-service:local:{id}")).unwrap();
        assert_eq!(
            policy_target(&uri("user/opute-host-agent")).unwrap(),
            ("user".into(), "opute-host-agent.service".into())
        );
        assert!(policy_target(&uri("system/opute-host-agent@a-1.service")).is_ok());
        assert!(policy_target(&uri("system/opute-host-agent@A")).is_err());
        assert!(policy_target(&uri("system/web.service")).is_err());
    }

    #[test]
    fn stricter_units_are_preserved() {
        let strict = "[Unit]\nDescription=operator\n[Slice]\nMemoryHigh=8G\nMemoryMax=9G\nMemorySwapMax=0\nCPUQuota=400%\nCPUWeight=50\nTasksMax=2048\n";
        assert!(
            !preserves_stricter(WORKLOAD_SLICE, strict),
            "MemorySwapMax=0 is not a limit"
        );
        let strict = strict.replace("MemorySwapMax=0", "MemorySwapMax=512M");
        assert!(preserves_stricter(WORKLOAD_SLICE, &strict));
        let older = slice_units()[1].1.replace(".v2)", ".v1)");
        assert!(
            !preserves_stricter(WORKLOAD_SLICE, &older),
            "an older managed unit migrates"
        );
        assert!(needs_migration(
            PROTECTED_SLICE,
            "Protected Opute Host Agent control slice"
        ));
    }
}
