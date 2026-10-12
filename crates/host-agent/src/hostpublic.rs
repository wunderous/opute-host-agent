//! Public MCP exposure through a pinned connector
//! (`internal/domain/host/public_mcp.go`, `public_mcp_quick.go`): a stable
//! tunnel whose provider-issued token lives in an Opute-owned 0600 file, and
//! an ephemeral quick tunnel for this agent's loopback endpoint. The edge
//! provider owns DNS, TLS and token issuance; the host owns only the local
//! connector, its files and its service, and proves the endpoint with an
//! authenticated `tools/list`.

use std::io;
use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{json, Value as J};

use crate::config::Env;
use crate::goerr::{errno_text, path_error, shell_quote};
use crate::hostfiles::{ensure_host_file, require_shared_host_owner, Owner};
use crate::hostread::{clean, home_dir, host_owned_path, sha256_label};
use crate::mcpprobe::{is_loopback_ip, probe_authenticated, validate_endpoint, GoUrl};
use crate::plan::runner::RunCtx;

const DEFAULT_ARTIFACT_URI: &str =
    "https://github.com/cloudflare/cloudflared/releases/download/2026.8.2/cloudflared-linux-amd64";
const DEFAULT_ARTIFACT_SHA256: &str =
    "sha256:fcfb02b575a52ca1af2e3267af4e1517bcdeb30ac48c834c69abaed3c0576ad2";
const SERVICE_PREFIX: &str = "opute-cloudflared-";
const QUICK_SERVICE_PREFIX: &str = "opute-cloudflared-quick-";
const CONTRACT_VERSION: &str = "mcp-exposure.v1";
const TOKEN_ENV: &str = "OPUTE_CLOUDFLARED_TUNNEL_TOKEN";
const MANAGED_MARKER: &str = "# Managed by Opute Host Agent: public MCP tunnel\n";
const QUICK_MARKER: &str = "# Managed by Opute Host Agent: public MCP quick tunnel\n";
const PROBE_RETRY_WINDOW: Duration = Duration::from_secs(90);
const PROBE_RETRY_DELAY: Duration = Duration::from_secs(2);
const QUICK_LOG_LIMIT: u64 = 256 * 1024;
const QUICK_POLL_DELAY: Duration = Duration::from_millis(500);
const BINDING_ERROR: &str =
    "bindingId must contain only letters, numbers, dots, colons, underscores, and hyphens";

/// `^[A-Za-z0-9][A-Za-z0-9._:-]{0,254}$`.
fn valid_binding(id: &str) -> bool {
    let b = id.as_bytes();
    !b.is_empty()
        && b.len() <= 255
        && b[0].is_ascii_alphanumeric()
        && b[1..]
            .iter()
            .all(|c| c.is_ascii_alphanumeric() || b"._:-".contains(c))
}

/// `strings.TrimSpace` followed by the first non-empty value.
fn first_non_empty(values: &[&str]) -> String {
    values
        .iter()
        .map(|v| v.trim())
        .find(|v| !v.is_empty())
        .unwrap_or("")
        .to_string()
}

fn is_root() -> bool {
    nix::unistd::geteuid().is_root()
}

/// `normalizePublicMCPInstanceID`: the Platform's service-name derivation,
/// with a digest suffix whenever normalization changed the binding.
fn instance_id(binding: &str) -> String {
    use sha2::{Digest, Sha256};
    let raw = binding.trim().to_lowercase();
    // `[^a-z0-9-]+` → `-`, then trim `-`.
    let mut replaced = String::new();
    let mut in_run = false;
    for c in raw.chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' {
            replaced.push(c);
            in_run = false;
        } else if !in_run {
            replaced.push('-');
            in_run = true;
        }
    }
    let mut normalized = replaced.trim_matches('-').to_string();
    if normalized.is_empty() {
        normalized = "host-agent".into();
    }
    let mut base = normalized.clone();
    if base.len() > 63 {
        base = base[..63].trim_end_matches('-').to_string();
    }
    if raw == base {
        return base;
    }
    let suffix = &hex::encode(Sha256::digest(raw.as_bytes()))[..10];
    let length = (63 - suffix.len() - 1).max(1);
    let mut base = normalized[..length.min(normalized.len())]
        .trim_end_matches('-')
        .to_string();
    if base.is_empty() {
        base = "host-agent".into();
    }
    let mut result = format!("{base}-{suffix}");
    if result.len() > 63 {
        result = result[..63].trim_end_matches('-').to_string();
    }
    result
}

/// `validateHostArtifactURI` then `normalizeSHA256`, with the pinned
/// connector as the default.
fn artifact_source(uri: &str, sha256: &str) -> Result<(String, String), String> {
    let uri = match uri.trim() {
        "" => DEFAULT_ARTIFACT_URI,
        uri => uri,
    };
    if !crate::hostartifact::valid_https_uri(uri) {
        return Err("host artifact URI must be an absolute HTTPS URL without credentials".into());
    }
    let sha = crate::hostartifact::normalize_sha256(&first_non_empty(&[
        sha256,
        DEFAULT_ARTIFACT_SHA256,
    ]))?;
    Ok((uri.to_string(), sha))
}

/// The scope a connector names: `user` by default, `system` only as root.
fn connector_scope(scope: &str, what: &str) -> Result<String, String> {
    let scope = match scope.trim().to_lowercase() {
        s if s.is_empty() => "user".to_string(),
        s => s,
    };
    if scope != "user" && scope != "system" {
        return Err("scope must be user or system".into());
    }
    if scope == "system" && !is_root() {
        return Err(format!("system-scoped public MCP {what} requires root"));
    }
    Ok(scope)
}

