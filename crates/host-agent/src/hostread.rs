//! The host domain's `normal`-class reads: `inspect_host_file`
//! (`internal/domain/host/file.go`) and `probe_http_endpoint`
//! (`internal/domain/host/http_probe.go`). Both run only after M4 admission
//! has reserved a normal slot.

use serde_json::{json, Map, Value as J};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::time::Duration;

/// `filepath.Clean` for a Unix path.
pub(crate) fn clean(p: &str) -> String {
    if p.is_empty() {
        return ".".into();
    }
    let rooted = p.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                if parts.last().is_some_and(|l| *l != "..") {
                    parts.pop();
                } else if !rooted {
                    parts.push("..");
                }
            }
            s => parts.push(s),
        }
    }
    let joined = parts.join("/");
    match (rooted, joined.is_empty()) {
        (true, _) => format!("/{joined}"),
        (false, true) => ".".into(),
        (false, false) => joined,
    }
}

/// `filepath.Abs(filepath.Clean(p))`.
fn absolute(p: &str) -> Result<String, String> {
    if p.starts_with('/') {
        return Ok(clean(p));
    }
    let cwd = std::env::current_dir().map_err(|e| crate::goerr::errno_text(&e))?;
    Ok(clean(&format!("{}/{p}", cwd.display())))
}

/// `hostHomeDir`: `os.UserHomeDir` ($HOME), then the account database.
fn home_dir() -> Result<String, String> {
    if let Ok(home) = std::env::var("HOME") {
        if !home.trim().is_empty() {
            return Ok(home);
        }
    }
    match nix::unistd::User::from_uid(nix::unistd::getuid()) {
        Ok(Some(user)) if !user.dir.as_os_str().is_empty() => Ok(user.dir.display().to_string()),
        Ok(_) => Err("account home directory is empty".into()),
        Err(e) => Err(e.desc().to_lowercase()),
    }
}

/// `hostOwnedPath`: beneath the home directory, through no symlink.
fn host_owned_path(home: &str, raw: &str) -> Result<String, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("path is required".into());
    }
    let home = absolute(&clean(home)).map_err(|e| format!("resolve home directory: {e}"))?;
    let joined = if raw == "~" || raw.starts_with("~/") {
        format!("{home}/{}", raw.strip_prefix("~/").unwrap_or(raw))
    } else if !raw.starts_with('/') {
        format!("{home}/{raw}")
    } else {
        raw.to_string()
    };
    let path = absolute(&clean(&joined)).map_err(|e| format!("resolve path: {e}"))?;
    // `filepath.Rel` between two clean absolute paths.
    let relative = if path == home {
        ".".to_string()
    } else if home == "/" {
        path[1..].to_string()
    } else if let Some(rest) = path.strip_prefix(&format!("{home}/")) {
        rest.to_string()
    } else {
        return Err("path must be beneath the current user's home directory".into());
    };
    let mut current = home.clone();
    for component in relative.split('/') {
        if component.is_empty() || component == "." {
            continue;
        }
        current = clean(&format!("{current}/{component}"));
        match std::fs::symlink_metadata(&current) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => break,
            Err(e) => {
                return Err(format!(
                    "inspect managed host file path: {}",
                    crate::goerr::path_error("lstat", Path::new(&current), &e)
                ))
            }
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err("managed host file path must not traverse a symlink".into())
            }
            Ok(_) => {}
        }
    }
    Ok(path)
}

/// `systemdUnitPath`.
fn systemd_unit_path(raw: &str) -> Result<String, String> {
    let path = clean(raw.trim());
    if !path.starts_with("/etc/systemd/system/") {
        return Err(
            "system-scoped managed files must be systemd units beneath /etc/systemd/system".into(),
        );
    }
    let unit = path.rsplit('/').next().unwrap_or("");
    let valid = unit.len() > ".service".len()
        && unit.ends_with(".service")
        && unit[..unit.len() - ".service".len()]
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_.@:-".contains(c));
    if !valid {
        return Err("system-scoped managed files must name a .service unit".into());
    }
    Ok(path)
}

