//! Host service lifecycle (`internal/domain/host/host_service.go`,
//! `types.go`): unit state changes through the matching systemd manager,
//! the persistent user-manager supervisor, the bridge probe and its
//! recovery, and the container-engine stub an Incus host refuses.
//! (`ensure_k3d` has no counterpart: Go omits it from every catalog
//! (`IncusOmittedToolNames`), so no call can reach it.)

use std::time::Duration;

use serde_json::{json, Value as J};

use crate::config::Env;
use crate::hostfiles::{require_shared_host_owner, Owner};
use crate::hostobs::{run_command, CommandResult};
use crate::incus::first_non_empty;

const SYSTEMCTL: &str = "/usr/bin/systemctl";
const SYSTEMD_RUN: &str = "/usr/bin/systemd-run";

fn argv(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

/// `safeSystemdUnitName`.
fn safe_unit_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.@:-".contains(&b))
}

/// A run with Go's zero timeout: no deadline.
fn run(env: &Env, command: Vec<String>, timeout: Duration) -> CommandResult {
    run_command(env, &command, timeout)
}

fn scope_of(raw: &str) -> Result<String, String> {
    let scope = raw.trim().to_lowercase();
    let scope = if scope.is_empty() {
        "user".into()
    } else {
        scope
    };
    if scope != "user" && scope != "system" {
        return Err("scope must be user or system".into());
    }
    Ok(scope)
}

/// `serviceStateUnit`.
fn state_unit(service: &str) -> String {
    let name: String = service
        .chars()
        .map(|c| if "/:@.".contains(c) { '-' } else { c })
        .collect();
    format!("host-service-state-{name}")
}

/// `serviceStateCommand`.
fn state_command(service: &str, state: &str, scope: &str) -> Vec<String> {
    if state == "restart" && scope == "user" && service.starts_with("opute-host-agent") {
        // A transient user job outside the target's cgroup, so restarting
        // the serving agent can still report "scheduled".
        return argv(&[
            SYSTEMD_RUN,
            "--user",
            &format!("--unit={}", state_unit(service)),
            "--collect",
            "--no-block",
            SYSTEMCTL,
            "--user",
            "--no-block",
            "restart",
            service,
        ]);
    }
    let mut command = argv(&[SYSTEMCTL]);
    if scope == "user" {
        command.push("--user".into());
    } else {
        command.splice(0..0, argv(&["sudo", "-n"]));
    }
    command.extend(argv(&[state, service]));
    command
}

/// `Service.SetHostServiceState`.
pub fn set_host_service_state(
    env: &Env,
    owner: &Owner,
    service: &str,
    state: &str,
    scope: &str,
) -> Result<J, String> {
    require_shared_host_owner(
        owner.instance_id,
        owner.owner_instance,
        "set_host_service_state",
    )?;
    let service = service.trim();
    let state = state.trim().to_lowercase();
    if !safe_unit_name(service) {
        return Err("serviceName is required and must be a valid systemd unit name".into());
    }
    let scope = scope_of(scope)?;
    if !["start", "stop", "restart", "enable", "disable"].contains(&state.as_str()) {
        return Err("state must be start, stop, restart, enable, or disable".into());
    }
    // Reload the matching manager first, so a unit file reconciled just
    // before this call is not acted on stale.
    let reload = if scope == "user" {
        argv(&[SYSTEMCTL, "--user", "daemon-reload"])
    } else {
        argv(&["sudo", "-n", SYSTEMCTL, "daemon-reload"])
    };
    let res = run(env, reload, Duration::ZERO);
    if res.exit_code != 0 {
        return Err(format!(
            "service manager reload failed: {}",
            first_non_empty(&[&res.stderr, &res.stdout, "command failed"])
        ));
    }
    let res = run(env, state_command(service, &state, &scope), Duration::ZERO);
    if res.exit_code != 0 {
        return Err(format!(
            "service state change failed: {}",
            first_non_empty(&[&res.stderr, &res.stdout, "command failed"])
        ));
    }
    let status = if state == "restart" {
        "scheduled"
    } else {
        "applied"
    };
    Ok(json!({"serviceName": service, "state": state, "scope": scope, "status": status}))
}