fn resolve_home() -> Result<String, String> {
    home_dir().map_err(|e| format!("resolve home directory: {e}"))
}

/// `filepath.Join` of already-clean components.
fn join(parts: &[&str]) -> String {
    clean(&parts.join("/"))
}

/// The origin URL rules both tunnel kinds share.
fn parse_origin(raw: &str) -> Result<GoUrl, String> {
    match GoUrl::parse(raw.trim()) {
        Some(url)
            if !url.host.is_empty()
                && !url.has_user
                && (url.scheme == "http" || url.scheme == "https")
                && url.raw_query.is_empty()
                && url.fragment.is_empty() =>
        {
            Ok(url)
        }
        _ => Err(
            "localTarget must be an absolute HTTP(S) URL without credentials, query, or fragment"
                .into(),
        ),
    }
}

fn trim_trailing_slashes(path: &[u8]) -> &[u8] {
    let end = path.len() - path.iter().rev().take_while(|&&b| b == b'/').count();
    &path[..end]
}

fn is_loopback_name(hostname: &str) -> bool {
    hostname == "localhost" || crate::gojson::equal_fold(hostname, "ip6-localhost")
}

/// `validatePublicMCPOrigin`.
fn validate_origin(raw: &str, origin_host_id: &str) -> Result<(), String> {
    let parsed = parse_origin(raw)?;
    if !trim_trailing_slashes(&parsed.path).ends_with(b"/mcp") {
        return Err("localTarget must end with /mcp".into());
    }
    let hostname = parsed.hostname();
    let hostname = hostname.trim();
    if is_loopback_name(hostname) || is_loopback_ip(hostname) {
        return Ok(());
    }
    if origin_host_id.trim().is_empty() {
        return Err("non-loopback localTarget requires an explicit originHostId".into());
    }
    Ok(())
}

/// `validatePublicMCPTunnelToken`.
fn validate_token(token: &str) -> Result<(), String> {
    if token.len() < 32 {
        return Err("tunnelToken must contain at least 32 characters".into());
    }
    if token.contains(['\r', '\n', '\0', '\t', ' ']) {
        return Err("tunnelToken must not contain whitespace or control characters".into());
    }
    Ok(())
}

/// `requireOwnedManagedFile`: absent, or a regular file carrying `marker`.
fn require_owned(path: &str, marker: &str) -> Result<(), String> {
    match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => {
            return Err(format!(
                "inspect Opute-owned public MCP file: {}",
                path_error("lstat", Path::new(path), &e).0
            ))
        }
        Ok(meta) if !meta.file_type().is_file() => {
            return Err("refusing to overwrite non-regular Opute public MCP path".into())
        }
        Ok(_) => {}
    }
    let data = read_file(path).map_err(|e| format!("read Opute-owned public MCP file: {e}"))?;
    if !data.starts_with(marker.as_bytes()) {
        return Err("refusing to overwrite non-Opute file at public MCP path".into());
    }
    Ok(())
}

/// `os.ReadFile`, with Go's error text for the failing step.
fn read_file(path: &str) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut file =
        std::fs::File::open(path).map_err(|e| path_error("open", Path::new(path), &e).0)?;
    let mut data = Vec::new();
    file.read_to_end(&mut data)
        .map_err(|e| path_error("read", Path::new(path), &e).0)?;
    Ok(data)
}

fn managed_observation(path: &str, content: &str, changed: bool) -> J {
    json!({
        "path": path,
        "changed": changed,
        "contentSha256": sha256_label(content.as_bytes()),
        "mode": "600",
    })
}

/// `ensurePrivateManagedFile`: the system-scope secret and state writer
/// (0600, replaced atomically, unchanged content left alone).
fn ensure_private_file(path: &str, content: &str) -> Result<J, String> {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    match std::fs::symlink_metadata(path) {
        Ok(meta) => {
            if !meta.file_type().is_file() {
                return Err("managed public MCP secret path is not a regular file".into());
            }
            let data =
                read_file(path).map_err(|e| format!("read managed public MCP secret: {e}"))?;
            if data == content.as_bytes() && meta.permissions().mode() & 0o777 == 0o600 {
                return Ok(managed_observation(path, content, false));
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(format!(
                "inspect managed public MCP secret: {}",
                path_error("lstat", Path::new(path), &e).0
            ))
        }
    }
    let dir = Path::new(path)
        .parent()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| ".".into());
    crate::goerr::mkdir_all(Path::new(&dir), 0o700)
        .map_err(|e| format!("create managed public MCP secret directory: {}", e.0))?;
    let (mut file, temporary) = crate::goerr::create_temp(&dir, ".opute-public-mcp-", "")
        .map_err(|e| format!("create managed public MCP secret temporary: {e}"))?;
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
                "set managed public MCP secret mode: {}",
                path_error("chmod", Path::new(&temporary), &e).0
            )
        })?;
    file.write_all(content.as_bytes()).map_err(|e| {
        format!(
            "write managed public MCP secret: {}",
            path_error("write", Path::new(&temporary), &e).0
        )
    })?;
    drop(file);
    std::fs::rename(&temporary, path).map_err(|e| {
        format!(
            "install managed public MCP secret: rename {temporary} {path}: {}",
            errno_text(&e)
        )
    })?;
    Ok(managed_observation(path, content, true))
}

