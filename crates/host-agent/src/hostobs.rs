//! Host observations reported by `/health`: tested host capabilities and VM
//! inventory capacity (`fingerprint.DetectCapabilities`,
//! `incus.Service.VMInventoryCapacity`), plus the provider command runner
//! they use (`internal/exec.RunCommandContext`).
//!
//! Commands see the agent's effective environment (process environment plus
//! the env file), which in Go is the process environment after `os.Setenv`.

use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value as J};

use crate::config::Env;
use crate::gojson::{self, Node, Value};

const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(45);
const OWNER_LABEL: &str = "user.opute.host_agent_instance";

pub struct CommandResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// `exec.LookPath`: a name with a slash must be an executable file; other
/// names are searched in PATH, and a relative match is an error (ErrDot).
pub fn look_path(env: &Env, name: &str) -> Option<PathBuf> {
    let executable = |p: &Path| {
        std::fs::metadata(p)
            .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    };
    if name.contains('/') {
        let p = PathBuf::from(name);
        return executable(&p).then_some(p);
    }
    for dir in env.get("PATH").split(':') {
        let dir = if dir.is_empty() { "." } else { dir };
        let candidate = Path::new(dir).join(name);
        if executable(&candidate) {
            return candidate.is_absolute().then_some(candidate);
        }
    }
    None
}

/// `RunCommandContext` without streaming: start failures and timeouts are
/// results, not errors, exactly as in Go.
pub fn run_command(env: &Env, argv: &[String], timeout: Duration) -> CommandResult {
    if argv.is_empty() {
        return CommandResult {
            exit_code: 1,
            stdout: String::new(),
            stderr: "empty command".into(),
        };
    }
    let program = if argv[0].contains('/') {
        Some(PathBuf::from(&argv[0]))
    } else {
        look_path(env, &argv[0])
    };
    let Some(program) = program else {
        return CommandResult {
            exit_code: 1,
            stdout: String::new(),
            stderr: format!(
                "exec: {}: executable file not found in $PATH",
                crate::goerr::quote(&argv[0])
            ),
        };
    };
    let mut cmd = Command::new(&program);
    cmd.args(&argv[1..])
        .env_clear()
        .envs(env.pairs())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return CommandResult {
                exit_code: 1,
                stdout: String::new(),
                stderr: format!(
                    "fork/exec {}: {}",
                    program.display(),
                    crate::goerr::errno_text(&e)
                ),
            }
        }
    };
    let mut out_pipe = child.stdout.take().expect("piped stdout");
    let mut err_pipe = child.stderr.take().expect("piped stderr");
    let out_reader = std::thread::spawn(move || {
        let mut s = Vec::new();
        let _ = out_pipe.read_to_end(&mut s);
        s
    });
    let err_reader = std::thread::spawn(move || {
        let mut s = Vec::new();
        let _ = err_pipe.read_to_end(&mut s);
        s
    });
    let deadline = Instant::now() + timeout;
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() >= deadline => {
                timed_out = true;
                let pgid = nix::unistd::Pid::from_raw(child.id() as i32);
                let _ = nix::sys::signal::killpg(pgid, nix::sys::signal::Signal::SIGKILL);
                break child.wait().ok();
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(_) => break None,
        }
    };
    let stdout = String::from_utf8_lossy(&out_reader.join().unwrap_or_default()).into_owned();
    let mut stderr = String::from_utf8_lossy(&err_reader.join().unwrap_or_default()).into_owned();
    if timed_out {
        if !stderr.is_empty() {
            stderr.push('\n');
        }
        stderr.push_str(&format!(
            "Error: Command timed out after {}",
            go_duration(timeout)
        ));
        return CommandResult {
            exit_code: 124,
            stdout,
            stderr,
        };
    }
    let exit_code = status.and_then(|s| s.code()).unwrap_or(-1);
    CommandResult {
        exit_code,
        stdout,
        stderr,
    }
}

