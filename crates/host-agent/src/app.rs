//! Composition root: configuration check and the ordered server lifecycle.
//!
//! Startup opens resources in the Go baseline's order and stops at the first
//! failure, before any listener exists:
//!
//! ```text
//!   config.Load → Validate → platform check
//!     → resource coordinator lock dir (0700)
//!     → state store (state.db)
//!     → authz store (authz.sqlite, built-in clients)
//!     → "HTTP transport listening" → bind → serve until SIGINT/SIGTERM
//! ```
//!
//! Every opened resource is owned by a local that is dropped in reverse
//! order, so a failure at any stage releases what earlier stages opened (and
//! checkpoints the SQLite WAL) exactly once.

use std::io::Write;
use std::net::{SocketAddr, ToSocketAddrs};

use crate::config::{Config, Env};
use crate::goerr::{self, Result};
use crate::store::{AuthzStore, StateStore};

/// Fault injection for lifecycle tests: fail the named startup stage. The
/// public entry points always pass `None`, so it is not a runtime surface.
fn inject_fault(fault: Option<&str>, stage: &str) -> Result<()> {
    if fault == Some(stage) {
        return Err(go_err!("injected fault at {stage}"));
    }
    Ok(())
}

fn require_supported_platform() -> Result<()> {
    if cfg!(target_os = "linux") {
        Ok(())
    } else {
        Err(go_err!(
            "opute-host-agent requires Linux (native or WSL); unsupported platform {}",
            goerr::quote(std::env::consts::OS)
        ))
    }
}

/// `app.validateConfig`.
fn validate(cfg: &Config) -> Result<()> {
    cfg.validate()?;
    // Every provider value normalizes to incus; Validate already rejected
    // anything else, so only the platform guard remains.
    require_supported_platform()
}

/// `app.Check`: validate configuration and state access without a listener.
pub fn check(env: &Env) -> Result<()> {
    let cfg = Config::load(env);
    validate(&cfg)?;
    if cfg.agent_mode == "standalone" {
        let mut store = StateStore::open(&cfg.standalone_state_dir)?;
        store.close();
    }
    Ok(())
}

/// The host runtime resources `app.NewRuntime` opens before any transport.
pub struct Runtime {
    pub config: Config,
    // Field order is drop order: state is released before anything declared
    // later in `run` (which is dropped first because locals drop in reverse).
    pub state: StateStore,
}

/// `app.NewRuntime` for M1: validation, coordinator lock dir, state store.
pub fn new_runtime(env: &Env) -> Result<Runtime> {
    new_runtime_with(env, None)
}

fn new_runtime_with(env: &Env, fault: Option<&str>) -> Result<Runtime> {
    let cfg = Config::load(env);
    validate(&cfg)?;
    inject_fault(fault, "coordinator")?;
    goerr::mkdir_all(&cfg.host_resource_lock_dir, 0o700)?;
    inject_fault(fault, "state")?;
    let state = StateStore::open(&cfg.standalone_state_dir)?;
    Ok(Runtime { config: cfg, state })
}

/// A Go `log/slog` TextHandler line at INFO level.
fn log_info(stderr: &mut dyn Write, msg: &str, attrs: &[(&str, &str)]) {
    let mut line = format!("time={} level=INFO msg={}", slog_time(), slog_value(msg));
    for (k, v) in attrs {
        line.push_str(&format!(" {k}={}", slog_value(v)));
    }
    let _ = writeln!(stderr, "{line}");
    let _ = stderr.flush();
}

fn slog_value(v: &str) -> String {
    let needs_quote = v.is_empty()
        || v.chars()
            .any(|c| c == ' ' || c == '=' || c == '"' || c.is_control());
    if needs_quote {
        goerr::quote(v)
    } else {
        v.to_string()
    }
}

