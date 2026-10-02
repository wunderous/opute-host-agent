//! Host Agent configuration: environment snapshot, env-file loading, defaults
//! and fail-closed validation.
//!
//! Behavior follows `internal/config` of the pinned Go baseline, including
//! quote stripping, precedence and error text. Unlike Go, configuration is
//! never written back into the process environment: the CLI builds an [`Env`]
//! overlay (process environment < env file < `--env` < resolved flags) and
//! [`Config::load`] reads only from it.

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr};
use std::path::{Path, PathBuf};

use crate::goerr::{self, atoi, quote, Result};
use crate::identity::{self, Identity};

/// An immutable-by-convention view of the environment the agent runs with.
#[derive(Debug, Clone, Default)]
pub struct Env {
    vars: BTreeMap<String, String>,
}

impl Env {
    pub fn from_process() -> Self {
        let vars = std::env::vars_os()
            .filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?)))
            .collect();
        Env { vars }
    }

    #[cfg(test)]
    pub fn from_pairs<I, K, V>(pairs: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        Env {
            vars: pairs
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
        }
    }

    /// Every variable, for child processes (Go's `os.Environ()` after
    /// `os.Setenv` of the env file).
    pub fn pairs(&self) -> impl Iterator<Item = (&str, &str)> {
        self.vars.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }

    /// `os.Getenv`.
    pub fn get(&self, key: &str) -> String {
        self.vars.get(key).cloned().unwrap_or_default()
    }

    /// `os.Setenv`.
    pub fn set(&mut self, key: &str, value: &str) {
        self.vars.insert(key.to_string(), value.to_string());
    }

    /// Go `envValue`: trimmed, with one pair of matching surrounding quotes
    /// removed (and the inside trimmed again).
    pub fn value(&self, key: &str) -> String {
        let value = self.get(key).trim().to_string();
        strip_quotes(&value)
            .map(|v| v.trim().to_string())
            .unwrap_or(value)
    }

    fn value_or(&self, key: &str, fallback: &str) -> String {
        let v = self.value(key);
        if v.trim().is_empty() {
            fallback.to_string()
        } else {
            v.trim().to_string()
        }
    }

    /// `config.LoadEnvFile`: KEY=VALUE lines that never override a non-blank
    /// variable already present.
    pub fn load_env_file(&mut self, path: &str) -> Result<()> {
        let data = std::fs::read(path).map_err(|e| {
            go_err!(
                "open env file {}: {}",
                quote(path),
                goerr::path_error("open", Path::new(path), &e)
            )
        })?;
        let text = String::from_utf8_lossy(&data);
        for (index, raw_line) in text.split('\n').enumerate() {
            let line_number = index + 1;
            let raw_line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let line = line.strip_prefix("export ").unwrap_or(line);
            let (key, value, ok) = match line.split_once('=') {
                Some((k, v)) => (k, v, true),
                None => (line, "", false),
            };
            let key = key.trim();
            if !ok || key.is_empty() {
                return Err(go_err!("invalid env assignment at {path}:{line_number}"));
            }
            if !self.get(key).trim().is_empty() {
                continue;
            }
            let value = value.trim();
            let value = strip_quotes(value).unwrap_or(value);
            self.set(key, value);
        }
        Ok(())
    }
}

fn strip_quotes(value: &str) -> Option<&str> {
    let b = value.as_bytes();
    if b.len() >= 2
        && ((b[0] == b'\'' && b[b.len() - 1] == b'\'') || (b[0] == b'"' && b[b.len() - 1] == b'"'))
    {
        Some(&value[1..value.len() - 1])
    } else {
        None
    }
}

