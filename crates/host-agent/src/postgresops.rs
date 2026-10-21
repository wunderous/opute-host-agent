//! M8e: Go's `internal/domain/postgres`, the host's SQL surface: the
//! CloudNativePG service reconciled onto a cluster, the authenticated relay
//! that lets a client reach it, the reference-counted host TCP connector, and
//! local SQLite databases.
//!
//! Every kubectl call goes through the kubernetes domain, which reaches the
//! active Kubernetes provider's exec-command operation (`Deps`); the relay's
//! loopback forward is an Incus proxy device (`Shared.CommandRunner`).
//!
//! As in Go, the domain is built once, on its first call: that is when a
//! persisted relay is restored.

use crate::goerr::{quote, shell_quote};
use crate::incus::first_non_empty;
use crate::plan::runner::RunCtx;
use base64::Engine as _;
use serde_json::{json, Map, Value as J};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{IpAddr, Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

/// `defaultDiscoveryTimeout`: a read-only kubectl call's budget. The provider
/// path does not apply it (`runProviderCommand` ignores the timeout); it is
/// kept for the shape of each call.
const OPERATOR_RELEASE: &str = "cloudnativepg";
const OPERATOR_NAMESPACE: &str = "cnpg-system";
const OPERAND_IMAGE: &str = "ghcr.io/cloudnative-pg/postgresql:16-minimal-trixie";
const SERVICE_STORAGE_SIZE: &str = "10Gi";
const SERVICE_PORT: i64 = 5432;
const SERVICE_READINESS_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const RESOURCE_PROFILE: &str = "constrained-2GiB-v3";
const TICK: Duration = Duration::from_secs(2);

const CONNECTOR_MAX_PER_HOST: usize = 32;
const CONNECTOR_IDLE_DRAIN: Duration = Duration::from_secs(120);

/// The postgres domain's seams. It is cheap to clone; the stateful part
/// (`Live`) is built on first use and shared.
#[derive(Clone)]
pub struct Postgres {
    pub k8s: crate::clusterops::Kubernetes,
    pub incus: crate::incus::Incus,
    /// `filepath.Join(RelayConfigDir, "postgresql-service-relays")`, or
    /// empty to keep relays in memory only.
    pub relay_config_dir: String,
    pub sqlite_root: String,
    pub live: Arc<OnceLock<Arc<Live>>>,
}

/// What Go's `postgres.New` builds: the two relay holders own live
/// listeners, so they exist once per host service.
pub struct Live {
    pub relay: Arc<RelayManager>,
    connector: Connectors,
}

impl Postgres {
    /// `Service.Postgres()`: the domain, constructed (and its persisted
    /// relays restored) on first use.
    fn live(&self) -> &Arc<Live> {
        self.live.get_or_init(|| {
            Arc::new(Live {
                relay: Arc::new(RelayManager::persistent_at(&self.relay_config_dir)),
                connector: Connectors::default(),
            })
        })
    }

    fn kubectl(&self, vm: &str, args: &[String], label: &str) -> Result<String, String> {
        self.k8s.run_kubectl(vm, args, label)
    }

    fn kubectl_input(
        &self,
        vm: &str,
        args: &[String],
        input: &[u8],
        label: &str,
    ) -> Result<String, String> {
        self.k8s.run_kubectl_input(vm, args, Some(input), label)
    }
}

fn argv(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| s.to_string()).collect()
}

// --- SQLite databases (sqlite_database.go) -----------------------------------------------

#[derive(Clone, Debug, Default)]
pub struct SqliteArgs {
    pub consumer_id: String,
    pub database_name: String,
}

/// `sqliteDatabaseNamePattern`: `^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$`.
fn sqlite_name(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 64
        && bytes[0].is_ascii_alphanumeric()
        && bytes[1..]
            .iter()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
}

/// `SQLiteDatabaseResult`, in its field order.
fn sqlite_result(args: &SqliteArgs, path: &str, exists: bool, created: bool) -> J {
    let mut out = Map::new();
    out.insert("provider".into(), J::from("sqlite"));
    out.insert("consumerId".into(), J::from(args.consumer_id.trim()));
    out.insert("databaseName".into(), J::from(args.database_name.trim()));
    out.insert("path".into(), J::from(path));
    out.insert("exists".into(), J::Bool(exists));
    out.insert("created".into(), J::Bool(created));
    J::Object(out)
}

impl Postgres {
    /// `sqliteDatabasePath`.
    fn sqlite_path(&self, args: &SqliteArgs) -> Result<String, String> {
        let consumer = args.consumer_id.trim();
        let database = args.database_name.trim();
        if !sqlite_name(consumer) {
            return Err("invalid SQLite consumerId".into());
        }
        if !sqlite_name(database) {
            return Err("invalid SQLite databaseName".into());
        }
        let root = self.sqlite_root.trim();
        if root.is_empty() {
            return Err("SQLite database root is not configured".into());
        }
        let root = crate::clusterops::absolute(root)
            .map_err(|e| format!("resolve SQLite database root: {e}"))?;
        let path = crate::clusterops::clean(&format!("{root}/{consumer}/{database}.sqlite"));
        // `filepath.Rel(root, path)` never leaves the root: neither name may
        // hold a separator, and neither may start with a dot.
        if !path.starts_with(&format!("{}/", root.trim_end_matches('/'))) {
            return Err("SQLite database path escapes the managed root".into());
        }
        Ok(path)
    }

    /// `EnsureSQLiteDatabase`.
    pub fn ensure_sqlite(&self, args: &SqliteArgs) -> Result<J, String> {
        let path = self.sqlite_path(args)?;
        let dir = Path::new(&path).parent().unwrap_or(Path::new("/"));
        mkdir_all(dir, 0o700).map_err(|e| format!("create SQLite database directory: {e}"))?;
        let created = match std::fs::metadata(&path) {
            Ok(_) => false,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
            Err(e) => {
                return Err(format!(
                    "inspect SQLite database: {}",
                    crate::goerr::path_error("stat", Path::new(&path), &e).0
                ))
            }
        };
        let conn = rusqlite::Connection::open(&path)
            .map_err(|e| format!("ping SQLite database: {}", sqlite_error(&e)))?;
        conn.execute_batch("PRAGMA busy_timeout = 5000; PRAGMA journal_mode = WAL;")
            .map_err(|e| format!("configure SQLite database: {}", sqlite_error(&e)))?;
        drop(conn);
        Ok(sqlite_result(args, &path, true, created))
    }

    /// `GetSQLiteDatabaseStatus`.
    pub fn sqlite_status(&self, args: &SqliteArgs) -> Result<J, String> {
        let path = self.sqlite_path(args)?;
        let exists = match std::fs::metadata(&path) {
            Ok(_) => true,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
            Err(e) => {
                return Err(format!(
                    "inspect SQLite database: {}",
                    crate::goerr::path_error("stat", Path::new(&path), &e).0
                ))
            }
        };
        Ok(sqlite_result(args, &path, exists, false))
    }

    /// `RemoveSQLiteDatabase`.
    pub fn remove_sqlite(&self, args: &SqliteArgs, confirm: bool) -> Result<J, String> {
        if !confirm {
            return Err("remove SQLite database requires confirm=true".into());
        }
        let path = self.sqlite_path(args)?;
        for candidate in [path.clone(), format!("{path}-wal"), format!("{path}-shm")] {
            match std::fs::remove_file(&candidate) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    return Err(format!(
                        "remove SQLite database file: {}",
                        crate::goerr::path_error("remove", Path::new(&candidate), &e).0
                    ))
                }
            }
        }
        Ok(sqlite_result(args, &path, false, false))
    }
}

/// The SQLite engine's own message for a failed call.
fn sqlite_error(e: &rusqlite::Error) -> String {
    match e {
        rusqlite::Error::SqliteFailure(_, Some(message)) => message.clone(),
        other => other.to_string(),
    }
}

fn path_err(op: &str, path: &Path, e: &std::io::Error) -> String {
    crate::goerr::path_error(op, path, e).0
}

/// `os.CreateTemp(dir, prefix+"*"+suffix)`: a new private file with a random
/// name.
fn create_temp(dir: &Path, prefix: &str, suffix: &str) -> Result<(PathBuf, std::fs::File), String> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut last = None;
    for _ in 0..10000 {
        let mut random = [0u8; 4];
        getrandom::getrandom(&mut random).map_err(|e| e.to_string())?;
        let path = dir.join(format!("{prefix}{}{suffix}", u32::from_le_bytes(random)));
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
        {
            Ok(file) => return Ok((path, file)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => last = Some(e),
            Err(e) => return Err(path_err("open", &path, &e)),
        }
    }
    Err(last.map_or_else(String::new, |e| e.to_string()))
}

/// `json.Unmarshal` of kubectl output: Go's error text for invalid JSON.
fn unmarshal(data: &str) -> Result<J, String> {
    if data.is_empty() {
        return Err("unexpected end of JSON input".into());
    }
    if let Some(e) = crate::gojson::syntax_error(data.as_bytes()) {
        return Err(e);
    }
    serde_json::from_str(data).map_err(|e| e.to_string())
}

fn kind(value: &J) -> &'static str {
    match value {
        J::Object(_) => "object",
        J::Array(_) => "array",
        J::String(_) => "string",
        J::Bool(_) => "bool",
        J::Null => "null",
        J::Number(_) => "number",
    }
}

/// `json.Unmarshal` into `map[string]any`.
fn unmarshal_map(data: &str) -> Result<J, String> {
    match unmarshal(data)? {
        value @ J::Object(_) => Ok(value),
        J::Null => Ok(J::Object(Map::new())),
        other => Err(format!(
            "json: cannot unmarshal {} into Go value of type map[string]interface {{}}",
            kind(&other)
        )),
    }
}

/// `json.Unmarshal` into a struct the code reads a few fields of: a value
/// that is not an object reads as empty.
fn unmarshal_struct(data: &str) -> Result<J, String> {
    match unmarshal(data)? {
        value @ J::Object(_) => Ok(value),
        _ => Ok(J::Object(Map::new())),
    }
}

/// `os.MkdirAll(path, perm)`.
fn mkdir_all(path: &Path, mode: u32) -> Result<(), String> {
    crate::goerr::mkdir_all(path, mode).map_err(|e| e.0)
}

// --- SQL connector (sql_connector.go) ------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub struct ConnectorArgs {
    pub database_id: String,
    pub target_host: String,
    pub target_port: i64,
    pub listen_port: i64,
    pub listen_host: String,
}

struct ConnectorSession {
    session_id: String,
    listen_host: String,
    listen_port: i64,
    target_host: String,
    target_port: i64,
    ref_count: i64,
    /// A pending idle drain; setting it cancels the drain (`Timer.Stop`).
    idle: Option<Arc<AtomicBool>>,
}

#[derive(Default)]
struct Connectors {
    relay: crate::tcprelay::Manager,
    sessions: Mutex<BTreeMap<String, ConnectorSession>>,
}

/// `SQLConnectorResult`, in its field order. The dispatch boundary fills
/// `uri`.
fn connector_result(database: &str, session: &ConnectorSession) -> Map<String, J> {
    let mut out = Map::new();
    out.insert("databaseId".into(), J::from(database));
    out.insert("uri".into(), J::from(""));
    out.insert("sessionId".into(), J::from(session.session_id.clone()));
    out.insert("listenHost".into(), J::from(session.listen_host.clone()));
    out.insert("listenPort".into(), J::from(session.listen_port));
    out.insert("pathMode".into(), J::from("host_tcp_relay"));
    out.insert("refCount".into(), J::from(session.ref_count));
    out
}

