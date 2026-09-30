//! Physical host fingerprint and execution context (`internal/fingerprint`).
//!
//! These are evidence attached to the explicit `OPUTE_REMOTE_AGENT_ID`, never
//! a substitute for it. Reading them can fail, and a failure makes startup
//! fail closed ("host identity unavailable: ...").

use std::path::PathBuf;
use std::process::Command;

use sha2::{Digest, Sha256};

use crate::config::Env;
use crate::goerr::{self, path_error};

pub const VERSION: &str = "v2";

const SOURCE_LINUX_MACHINE_ID: &str = "linux-machine-id";
const SOURCE_WINDOWS_MACHINE_GUID: &str = "windows-machine-guid";
const SOURCE_WINDOWS_MACHINE_GUID_VIA_WSL: &str = "windows-machine-guid-via-wsl";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecutionContext {
    pub id: String,
    pub kind: String,
    pub display_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub fingerprint: String,
    pub fingerprint_version: String,
    pub fingerprint_source: String,
    pub execution_context: ExecutionContext,
}

fn machine_id_path() -> PathBuf {
    PathBuf::from("/etc/machine-id")
}

/// `fingerprint.ReadIdentity` on Linux (native and WSL).
pub fn read_identity(env: &Env) -> Result<Identity, String> {
    let context = read_linux_execution_context(env)?;
    if context.kind == "wsl" {
        let guid = match read_windows_machine_guid() {
            Ok(guid) => {
                write_cached_machine_guid(env, SOURCE_WINDOWS_MACHINE_GUID_VIA_WSL, &guid);
                guid
            }
            Err(err) => match read_cached_machine_guid(env, SOURCE_WINDOWS_MACHINE_GUID_VIA_WSL) {
                Some(cached) => cached,
                None => {
                    return Err(format!(
                        "read Windows MachineGuid through WSL interop: {err}"
                    ))
                }
            },
        };
        let mut identity = format_identity(SOURCE_WINDOWS_MACHINE_GUID_VIA_WSL, &guid);
        identity.execution_context = context;
        return Ok(identity);
    }
    let raw = read_machine_id()?;
    let value = raw.trim();
    if value.is_empty() {
        return Err("empty /etc/machine-id".into());
    }
    let mut identity = format_identity(SOURCE_LINUX_MACHINE_ID, value);
    identity.execution_context = context;
    Ok(identity)
}

fn read_machine_id() -> Result<String, String> {
    let path = machine_id_path();
    std::fs::read(&path)
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .map_err(|e| path_error("open", &path, &e).0)
}

fn read_linux_execution_context(env: &Env) -> Result<ExecutionContext, String> {
    let raw = read_machine_id()?;
    let machine_id = raw.trim();
    if machine_id.is_empty() {
        return Err("empty /etc/machine-id for execution context".into());
    }
    if is_wsl(env) {
        let digest = Sha256::digest(format!(
            "execution-context:{VERSION}:wsl:{}",
            machine_id.to_lowercase()
        ));
        return Ok(ExecutionContext {
            id: format!("wsl:{}", hex::encode(digest)),
            kind: "wsl".into(),
            display_name: env.get("WSL_DISTRO_NAME").trim().to_string(),
        });
    }
    Ok(ExecutionContext {
        id: "native-linux".into(),
        kind: "native-linux".into(),
        display_name: "Linux".into(),
    })
}

fn is_wsl(env: &Env) -> bool {
    if !env.get("WSL_INTEROP").trim().is_empty() || !env.get("WSL_DISTRO_NAME").trim().is_empty() {
        return true;
    }
    match std::fs::read("/proc/version") {
        Ok(raw) => {
            let value = String::from_utf8_lossy(&raw).to_lowercase();
            value.contains("microsoft") || value.contains("wsl")
        }
        Err(_) => false,
    }
}

fn format_identity(source: &str, value: &str) -> Identity {
    let normalized = value.trim().to_lowercase();
    // Native Windows and WSL read the same installation GUID; the physical
    // digest uses one canonical source so both group under one host.
    let physical = if source == SOURCE_WINDOWS_MACHINE_GUID_VIA_WSL {
        SOURCE_WINDOWS_MACHINE_GUID
    } else {
        source
    };
    let digest = Sha256::digest(format!("{VERSION}:{physical}:{normalized}"));
    Identity {
        fingerprint: format!("host:{VERSION}:{}", hex::encode(digest)),
        fingerprint_version: VERSION.into(),
        fingerprint_source: source.into(),
        execution_context: ExecutionContext {
            id: String::new(),
            kind: String::new(),
            display_name: String::new(),
        },
    }
}