/// A user-scope file through `EnsureHostFile`, a system-scope one through
/// the private writer.
fn ensure_scoped_file(owner: &Owner, scope: &str, path: &str, content: &str) -> Result<J, String> {
    if scope == "user" {
        ensure_host_file(owner, path, content, 0o600, "user")
    } else {
        ensure_private_file(path, content)
    }
}

/// Waits `delay` unless the caller cancels first.
fn sleep_or_cancel(ctx: &RunCtx, delay: Duration) -> Result<(), String> {
    let until = Instant::now() + delay;
    loop {
        if let Some(err) = ctx.err() {
            return Err(err);
        }
        let now = Instant::now();
        if now >= until {
            return Ok(());
        }
        std::thread::sleep((until - now).min(Duration::from_millis(50)));
    }
}

/// `probePublicMCPWithRetry`: bounded retries across the DNS publication
/// race, keeping the final typed error.
fn probe_with_retry(ctx: &RunCtx, endpoint: &str) -> Result<J, String> {
    let deadline = Instant::now() + PROBE_RETRY_WINDOW;
    loop {
        let last = match probe_authenticated(endpoint, "") {
            Ok(result) => return Ok(result),
            Err(e) => e,
        };
        if let Some(err) = ctx.err() {
            return Err(err);
        }
        if Instant::now() > deadline {
            return Err(format!(
                "public MCP endpoint did not become ready within {}: {last}",
                crate::hostobs::go_duration(PROBE_RETRY_WINDOW)
            ));
        }
        sleep_or_cancel(ctx, PROBE_RETRY_DELAY)?;
    }
}

/// The origin probe both tunnel kinds run once the service is started.
fn probe_origin(local_target: &str, what: &str) -> Result<J, String> {
    let origin = probe_authenticated(local_target, "")
        .map_err(|e| format!("probe public MCP {what}origin: {e}"))?;
    if origin.get("ready") != Some(&J::Bool(true)) {
        return Err(format!("public MCP {what}origin is not ready"));
    }
    Ok(origin)
}

/// `EnsurePublicMcpTunnelArgs`.
#[derive(Default)]
pub struct TunnelArgs {
    pub binding_id: String,
    pub endpoint: String,
    pub local_target: String,
    pub origin_host_id: String,
    pub tunnel_token: String,
    pub artifact_uri: String,
    pub artifact_sha256: String,
    pub artifact_path: String,
    pub token_file: String,
    pub service_name: String,
    pub service_file: String,
    pub scope: String,
}

struct TunnelPaths {
    scope: String,
    binding_id: String,
    artifact_uri: String,
    artifact_sha256: String,
    artifact_path: String,
    token_file: String,
    service_name: String,
    service_file: String,
    unit_artifact: String,
    unit_token_file: String,
}

/// `resolvePublicMCPPaths`.
fn resolve_tunnel(args: &TunnelArgs) -> Result<(TunnelPaths, String, String), String> {
    let binding_id = args.binding_id.trim();
    if !valid_binding(binding_id) {
        return Err(BINDING_ERROR.into());
    }
    let endpoint = validate_endpoint(args.endpoint.trim())?;
    if GoUrl::parse(&endpoint).is_none_or(|u| u.scheme != "https") {
        return Err("endpoint must be an absolute HTTPS URL ending in /mcp".into());
    }
    let local_target = args.local_target.trim().to_string();
    validate_origin(&local_target, &args.origin_host_id)?;
    let scope = connector_scope(&args.scope, "connector")?;
    let instance = instance_id(binding_id);
    let service_name = format!("{SERVICE_PREFIX}{instance}.service");
    let provided = args.service_name.trim();
    if !provided.is_empty() && provided != service_name {
        return Err(format!(
            "serviceName must be the Opute-owned name {}",
            crate::goerr::quote(&service_name)
        ));
    }
    let (artifact_uri, artifact_sha256) =
        artifact_source(&args.artifact_uri, &args.artifact_sha256)?;
    let paths = if scope == "user" {
        let home = resolve_home()?;
        TunnelPaths {
            artifact_path: join(&[
                &home,
                ".local/share/opute/tunnels",
                &instance,
                "cloudflared",
            ]),
            token_file: join(&[&home, ".config/opute/tunnels", &format!("{instance}.env")]),
            service_file: join(&[&home, ".config/systemd/user", &service_name]),
            unit_artifact: join(&["%h/.local/share/opute/tunnels", &instance, "cloudflared"]),
            unit_token_file: join(&["%h/.config/opute/tunnels", &format!("{instance}.env")]),
            scope,
            binding_id: binding_id.into(),
            artifact_uri,
            artifact_sha256,
            service_name,
        }
    } else {
        let artifact_path = join(&["/opt/opute/tunnels", &instance, "cloudflared"]);
        let token_file = join(&["/etc/opute/tunnels", &format!("{instance}.env")]);
        TunnelPaths {
            unit_artifact: artifact_path.clone(),
            unit_token_file: token_file.clone(),
            artifact_path,
            token_file,
            service_file: join(&["/etc/systemd/system", &service_name]),
            scope,
            binding_id: binding_id.into(),
            artifact_uri,
            artifact_sha256,
            service_name,
        }
    };
    // Go ranges over a map here, so with several moved paths the one it
    // names varies; this checks them in declaration order.
    for (label, provided, expected) in [
        ("artifactPath", &args.artifact_path, &paths.artifact_path),
        ("tokenFile", &args.token_file, &paths.token_file),
        ("serviceFile", &args.service_file, &paths.service_file),
    ] {
        if !provided.trim().is_empty() && clean(provided) != *expected {
            return Err(format!(
                "{label} must remain at the Opute-owned public MCP path"
            ));
        }
    }
    Ok((paths, endpoint, local_target))
}