impl Postgres {
    /// `EnsureSQLConnector`.
    pub fn ensure_connector(&self, args: &ConnectorArgs) -> Result<Map<String, J>, String> {
        let supervisor = &self.live().connector;
        let database = args.database_id.trim().to_string();
        if database.is_empty() {
            return Err("databaseId is required".into());
        }
        {
            let mut sessions = supervisor.sessions.lock().unwrap();
            if let Some(existing) = sessions.get_mut(&database) {
                if let Some(idle) = existing.idle.take() {
                    idle.store(true, Ordering::SeqCst);
                }
                existing.ref_count += 1;
                return Ok(connector_result(&database, existing));
            }
            if sessions.len() >= CONNECTOR_MAX_PER_HOST {
                return Err(format!(
                    "host SQL connector limit reached ({CONNECTOR_MAX_PER_HOST})"
                ));
            }
        }
        // `sessionIDForDatabase`: deliberately not the canonical URI, which
        // carries a tenant this supervisor does not know.
        let session_id = format!("sql-connector:{database}");
        let bound = supervisor.relay.start(
            &session_id,
            &args.listen_host,
            args.listen_port,
            &args.target_host,
            args.target_port,
        )?;
        let session = ConnectorSession {
            session_id,
            listen_host: bound.listen_host,
            listen_port: bound.listen_port,
            target_host: bound.target_host,
            target_port: bound.target_port,
            ref_count: 1,
            idle: None,
        };
        let out = connector_result(&database, &session);
        supervisor
            .sessions
            .lock()
            .unwrap()
            .insert(database, session);
        Ok(out)
    }

    /// `GetSQLConnectorStatus`.
    pub fn connector_status(&self, database: &str) -> Map<String, J> {
        let database = database.trim();
        let sessions = self.live().connector.sessions.lock().unwrap();
        let mut out = Map::new();
        out.insert("databaseId".into(), J::from(database));
        match sessions.get(database) {
            None => {
                out.insert("active".into(), J::Bool(false));
                out.insert("refCount".into(), J::from(0));
            }
            Some(session) => {
                out.insert("active".into(), J::Bool(true));
                out.insert("sessionId".into(), J::from(session.session_id.clone()));
                out.insert("listenHost".into(), J::from(session.listen_host.clone()));
                out.insert("listenPort".into(), J::from(session.listen_port));
                out.insert("refCount".into(), J::from(session.ref_count));
                out.insert("targetHost".into(), J::from(session.target_host.clone()));
                out.insert("targetPort".into(), J::from(session.target_port));
            }
        }
        out
    }

    /// `ReleaseSQLConnector`: the last release drains after an idle period;
    /// `force` drains now.
    pub fn release_connector(&self, database: &str, force: bool) -> Result<bool, String> {
        let live = Arc::clone(self.live());
        let database = database.trim().to_string();
        let mut sessions = live.connector.sessions.lock().unwrap();
        let Some(session) = sessions.get_mut(&database) else {
            return Ok(false);
        };
        if force {
            drop(sessions);
            live.connector.drain(&database);
            return Ok(true);
        }
        session.ref_count -= 1;
        if session.ref_count > 0 {
            return Ok(true);
        }
        if let Some(idle) = session.idle.take() {
            idle.store(true, Ordering::SeqCst);
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        session.idle = Some(cancelled.clone());
        drop(sessions);
        std::thread::spawn(move || {
            std::thread::sleep(CONNECTOR_IDLE_DRAIN);
            if cancelled.load(Ordering::SeqCst) {
                return;
            }
            let idle = {
                let sessions = live.connector.sessions.lock().unwrap();
                sessions
                    .get(&database)
                    .is_some_and(|current| current.ref_count <= 0)
            };
            if idle {
                live.connector.drain(&database);
            }
        });
        Ok(true)
    }
}

impl Connectors {
    /// `drainSession`.
    fn drain(&self, database: &str) {
        let session = {
            let mut sessions = self.sessions.lock().unwrap();
            let Some(session) = sessions.remove(database) else {
                return;
            };
            if let Some(idle) = &session.idle {
                idle.store(true, Ordering::SeqCst);
            }
            session
        };
        self.relay.stop(&session.session_id);
    }
}

// --- The authenticated relay (platform_postgres_relay.go) ----------------------------------

#[derive(Clone, Debug, Default)]
pub struct RelayArgs {
    pub session_id: String,
    pub listen_host: String,
    pub listen_port: i64,
    pub target_host: String,
    pub target_port: i64,
    pub ttl_seconds: i64,
    pub relay_token: String,
    pub persistent: bool,
    pub replace_existing: bool,
}

struct RelaySession {
    id: String,
    listen_host: String,
    listen_port: i64,
    token_hash: [u8; 32],
    listener: TcpListener,
    target_host: String,
    target_port: i64,
    persistent: bool,
    expires_at: Option<SystemTime>,
    connections: Mutex<(bool, Vec<TcpStream>)>,
}

impl RelaySession {
    fn add_connection(&self, conn: &TcpStream) -> bool {
        let mut guard = self.connections.lock().unwrap();
        if guard.0 {
            return false;
        }
        if let Ok(clone) = conn.try_clone() {
            guard.1.push(clone);
        }
        true
    }

    fn remove_connection(&self, conn: &TcpStream) {
        let key = (conn.peer_addr().ok(), conn.local_addr().ok());
        self.connections
            .lock()
            .unwrap()
            .1
            .retain(|c| (c.peer_addr().ok(), c.local_addr().ok()) != key);
    }

    fn is_open(&self) -> bool {
        !self.connections.lock().unwrap().0
    }

    fn expired(&self) -> bool {
        !self.persistent && self.expires_at.is_some_and(|at| SystemTime::now() > at)
    }

    fn active_connection_count(&self) -> usize {
        self.connections.lock().unwrap().1.len()
    }

    /// `close`: the listener, then every connection.
    fn close(&self) {
        let _ =
            nix::sys::socket::shutdown(self.listener.as_raw_fd(), nix::sys::socket::Shutdown::Both);
        let connections = {
            let mut guard = self.connections.lock().unwrap();
            guard.0 = true;
            std::mem::take(&mut guard.1)
        };
        for conn in connections {
            let _ = conn.shutdown(Shutdown::Both);
        }
    }

    /// `relayDescriptor`.
    fn descriptor(&self) -> Map<String, J> {
        let mut out = Map::new();
        out.insert("sessionId".into(), J::from(self.id.clone()));
        out.insert("listenHost".into(), J::from(self.listen_host.clone()));
        out.insert("listenPort".into(), J::from(self.listen_port));
        out.insert("targetPort".into(), J::from(self.target_port));
        out.insert("authMode".into(), J::from("token-line"));
        out.insert("persistent".into(), J::Bool(self.persistent));
        out.insert("ready".into(), J::Bool(true));
        if !self.persistent {
            if let Some(at) = self.expires_at {
                out.insert(
                    "expiresAt".into(),
                    J::from(crate::hostobs::rfc3339_nano(at)),
                );
            }
        }
        out
    }
}

/// `postgresqlServiceRelayManager`.
pub struct RelayManager {
    sessions: Mutex<BTreeMap<String, Arc<RelaySession>>>,
    config_dir: Option<PathBuf>,
}

/// `postgresqlServiceRelayState`, in its field order.
#[derive(serde::Deserialize, Default)]
struct RelayState {
    #[serde(rename = "sessionId", default)]
    session_id: String,
    #[serde(rename = "listenHost", default)]
    listen_host: String,
    #[serde(rename = "listenPort", default)]
    listen_port: i64,
    #[serde(rename = "targetHost", default)]
    target_host: String,
    #[serde(rename = "targetPort", default)]
    target_port: i64,
    #[serde(rename = "tokenHash", default)]
    token_hash: String,
    #[serde(default)]
    persistent: bool,
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

/// `subtle.ConstantTimeCompare` over two digests.
fn same_digest(a: &[u8; 32], b: &[u8; 32]) -> bool {
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// `net.ParseIP(value).IsLoopback()`, an IPv4-mapped address included.
fn is_loopback_ip(value: &str) -> bool {
    match value.trim().parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => ip.is_loopback(),
        Ok(IpAddr::V6(ip)) => ip
            .to_ipv4_mapped()
            .map_or(ip.is_loopback(), |v4| v4.is_loopback()),
        Err(_) => false,
    }
}

/// `net.JoinHostPort`.
fn join_host_port(host: &str, port: i64) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

impl RelayManager {
    /// `newPersistentPostgreSQLServiceRelayManagerAt`.
    fn persistent_at(config_dir: &str) -> Self {
        let config_dir = config_dir.trim();
        let manager = RelayManager {
            sessions: Mutex::default(),
            config_dir: (!config_dir.is_empty()).then(|| PathBuf::from(config_dir)),
        };
        if manager.config_dir.is_some() {
            manager.restore();
        }
        manager
    }

    fn state_path(&self, session_id: &str) -> Option<PathBuf> {
        self.config_dir.as_ref().map(|dir| {
            dir.join(format!(
                "{}.json",
                hex::encode(sha256(session_id.as_bytes()))
            ))
        })
    }

    /// `persistSession`: a temporary file, made private, then renamed.
    fn persist(&self, session: &RelaySession) -> Result<(), String> {
        let (Some(dir), true) = (&self.config_dir, session.persistent) else {
            return Ok(());
        };
        mkdir_all(dir, 0o700)
            .map_err(|e| format!("create PostgreSQL service relay state directory: {e}"))?;
        // `json.Marshal(postgresqlServiceRelayState)`, in its field order.
        let mut data = String::from("{\"sessionId\":");
        crate::gojson::encode_string(&session.id, &mut data);
        data.push_str(",\"listenHost\":");
        crate::gojson::encode_string(&session.listen_host, &mut data);
        data.push_str(&format!(
            ",\"listenPort\":{},\"targetHost\":",
            session.listen_port
        ));
        crate::gojson::encode_string(&session.target_host, &mut data);
        data.push_str(&format!(
            ",\"targetPort\":{},\"tokenHash\":\"{}\",\"persistent\":true}}",
            session.target_port,
            hex::encode(session.token_hash)
        ));
        let (temporary, mut file) = create_temp(dir, ".relay-", ".tmp")
            .map_err(|e| format!("create PostgreSQL service relay state: {e}"))?;
        let result = (|| {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))
                .map_err(|e| {
                    format!(
                        "protect PostgreSQL service relay state: {}",
                        path_err("chmod", &temporary, &e)
                    )
                })?;
            file.write_all(data.as_bytes()).map_err(|e| {
                format!(
                    "write PostgreSQL service relay state: {}",
                    path_err("write", &temporary, &e)
                )
            })?;
            drop(file);
            let path = self
                .state_path(&session.id)
                .expect("a configured directory");
            std::fs::rename(&temporary, &path).map_err(|e| {
                format!(
                    "commit PostgreSQL service relay state: rename {} {}: {}",
                    temporary.display(),
                    path.display(),
                    crate::goerr::errno_text(&e)
                )
            })
        })();
        let _ = std::fs::remove_file(&temporary);
        result
    }

    /// `removePersistedSession`.
    fn remove_persisted(&self, session_id: &str) {
        if session_id.trim().is_empty() {
            return;
        }
        if let Some(path) = self.state_path(session_id) {
            let _ = std::fs::remove_file(path);
        }
    }

