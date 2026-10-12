//! The virtualization stack's lifecycle on the host
//! (`internal/domain/host/tool.go:InstallIncusStack`, `incus_uninstall.go`):
//! the Zabbly repository, the packages, admin access, the daemon and the
//! container runtime defaults on install; on uninstall, a fail-closed purge
//! that refuses while any instance remains.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value as J};

use crate::config::Env;
use crate::hostexec::{combined_output, run_privileged_package};
use crate::hostfiles::{require_shared_host_owner, Owner};
use crate::hostobs::look_path;
use crate::incus::{first_non_empty, Incus};

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

/// `exec.Command(...).CombinedOutput()` with no deadline: the output, or
/// `None` when the command failed.
fn output_ok(env: &Env, argv: &[&str]) -> Option<String> {
    let (output, result) = combined_output(env, &strings(argv), None);
    result
        .ok()
        .map(|()| String::from_utf8_lossy(&output).into_owned())
}

/// `packageAvailable`.
fn package_available(env: &Env, name: &str) -> bool {
    output_ok(env, &["apt-cache", "show", name]).is_some_and(|out| out.contains("Package:"))
}

/// `aptCandidate`: the first match of `(?m)^\s*Candidate:\s*(\S+)` in
/// `apt-cache policy` (RE2 `\s` is `[\t\n\f\r ]`, so the value may follow
/// on a later line).
fn apt_candidate(env: &Env, package: &str) -> String {
    let Some(out) = output_ok(env, &["apt-cache", "policy", package]) else {
        return String::new();
    };
    candidate_from(&out)
}

fn candidate_from(out: &str) -> String {
    let space = |b: u8| matches!(b, b'\t' | b'\n' | b'\x0c' | b'\r' | b' ');
    let bytes = out.as_bytes();
    let mut from = 0;
    while let Some(found) = out[from..].find("Candidate:") {
        let at = from + found;
        let line_start = out[..at].rfind('\n').map_or(0, |n| n + 1);
        if bytes[line_start..at].iter().all(|b| space(*b)) {
            let mut k = at + "Candidate:".len();
            while k < bytes.len() && space(bytes[k]) {
                k += 1;
            }
            let end = (k..bytes.len())
                .find(|&e| space(bytes[e]))
                .unwrap_or(bytes.len());
            if end > k {
                return out[k..end].to_string();
            }
        }
        from = at + 1;
    }
    String::new()
}

/// `incusCandidateSatisfies`: the candidate, without its epoch, starts with
/// the requested major version.
fn candidate_satisfies(candidate: &str, requested: &str) -> bool {
    let major = requested.trim().split('.').next().unwrap_or("");
    let mut version = candidate.trim();
    if let Some((_, rest)) = version.split_once(':') {
        version = rest;
    }
    version.starts_with(major)
}

/// `virtualizationVersions`: each version recorded only when its command
/// succeeds. (Go ranges over a map; the result is a map either way.)
pub(crate) fn virtualization_versions(env: &Env) -> Map<String, J> {
    let mut out = Map::new();
    for (key, argv) in [
        ("incus", &["incus", "version"][..]),
        ("qemu", &["qemu-system-x86_64", "--version"][..]),
        (
            "virglrenderer",
            &["dpkg-query", "-W", "-f=${Version}", "virglrenderer2"][..],
        ),
    ] {
        if let Some(text) = output_ok(env, argv) {
            out.insert(key.into(), J::from(text.trim()));
        }
    }
    out
}

/// `currentUserName`: the account name, else "opute".
fn current_user_name() -> String {
    match nix::unistd::User::from_uid(nix::unistd::getuid()) {
        Ok(Some(user)) if !user.name.is_empty() => user.name,
        _ => "opute".into(),
    }
}

/// `os.CreateTemp("", pattern)`: in `$TMPDIR`, else /tmp.
fn create_temp(env: &Env, prefix: &str) -> Result<(std::fs::File, String), String> {
    let dir = env.get("TMPDIR");
    let dir = if dir.is_empty() {
        "/tmp".to_string()
    } else {
        dir
    };
    crate::goerr::create_temp(&dir, prefix, "")
}

