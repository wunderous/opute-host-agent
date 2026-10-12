//! `ensure_host_artifact` (`internal/domain/host/artifact.go`): a
//! caller-declared HTTPS artifact downloaded beneath the account home and
//! verified against its SHA-256 before it is installed.
//!
//! The client follows Go's `http.Client` as the artifact tool configures it:
//! system roots (honouring `SSL_CERT_FILE` and `SSL_CERT_DIR`), a five-minute
//! timeout, and redirects followed only while they stay HTTPS. A custom
//! `CheckRedirect` replaces Go's default ten-redirect limit, so none applies.

use crate::goerr::{errno_text, path_error};
use crate::hostread::{home_dir, host_owned_path};
use serde_json::{json, Value as J};
use sha2::{Digest, Sha256};
use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

const MAX_ARTIFACT_BYTES: u64 = 4 * 1024 * 1024 * 1024;

/// `validateHostArtifactURI`: `url.Parse` succeeds, the scheme is https,
/// there is a host, and there is no userinfo.
pub(crate) fn valid_https_uri(raw: &str) -> bool {
    if raw.bytes().any(|b| b < 0x20 || b == 0x7f) {
        return false;
    }
    let Some((scheme, rest)) = raw.split_once(':') else {
        return false;
    };
    let scheme_ok = scheme
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c));
    if !scheme_ok || !scheme.eq_ignore_ascii_case("https") {
        return false;
    }
    let Some(rest) = rest.strip_prefix("//") else {
        return false;
    };
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    if authority.contains('@') {
        return false;
    }
    // Go rejects a port that is not all digits.
    let host = if let Some(stripped) = authority.strip_prefix('[') {
        match stripped.split_once(']') {
            Some((_, after)) if after.is_empty() || after.starts_with(':') => {
                let port = after.trim_start_matches(':');
                if !port.chars().all(|c| c.is_ascii_digit()) {
                    return false;
                }
                authority
            }
            _ => return false,
        }
    } else {
        match authority.rsplit_once(':') {
            Some((host, port)) => {
                if !port.chars().all(|c| c.is_ascii_digit()) {
                    return false;
                }
                host
            }
            None => authority,
        }
    };
    !host.is_empty() && valid_escapes(raw)
}

/// Every `%` starts a two-digit hex escape, as `url.Parse` requires.
fn valid_escapes(raw: &str) -> bool {
    let b = raw.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            if i + 2 >= b.len() || !(b[i + 1].is_ascii_hexdigit() && b[i + 2].is_ascii_hexdigit()) {
                return false;
            }
            i += 3;
        } else {
            i += 1;
        }
    }
    true
}

/// `normalizeSHA256`.
pub(crate) fn normalize_sha256(raw: &str) -> Result<String, String> {
    let value = raw.strip_prefix("sha256:").unwrap_or(raw).trim();
    if value.len() != 64 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("host artifact sha256 must be a 64-character hexadecimal digest".into());
    }
    Ok(format!("sha256:{}", value.to_ascii_lowercase()))
}

fn file_sha256(path: &str) -> Result<String, String> {
    let mut file = std::fs::File::open(path).map_err(|e| {
        format!(
            "open host artifact: {}",
            path_error("open", Path::new(path), &e).0
        )
    })?;
    let mut hasher = Sha256::new();
    io::copy(&mut file, &mut hasher).map_err(|e| {
        format!(
            "hash host artifact: {}",
            path_error("read", Path::new(path), &e).0
        )
    })?;
    Ok(format!("sha256:{}", hex::encode(hasher.finalize())))
}

/// Go's root pool: `SSL_CERT_FILE` / `SSL_CERT_DIR`, else the system store.
fn tls_config() -> Arc<rustls::ClientConfig> {
    let mut roots = rustls::RootCertStore::empty();
    for cert in rustls_native_certs::load_native_certs().certs {
        let _ = roots.add(cert);
    }
    Arc::new(
        rustls::ClientConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .expect("ring supports the default protocol versions")
        .with_root_certificates(roots)
        .with_no_client_auth(),
    )
}