/// The connector unit: `environment` lines go before `ExecStart`, `tail`
/// lines after `RestartSec`.
fn render_unit(
    scope: &str,
    exec_start: &str,
    description: &str,
    environment: &str,
    tail: &str,
) -> String {
    let wanted_by = if scope == "system" {
        "multi-user.target"
    } else {
        "default.target"
    };
    format!(
        "{MANAGED_MARKER}[Unit]\nDescription={description}\nAfter=network-online.target\nWants=network-online.target\n\n[Service]\n{environment}ExecStart=/bin/sh -c {}\nRestart=always\nRestartSec=15\n{tail}\n[Install]\nWantedBy={wanted_by}\n",
        shell_quote(exec_start),
    )
}

/// The connector binary beneath the scope's Opute tunnel directory.
fn ensure_connector(
    scope: &str,
    artifact_path: &str,
    user_error: &str,
    system_root: &str,
    uri: &str,
    sha256: &str,
) -> Result<J, String> {
    if scope == "user" {
        let home = resolve_home()?;
        match host_owned_path(&home, artifact_path) {
            Ok(owned) if owned == clean(artifact_path) => {}
            _ => return Err(user_error.into()),
        }
    } else if !clean(artifact_path).starts_with(&format!("{system_root}/")) {
        return Err(format!(
            "system artifactPath must remain beneath {system_root}"
        ));
    }
    crate::hostartifact::ensure_at(uri, artifact_path, sha256, true)
}

/// `Service.EnsurePublicMcpTunnel`.
pub fn ensure_public_mcp_tunnel(
    env: &Env,
    owner: &Owner,
    ctx: &RunCtx,
    args: &TunnelArgs,
) -> Result<J, String> {
    require_shared_host_owner(
        owner.instance_id,
        owner.owner_instance,
        "ensure_public_mcp_tunnel",
    )?;
    let (paths, endpoint, local_target) = resolve_tunnel(args)?;
    let token = args.tunnel_token.trim();
    validate_token(token)?;
    let artifact = ensure_connector(
        &paths.scope,
        &paths.artifact_path,
        "artifactPath must remain beneath the current user's Opute tunnel directory",
        "/opt/opute/tunnels",
        &paths.artifact_uri,
        &paths.artifact_sha256,
    )
    .map_err(|e| format!("ensure public MCP connector artifact: {e}"))?;
    let token_content = format!("{MANAGED_MARKER}{TOKEN_ENV}={token}\n");
    require_owned(&paths.token_file, MANAGED_MARKER)?;
    let token_file = ensure_scoped_file(owner, &paths.scope, &paths.token_file, &token_content)
        .map_err(|e| format!("store public MCP tunnel token: {e}"))?;
    let command = format!(
        "exec {} tunnel --no-autoupdate run --token \"${TOKEN_ENV}\"",
        shell_quote(&paths.unit_artifact)
    );
    let unit = render_unit(
        &paths.scope,
        &command,
        "Opute authenticated public MCP tunnel",
        &format!("EnvironmentFile={}\n", paths.unit_token_file),
        "",
    );
    require_owned(&paths.service_file, MANAGED_MARKER)?;
    ensure_host_file(owner, &paths.service_file, &unit, 0o600, &paths.scope)
        .map_err(|e| format!("write public MCP connector service: {e}"))?;
    crate::hostservice::ensure_host_service_supervisor(env, owner, &paths.scope)
        .map_err(|e| format!("ensure public MCP connector supervisor: {e}"))?;
    for (state, what) in [("enable", "enable"), ("start", "start")] {
        crate::hostservice::set_host_service_state(
            env,
            owner,
            &paths.service_name,
            state,
            &paths.scope,
        )
        .map_err(|e| format!("{what} public MCP connector service: {e}"))?;
    }
    // An MCP resource answers GET with 405, so the origin gets the same
    // authenticated tools/list probe as the public endpoint.
    let origin = probe_origin(&local_target, "")?;
    let public = probe_with_retry(ctx, &endpoint)
        .map_err(|e| format!("probe authenticated public MCP endpoint: {e}"))?;
    Ok(json!({
        "contractVersion": CONTRACT_VERSION,
        "ready": true,
        "bindingId": paths.binding_id,
        "endpoint": endpoint,
        "localTarget": local_target,
        "originHostId": args.origin_host_id.trim(),
        "scope": paths.scope,
        "serviceName": paths.service_name,
        "serviceFile": paths.service_file,
        "tokenFile": token_file,
        "artifact": artifact,
        "origin": origin,
        "publicAuth": public,
    }))
}

struct QuickPaths {
    scope: String,
    binding_id: String,
    artifact_uri: String,
    artifact_sha256: String,
    artifact_path: String,
    service_name: String,
    service_file: String,
    log_file: String,
    state_file: String,
    unit_artifact: String,
    unit_log_file: String,
}