/// `Service.RestartHostService`. Opute-owned units are user units, restarted
/// without blocking because the target may be this agent.
pub fn restart_host_service(env: &Env, owner: &Owner, service: &str) -> Result<J, String> {
    require_shared_host_owner(
        owner.instance_id,
        owner.owner_instance,
        "restart_host_service",
    )?;
    let service = service.trim();
    if service.is_empty() {
        return Err("serviceName is required".into());
    }
    if !safe_unit_name(service) {
        return Err("serviceName contains invalid characters".into());
    }
    let opute = service.starts_with("opute-");
    let restart = if opute {
        argv(&[SYSTEMCTL, "--user", "--no-block", "restart", service])
    } else {
        argv(&[SYSTEMCTL, "restart", service])
    };
    let res = run(env, restart, Duration::ZERO);
    if res.exit_code != 0 {
        return Err(
            first_non_empty(&[&res.stderr, &res.stdout, "failed to restart service"]).to_string(),
        );
    }
    if opute {
        return Ok(json!({"serviceName": service, "status": "scheduled"}));
    }
    let verify = run(
        env,
        argv(&[SYSTEMCTL, "is-active", service]),
        Duration::ZERO,
    );
    if verify.exit_code != 0 || verify.stdout.trim() != "active" {
        return Err(format!("service '{service}' is not active after restart"));
    }
    Ok(json!({"serviceName": service, "status": "active"}))
}

/// `$USER`, else the account name, as the supervisor tools resolve it.
fn service_user(env: &Env) -> Result<String, String> {
    let mut user = env.get("USER").trim().to_string();
    if user.is_empty() {
        user = match nix::unistd::User::from_uid(nix::unistd::getuid()) {
            Ok(Some(account)) => account.name,
            Ok(None) => {
                return Err(format!(
                    "resolve host service user: user: unknown userid {}",
                    nix::unistd::getuid()
                ))
            }
            Err(e) => {
                return Err(format!(
                    "resolve host service user: {}",
                    e.desc().to_lowercase()
                ))
            }
        };
    }
    if user.is_empty() || user.contains(['\r', '\n']) {
        return Err("resolve host service user: invalid username".into());
    }
    Ok(user)
}

const SECONDS_10: Duration = Duration::from_secs(10);
const SECONDS_15: Duration = Duration::from_secs(15);

/// `Service.EnsureHostServiceSupervisor`: a lingering user manager (or a
/// reachable system manager), observed rather than assumed.
pub fn ensure_host_service_supervisor(env: &Env, owner: &Owner, scope: &str) -> Result<J, String> {
    require_shared_host_owner(
        owner.instance_id,
        owner.owner_instance,
        "ensure_host_service_supervisor",
    )?;
    let scope = scope_of(scope)?;
    if scope == "system" {
        let res = run(env, argv(&[SYSTEMCTL, "is-system-running"]), SECONDS_10);
        if res.exit_code != 0 && res.stdout.trim().is_empty() {
            return Err(format!(
                "system service supervisor is unavailable: {}",
                first_non_empty(&[&res.stderr, &res.stdout, "systemctl failed"])
            ));
        }
        return Ok(
            json!({"scope": scope, "status": "ready", "persistent": true,
                         "state": res.stdout.trim()}),
        );
    }
    let user = service_user(env)?;
    let mut res = run(env, argv(&["loginctl", "enable-linger", &user]), SECONDS_15);
    if res.exit_code != 0 {
        // A narrowly scoped bootstrap sudo policy; never an interactive prompt.
        res = run(
            env,
            argv(&["sudo", "-n", "loginctl", "enable-linger", &user]),
            SECONDS_15,
        );
    }
    if res.exit_code != 0 {
        return Err(format!(
            "enable persistent user service supervisor: {}",
            first_non_empty(&[&res.stderr, &res.stdout, "loginctl failed"])
        ));
    }
    let observed = run(
        env,
        argv(&["loginctl", "show-user", &user, "-p", "Linger"]),
        SECONDS_15,
    );
    if observed.exit_code != 0 || !observed.stdout.contains("Linger=yes") {
        return Err(format!(
            "verify persistent user service supervisor: {}",
            first_non_empty(&[
                &observed.stderr,
                &observed.stdout,
                "Linger=yes was not observed"
            ])
        ));
    }
    let bus = run(
        env,
        argv(&[SYSTEMCTL, "--user", "show-environment"]),
        SECONDS_15,
    );
    if bus.exit_code != 0 {
        return Err(format!(
            "user service supervisor bus is unavailable: {}",
            first_non_empty(&[&bus.stderr, &bus.stdout, "systemctl --user failed"])
        ));
    }
    Ok(
        json!({"scope": scope, "status": "ready", "persistent": true, "user": user,
              "linger": true, "userBus": true}),
    )
}

fn status_word(ready: bool) -> &'static str {
    if ready {
        "ready"
    } else {
        "unavailable"
    }
}