/// The `*url.Error` cause Go reports for a failed request.
fn transport_cause(url: &str, err: &ureq::Transport) -> String {
    use std::error::Error as _;
    let mut source = err.source();
    while let Some(e) = source {
        if let Some(tls) = e.downcast_ref::<rustls::Error>() {
            return tls_cause(tls);
        }
        if let Some(io) = e.downcast_ref::<io::Error>() {
            if let Some(tls) = io.get_ref().and_then(|i| i.downcast_ref::<rustls::Error>()) {
                return tls_cause(tls);
            }
            if matches!(
                io.kind(),
                io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
            ) {
                return "context deadline exceeded (Client.Timeout exceeded while awaiting headers)"
                    .into();
            }
            if io.raw_os_error().is_some() {
                return format!(
                    "dial tcp {}: connect: {}",
                    dial_address(url),
                    errno_text(io)
                );
            }
        }
        source = e.source();
    }
    if err.kind() == ureq::ErrorKind::Dns {
        let address = dial_address(url);
        let host = address
            .rsplit_once(':')
            .map_or(address.as_str(), |(h, _)| h);
        return format!("dial tcp: lookup {host}: no such host");
    }
    err.to_string()
}

fn tls_cause(e: &rustls::Error) -> String {
    use rustls::{CertificateError, Error};
    let x509 = match e {
        Error::InvalidCertificate(CertificateError::UnknownIssuer) => {
            "x509: certificate signed by unknown authority".to_string()
        }
        Error::InvalidCertificate(CertificateError::Expired)
        | Error::InvalidCertificate(CertificateError::NotValidYet) => {
            "x509: certificate has expired or is not yet valid".to_string()
        }
        Error::InvalidCertificate(CertificateError::NotValidForName)
        | Error::InvalidCertificate(CertificateError::NotValidForNameContext { .. }) => {
            "x509: certificate is not valid for the requested name".to_string()
        }
        other => return format!("tls: {other}"),
    };
    format!("tls: failed to verify certificate: {x509}")
}

/// The address Go dials: `host:port` with the scheme's default port.
fn dial_address(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(u) => {
            let host = match u.host() {
                Some(url::Host::Ipv6(ip)) => format!("[{ip}]"),
                Some(h) => h.to_string(),
                None => String::new(),
            };
            format!("{host}:{}", u.port_or_known_default().unwrap_or(443))
        }
        Err(_) => url.to_string(),
    }
}

/// One GET per hop; a redirect is followed only while it stays HTTPS.
fn fetch(agent: &ureq::Agent, uri: &str) -> Result<ureq::Response, String> {
    let mut current = uri.to_string();
    loop {
        let response = match agent.get(&current).call() {
            Ok(response) | Err(ureq::Error::Status(_, response)) => response,
            Err(ureq::Error::Transport(t)) => {
                return Err(format!(
                    "download host artifact: Get \"{current}\": {}",
                    transport_cause(&current, &t)
                ))
            }
        };
        let redirect = matches!(response.status(), 301 | 302 | 303 | 307 | 308);
        let location = response.header("Location").map(str::to_string);
        match (redirect, location) {
            (true, Some(location)) if !location.is_empty() => {
                let next = url::Url::parse(&current)
                    .and_then(|base| base.join(&location))
                    .map(|u| u.to_string())
                    .map_err(|e| {
                        format!("download host artifact: failed to parse Location header {location:?}: {e}")
                    })?;
                if !next.starts_with("https:") {
                    return Err(format!(
                        "download host artifact: Get \"{next}\": host artifact redirect must remain HTTPS"
                    ));
                }
                current = next;
            }
            _ => return Ok(response),
        }
    }
}

/// `os.CreateTemp(dir, ".opute-host-artifact-*")`.
fn create_temp(dir: &str) -> Result<(std::fs::File, String), String> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut last = None;
    for _ in 0..10_000 {
        let mut seed = [0u8; 4];
        getrandom::getrandom(&mut seed).map_err(|e| e.to_string())?;
        let name = format!("{dir}/.opute-host-artifact-{}", u32::from_ne_bytes(seed));
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&name)
        {
            Ok(file) => return Ok((file, name)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => last = Some(e),
            Err(e) => {
                last = Some(e);
                break;
            }
        }
    }
    let e = last.expect("create_temp tried at least once");
    Err(format!(
        "createtemp {dir}/.opute-host-artifact-*: {}",
        errno_text(&e)
    ))
}