/// `resolvePublicMCPQuickPaths`; with an origin, also the trimmed target
/// and its scheme-and-authority base.
fn resolve_quick(
    binding_id: &str,
    local_target: Option<&str>,
    artifact_uri: &str,
    artifact_sha256: &str,
    scope: &str,
) -> Result<(QuickPaths, String, String), String> {
    let binding_id = binding_id.trim();
    if !valid_binding(binding_id) {
        return Err(BINDING_ERROR.into());
    }
    let scope = connector_scope(scope, "quick connector")?;
    let (artifact_uri, artifact_sha256) = artifact_source(artifact_uri, artifact_sha256)?;
    let instance = instance_id(binding_id);
    let service_name = format!("{QUICK_SERVICE_PREFIX}{instance}.service");
    let paths = if scope == "user" {
        let home = resolve_home()?;
        let root = join(&[&home, ".local/share/opute/tunnels/quick", &instance]);
        let unit_root = join(&["%h/.local/share/opute/tunnels/quick", &instance]);
        QuickPaths {
            artifact_path: join(&[&root, "cloudflared"]),
            log_file: join(&[&root, "quick.log"]),
            state_file: join(&[&root, "endpoint.json"]),
            service_file: join(&[&home, ".config/systemd/user", &service_name]),
            unit_artifact: join(&[&unit_root, "cloudflared"]),
            unit_log_file: join(&[&unit_root, "quick.log"]),
            scope,
            binding_id: binding_id.into(),
            artifact_uri,
            artifact_sha256,
            service_name,
        }
    } else {
        let root = join(&["/opt/opute/tunnels/quick", &instance]);
        let artifact_path = join(&[&root, "cloudflared"]);
        let log_file = join(&[&root, "quick.log"]);
        QuickPaths {
            unit_artifact: artifact_path.clone(),
            unit_log_file: log_file.clone(),
            artifact_path,
            log_file,
            state_file: join(&[&root, "endpoint.json"]),
            service_file: join(&["/etc/systemd/system", &service_name]),
            scope,
            binding_id: binding_id.into(),
            artifact_uri,
            artifact_sha256,
            service_name,
        }
    };
    let Some(local_target) = local_target else {
        return Ok((paths, String::new(), String::new()));
    };
    let mut parsed = parse_origin(local_target)?;
    if trim_trailing_slashes(&parsed.path) != b"/mcp" {
        return Err("localTarget must end exactly with /mcp for a quick tunnel".into());
    }
    let hostname = parsed.hostname();
    let hostname = hostname.trim();
    if !is_loopback_name(hostname) && !is_loopback_ip(hostname) {
        return Err("quick tunnel localTarget must resolve to loopback".into());
    }
    parsed.set_path("");
    Ok((paths, local_target.trim().to_string(), parsed.string()))
}

/// `ensurePublicMCPQuickArtifact`: an existing binary must already be the
/// pinned one; it is never overwritten.
fn ensure_quick_connector(paths: &QuickPaths) -> Result<J, String> {
    if paths.scope == "user" {
        let home = resolve_home()?;
        match host_owned_path(&home, &paths.artifact_path) {
            Ok(owned) if owned == clean(&paths.artifact_path) => {}
            _ => return Err(
                "artifactPath must remain beneath the current user's Opute quick tunnel directory"
                    .into(),
            ),
        }
    } else if !clean(&paths.artifact_path).starts_with("/opt/opute/tunnels/quick/") {
        return Err("system artifactPath must remain beneath /opt/opute/tunnels/quick".into());
    }
    match std::fs::symlink_metadata(&paths.artifact_path) {
        Ok(meta) if meta.file_type().is_symlink() => {
            return Err("refusing to use symlink at quick tunnel artifact path".into())
        }
        Ok(meta) => {
            if !meta.file_type().is_file() {
                return Err("refusing to use non-regular quick tunnel artifact path".into());
            }
            let observed = crate::hostartifact::file_sha256(&paths.artifact_path)?;
            if !crate::gojson::equal_fold(&observed, &paths.artifact_sha256) {
                return Err(
                    "quick tunnel artifact hash mismatch; refusing to overwrite existing file"
                        .into(),
                );
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(format!(
                "inspect quick tunnel artifact path: {}",
                path_error("lstat", Path::new(&paths.artifact_path), &e).0
            ))
        }
    }
    crate::hostartifact::ensure_at(
        &paths.artifact_uri,
        &paths.artifact_path,
        &paths.artifact_sha256,
        true,
    )
}

/// `ensurePublicMCPQuickLog`: create the marked log once.
fn ensure_quick_log(owner: &Owner, paths: &QuickPaths) -> Result<(), String> {
    match std::fs::symlink_metadata(&paths.log_file) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            ensure_scoped_file(owner, &paths.scope, &paths.log_file, QUICK_MARKER).map(|_| ())
        }
        Err(e) => Err(format!(
            "inspect quick tunnel log: {}",
            path_error("lstat", Path::new(&paths.log_file), &e).0
        )),
        Ok(meta) if !meta.file_type().is_file() => {
            Err("quick tunnel log is not a regular file".into())
        }
        Ok(_) => Ok(()),
    }
}

/// `validatePublicMCPQuickEndpoint`: a bare `https://*.trycloudflare.com`
/// origin, returned as its `/mcp` resource.
fn quick_endpoint(raw: &str) -> Option<String> {
    let mut parsed = GoUrl::parse(raw.trim())?;
    if parsed.scheme != "https"
        || parsed.host.is_empty()
        || parsed.has_user
        || !parsed.raw_query.is_empty()
        || !parsed.fragment.is_empty()
    {
        return None;
    }
    if !parsed
        .hostname()
        .to_lowercase()
        .ends_with(".trycloudflare.com")
        || !trim_trailing_slashes(&parsed.path).is_empty()
    {
        return None;
    }
    parsed.set_path("/mcp");
    validate_endpoint(&parsed.string()).ok()
}