/// `managedHostPath`.
fn managed_host_path(scope: &str, raw: &str) -> Result<String, String> {
    match scope.trim().to_lowercase().as_str() {
        "" | "user" => {
            let home = home_dir().map_err(|e| format!("resolve home directory: {e}"))?;
            host_owned_path(&home, raw)
        }
        "system" => systemd_unit_path(raw),
        _ => Err("scope must be user or system".into()),
    }
}

fn sha256_label(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

/// `Service.InspectHostFile`. `expected_content` is taken raw (untrimmed).
pub fn inspect_host_file(
    path: &str,
    scope: &str,
    expected_sha256: &str,
    expected_content: &str,
) -> Result<J, String> {
    use std::os::unix::fs::PermissionsExt;
    let path = managed_host_path(scope, path)?;
    let meta = match std::fs::metadata(&path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(json!({"path": path, "exists": false, "regular": false,
                             "executable": false, "matches": false}))
        }
        Err(e) => {
            return Err(format!(
                "inspect managed host file: {}",
                crate::goerr::path_error("stat", Path::new(&path), &e)
            ))
        }
        Ok(meta) => meta,
    };
    let perm = meta.permissions().mode() & 0o777;
    let regular = meta.file_type().is_file();
    let mut result = Map::new();
    result.insert("path".into(), J::from(path.clone()));
    result.insert("exists".into(), J::Bool(true));
    result.insert("regular".into(), J::Bool(regular));
    result.insert("executable".into(), J::Bool(regular && perm & 0o111 != 0));
    result.insert("mode".into(), J::from(format!("{perm:o}")));
    if regular {
        let content = std::fs::read(&path).map_err(|e| {
            format!(
                "read inspected host file: {}",
                crate::goerr::path_error("open", Path::new(&path), &e)
            )
        })?;
        let observed = sha256_label(&content);
        let mut matches = true;
        let expected = expected_sha256.trim();
        if !expected.is_empty() {
            let expected = format!(
                "sha256:{}",
                expected.strip_prefix("sha256:").unwrap_or(expected)
            );
            matches = expected.eq_ignore_ascii_case(&observed);
        }
        if !expected_content.is_empty() {
            matches = matches
                && observed.eq_ignore_ascii_case(&sha256_label(expected_content.as_bytes()));
        }
        result.insert("sha256".into(), J::from(observed));
        result.insert("matches".into(), J::Bool(matches));
    } else {
        result.insert("matches".into(), J::Bool(false));
    }
    Ok(J::Object(result))
}

/// The scheme and `host[:port]` of an absolute HTTP(S) URL, as `url.Parse`
/// accepts it for `ProbeHTTPEndpoint` (scheme case-folded, userinfo dropped).
pub(crate) fn http_authority(endpoint: &str) -> Option<(String, String)> {
    let (scheme, rest) = endpoint.split_once(':')?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let rest = rest.strip_prefix("//")?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    if host.is_empty() || host.contains(' ') {
        return None;
    }
    Some((scheme, host.to_string()))
}

/// The dial address Go reports: `host:port` with the scheme's default port.
pub(crate) fn dial_address(scheme: &str, host: &str) -> String {
    let bracketed = host.starts_with('[');
    let has_port = if bracketed {
        host.contains("]:")
    } else {
        host.contains(':')
    };
    if has_port {
        host.to_string()
    } else {
        format!("{host}:{}", if scheme == "https" { 443 } else { 80 })
    }
}

/// `net/http`'s `*url.Error` text for a failed IPv4 retry.
fn transport_error(endpoint: &str, address: &str, err: &ureq::Transport) -> String {
    use std::error::Error as _;
    let mut source = err.source();
    while let Some(e) = source {
        if let Some(io) = e.downcast_ref::<std::io::Error>() {
            if io.kind() == std::io::ErrorKind::TimedOut
                || io.kind() == std::io::ErrorKind::WouldBlock
            {
                return format!(
                    "Get \"{endpoint}\": context deadline exceeded (Client.Timeout exceeded while awaiting headers)"
                );
            }
            if io.raw_os_error().is_some() {
                return format!(
                    "Get \"{endpoint}\": dial tcp4 {address}: connect: {}",
                    crate::goerr::errno_text(io)
                );
            }
        }
        source = e.source();
    }
    if err.kind() == ureq::ErrorKind::Dns {
        let host = address.rsplit_once(':').map_or(address, |(h, _)| h);
        return format!("Get \"{endpoint}\": dial tcp4: lookup {host}: no such host");
    }
    format!("Get \"{endpoint}\": {err}")
}