/// `ensureHostArtifactAt`, shared with the public MCP connector install.
pub(crate) fn ensure_at(
    uri: &str,
    destination: &str,
    expected: &str,
    executable: bool,
) -> Result<J, String> {
    use std::os::unix::fs::PermissionsExt;
    match std::fs::metadata(destination) {
        Ok(meta) if meta.is_file() => {
            let observed = file_sha256(destination)?;
            if crate::gojson::equal_fold(&observed, expected) {
                let perm = meta.permissions().mode() & 0o777;
                if executable && perm & 0o111 == 0 {
                    std::fs::set_permissions(
                        destination,
                        std::fs::Permissions::from_mode(perm | 0o755),
                    )
                    .map_err(|e| {
                        format!(
                            "make host artifact executable: {}",
                            path_error("chmod", Path::new(destination), &e).0
                        )
                    })?;
                }
                return Ok(
                    json!({"uri": uri, "destination": destination, "sha256": observed,
                                 "changed": false, "executable": executable}),
                );
            }
        }
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(format!(
                "inspect host artifact destination: {}",
                path_error("stat", Path::new(destination), &e).0
            ))
        }
    }
    let agent = ureq::AgentBuilder::new()
        .redirects(0)
        .timeout(Duration::from_secs(300))
        .tls_config(tls_config())
        .build();
    let response = fetch(&agent, uri)?;
    let status = response.status();
    if !(200..300).contains(&status) {
        return Err(format!("download host artifact: HTTP {status}"));
    }
    if let Some(length) = response
        .header("Content-Length")
        .and_then(|v| v.trim().parse::<u64>().ok())
    {
        if length > MAX_ARTIFACT_BYTES {
            return Err(format!("host artifact exceeds {MAX_ARTIFACT_BYTES} bytes"));
        }
    }
    let dir = Path::new(destination)
        .parent()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| ".".into());
    crate::goerr::mkdir_all(Path::new(&dir), 0o700)
        .map_err(|e| format!("create host artifact directory: {}", e.0))?;
    let (mut file, temporary) =
        create_temp(&dir).map_err(|e| format!("create host artifact temporary: {e}"))?;
    struct Remove<'a>(&'a str);
    impl Drop for Remove<'_> {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(self.0);
        }
    }
    let _cleanup = Remove(&temporary);
    let mut hasher = Sha256::new();
    let mut body = response.into_reader().take(MAX_ARTIFACT_BYTES + 1);
    let mut buf = vec![0u8; 32 * 1024];
    loop {
        let n = match body.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => {
                let cause = if e.kind() == io::ErrorKind::UnexpectedEof {
                    "unexpected EOF".to_string()
                } else {
                    e.to_string()
                };
                return Err(format!("read host artifact: {cause}"));
            }
        };
        file.write_all(&buf[..n]).map_err(|e| {
            format!(
                "read host artifact: {}",
                path_error("write", Path::new(&temporary), &e).0
            )
        })?;
        hasher.update(&buf[..n]);
    }
    drop(file);
    let observed = format!("sha256:{}", hex::encode(hasher.finalize()));
    if !crate::gojson::equal_fold(&observed, expected) {
        return Err(format!(
            "host artifact sha256 mismatch: expected {expected}, got {observed}"
        ));
    }
    let mode = if executable { 0o755 } else { 0o644 };
    std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(mode)).map_err(|e| {
        format!(
            "set host artifact mode: {}",
            path_error("chmod", Path::new(&temporary), &e).0
        )
    })?;
    std::fs::rename(&temporary, destination).map_err(|e| {
        format!(
            "install host artifact: rename {temporary} {destination}: {}",
            errno_text(&e)
        )
    })?;
    Ok(
        json!({"uri": uri, "destination": destination, "sha256": observed,
              "changed": true, "executable": executable}),
    )
}

/// `Service.EnsureHostArtifact`.
pub fn ensure_host_artifact(
    owner: &crate::hostfiles::Owner,
    uri: &str,
    destination: &str,
    sha256: &str,
    executable: bool,
) -> Result<J, String> {
    crate::hostfiles::require_shared_host_owner(
        owner.instance_id,
        owner.owner_instance,
        "ensure_host_artifact",
    )?;
    let uri = uri.trim();
    if !valid_https_uri(uri) {
        return Err("host artifact URI must be an absolute HTTPS URL without credentials".into());
    }
    let expected = normalize_sha256(sha256)?;
    let home = home_dir().map_err(|e| format!("resolve home directory: {e}"))?;
    let destination = host_owned_path(&home, destination)?;
    ensure_at(uri, &destination, &expected, executable)
}