/// `readPublicMCPQuickEndpoint`: the first quick-tunnel URL in the log.
fn read_quick_endpoint(path: &str) -> String {
    let Ok(data) = std::fs::read(path) else {
        return String::new();
    };
    if data.len() as u64 > QUICK_LOG_LIMIT {
        return String::new();
    }
    // `strings.Fields` splits on Unicode spaces; invalid UTF-8 never is one.
    let text = String::from_utf8_lossy(&data);
    for field in text.split_whitespace() {
        let field = field.trim_matches(|c| "\"'`()[]{}<>,.;".contains(c));
        if let Some(endpoint) = quick_endpoint(field) {
            return endpoint;
        }
    }
    String::new()
}

/// `waitForPublicMCPQuickEndpoint`.
fn wait_for_quick_endpoint(ctx: &RunCtx, path: &str) -> Result<String, String> {
    let deadline = Instant::now() + PROBE_RETRY_WINDOW;
    loop {
        let endpoint = read_quick_endpoint(path);
        if !endpoint.is_empty() {
            return Ok(endpoint);
        }
        if let Some(err) = ctx.err() {
            return Err(err);
        }
        if Instant::now() > deadline {
            return Err(format!(
                "public MCP quick tunnel did not emit a trycloudflare.com endpoint within {}",
                crate::hostobs::go_duration(PROBE_RETRY_WINDOW)
            ));
        }
        sleep_or_cancel(ctx, QUICK_POLL_DELAY)?;
    }
}

/// `readPublicMCPQuickState`: the recorded endpoint, only when the state
/// file is ours and describes this binding and origin exactly.
fn read_quick_state(paths: &QuickPaths, binding_id: &str, local_target: &str) -> String {
    use crate::gojson::Value;
    let Ok(data) = std::fs::read(&paths.state_file) else {
        return String::new();
    };
    if data.len() > 16 * 1024 || !data.starts_with(QUICK_MARKER.as_bytes()) {
        return String::new();
    }
    let body = &data[QUICK_MARKER.len()..];
    if crate::gojson::syntax_error(body).is_some() {
        return String::new();
    }
    let Ok(node) = crate::gojson::parse(body) else {
        return String::new();
    };
    // `json.Unmarshal` into the state struct: any type error fails it.
    let mut fields = [
        ("contractVersion", String::new()),
        ("mode", String::new()),
        ("bindingId", String::new()),
        ("localTarget", String::new()),
        ("endpoint", String::new()),
    ];
    let mut stable = false;
    match &node.value {
        Value::Null => {}
        Value::Object(members) => {
            for (key, value) in members {
                if crate::gojson::equal_fold(key, "stable") {
                    match &value.value {
                        Value::Null => {}
                        Value::Bool(b) => stable = *b,
                        _ => return String::new(),
                    }
                    continue;
                }
                for (name, slot) in fields.iter_mut() {
                    if crate::gojson::equal_fold(key, name) {
                        match &value.value {
                            Value::Null => {}
                            Value::String(s) => *slot = s.clone(),
                            _ => return String::new(),
                        }
                    }
                }
            }
        }
        _ => return String::new(),
    }
    let [(_, contract), (_, mode), (_, binding), (_, target), (_, endpoint)] = fields;
    if contract != CONTRACT_VERSION
        || mode != "quick"
        || stable
        || binding != binding_id.trim()
        || target != local_target
    {
        return String::new();
    }
    quick_endpoint(&endpoint).unwrap_or_default()
}

/// `writePublicMCPQuickState`: `json.Marshal` of the state struct.
fn write_quick_state(
    owner: &Owner,
    paths: &QuickPaths,
    binding_id: &str,
    local_target: &str,
    endpoint: &str,
) -> Result<(), String> {
    let mut encoded = format!("{{\"contractVersion\":\"{CONTRACT_VERSION}\",\"mode\":\"quick\",\"stable\":false,\"bindingId\":");
    crate::gojson::encode_string(binding_id.trim(), &mut encoded);
    encoded.push_str(",\"localTarget\":");
    crate::gojson::encode_string(local_target, &mut encoded);
    encoded.push_str(",\"endpoint\":");
    crate::gojson::encode_string(endpoint, &mut encoded);
    encoded.push('}');
    let content = format!("{QUICK_MARKER}{encoded}\n");
    ensure_scoped_file(owner, &paths.scope, &paths.state_file, &content).map(|_| ())
}

/// `truncatePublicMCPQuickLog`: back to just the marker.
fn truncate_quick_log(paths: &QuickPaths) -> Result<(), String> {
    use std::io::Write;
    require_owned(&paths.log_file, QUICK_MARKER)?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(&paths.log_file)
        .map_err(|e| path_error("open", Path::new(&paths.log_file), &e).0)?;
    file.write_all(QUICK_MARKER.as_bytes())
        .map_err(|e| path_error("write", Path::new(&paths.log_file), &e).0)
}