fn read_windows_machine_guid() -> Result<String, String> {
    let powershell_args = [
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        "(Get-ItemProperty -Path 'HKLM:\\SOFTWARE\\Microsoft\\Cryptography' -Name MachineGuid).MachineGuid",
    ];
    let reg_args = [
        "query",
        "HKLM\\SOFTWARE\\Microsoft\\Cryptography",
        "/v",
        "MachineGuid",
    ];
    // systemd services in WSL lack the interop PATH entries; try the stable
    // mounted locations first, then PATH lookup.
    let candidates: Vec<(&str, &[&str])> = vec![
        (
            "/mnt/c/Windows/System32/WindowsPowerShell/v1.0/powershell.exe",
            &powershell_args,
        ),
        ("/mnt/c/Windows/System32/reg.exe", &reg_args),
        ("/mnt/c/Windows/system32/reg.exe", &reg_args),
        ("powershell.exe", &powershell_args),
        ("reg.exe", &reg_args),
        ("reg", &reg_args),
    ];
    let mut last_err: Option<String> = None;
    for (program, args) in candidates {
        let output = match Command::new(program).args(args).output() {
            Ok(o) if o.status.success() => o,
            Ok(o) => {
                last_err = Some(format!("exit status {}", o.status.code().unwrap_or(-1)));
                continue;
            }
            Err(e) => {
                // Go: PATH lookup failures and fork/exec failures read differently.
                last_err = Some(if program.contains('/') {
                    format!("fork/exec {program}: {}", goerr::errno_text(&e))
                } else {
                    format!(
                        "exec: {}: executable file not found in $PATH",
                        goerr::quote(program)
                    )
                });
                continue;
            }
        };
        let text = String::from_utf8_lossy(&output.stdout);
        if args.first() == Some(&"-NoProfile") {
            let value = text.trim();
            if !value.is_empty() {
                return Ok(value.to_string());
            }
        }
        for line in text.split('\n') {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 3 && parts[0].eq_ignore_ascii_case("MachineGuid") {
                return Ok(parts[parts.len() - 1].to_string());
            }
        }
    }
    match last_err {
        Some(e) => Err(format!("MachineGuid not found: {e}")),
        None => Err("MachineGuid not found".into()),
    }
}

fn cache_path(env: &Env) -> Option<PathBuf> {
    let base = env.get("XDG_STATE_HOME");
    let base = if base.trim().is_empty() {
        let home = env.get("HOME");
        if home.is_empty() {
            return None;
        }
        PathBuf::from(home).join(".local").join("state")
    } else {
        PathBuf::from(base.trim())
    };
    Some(base.join("opute").join("host-fingerprint.json"))
}

fn read_cached_machine_guid(env: &Env, source: &str) -> Option<String> {
    let raw = std::fs::read(cache_path(env)?).ok()?;
    let entry: serde_json::Value = serde_json::from_slice(&raw).ok()?;
    if entry.get("fingerprintVersion")?.as_str()? != VERSION
        || entry.get("source")?.as_str()? != source
    {
        return None;
    }
    let value = entry.get("value")?.as_str()?.trim().to_string();
    (!value.is_empty()).then_some(value)
}

/// Best effort, like Go: a host that just read its identity must not fail to
/// start because the cache is unwritable.
fn write_cached_machine_guid(env: &Env, source: &str, value: &str) {
    use std::os::unix::fs::PermissionsExt;
    let Some(path) = cache_path(env) else { return };
    let Some(dir) = path.parent() else { return };
    if goerr::mkdir_all(dir, 0o700).is_err() {
        return;
    }
    let raw =
        serde_json::json!({"fingerprintVersion": VERSION, "source": source, "value": value.trim()});
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, raw.to_string()).is_err() {
        return;
    }
    let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    if std::fs::rename(&tmp, &path).is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
}

#[cfg(test)]
pub fn test_identity() -> Identity {
    let mut identity = format_identity(SOURCE_LINUX_MACHINE_ID, "0123456789abcdef");
    identity.execution_context = ExecutionContext {
        id: "native-linux".into(),
        kind: "native-linux".into(),
        display_name: "Linux".into(),
    };
    identity
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_digest_matches_go_formula() {
        // sha256("v2:linux-machine-id:0123456789abcdef")
        let id = format_identity(SOURCE_LINUX_MACHINE_ID, " 0123456789ABCDEF ");
        let expected = hex::encode(Sha256::digest("v2:linux-machine-id:0123456789abcdef"));
        assert_eq!(id.fingerprint, format!("host:v2:{expected}"));
        let wsl = format_identity(SOURCE_WINDOWS_MACHINE_GUID_VIA_WSL, "GUID");
        let expected = hex::encode(Sha256::digest("v2:windows-machine-guid:guid"));
        assert_eq!(wsl.fingerprint, format!("host:v2:{expected}"));
        assert_eq!(wsl.fingerprint_source, SOURCE_WINDOWS_MACHINE_GUID_VIA_WSL);
    }
}