/// `time.Duration.String` for whole seconds and minutes.
fn go_duration(d: Duration) -> String {
    let secs = d.as_secs();
    if secs >= 60 {
        let (m, s) = (secs / 60, secs % 60);
        return format!("{m}m{s}s");
    }
    format!("{secs}s")
}

fn is_wsl(env: &Env) -> bool {
    if !env.get("WSL_INTEROP").trim().is_empty() || !env.get("WSL_DISTRO_NAME").trim().is_empty() {
        return true;
    }
    std::fs::read_to_string("/proc/version")
        .map(|v| {
            let v = v.to_lowercase();
            v.contains("microsoft") || v.contains("wsl")
        })
        .unwrap_or(false)
}

fn windows_interop_command(env: &Env, name: &str) -> bool {
    let mut candidates: Vec<String> = Vec::new();
    match name {
        "powershell.exe" => {
            candidates.push("/mnt/c/Windows/System32/WindowsPowerShell/v1.0/powershell.exe".into())
        }
        "reg.exe" | "wsl.exe" => {
            candidates.push(format!("/mnt/c/Windows/System32/{name}"));
            candidates.push(format!("/mnt/c/Windows/system32/{name}"));
        }
        _ => {}
    }
    candidates.push(name.into());
    candidates.iter().any(|c| look_path(env, c).is_some())
}

/// `fingerprint.DetectCapabilities` on Linux.
pub fn detect_capabilities(env: &Env) -> J {
    let wsl = is_wsl(env);
    let interop = wsl
        && (windows_interop_command(env, "powershell.exe")
            || windows_interop_command(env, "reg.exe"));
    let manage = wsl && windows_interop_command(env, "wsl.exe");
    json!({
        "canInvokeWindowsInterop": interop,
        "canManageWsl": manage,
        "canTerminateWslDistribution": manage,
        "canShutdownWsl": manage,
    })
}

fn provider_binary(env: &Env) -> String {
    for key in ["OPUTE_INCUS_BINARY_PATH", "OPUTE_VM_BINARY_PATH"] {
        let v = env.get(key).trim().to_string();
        if !v.is_empty() {
            return v;
        }
    }
    for path in ["/usr/bin/incus", "/snap/bin/incus"] {
        if Path::new(path).exists() {
            return path.into();
        }
    }
    "incus".into()
}

/// Go's typed decode of one `incusListItem`; any type mismatch makes the
/// whole list "invalid JSON".
struct ListItem {
    name: String,
    status: String,
    kind: String,
    owner: String,
}

fn string_field(item: &Node, key: &str) -> Result<String, ()> {
    gojson::go_string(item.field(key))
}

fn string_map_value(item: &Node, key: &str, label: &str) -> Result<String, ()> {
    match item.field(key).map(|n| &n.value) {
        None | Some(Value::Null) => Ok(String::new()),
        Some(Value::Object(members)) => {
            let mut found = String::new();
            for (k, v) in members {
                let s = gojson::go_string(Some(v))?;
                if k == label {
                    found = s;
                }
            }
            Ok(found.trim().to_string())
        }
        _ => Err(()),
    }
}

fn check_object_of_objects(item: &Node, key: &str) -> Result<(), ()> {
    match item.field(key).map(|n| &n.value) {
        None | Some(Value::Null) => Ok(()),
        Some(Value::Object(members)) => members
            .iter()
            .all(|(_, v)| matches!(v.value, Value::Object(_) | Value::Null))
            .then_some(())
            .ok_or(()),
        _ => Err(()),
    }
}