    /// `restore`: every valid persisted relay listens again; an invalid one
    /// is reported and left on disk.
    fn restore(&self) {
        let dir = self.config_dir.as_ref().expect("a configured directory");
        let mut entries: Vec<_> = match std::fs::read_dir(dir) {
            Ok(entries) => entries.filter_map(Result::ok).collect(),
            Err(e) => {
                if e.kind() != std::io::ErrorKind::NotFound {
                    eprintln!(
                        "restore PostgreSQL service relay state: {}",
                        crate::goerr::path_error("open", dir, &e).0
                    );
                }
                return;
            }
        };
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let name = entry.file_name().to_string_lossy().into_owned();
            if entry.file_type().is_ok_and(|t| t.is_dir()) || !name.ends_with(".json") {
                continue;
            }
            let data = match std::fs::read(entry.path()) {
                Ok(data) => data,
                Err(e) => {
                    eprintln!("read PostgreSQL service relay state {name}: {e}");
                    continue;
                }
            };
            let state: RelayState = match serde_json::from_slice(&data) {
                Ok(state) => state,
                Err(e) => {
                    eprintln!("decode PostgreSQL service relay state {name}: {e}");
                    continue;
                }
            };
            if !state.persistent
                || state.session_id.trim().is_empty()
                || !is_loopback_ip(&state.listen_host)
                || !(1..=65535).contains(&state.listen_port)
                || state.target_host.trim().is_empty()
                || !(1..=65535).contains(&state.target_port)
            {
                eprintln!("ignore invalid PostgreSQL service relay state {name}");
                continue;
            }
            let token_hash: [u8; 32] = match hex::decode(&state.token_hash)
                .ok()
                .and_then(|bytes| bytes.try_into().ok())
            {
                Some(hash) => hash,
                None => {
                    eprintln!(
                        "ignore PostgreSQL service relay state with invalid token hash {name}"
                    );
                    continue;
                }
            };
            let listener = match crate::tcprelay::listen(&join_host_port(
                &state.listen_host,
                state.listen_port,
            )) {
                Ok((listener, _, _)) => listener,
                Err(e) => {
                    // The desired state stays on disk: the next explicit
                    // reconciliation can replace it.
                    eprintln!(
                        "restore PostgreSQL service relay {}: listen failed: {e}",
                        state.session_id
                    );
                    continue;
                }
            };
            let session = Arc::new(RelaySession {
                id: state.session_id.clone(),
                listen_host: state.listen_host.clone(),
                listen_port: state.listen_port,
                token_hash,
                listener,
                target_host: state.target_host.clone(),
                target_port: state.target_port,
                persistent: true,
                expires_at: None,
                connections: Mutex::default(),
            });
            self.sessions
                .lock()
                .unwrap()
                .insert(session.id.clone(), session.clone());
            spawn_accept(session);
        }
    }

    /// `start`.
    fn start(
        self: &Arc<Self>,
        args: &RelayArgs,
        target_host: &str,
        target_port: i64,
    ) -> Result<Map<String, J>, String> {
        let session_id = args.session_id.trim().to_string();
        if session_id.is_empty() {
            return Err("localRelay.sessionId is required".into());
        }
        let token = args.relay_token.trim();
        if token.len() < 32 {
            return Err("localRelay.relayToken must contain at least 32 characters".into());
        }
        let listen_host = match args.listen_host.trim() {
            "" => "127.0.0.1".to_string(),
            host => host.to_string(),
        };
        if listen_host.parse::<IpAddr>().is_err() || !is_loopback_ip(&listen_host) {
            return Err("localRelay.listenHost must be a loopback IP".into());
        }
        if !(0..=65535).contains(&args.listen_port) {
            return Err("localRelay.listenPort must be between 0 and 65535".into());
        }
        let target_host = target_host.trim().to_string();
        if target_host.is_empty() {
            return Err("localRelay.targetHost is required".into());
        }
        if !(1..=65535).contains(&target_port) {
            return Err("localRelay.targetPort must be between 1 and 65535".into());
        }
        let mut ttl = Duration::ZERO;
        if !args.persistent {
            let seconds = if args.ttl_seconds == 0 {
                300
            } else {
                args.ttl_seconds
            };
            if !(10..=3600).contains(&seconds) {
                return Err("localRelay.ttlSeconds must be between 10 and 3600".into());
            }
            ttl = Duration::from_secs(seconds as u64);
        }
        let hash = sha256(token.as_bytes());
        let mut sessions = self.sessions.lock().unwrap();
        if let Some(existing) = sessions.get(&session_id).cloned() {
            if existing.listen_host == listen_host
                && (args.listen_port == 0 || existing.listen_port == args.listen_port)
                && existing.target_host == target_host
                && existing.target_port == target_port
                && existing.persistent == args.persistent
                && same_digest(&existing.token_hash, &hash)
            {
                return Ok(existing.descriptor());
            }
            if !args.replace_existing
                || !existing.persistent
                || !args.persistent
                || existing.active_connection_count() > 0
            {
                return Err(format!(
                    "relay_session_conflict: sessionId {} is already owned by another relay",
                    quote(&session_id)
                ));
            }
            // An explicit persistent handoff, only once the incumbent has no
            // authenticated connection.
            sessions.remove(&session_id);
            existing.close();
            self.remove_persisted(&session_id);
        }
        let (listener, _, port) =
            crate::tcprelay::listen(&join_host_port(&listen_host, args.listen_port))
                .map_err(|e| format!("listen for PostgreSQL service relay: {e}"))?;
        let session = Arc::new(RelaySession {
            id: session_id.clone(),
            listen_host,
            listen_port: i64::from(port),
            token_hash: hash,
            listener,
            target_host,
            target_port,
            persistent: args.persistent,
            expires_at: (!args.persistent).then(|| SystemTime::now() + ttl),
            connections: Mutex::default(),
        });
        sessions.insert(session_id.clone(), session.clone());
        drop(sessions);
        spawn_accept(session.clone());
        if let Err(e) = self.persist(&session) {
            self.stop_if_current(&session_id, Some(&session));
            return Err(e);
        }
        if !args.persistent {
            let (manager, expiring) = (Arc::clone(self), session.clone());
            std::thread::spawn(move || {
                std::thread::sleep(ttl);
                manager.stop_if_current(&expiring.id.clone(), Some(&expiring));
            });
        }
        Ok(session.descriptor())
    }

    /// `stopOwned`: only the token that created a session may release it.
    fn stop_owned(&self, session_id: &str, token: &str) -> Result<bool, String> {
        let session_id = session_id.trim();
        let token = token.trim();
        if session_id.is_empty() {
            return Err("sessionId is required".into());
        }
        if token.len() < 32 || token.contains(['\r', '\n']) {
            return Err("relayToken is invalid".into());
        }
        let hash = sha256(token.as_bytes());
        let session = {
            let mut sessions = self.sessions.lock().unwrap();
            let Some(session) = sessions.get(session_id).cloned() else {
                return Ok(false);
            };
            if !same_digest(&session.token_hash, &hash) {
                return Err("relay_ownership_mismatch: relayToken does not own sessionId".into());
            }
            sessions.remove(session_id);
            session
        };
        session.close();
        self.remove_persisted(session_id);
        Ok(true)
    }

    /// `stopIfCurrent`.
    fn stop_if_current(&self, session_id: &str, expected: Option<&Arc<RelaySession>>) -> bool {
        let session = {
            let mut sessions = self.sessions.lock().unwrap();
            let current = sessions.get(session_id).cloned();
            match (current, expected) {
                (Some(current), Some(expected)) if Arc::ptr_eq(&current, expected) => {
                    sessions.remove(session_id);
                    current
                }
                (Some(current), None) => {
                    sessions.remove(session_id);
                    current
                }
                _ => {
                    drop(sessions);
                    if expected.is_none() {
                        self.remove_persisted(session_id);
                    }
                    return false;
                }
            }
        };
        session.close();
        self.remove_persisted(session_id);
        true
    }

    /// `stopAll`: a revocation, used when the backing stack goes away.
    pub fn stop_all(&self) {
        let ids: Vec<String> = self.sessions.lock().unwrap().keys().cloned().collect();
        for id in ids {
            self.stop_if_current(&id, None);
        }
    }
}

fn spawn_accept(session: Arc<RelaySession>) {
    std::thread::spawn(move || {
        for client in session.listener.incoming() {
            let Ok(client) = client else { return };
            let session = session.clone();
            std::thread::spawn(move || handle_connection(&session, client));
        }
    });
}

/// `handleConnection`: one token line, never forwarded, then a plain pipe.
fn handle_connection(session: &RelaySession, client: TcpStream) {
    if !session.add_connection(&client) || session.expired() {
        let _ = client.shutdown(Shutdown::Both);
        return;
    }
    relay_authenticated(session, &client);
    session.remove_connection(&client);
    let _ = client.shutdown(Shutdown::Both);
}

fn relay_authenticated(session: &RelaySession, client: &TcpStream) {
    let Ok(line) = read_token_line(client) else {
        return;
    };
    let presented = line.strip_suffix('\r').unwrap_or(&line);
    if !same_digest(&sha256(presented.as_bytes()), &session.token_hash) {
        return;
    }
    if !session.is_open() || session.expired() {
        return;
    }
    let Some(upstream) = dial(&session.target_host, session.target_port) else {
        return;
    };
    let (Ok(mut from_client), Ok(mut to_upstream)) = (client.try_clone(), upstream.try_clone())
    else {
        return;
    };
    let (Ok(mut from_upstream), Ok(mut to_client)) = (upstream.try_clone(), client.try_clone())
    else {
        return;
    };
    let (done, finished) = std::sync::mpsc::channel();
    let forward = done.clone();
    std::thread::spawn(move || {
        let _ = std::io::copy(&mut from_client, &mut to_upstream);
        let _ = forward.send(());
    });
    std::thread::spawn(move || {
        let _ = std::io::copy(&mut from_upstream, &mut to_client);
        let _ = done.send(());
    });
    // Either direction ending closes both, as Go's deferred closes do.
    let _ = finished.recv();
    let _ = upstream.shutdown(Shutdown::Both);
}

/// `net.DialTimeout("tcp", JoinHostPort(host, port), 10s)`.
fn dial(host: &str, port: i64) -> Option<TcpStream> {
    let port = u16::try_from(port).ok()?;
    let addresses: Vec<SocketAddr> = (host, port).to_socket_addrs().ok()?.collect();
    addresses
        .iter()
        .find_map(|a| TcpStream::connect_timeout(a, Duration::from_secs(10)).ok())
}

/// `readPostgreSQLServiceRelayTokenLine`: at most 512 bytes before the
/// newline. Read one byte at a time so nothing after it is consumed.
fn read_token_line(mut conn: &TcpStream) -> Result<String, ()> {
    let mut line = Vec::with_capacity(64);
    let mut byte = [0u8; 1];
    loop {
        match conn.read(&mut byte) {
            Ok(1) => {}
            _ => return Err(()),
        }
        if byte[0] == b'\n' {
            return Ok(String::from_utf8_lossy(&line).into_owned());
        }
        if line.len() >= 512 {
            return Err(());
        }
        line.push(byte[0]);
    }
}

// --- The platform PostgreSQL service (platform_postgres.go) --------------------------------

#[derive(Clone, Debug, Default)]
pub struct ServiceArgs {
    pub vm_name: String,
    pub cluster_name: String,
    pub namespace: String,
    pub instances: i64,
    pub storage_class: String,
    pub storage_size: String,
    pub retention_policy: String,
    pub restart_consumers: Option<bool>,
    pub relay: Option<RelayArgs>,
    pub databases: Vec<String>,
    pub consumer_secret_name: String,
    pub consumer_secret_label: String,
    pub service_owner: String,
    pub service_part_of: String,
    pub consumer_database_keys: BTreeMap<String, String>,
    pub relay_device_name: String,
}

#[derive(Clone, Debug, Default)]
struct Spec {
    vm_name: String,
    cluster_name: String,
    namespace: String,
    instances: i64,
    storage_class: String,
    storage_size: String,
    retention_policy: String,
    databases: Vec<String>,
    consumer_secret_name: String,
    consumer_secret_label: String,
    service_owner: String,
    service_part_of: String,
    consumer_database_keys: BTreeMap<String, String>,
    relay_device_name: String,
}

#[derive(Clone, Debug, Default)]
struct Credentials {
    username: String,
    password: String,
}

#[derive(Clone, Debug, Default)]
struct Probe {
    operator_ready: bool,
    crd_present: bool,
    cluster_ready: bool,
    primary_ready: bool,
    service_ready: bool,
    secret_ready: bool,
    sql_ready: bool,
    task_ledger_sql_ready: bool,
    blockers: Vec<String>,
    primary_pod: String,
    credentials: Credentials,
}