/// `Service.ProbeHTTPEndpoint`: one GET, retried over IPv4 on a transport
/// failure; 2xx/3xx is ready, and 401/403 too when a challenge is accepted.
pub fn probe_http_endpoint(endpoint: &str, accept_challenge: bool) -> Result<J, String> {
    let endpoint = endpoint.trim();
    let Some((scheme, host)) =
        http_authority(endpoint).filter(|_| !endpoint.contains(['\r', '\n', '\0']))
    else {
        return Err("endpoint must be an absolute HTTP(S) URL".into());
    };
    let agent = |ipv4: bool| {
        let mut builder = ureq::AgentBuilder::new()
            .redirects(10)
            .timeout(Duration::from_secs(15));
        if ipv4 {
            builder = builder.resolver(|address: &str| {
                use std::net::ToSocketAddrs;
                address
                    .to_socket_addrs()
                    .map(|addrs| addrs.filter(|a| a.is_ipv4()).collect())
            });
        }
        builder.build()
    };
    let attempt = |ipv4: bool| match agent(ipv4).get(endpoint).call() {
        Ok(response) | Err(ureq::Error::Status(_, response)) => Ok(response),
        Err(ureq::Error::Transport(t)) => Err(Box::new(t)),
    };
    let response = match attempt(false).or_else(|_| attempt(true)) {
        Ok(response) => response,
        Err(t) => {
            let message = transport_error(endpoint, &dial_address(&scheme, &host), &t);
            return Ok(json!({"endpoint": endpoint, "ready": false, "error": message}));
        }
    };
    let status = response.status();
    let mut sink = Vec::new();
    let _ = std::io::Read::read_to_end(
        &mut std::io::Read::take(response.into_reader(), 64 * 1024),
        &mut sink,
    );
    let ready =
        (200..400).contains(&status) || (accept_challenge && (status == 401 || status == 403));
    Ok(json!({"endpoint": endpoint, "statusCode": status, "ready": ready}))
}

const SYSTEMCTL: &str = "/usr/bin/systemctl";

/// `Service.ListHostServices`: systemd units of one scope with their canonical
/// tenant-scoped URIs, each registered as it is listed. Enablement comes from
/// `list-unit-files` and is best-effort: an unknown unit says "unknown".
/// `InspectHostService`: read-only systemd evidence for a caller-owned
/// service. It never starts, stops, enables, or reloads the unit.
pub fn inspect_host_service(
    env: &crate::config::Env,
    service_name: &str,
    scope: &str,
    listen_port: i64,
) -> Result<J, String> {
    let service_name = service_name.trim();
    let safe = !service_name.is_empty()
        && service_name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '@' | ':' | '-'));
    if !safe {
        return Err("serviceName is required and must be a valid systemd unit name".into());
    }
    let scope = scope.trim().to_lowercase();
    let scope = if scope.is_empty() {
        "user".to_string()
    } else {
        scope
    };
    if scope != "user" && scope != "system" {
        return Err("scope must be user or system".into());
    }
    if !(0..=65535).contains(&listen_port) {
        return Err("listenPort must be between 1 and 65535".into());
    }
    let mut prefix = vec![SYSTEMCTL.to_string()];
    if scope == "user" {
        prefix.push("--user".into());
    }
    let run = |args: &[&str]| {
        let mut argv = prefix.clone();
        argv.extend(args.iter().map(|a| a.to_string()));
        crate::hostobs::run_command(env, &argv, Duration::from_secs(15))
    };
    let result = run(&["is-active", service_name]);
    let mut status = result.stdout.trim().to_string();
    if status.is_empty() {
        status = result.stderr.trim().to_string();
    }
    let enabled = run(&["is-enabled", service_name]);
    let mut unit_file_state = enabled.stdout.trim().to_string();
    if unit_file_state.is_empty() {
        unit_file_state = enabled.stderr.trim().to_string();
    }
    // Where the unit lives and what it launches: read-only properties that
    // say whose a unit is.
    let show = run(&[
        "show",
        service_name,
        "-p",
        "FragmentPath",
        "-p",
        "ExecStart",
        "-p",
        "MainPID",
        "--no-pager",
    ]);
    let (fragment_path, exec_start) = if show.exit_code == 0 {
        unit_launch_properties(&show.stdout)
    } else {
        (String::new(), String::new())
    };
    let active = result.exit_code == 0 && status == "active";
    let mut out = json!({
        "serviceName": service_name,
        "scope": scope,
        "status": status,
        "active": active,
        "enabled": enabled.exit_code == 0 && unit_file_state == "enabled",
        "unitFileState": unit_file_state,
        "exitCode": result.exit_code,
        "fragmentPath": fragment_path,
        "execStart": exec_start,
    });
    if listen_port > 0 {
        let mut owned = false;
        if show.exit_code == 0 && active {
            let pid = unit_main_pid(&show.stdout);
            if pid > 0 {
                owned = listener_owned_by_pid(pid, listen_port).unwrap_or(false);
            }
        }
        out["listenPort"] = J::from(listen_port);
        out["listenerOwned"] = J::Bool(owned);
    }
    Ok(out)
}