fn decode_items(stdout: &str) -> Result<Vec<ListItem>, ()> {
    let doc = gojson::parse(stdout.as_bytes()).map_err(|_| ())?;
    let items = match &doc.value {
        Value::Null => return Ok(Vec::new()),
        Value::Array(items) => items,
        _ => return Err(()),
    };
    let mut out = Vec::new();
    for item in items {
        match &item.value {
            Value::Null => continue,
            Value::Object(_) => {}
            _ => return Err(()),
        }
        check_object_of_objects(item, "devices")?;
        check_object_of_objects(item, "expanded_devices")?;
        if !matches!(
            item.field("state").map(|n| &n.value),
            None | Some(Value::Null) | Some(Value::Object(_))
        ) {
            return Err(());
        }
        let config_owner = string_map_value(item, "config", OWNER_LABEL)?;
        let expanded_owner = string_map_value(item, "expanded_config", OWNER_LABEL)?;
        out.push(ListItem {
            name: string_field(item, "name")?,
            status: string_field(item, "status")?,
            kind: string_field(item, "type")?,
            owner: if config_owner.is_empty() {
                expanded_owner
            } else {
                config_owner
            },
        });
    }
    Ok(out)
}

/// `VMInventoryCapacity` counts (the two `/health` reports). The error is
/// the provider's own message, as Go reports it.
pub fn vm_capacity(
    env: &Env,
    instance_id: &str,
    ownership_mode: &str,
) -> Result<(i64, i64), String> {
    let binary = provider_binary(env);
    if look_path(env, &binary).is_none() {
        return Err(format!(
            "virtualization_stack_absent: the incus virtualization stack is not installed on this host; install it with {}",
            "install_incus_stack"
        ));
    }
    let argv: Vec<String> = [binary.as_str(), "list", "--format", "json"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    let res = run_command(env, &argv, DISCOVERY_TIMEOUT);
    if res.exit_code != 0 {
        let message = [res.stderr.trim(), res.stdout.trim()]
            .into_iter()
            .find(|s| !s.is_empty())
            .unwrap_or("incus list failed");
        return Err(message.to_string());
    }
    let items =
        decode_items(&res.stdout).map_err(|_| "incus list returned invalid JSON".to_string())?;
    let enforce = !instance_id.trim().is_empty() && ownership_mode == "enforce";
    let (mut total, mut running) = (0, 0);
    for item in items {
        if item.name.is_empty() {
            continue;
        }
        let kind = item.kind.trim().to_lowercase();
        let is_vm = kind == "virtual-machine" || kind == "virtual machine" || kind == "container";
        if !is_vm || (enforce && item.owner != instance_id) {
            continue;
        }
        total += 1;
        if item.status.eq_ignore_ascii_case("running") {
            running += 1;
        }
    }
    Ok((running, total))
}

/// The `HealthObserver` closure from `internal/app/app.go`.
pub fn health_observer(
    env: Env,
    instance_id: String,
    ownership_mode: String,
) -> impl Fn() -> Map<String, J> {
    move || {
        let capacity = vm_capacity(&env, &instance_id, &ownership_mode);
        let mut result = Map::new();
        result.insert(
            "capabilities".into(),
            json!({"host": detect_capabilities(&env)}),
        );
        if let Ok((running, total)) = capacity {
            result.insert("runningVmCount".into(), J::from(running));
            result.insert("totalVmCount".into(), J::from(total));
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_counts_and_type_strictness() {
        let ok = r#"[{"name":"a","status":"Running","type":"container"},
                     {"name":"","status":"Running","type":"container"},
                     {"name":"b","status":"Stopped","type":"virtual-machine","config":{"user.opute.host_agent_instance":"x"}}]"#;
        let items = decode_items(ok).unwrap();
        assert_eq!(items.len(), 3);
        assert_eq!(items[2].owner, "x");
        assert!(decode_items(r#"[{"name":1}]"#).is_err());
        assert!(decode_items(r#"[{"name":"a","config":{"k":1}}]"#).is_err());
        assert!(decode_items("null").unwrap().is_empty());
    }

    #[test]
    fn durations_print_like_go() {
        assert_eq!(go_duration(Duration::from_secs(45)), "45s");
        assert_eq!(go_duration(Duration::from_secs(120)), "2m0s");
    }
}
