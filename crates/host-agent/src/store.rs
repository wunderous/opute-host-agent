//! Local durable stores opened at startup: standalone state (`state.db`) and
//! the authorization server store (`authz.sqlite`).
//!
//! M1 opens and migrates them exactly as the Go baseline does at startup, so
//! a Rust agent and a Go agent leave the same schema, rows and files behind.
//! Reads and writes beyond startup arrive with the milestones that own them
//! (M2 authz flows, M5 durable state).

use std::path::Path;

use rusqlite::{Connection, OpenFlags, TransactionBehavior};
use serde_json::{Map, Value as J};

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

/// A registry record: its status and provider-native coordinates.
pub type ResourceRecord = (String, Map<String, J>);

pub struct StateStore {
    conn: Option<Connection>,
}

// M5 adds the operations/plan/active-capability CRUD surface below ahead of
// wiring it into tasks.rs's in-memory lifecycle (a separate, deliberate
// decision): until that call site lands, these are only exercised by tests.
#[allow(dead_code)]
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

    /// `Store.GetResource`: the record's status and coordinates, or `None`
    /// when the registry has never seen the URI.
    pub fn get_resource(
        &self,
        uri: &crate::resource::Uri,
    ) -> std::result::Result<Option<ResourceRecord>, String> {
        let conn = self.conn.as_ref().ok_or("state store closed")?;
        let row = conn.query_row(
            "SELECT uri, resource_type, tenant_id, resource_id, coordinates_json, status
        FROM resource_registry WHERE uri = ?",
            [uri.to_string()],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                ))
            },
        );
        let (stored, kind, tenant, id, coordinates, status) = match row {
            Ok(row) => row,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
            Err(e) => return Err(sqlite_err(e)),
        };
        let coordinates = match serde_json::from_str::<J>(&coordinates) {
            Ok(J::Object(m)) => m,
            Ok(J::Null) => Map::new(),
            Ok(_) | Err(_) => return Err("decode resource coordinates: invalid JSON".into()),
        };
        if tenant != uri.tenant_id
            || kind != uri.resource_type
            || id != uri.resource_id
            || stored != uri.to_string()
        {
            return Err(format!(
                "resource registry record does not match URI {}",
                crate::goerr::quote(&uri.to_string())
            ));
        }
        Ok(Some((status, coordinates)))
    }

    /// Go's `sql.DB.Begin()` under the DSN's `_txlock=immediate`: a bare
    /// `BEGIN` is deferred and only takes a lock on the transaction's first
    /// statement, so a transaction that reads then writes can be asked to
    /// upgrade a shared read lock to a write lock after another connection
    /// already holds one — SQLite refuses that upgrade immediately with
    /// SQLITE_BUSY instead of waiting out `busy_timeout`. `BEGIN IMMEDIATE`
    /// takes the write lock up front, so a concurrent writer waits instead.
    /// Every multi-statement write against `state.db` must use this, not
    /// `Connection::transaction()` (which defaults to deferred).
    fn transaction_immediate(&mut self) -> std::result::Result<rusqlite::Transaction<'_>, String> {
        let conn = self.conn.as_mut().ok_or("state store closed")?;
        conn.transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(sqlite_err)
    }

    /// `Store.Create`: mark an operation `working`.
    pub fn create_operation(
        &self,
        operation_id: &str,
        tool_name: &str,
        description: &str,
    ) -> std::result::Result<(), String> {
        let conn = self.conn.as_ref().ok_or("state store closed")?;
        let now = crate::hostobs::rfc3339_nano_now();
        conn.execute(
            "INSERT OR REPLACE INTO operations(operation_id, tool_name, status, description, created_at, updated_at) VALUES (?, ?, 'working', ?, ?, ?)",
            rusqlite::params![operation_id, tool_name, description, now, now],
        )
        .map_err(sqlite_err)?;
        Ok(())
    }

    /// `Store.Complete`.
    pub fn complete_operation(
        &self,
        operation_id: &str,
        result: &J,
    ) -> std::result::Result<(), String> {
        let conn = self.conn.as_ref().ok_or("state store closed")?;
        let encoded = serde_json::to_string(result).map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE operations SET status = 'completed', updated_at = ?, result_json = ?, error_message = NULL WHERE operation_id = ?",
            rusqlite::params![crate::hostobs::rfc3339_nano_now(), encoded, operation_id],
        )
        .map_err(sqlite_err)?;
        Ok(())
    }

    /// `Store.Fail`.
    pub fn fail_operation(
        &self,
        operation_id: &str,
        message: &str,
    ) -> std::result::Result<(), String> {
        let conn = self.conn.as_ref().ok_or("state store closed")?;
        conn.execute(
            "UPDATE operations SET status = 'failed', updated_at = ?, error_message = ? WHERE operation_id = ?",
            rusqlite::params![crate::hostobs::rfc3339_nano_now(), message, operation_id],
        )
        .map_err(sqlite_err)?;
        Ok(())
    }

    /// `Store.Cancel`: only a `working` or `unknown` (interrupted-by-restart)
    /// operation can still be cancelled.
    pub fn cancel_operation(&self, operation_id: &str) -> std::result::Result<(), String> {
        let conn = self.conn.as_ref().ok_or("state store closed")?;
        conn.execute(
            "UPDATE operations SET status = 'cancelled', updated_at = ? WHERE operation_id = ? AND status IN ('working', 'unknown')",
            rusqlite::params![crate::hostobs::rfc3339_nano_now(), operation_id],
        )
        .map_err(sqlite_err)?;
        Ok(())
    }

    /// `Store.SaveTaskSnapshot`: the protocol-visible state of an MCP Tasks
    /// handle, kept separate from the legacy operation projection so a task
    /// can be reconstructed after the in-memory executor is gone. The
    /// snapshot's own `status` field (when present) becomes the row's
    /// status; callers must redact write-only fields before this call, since
    /// the snapshot is stored verbatim.
    pub fn save_task_snapshot(
        &self,
        task_id: &str,
        tool_name: &str,
        description: &str,
        snapshot: &J,
    ) -> std::result::Result<(), String> {
        let status = snapshot
            .get("status")
            .and_then(J::as_str)
            .filter(|s| !s.is_empty())
            .unwrap_or("working");
        self.save_snapshot_with_status(task_id, tool_name, description, snapshot, status)
    }

    /// Go's live Tasks projection carries `tasks.Status`, a named string
    /// type. `SaveTaskSnapshot`'s plain-string assertion therefore falls back
    /// to `working`, even for a terminal task. Preserve that legacy row
    /// projection without altering the truthful protocol snapshot. Decoded
    /// JSON callers of `save_task_snapshot` still have plain-string semantics.
    pub fn save_registry_task_snapshot(
        &self,
        task_id: &str,
        tool_name: &str,
        description: &str,
        snapshot: &J,
    ) -> std::result::Result<(), String> {
        self.save_snapshot_with_status(task_id, tool_name, description, snapshot, "working")
    }

    fn save_snapshot_with_status(
        &self,
        task_id: &str,
        tool_name: &str,
        description: &str,
        snapshot: &J,
        status: &str,
    ) -> std::result::Result<(), String> {
        let conn = self.conn.as_ref().ok_or("state store closed")?;
        let encoded = serde_json::to_string(snapshot).map_err(|e| e.to_string())?;
        let now = crate::hostobs::rfc3339_nano_now();
        conn.execute(
            "INSERT INTO operations(
        operation_id, tool_name, status, description, created_at, updated_at, task_snapshot_json
    ) VALUES (?, ?, ?, ?, ?, ?, ?)
    ON CONFLICT(operation_id) DO UPDATE SET
        tool_name=excluded.tool_name,
        status=excluded.status,
        description=excluded.description,
        updated_at=excluded.updated_at,
        task_snapshot_json=excluded.task_snapshot_json",
            rusqlite::params![task_id, tool_name, status, description, now, now, encoded],
        )
        .map_err(sqlite_err)?;
        Ok(())
    }

    /// `Store.ListTaskSnapshots`, most recently updated first.
    pub fn list_task_snapshots(&self) -> std::result::Result<Vec<J>, String> {
        let conn = self.conn.as_ref().ok_or("state store closed")?;
        let mut stmt = conn
            .prepare("SELECT task_snapshot_json FROM operations WHERE task_snapshot_json <> '' ORDER BY updated_at DESC")
            .map_err(sqlite_err)?;
        let rows = stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(sqlite_err)?;
        let mut out = Vec::new();
        for encoded in rows {
            let encoded = encoded.map_err(sqlite_err)?;
            out.push(
                serde_json::from_str(&encoded).map_err(|e| format!("decode task snapshot: {e}"))?,
            );
        }
        Ok(out)
    }

    /// `Store.List`: the `limit` most recently updated operations (Go clamps
    /// an out-of-range limit to 50, the same default it uses for `<= 0`).
    pub fn list_operations(&self, limit: i64) -> std::result::Result<Vec<J>, String> {
        let conn = self.conn.as_ref().ok_or("state store closed")?;
        let limit = if limit <= 0 || limit > 100 { 50 } else { limit };
        let mut stmt = conn
            .prepare("SELECT operation_id, tool_name, status, description, created_at, updated_at, result_json, error_message FROM operations ORDER BY updated_at DESC LIMIT ?")
            .map_err(sqlite_err)?;
        let rows = stmt
            .query_map([limit], scan_operation)
            .map_err(sqlite_err)?;
        rows.collect::<std::result::Result<Vec<_>, _>>()
            .map_err(sqlite_err)
    }

    /// `Store.Get`.
    pub fn get_operation(&self, operation_id: &str) -> std::result::Result<Option<J>, String> {
        let conn = self.conn.as_ref().ok_or("state store closed")?;
        match conn.query_row(
            "SELECT operation_id, tool_name, status, description, created_at, updated_at, result_json, error_message FROM operations WHERE operation_id = ?",
            [operation_id],
            scan_operation,
        ) {
            Ok(item) => Ok(Some(item)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(sqlite_err(e)),
        }
    }

    /// `Store.CreatePlan`. Returns the stored record and whether this call
    /// was the one that inserted it, vs. an existing run with the same
    /// `(plan_id, generation, idempotency_key)` identity.
    pub fn create_plan(
        &self,
        record: &PlanRecord,
    ) -> std::result::Result<(PlanRecord, bool), String> {
        let conn = self.conn.as_ref().ok_or("state store closed")?;
        let now = crate::hostobs::rfc3339_nano_now();
        let created_at = if record.created_at.is_empty() {
            &now
        } else {
            &record.created_at
        };
        let updated_at = if record.updated_at.is_empty() {
            &now
        } else {
            &record.updated_at
        };
        conn.execute(
            "INSERT INTO plan_runs(
        run_id, plan_id, generation, idempotency_key, document_hash,
        catalog_revision, status, plan_json, recipe_json, state_json, created_at, updated_at,
        error_message
    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL)
    ON CONFLICT(plan_id, generation, idempotency_key) DO NOTHING",
            rusqlite::params![
                record.run_id,
                record.plan_id,
                record.generation,
                record.idempotency_key,
                record.document_hash,
                record.catalog_revision,
                record.status,
                record.plan_json,
                record.recipe_json,
                record.state_json,
                created_at,
                updated_at,
            ],
        )
        .map_err(sqlite_err)?;
        match self.find_plan(&record.plan_id, record.generation, &record.idempotency_key)? {
            Some(existing) => {
                let created = existing.run_id == record.run_id;
                Ok((existing, created))
            }
            None => Err(format!("plan run was not persisted: {}", record.run_id)),
        }
    }

    /// `Store.FindPlan`.
    pub fn find_plan(
        &self,
        plan_id: &str,
        generation: i64,
        idempotency_key: &str,
    ) -> std::result::Result<Option<PlanRecord>, String> {
        let conn = self.conn.as_ref().ok_or("state store closed")?;
        match conn.query_row(
            "SELECT run_id, plan_id, generation, idempotency_key,
        document_hash, catalog_revision, status, plan_json, recipe_json, state_json,
        created_at, updated_at, COALESCE(error_message, '')
        FROM plan_runs WHERE plan_id = ? AND generation = ? AND idempotency_key = ?",
            rusqlite::params![plan_id, generation, idempotency_key],
            scan_plan,
        ) {
            Ok(record) => Ok(Some(record)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(sqlite_err(e)),
        }
    }

    /// `Store.GetPlan`.
    pub fn get_plan(&self, run_id: &str) -> std::result::Result<Option<PlanRecord>, String> {
        let conn = self.conn.as_ref().ok_or("state store closed")?;
        match conn.query_row(
            "SELECT run_id, plan_id, generation, idempotency_key,
        document_hash, catalog_revision, status, plan_json, recipe_json, state_json,
        created_at, updated_at, COALESCE(error_message, '')
        FROM plan_runs WHERE run_id = ?",
            [run_id],
            scan_plan,
        ) {
            Ok(record) => Ok(Some(record)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(sqlite_err(e)),
        }
    }

    /// `Store.UpdatePlan`. An empty `error_message` clears the column, as
    /// Go's `NULLIF(?, '')` does.
    pub fn update_plan(
        &self,
        run_id: &str,
        status: &str,
        state_json: &str,
        error_message: &str,
    ) -> std::result::Result<(), String> {
        let conn = self.conn.as_ref().ok_or("state store closed")?;
        conn.execute(
            "UPDATE plan_runs SET status = ?, state_json = ?,
        updated_at = ?, error_message = NULLIF(?, '') WHERE run_id = ?",
            rusqlite::params![
                status,
                state_json,
                crate::hostobs::rfc3339_nano_now(),
                error_message,
                run_id
            ],
        )
        .map_err(sqlite_err)?;
        Ok(())
    }

    /// `Store.UpdatePlanDocumentHash`.
    pub fn update_plan_document_hash(
        &self,
        run_id: &str,
        document_hash: &str,
    ) -> std::result::Result<(), String> {
        let conn = self.conn.as_ref().ok_or("state store closed")?;
        conn.execute(
            "UPDATE plan_runs SET document_hash = ?, updated_at = ? WHERE run_id = ?",
            rusqlite::params![document_hash, crate::hostobs::rfc3339_nano_now(), run_id],
        )
        .map_err(sqlite_err)?;
        Ok(())
    }

    /// `Store.UpdatePlanCatalogRevision`.
    pub fn update_plan_catalog_revision(
        &self,
        run_id: &str,
        catalog_revision: &str,
    ) -> std::result::Result<(), String> {
        let conn = self.conn.as_ref().ok_or("state store closed")?;
        conn.execute(
            "UPDATE plan_runs SET catalog_revision = ?, updated_at = ? WHERE run_id = ?",
            rusqlite::params![catalog_revision, crate::hostobs::rfc3339_nano_now(), run_id],
        )
        .map_err(sqlite_err)?;
        Ok(())
    }

    /// `Store.CompletePlanWithActiveCapability`: the plan's terminal write
    /// and the new active-capability selection commit atomically. This is
    /// the write Go's `_txlock=immediate` comment calls out by name — it
    /// reads nothing first, but it is two statements against two tables, and
    /// without `BEGIN IMMEDIATE` a second concurrent writer's own deferred
    /// transaction could be refused SQLITE_BUSY on commit instead of waiting.
    pub fn complete_plan_with_active_capability(
        &mut self,
        run_id: &str,
        state_json: &str,
        active: &ActiveCapabilityRecord,
    ) -> std::result::Result<(), String> {
        let tx = self.transaction_immediate()?;
        let now = crate::hostobs::rfc3339_nano_now();
        tx.execute(
            "UPDATE plan_runs SET status = 'completed', state_json = ?, updated_at = ?, error_message = NULL WHERE run_id = ?",
            rusqlite::params![state_json, now, run_id],
        )
        .map_err(sqlite_err)?;
        let activated_at = if active.activated_at.is_empty() {
            &now
        } else {
            &active.activated_at
        };
        tx.execute(
            "INSERT INTO active_capabilities(
        capability, serving_contract, provider, recipe_id, recipe_version,
        recipe_hash, run_id, input_bindings_json, observation_json, activated_at
    ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
    ON CONFLICT(capability) DO UPDATE SET
        serving_contract=excluded.serving_contract,
        provider=excluded.provider,
        recipe_id=excluded.recipe_id,
        recipe_version=excluded.recipe_version,
        recipe_hash=excluded.recipe_hash,
        run_id=excluded.run_id,
        input_bindings_json=excluded.input_bindings_json,
        observation_json=excluded.observation_json,
        activated_at=excluded.activated_at",
            rusqlite::params![
                active.capability,
                active.serving_contract,
                active.provider,
                active.recipe_id,
                active.recipe_version,
                active.recipe_hash,
                active.run_id,
                active.input_bindings_json,
                active.observation_json,
                activated_at,
            ],
        )
        .map_err(sqlite_err)?;
        tx.commit().map_err(sqlite_err)
    }

    /// `Store.GetActiveCapability`.
    pub fn get_active_capability(
        &self,
        capability: &str,
    ) -> std::result::Result<Option<ActiveCapabilityRecord>, String> {
        let conn = self.conn.as_ref().ok_or("state store closed")?;
        match conn.query_row(
            "SELECT capability, serving_contract, provider, recipe_id,
        recipe_version, recipe_hash, run_id, input_bindings_json,
        observation_json, activated_at FROM active_capabilities WHERE capability = ?",
            [capability],
            |r| {
                Ok(ActiveCapabilityRecord {
                    capability: r.get(0)?,
                    serving_contract: r.get(1)?,
                    provider: r.get(2)?,
                    recipe_id: r.get(3)?,
                    recipe_version: r.get(4)?,
                    recipe_hash: r.get(5)?,
                    run_id: r.get(6)?,
                    input_bindings_json: r.get(7)?,
                    observation_json: r.get(8)?,
                    activated_at: r.get(9)?,
                })
            },
        ) {
            Ok(record) => Ok(Some(record)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(sqlite_err(e)),
        }
    }

    /// `Store.RemoveActiveCapabilitiesForProvider`: clears active selections
    /// owned by a provider after its confirmed teardown completes.
    pub fn remove_active_capabilities_for_provider(
        &self,
        provider: &str,
    ) -> std::result::Result<(), String> {
        let provider = provider.trim();
        if provider.is_empty() {
            return Err("provider is required".into());
        }
        let conn = self.conn.as_ref().ok_or("state store closed")?;
        conn.execute(
            "DELETE FROM active_capabilities WHERE provider = ?",
            [provider],
        )
        .map_err(sqlite_err)?;
        Ok(())
    }

    /// `Store.SaveProviderGeneration`: persists observations, never an adapter.
    pub fn save_provider_generation(
        &self,
        record: &ProviderGenerationRecord,
    ) -> std::result::Result<(), String> {
        if [
            &record.generation_id,
            &record.provider_id,
            &record.provider_version,
            &record.manifest_hash,
            &record.endpoint,
        ]
        .iter()
        .any(|s| s.is_empty())
        {
            return Err(
                "provider generation identity, manifest hash, and endpoint are required".into(),
            );
        }
        let conn = self.conn.as_ref().ok_or("state store closed")?;
        let now = crate::hostobs::rfc3339_nano_now();
        let created = if record.created_at.is_empty() {
            &now
        } else {
            &record.created_at
        };
        conn.execute(
            "INSERT INTO provider_generations(generation_id, provider_id, provider_version, manifest_hash, endpoint, descriptor_json, manifest_json, catalog_revision, status, created_at, active_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(generation_id) DO UPDATE SET provider_id=excluded.provider_id, provider_version=excluded.provider_version,
             manifest_hash=excluded.manifest_hash, endpoint=excluded.endpoint, descriptor_json=excluded.descriptor_json,
             manifest_json=excluded.manifest_json, catalog_revision=excluded.catalog_revision, status=excluded.status, active_at=excluded.active_at",
            rusqlite::params![record.generation_id, record.provider_id, record.provider_version, record.manifest_hash,
                record.endpoint, record.descriptor_json, record.manifest_json, record.catalog_revision, record.status, created, record.active_at],
        ).map_err(sqlite_err)?;
        Ok(())
    }

    pub fn list_provider_generations(
        &self,
    ) -> std::result::Result<Vec<ProviderGenerationRecord>, String> {
        let conn = self.conn.as_ref().ok_or("state store closed")?;
        let mut stmt = conn.prepare("SELECT generation_id, provider_id, provider_version, manifest_hash, endpoint,
            COALESCE(descriptor_json, ''), COALESCE(manifest_json, ''), catalog_revision, status, created_at, COALESCE(active_at, '')
            FROM provider_generations ORDER BY created_at").map_err(sqlite_err)?;
        let rows = stmt
            .query_map([], |r| {
                Ok(ProviderGenerationRecord {
                    generation_id: r.get(0)?,
                    provider_id: r.get(1)?,
                    provider_version: r.get(2)?,
                    manifest_hash: r.get(3)?,
                    endpoint: r.get(4)?,
                    descriptor_json: r.get(5)?,
                    manifest_json: r.get(6)?,
                    catalog_revision: r.get(7)?,
                    status: r.get(8)?,
                    created_at: r.get(9)?,
                    active_at: r.get(10)?,
                })
            })
            .map_err(sqlite_err)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(sqlite_err)
    }

    /// The caller owns schema redaction; the store retains its opaque projection.
    pub fn record_capability_invocation(
        &self,
        record: &CapabilityInvocationRecord,
    ) -> std::result::Result<(), String> {
        if record.invocation_id.is_empty()
            || record.operation_id.is_empty()
            || record.capability_version < 1
        {
            return Err("capability invocation identity and version are required".into());
        }
        let conn = self.conn.as_ref().ok_or("state store closed")?;
        let default = |value: &str, fallback: &str| {
            if value.is_empty() {
                fallback.to_string()
            } else {
                value.to_string()
            }
        };
        let created = default(&record.created_at, &crate::hostobs::rfc3339_nano_now());
        conn.execute(
            "INSERT INTO capability_invocations(invocation_id, operation_id, capability_version, catalog_revision, generation_id,
             authorization, arguments_json, binding_json, result_json, observation_json, terminal_status, created_at)
             VALUES (?, ?, ?, ?, NULLIF(?, ''), ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(invocation_id) DO NOTHING",
            rusqlite::params![record.invocation_id, record.operation_id, record.capability_version, record.catalog_revision,
                record.generation_id, default(&record.authorization, "unknown"), default(&record.arguments_json, "{}"),
                default(&record.binding_json, "{}"), default(&record.result_json, "{}"), default(&record.observation_json, "{}"),
                default(&record.terminal_status, "unknown"), created],
        ).map_err(sqlite_err)?;
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

/// `scanOperation`: the columns `List` and `Get` project, as the same JSON
/// shape the MCP surface returns.
#[allow(dead_code)] // M5: used once list_operations/get_operation are wired into tasks.rs
fn scan_operation(row: &rusqlite::Row) -> rusqlite::Result<J> {
    let id: String = row.get(0)?;
    let tool: String = row.get(1)?;
    let status: String = row.get(2)?;
    let description: String = row.get(3)?;
    let created: String = row.get(4)?;
    let updated: String = row.get(5)?;
    let result: Option<String> = row.get(6)?;
    let message: Option<String> = row.get(7)?;
    let mut item = Map::new();
    item.insert("operationId".into(), J::String(id));
    item.insert("toolName".into(), J::String(tool));
    item.insert("status".into(), J::String(status));
    item.insert("description".into(), J::String(description));
    item.insert("createdAt".into(), J::String(created));
    item.insert("updatedAt".into(), J::String(updated));
    if let Some(result) = result.filter(|r| !r.is_empty()) {
        if let Ok(parsed) = serde_json::from_str(&result) {
            item.insert("result".into(), parsed);
        }
    }
    if let Some(message) = message.filter(|m| !m.is_empty()) {
        item.insert("error".into(), J::String(message));
    }
    Ok(J::Object(item))
}

/// `scanPlan`.
#[allow(dead_code)] // M5: used once the plan CRUD surface is wired into tasks.rs
fn scan_plan(row: &rusqlite::Row) -> rusqlite::Result<PlanRecord> {
    Ok(PlanRecord {
        run_id: row.get(0)?,
        plan_id: row.get(1)?,
        generation: row.get(2)?,
        idempotency_key: row.get(3)?,
        document_hash: row.get(4)?,
        catalog_revision: row.get(5)?,
        status: row.get(6)?,
        plan_json: row.get(7)?,
        recipe_json: row.get(8)?,
        state_json: row.get(9)?,
        created_at: row.get(10)?,
        updated_at: row.get(11)?,
        error_message: row.get(12)?,
    })
}

/// `state.PlanRecord`: the durable envelope for a host-plan.v1 run.
/// `plan_json`/`recipe_json`/`state_json` must contain only caller-approved,
/// redacted JSON; secrets must be represented by references before a plan
/// reaches this boundary.
#[allow(dead_code)] // M5: constructed once plan creation is wired into tasks.rs
#[derive(Debug, Clone, Default)]
pub struct PlanRecord {
    pub run_id: String,
    pub plan_id: String,
    pub generation: i64,
    pub idempotency_key: String,
    pub document_hash: String,
    pub catalog_revision: String,
    pub status: String,
    pub plan_json: String,
    pub recipe_json: String,
    pub state_json: String,
    pub created_at: String,
    pub updated_at: String,
    pub error_message: String,
}

/// `state.ActiveCapabilityRecord`: the host-local, product-neutral active
/// selection for a capability.
#[allow(dead_code)] // M5: constructed once active-capability selection is wired into tasks.rs
#[derive(Debug, Clone, Default)]
pub struct ActiveCapabilityRecord {
    pub capability: String,
    pub serving_contract: String,
    pub provider: String,
    pub recipe_id: String,
    pub recipe_version: String,
    pub recipe_hash: String,
    pub run_id: String,
    pub input_bindings_json: String,
    pub observation_json: String,
    pub activated_at: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProviderGenerationRecord {
    pub generation_id: String,
    pub provider_id: String,
    pub provider_version: String,
    pub manifest_hash: String,
    pub endpoint: String,
    pub descriptor_json: String,
    pub manifest_json: String,
    pub catalog_revision: String,
    pub status: String,
    pub created_at: String,
    pub active_at: String,
}

#[derive(Clone, Debug, Default)]
pub struct CapabilityInvocationRecord {
    pub invocation_id: String,
    pub operation_id: String,
    pub capability_version: i64,
    pub catalog_revision: String,
    pub generation_id: String,
    pub authorization: String,
    pub arguments_json: String,
    pub binding_json: String,
    pub result_json: String,
    pub observation_json: String,
    pub terminal_status: String,
    pub created_at: String,
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
    fn operation_lifecycle_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let store = StateStore::open(&dir.path().join("state")).unwrap();
        store
            .create_operation("op-1", "run_host_command", "test")
            .unwrap();
        let fetched = store.get_operation("op-1").unwrap().unwrap();
        assert_eq!(fetched["status"], "working");
        store
            .complete_operation("op-1", &serde_json::json!({"ok": true}))
            .unwrap();
        let fetched = store.get_operation("op-1").unwrap().unwrap();
        assert_eq!(fetched["status"], "completed");
        assert_eq!(fetched["result"], serde_json::json!({"ok": true}));

        store
            .create_operation("op-2", "run_host_command", "test")
            .unwrap();
        store.fail_operation("op-2", "boom").unwrap();
        let fetched = store.get_operation("op-2").unwrap().unwrap();
        assert_eq!(fetched["status"], "failed");
        assert_eq!(fetched["error"], "boom");

        store
            .create_operation("op-3", "run_host_command", "test")
            .unwrap();
        store.cancel_operation("op-3").unwrap();
        assert_eq!(
            store.get_operation("op-3").unwrap().unwrap()["status"],
            "cancelled"
        );
        // Already-terminal operations cannot be cancelled after the fact.
        store.cancel_operation("op-1").unwrap();
        assert_eq!(
            store.get_operation("op-1").unwrap().unwrap()["status"],
            "completed"
        );

        assert!(store.get_operation("missing").unwrap().is_none());
        assert_eq!(store.list_operations(0).unwrap().len(), 3);
    }

    #[test]
    fn task_snapshot_survives_store_restart() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        let store = StateStore::open(&state).unwrap();
        store
            .save_task_snapshot(
                "task-1",
                "run_host_command",
                "running",
                &serde_json::json!({
                    "taskId": "task-1", "toolName": "run_host_command", "status": "completed",
                    "result": {"content": [{"type": "text", "text": "done"}]},
                }),
            )
            .unwrap();
        drop(store);

        let reopened = StateStore::open(&state).unwrap();
        let snapshots = reopened.list_task_snapshots().unwrap();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0]["taskId"], "task-1");
        assert_eq!(snapshots[0]["status"], "completed");
    }

    #[test]
    fn registry_snapshot_preserves_go_named_status_projection() {
        let dir = tempfile::tempdir().unwrap();
        let store = StateStore::open(dir.path()).unwrap();
        for status in [
            "working",
            "input_required",
            "completed",
            "failed",
            "cancelled",
        ] {
            let snapshot = serde_json::json!({"taskId": status, "status": status});
            store
                .save_registry_task_snapshot(status, "test", "", &snapshot)
                .unwrap();
            let conn = store.conn.as_ref().unwrap();
            let (legacy, encoded): (String, String) = conn
                .query_row(
                    "SELECT status, task_snapshot_json FROM operations WHERE operation_id = ?",
                    [status],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap();
            assert_eq!(legacy, "working");
            assert_eq!(serde_json::from_str::<J>(&encoded).unwrap(), snapshot);
        }
        // A JSON-decoded string does pass Go's assertion.
        store
            .save_task_snapshot(
                "decoded",
                "test",
                "",
                &serde_json::json!({"status": "completed"}),
            )
            .unwrap();
        let status: String = store
            .conn
            .as_ref()
            .unwrap()
            .query_row(
                "SELECT status FROM operations WHERE operation_id = 'decoded'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(status, "completed");
    }

    #[test]
    fn provider_generation_replacement_keeps_creation_and_survives_restart() {
        let dir = tempfile::tempdir().unwrap();
        let store = StateStore::open(dir.path()).unwrap();
        assert!(store
            .save_provider_generation(&ProviderGenerationRecord::default())
            .is_err());
        let mut record = ProviderGenerationRecord {
            generation_id: "g1".into(),
            provider_id: "fixture".into(),
            provider_version: "1".into(),
            manifest_hash: "sha256:fixture".into(),
            endpoint: "http://127.0.0.1:1/mcp".into(),
            status: "candidate".into(),
            created_at: "2026-10-04T00:00:00Z".into(),
            ..Default::default()
        };
        store.save_provider_generation(&record).unwrap();
        record.status = "active".into();
        record.created_at = "2026-10-04T01:00:00Z".into();
        record.active_at = record.created_at.clone();
        store.save_provider_generation(&record).unwrap();
        drop(store);
        let reopened = StateStore::open(dir.path()).unwrap();
        let records = reopened.list_provider_generations().unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].created_at, "2026-10-04T00:00:00Z");
        assert_eq!(records[0].active_at, record.active_at);
        assert_eq!(records[0].status, "active");
    }

    #[test]
    fn invocation_replay_preserves_original_and_go_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let store = StateStore::open(dir.path()).unwrap();
        let mut record = CapabilityInvocationRecord {
            invocation_id: "i1".into(),
            operation_id: "op".into(),
            capability_version: 1,
            ..Default::default()
        };
        assert!(store
            .record_capability_invocation(&CapabilityInvocationRecord::default())
            .is_err());
        store.record_capability_invocation(&record).unwrap();
        record.arguments_json = "{\"replacement\":true}".into();
        record.authorization = "admitted".into();
        store.record_capability_invocation(&record).unwrap();
        drop(store);
        let reopened = StateStore::open(dir.path()).unwrap();
        let row: (String, String, String, String, String, String, Option<String>) = reopened.conn.as_ref().unwrap()
            .query_row("SELECT arguments_json, binding_json, result_json, observation_json, authorization, terminal_status, generation_id FROM capability_invocations WHERE invocation_id='i1'", [],
                |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?))).unwrap();
        assert_eq!(
            row,
            (
                "{}".into(),
                "{}".into(),
                "{}".into(),
                "{}".into(),
                "unknown".into(),
                "unknown".into(),
                None
            )
        );
    }

    #[test]
    fn plan_create_is_idempotent_on_conflict() {
        let dir = tempfile::tempdir().unwrap();
        let store = StateStore::open(&dir.path().join("state")).unwrap();
        let record = PlanRecord {
            run_id: "run-1".into(),
            plan_id: "plan-1".into(),
            generation: 0,
            idempotency_key: "key-1".into(),
            document_hash: "sha256:doc".into(),
            catalog_revision: "rev".into(),
            status: "running".into(),
            plan_json: "{}".into(),
            state_json: "{}".into(),
            ..Default::default()
        };
        let (first, created) = store.create_plan(&record).unwrap();
        assert!(created);
        assert_eq!(first.run_id, "run-1");

        let mut retry = record.clone();
        retry.run_id = "run-2".into();
        let (existing, created) = store.create_plan(&retry).unwrap();
        assert!(
            !created,
            "a conflicting identity must not insert a second row"
        );
        assert_eq!(existing.run_id, "run-1");

        store
            .update_plan("run-1", "completed", "{\"done\":true}", "")
            .unwrap();
        let fetched = store.get_plan("run-1").unwrap().unwrap();
        assert_eq!(fetched.status, "completed");
        assert_eq!(fetched.state_json, "{\"done\":true}");
        assert_eq!(fetched.error_message, "");
    }

    #[test]
    fn complete_plan_writes_active_capability_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = StateStore::open(&dir.path().join("state")).unwrap();
        let record = PlanRecord {
            run_id: "run-1".into(),
            plan_id: "plan-1".into(),
            generation: 0,
            idempotency_key: "key-1".into(),
            document_hash: "sha256:doc".into(),
            catalog_revision: "rev".into(),
            status: "running".into(),
            plan_json: "{}".into(),
            state_json: "{}".into(),
            ..Default::default()
        };
        store.create_plan(&record).unwrap();
        store
            .complete_plan_with_active_capability(
                "run-1",
                "{\"done\":true}",
                &ActiveCapabilityRecord {
                    capability: "cap-1".into(),
                    serving_contract: "c.v1".into(),
                    provider: "test".into(),
                    recipe_id: "r".into(),
                    recipe_version: "1.0.0".into(),
                    recipe_hash: "sha256:test".into(),
                    run_id: "run-1".into(),
                    input_bindings_json: "{}".into(),
                    observation_json: "{}".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(
            store.get_plan("run-1").unwrap().unwrap().status,
            "completed"
        );
        let active = store.get_active_capability("cap-1").unwrap().unwrap();
        assert_eq!(active.provider, "test");
        store
            .remove_active_capabilities_for_provider("test")
            .unwrap();
        assert!(store.get_active_capability("cap-1").unwrap().is_none());
    }

    /// A plan run finishes by writing its terminal state while the agent
    /// keeps serving other calls against the same store. Before
    /// `transaction_immediate` this was a deferred transaction that upgrades
    /// from read to write; a concurrent writer was refused outright with
    /// SQLITE_BUSY rather than waiting, turning a run whose work had all
    /// applied into a durable failure. Mirrors Go's
    /// `TestConcurrentWritersWaitRatherThanFailBusy`.
    #[test]
    fn concurrent_writers_wait_rather_than_fail_busy() {
        use std::sync::{Arc, Barrier};
        let dir = tempfile::tempdir().unwrap();
        let state_path = dir.path().join("state");
        // Schema creation (CREATE TABLE/ALTER) is not itself immediate-transaction
        // protected, so run it once up front; each thread then opens its own
        // real connection to the same file. rusqlite::Connection is Send but not
        // Sync (its statement cache uses RefCell), so sharing one Arc<StateStore>
        // across threads wouldn't even compile -- and if it somehow did, a
        // single shared connection would serialize every call at the Rust level
        // and never exercise SQLite's own cross-connection locking, which is
        // exactly what `_txlock=immediate` is about. Separate connections to one
        // file is also what Go's `*sql.DB` pool does under the hood.
        StateStore::open(&state_path).unwrap().close();
        const WRITERS: usize = 8;
        let start = Arc::new(Barrier::new(WRITERS));
        let handles: Vec<_> = (0..WRITERS)
            .map(|i| {
                let state_path = state_path.clone();
                let start = Arc::clone(&start);
                std::thread::spawn(move || {
                    let mut store = StateStore::open(&state_path).unwrap();
                    start.wait();
                    for attempt in 0..4 {
                        let id = format!("op-{i}-{attempt}");
                        store
                            .create_operation(&id, "test_tool", "concurrent write")
                            .unwrap();
                        store
                            .complete_operation(&id, &serde_json::json!({"ok": true}))
                            .unwrap();
                        let run = PlanRecord {
                            run_id: id.clone(),
                            plan_id: format!("plan-{i}"),
                            generation: attempt,
                            idempotency_key: id.clone(),
                            document_hash: "sha256:test".into(),
                            catalog_revision: "rev".into(),
                            status: "running".into(),
                            plan_json: "{}".into(),
                            state_json: "{}".into(),
                            ..Default::default()
                        };
                        store.create_plan(&run).unwrap();
                        store
                            .complete_plan_with_active_capability(
                                &id,
                                "{}",
                                &ActiveCapabilityRecord {
                                    capability: format!("cap-{i}"),
                                    serving_contract: "c.v1".into(),
                                    provider: "test".into(),
                                    recipe_id: "r".into(),
                                    recipe_version: "1.0.0".into(),
                                    recipe_hash: "sha256:test".into(),
                                    run_id: id.clone(),
                                    input_bindings_json: "{}".into(),
                                    observation_json: "{}".into(),
                                    ..Default::default()
                                },
                            )
                            .unwrap();
                    }
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
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

    /// Mirrors Go's `TestOpenMigratesLegacyActiveRuntimeColumns`
    /// (`internal/state/store_test.go`): a pre-column-addition state
    /// directory has only `active_runtimes`'s three original columns, no
    /// `active_capabilities` table at all. `StateStore::open` must add the
    /// missing columns with the same defaults Go uses, then copy the row
    /// into `active_capabilities` (renaming `runtime` to `provider`) rather
    /// than losing the active selection on upgrade.
    #[test]
    fn open_migrates_legacy_active_runtime_columns() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("state.db");
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(
                "CREATE TABLE active_runtimes (
                    capability TEXT PRIMARY KEY,
                    serving_contract TEXT NOT NULL,
                    runtime TEXT NOT NULL
                );
                INSERT INTO active_runtimes(capability, serving_contract, runtime)
                VALUES ('llm-serving', 'openai-chat.v1', 'ollama')",
            )
            .unwrap();
        }
        let mut store = StateStore::open(dir.path()).unwrap();
        let active = store
            .get_active_capability("llm-serving")
            .unwrap()
            .expect("legacy row should have migrated into active_capabilities");
        assert_eq!(active.provider, "ollama");
        assert_eq!(active.serving_contract, "openai-chat.v1");
        store.close();
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