/// Resolved runtime configuration (`config.Config`).
///
/// Every Go field is loaded now so defaults and validation are complete;
/// fields the M1 lifecycle does not read yet are consumed by later milestones.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct Config {
    pub env: Env,
    pub agent_mode: String,
    pub tenant_id: String,
    pub instance_id: String,
    pub instance_root: PathBuf,
    pub relay_config_dir: PathBuf,
    pub ownership_mode: String,
    pub standalone_state_dir: PathBuf,
    pub sqlite_database_root: PathBuf,
    pub standalone_allow_mutations: bool,
    pub standalone_instance_id: String,
    pub host_mcp_port: i64,
    pub host_mcp_bind_host: String,
    pub incus_network_name: String,
    pub incus_network_address: String,
    pub remote_agent_id: String,
    pub identity: std::result::Result<Identity, String>,
    pub mcp_auth_token: String,
    pub opute_client_secret: String,
    pub provider_id: String,
    pub host_resource_lock_dir: PathBuf,
    pub host_resource_max_normal: i64,
    pub host_resource_max_heavy: i64,
    pub host_resource_max_queued: i64,
    pub host_resource_min_memory_bytes: i64,
    pub host_resource_min_disk_bytes: i64,
    pub host_resource_cpu_capacity: f64,
    pub host_resource_memory_capacity: i64,
    pub host_resource_disk_capacity: i64,
    pub host_resource_task_capacity: i64,
    pub host_resource_disk_paths: Vec<String>,
    pub host_resource_policy_revision: String,
    pub allow_legacy_handshake: bool,
    pub prefix_tool_names: bool,
    pub disable_localhost_protection: bool,
}

fn normalize_mode(raw: &str) -> String {
    if raw.trim().eq_ignore_ascii_case("standalone") {
        "standalone".into()
    } else {
        "platform".into()
    }
}

fn user_home_dir(env: &Env) -> PathBuf {
    let home = env.get("HOME");
    if home.trim().is_empty() {
        PathBuf::from(".")
    } else {
        PathBuf::from(home)
    }
}

fn user_config_dir(env: &Env) -> PathBuf {
    let configured = env.value("XDG_CONFIG_HOME");
    if !configured.trim().is_empty() {
        return PathBuf::from(configured.trim());
    }
    user_home_dir(env).join(".config").join("opute")
}

fn env_int_or(env: &Env, key: &str, fallback: i64) -> i64 {
    let value = env.value(key);
    let value = value.trim();
    if value.is_empty() {
        return fallback;
    }
    match atoi(value) {
        (v, false) if v > 0 => v,
        _ => fallback,
    }
}

fn env_int64_or(env: &Env, key: &str, fallback: i64) -> i64 {
    let value = env.value(key);
    let value = value.trim();
    if value.is_empty() {
        return fallback;
    }
    match atoi(value) {
        (v, false) if v >= 0 => v,
        _ => fallback,
    }
}

fn env_float_or(env: &Env, key: &str, fallback: f64) -> f64 {
    let value = env.value(key);
    let value = value.trim();
    if value.is_empty() {
        return fallback;
    }
    match parse_go_float(value) {
        // NaN is not <= 0, so Go returns it and Validate rejects it.
        Some(v) if v > 0.0 || v.is_nan() => v,
        _ => fallback,
    }
}