impl Probe {
    /// `postgresqlServiceProbeReady`.
    fn ready(&self) -> bool {
        self.infrastructure_ready() && self.task_ledger_sql_ready
    }

    /// `postgresqlServiceInfrastructureReady`: the system database answers;
    /// a configured database may still be missing.
    fn infrastructure_ready(&self) -> bool {
        self.operator_ready
            && self.crd_present
            && self.cluster_ready
            && self.service_ready
            && self.secret_ready
            && self.primary_ready
            && self.sql_ready
    }
}

/// `postgresDatabaseIdentifier`: `^[a-z][a-z0-9_-]{0,62}$`.
fn database_identifier(value: &str) -> bool {
    identifier(value, true)
}

/// `standaloneIdentifier`: `^[a-z][a-z0-9-]{0,62}$`.
fn standalone_identifier(value: &str) -> bool {
    identifier(value, false)
}

fn identifier(value: &str, underscore: bool) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= 63
        && bytes[0].is_ascii_lowercase()
        && bytes[1..].iter().all(|&c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-' || (underscore && c == b'_')
        })
}

/// `isSafeKubernetesName`.
fn safe_kubernetes_name(value: &str) -> bool {
    if value.is_empty() || value.len() > 253 {
        return false;
    }
    value
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.')
        && !value.starts_with('-')
        && !value.ends_with('-')
}

/// `isValidStorageSize`: digits then Mi, Gi or Ti.
fn valid_storage_size(value: &str) -> bool {
    let value = value.trim();
    if value.len() < 3 {
        return false;
    }
    let (digits, suffix) = value.split_at(value.len() - 2);
    matches!(suffix, "Mi" | "Gi" | "Ti") && digits.bytes().all(|c| c.is_ascii_digit())
}

/// `validatePostgreSQLServiceSpec`.
fn validate(args: &ServiceArgs) -> Result<Spec, String> {
    let mut spec = Spec {
        vm_name: args.vm_name.trim().into(),
        cluster_name: args.cluster_name.trim().into(),
        namespace: args.namespace.trim().into(),
        instances: args.instances,
        storage_class: args.storage_class.trim().into(),
        storage_size: args.storage_size.trim().into(),
        retention_policy: args.retention_policy.trim().into(),
        relay_device_name: args.relay_device_name.trim().into(),
        ..Spec::default()
    };
    if spec.vm_name.is_empty() {
        return Err("vmName is required".into());
    }
    if spec.cluster_name.is_empty() {
        return Err("clusterName is required".into());
    }
    if spec.namespace.is_empty() {
        return Err("namespace is required".into());
    }
    if spec.instances == 0 {
        spec.instances = 1;
    }
    if !(1..=5).contains(&spec.instances) {
        return Err("instances must be between 1 and 5".into());
    }
    if spec.storage_class.is_empty() {
        spec.storage_class = "local-path".into();
    }
    if spec.storage_size.is_empty() {
        spec.storage_size = SERVICE_STORAGE_SIZE.into();
    }
    if spec.retention_policy.is_empty() {
        spec.retention_policy = "delete".into();
    }
    if spec.retention_policy != "delete" && spec.retention_policy != "retain" {
        return Err("retentionPolicy must be delete or retain".into());
    }
    if args.databases.is_empty() {
        return Err("databases must contain at least one database".into());
    }
    for raw in &args.databases {
        let database = raw.trim().to_string();
        if !database_identifier(&database) || spec.databases.contains(&database) {
            return Err("databases must be unique lowercase SQL identifiers".into());
        }
        spec.databases.push(database);
    }
    spec.consumer_secret_name = args.consumer_secret_name.trim().into();
    if spec.consumer_secret_name.is_empty() {
        return Err("consumerSecretName is required".into());
    }
    spec.consumer_secret_label = args.consumer_secret_label.trim().into();
    if spec.consumer_secret_label.is_empty() {
        return Err("consumerSecretLabel is required".into());
    }
    spec.service_owner = args.service_owner.trim().into();
    if spec.service_owner.is_empty() {
        return Err("serviceOwner is required".into());
    }
    spec.service_part_of = args.service_part_of.trim().into();
    if spec.service_part_of.is_empty() {
        return Err("servicePartOf is required".into());
    }
    if spec.relay_device_name.is_empty() && args.relay.is_some() {
        return Err("relayDeviceName is required when localRelay is requested".into());
    }
    for database in &spec.databases {
        let key = args
            .consumer_database_keys
            .get(database)
            .map(|k| k.trim().to_string())
            .unwrap_or_default();
        if !key.is_empty() {
            spec.consumer_database_keys.insert(database.clone(), key);
        }
    }
    if spec.consumer_database_keys.is_empty() {
        return Err("consumerDatabaseKeys must map every database to a Secret key".into());
    }
    for database in &spec.databases {
        if !spec.consumer_database_keys.contains_key(database) {
            return Err(format!(
                "consumerDatabaseKeys missing key for database {}",
                quote(database)
            ));
        }
    }
    // Go walks these two groups as maps, so which invalid field it names
    // first is unordered; each group is checked in full before the next.
    for (field, value) in [
        ("consumerSecretName", &spec.consumer_secret_name),
        ("serviceOwner", &spec.service_owner),
        ("servicePartOf", &spec.service_part_of),
    ] {
        if !safe_kubernetes_name(value) {
            return Err(format!("{field} is not a valid Kubernetes name/value"));
        }
    }
    for (field, value) in [
        ("clusterName", &spec.cluster_name),
        ("namespace", &spec.namespace),
        ("storageClass", &spec.storage_class),
    ] {
        if !safe_kubernetes_name(value) {
            return Err(format!("{field} is not a valid Kubernetes name/value"));
        }
    }
    if !valid_storage_size(&spec.storage_size) {
        return Err("storageSize must be a Kubernetes quantity such as 10Gi".into());
    }
    Ok(spec)
}

/// `renderPostgreSQLServiceOperatorManifest`.
fn operator_manifest() -> &'static str {
    r#"apiVersion: helm.cattle.io/v1
kind: HelmChart
metadata:
  name: cloudnativepg
  namespace: kube-system
spec:
  chart: cloudnative-pg
  repo: https://cloudnative-pg.github.io/charts
  targetNamespace: cnpg-system
  createNamespace: true
  valuesContent: |
    monitoring:
      enabled: false
    config:
      maxConcurrentReconciles: 1
    webhook:
      livenessProbe:
        initialDelaySeconds: 30
      readinessProbe:
        initialDelaySeconds: 30
      startupProbe:
        failureThreshold: 24
        periodSeconds: 5
    resources:
      requests:
        cpu: 25m
        memory: 32Mi
      limits:
        cpu: 500m
        memory: 128Mi
"#
}

/// `renderPostgreSQLServiceClusterManifest`.
fn cluster_manifest(spec: &Spec) -> String {
    let primary = &spec.databases[0];
    // No peer to protect in a single-instance cell: only the peer/API
    // isolation check is disabled.
    let isolation = if spec.instances == 1 {
        "      isolationCheck:\n        enabled: false\n"
    } else {
        ""
    };
    format!(
        r#"apiVersion: postgresql.cnpg.io/v1
kind: Cluster
metadata:
  name: {name}
  namespace: {namespace}
  annotations:
    host-agent.io/retention-policy: {retention}
    host-agent.io/resource-profile: {profile}
  labels:
    app.kubernetes.io/part-of: {part_of}
    app.kubernetes.io/managed-by: host-agent
spec:
  instances: {instances}
  imageName: {image}
  postgresql:
    parameters:
      max_connections: "40"
      max_worker_processes: "8"
      max_parallel_workers: "4"
      max_parallel_workers_per_gather: "2"
      max_replication_slots: "8"
      max_wal_senders: "5"
      shared_buffers: "32MB"
      work_mem: "2MB"
      maintenance_work_mem: "32MB"
      autovacuum_max_workers: "2"
      wal_keep_size: "128MB"
  startDelay: 180
  livenessProbeTimeout: 60
  resources:
    requests:
      cpu: 100m
      memory: 128Mi
    limits:
      cpu: 500m
      memory: 768Mi
  probes:
    startup:
      periodSeconds: 10
      timeoutSeconds: 10
      failureThreshold: 18
    liveness:
      periodSeconds: 10
      timeoutSeconds: 10
      failureThreshold: 6
{isolation}    readiness:
      periodSeconds: 10
      timeoutSeconds: 10
      failureThreshold: 6
  managed:
    roles:
      - name: {owner}
        ensure: present
        login: true
        superuser: false
        createdb: true
  storage:
    size: {size}
    storageClass: {class}
  bootstrap:
    initdb:
      database: {primary}
      owner: {owner}
      encoding: UTF8
      localeCType: C
      localeCollate: C
"#,
        name = spec.cluster_name,
        namespace = spec.namespace,
        retention = spec.retention_policy,
        profile = RESOURCE_PROFILE,
        part_of = spec.service_part_of,
        instances = spec.instances,
        image = OPERAND_IMAGE,
        owner = spec.service_owner,
        size = spec.storage_size,
        class = spec.storage_class,
    )
}

/// `nestedString`.
fn nested_str(value: &J, keys: &[&str]) -> String {
    let mut current = value;
    for key in keys {
        match current.get(*key) {
            Some(next) => current = next,
            None => return String::new(),
        }
    }
    current.as_str().unwrap_or("").to_string()
}

/// `nestedInt`: a JSON number truncated; anything else is 0.
fn nested_int(value: &J, keys: &[&str]) -> i64 {
    let mut current = value;
    for key in keys {
        match current.get(*key) {
            Some(next) => current = next,
            None => return 0,
        }
    }
    current.as_f64().map_or(0, |v| v as i64)
}

/// `decodeSecretValue`: base64 standard, or empty.
fn secret_value(data: &J, key: &str) -> String {
    data.get(key)
        .and_then(J::as_str)
        .and_then(|encoded| {
            base64::engine::general_purpose::STANDARD
                .decode(encoded)
                .ok()
        })
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .unwrap_or_default()
}

/// `subsets[*].addresses` holds at least one address.
fn has_address(endpoints: &J) -> bool {
    endpoints
        .get("subsets")
        .and_then(J::as_array)
        .is_some_and(|subsets| {
            subsets.iter().any(|subset| {
                subset
                    .get("addresses")
                    .and_then(J::as_array)
                    .is_some_and(|a| !a.is_empty())
            })
        })
}

/// `postgresqlServiceSQLScript`.
fn service_sql_script(host: &str, username: &str, database: &str, sql: &str) -> String {
    format!(
        r#"set -eu
pgpass_dir="${{TMPDIR:-/controller/tmp}}"
if [ ! -d "$pgpass_dir" ] || [ ! -w "$pgpass_dir" ]; then
  pgpass_dir="/tmp"
fi
pgpass="$(mktemp "$pgpass_dir/opute-pgpass.XXXXXX")"
trap 'rm -f "$pgpass"' EXIT
cat >"$pgpass"
chmod 600 "$pgpass"
PGPASSFILE="$pgpass" psql -h {} -p {} -U {} -d {} -v ON_ERROR_STOP=1 -Atqc {}
"#,
        shell_quote(host),
        SERVICE_PORT,
        shell_quote(username),
        shell_quote(database),
        shell_quote(sql)
    )
}

/// `kubectlShellScriptArgument`: the provider takes single-line arguments,
/// so the script travels base64-encoded and is evaluated in the exec's own
/// shell, which keeps its stdin for the pgpass file.
fn shell_script_argument(script: &str) -> String {
    let encoded = base64::engine::general_purpose::STANDARD.encode(script);
    format!("eval \"$(printf '%s' '{encoded}' | base64 -d)\"")
}

