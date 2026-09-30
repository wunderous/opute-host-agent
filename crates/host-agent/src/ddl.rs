//! SQLite DDL copied verbatim from the pinned Go tree (`internal/state/store.go`
//! and `internal/authz/store.go`). SQLite stores each CREATE statement's text
//! in `sqlite_master`, and the parity harness compares that text, so these
//! strings must stay byte-identical to Go. Regenerate them from the source
//! lock rather than editing by hand.

pub const STATE_INIT: &str = r#"CREATE TABLE IF NOT EXISTS operations (
        operation_id TEXT PRIMARY KEY,
        tool_name TEXT NOT NULL,
        status TEXT NOT NULL,
        description TEXT,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL,
        result_json TEXT,
        error_message TEXT,
        task_snapshot_json TEXT NOT NULL DEFAULT ''
    );
    CREATE TABLE IF NOT EXISTS plan_runs (
        run_id TEXT PRIMARY KEY,
        plan_id TEXT NOT NULL,
        generation INTEGER NOT NULL,
        idempotency_key TEXT NOT NULL,
        document_hash TEXT NOT NULL,
        catalog_revision TEXT NOT NULL,
        status TEXT NOT NULL,
        plan_json TEXT NOT NULL,
        recipe_json TEXT NOT NULL DEFAULT '',
        state_json TEXT NOT NULL,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL,
        error_message TEXT,
        UNIQUE(plan_id, generation, idempotency_key)
    );
    CREATE TABLE IF NOT EXISTS active_runtimes (
        capability TEXT PRIMARY KEY,
        serving_contract TEXT NOT NULL,
        runtime TEXT NOT NULL,
        recipe_id TEXT NOT NULL,
        recipe_version TEXT NOT NULL,
        recipe_hash TEXT NOT NULL,
        run_id TEXT NOT NULL,
        input_bindings_json TEXT NOT NULL,
        observation_json TEXT NOT NULL,
        activated_at TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS provider_generations (
        generation_id TEXT PRIMARY KEY,
        provider_id TEXT NOT NULL,
        provider_version TEXT NOT NULL,
        manifest_hash TEXT NOT NULL,
        endpoint TEXT NOT NULL,
        descriptor_json TEXT NOT NULL DEFAULT '',
        manifest_json TEXT NOT NULL DEFAULT '',
        catalog_revision TEXT NOT NULL,
        status TEXT NOT NULL,
        created_at TEXT NOT NULL,
        active_at TEXT
    );
    CREATE TABLE IF NOT EXISTS capability_invocations (
        invocation_id TEXT PRIMARY KEY,
        operation_id TEXT NOT NULL,
        capability_version INTEGER NOT NULL,
        catalog_revision TEXT NOT NULL,
        generation_id TEXT,
        authorization TEXT NOT NULL,
        arguments_json TEXT NOT NULL,
        result_json TEXT NOT NULL,
        observation_json TEXT NOT NULL,
        terminal_status TEXT NOT NULL,
        created_at TEXT NOT NULL
    );
    UPDATE operations SET status = 'unknown', updated_at = datetime('now') WHERE status = 'working';
    UPDATE plan_runs SET status = 'unknown', updated_at = datetime('now') WHERE status IN ('working', 'running');"#;

pub const STATE_ACTIVE_CAPABILITIES: &str = r#"CREATE TABLE IF NOT EXISTS active_capabilities (
        capability TEXT PRIMARY KEY,
        serving_contract TEXT NOT NULL,
        provider TEXT NOT NULL,
        recipe_id TEXT NOT NULL,
        recipe_version TEXT NOT NULL,
        recipe_hash TEXT NOT NULL,
        run_id TEXT NOT NULL,
        input_bindings_json TEXT NOT NULL,
        observation_json TEXT NOT NULL,
        activated_at TEXT NOT NULL
    )"#;

pub const STATE_COPY_ACTIVE_RUNTIMES: &str = r#"INSERT OR IGNORE INTO active_capabilities(
            capability, serving_contract, provider, recipe_id, recipe_version,
            recipe_hash, run_id, input_bindings_json, observation_json, activated_at
        ) SELECT capability, serving_contract, runtime, recipe_id, recipe_version,
            recipe_hash, run_id, input_bindings_json, observation_json, activated_at
        FROM active_runtimes"#;

pub const STATE_RESOURCE_REGISTRY: &str = r#"CREATE TABLE IF NOT EXISTS resource_registry (
        uri TEXT PRIMARY KEY,
        resource_type TEXT NOT NULL,
        tenant_id TEXT NOT NULL,
        resource_id TEXT NOT NULL,
        coordinates_json TEXT NOT NULL,
        status TEXT NOT NULL DEFAULT 'active',
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL
    )"#;

pub const AUTHZ_INIT: &str = r#"
		CREATE TABLE IF NOT EXISTS clients (
			client_id TEXT PRIMARY KEY,
			secret_hash TEXT NOT NULL DEFAULT '',
			client_type TEXT NOT NULL,
			redirect_uris TEXT NOT NULL DEFAULT '[]',
			metadata_url TEXT NOT NULL DEFAULT '',
			confidential INTEGER NOT NULL DEFAULT 0,
			created_at INTEGER NOT NULL
		);
		CREATE TABLE IF NOT EXISTS codes (
			code TEXT PRIMARY KEY,
			client_id TEXT NOT NULL,
			resource TEXT NOT NULL,
			code_challenge TEXT NOT NULL,
			redirect_uri TEXT NOT NULL,
			expires_at INTEGER NOT NULL,
			used INTEGER NOT NULL DEFAULT 0
		);
		CREATE TABLE IF NOT EXISTS tokens (
			token_hash TEXT PRIMARY KEY,
			client_id TEXT NOT NULL,
			resource TEXT NOT NULL,
			scope TEXT NOT NULL,
			expires_at INTEGER NOT NULL,
			revoked INTEGER NOT NULL DEFAULT 0,
			created_at INTEGER NOT NULL
		);
	"#;