/// `Service.InspectHostServiceSupervisor`: the same observation, changing
/// nothing.
pub fn inspect_host_service_supervisor(env: &Env, scope: &str) -> Result<J, String> {
    let scope = scope_of(scope)?;
    if scope == "system" {
        // "degraded" exits non-zero yet is a supervising manager.
        let res = run(env, argv(&[SYSTEMCTL, "is-system-running"]), SECONDS_10);
        let state = res.stdout.trim().to_string();
        let ready = !state.is_empty();
        return Ok(
            json!({"scope": scope, "status": status_word(ready), "persistent": ready,
                         "state": state}),
        );
    }
    let user = service_user(env)?;
    let observed = run(
        env,
        argv(&["loginctl", "show-user", &user, "-p", "Linger"]),
        SECONDS_15,
    );
    let linger = observed.exit_code == 0 && observed.stdout.contains("Linger=yes");
    let bus = run(
        env,
        argv(&[SYSTEMCTL, "--user", "show-environment"]),
        SECONDS_15,
    );
    let user_bus = bus.exit_code == 0;
    let ready = linger && user_bus;
    Ok(
        json!({"scope": scope, "status": status_word(ready), "persistent": ready,
              "user": user, "linger": linger, "userBus": user_bus}),
    )
}

/// `Service.EnsureDocker`: unsupported on an Incus host.
pub fn ensure_docker() -> Result<J, String> {
    Err("ensure_docker is not supported on Incus Linux host agents".into())
}

// --- bridge ------------------------------------------------------------------------

/// `hostruntime.EnvOr`.
fn env_or(env: &Env, key: &str, fallback: &str) -> String {
    let value = env.get(key).trim().to_string();
    if value.is_empty() {
        fallback.into()
    } else {
        value
    }
}

/// `fmt.Sscanf(p, "%d", &port)`: a leading signed decimal; on failure the
/// port keeps its default.
fn scan_port(raw: &str, default: i64) -> i64 {
    let bytes = raw.as_bytes();
    let mut end = usize::from(matches!(bytes.first(), Some(b'+' | b'-')));
    let digits_start = end;
    while end < bytes.len() && bytes[end].is_ascii_digit() {
        end += 1;
    }
    if end == digits_start {
        return default;
    }
    raw[..end].parse::<i64>().unwrap_or(default)
}

/// `probeTCPPort`: a two-second dial.
fn probe_tcp_port(port: i64) -> Result<(), String> {
    use std::net::{SocketAddr, TcpStream};
    let address = format!("127.0.0.1:{port}");
    let Ok(socket) = address.parse::<SocketAddr>() else {
        return Err(format!("dial tcp: address {port}: invalid port"));
    };
    TcpStream::connect_timeout(&socket, Duration::from_secs(2))
        .map(drop)
        .map_err(|e| {
            if matches!(
                e.kind(),
                std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
            ) {
                format!("dial tcp {address}: i/o timeout")
            } else {
                format!(
                    "dial tcp {address}: connect: {}",
                    crate::goerr::errno_text(&e)
                )
            }
        })
}

/// The bridge health body, decoded as Go's `json.Decoder` fills its struct:
/// keys match case-insensitively, the last match wins, null leaves a field
/// alone, and any type mismatch fails the decode.
struct Health {
    status: String,
    error: String,
    last_heartbeat: Option<String>,
}

fn decode_health(body: &[u8]) -> Option<Health> {
    use crate::gojson::{equal_fold, parse, Node, Value};
    fn string_field(node: &Node, out: &mut String) -> bool {
        match &node.value {
            Value::Null => true,
            Value::String(_) => {
                *out = crate::gojson::go_string(Some(node)).unwrap_or_default();
                true
            }
            _ => false,
        }
    }
    let root = parse(body).ok()?;
    let mut health = Health {
        status: String::new(),
        error: String::new(),
        last_heartbeat: None,
    };
    let mut ok = true;
    match &root.value {
        Value::Null => return Some(health),
        Value::Object(members) => {
            for (key, value) in members {
                if equal_fold(key, "database") {
                    match &value.value {
                        Value::Null => {}
                        Value::Object(fields) => {
                            for (k, v) in fields {
                                if equal_fold(k, "status") {
                                    ok &= string_field(v, &mut health.status);
                                } else if equal_fold(k, "error") {
                                    ok &= string_field(v, &mut health.error);
                                }
                            }
                        }
                        _ => ok = false,
                    }
                } else if equal_fold(key, "lastHeartbeatAt") {
                    match &value.value {
                        Value::Null => health.last_heartbeat = None,
                        Value::String(_) => {
                            health.last_heartbeat = crate::gojson::go_string(Some(value)).ok()
                        }
                        _ => ok = false,
                    }
                }
            }
        }
        _ => ok = false,
    }
    ok.then_some(health)
}