/// Go `strconv.ParseFloat(s, 64)` for decimal, inf and nan forms.
fn parse_go_float(s: &str) -> Option<f64> {
    let lower = s.to_ascii_lowercase();
    let unsigned = lower.trim_start_matches(['+', '-']);
    if matches!(unsigned, "inf" | "infinity" | "nan") {
        return s.parse::<f64>().ok();
    }
    if s.contains('_') || !s.bytes().any(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse::<f64>().ok()
}

impl Config {
    /// `config.Load`.
    pub fn load(env: &Env) -> Config {
        let mode = normalize_mode(&env.get("OPUTE_AGENT_MODE"));
        let mut instance_id = env.value("OPUTE_HOST_AGENT_INSTANCE").trim().to_string();
        if instance_id.is_empty() {
            instance_id = if mode == "standalone" {
                "standalone"
            } else {
                "platform"
            }
            .into();
        }
        let instance_root = match env.value("OPUTE_HOST_AGENT_INSTANCE_ROOT").trim() {
            "" => user_config_dir(env).join("instances").join(&instance_id),
            v => PathBuf::from(v),
        };
        let state_dir = match env.value("OPUTE_STANDALONE_STATE_DIR").trim() {
            "" => instance_root.join("state"),
            v => PathBuf::from(v),
        };
        let sqlite_root = match env.value("OPUTE_HOST_AGENT_SQLITE_ROOT").trim() {
            "" => instance_root.join("databases"),
            v => PathBuf::from(v),
        };
        let relay_dir = match env.value("OPUTE_HOST_AGENT_RELAY_DIR").trim() {
            "" => instance_root.join("local-llm-relays"),
            v => PathBuf::from(v),
        };
        // Standalone avoids colliding with the platform dogfood host on 3004.
        let default_port = if mode == "standalone" { "3014" } else { "3004" };
        let (port, _) = atoi(&env.value_or("HOST_MCP_PORT", default_port));
        let tenant = match env.value("OPUTE_TENANT_ID").trim() {
            "" => "local".to_string(),
            v => v.to_string(),
        };
        // Platform binds the host bridge because Platform pods cannot reach
        // host loopback; standalone stays loopback-only.
        let default_bind = if mode == "platform" {
            "0.0.0.0"
        } else {
            "127.0.0.1"
        };
        let enforcement = env
            .value_or("OPUTE_HOST_RESOURCE_ENFORCEMENT", "fail-closed")
            .to_lowercase();
        let _ = enforcement;
        let ownership = if env
            .value("OPUTE_INCUS_OWNERSHIP_MODE")
            .trim()
            .eq_ignore_ascii_case("enforce")
        {
            "enforce"
        } else {
            "audit"
        };
        Config {
            env: env.clone(),
            agent_mode: mode,
            tenant_id: tenant,
            instance_id,
            instance_root,
            relay_config_dir: relay_dir,
            ownership_mode: ownership.into(),
            standalone_state_dir: state_dir,
            sqlite_database_root: sqlite_root,
            standalone_allow_mutations: env.get("OPUTE_STANDALONE_ALLOW_MUTATIONS") == "true",
            standalone_instance_id: env
                .value("OPUTE_LOCAL_HOST_AGENT_INSTANCE_ID")
                .trim()
                .to_string(),
            host_mcp_port: port,
            host_mcp_bind_host: env.value_or("HOST_MCP_BIND_HOST", default_bind),
            incus_network_name: env.value_or("OPUTE_INCUS_NETWORK_NAME", "incusbr0"),
            incus_network_address: env.value_or("OPUTE_INCUS_NETWORK_ADDRESS", "10.0.100.1/24"),
            remote_agent_id: env.value("OPUTE_REMOTE_AGENT_ID").trim().to_string(),
            identity: identity::read_identity(env),
            mcp_auth_token: env.value("MCP_AUTH_TOKEN").trim().to_string(),
            opute_client_secret: env
                .value("OPUTE_HOST_OAUTH_CLIENT_SECRET")
                .trim()
                .to_string(),
            provider_id: "incus".into(),
            host_resource_lock_dir: PathBuf::from(
                env.value_or(
                    "OPUTE_HOST_RESOURCE_LOCK_DIR",
                    &user_config_dir(env)
                        .join("host-resource-coordinator")
                        .to_string_lossy(),
                ),
            ),
            host_resource_disk_paths: {
                // envPathsOr: comma-separated, blanks dropped, "/" by default.
                let paths: Vec<String> = env
                    .value("OPUTE_HOST_RESOURCE_DISK_PATHS")
                    .trim()
                    .split(',')
                    .map(|p| p.trim().to_string())
                    .filter(|p| !p.is_empty())
                    .collect();
                if paths.is_empty() {
                    vec!["/".to_string()]
                } else {
                    paths
                }
            },
            host_resource_policy_revision: env.value_or(
                "OPUTE_HOST_RESOURCE_POLICY_REVISION",
                crate::resource::POLICY_REVISION,
            ),
            host_resource_max_normal: env_int_or(env, "OPUTE_HOST_MAX_NORMAL_OPERATIONS", 2),
            host_resource_max_heavy: env_int_or(env, "OPUTE_HOST_MAX_HEAVY_OPERATIONS", 1),
            host_resource_max_queued: env_int_or(env, "OPUTE_HOST_MAX_QUEUED_OPERATIONS", 16),
            host_resource_min_memory_bytes: env_int64_or(
                env,
                "OPUTE_HOST_MIN_AVAILABLE_MEMORY_BYTES",
                0,
            ),
            host_resource_min_disk_bytes: env_int64_or(
                env,
                "OPUTE_HOST_MIN_AVAILABLE_DISK_BYTES",
                0,
            ),
            host_resource_cpu_capacity: env_float_or(
                env,
                "OPUTE_HOST_RESOURCE_CPU_CAPACITY_CORES",
                6.0,
            ),
            host_resource_memory_capacity: env_int64_or(
                env,
                "OPUTE_HOST_RESOURCE_MEMORY_CAPACITY_BYTES",
                11 << 30,
            ),
            host_resource_disk_capacity: env_int64_or(
                env,
                "OPUTE_HOST_RESOURCE_DISK_CAPACITY_BYTES",
                0,
            ),
            host_resource_task_capacity: env_int64_or(
                env,
                "OPUTE_HOST_RESOURCE_TASK_CAPACITY",
                4096,
            ),
            allow_legacy_handshake: env.get("OPUTE_MCP_ALLOW_LEGACY_HANDSHAKE") == "true",
            prefix_tool_names: env.get("OPUTE_MCP_PREFIX_TOOL_NAMES") == "true",
            disable_localhost_protection: env.get("OPUTE_MCP_DISABLE_LOCALHOST_PROTECTION")
                == "true",
        }
    }

    /// `Config.Validate`: rejects ambiguous profiles before any listener,
    /// protocol output or control-plane contact. Order matters: the first
    /// failing check determines the message.
    pub fn validate(&self) -> Result<()> {
        let env = &self.env;
        let identity = match &self.identity {
            Err(e) => return Err(go_err!("host identity unavailable: {e}")),
            Ok(identity) => identity,
        };
        if identity.fingerprint.trim().is_empty()
            || identity.fingerprint_version.trim().is_empty()
            || identity.fingerprint_source.trim().is_empty()
        {
            return Err(go_err!(
                "host identity is incomplete: physical fingerprint, version, and source are required"
            ));
        }
        if identity.execution_context.id.trim().is_empty()
            || identity.execution_context.kind.trim().is_empty()
        {
            return Err(go_err!("execution context identity is required"));
        }
        if !self.tenant_id.is_empty() {
            validate_tenant_id(&self.tenant_id)?;
        }
        let mut instance_id = self.instance_id.trim().to_string();
        if instance_id.is_empty() {
            instance_id = if self.agent_mode.trim().eq_ignore_ascii_case("standalone") {
                "standalone".into()
            } else {
                "platform".into()
            };
        }
        validate_instance_id(&instance_id)?;
        if !self.ownership_mode.is_empty()
            && self.ownership_mode != "audit"
            && self.ownership_mode != "enforce"
        {
            return Err(go_err!(
                "invalid OPUTE_INCUS_OWNERSHIP_MODE {}: expected audit or enforce",
                quote(&self.ownership_mode)
            ));
        }
        let raw_mode = env.get("OPUTE_AGENT_MODE").trim().to_string();
        if !raw_mode.is_empty()
            && !raw_mode.eq_ignore_ascii_case("platform")
            && !raw_mode.eq_ignore_ascii_case("standalone")
        {
            return Err(go_err!(
                "invalid OPUTE_AGENT_MODE {}: expected platform or standalone",
                quote(&raw_mode)
            ));
        }
        let mut mode = self.agent_mode.trim().to_lowercase();
        if mode.is_empty() {
            mode = normalize_mode(&raw_mode);
        }
        if mode != "platform" && mode != "standalone" {
            return Err(go_err!(
                "invalid agent mode {}: expected platform or standalone",
                quote(&self.agent_mode)
            ));
        }
        if self.remote_agent_id.trim().is_empty() {
            return Err(go_err!(
                "OPUTE_REMOTE_AGENT_ID is required; the host agent must be onboarded with one canonical id"
            ));
        }
        validate_agent_id(&self.remote_agent_id)?;
        if self.host_mcp_port <= 0 {
            return Err(go_err!("HOST_MCP_PORT must be positive"));
        }
        if !self.incus_network_name.trim().is_empty() {
            validate_incus_network_name(&self.incus_network_name)?;
        }
        let address = self.incus_network_address.trim();
        if !address.is_empty() {
            match parse_cidr_ip(address) {
                None => {
                    return Err(go_err!(
                        "OPUTE_INCUS_NETWORK_ADDRESS must be an IPv4 CIDR: invalid CIDR address: {address}"
                    ))
                }
                Some(ip) if to4(ip).is_none() => {
                    return Err(go_err!("OPUTE_INCUS_NETWORK_ADDRESS must be an IPv4 CIDR"))
                }
                Some(_) => {}
            }
        }
        if self.host_resource_max_normal < 0
            || self.host_resource_max_heavy < 0
            || self.host_resource_max_queued < 0
        {
            return Err(go_err!("host resource operation limits cannot be negative"));
        }
        if self.host_resource_min_memory_bytes < 0
            || self.host_resource_min_disk_bytes < 0
            || self.host_resource_memory_capacity < 0
            || self.host_resource_disk_capacity < 0
            || self.host_resource_task_capacity < 0
        {
            return Err(go_err!(
                "host resource byte, disk, and task limits cannot be negative"
            ));
        }
        let cpu = self.host_resource_cpu_capacity;
        if cpu.is_nan() || cpu.is_infinite() || cpu < 0.0 {
            return Err(go_err!(
                "host resource CPU capacity must be finite and non-negative"
            ));
        }
        let raw_transport = env.get("OPUTE_TRANSPORT").trim().to_string();
        if !raw_transport.is_empty() && !raw_transport.eq_ignore_ascii_case("http") {
            return Err(go_err!(
                "invalid OPUTE_TRANSPORT {}: only Streamable HTTP (http) is supported",
                quote(&raw_transport)
            ));
        }
        let raw_provider = env.get("OPUTE_INFRA_PROVIDER_ID").trim().to_string();
        let mut provider = self.provider_id.trim().to_string();
        if !raw_provider.is_empty() {
            provider = raw_provider;
        }
        if !provider.is_empty() && !provider.eq_ignore_ascii_case("incus") {
            return Err(go_err!(
                "unsupported provider {}: only incus is supported",
                quote(&provider)
            ));
        }
        if env
            .get("OPUTE_REVERSE_TUNNEL")
            .trim()
            .eq_ignore_ascii_case("true")
        {
            return Err(go_err!(
                "OPUTE_REVERSE_TUNNEL is retired; the kernel serves mode-scoped Streamable HTTP POST /mcp"
            ));
        }
        for key in [
            "OPUTE_HOST_WS_URL",
            "OPUTE_CPC_TOKEN",
            "OPUTE_REMOTE_AGENT_AUTH_TOKEN",
            "OPUTE_ONBOARDING_TOKEN",
            "OPUTE_ONBOARDING_SESSION_ID",
        ] {
            if !env.get(key).trim().is_empty() {
                return Err(go_err!(
                    "{key} is retired; enroll a host resource URL instead of phone-home credentials"
                ));
            }
        }
        if mode == "standalone" {
            for key in ["OPUTE_MCP_URL", "OPUTE_MCP_HEALTH_URL"] {
                if !env.get(key).trim().is_empty() {
                    return Err(go_err!("standalone mode cannot use platform setting {key}"));
                }
            }
        }
        Ok(())
    }
}

fn validate_tenant_id(value: &str) -> Result<()> {
    let value = value.trim();
    if value.is_empty() {
        return Err(go_err!("OPUTE_TENANT_ID is required"));
    }
    if value.len() > 32 {
        return Err(go_err!("OPUTE_TENANT_ID must be at most 32 characters"));
    }
    for (i, ch) in value.char_indices() {
        let valid = ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-';
        if !valid || (i == 0 && ch == '-') {
            return Err(go_err!(
                "OPUTE_TENANT_ID {} is invalid: use [a-z][a-z0-9-]{{0,31}}",
                quote(value)
            ));
        }
    }
    Ok(())
}

fn validate_instance_id(value: &str) -> Result<()> {
    let value = value.trim();
    if value.is_empty() {
        return Err(go_err!("OPUTE_HOST_AGENT_INSTANCE is required"));
    }
    if value.len() > 63 {
        return Err(go_err!(
            "OPUTE_HOST_AGENT_INSTANCE must be at most 63 characters"
        ));
    }
    for (i, ch) in value.char_indices() {
        let valid = ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-';
        if !valid || (i == 0 && ch == '-') || (i == value.len() - 1 && ch == '-') {
            return Err(go_err!(
                "OPUTE_HOST_AGENT_INSTANCE {} is invalid: use [a-z0-9][a-z0-9-]{{0,62}}",
                quote(value)
            ));
        }
    }
    Ok(())
}

fn validate_agent_id(value: &str) -> Result<()> {
    let value = value.trim();
    if value.is_empty() {
        return Err(go_err!("OPUTE_REMOTE_AGENT_ID is required"));
    }
    if value.len() > 255 {
        return Err(go_err!(
            "OPUTE_REMOTE_AGENT_ID must be at most 255 characters"
        ));
    }
    if value.chars().any(|ch| !('\u{21}'..='\u{7e}').contains(&ch)) {
        return Err(go_err!(
            "OPUTE_REMOTE_AGENT_ID {} is invalid: use printable non-whitespace characters",
            quote(value)
        ));
    }
    Ok(())
}

fn validate_incus_network_name(value: &str) -> Result<()> {
    let value = value.trim();
    if value.is_empty() {
        return Err(go_err!("OPUTE_INCUS_NETWORK_NAME must not be empty"));
    }
    if value.len() > 63 {
        return Err(go_err!(
            "OPUTE_INCUS_NETWORK_NAME must be at most 63 characters"
        ));
    }
    if value
        .chars()
        .any(|ch| !(ch.is_ascii_alphanumeric() || ch == '-'))
    {
        return Err(go_err!(
            "OPUTE_INCUS_NETWORK_NAME {} is invalid: use letters, digits, and hyphens",
            quote(value)
        ));
    }
    Ok(())
}

/// Go `net.ParseCIDR`, returning only the address part.
fn parse_cidr_ip(s: &str) -> Option<IpAddr> {
    let (addr, mask) = s.split_once('/')?;
    if addr.contains('%') {
        return None;
    }
    let ip: IpAddr = addr.parse().ok()?;
    let bits: u64 = if ip.is_ipv4() { 32 } else { 128 };
    // Go dtoi: decimal digits only, leading zeros allowed, capped.
    if mask.is_empty() || !mask.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let mut n: u64 = 0;
    for b in mask.bytes() {
        n = n * 10 + u64::from(b - b'0');
        if n >= 0xFF_FFFF {
            return None;
        }
    }
    if n > bits {
        return None;
    }
    Some(ip)
}

/// Go `IP.To4()`: IPv4, or an IPv4-mapped IPv6 address.
fn to4(ip: IpAddr) -> Option<Ipv4Addr> {
    match ip {
        IpAddr::V4(v4) => Some(v4),
        IpAddr::V6(v6) => v6.to_ipv4_mapped(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Env {
        Env::from_pairs([
            ("HOME", "/home/test"),
            ("OPUTE_REMOTE_AGENT_ID", "agent-1"),
            ("OPUTE_AGENT_MODE", "standalone"),
        ])
    }

    fn validate_with(extra: &[(&str, &str)]) -> std::result::Result<(), String> {
        let mut env = base();
        for (k, v) in extra {
            env.set(k, v);
        }
        let mut cfg = Config::load(&env);
        cfg.identity = Ok(identity::test_identity());
        cfg.validate().map_err(|e| e.0)
    }

    #[test]
    fn defaults_follow_mode() {
        let s = Config::load(&base());
        assert_eq!(
            (s.host_mcp_port, s.host_mcp_bind_host.as_str()),
            (3014, "127.0.0.1")
        );
        assert_eq!(
            s.standalone_state_dir,
            PathBuf::from("/home/test/.config/opute/instances/standalone/state")
        );
        let mut env = base();
        env.set("OPUTE_AGENT_MODE", "platform");
        let p = Config::load(&env);
        assert_eq!(
            (p.host_mcp_port, p.host_mcp_bind_host.as_str()),
            (3004, "0.0.0.0")
        );
        assert_eq!(p.instance_id, "platform");
        env.set("OPUTE_AGENT_MODE", "");
        assert_eq!(Config::load(&env).agent_mode, "platform");
    }

    #[test]
    fn xdg_config_home_replaces_opute_config_dir() {
        let mut env = base();
        env.set("XDG_CONFIG_HOME", "/x");
        let c = Config::load(&env);
        assert_eq!(c.instance_root, PathBuf::from("/x/instances/standalone"));
        assert_eq!(
            c.host_resource_lock_dir,
            PathBuf::from("/x/host-resource-coordinator")
        );
    }

    #[test]
    fn env_value_strips_one_quote_pair() {
        let env = Env::from_pairs([("A", "  'x y'  "), ("B", "\"q\""), ("C", "'mismatch\"")]);
        assert_eq!(env.value("A"), "x y");
        assert_eq!(env.value("B"), "q");
        assert_eq!(env.value("C"), "'mismatch\"");
    }

    #[test]
    fn validation_messages_match_go() {
        assert_eq!(validate_with(&[]), Ok(()));
        assert_eq!(
            validate_with(&[("OPUTE_REMOTE_AGENT_ID", "  ")]),
            Err("OPUTE_REMOTE_AGENT_ID is required; the host agent must be onboarded with one canonical id".into())
        );
        assert_eq!(
            validate_with(&[("OPUTE_REMOTE_AGENT_ID", "a b")]),
            Err(
                "OPUTE_REMOTE_AGENT_ID \"a b\" is invalid: use printable non-whitespace characters"
                    .into()
            )
        );
        assert_eq!(
            validate_with(&[("OPUTE_AGENT_MODE", "standalonee")]),
            Err("invalid OPUTE_AGENT_MODE \"standalonee\": expected platform or standalone".into())
        );
        assert_eq!(
            validate_with(&[("HOST_MCP_PORT", "abc")]),
            Err("HOST_MCP_PORT must be positive".into())
        );
        assert_eq!(
            validate_with(&[("OPUTE_TENANT_ID", "-x")]),
            Err("OPUTE_TENANT_ID \"-x\" is invalid: use [a-z][a-z0-9-]{0,31}".into())
        );
        assert_eq!(
            validate_with(&[("OPUTE_INCUS_NETWORK_ADDRESS", "fd00::1/64")]),
            Err("OPUTE_INCUS_NETWORK_ADDRESS must be an IPv4 CIDR".into())
        );
        assert_eq!(
            validate_with(&[("OPUTE_INCUS_NETWORK_ADDRESS", "10.0.0.1")]),
            Err(
                "OPUTE_INCUS_NETWORK_ADDRESS must be an IPv4 CIDR: invalid CIDR address: 10.0.0.1"
                    .into()
            )
        );
        assert_eq!(
            validate_with(&[("OPUTE_INCUS_NETWORK_ADDRESS", "::ffff:10.0.0.1/120")]),
            Ok(())
        );
        assert_eq!(
            validate_with(&[("OPUTE_HOST_RESOURCE_CPU_CAPACITY_CORES", "NaN")]),
            Err("host resource CPU capacity must be finite and non-negative".into())
        );
        assert_eq!(
            validate_with(&[("OPUTE_HOST_RESOURCE_CPU_CAPACITY_CORES", "-3")]),
            Ok(())
        );
        assert_eq!(
            validate_with(&[("OPUTE_INFRA_PROVIDER_ID", "'incus'")]),
            Err("unsupported provider \"'incus'\": only incus is supported".into())
        );
        assert_eq!(
            validate_with(&[("OPUTE_MCP_HEALTH_URL", "x")]),
            Err("standalone mode cannot use platform setting OPUTE_MCP_HEALTH_URL".into())
        );
    }

    #[test]
    fn env_file_never_overrides_non_blank_values() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.env");
        std::fs::write(
            &path,
            "# comment\nexport A='one'\nB = two \nC=\"three\"\nA=ignored\nD=\n",
        )
        .unwrap();
        let mut env = Env::from_pairs([("B", "kept"), ("C", "   ")]);
        env.load_env_file(path.to_str().unwrap()).unwrap();
        assert_eq!(env.get("A"), "one");
        assert_eq!(env.get("B"), "kept");
        assert_eq!(env.get("C"), "three");
        assert_eq!(env.get("D"), "");
        std::fs::write(&path, "GOOD=1\nnoequals\n").unwrap();
        let err = Env::default()
            .load_env_file(path.to_str().unwrap())
            .unwrap_err();
        assert_eq!(
            err.0,
            format!("invalid env assignment at {}:2", path.display())
        );
        let err = Env::default()
            .load_env_file("/nonexistent/x.env")
            .unwrap_err();
        assert_eq!(
            err.0,
            "open env file \"/nonexistent/x.env\": open /nonexistent/x.env: no such file or directory"
        );
    }
}