fn quick_observation(
    paths: &QuickPaths,
    local_target: &str,
    endpoint: &str,
    artifact: J,
    origin: J,
    public: J,
) -> J {
    json!({
        "contractVersion": CONTRACT_VERSION,
        "ready": true,
        "mode": "quick",
        "stable": false,
        "bindingId": paths.binding_id,
        "endpoint": endpoint,
        "localTarget": local_target,
        "scope": paths.scope,
        "serviceName": paths.service_name,
        "serviceFile": paths.service_file,
        "logFile": paths.log_file,
        "stateFile": paths.state_file,
        "artifact": artifact,
        "origin": origin,
        "publicAuth": public,
    })
}

/// `Service.EnsurePublicMcpQuickTunnel`.
#[allow(clippy::too_many_arguments)]
pub fn ensure_public_mcp_quick_tunnel(
    env: &Env,
    owner: &Owner,
    ctx: &RunCtx,
    binding_id: &str,
    local_target: &str,
    artifact_uri: &str,
    artifact_sha256: &str,
    scope: &str,
) -> Result<J, String> {
    use crate::hostservice::set_host_service_state;
    require_shared_host_owner(
        owner.instance_id,
        owner.owner_instance,
        "ensure_public_mcp_quick_tunnel",
    )?;
    let (paths, local_target, origin_base) = resolve_quick(
        binding_id,
        Some(local_target),
        artifact_uri,
        artifact_sha256,
        scope,
    )?;
    let artifact = ensure_quick_connector(&paths)
        .map_err(|e| format!("ensure public MCP quick connector artifact: {e}"))?;
    require_owned(&paths.service_file, MANAGED_MARKER)?;
    require_owned(&paths.log_file, QUICK_MARKER)?;
    require_owned(&paths.state_file, QUICK_MARKER)?;
    ensure_quick_log(owner, &paths)
        .map_err(|e| format!("prepare public MCP quick tunnel log: {e}"))?;
    let command = format!(
        "exec {} tunnel --no-autoupdate --url {} >> {} 2>&1",
        shell_quote(&paths.unit_artifact),
        shell_quote(&origin_base),
        shell_quote(&paths.unit_log_file)
    );
    let unit = render_unit(
        &paths.scope,
        &command,
        "Opute ephemeral authenticated public MCP quick tunnel",
        "",
        "KillMode=control-group\n",
    );
    let unit_observation = ensure_host_file(owner, &paths.service_file, &unit, 0o600, &paths.scope)
        .map_err(|e| format!("write public MCP quick connector service: {e}"))?;
    crate::hostservice::ensure_host_service_supervisor(env, owner, &paths.scope)
        .map_err(|e| format!("ensure public MCP quick connector supervisor: {e}"))?;
    let active = crate::hostread::inspect_host_service(env, &paths.service_name, &paths.scope, 0)
        .ok()
        .and_then(|observed| observed.get("active").and_then(J::as_bool))
        .unwrap_or(false);
    let unit_changed = unit_observation
        .get("changed")
        .and_then(J::as_bool)
        .unwrap_or(false);
    let state = if active && unit_changed {
        "restart"
    } else {
        "start"
    };
    set_host_service_state(env, owner, &paths.service_name, state, &paths.scope)
        .map_err(|e| format!("start public MCP quick connector service: {e}"))?;
    let origin = probe_origin(&local_target, "quick ")?;

    let mut endpoint = String::new();
    if !unit_changed && active {
        endpoint = read_quick_state(&paths, binding_id, &local_target);
        if !endpoint.is_empty() {
            if let Ok(public) = probe_authenticated(&endpoint, "") {
                return Ok(quick_observation(
                    &paths,
                    &local_target,
                    &endpoint,
                    artifact,
                    origin,
                    public,
                ));
            }
            endpoint.clear();
        }
    }
    if endpoint.is_empty() {
        endpoint = read_quick_endpoint(&paths.log_file);
    }
    if endpoint.is_empty() || unit_changed || !active {
        if !endpoint.is_empty() {
            // A prior tunnel's log is no evidence for the current process:
            // clear it, restart, and wait for a freshly emitted hostname.
            truncate_quick_log(&paths)
                .map_err(|e| format!("reset public MCP quick tunnel log: {e}"))?;
            set_host_service_state(env, owner, &paths.service_name, "restart", &paths.scope)
                .map_err(|e| format!("restart public MCP quick connector service: {e}"))?;
        }
        endpoint = wait_for_quick_endpoint(ctx, &paths.log_file)?;
    }
    let public = probe_with_retry(ctx, &endpoint)
        .map_err(|e| format!("probe authenticated public MCP quick endpoint: {e}"))?;
    write_quick_state(owner, &paths, binding_id, &local_target, &endpoint)
        .map_err(|e| format!("record public MCP quick tunnel endpoint: {e}"))?;
    Ok(quick_observation(
        &paths,
        &local_target,
        &endpoint,
        artifact,
        origin,
        public,
    ))
}

/// `removeOwnedQuickFile`.
fn remove_owned_file(path: &str, marker: &str) -> Result<bool, String> {
    match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(path_error("lstat", Path::new(path), &e).0),
        Ok(meta) if !meta.file_type().is_file() => {
            return Err(format!(
                "refusing to remove non-regular managed path {}",
                crate::goerr::quote(path)
            ))
        }
        Ok(_) => {}
    }
    require_owned(path, marker)?;
    std::fs::remove_file(path).map_err(|e| path_error("remove", Path::new(path), &e).0)?;
    Ok(true)
}

