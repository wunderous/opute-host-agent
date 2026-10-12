//! Connection plumbing a control plane or tunnel provider asks the host for
//! (`internal/domain/host/generic_agent_connection.go`, `firewall.go`): a
//! caller-owned environment file, optionally followed by a service restart,
//! and the host firewall rule, which a Linux host reports as not needed.

use std::collections::{BTreeMap, HashMap};
use std::io;
use std::path::Path;

use serde_json::{json, Value as J};

use crate::config::Env;
use crate::goerr::{errno_text, path_error};
use crate::hostfiles::Owner;

/// `upsertEnvFile`: comments first, then keys in first-seen order with the
/// last value winning; removals drop a key, `export ` prefixes are dropped,
/// and the file is replaced atomically with mode 0600.
///
/// Go appends new keys while ranging over the assignment map, so their
/// relative order varies run to run; sorted order is one of those orders.
fn upsert_env_file(
    path: &str,
    assignments: &BTreeMap<String, String>,
    removals: &[String],
) -> Result<(), String> {
    use std::io::{Read, Write};
    use std::os::unix::fs::PermissionsExt;
    let removed: Vec<&str> = removals
        .iter()
        .map(|k| k.trim())
        .filter(|k| !k.is_empty())
        .collect();
    let mut values: HashMap<String, String> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    let mut comments: Vec<String> = Vec::new();
    let mut raw = Vec::new();
    match std::fs::File::open(path) {
        Ok(mut file) => {
            file.read_to_end(&mut raw).map_err(|e| {
                format!(
                    "read env file: {}",
                    path_error("read", Path::new(path), &e).0
                )
            })?;
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(format!(
                "read env file: {}",
                path_error("open", Path::new(path), &e).0
            ))
        }
    }
    let text = String::from_utf8_lossy(&raw).into_owned();
    if !raw.is_empty() {
        for line in text.split('\n') {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                if !trimmed.is_empty() {
                    comments.push(line.to_string());
                }
                continue;
            }
            let trimmed = trimmed.strip_prefix("export ").unwrap_or(trimmed);
            let Some((key, value)) = trimmed.split_once('=') else {
                continue;
            };
            let key = key.trim();
            if key.is_empty() || removed.contains(&key) {
                continue;
            }
            if !values.contains_key(key) {
                order.push(key.to_string());
            }
            values.insert(key.to_string(), value.to_string());
        }
    }
    for (key, value) in assignments {
        if !values.contains_key(key) {
            order.push(key.clone());
        }
        values.insert(key.clone(), value.clone());
    }
    let mut out = String::new();
    for comment in &comments {
        out.push_str(comment);
        out.push('\n');
    }
    for key in &order {
        out.push_str(key);
        out.push('=');
        out.push_str(&values[key]);
        out.push('\n');
    }
    let dir = Path::new(path)
        .parent()
        .map(|p| p.display().to_string())
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| ".".into());
    crate::goerr::mkdir_all(Path::new(&dir), 0o700)
        .map_err(|e| format!("create env directory: {}", e.0))?;
    let (mut file, temporary) = crate::goerr::create_temp(&dir, ".host-agent.env.", "")
        .map_err(|e| format!("create temp env file: {e}"))?;
    struct Remove<'a>(&'a str);
    impl Drop for Remove<'_> {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(self.0);
        }
    }
    let _cleanup = Remove(&temporary);
    file.set_permissions(std::fs::Permissions::from_mode(0o600))
        .map_err(|e| {
            format!(
                "chmod env file: {}",
                path_error("chmod", Path::new(&temporary), &e).0
            )
        })?;
    file.write_all(out.as_bytes()).map_err(|e| {
        format!(
            "write env file: {}",
            path_error("write", Path::new(&temporary), &e).0
        )
    })?;
    drop(file);
    std::fs::rename(&temporary, path).map_err(|e| {
        format!(
            "replace env file: rename {temporary} {path}: {}",
            errno_text(&e)
        )
    })
}

/// `Service.ConfigureAgentConnection`.
#[allow(clippy::too_many_arguments)]
pub fn configure_agent_connection(
    env: &Env,
    owner: &Owner,
    env_file: &str,
    environment: &BTreeMap<String, String>,
    remove: &[String],
    service: &str,
    restart: Option<bool>,
    scope: &str,
) -> Result<J, String> {
    if env_file.trim().is_empty() {
        return Err("envFile is required".into());
    }
    if environment.is_empty() {
        return Err("environment is required".into());
    }
    upsert_env_file(env_file, environment, remove)?;
    let restart = restart == Some(true);
    let mut result = json!({"envFile": env_file, "restart": restart, "status": "env_written"});
    if service.trim().is_empty() || !restart {
        return Ok(result);
    }
    let state = crate::hostservice::set_host_service_state(env, owner, service, "restart", scope)?;
    result["status"] = J::from("restart_scheduled");
    result["service"] = state;
    Ok(result)
}

/// `Service.EnsureHostFirewallRule`: a Linux host has no rule to apply.
/// (`OPUTE_HOST_AGENT_ELEVATED` is consulted only on Windows.)
pub fn ensure_host_firewall_rule(binding_id: &str, port: i64) -> J {
    json!({"bindingId": binding_id, "port": port, "applied": true, "code": "skipped.non_windows"})
}