/// `probeBridgeHealth`.
fn probe_bridge_health(env: &Env) -> J {
    let mut port = 9093;
    let raw = env.get("PLATFORM_MCP_PORT").trim().to_string();
    if !raw.is_empty() {
        port = scan_port(&raw, port);
    }
    let bridge_url = env_or(env, "BRIDGE_URL", &format!("http://127.0.0.1:{port}"));
    let service = env_or(env, "BRIDGE_SERVICE_NAME", "opute-bridge");
    let checked_at = crate::hostobs::rfc3339_now();
    let probe = probe_tcp_port(port);
    let (process, port_status, bridge_status) = if probe.is_ok() {
        ("running", "open", "online")
    } else {
        ("stopped", "closed", "offline")
    };
    let mut bridge_port = json!({"port": port, "status": port_status});
    if let Err(e) = &probe {
        bridge_port["error"] = J::from(e.clone());
    }
    let mut db_status = "unhealthy";
    let mut db_error = "Bridge health check failed".to_string();
    let mut last_heartbeat = None;
    let url = format!("{}/health", bridge_url.trim_end_matches('/'));
    let scheme = url
        .split_once(':')
        .map_or("", |(s, _)| s)
        .to_ascii_lowercase();
    if url.bytes().any(|b| b < 0x20 || b == 0x7f) {
        // `http.NewRequest` refuses the URL; the defaults stand.
    } else if scheme != "http" && scheme != "https" {
        db_error = format!("Get \"{url}\": unsupported protocol scheme \"{scheme}\"");
    } else {
        let agent = ureq::AgentBuilder::new().redirects(10).build();
        match agent.get(&url).call() {
            Ok(response) => {
                let mut body = Vec::new();
                let _ = std::io::Read::read_to_end(&mut response.into_reader(), &mut body);
                if let Some(health) = decode_health(&body) {
                    last_heartbeat = health.last_heartbeat;
                    if health.status == "healthy" {
                        db_status = "healthy";
                        db_error.clear();
                    } else if !health.error.is_empty() {
                        db_error = health.error;
                    }
                }
            }
            Err(ureq::Error::Status(code, _)) => {
                db_error = format!("Bridge health check failed with HTTP {code}");
            }
            Err(ureq::Error::Transport(t)) => {
                db_error = crate::hostartifact::get_error(&url, &t);
            }
        }
    }
    let mut database = json!({"status": db_status});
    if !db_error.is_empty() {
        database["error"] = J::from(db_error);
    }
    json!({
        "bridgeProcess": {"status": process, "command": service},
        "bridgePort": bridge_port,
        "database": database,
        "lastHeartbeat": {"at": last_heartbeat},
        "bridgeStatus": bridge_status,
        "checkedAt": checked_at,
    })
}

/// `json.Marshal(BridgeDiagnosticResult)`: struct field order.
pub fn bridge_text(result: &J) -> String {
    use crate::gojson::Shape::{Any, Struct};
    static SHAPE: crate::gojson::Shape = Struct(&[
        (
            "bridgeProcess",
            Struct(&[("status", Any), ("command", Any), ("restarted", Any)]),
        ),
        (
            "bridgePort",
            Struct(&[("port", Any), ("status", Any), ("error", Any)]),
        ),
        ("database", Struct(&[("status", Any), ("error", Any)])),
        ("lastHeartbeat", Struct(&[("at", Any)])),
        ("bridgeStatus", Any),
        ("checkedAt", Any),
    ]);
    let mut out = String::new();
    crate::gojson::encode_shaped(result, &SHAPE, &mut out);
    out
}

/// `Service.DiagnoseBridge`.
pub fn diagnose_bridge(env: &Env) -> J {
    probe_bridge_health(env)
}

/// `Service.RecoverBridge`: restart the bridge unit, then probe again.
pub fn recover_bridge(env: &Env, owner: &Owner) -> Result<J, String> {
    let service = env_or(env, "BRIDGE_SERVICE_NAME", "opute-bridge");
    restart_host_service(env, owner, &service)?;
    let mut result = probe_bridge_health(env);
    result["bridgeProcess"]["restarted"] = J::Bool(true);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_commands_follow_go() {
        assert_eq!(
            state_command("web.service", "start", "system"),
            argv(&["sudo", "-n", SYSTEMCTL, "start", "web.service"])
        );
        assert_eq!(
            state_command("opute-host-agent@a.service", "restart", "user")[2],
            "--unit=host-service-state-opute-host-agent-a-service"
        );
        assert!(safe_unit_name("a@b:c.service") && !safe_unit_name("a b"));
    }

    #[test]
    fn ports_scan_like_sscanf() {
        assert_eq!(scan_port("8080x", 9093), 8080);
        assert_eq!(scan_port("-5", 9093), -5);
        assert_eq!(scan_port("x", 9093), 9093);
    }

    #[test]
    fn health_decodes_like_go() {
        let h =
            decode_health(br#"{"DATABASE":{"Status":"healthy"},"lastheartbeatat":"t"}"#).unwrap();
        assert_eq!(
            (h.status.as_str(), h.last_heartbeat.as_deref()),
            ("healthy", Some("t"))
        );
        assert!(decode_health(br#"{"database":{"status":1}}"#).is_none());
        assert!(decode_health(b"null").is_some());
    }
}