/// slog's default time format, "2006-01-02T15:04:05.000Z07:00", in UTC.
fn slog_time() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs() as i64;
    let millis = now.subsec_millis();
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Howard Hinnant's days-to-civil algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Go `net.SplitHostPort` + `net.Listen("tcp", addr)` address resolution,
/// with Go's error text.
fn resolve_listen_addr(addr: &str) -> Result<Vec<SocketAddr>> {
    let fail = |reason: &str| go_err!("listen tcp: address {addr}: {reason}");
    let (host, port) = if let Some(rest) = addr.strip_prefix('[') {
        let end = rest
            .find(']')
            .ok_or_else(|| fail("missing ']' in address"))?;
        let host = &rest[..end];
        let after = &rest[end + 1..];
        let port = after
            .strip_prefix(':')
            .ok_or_else(|| fail("missing port in address"))?;
        (host.to_string(), port.to_string())
    } else {
        let idx = addr
            .rfind(':')
            .ok_or_else(|| fail("missing port in address"))?;
        let host = &addr[..idx];
        if host.contains(':') {
            return Err(fail("too many colons in address"));
        }
        (host.to_string(), addr[idx + 1..].to_string())
    };
    let port: u16 = match port.parse::<u32>() {
        Ok(p) if p <= 65_535 => p as u16,
        _ => return Err(go_err!("listen tcp: address {port}: invalid port")),
    };
    let host = if host.is_empty() {
        "0.0.0.0".to_string()
    } else {
        host
    };
    (host.as_str(), port)
        .to_socket_addrs()
        .map(|it| it.collect())
        .map_err(|_| go_err!("listen tcp: lookup {host}: no such host"))
}

/// `app.Run`: build the runtime, open authz, bind, and serve until SIGINT or
/// SIGTERM. A graceful stop returns `Ok`, which exits 0 like Go.
pub fn run(env: &Env, stderr: &mut dyn Write) -> Result<()> {
    run_with(env, stderr, None)
}

fn run_with(env: &Env, stderr: &mut dyn Write, fault: Option<&str>) -> Result<()> {
    let mut runtime = new_runtime_with(env, fault)?;
    let cfg = runtime.config.clone();
    let authz_dir = if cfg.standalone_state_dir.as_os_str().is_empty() {
        cfg.instance_root.clone()
    } else {
        cfg.standalone_state_dir.clone()
    };
    inject_fault(fault, "authz")?;
    let mut authz = AuthzStore::open(&authz_dir, &cfg.opute_client_secret)?;

    let addr = format!("{}:{}", cfg.host_mcp_bind_host, cfg.host_mcp_port);
    if addr.ends_with(":0") {
        return Err(go_err!(
            "HOST_MCP_PORT must be positive for direct HTTP mode"
        ));
    }
    log_info(stderr, "HTTP transport listening", &[("addr", &addr)]);
    inject_fault(fault, "listener")?;
    let result = serve(&addr);
    // Reverse order: transport has stopped; release authz, then state.
    authz.close();
    runtime.state.close();
    result
}

fn serve(addr: &str) -> Result<()> {
    let addrs = resolve_listen_addr(addr)?;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
        .map_err(|e| go_err!("start runtime: {e}"))?;
    rt.block_on(async move {
        let listener = bind_first(&addrs, addr).await?;
        let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
            .map_err(|e| go_err!("install signal handler: {e}"))?;
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .map_err(|e| go_err!("install signal handler: {e}"))?;
        loop {
            tokio::select! {
                _ = sigint.recv() => return Ok(()),
                _ = sigterm.recv() => return Ok(()),
                accepted = listener.accept() => {
                    if let Ok((stream, _)) = accepted {
                        tokio::spawn(placeholder_response(stream));
                    }
                }
            }
        }
    })
}

async fn bind_first(addrs: &[SocketAddr], addr: &str) -> Result<tokio::net::TcpListener> {
    let mut last = None;
    for candidate in addrs {
        match tokio::net::TcpListener::bind(candidate).await {
            Ok(l) => return Ok(l),
            Err(e) => last = Some(e),
        }
    }
    let e = last.unwrap_or_else(|| std::io::Error::from_raw_os_error(99));
    Err(go_err!(
        "listen tcp {addr}: bind: {}",
        goerr::errno_text(&e)
    ))
}