/// `pgpass` content: `*:*:*:user:password`.
fn pgpass(credentials: &Credentials) -> Vec<u8> {
    format!("*:*:*:{}:{}\n", credentials.username, credentials.password).into_bytes()
}

/// `postgresqlServiceCreateDatabaseSQL`.
fn create_database_sql(database: &str) -> String {
    format!("CREATE DATABASE \"{}\"", database.replace('"', "\"\""))
}

/// Go's `url.UserPassword` escaping (`encodeUserPassword`): unreserved
/// characters and the sub-delimiters `$&+,;=` stay; everything else is
/// percent-encoded in upper case.
fn escape_userinfo(value: &str) -> String {
    let mut out = String::new();
    for &b in value.as_bytes() {
        let keep = b.is_ascii_alphanumeric()
            || matches!(
                b,
                b'-' | b'_' | b'.' | b'~' | b'$' | b'&' | b'+' | b',' | b';' | b'='
            );
        if keep {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// `postgresqlServiceDatabaseURL`.
fn database_url(spec: &Spec, credentials: &Credentials, database: &str) -> String {
    format!(
        "postgresql://{}:{}@{}-rw.{}.svc:{}/{}",
        escape_userinfo(&credentials.username),
        escape_userinfo(&credentials.password),
        spec.cluster_name,
        spec.namespace,
        SERVICE_PORT,
        database
    )
}

/// `renderPostgreSQLServiceConsumerSecret`.
fn consumer_secret_manifest(spec: &Spec, credentials: &Credentials) -> String {
    let mut data = String::new();
    for database in &spec.databases {
        data.push_str(&format!(
            "  {}: {}\n",
            spec.consumer_database_keys
                .get(database)
                .map(String::as_str)
                .unwrap_or(""),
            base64::engine::general_purpose::STANDARD.encode(database_url(
                spec,
                credentials,
                database
            ))
        ));
    }
    format!(
        "apiVersion: v1\nkind: Secret\nmetadata:\n  name: {}\n  namespace: {}\n  labels:\n    {}: \"true\"\ntype: Opaque\ndata:\n{}",
        spec.consumer_secret_name, spec.namespace, spec.consumer_secret_label, data
    )
}

/// A polling deadline with Go's two-second ticker.
struct Ticker {
    deadline: Instant,
    next: Instant,
}

impl Ticker {
    fn new(timeout: Duration) -> Self {
        let now = Instant::now();
        Ticker {
            deadline: now + timeout,
            next: now + TICK,
        }
    }

    /// The `select` of a wait loop: the call's cancellation, the deadline
    /// (`Err(None)`), or the next tick.
    fn wait(&mut self, ctx: &RunCtx) -> Result<(), Option<String>> {
        if let Some(e) = ctx.err() {
            return Err(Some(e));
        }
        let now = Instant::now();
        if now >= self.deadline {
            return Err(None);
        }
        // A tick missed while probing fires at once; later ticks keep the
        // original cadence.
        while self.next + TICK <= now {
            self.next += TICK;
        }
        let until = self.next.min(self.deadline);
        ctx.sleep(until.saturating_duration_since(now))
            .map_err(Some)?;
        if Instant::now() >= self.deadline && Instant::now() < self.next {
            return Err(None);
        }
        self.next += TICK;
        Ok(())
    }
}

impl Postgres {
    /// `postgresqlServiceJSON`: a kubectl read with `-o json`.
    fn json(&self, spec: &Spec, args: &[&str], label: &str) -> Result<J, String> {
        let mut argv = argv(args);
        argv.extend(["-o".to_string(), "json".to_string()]);
        let raw = self.kubectl(&spec.vm_name, &argv, label)?;
        unmarshal_map(&raw).map_err(|e| format!("decode {label} response: {e}"))
    }

    fn apply(&self, spec: &Spec, manifest: &str, label: &str) -> Result<(), String> {
        self.kubectl_input(
            &spec.vm_name,
            &argv(&["apply", "-f", "-"]),
            manifest.as_bytes(),
            label,
        )
        .map(|_| ())
    }

    /// `replacePostgreSQLServiceConsumerSecret`.
    fn replace_consumer_secret(&self, spec: &Spec, manifest: &str) -> Result<(), String> {
        self.kubectl(
            &spec.vm_name,
            &argv(&[
                "delete",
                "secret",
                &spec.consumer_secret_name,
                "-n",
                &spec.namespace,
                "--ignore-not-found=true",
            ]),
            "replace PostgreSQL service consumer Secret",
        )?;
        self.apply(spec, manifest, "apply PostgreSQL service consumer Secret")
    }

    /// `ensurePostgreSQLServiceNamespace`.
    fn ensure_namespaces(&self, spec: &Spec) -> Result<(), String> {
        for namespace in [spec.namespace.as_str(), OPERATOR_NAMESPACE] {
            if self
                .kubectl(
                    &spec.vm_name,
                    &argv(&["get", "namespace", namespace]),
                    "get namespace",
                )
                .is_ok()
            {
                continue;
            }
            self.kubectl(
                &spec.vm_name,
                &argv(&["create", "namespace", namespace]),
                "create namespace",
            )?;
        }
        Ok(())
    }

    /// `postgresqlServiceCRDPresent`.
    fn crd_present(&self, spec: &Spec) -> Result<bool, String> {
        let result = self
            .kubectl(
                &spec.vm_name,
                &argv(&[
                    "get",
                    "crd",
                    "clusters.postgresql.cnpg.io",
                    "--ignore-not-found=true",
                    "-o",
                    "name",
                ]),
                "get CloudNativePG CRD",
            )
            .map_err(|e| format!("get CloudNativePG Cluster CRD: {e}"))?;
        Ok(!result.trim().is_empty())
    }

    /// `postgresqlServiceOperatorReady`: (ready, crd present).
    fn operator_ready(&self, spec: &Spec) -> Result<(bool, bool), String> {
        if !self.crd_present(spec)? {
            return Ok((false, false));
        }
        let Ok(deployments) = self.json(
            spec,
            &[
                "get",
                "deployments",
                "-n",
                OPERATOR_NAMESPACE,
                "-l",
                "app.kubernetes.io/name=cloudnative-pg",
            ],
            "get CloudNativePG operator",
        ) else {
            return Ok((false, true));
        };
        let ready = deployments
            .get("items")
            .and_then(J::as_array)
            .is_some_and(|items| {
                items.iter().any(|d| {
                    let desired = nested_int(d, &["spec", "replicas"]);
                    desired > 0 && nested_int(d, &["status", "availableReplicas"]) >= desired
                })
            });
        Ok((ready, true))
    }

    fn get_cluster(&self, spec: &Spec, label: &str) -> Result<J, String> {
        self.json(
            spec,
            &[
                "get",
                "cluster.postgresql.cnpg.io",
                &spec.cluster_name,
                "-n",
                &spec.namespace,
            ],
            label,
        )
    }

    /// `postgresqlServiceClusterReady`.
    fn cluster_ready(&self, spec: &Spec) -> bool {
        let Ok(cluster) = self.get_cluster(spec, "get PostgreSQL service Cluster") else {
            return false;
        };
        nested_str(&cluster, &["status", "phase"])
            .to_lowercase()
            .contains("healthy")
            && nested_int(&cluster, &["spec", "instances"]) == spec.instances
            && nested_int(&cluster, &["status", "readyInstances"]) >= spec.instances
    }

    /// `postgresqlServiceClusterConfigurationReady`.
    fn cluster_configured(&self, spec: &Spec) -> bool {
        let Ok(cluster) = self.get_cluster(spec, "get PostgreSQL service Cluster configuration")
        else {
            return false;
        };
        cluster
            .pointer("/metadata/annotations/host-agent.io~1resource-profile")
            .and_then(J::as_str)
            == Some(RESOURCE_PROFILE)
            && nested_str(&cluster, &["spec", "storage", "size"]) == spec.storage_size
    }

    /// `postgresqlServiceOperatorConfigurationReady`.
    fn operator_configured(&self, spec: &Spec) -> bool {
        let Ok(chart) = self.json(
            spec,
            &[
                "get",
                "helmchart.helm.cattle.io",
                OPERATOR_RELEASE,
                "-n",
                "kube-system",
            ],
            "get CloudNativePG resource profile",
        ) else {
            return false;
        };
        let values = nested_str(&chart, &["spec", "valuesContent"]);
        [
            "maxConcurrentReconciles: 1",
            "initialDelaySeconds: 30",
            "failureThreshold: 24",
            "periodSeconds: 5",
            "memory: 128Mi",
            "cpu: 500m",
        ]
        .iter()
        .all(|needle| values.contains(needle))
    }

    /// `postgresqlServiceReady`: the read/write Service has an address.
    fn service_ready(&self, spec: &Spec) -> bool {
        let rw = format!("{}-rw", spec.cluster_name);
        let Ok(service) = self.json(
            spec,
            &["get", "service", &rw, "-n", &spec.namespace],
            "get PostgreSQL service read/write service",
        ) else {
            return false;
        };
        if nested_str(&service, &["spec", "clusterIP"]).is_empty() {
            return false;
        }
        self.json(
            spec,
            &["get", "endpoints", &rw, "-n", &spec.namespace],
            "get PostgreSQL service endpoints",
        )
        .is_ok_and(|endpoints| has_address(&endpoints))
    }

    /// `postgresqlServiceSecret`.
    fn app_secret(&self, spec: &Spec) -> Option<Credentials> {
        let secret = self
            .json(
                spec,
                &[
                    "get",
                    "secret",
                    &format!("{}-app", spec.cluster_name),
                    "-n",
                    &spec.namespace,
                ],
                "get PostgreSQL service application secret",
            )
            .ok()?;
        let data = secret.get("data").cloned().unwrap_or(J::Null);
        let credentials = Credentials {
            username: secret_value(&data, "username"),
            password: secret_value(&data, "password"),
        };
        (!credentials.username.is_empty() && !credentials.password.is_empty())
            .then_some(credentials)
    }

    /// `postgresqlServiceConsumerSecretReady`.
    fn consumer_secret_ready(&self, spec: &Spec) -> bool {
        let Ok(secret) = self.json(
            spec,
            &[
                "get",
                "secret",
                &spec.consumer_secret_name,
                "-n",
                &spec.namespace,
            ],
            "get PostgreSQL consumer Secret",
        ) else {
            return false;
        };
        let data = secret.get("data").cloned().unwrap_or(J::Null);
        spec.consumer_database_keys.values().all(|key| {
            data.get(key)
                .and_then(J::as_str)
                .is_some_and(|v| !v.trim().is_empty())
        }) && !spec.consumer_database_keys.is_empty()
    }

    /// `postgresqlServicePrimary`: a running primary whose Ready condition
    /// and every container are ready.
    fn primary(&self, spec: &Spec) -> String {
        let Ok(pods) = self.json(
            spec,
            &[
                "get",
                "pods",
                "-n",
                &spec.namespace,
                "-l",
                &format!("cnpg.io/cluster={},role=primary", spec.cluster_name),
            ],
            "get PostgreSQL service primary",
        ) else {
            return String::new();
        };
        for pod in pods
            .get("items")
            .and_then(J::as_array)
            .into_iter()
            .flatten()
        {
            let name = nested_str(pod, &["metadata", "name"]);
            if name.is_empty() || nested_str(pod, &["status", "phase"]) != "Running" {
                continue;
            }
            let status = pod.get("status").cloned().unwrap_or(J::Null);
            let ready_condition =
                status
                    .get("conditions")
                    .and_then(J::as_array)
                    .is_some_and(|conditions| {
                        conditions.iter().any(|c| {
                            nested_str(c, &["type"]) == "Ready"
                                && nested_str(c, &["status"]) == "True"
                        })
                    });
            if !ready_condition {
                continue;
            }
            let statuses = status
                .get("containerStatuses")
                .and_then(J::as_array)
                .cloned()
                .unwrap_or_default();
            if statuses.is_empty() {
                continue;
            }
            if statuses
                .iter()
                .all(|s| s.get("ready").and_then(J::as_bool) == Some(true))
            {
                return name;
            }
        }
        String::new()
    }

    fn service_exec(
        &self,
        spec: &Spec,
        credentials: &Credentials,
        pod: &str,
        script: &str,
        label: &str,
    ) -> Result<String, String> {
        self.kubectl_input(
            &spec.vm_name,
            &argv(&[
                "exec",
                "-i",
                pod,
                "-n",
                &spec.namespace,
                "--",
                "sh",
                "-ceu",
                &shell_script_argument(script),
            ]),
            &pgpass(credentials),
            label,
        )
    }

    /// `runPostgreSQLServiceSQL`. As in Go, the query always asks for the
    /// database's own row; the `sql` given only names the probe.
    fn run_service_sql(
        &self,
        spec: &Spec,
        credentials: &Credentials,
        pod: &str,
        database: &str,
    ) -> Result<String, String> {
        let host = format!("{}-rw.{}.svc", spec.cluster_name, spec.namespace);
        let script = service_sql_script(
            &host,
            &credentials.username,
            database,
            &format!(
                "SELECT 1 FROM pg_database WHERE datname = {}",
                shell_quote(database)
            ),
        );
        self.service_exec(
            spec,
            credentials,
            pod,
            &script,
            "query PostgreSQL service through read/write service",
        )
    }

    /// `ensurePostgreSQLServiceDatabase`.
    fn ensure_database(
        &self,
        spec: &Spec,
        credentials: &Credentials,
        pod: &str,
        database: &str,
    ) -> Result<(), String> {
        let host = format!("{}-rw.{}.svc", spec.cluster_name, spec.namespace);
        let check = format!(
            "SELECT 1 FROM pg_database WHERE datname = {}",
            shell_quote(database)
        );
        let script = service_sql_script(&host, &credentials.username, "postgres", &check);
        let result = self.service_exec(
            spec,
            credentials,
            pod,
            &script,
            "check PostgreSQL service database",
        )?;
        if !result.trim().is_empty() {
            return Ok(());
        }
        let script = service_sql_script(
            &host,
            &credentials.username,
            "postgres",
            &create_database_sql(database),
        );
        self.service_exec(
            spec,
            credentials,
            pod,
            &script,
            "create PostgreSQL service database",
        )
        .map(|_| ())
    }

    /// `restartPostgreSQLServiceConsumers`: every Deployment labelled for
    /// the consumer Secret is restarted and waited for.
    fn restart_consumers(&self, spec: &Spec) -> Result<(), String> {
        let Ok(deployments) = self.json(
            spec,
            &[
                "get",
                "deployments",
                "-n",
                &spec.namespace,
                "-l",
                &spec.consumer_secret_label,
            ],
            "find PostgreSQL consumers",
        ) else {
            // A CNPG-only VM may have no consumer yet.
            return Ok(());
        };
        for deployment in deployments
            .get("items")
            .and_then(J::as_array)
            .into_iter()
            .flatten()
        {
            let name = nested_str(deployment, &["metadata", "name"]);
            if name.is_empty() {
                continue;
            }
            self.kubectl(
                &spec.vm_name,
                &argv(&[
                    "rollout",
                    "restart",
                    "deployment",
                    &name,
                    "-n",
                    &spec.namespace,
                ]),
                "restart PostgreSQL service consumer",
            )?;
            self.kubectl(
                &spec.vm_name,
                &argv(&[
                    "rollout",
                    "status",
                    "deployment",
                    &name,
                    "-n",
                    &spec.namespace,
                    "--timeout=2m",
                ]),
                "wait for PostgreSQL service consumer",
            )?;
        }
        Ok(())
    }

    /// `probePostgreSQLService`.
    fn probe(&self, spec: &Spec) -> Probe {
        let (operator_ready, crd_present) = self.operator_ready(spec).unwrap_or((false, false));
        let cluster_ready = self.cluster_ready(spec);
        let service_ready = self.service_ready(spec);
        let secret = self.app_secret(spec);
        let secret_ready = secret.is_some();
        let primary = if service_ready && secret_ready {
            self.primary(spec)
        } else {
            String::new()
        };
        let mut probe = Probe {
            operator_ready,
            crd_present,
            cluster_ready,
            primary_ready: !primary.is_empty(),
            service_ready,
            secret_ready,
            primary_pod: primary.clone(),
            credentials: secret.clone().unwrap_or_default(),
            ..Probe::default()
        };
        for (ready, blocker) in [
            (operator_ready, "CloudNativePG operator is not ready"),
            (crd_present, "CloudNativePG Cluster CRD is not present"),
            (
                cluster_ready,
                "PostgreSQL service Cluster is not healthy with the expected instance count",
            ),
            (
                service_ready,
                "PostgreSQL service read/write Service has no ready endpoint",
            ),
            (
                secret_ready,
                "PostgreSQL service application Secret is missing required keys",
            ),
            (
                probe.primary_ready,
                "PostgreSQL service primary pod is not ready",
            ),
        ] {
            if !ready {
                probe.blockers.push(blocker.into());
            }
        }
        if probe.primary_ready && probe.secret_ready {
            let credentials = probe.credentials.clone();
            if let Err(e) = self.run_service_sql(spec, &credentials, &primary, "postgres") {
                probe.blockers.push(format!(
                    "SQL SELECT 1 failed through the read/write Service for postgres: {e}"
                ));
                return probe;
            }
            probe.sql_ready = true;
            probe.task_ledger_sql_ready = true;
            for database in &spec.databases {
                if let Err(e) = self.run_service_sql(spec, &credentials, &primary, database) {
                    probe.task_ledger_sql_ready = false;
                    probe.blockers.push(format!(
                        "SQL SELECT 1 failed through the read/write Service for {database}: {e}"
                    ));
                }
            }
        }
        probe
    }

    /// `waitForPostgreSQLService`.
    fn wait_for_service(&self, ctx: &RunCtx, spec: &Spec) -> Result<Probe, String> {
        let mut ticker = Ticker::new(SERVICE_READINESS_TIMEOUT);
        loop {
            let probe = self.probe(spec);
            if probe.infrastructure_ready() {
                return Ok(probe);
            }
            match ticker.wait(ctx) {
                Ok(()) => {}
                Err(Some(e)) => return Err(e),
                Err(None) => {
                    return Err(format!(
                        "PostgreSQL service did not become SQL-ready: {}",
                        probe.blockers.join("; ")
                    ))
                }
            }
        }
    }

    /// `postgresqlServiceWebhookReady`.
    fn webhook_ready(&self, spec: &Spec) -> bool {
        self.json(
            spec,
            &[
                "get",
                "endpoints",
                "cnpg-webhook-service",
                "-n",
                OPERATOR_NAMESPACE,
            ],
            "get CloudNativePG webhook endpoints",
        )
        .is_ok_and(|endpoints| has_address(&endpoints))
    }

    /// `waitForPostgreSQLServiceCRD`: the CRD and the admission webhook.
    fn wait_for_crd(&self, ctx: &RunCtx, spec: &Spec) -> Result<(), String> {
        let mut ticker = Ticker::new(Duration::from_secs(5 * 60));
        loop {
            let present = self.crd_present(spec)?;
            if present && self.webhook_ready(spec) {
                return Ok(());
            }
            match ticker.wait(ctx) {
                Ok(()) => {}
                Err(Some(e)) => return Err(e),
                Err(None) => return Err("CloudNativePG operator did not become ready (CRD or admission webhook) after applying the HelmChart".into()),
            }
        }
    }

    /// `postgresqlServiceK3sReady`.
    fn k3s_ready(&self, spec: &Spec) -> Result<bool, String> {
        let Ok(raw) = self.kubectl(
            &spec.vm_name,
            &argv(&["get", "nodes", "-o", "json"]),
            "get K3s nodes",
        ) else {
            return Ok(false);
        };
        let nodes = unmarshal_struct(&raw).map_err(|e| format!("parse K3s nodes: {e}"))?;
        let items = nodes
            .get("items")
            .and_then(J::as_array)
            .cloned()
            .unwrap_or_default();
        Ok(items.iter().any(|node| {
            node.pointer("/status/conditions")
                .and_then(J::as_array)
                .is_some_and(|conditions| {
                    conditions.iter().any(|c| {
                        nested_str(c, &["type"]) == "Ready" && nested_str(c, &["status"]) == "True"
                    })
                })
        }))
    }

    /// `waitForPostgreSQLServiceK3sReady`.
    fn wait_for_k3s(&self, ctx: &RunCtx, spec: &Spec) -> Result<(), String> {
        let mut ticker = Ticker::new(Duration::from_secs(10 * 60));
        loop {
            if self.k3s_ready(spec)? {
                return Ok(());
            }
            match ticker.wait(ctx) {
                Ok(()) => {}
                Err(Some(e)) => return Err(e),
                Err(None) => {
                    return Err(
                        "K3s did not report a Ready node before PostgreSQL service reconciliation"
                            .into(),
                    )
                }
            }
        }
    }

    /// `ensurePostgreSQLServiceOrdered`: K3s readiness, the operator
    /// HelmChart only when the operator is not serving, the CRD and webhook,
    /// then the tenant Cluster.
    fn ensure_ordered(&self, ctx: &RunCtx, spec: &Spec) -> Result<(), String> {
        self.wait_for_k3s(ctx, spec)?;
        let (operator_ready, _) = self.operator_ready(spec)?;
        if !operator_ready {
            self.apply(spec, operator_manifest(), "apply CloudNativePG HelmChart")?;
        }
        self.wait_for_crd(ctx, spec)?;
        self.apply(
            spec,
            &cluster_manifest(spec),
            "apply PostgreSQL service Cluster",
        )
    }

    /// `probePostgreSQLServiceStable`: up to three probes, two seconds
    /// apart, before the repair path is chosen.
    fn probe_stable(&self, ctx: &RunCtx, spec: &Spec) -> Result<Probe, String> {
        let mut last = Probe::default();
        for attempt in 0..3 {
            let probe = self.probe(spec);
            if probe.ready() {
                return Ok(probe);
            }
            last = probe;
            if attempt < 2 {
                ctx.sleep(TICK)?;
            }
        }
        Ok(last)
    }

    /// `admitPostgreSQLStorage`: a persistent volume only grows, and only on
    /// a class that allows expansion.
    fn admit_storage(&self, spec: &Spec) -> Result<(), String> {
        let present = self.crd_present(spec).map_err(|e| {
            format!("storageSize cannot be admitted: could not determine whether CloudNativePG Cluster storage exists: {e}")
        })?;
        if !present {
            // A fresh cluster lacks the CNPG API until the operator is
            // installed.
            return Ok(());
        }
        let cluster = match self.get_cluster(spec, "get PostgreSQL service Cluster storage") {
            Ok(cluster) => cluster,
            Err(e) if is_not_found(&e) => return Ok(()),
            Err(e) => return Err(format!("storageSize cannot be admitted: {e}")),
        };
        let current = nested_str(&cluster, &["spec", "storage", "size"]);
        admit_pvc_size_change(&current, &spec.storage_size, || {
            let class = match nested_str(&cluster, &["spec", "storage", "storageClass"]).trim() {
                "" => spec.storage_class.clone(),
                class => class.to_string(),
            };
            self.assert_class_expands(spec, &class)
        })
    }

    /// `assertStorageClassExpands`.
    fn assert_class_expands(&self, spec: &Spec, class: &str) -> Result<(), String> {
        let class = class.trim();
        if class.is_empty() {
            return Err("storageSize cannot grow: StorageClass is unknown, so allowVolumeExpansion cannot be verified".into());
        }
        let value = self
            .json(
                spec,
                &["get", "storageclass", class],
                "get PostgreSQL StorageClass",
            )
            .map_err(|e| {
                format!(
                    "storageSize cannot grow: StorageClass {} could not be read: {e}",
                    quote(class)
                )
            })?;
        if value.get("allowVolumeExpansion").and_then(J::as_bool) == Some(true) {
            return Ok(());
        }
        Err(format!(
            "storageSize cannot grow: StorageClass {} does not allow volume expansion",
            quote(class)
        ))
    }

    /// `ReconcilePostgreSQLService`.
    pub fn reconcile_service(
        &self,
        ctx: &RunCtx,
        args: &ServiceArgs,
    ) -> Result<Map<String, J>, String> {
        let spec = validate(args)?;
        self.admit_storage(&spec)?;
        self.ensure_namespaces(&spec)?;
        // Probe first: a ready service is not reinstalled.
        let stable = self.probe_stable(ctx, &spec);
        let mut probe = match stable {
            Ok(probe) if probe.infrastructure_ready() => {
                // A ready legacy cluster may still run the pre-budget
                // manifest: the operator profile first, then the tenant's.
                let operator_configured = self.operator_configured(&spec);
                let configured = self.cluster_configured(&spec);
                let mut probe = probe;
                if !operator_configured {
                    self.apply(
                        &spec,
                        operator_manifest(),
                        "apply CloudNativePG resource profile",
                    )?;
                    self.wait_for_crd(ctx, &spec)?;
                }
                if !configured {
                    self.apply(
                        &spec,
                        &cluster_manifest(&spec),
                        "apply PostgreSQL service resource profile",
                    )?;
                    probe = self.wait_for_service(ctx, &spec)?;
                }
                probe
            }
            _ => {
                self.ensure_ordered(ctx, &spec)?;
                self.wait_for_service(ctx, &spec)?
            }
        };
        let credentials = probe.credentials.clone();
        for database in &spec.databases {
            self.ensure_database(&spec, &credentials, &probe.primary_pod, database)?;
        }
        probe = self.probe(&spec);
        if !probe.sql_ready || !probe.task_ledger_sql_ready {
            return Err("PostgreSQL service databases did not become SQL-ready".into());
        }
        let mut result = Map::new();
        result.insert("ready".into(), J::Bool(false));
        result.insert("vmName".into(), J::from(spec.vm_name.clone()));
        result.insert("namespace".into(), J::from(spec.namespace.clone()));
        result.insert("clusterName".into(), J::from(spec.cluster_name.clone()));
        result.insert("instances".into(), J::from(spec.instances));
        result.insert("storageClass".into(), J::from(spec.storage_class.clone()));
        result.insert("storageSize".into(), J::from(spec.storage_size.clone()));
        result.insert(
            "serviceName".into(),
            J::from(format!("{}-rw", spec.cluster_name)),
        );
        result.insert("servicePort".into(), J::from(SERVICE_PORT));
        result.insert(
            "secretName".into(),
            J::from(spec.consumer_secret_name.clone()),
        );
        result.insert(
            "cnpgSecretName".into(),
            J::from(format!("{}-app", spec.cluster_name)),
        );
        result.insert("databases".into(), json!(spec.databases));
        result.insert(
            "sqlReady".into(),
            J::Bool(probe.sql_ready && probe.task_ledger_sql_ready),
        );
        result.insert(
            "taskLedgerSqlReady".into(),
            J::Bool(probe.task_ledger_sql_ready),
        );
        result.insert("credentialState".into(), J::from("cnpg-owned"));
        result.insert("postgresVersion".into(), J::from("cloudnativepg"));
        // The consumer Secret is rendered with the credentials the wait saw.
        self.replace_consumer_secret(&spec, &consumer_secret_manifest(&spec, &credentials))?;
        if args.restart_consumers.unwrap_or(true) {
            self.restart_consumers(&spec)?;
        }
        let consumer_secret_ready = self.consumer_secret_ready(&spec);
        result.insert("consumerSecretReady".into(), J::Bool(consumer_secret_ready));
        result.insert(
            "ready".into(),
            J::Bool(probe.sql_ready && consumer_secret_ready),
        );
        if let Some(relay) = &args.relay {
            let relay = self.ensure_relay(&spec, relay)?;
            result.insert("localRelay".into(), J::Object(relay));
        }
        Ok(result)
    }

    /// `GetPostgreSQLServiceStatus`; with a relay requested and the service
    /// ready, it also ensures the relay.
    pub fn service_status(&self, args: &ServiceArgs) -> Result<Map<String, J>, String> {
        let spec = validate(args)?;
        let probe = self.probe(&spec);
        let consumer_secret_ready = self.consumer_secret_ready(&spec);
        let ready = probe.ready() && consumer_secret_ready;
        let mut result = Map::new();
        result.insert("ready".into(), J::Bool(ready));
        result.insert("operatorReady".into(), J::Bool(probe.operator_ready));
        result.insert("crdPresent".into(), J::Bool(probe.crd_present));
        result.insert("clusterReady".into(), J::Bool(probe.cluster_ready));
        result.insert("serviceReady".into(), J::Bool(probe.service_ready));
        result.insert("secretReady".into(), J::Bool(probe.secret_ready));
        result.insert("primaryReady".into(), J::Bool(probe.primary_ready));
        result.insert(
            "sqlReady".into(),
            J::Bool(probe.sql_ready && probe.task_ledger_sql_ready),
        );
        result.insert(
            "taskLedgerSqlReady".into(),
            J::Bool(probe.task_ledger_sql_ready),
        );
        result.insert("consumerSecretReady".into(), J::Bool(consumer_secret_ready));
        result.insert("blockers".into(), json!(probe.blockers));
        result.insert("vmName".into(), J::from(spec.vm_name.clone()));
        result.insert("namespace".into(), J::from(spec.namespace.clone()));
        result.insert("clusterName".into(), J::from(spec.cluster_name.clone()));
        result.insert(
            "serviceName".into(),
            J::from(format!("{}-rw", spec.cluster_name)),
        );
        result.insert(
            "secretName".into(),
            J::from(spec.consumer_secret_name.clone()),
        );
        result.insert(
            "cnpgSecretName".into(),
            J::from(format!("{}-app", spec.cluster_name)),
        );
        result.insert("credentialState".into(), J::from("cnpg-owned"));
        if let (Some(relay), true) = (&args.relay, ready) {
            let relay = self.ensure_relay(&spec, relay)?;
            result.insert("localRelay".into(), J::Object(relay));
        }
        Ok(result)
    }

    /// `RemovePostgreSQLService`: the Cluster, and with the delete retention
    /// policy its Secrets and volumes; every relay is revoked. The operator
    /// stays.
    pub fn remove_service(
        &self,
        args: &ServiceArgs,
        confirm: bool,
    ) -> Result<Map<String, J>, String> {
        if !confirm {
            return Err("remove_postgresql_service requires confirm=true".into());
        }
        let spec = validate(args)?;
        self.kubectl(
            &spec.vm_name,
            &argv(&[
                "delete",
                "cluster.postgresql.cnpg.io",
                &spec.cluster_name,
                "-n",
                &spec.namespace,
                "--ignore-not-found=true",
                "--wait=true",
            ]),
            "delete PostgreSQL service Cluster",
        )?;
        if spec.retention_policy == "delete" {
            let _ = self.kubectl(
                &spec.vm_name,
                &argv(&[
                    "delete",
                    "secret",
                    &format!("{}-app", spec.cluster_name),
                    "-n",
                    &spec.namespace,
                    "--ignore-not-found=true",
                ]),
                "delete PostgreSQL service Secret",
            );
            let _ = self.kubectl(
                &spec.vm_name,
                &argv(&[
                    "delete",
                    "secret",
                    &spec.consumer_secret_name,
                    "-n",
                    &spec.namespace,
                    "--ignore-not-found=true",
                ]),
                "delete PostgreSQL consumer Secret",
            );
            let _ = self.kubectl(
                &spec.vm_name,
                &argv(&[
                    "delete",
                    "pvc",
                    "-l",
                    &format!("cnpg.io/cluster={}", spec.cluster_name),
                    "-n",
                    &spec.namespace,
                    "--ignore-not-found=true",
                ]),
                "delete PostgreSQL service PVCs",
            );
        }
        self.live().relay.stop_all();
        let mut out = Map::new();
        out.insert("removed".into(), J::Bool(true));
        out.insert("vmName".into(), J::from(spec.vm_name));
        out.insert("namespace".into(), J::from(spec.namespace));
        out.insert("clusterName".into(), J::from(spec.cluster_name));
        out.insert("operatorPreserved".into(), J::Bool(true));
        Ok(out)
    }

    /// `ReleasePostgreSQLServiceRelay`: only the token that created the
    /// relay releases it.
    pub fn release_relay(&self, session_id: &str, token: &str) -> Result<Map<String, J>, String> {
        let session_id = session_id.trim();
        if session_id.is_empty() {
            return Err("sessionId is required".into());
        }
        let removed = self.live().relay.stop_owned(session_id, token)?;
        let mut out = Map::new();
        out.insert("sessionId".into(), J::from(session_id));
        out.insert("removed".into(), J::Bool(removed));
        Ok(out)
    }

    /// `ensurePostgreSQLServiceHostForward`: an Incus proxy device on the
    /// guest puts the read/write Service on the host's loopback. A device
    /// already aimed at the Service's current ClusterIP is reused.
    fn ensure_host_forward(&self, spec: &Spec) -> Result<i64, String> {
        let rw = format!("{}-rw", spec.cluster_name);
        let service = self.json(
            spec,
            &["get", "service", &rw, "-n", &spec.namespace],
            "get PostgreSQL service read/write service",
        )?;
        let cluster_ip = nested_str(&service, &["spec", "clusterIP"]);
        if cluster_ip.is_empty() {
            return Err("PostgreSQL service read/write Service has no clusterIP".into());
        }
        let device = if spec.relay_device_name.is_empty() {
            "opute-platform-postgres-rw".to_string()
        } else {
            spec.relay_device_name.clone()
        };
        let incus =
            |args: &[&str], timeout: u64| self.incus.run(args, Duration::from_secs(timeout));
        if let Ok(existing) = incus(&["config", "device", "show", &spec.vm_name], 30) {
            if existing.exit_code == 0
                && existing
                    .stdout
                    .contains(&format!("connect: tcp:{cluster_ip}:{SERVICE_PORT}"))
            {
                let port = proxy_listen_port(&existing.stdout);
                if port > 0 {
                    return Ok(port);
                }
            }
        }
        let _ = incus(&["config", "device", "remove", &spec.vm_name, &device], 120);
        let port = free_loopback_port()?;
        let listen = format!("listen=tcp:127.0.0.1:{port}");
        let connect = format!("connect=tcp:{cluster_ip}:{SERVICE_PORT}");
        let added = incus(
            &[
                "config",
                "device",
                "add",
                &spec.vm_name,
                &device,
                "proxy",
                &listen,
                &connect,
            ],
            120,
        );
        match added {
            Ok(added) if added.exit_code == 0 => Ok(port),
            Ok(added) => Err(first_non_empty(&[
                &added.stderr,
                &added.stdout,
                "add PostgreSQL service loopback forward failed",
            ])
            .to_string()),
            Err(e) => Err(first_non_empty(&[&e]).to_string()),
        }
    }

    /// `ensurePostgreSQLServiceRelay`: to a host-agent-managed loopback
    /// forward unless the caller names its own target.
    fn ensure_relay(&self, spec: &Spec, args: &RelayArgs) -> Result<Map<String, J>, String> {
        let mut target_host = args.target_host.trim().to_string();
        let mut target_port = args.target_port;
        if target_host.is_empty() {
            target_port = self.ensure_host_forward(spec)?;
            target_host = "127.0.0.1".into();
        }
        if target_port == 0 {
            target_port = SERVICE_PORT;
        }
        let expected = format!("{}-rw.{}.svc", spec.cluster_name, spec.namespace);
        if target_host != expected && target_host != "127.0.0.1" && target_host != "::1" {
            return Err("localRelay.targetHost must be the CNPG read/write Service or a loopback port-forward".into());
        }
        if target_host == expected && target_port != SERVICE_PORT {
            return Err(format!(
                "localRelay.targetPort must be {SERVICE_PORT} when targeting the read/write Service"
            ));
        }
        self.live().relay.start(args, &target_host, target_port)
    }
}

/// `extractProxyDeviceListenPort`.
fn proxy_listen_port(output: &str) -> i64 {
    let marker = "listen: tcp:127.0.0.1:";
    let Some(index) = output.find(marker) else {
        return 0;
    };
    let rest = &output[index + marker.len()..];
    let end = rest.find(['\n', '\r']).unwrap_or(rest.len());
    match rest[..end].trim().parse::<i64>() {
        Ok(port) if (1..=65535).contains(&port) => port,
        _ => 0,
    }
}

/// `freeLoopbackPort`.
fn free_loopback_port() -> Result<i64, String> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| {
        format!(
            "reserve loopback port: listen tcp 127.0.0.1:0: bind: {}",
            crate::goerr::errno_text(&e)
        )
    })?;
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();
    Ok(i64::from(port))
}

/// `isKubernetesNotFound`.
fn is_not_found(err: &str) -> bool {
    let message = err.to_lowercase();
    message.contains("notfound") || message.contains("not found")
}

/// `admitPVCSizeChange`.
fn admit_pvc_size_change(
    current: &str,
    requested: &str,
    assert_expand: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    let (current, requested) = (current.trim(), requested.trim());
    if requested.is_empty() || current.is_empty() {
        return Ok(());
    }
    let want = crate::resource::parse_byte_capacity(requested)
        .map_err(|e| format!("storageSize {} is invalid: {e}", quote(requested)))?;
    let have = crate::resource::parse_byte_capacity(current)
        .map_err(|e| format!("observed storage size {} is invalid: {e}", quote(current)))?;
    if want < have {
        return Err(format!(
            "storage shrink from {current} to {requested} is refused; persistent volumes are grow-only"
        ));
    }
    if want == have {
        return Ok(());
    }
    assert_expand()
}

// --- Standalone PostgreSQL (standalone_postgres.go) ----------------------------------------

#[derive(Clone, Debug, Default)]
pub struct InstallArgs {
    pub vm_name: String,
    pub namespace: String,
    pub database: String,
}

/// `standalonePostgresOperatorManifest`.
fn standalone_operator_manifest(namespace: &str) -> String {
    format!(
        "apiVersion: v1\nkind: Namespace\nmetadata:\n  name: {namespace}\n---\n{}---\n",
        operator_manifest()
    )
}

/// `standalonePostgresClusterManifest`.
fn standalone_cluster_manifest(namespace: &str, database: &str) -> String {
    format!(
        r#"apiVersion: postgresql.cnpg.io/v1
kind: Cluster
metadata:
  name: postgres
  namespace: {namespace}
spec:
  instances: 1
  imageName: {OPERAND_IMAGE}
  storage:
    size: 2Gi
  bootstrap:
    initdb:
      database: {database}
      owner: app
      encoding: UTF8
      localeCType: C
      localeCollate: C
"#
    )
}

/// `standalonePostgresSQLScript`.
fn standalone_sql_script(username: &str, database: &str, sql: &str) -> String {
    format!(
        "set -eu\npgpass=\"$(mktemp)\"\ntrap 'rm -f \"$pgpass\"' EXIT\ncat >\"$pgpass\"\nchmod 600 \"$pgpass\"\nPGPASSFILE=\"$pgpass\" psql -h 127.0.0.1 -p 5432 -U {} -d {} -v ON_ERROR_STOP=1 -Atqc {}\n",
        shell_quote(username),
        shell_quote(database),
        shell_quote(sql)
    )
}

fn default_namespace(namespace: &str) -> String {
    match namespace.trim() {
        "" => "opute-local-db".into(),
        _ => namespace.to_string(),
    }
}

impl Postgres {
    /// `InstallPostgreSQL`: the operator, its CRD, the Cluster, then up to
    /// six minutes for it to report ready.
    pub fn install(&self, ctx: &RunCtx, args: &InstallArgs) -> Result<Map<String, J>, String> {
        let vm = args.vm_name.trim().to_string();
        if vm.is_empty() {
            return Err("vmName is required".into());
        }
        let namespace = match args.namespace.trim() {
            "" => "opute-local-db".to_string(),
            namespace => namespace.to_string(),
        };
        let database = match args.database.trim() {
            "" => "app".to_string(),
            database => database.to_string(),
        };
        if !standalone_identifier(&namespace) || !standalone_identifier(&database) {
            return Err("namespace and database must be lowercase DNS-safe identifiers".into());
        }
        let apply = argv(&["apply", "-f", "-"]);
        self.kubectl_input(
            &vm,
            &apply,
            standalone_operator_manifest(&namespace).as_bytes(),
            "apply CloudNativePG operator",
        )?;
        self.wait_for_standalone_crd(ctx, &vm)?;
        self.kubectl_input(
            &vm,
            &apply,
            standalone_cluster_manifest(&namespace, &database).as_bytes(),
            "apply CloudNativePG PostgreSQL Cluster",
        )?;
        // `time.Sleep`: this wait is not cancelled with the call.
        let deadline = Instant::now() + Duration::from_secs(6 * 60);
        while Instant::now() < deadline {
            if let Ok(status) = self.standalone_status(&vm, &namespace) {
                if status.get("ready") == Some(&J::Bool(true)) {
                    return Ok(status);
                }
            }
            std::thread::sleep(Duration::from_secs(5));
        }
        Err(format!(
            "PostgreSQL deployment did not become ready in {namespace}"
        ))
    }

    /// `waitForStandalonePostgresCRD`.
    fn wait_for_standalone_crd(&self, ctx: &RunCtx, vm: &str) -> Result<(), String> {
        let mut ticker = Ticker::new(Duration::from_secs(3 * 60));
        loop {
            if self
                .kubectl(
                    vm,
                    &argv(&["get", "crd", "clusters.postgresql.cnpg.io"]),
                    "wait for CloudNativePG Cluster CRD",
                )
                .is_ok()
            {
                return Ok(());
            }
            match ticker.wait(ctx) {
                Ok(()) => {}
                Err(Some(e)) => return Err(e),
                Err(None) => {
                    return Err("CloudNativePG Cluster CRD did not become available".into())
                }
            }
        }
    }

    /// `GetPostgreSQLStatus`.
    pub fn standalone_status(&self, vm: &str, namespace: &str) -> Result<Map<String, J>, String> {
        let namespace = default_namespace(namespace);
        let raw = self.kubectl(
            vm,
            &argv(&[
                "get",
                "cluster.postgresql.cnpg.io",
                "postgres",
                "-n",
                &namespace,
                "-o",
                "json",
            ]),
            "get CloudNativePG PostgreSQL Cluster",
        )?;
        let cluster: J = unmarshal_struct(&raw)
            .map_err(|e| format!("parse CloudNativePG PostgreSQL Cluster: {e}"))?;
        let mut instances = nested_int(&cluster, &["spec", "replicas"]);
        if instances == 0 {
            instances = 1;
        }
        let phase = nested_str(&cluster, &["status", "phase"]);
        let ready_instances = nested_int(&cluster, &["status", "readyInstances"]);
        let ready = phase.to_lowercase().contains("healthy") && ready_instances >= instances;
        let mut out = Map::new();
        out.insert("vmName".into(), J::from(vm));
        out.insert("namespace".into(), J::from(namespace));
        out.insert("cluster".into(), J::from("postgres"));
        out.insert("phase".into(), J::from(phase));
        out.insert("ready".into(), J::Bool(ready));
        out.insert("readyInstances".into(), J::from(ready_instances));
        out.insert("instances".into(), J::from(instances));
        Ok(out)
    }

    /// `DeletePostgreSQL`: the namespace.
    pub fn delete_standalone(&self, vm: &str, namespace: &str) -> Result<Map<String, J>, String> {
        let namespace = default_namespace(namespace);
        self.kubectl(
            vm,
            &argv(&["delete", "namespace", &namespace, "--ignore-not-found=true"]),
            "delete PostgreSQL namespace",
        )?;
        let mut out = Map::new();
        out.insert("vmName".into(), J::from(vm));
        out.insert("namespace".into(), J::from(namespace));
        out.insert("status".into(), J::from("deleted"));
        Ok(out)
    }

    /// `RunSQL`: psql in the primary, with the application credentials
    /// passed on stdin, never in the argv.
    pub fn run_sql(
        &self,
        vm: &str,
        namespace: &str,
        database: &str,
        sql: &str,
    ) -> Result<Map<String, J>, String> {
        if vm.trim().is_empty() || sql.trim().is_empty() {
            return Err("vmName and sql are required".into());
        }
        let namespace = if namespace.is_empty() {
            "opute-local-db"
        } else {
            namespace
        }
        .trim();
        let database = if database.is_empty() { "app" } else { database }.trim();
        if !standalone_identifier(namespace) || !standalone_identifier(database) {
            return Err("namespace and database must be lowercase DNS-safe identifiers".into());
        }
        if sql.len() > 64 * 1024 {
            return Err("sql exceeds 64 KiB limit".into());
        }
        let (pod, credentials) = self.standalone_target(vm, namespace)?;
        let script = standalone_sql_script(&credentials.username, database, sql);
        let out = self.kubectl_input(
            vm,
            &argv(&[
                "exec", "-i", &pod, "-n", namespace, "--", "sh", "-ceu", &script,
            ]),
            &pgpass(&credentials),
            "run SQL through CloudNativePG primary",
        )?;
        let mut result = Map::new();
        result.insert("vmName".into(), J::from(vm));
        result.insert("namespace".into(), J::from(namespace));
        result.insert("database".into(), J::from(database));
        result.insert("output".into(), J::from(out));
        Ok(result)
    }

    /// `standalonePostgresExecutionTarget`: the first running primary and
    /// the application Secret's credentials.
    fn standalone_target(
        &self,
        vm: &str,
        namespace: &str,
    ) -> Result<(String, Credentials), String> {
        let raw = self.kubectl(
            vm,
            &argv(&[
                "get",
                "pods",
                "-n",
                namespace,
                "-l",
                "cnpg.io/cluster=postgres,role=primary",
                "-o",
                "json",
            ]),
            "find CloudNativePG primary",
        )?;
        let pods: J =
            unmarshal_struct(&raw).map_err(|e| format!("parse CloudNativePG primary pods: {e}"))?;
        let pod = pods
            .get("items")
            .and_then(J::as_array)
            .into_iter()
            .flatten()
            .map(|item| {
                (
                    nested_str(item, &["metadata", "name"]),
                    nested_str(item, &["status", "phase"]),
                )
            })
            .find(|(name, phase)| !name.is_empty() && phase == "Running")
            .map(|(name, _)| name)
            .ok_or("CloudNativePG primary pod is not running")?;
        let raw = self.kubectl(
            vm,
            &argv(&[
                "get",
                "secret",
                "postgres-app",
                "-n",
                namespace,
                "-o",
                "json",
            ]),
            "read CloudNativePG application Secret",
        )?;
        let secret = unmarshal_map(&raw)
            .map_err(|e| format!("parse CloudNativePG application Secret: {e}"))?;
        let data = secret.get("data").cloned().unwrap_or(J::Null);
        let credentials = Credentials {
            username: secret_value(&data, "username"),
            password: secret_value(&data, "password"),
        };
        if credentials.username.is_empty() || credentials.password.is_empty() {
            return Err("CloudNativePG application Secret is incomplete".into());
        }
        Ok((pod, credentials))
    }
}