/// `hostAgentArtifactDestAllowed`: beneath the source, a
/// `.opute-host-agent-build` directory in an existing parent, or beneath
/// `~/.opute/host-agent-build`.
fn agent_artifact_dest_allowed(dest: &str, source: &str) -> bool {
    use crate::hostread::clean;
    let (dest, source) = (clean(dest), clean(source));
    if dest == source || dest.starts_with(&format!("{source}/")) {
        return true;
    }
    if dest.ends_with("/.opute-host-agent-build") {
        let parent = Path::new(&dest).parent().map(|p| p.display().to_string());
        if parent.is_some_and(|p| !p.is_empty() && std::fs::metadata(&p).is_ok_and(|m| m.is_dir()))
        {
            return true;
        }
    }
    let home = home_dir().unwrap_or_default();
    if home.is_empty() {
        return false;
    }
    let root = clean(&format!("{home}/.opute/host-agent-build"));
    dest == root || dest.starts_with(&format!("{root}/"))
}

/// `Service.PrepareHostAgentArtifacts`: Linux host-agent binaries built from
/// a caller-selected source tree inside a bounded workload unit.
pub fn prepare_host_agent_artifacts(
    env: &crate::config::Env,
    owner: &crate::hostfiles::Owner,
    source_dir: &str,
    dest_dir: &str,
    archs: &[String],
) -> Result<J, String> {
    use crate::goerr::{quote, shell_quote};
    let (source_dir, dest_dir) = (source_dir.trim(), dest_dir.trim());
    if source_dir.is_empty() {
        return Err("sourceDir is required".into());
    }
    if dest_dir.is_empty() {
        return Err("destDir is required".into());
    }
    let source =
        crate::hostread::absolute(source_dir).map_err(|e| format!("resolve sourceDir: {e}"))?;
    let dest = crate::hostread::absolute(dest_dir).map_err(|e| format!("resolve destDir: {e}"))?;
    if !std::fs::metadata(&source).is_ok_and(|m| m.is_dir()) {
        return Err(format!("sourceDir must be an existing directory: {source}"));
    }
    if !agent_artifact_dest_allowed(&dest, &source) {
        return Err("destDir must be under sourceDir or ~/.opute/host-agent-build".into());
    }
    let defaults = ["amd64".to_string(), "arm64".to_string()];
    let archs = if archs.is_empty() {
        &defaults[..]
    } else {
        archs
    };
    crate::hosttool::ensure_host_tool(env, owner, "go")?;
    crate::goerr::mkdir_all(Path::new(&dest), 0o755)
        .map_err(|e| format!("create destDir: {}", e.0))?;
    let mut built = Vec::new();
    for arch in archs {
        let arch = arch.trim();
        if arch != "amd64" && arch != "arm64" {
            return Err(format!(
                "unsupported arch {} (use amd64 or arm64)",
                quote(arch)
            ));
        }
        let output = if arch == "arm64" {
            "host-agent-linux-arm64"
        } else {
            "host-agent-linux-x64"
        };
        let output_path = crate::hostread::clean(&format!("{dest}/{output}"));
        let script = format!(
            "cd {} && export GOCACHE=\"${{GOCACHE:-$HOME/.cache/go-build}}\" && mkdir -p \"$GOCACHE\" && GOOS=linux GOARCH={arch} go build -buildvcs=false -ldflags={} -o {} github.com/wunderous/host-agents/cmd/opute-host-agent",
            shell_quote(&source),
            shell_quote("-s -w"),
            shell_quote(&output_path),
        );
        let result = crate::hostexec::run_host_workload(
            env,
            &format!("prepare-host-agent-artifacts-{arch}"),
            &["bash".into(), "-lc".into(), script],
            Duration::from_secs(15 * 60),
        )?;
        if result.exit_code != 0 {
            return Err(crate::incus::first_non_empty(&[
                &result.stderr,
                &result.stdout,
                &format!("go build failed for {arch}"),
            ])
            .to_string());
        }
        built.push(output);
    }
    Ok(json!({"sourceDir": source, "destDir": dest, "artifacts": built}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uris_follow_go() {
        assert!(valid_https_uri("https://example.com/a"));
        assert!(valid_https_uri("HTTPS://example.com:8443/a?x#y"));
        assert!(!valid_https_uri("http://example.com/a"));
        assert!(!valid_https_uri("https://u:p@example.com/a"));
        assert!(!valid_https_uri("https:///a"));
        assert!(!valid_https_uri("/a"));
        assert!(!valid_https_uri("https://example.com:x/a"));
        assert!(!valid_https_uri("https://example.com/%zz"));
    }

    #[test]
    fn digests_follow_go() {
        let hex = "A".repeat(64);
        assert_eq!(
            normalize_sha256(&hex).unwrap(),
            format!("sha256:{}", "a".repeat(64))
        );
        assert!(normalize_sha256(&format!("sha256:{hex}")).is_ok());
        assert!(normalize_sha256("abc").is_err());
    }
}
