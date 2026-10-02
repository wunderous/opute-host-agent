//! Local durable stores opened at startup: standalone state (`state.db`) and
//! the authorization server store (`authz.sqlite`).
//!
//! M1 opens and migrates them exactly as the Go baseline does at startup, so
//! a Rust agent and a Go agent leave the same schema, rows and files behind.
//! Reads and writes beyond startup arrive with the milestones that own them
//! (M2 authz flows, M5 durable state).

use std::path::Path;

use rusqlite::{Connection, OpenFlags};

use crate::ddl;
use crate::goerr::{self, Result};

/// modernc.org/sqlite formats errors as "<message> (<extended code>)".
fn sqlite_err(err: rusqlite::Error) -> String {
    match &err {
        rusqlite::Error::SqliteFailure(e, msg) => {
            let text = msg
                .clone()
                .unwrap_or_else(|| rusqlite::ffi::code_to_str(e.extended_code).to_string());
            format!("{text} ({})", e.extended_code)
        }
        other => other.to_string(),
    }
}

/// Connect-stage failures (Go's `db.Ping`, which also applies the DSN
/// pragmas). modernc reports SQLite's own message and extended code, e.g.
/// "unable to open database file (14)"; rusqlite appends ": <path>" to open
/// failures, so that suffix is removed.
fn open_err(path: &Path, err: rusqlite::Error) -> String {
    match &err {
        rusqlite::Error::SqliteFailure(e, Some(msg)) => {
            let suffix = format!(": {}", path.display());
            let msg = msg.strip_suffix(&suffix).unwrap_or(msg);
            format!("{msg} ({})", e.extended_code)
        }
        _ => sqlite_err(err),
    }
}

fn open(path: &Path, pragmas: &[&str]) -> std::result::Result<Connection, String> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| open_err(path, e))?;
    for pragma in pragmas {
        // journal_mode returns a row; query_row handles both shapes.
        conn.query_row(&format!("PRAGMA {pragma}"), [], |_| Ok(()))
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(()),
                e => Err(open_err(path, e)),
            })?;
    }
    Ok(conn)
}

fn has_column(conn: &Connection, table: &str, column: &str) -> std::result::Result<bool, String> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(sqlite_err)?;
    let mut rows = stmt.query([]).map_err(sqlite_err)?;
    while let Some(row) = rows.next().map_err(sqlite_err)? {
        let name: String = row.get(1).map_err(sqlite_err)?;
        if name == column {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Go `ensureTableColumn`.
fn ensure_column(
    conn: &Connection,
    table: &str,
    column: &str,
    definition: &str,
) -> std::result::Result<(), String> {
    if !has_column(conn, table, column)? {
        conn.execute_batch(&format!(
            "ALTER TABLE {table} ADD COLUMN {column} {definition}"
        ))
        .map_err(sqlite_err)?;
    }
    Ok(())
}

pub struct StateStore {
    conn: Option<Connection>,
}

impl StateStore {
    /// `state.Open`: create the directory (0700), open `state.db` with WAL and
    /// a busy timeout, create/migrate tables, and mark interrupted work
    /// `unknown` rather than inventing an outcome.
    pub fn open(dir: &Path) -> Result<StateStore> {
        if dir.as_os_str().to_string_lossy().trim().is_empty() {
            return Err(go_err!("state directory is required"));
        }
        goerr::mkdir_all(dir, 0o700).map_err(|e| go_err!("create state directory: {e}"))?;
        let conn = open(
            &dir.join("state.db"),
            &["busy_timeout = 5000", "journal_mode = WAL"],
        )
        .map_err(|e| go_err!("configure standalone state: {e}"))?;
        conn.execute_batch(ddl::STATE_INIT)
            .map_err(|e| go_err!("initialize standalone state: {}", sqlite_err(e)))?;
        ensure_column(
            &conn,
            "plan_runs",
            "recipe_json",
            "TEXT NOT NULL DEFAULT ''",
        )
        .map_err(|e| go_err!("migrate standalone plan state: {e}"))?;
        Self::ensure_active_capabilities(&conn)
            .map_err(|e| go_err!("migrate active capability state: {e}"))?;
        for (name, def) in [
            ("descriptor_json", "TEXT NOT NULL DEFAULT ''"),
            ("manifest_json", "TEXT NOT NULL DEFAULT ''"),
        ] {
            ensure_column(&conn, "provider_generations", name, def)
                .map_err(|e| go_err!("migrate provider generation {name}: {e}"))?;
        }
        ensure_column(
            &conn,
            "operations",
            "task_snapshot_json",
            "TEXT NOT NULL DEFAULT ''",
        )
        .map_err(|e| go_err!("migrate task snapshot state: {e}"))?;
        // Legacy invocation rows predate the separate execution binding.
        ensure_column(
            &conn,
            "capability_invocations",
            "binding_json",
            "TEXT NOT NULL DEFAULT ''",
        )
        .map_err(|e| go_err!("migrate capability invocation binding: {e}"))?;
        conn.execute_batch(ddl::STATE_RESOURCE_REGISTRY)
            .map_err(|e| go_err!("migrate resource registry: {}", sqlite_err(e)))?;
        Ok(StateStore { conn: Some(conn) })
    }

    fn ensure_active_capabilities(conn: &Connection) -> std::result::Result<(), String> {
        conn.execute_batch(ddl::STATE_ACTIVE_CAPABILITIES)
            .map_err(sqlite_err)?;
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='active_runtimes'",
                [],
                |r| r.get(0),
            )
            .map_err(sqlite_err)?;
        if exists == 1 {
            for (name, def) in [
                ("recipe_id", "TEXT NOT NULL DEFAULT ''"),
                ("recipe_version", "TEXT NOT NULL DEFAULT ''"),
                ("recipe_hash", "TEXT NOT NULL DEFAULT ''"),
                ("run_id", "TEXT NOT NULL DEFAULT ''"),
                ("input_bindings_json", "TEXT NOT NULL DEFAULT '{}'"),
                ("observation_json", "TEXT NOT NULL DEFAULT '{}'"),
                ("activated_at", "TEXT NOT NULL DEFAULT ''"),
            ] {
                ensure_column(conn, "active_runtimes", name, def)?;
            }
            conn.execute_batch(ddl::STATE_COPY_ACTIVE_RUNTIMES)
                .map_err(sqlite_err)?;
        }
        Ok(())
    }

    /// `Store.UpsertResource` for a parsed, tenant-checked URI.
    pub fn upsert_resource(
        &self,
        uri: &crate::resource::Uri,
        coordinates_json: &str,
    ) -> std::result::Result<(), String> {
        let conn = self.conn.as_ref().ok_or("state store closed")?;
        let now = crate::hostobs::rfc3339_nano_now();
        conn.execute(
            "INSERT INTO resource_registry(
        uri, resource_type, tenant_id, resource_id, coordinates_json, status, created_at, updated_at
    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?)
    ON CONFLICT(uri) DO UPDATE SET
        resource_type=excluded.resource_type, tenant_id=excluded.tenant_id,
        resource_id=excluded.resource_id, coordinates_json=excluded.coordinates_json,
        status=excluded.status, updated_at=excluded.updated_at",
            rusqlite::params![
                uri.to_string(),
                uri.resource_type,
                uri.tenant_id,
                uri.resource_id,
                coordinates_json,
                "active",
                now,
                now
            ],
        )
        .map_err(sqlite_err)?;
        Ok(())
    }

    /// Close explicitly so the WAL is checkpointed and removed, as Go's
    /// `db.Close` does; a leftover `-wal` file would be an observable diff.
    pub fn close(&mut self) {
        if let Some(conn) = self.conn.take() {
            let _ = conn.close();
        }
    }
}