/// M1 owns the listener lifecycle only. HTTP routes (`/health`, `/mcp`,
/// OAuth) arrive in M2; until then every request is answered 501 so a
/// client never mistakes this build for a working agent.
async fn placeholder_response(mut stream: tokio::net::TcpStream) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut buf = [0u8; 4096];
    let _ = tokio::time::timeout(std::time::Duration::from_secs(2), stream.read(&mut buf)).await;
    let body = "not implemented until milestone M2\n";
    let response = format!(
        "HTTP/1.1 501 Not Implemented\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(dir: &std::path::Path, extra: &[(&str, &str)]) -> Env {
        let mut env = Env::from_pairs([
            ("HOME", dir.join("home").to_str().unwrap()),
            ("XDG_CONFIG_HOME", dir.join("xdg").to_str().unwrap()),
            ("OPUTE_REMOTE_AGENT_ID", "agent-under-test"),
            ("OPUTE_AGENT_MODE", "standalone"),
            (
                "OPUTE_STANDALONE_STATE_DIR",
                dir.join("state").to_str().unwrap(),
            ),
            ("HOST_MCP_BIND_HOST", "127.0.0.1"),
            ("HOST_MCP_PORT", "1"),
        ]);
        for (k, v) in extra {
            env.set(k, v);
        }
        env
    }

    fn listing(dir: &std::path::Path) -> Vec<String> {
        let mut out = vec![];
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap() {
                let p = e.unwrap().path();
                out.push(p.strip_prefix(dir).unwrap().display().to_string());
                if p.is_dir() {
                    stack.push(p);
                }
            }
        }
        out.sort();
        out
    }

    #[test]
    fn each_stage_failure_stops_startup_and_releases_earlier_stages() {
        // (fault stage, files that must exist afterwards)
        let cases: [(&str, &[&str]); 3] = [
            ("coordinator", &[]),
            ("state", &["xdg", "xdg/host-resource-coordinator"]),
            (
                "authz",
                &[
                    "state",
                    "state/state.db",
                    "xdg",
                    "xdg/host-resource-coordinator",
                ],
            ),
        ];
        for (stage, expected) in cases {
            let dir = tempfile::tempdir().unwrap();
            let env = env(dir.path(), &[]);
            let mut sink = Vec::new();
            let err = run_with(&env, &mut sink, Some(stage)).unwrap_err();
            assert_eq!(err.0, format!("injected fault at {stage}"));
            assert!(sink.is_empty(), "{stage}: no listener log before failure");
            assert_eq!(
                listing(dir.path()),
                expected.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
                "{stage}"
            );
        }
    }

    #[test]
    fn listener_failure_happens_after_the_listening_log_like_go() {
        let dir = tempfile::tempdir().unwrap();
        let env = env(dir.path(), &[]);
        let mut sink = Vec::new();
        assert!(run_with(&env, &mut sink, Some("listener")).is_err());
        let log = String::from_utf8(sink).unwrap();
        assert!(
            log.contains(" level=INFO msg=\"HTTP transport listening\" addr=127.0.0.1:1\n"),
            "{log}"
        );
        let files = listing(dir.path());
        assert!(files.contains(&"state/authz.sqlite".to_string()));
        assert!(
            !files.iter().any(|f| f.ends_with("-wal")),
            "stores closed: {files:?}"
        );
    }

    #[test]
    fn listen_address_errors_match_go() {
        assert_eq!(
            resolve_listen_addr("::1:3014").unwrap_err().0,
            "listen tcp: address ::1:3014: too many colons in address"
        );
        assert_eq!(
            resolve_listen_addr("127.0.0.1:70000").unwrap_err().0,
            "listen tcp: address 70000: invalid port"
        );
        assert_eq!(resolve_listen_addr("[::1]:3014").unwrap().len(), 1);
    }

    #[test]
    fn check_opens_state_only_in_standalone() {
        let dir = tempfile::tempdir().unwrap();
        check(&env(dir.path(), &[])).unwrap();
        assert!(dir.path().join("state/state.db").exists());
        let dir = tempfile::tempdir().unwrap();
        check(&env(dir.path(), &[("OPUTE_AGENT_MODE", "platform")])).unwrap();
        assert!(!dir.path().join("state").exists());
    }

    #[test]
    fn slog_formatting() {
        assert_eq!(
            slog_value("HTTP transport listening"),
            "\"HTTP transport listening\""
        );
        assert_eq!(slog_value("127.0.0.1:3014"), "127.0.0.1:3014");
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_726), (2026, 9, 30));
    }
}
