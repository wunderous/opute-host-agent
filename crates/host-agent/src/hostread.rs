//! The host domain's `normal`-class reads: `inspect_host_file`
//! (`internal/domain/host/file.go`) and `probe_http_endpoint`
//! (`internal/domain/host/http_probe.go`). Both run only after M4 admission
//! has reserved a normal slot.

use serde_json::{json, Map, Value as J};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::time::Duration;

/// `filepath.Clean` for a Unix path.
fn clean(p: &str) -> String {
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
fn http_authority(endpoint: &str) -> Option<(String, String)> {
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
fn dial_address(scheme: &str, host: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

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