impl Drop for StateStore {
    fn drop(&mut self) {
        self.close();
    }
}

/// Open `authz.sqlite` for the operator CLI without running the server's
/// preparation: the CLI must never provision or rotate secrets as a side
/// effect of a different environment.
pub fn open_authz_for_operator(dir: &Path) -> Result<Connection> {
    let path = dir.join("authz.sqlite");
    if !path.exists() {
        return Err(go_err!(
            "no authz store at {}; start the Host Agent once, or pass --state-dir",
            path.display()
        ));
    }
    let conn = open(&path, &["busy_timeout = 5000", "foreign_keys = ON"]).map_err(goerr::Error)?;
    let ready: bool = conn
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='schema_migrations'",
            [],
            |_| Ok(()),
        )
        .is_ok();
    if !ready {
        return Err(go_err!(
            "the authz store at {} predates OAuth issuance; start the Host Agent once to migrate it",
            path.display()
        ));
    }
    Ok(conn)
}

pub struct AuthzStore {
    conn: Option<Connection>,
}

impl AuthzStore {
    /// `authz.Open`: open `authz.sqlite` and register the two built-in clients.
    pub fn open(dir: &Path, opute_secret: &str) -> Result<AuthzStore> {
        let dir = if dir.as_os_str().to_string_lossy().trim().is_empty() {
            std::env::temp_dir()
        } else {
            dir.to_path_buf()
        };
        goerr::mkdir_all(&dir, 0o700).map_err(|e| go_err!("create authz state dir: {e}"))?;
        let conn = open(
            &dir.join("authz.sqlite"),
            &[
                "busy_timeout = 5000",
                "journal_mode = WAL",
                "foreign_keys = ON",
            ],
        )
        .map_err(|e| go_err!("configure authz store: {e}"))?;
        conn.execute_batch(ddl::AUTHZ_INIT)
            .map_err(|e| go_err!("init authz store: {}", sqlite_err(e)))?;
        let store = AuthzStore { conn: Some(conn) };
        // Issuance contract (decision D8): the built-in client rows, their
        // provisioned secrets, and the one-time revocation of earlier tokens.
        // Go re-registers the built-ins on every open; here `prepare` owns
        // them so a restart never resets a provisioned secret.
        crate::oauth::prepare(store.conn(), &dir, opute_secret)
            .map_err(|e| go_err!("prepare oauth issuance: {e}"))?;
        Ok(store)
    }