/// `removeOwnedQuickArtifact`: only the pinned binary is removed.
fn remove_owned_artifact(paths: &QuickPaths) -> Result<bool, String> {
    let path = &paths.artifact_path;
    match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(path_error("lstat", Path::new(path), &e).0),
        Ok(meta) if !meta.file_type().is_file() => {
            return Err("refusing to remove non-regular quick tunnel artifact".into())
        }
        Ok(_) => {}
    }
    let observed = crate::hostartifact::file_sha256(path)?;
    if !crate::gojson::equal_fold(&observed, &paths.artifact_sha256) {
        return Err("quick tunnel artifact hash mismatch; refusing removal".into());
    }
    std::fs::remove_file(path).map_err(|e| path_error("remove", Path::new(path), &e).0)?;
    Ok(true)
}

/// `Service.RemovePublicMcpQuickTunnel`: every attempt is reported, and a
/// failed cleanup is `ready:false`, never a pass.
#[allow(clippy::too_many_arguments)]
pub fn remove_public_mcp_quick_tunnel(
    env: &Env,
    owner: &Owner,
    ctx: &RunCtx,
    binding_id: &str,
    artifact_uri: &str,
    artifact_sha256: &str,
    scope: &str,
    confirm: bool,
) -> Result<J, String> {
    require_shared_host_owner(
        owner.instance_id,
        owner.owner_instance,
        "remove_public_mcp_quick_tunnel",
    )?;
    if !confirm {
        return Err("remove_public_mcp_quick_tunnel requires confirm=true".into());
    }
    let (paths, _, _) = resolve_quick(binding_id, None, artifact_uri, artifact_sha256, scope)?;
    let mut attempts = Vec::new();
    let mut failures: Vec<String> = Vec::new();
    let mut command: Vec<String> = vec!["/usr/bin/systemctl".into()];
    if paths.scope == "user" {
        command.push("--user".into());
    } else {
        command.splice(0..0, ["sudo".to_string(), "-n".to_string()]);
    }
    command.extend(["disable", "--now", &paths.service_name].map(String::from));
    // `HostCommandRunnerContext`: a done context is the error.
    let (result, run_error) = match ctx.err() {
        Some(err) => (
            crate::hostobs::CommandResult {
                exit_code: 0,
                stdout: String::new(),
                stderr: String::new(),
            },
            err,
        ),
        None => {
            let timeout = Duration::from_secs(30);
            let result = crate::hostobs::run_command_with(
                env,
                &command,
                None,
                Some(Instant::now() + timeout),
                timeout,
                Some(ctx),
            );
            (result, String::new())
        }
    };
    let service_exists = std::fs::symlink_metadata(&paths.service_file).is_ok();
    if !run_error.is_empty() || (result.exit_code != 0 && service_exists) {
        let failure = format!(
            "stop service {}: {}",
            paths.service_name,
            first_non_empty(&[&result.stderr, &result.stdout, &run_error])
        );
        attempts.push(json!({"resource": "service", "path": paths.service_name, "removed": false, "error": failure}));
        failures.push(failure);
    } else {
        attempts.push(
            json!({"resource": "service", "path": paths.service_name, "removed": service_exists}),
        );
    }
    for (name, path, marker) in [
        ("serviceFile", &paths.service_file, MANAGED_MARKER),
        ("logFile", &paths.log_file, QUICK_MARKER),
        ("stateFile", &paths.state_file, QUICK_MARKER),
    ] {
        let (removed, error) = match remove_owned_file(path, marker) {
            Ok(removed) => (removed, None),
            Err(e) => (false, Some(e)),
        };
        let mut attempt = json!({"resource": name, "path": path, "removed": removed});
        if let Some(e) = error {
            failures.push(format!("remove {name}: {e}"));
            attempt["error"] = J::from(e);
        }
        attempts.push(attempt);
    }
    let (removed, error) = match remove_owned_artifact(&paths) {
        Ok(removed) => (removed, None),
        Err(e) => (false, Some(e)),
    };
    let mut attempt =
        json!({"resource": "artifact", "path": paths.artifact_path, "removed": removed});
    if let Some(e) = error {
        failures.push(format!("remove artifact: {e}"));
        attempt["error"] = J::from(e);
    }
    attempts.push(attempt);
    let mut result = json!({
        "contractVersion": CONTRACT_VERSION,
        "ready": failures.is_empty(),
        "status": "removed",
        "bindingId": paths.binding_id,
        "scope": paths.scope,
        "serviceName": paths.service_name,
        "attempts": attempts,
    });
    if !failures.is_empty() {
        result["status"] = J::from("cleanup_failed");
        result["failures"] = J::from(failures);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instance_ids_match_go() {
        assert_eq!(instance_id("tenant-a"), "tenant-a");
        // Normalization changed the binding, so a digest keeps it injective.
        let id = instance_id("Tenant:A");
        assert!(id.starts_with("tenant-a-") && id.len() == "tenant-a-".len() + 10);
        assert!(instance_id(&"a".repeat(80)).len() <= 63);
        assert!(valid_binding("a.b:c_d-e") && !valid_binding("-a") && !valid_binding(""));
    }

    #[test]
    fn quick_endpoints_are_bare_trycloudflare_origins() {
        assert_eq!(
            quick_endpoint("https://abc.trycloudflare.com").as_deref(),
            Some("https://abc.trycloudflare.com/mcp")
        );
        assert!(quick_endpoint("https://abc.trycloudflare.com/x").is_none());
        assert!(quick_endpoint("http://abc.trycloudflare.com").is_none());
        assert!(quick_endpoint("https://example.com").is_none());
    }
}