struct Remove(String);
impl Drop for Remove {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// `configureZabblyIncusRepository`: the signed apt source for `channel`.
fn configure_repository(env: &Env, deadline: Instant, channel: &str) -> Result<(), String> {
    use std::io::Write;
    let (key_file, key_path) = create_temp(env, "opute-zabbly-key-")?;
    drop(key_file);
    let _key = Remove(key_path.clone());
    let (output, result) = combined_output(
        env,
        &strings(&[
            "curl",
            "-fsSL",
            "https://pkgs.zabbly.com/key.asc",
            "-o",
            &key_path,
        ]),
        Some(deadline),
    );
    if result.is_err() {
        return Err(format!(
            "download repository key: {}",
            String::from_utf8_lossy(&output).trim()
        ));
    }
    let armored = std::fs::read(&key_path)
        .is_ok_and(|key| String::from_utf8_lossy(&key).contains("BEGIN PGP PUBLIC KEY BLOCK"));
    if !armored {
        return Err("repository key did not look like an armored OpenPGP key".into());
    }
    let os_release = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
    let codename = os_release.split('\n').find_map(|line| {
        let value = line.strip_prefix("VERSION_CODENAME=")?;
        let value: String = value
            .chars()
            .take_while(|c| !c.is_ascii_whitespace())
            .collect();
        (!value.is_empty()).then_some(value)
    });
    let Some(codename) = codename else {
        return Err("could not resolve Ubuntu VERSION_CODENAME".into());
    };
    // `exec.Command("dpkg", "--print-architecture").Output()`: stdout only.
    let arch = dpkg_architecture(env).map_err(|e| format!("resolve dpkg architecture: {e}"))?;
    let (mut source, source_path) = create_temp(env, "opute-zabbly-source-")?;
    let _source = Remove(source_path.clone());
    source
        .write_all(
            format!(
                "Enabled: yes\nTypes: deb\nURIs: https://pkgs.zabbly.com/incus/{channel}\nSuites: {}\nComponents: main\nArchitectures: {}\nSigned-By: /etc/apt/keyrings/zabbly.asc\n",
                codename.trim(),
                arch.trim()
            )
            .as_bytes(),
        )
        .map_err(|e| crate::goerr::path_error("write", std::path::Path::new(&source_path), &e).0)?;
    drop(source);
    run_privileged_package(env, deadline, "install", &["-d", "/etc/apt/keyrings"])?;
    run_privileged_package(
        env,
        deadline,
        "install",
        &["-m", "0644", &key_path, "/etc/apt/keyrings/zabbly.asc"],
    )?;
    run_privileged_package(
        env,
        deadline,
        "install",
        &[
            "-m",
            "0644",
            &source_path,
            "/etc/apt/sources.list.d/zabbly-incus.sources",
        ],
    )
}

/// `exec.Command("dpkg", "--print-architecture").Output()`.
fn dpkg_architecture(env: &Env) -> Result<String, String> {
    let Some(program) = look_path(env, "dpkg") else {
        return Err("exec: \"dpkg\": executable file not found in $PATH".into());
    };
    let output = std::process::Command::new(&program)
        .arg("--print-architecture")
        .env_clear()
        .envs(env.pairs())
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output()
        .map_err(|e| {
            format!(
                "fork/exec {}: {}",
                program.display(),
                crate::goerr::errno_text(&e)
            )
        })?;
    if !output.status.success() {
        return Err(crate::hostexec::exit_text(output.status));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// `incusProfileHasDevice`.
fn profile_has_device(profile: &str, device: &str) -> bool {
    let want = format!("{}:", device.trim());
    profile.split('\n').any(|line| line.trim() == want)
}

/// One provider command of the runtime defaults, failing with `what`.
fn provider(
    incus: &Incus,
    args: &[&str],
    timeout: Duration,
    what: &str,
    fallback: &str,
) -> Result<String, String> {
    match incus.run(args, timeout) {
        Ok(res) if res.exit_code == 0 => Ok(res.stdout),
        Ok(res) => Err(format!(
            "{what}: {}",
            first_non_empty(&[&res.stderr, &res.stdout, fallback])
        )),
        Err(e) => Err(format!("{what}: {e}")),
    }
}

/// `ensureIncusContainerRuntime`: the default storage pool, the managed
/// bridge, and the default profile's root disk and bridged NIC.
fn ensure_container_runtime(incus: &Incus) -> Result<(), String> {
    let network = if incus.network_name.is_empty() {
        "incusbr0"
    } else {
        &incus.network_name
    };
    let address = if incus.network_address.is_empty() {
        "10.0.100.1/24"
    } else {
        &incus.network_address
    };
    let short = Duration::from_secs(30);
    let long = Duration::from_secs(120);
    let storage = provider(
        incus,
        &["storage", "list", "--format", "json"],
        short,
        "inspect storage pools",
        "incus storage list failed",
    )?;
    if !storage.contains(r#""name":"default""#) {
        provider(
            incus,
            &["storage", "create", "default", "dir"],
            long,
            "create default storage pool",
            "incus storage create failed",
        )?;
    }
    let networks = provider(
        incus,
        &["network", "list", "--format", "json"],
        short,
        "inspect networks",
        "incus network list failed",
    )?;
    if !networks.contains(&format!(r#""name":"{network}""#)) {
        provider(
            incus,
            &[
                "network",
                "create",
                network,
                &format!("ipv4.address={address}"),
                "ipv4.nat=true",
                "ipv6.address=none",
            ],
            long,
            "create Incus container network",
            "incus network create failed",
        )?;
    }
    let profile = provider(
        incus,
        &["profile", "device", "show", "default"],
        short,
        "inspect default profile",
        "incus profile device show failed",
    )?;
    if !profile_has_device(&profile, "root") {
        provider(
            incus,
            &[
                "profile",
                "device",
                "add",
                "default",
                "root",
                "disk",
                "path=/",
                "pool=default",
            ],
            long,
            "attach default root disk",
            "incus profile device add failed",
        )?;
    }
    if !profile_has_device(&profile, "eth0") {
        provider(
            incus,
            &[
                "profile",
                "device",
                "add",
                "default",
                "eth0",
                "nic",
                "nictype=bridged",
                &format!("parent={network}"),
                "name=eth0",
            ],
            long,
            "attach default container network",
            "incus profile network device add failed",
        )?;
    } else {
        for (key, value) in [
            ("nictype", "bridged"),
            ("parent", network),
            ("name", "eth0"),
        ] {
            provider(
                incus,
                &["profile", "device", "set", "default", "eth0", key, value],
                long,
                &format!("reconcile default container network {key}"),
                "incus profile network device update failed",
            )?;
        }
    }
    Ok(())
}

/// The `install_incus_stack` arguments.
pub struct InstallArgs {
    pub package_name: String,
    pub qemu_package: String,
    pub gpu_packages: Vec<String>,
    pub channel: String,
    pub version: String,
    pub install_qemu: bool,
}

/// `Service.InstallIncusStack`: packages are upgraded; instances and the
/// storage pool are never torn down.
pub fn install_incus_stack(
    env: &Env,
    owner: &Owner,
    incus: &Incus,
    args: &InstallArgs,
) -> Result<J, String> {
    require_shared_host_owner(
        owner.instance_id,
        owner.owner_instance,
        "install_incus_stack",
    )?;
    let default = |value: &str, fallback: &str| {
        let value = value.trim();
        if value.is_empty() {
            fallback.to_string()
        } else {
            value.to_string()
        }
    };
    let package = default(&args.package_name, "incus");
    let qemu = default(&args.qemu_package, "qemu-system-x86");
    let channel = default(&args.channel, "stable");
    if !["stable", "lts-7.0", "lts-6.0"].contains(&channel.as_str()) {
        return Err("incusChannel must be stable, lts-7.0, or lts-6.0".into());
    }
    let mut packages = vec![package.clone()];
    if args.install_qemu {
        packages.push(qemu);
    }
    let mut optional: BTreeMap<String, String> = BTreeMap::new();
    for candidate in ["virglrenderer2", "libvirglrenderer1"] {
        if package_available(env, candidate) {
            packages.push(candidate.into());
            optional.insert("virglrenderer".into(), candidate.into());
            break;
        }
    }
    for gpu in &args.gpu_packages {
        let gpu = gpu.trim();
        if gpu.is_empty() {
            continue;
        }
        if package_available(env, gpu) {
            packages.push(gpu.into());
            optional.insert(gpu.into(), "available".into());
        } else {
            optional.insert(gpu.into(), "unavailable_in_configured_repositories".into());
        }
    }
    if look_path(env, "apt-get").is_none() {
        return Err("apt-get is required to install Incus/QEMU prerequisites".into());
    }
    let deadline = Instant::now() + Duration::from_secs(15 * 60);
    configure_repository(env, deadline, &channel)
        .map_err(|e| format!("configure Incus package repository: {e}"))?;
    run_privileged_package(env, deadline, "apt-get", &["update"])
        .map_err(|e| format!("update virtualization packages: {e}"))?;
    run_privileged_package(env, deadline, "dpkg", &["--configure", "-a"])
        .map_err(|e| format!("repair interrupted package state: {e}"))?;
    let version = default(&args.version, "7.2");
    // Zabbly's stable channel moves within the requested major line.
    let candidate = apt_candidate(env, &package);
    if !candidate.is_empty() && !candidate_satisfies(&candidate, &version) {
        return Err(format!(
            "Incus repository candidate {} does not satisfy requested version {}",
            crate::goerr::quote(&candidate),
            crate::goerr::quote(&version)
        ));
    }
    let mut install: Vec<String> = strings(&["install", "-y"]);
    install.extend(packages.iter().cloned());
    let candidate = apt_candidate(env, &package);
    if !candidate.is_empty() {
        install.push(format!("{package}={candidate}"));
    }
    let install: Vec<&str> = install.iter().map(String::as_str).collect();
    run_privileged_package(env, deadline, "apt-get", &install)
        .map_err(|e| format!("install virtualization packages: {e}"))?;
    // The package creates incus-admin; a root agent needs no membership.
    let user = current_user_name();
    if !user.is_empty() && user != "root" {
        run_privileged_package(env, deadline, "usermod", &["-aG", "incus-admin", &user])
            .map_err(|e| format!("grant Incus admin access: {e}"))?;
    }
    run_privileged_package(
        env,
        deadline,
        "systemctl",
        &["enable", "--now", "incus.service"],
    )
    .map_err(|e| format!("start Incus daemon: {e}"))?;
    ensure_container_runtime(incus)
        .map_err(|e| format!("initialize Incus container runtime: {e}"))?;
    let mut result = virtualization_versions(env);
    result.insert("optionalPackages".into(), json!(optional));
    Ok(J::Object(result))
}

/// `remainingIncusInstances`: Incus's own list, not the agent's view.
fn remaining_instances(env: &Env, incus: &Incus) -> Result<Vec<String>, String> {
    let listed = match incus.run(&["list", "--format", "json"], Duration::from_secs(60)) {
        Ok(res) if res.exit_code == 0 => res,
        Ok(res) => {
            return Err(format!(
                "inspect remaining Incus instances: {}",
                first_non_empty(&[&res.stderr, &res.stdout, "incus list failed"]).trim()
            ))
        }
        // A daemon that is already gone has no instances to protect.
        Err(_) if look_path(env, "incus").is_none() => return Ok(Vec::new()),
        Err(e) => return Err(format!("inspect remaining Incus instances: {e}")),
    };
    let names = crate::gojson::unmarshal_names(listed.stdout.trim().as_bytes())
        .map_err(|e| format!("decode Incus instance list: {e}"))?;
    Ok(names
        .iter()
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
        .collect())
}

/// `installedIncusPackages`: the installed members of the Incus family.
fn installed_packages(env: &Env) -> Vec<String> {
    let (output, result) = combined_output(
        env,
        &strings(&[
            "dpkg-query",
            "-W",
            "-f=${Package} ${Status}\n",
            "incus",
            "incus-base",
            "incus-client",
            "incus-agent",
            "incus-ui-canonical",
        ]),
        None,
    );
    if result.is_err() && output.is_empty() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output)
        .split('\n')
        .filter_map(|line| {
            let fields: Vec<&str> = line.split_whitespace().collect();
            (fields.len() >= 4 && fields[fields.len() - 1] == "installed")
                .then(|| fields[0].to_string())
        })
        .collect()
}

/// `Service.UninstallIncusStack`: fail-closed while any instance remains;
/// state is discarded only on request.
pub fn uninstall_incus_stack(
    env: &Env,
    owner: &Owner,
    incus: &Incus,
    confirm: bool,
    remove_state: bool,
    keep_repository: bool,
) -> Result<J, String> {
    require_shared_host_owner(
        owner.instance_id,
        owner.owner_instance,
        "uninstall_incus_stack",
    )?;
    if !confirm {
        return Err("uninstall_incus_stack requires confirm=true; it purges the virtualization stack from this host".into());
    }
    let mut result = json!({"removeState": remove_state, "keptRepository": keep_repository});
    if look_path(env, "incus").is_none() {
        result["alreadyAbsent"] = J::Bool(true);
        result["purgedPackages"] = json!([]);
        return Ok(result);
    }
    let remaining = remaining_instances(env, incus)?;
    if !remaining.is_empty() {
        return Err(format!(
            "refusing to uninstall Incus while {} instance(s) remain ({}); delete them first",
            remaining.len(),
            remaining.join(", ")
        ));
    }
    let deadline = Instant::now() + Duration::from_secs(15 * 60);
    let mut stopped = Vec::new();
    for unit in [
        "incus.service",
        "incus.socket",
        "incus-user.service",
        "incus-user.socket",
        "incus-startup.service",
    ] {
        if run_privileged_package(env, deadline, "systemctl", &["disable", "--now", unit]).is_ok() {
            stopped.push(unit);
        }
    }
    result["stoppedUnits"] = json!(stopped);
    let packages = installed_packages(env);
    if !packages.is_empty() {
        if look_path(env, "apt-get").is_none() {
            return Err("apt-get is required to purge Incus packages".into());
        }
        let mut purge = vec!["purge", "-y"];
        purge.extend(packages.iter().map(String::as_str));
        run_privileged_package(env, deadline, "apt-get", &purge)
            .map_err(|e| format!("purge Incus packages: {e}"))?;
        run_privileged_package(env, deadline, "apt-get", &["autoremove", "-y", "--purge"])
            .map_err(|e| format!("autoremove Incus dependencies: {e}"))?;
    }
    result["purgedPackages"] = json!(packages);
    if !keep_repository {
        for path in [
            "/etc/apt/sources.list.d/zabbly-incus.sources",
            "/etc/apt/keyrings/zabbly.asc",
        ] {
            run_privileged_package(env, deadline, "rm", &["-f", path])
                .map_err(|e| format!("remove {path}: {e}"))?;
        }
        run_privileged_package(env, deadline, "apt-get", &["update"]).map_err(|e| {
            format!("refresh package indexes after removing the Incus repository: {e}")
        })?;
    }
    if remove_state {
        run_privileged_package(env, deadline, "rm", &["-rf", "/var/lib/incus"])
            .map_err(|e| format!("remove Incus state directory: {e}"))?;
    }
    result["incusPresent"] = J::Bool(look_path(env, "incus").is_some());
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incus_candidate_satisfies_go_cases() {
        assert!(candidate_satisfies("1:7.3-ubuntu26.04-202607310350", "7.2"));
        assert!(candidate_satisfies("7.2-ubuntu26.04", "7.2"));
        assert!(!candidate_satisfies("1:6.2-ubuntu26.04", "7.2"));
        assert!(!candidate_satisfies("1:7.3-ubuntu26.04", "6.0"));
    }

    #[test]
    fn apt_candidates_follow_the_go_regexp() {
        assert_eq!(
            candidate_from("incus:\n  Installed: (none)\n  Candidate: 1:7.3-1\n"),
            "1:7.3-1"
        );
        assert_eq!(candidate_from("  Candidate:\n\n  7.2\n"), "7.2");
        assert_eq!(candidate_from("x Candidate: 1\n"), "");
    }

    #[test]
    fn incus_profile_has_device_go_cases() {
        let profile = "devices:\n  eth0:\n    name: eth0\n  root:\n    path: /\n";
        assert!(profile_has_device(profile, "eth0"));
        assert!(profile_has_device(profile, "root"));
        assert!(!profile_has_device(profile, "gpu"));
    }
}