/// `parseUnitMainPID`.
fn unit_main_pid(output: &str) -> i64 {
    output
        .split('\n')
        .filter_map(|line| line.trim().strip_prefix("MainPID="))
        .filter_map(|value| value.parse::<i64>().ok())
        .find(|pid| *pid > 0)
        .unwrap_or(0)
}

/// `parseUnitLaunchProperties`: FragmentPath and the ExecStart executable.
fn unit_launch_properties(output: &str) -> (String, String) {
    let (mut fragment_path, mut exec_start) = (String::new(), String::new());
    for line in output.split('\n') {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("FragmentPath=") {
            fragment_path = value.trim().to_string();
        } else if line.starts_with("ExecStart=") && exec_start.is_empty() {
            if let Some(index) = line.find("path=") {
                let mut value = &line[index + "path=".len()..];
                if let Some(end) = value.find(" ;") {
                    value = &value[..end];
                }
                exec_start = value.trim().to_string();
            }
        }
    }
    (fragment_path, exec_start)
}

/// `listenerOwnedByPID`: whether `pid` holds a LISTEN socket on `port`.
fn listener_owned_by_pid(pid: i64, port: i64) -> Result<bool, std::io::Error> {
    if pid <= 0 || port <= 0 || port > 65535 {
        return Ok(false);
    }
    let proc_dir = format!("/proc/{pid}");
    let mut inodes = std::collections::HashSet::new();
    for protocol in ["tcp", "tcp6"] {
        let content = match std::fs::read_to_string(format!("{proc_dir}/net/{protocol}")) {
            Ok(content) => content,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && protocol == "tcp6" => continue,
            Err(e) => return Err(e),
        };
        for line in content.split('\n') {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() < 10 || fields[3] != "0A" {
                continue;
            }
            let Some((_, hex_port)) = fields[1].split_once(':') else {
                continue;
            };
            if u16::from_str_radix(hex_port, 16).is_ok_and(|p| i64::from(p) == port) {
                inodes.insert(fields[9].to_string());
            }
        }
    }
    if inodes.is_empty() {
        return Ok(false);
    }
    for fd in std::fs::read_dir(format!("{proc_dir}/fd"))? {
        let Ok(fd) = fd else { continue };
        let Ok(link) = std::fs::read_link(fd.path()) else {
            continue;
        };
        let link = link.to_string_lossy();
        if let Some(inode) = link
            .strip_prefix("socket:[")
            .and_then(|l| l.strip_suffix(']'))
        {
            if inodes.contains(inode) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

pub fn list_host_services(
    env: &crate::config::Env,
    tenant_id: &str,
    scope: &str,
    register: &dyn Fn(&str, Map<String, J>),
) -> Result<J, String> {
    let scope = scope.trim().to_lowercase();
    let scope = if scope.is_empty() {
        "user".to_string()
    } else {
        scope
    };
    if scope != "user" && scope != "system" {
        return Err("scope must be user or system".into());
    }
    let mut prefix = vec![SYSTEMCTL.to_string()];
    if scope == "user" {
        prefix.push("--user".into());
    }
    let run = |args: &[&str]| {
        let mut argv = prefix.clone();
        argv.extend(args.iter().map(|a| a.to_string()));
        crate::hostobs::run_command(env, &argv, Duration::from_secs(15))
    };
    let listed = run(&[
        "list-units",
        "--type=service",
        "--all",
        "--no-pager",
        "--plain",
        "--no-legend",
    ]);
    let files = run(&[
        "list-unit-files",
        "--type=service",
        "--no-pager",
        "--plain",
        "--no-legend",
    ]);
    Ok(host_services_from(
        &listed.stdout,
        &files.stdout,
        tenant_id,
        &scope,
        register,
    ))
}

/// The listing half of `ListHostServices`, over `list-units` and
/// `list-unit-files` output.
fn host_services_from(
    listed: &str,
    files: &str,
    tenant_id: &str,
    scope: &str,
    register: &dyn Fn(&str, Map<String, J>),
) -> J {
    let mut enablement = std::collections::HashMap::new();

    for line in files.split('\n') {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() >= 2 {
            enablement.insert(fields[0].to_string(), fields[1].to_string());
        }
    }
    let mut services = Vec::new();
    for line in listed.split('\n') {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 4 {
            continue;
        }
        let unit = fields[0];
        let Some(service_name) = unit.strip_suffix(".service") else {
            continue;
        };
        let (active_state, sub_state) = (fields[2], fields[3]);
        let status = if !sub_state.is_empty() && sub_state != active_state {
            format!("{active_state}/{sub_state}")
        } else {
            active_state.to_string()
        };
        let Ok(uri) = crate::resource::Uri::new(
            "host-service",
            tenant_id,
            &format!("{scope}/{service_name}"),
        ) else {
            continue;
        };
        let mut coordinates = Map::new();
        coordinates.insert("serviceName".into(), J::from(service_name));
        coordinates.insert("scope".into(), J::from(scope));
        register(&uri.to_string(), coordinates);
        let state = enablement
            .get(unit)
            .cloned()
            .unwrap_or_else(|| "unknown".into());
        services.push(json!({
            "uri": uri.to_string(),
            "serviceName": service_name,
            "scope": scope,
            "status": status,
            "active": active_state == "active",
            "enabled": state == "enabled" || state == "enabled-runtime",
            "enablementState": state,
        }));
    }
    let total = services.len();
    json!({"services": services, "total": total})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_properties_follow_go_parsing() {
        let shown = "MainPID=0\nFragmentPath=/home/u/.config/systemd/user/x.service\n\
                     ExecStart={ path=/usr/bin/x ; argv[]=/usr/bin/x serve ; ignore_errors=no }\n\
                     ExecStart={ path=/second ; argv[]=/second }\nMainPID=42\n";
        assert_eq!(unit_main_pid(shown), 42);
        assert_eq!(
            unit_launch_properties(shown),
            (
                "/home/u/.config/systemd/user/x.service".to_string(),
                "/usr/bin/x".to_string()
            )
        );
        assert_eq!(unit_main_pid("MainPID=nope\n"), 0);
        assert_eq!(
            unit_launch_properties("ExecStart=\n"),
            (String::new(), String::new())
        );
        assert!(!listener_owned_by_pid(0, 80).unwrap());
        assert!(!listener_owned_by_pid(1, 70000).unwrap());
    }

    #[test]
    fn inspect_host_service_rejects_unsafe_input_before_running_systemctl() {
        let env = crate::config::Env::from_pairs(std::iter::empty::<(String, String)>());
        let unsafe_name = inspect_host_service(&env, "x; rm -rf /", "user", 0).unwrap_err();
        assert_eq!(
            unsafe_name,
            "serviceName is required and must be a valid systemd unit name"
        );
        assert_eq!(
            inspect_host_service(&env, "x.service", "global", 0).unwrap_err(),
            "scope must be user or system"
        );
        assert_eq!(
            inspect_host_service(&env, "x.service", "user", 70000).unwrap_err(),
            "listenPort must be between 1 and 65535"
        );
    }

    #[test]
    fn host_services_follow_go_listing_rules() {
        let listed = "ollama.service loaded active running Ollama\n\
            dbus.socket loaded active running D-Bus\n\
            stale.service not-found inactive dead stale.service\n\
            short.service loaded\n";
        let files = "ollama.service enabled enabled\nother.service static -\n";
        let registered = std::cell::RefCell::new(Vec::new());
        let out = host_services_from(listed, files, "local", "user", &|uri, _| {
            registered.borrow_mut().push(uri.to_string())
        });
        assert_eq!(
            out,
            json!({"services": [
                {"uri": "host-service:local:user/ollama", "serviceName": "ollama", "scope": "user",
                 "status": "active/running", "active": true, "enabled": true, "enablementState": "enabled"},
                {"uri": "host-service:local:user/stale", "serviceName": "stale", "scope": "user",
                 "status": "inactive/dead", "active": false, "enabled": false, "enablementState": "unknown"}
            ], "total": 2})
        );
        assert_eq!(
            *registered.borrow(),
            [
                "host-service:local:user/ollama",
                "host-service:local:user/stale"
            ]
        );
    }

    #[test]
    fn filepath_clean_matches_go() {
        for (input, want) in [
            ("/a/b/../c/", "/a/c"),
            ("/../x", "/x"),
            ("a/../../b", "../b"),
            ("", "."),
            ("//a//b", "/a/b"),
            ("/", "/"),
        ] {
            assert_eq!(clean(input), want, "{input}");
        }
    }

    #[test]
    fn managed_paths_stay_home_or_name_a_unit() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path().display().to_string();
        assert_eq!(host_owned_path(&h, "~/a/b").unwrap(), format!("{h}/a/b"));
        assert_eq!(host_owned_path(&h, "rel").unwrap(), format!("{h}/rel"));
        // Go: TrimPrefix("~", "~/") leaves "~", so a bare tilde is a file
        // named "~" in the home directory.
        assert_eq!(host_owned_path(&h, "~").unwrap(), format!("{h}/~"));
        assert_eq!(
            host_owned_path(&h, "/etc/passwd").unwrap_err(),
            "path must be beneath the current user's home directory"
        );
        assert_eq!(
            host_owned_path(&h, "~/../x").unwrap_err(),
            "path must be beneath the current user's home directory"
        );
        assert_eq!(host_owned_path(&h, "  ").unwrap_err(), "path is required");
        std::os::unix::fs::symlink("/etc", home.path().join("link")).unwrap();
        assert_eq!(
            host_owned_path(&h, "~/link/passwd").unwrap_err(),
            "managed host file path must not traverse a symlink"
        );
        assert_eq!(
            systemd_unit_path("/etc/systemd/system/a@b.service").unwrap(),
            "/etc/systemd/system/a@b.service"
        );
        assert!(systemd_unit_path("/etc/systemd/system/a b.service").is_err());
        assert!(systemd_unit_path("/etc/systemd/system/a.timer").is_err());
        assert!(systemd_unit_path("/etc/hosts").is_err());
        assert_eq!(
            managed_host_path("cluster", "x").unwrap_err(),
            "scope must be user or system"
        );
    }

    #[test]
    fn endpoints_must_be_absolute_http() {
        assert_eq!(
            http_authority("HTTP://u:p@127.0.0.1:8/x"),
            Some(("http".into(), "127.0.0.1:8".into()))
        );
        assert_eq!(http_authority("ftp://x"), None);
        assert_eq!(http_authority("http:///x"), None);
        assert_eq!(http_authority("127.0.0.1:80"), None);
        assert_eq!(dial_address("https", "example.test"), "example.test:443");
        assert_eq!(dial_address("http", "[::1]"), "[::1]:80");
        assert_eq!(dial_address("http", "[::1]:9"), "[::1]:9");
    }
}