    pub(crate) fn conn(&self) -> &Connection {
        self.conn.as_ref().expect("open store")
    }

    /// `Store.tokenByHash`.
    pub fn token_by_hash(&self, hash: &str) -> std::result::Result<Option<TokenRecord>, String> {
        let conn = self.conn.as_ref().ok_or("authz store closed")?;
        let row = conn.query_row(
            "SELECT token_hash, client_id, resource, scope, expires_at, revoked FROM tokens WHERE token_hash=?",
            [hash],
            |r| {
                Ok(TokenRecord {
                    resource: r.get(2)?,
                    scope: r.get(3)?,
                    expires_at: r.get(4)?,
                    revoked: r.get::<_, i64>(5)? == 1,
                })
            },
        );
        match row {
            Ok(rec) => Ok(Some(rec)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(sqlite_err(e)),
        }
    }

    /// `Store.revokeHash`.
    pub fn revoke_hash(&self, hash: &str) -> std::result::Result<(), String> {
        let conn = self.conn.as_ref().ok_or("authz store closed")?;
        conn.execute("UPDATE tokens SET revoked=1 WHERE token_hash=?", [hash])
            .map_err(sqlite_err)?;
        Ok(())
    }

    pub fn close(&mut self) {
        if let Some(conn) = self.conn.take() {
            let _ = conn.close();
        }
    }
}

/// The columns `Authorize` reads from a token row.
#[derive(Debug)]
pub struct TokenRecord {
    pub resource: String,
    pub scope: String,
    pub expires_at: i64,
    pub revoked: bool,
}

impl Drop for AuthzStore {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tables(path: &Path) -> Vec<(String, String)> {
        let conn = Connection::open(path).unwrap();
        let mut stmt = conn
            .prepare("SELECT name, sql FROM sqlite_master WHERE type='table' ORDER BY name")
            .unwrap();
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    }

    #[test]
    fn state_open_creates_schema_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("nested").join("state");
        let mut store = StateStore::open(&state).unwrap();
        store.close();
        let first = tables(&state.join("state.db"));
        let names: Vec<_> = first.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            [
                "active_capabilities",
                "active_runtimes",
                "capability_invocations",
                "operations",
                "plan_runs",
                "provider_generations",
                "resource_registry"
            ]
        );
        // binding_json is added by migration, exactly as in Go.
        let invocations = &first
            .iter()
            .find(|(n, _)| n == "capability_invocations")
            .unwrap()
            .1;
        assert!(invocations.ends_with(", binding_json TEXT NOT NULL DEFAULT '')"));
        StateStore::open(&state).unwrap().close();
        assert_eq!(first, tables(&state.join("state.db")));
        assert!(
            !state.join("state.db-wal").exists(),
            "WAL must be checkpointed on close"
        );
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&state).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }

    #[test]
    fn state_open_reports_go_style_mkdir_error() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("f");
        std::fs::write(&file, b"").unwrap();
        let err = StateStore::open(&file.join("state")).err().unwrap();
        assert_eq!(
            err.0,
            format!(
                "create state directory: mkdir {}: not a directory",
                file.display()
            )
        );
    }

    #[test]
    fn open_failure_text_matches_modernc() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("state.db")).unwrap();
        let err = StateStore::open(dir.path()).err().unwrap();
        assert_eq!(
            err.0,
            "configure standalone state: unable to open database file (14)"
        );
    }

    #[test]
    fn authz_restart_keeps_provisioned_secrets() {
        let dir = tempfile::tempdir().unwrap();
        AuthzStore::open(dir.path(), "").unwrap().close();
        let read = || {
            ["opute-mcp-host", "host-agent-provider"].map(|c| {
                std::fs::read_to_string(dir.path().join("credentials").join(format!("{c}.json")))
                    .unwrap()
            })
        };
        let before = read();
        AuthzStore::open(dir.path(), "").unwrap().close();
        assert_eq!(read(), before);
    }

    #[test]
    fn authz_open_registers_builtin_clients_once() {
        let dir = tempfile::tempdir().unwrap();
        AuthzStore::open(dir.path(), "").unwrap().close();
        AuthzStore::open(dir.path(), "secret").unwrap().close();
        let conn = Connection::open(dir.path().join("authz.sqlite")).unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM clients", [], |r| r.get(0))
            .unwrap();
        // Go's two built-ins plus host-agent-provider (oauth-issuance, D8).
        assert_eq!(count, 3);
        let hash: String = conn
            .query_row(
                "SELECT secret_hash FROM clients WHERE client_id='opute-mcp-host'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(
            hash,
            hex::encode(<sha2::Sha256 as sha2::Digest>::digest("secret"))
        );
    }
}
