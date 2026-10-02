//! OAuth token issuance (OpenSpec capability `oauth-issuance`, decision D8).
//!
//! This is the one deliberate divergence from the Go baseline: every token
//! the Rust agent issues is backed by a credential it trusts, or by the host
//! operator's approval given through a channel the requester cannot reach.
//!
//! ```text
//!  POST /oauth/token
//!    client_credentials  -> confidential client + registered secret
//!                           -> resource must be one this agent serves
//!    authorization_code  -> single-use code from an *approved* request
//!                           -> client, redirect, PKCE S256, resource bound
//!  GET|POST /oauth/authorize
//!    validate client, exact redirect, PKCE, resource
//!    remembered consent?  yes -> code now
//!                         no  -> pending request + approval page (no button)
//!  GET /oauth/authorize/status?request=ID
//!    pending -> page again | approved -> 302 code | denied -> access_denied
//!  opute-host-agent oauth approve|deny <user code>   (state directory = trust)
//! ```
//!
//! Secrets are 256-bit random values. The store keeps SHA-256 hashes only;
//! plaintext lives in 0600 files under `<state>/credentials/`.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value as J};
use sha2::{Digest, Sha256};

pub const PROVIDER_CLIENT_ID: &str = "host-agent-provider";
pub const OPUTE_CLIENT_ID: &str = "opute-mcp-host";
pub const BOOTSTRAP_CLIENT_ID: &str = "host-agent-bootstrap";
const MIGRATION_ID: &str = "oauth-issuance.v1";
pub const ACCESS_TOKEN_TTL: i64 = 3600;
const CODE_TTL: i64 = 600;
const PENDING_TTL: i64 = 600;
const MAX_PENDING_TOTAL: i64 = 20;
const MAX_PENDING_PER_CLIENT: i64 = 3;
const DEFAULT_CONSENT_DAYS: i64 = 30;
/// User codes avoid characters that are easy to misread (0/O, 1/I/L, vowels).
const USER_CODE_ALPHABET: &[u8] = b"BCDFGHJKMNPQRSTVWXZ23456789";

pub const ISSUANCE_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS pending_authorizations (
  id TEXT PRIMARY KEY,
  user_code TEXT NOT NULL UNIQUE,
  client_id TEXT NOT NULL,
  redirect_uri TEXT NOT NULL,
  resource TEXT NOT NULL,
  code_challenge TEXT NOT NULL,
  state TEXT NOT NULL DEFAULT '',
  status TEXT NOT NULL,
  decided_by TEXT NOT NULL DEFAULT '',
  created_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS consents (
  client_id TEXT NOT NULL,
  resource TEXT NOT NULL,
  granted_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL,
  PRIMARY KEY (client_id, resource)
);
CREATE TABLE IF NOT EXISTS schema_migrations (
  id TEXT PRIMARY KEY,
  applied_at INTEGER NOT NULL
);
"#;

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

pub fn sha256_hex(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

/// Constant-time equality of two hex digests.
pub fn digest_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    getrandom::getrandom(&mut buf).expect("the OS random source is available");
    hex::encode(buf)
}

fn user_code() -> String {
    let mut buf = [0u8; 8];
    getrandom::getrandom(&mut buf).expect("the OS random source is available");
    let chars: String = buf
        .iter()
        .map(|b| USER_CODE_ALPHABET[*b as usize % USER_CODE_ALPHABET.len()] as char)
        .collect();
    format!("{}-{}", &chars[..4], &chars[4..])
}

pub fn new_secret() -> String {
    format!("ohs_{}", random_hex(32))
}

pub fn new_access_token() -> String {
    format!("oha_{}", random_hex(32))
}

/// S256 PKCE transform: BASE64URL(SHA256(verifier)) without padding.
pub fn s256(verifier: &str) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

// --- audit ----------------------------------------------------------------------

/// One structured line per issuance-relevant event. Callers pass only
/// identifiers and outcomes; secrets, tokens, codes and verifiers never reach
/// this function.
pub fn audit(event: &str, attrs: &[(&str, &str)]) {
    let mut all: Vec<(&str, &str)> = vec![("event", event)];
    all.extend_from_slice(attrs);
    crate::app::log_info(&mut std::io::stderr(), "oauth", &all);
}

// --- credentials ------------------------------------------------------------------

#[derive(Debug, PartialEq)]
pub struct CredentialFile {
    pub client_id: String,
    pub client_secret: String,
}

pub fn credentials_dir(state_dir: &Path) -> PathBuf {
    state_dir.join("credentials")
}

pub fn credential_path(state_dir: &Path, client_id: &str) -> PathBuf {
    credentials_dir(state_dir).join(format!("{client_id}.json"))
}

pub fn read_credential(path: &Path) -> Option<CredentialFile> {
    let text = std::fs::read_to_string(path).ok()?;
    let v: J = serde_json::from_str(&text).ok()?;
    Some(CredentialFile {
        client_id: v.get("client_id")?.as_str()?.to_string(),
        client_secret: v.get("client_secret")?.as_str()?.to_string(),
    })
}

/// Atomic 0600 write inside a 0700 directory.
fn write_credential(state_dir: &Path, client_id: &str, secret: &str) -> std::io::Result<()> {
    let dir = credentials_dir(state_dir);
    std::fs::create_dir_all(&dir)?;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    let doc = json!({
        "client_id": client_id,
        "client_secret": secret,
        "token_endpoint_auth_method": "client_secret_basic",
        "rotated_at": now(),
    });
    let final_path = credential_path(state_dir, client_id);
    let tmp = dir.join(format!(".{client_id}.json.{}", random_hex(4)));
    {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(serde_json::to_string_pretty(&doc)?.as_bytes())?;
        f.write_all(b"\n")?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, &final_path)?;
    Ok(())
}

// --- store preparation --------------------------------------------------------------

fn client_secret_hash(conn: &Connection, client_id: &str) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "SELECT secret_hash FROM clients WHERE client_id=?",
        [client_id],
        |r| r.get::<_, String>(0),
    )
    .optional()
}

fn set_client(
    conn: &Connection,
    client_id: &str,
    secret_hash: &str,
    kind: &str,
    confidential: bool,
    redirects: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO clients(client_id, secret_hash, client_type, redirect_uris, metadata_url, confidential, created_at)
         VALUES(?,?,?,?,?,?,?)
         ON CONFLICT(client_id) DO UPDATE SET secret_hash=excluded.secret_hash, client_type=excluded.client_type,
           redirect_uris=excluded.redirect_uris, confidential=excluded.confidential",
        params![client_id, secret_hash, kind, redirects, "", confidential as i64, now()],
    )?;
    Ok(())
}

/// Revoke every token issued to `client_id`.
pub fn revoke_client_tokens(conn: &Connection, client_id: &str) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE tokens SET revoked=1 WHERE client_id=? AND revoked=0",
        [client_id],
    )
}

/// Make sure a provisioned confidential client has a secret whose plaintext
/// is in its credential file. A missing or mismatched file is a lost secret:
/// a new one is written and the client's tokens are revoked.
fn provision(conn: &Connection, state_dir: &Path, client_id: &str) -> Result<(), String> {
    let stored = client_secret_hash(conn, client_id)
        .map_err(|e| e.to_string())?
        .unwrap_or_default();
    let file = read_credential(&credential_path(state_dir, client_id));
    let file_ok = file.as_ref().is_some_and(|f| {
        f.client_id == client_id
            && !stored.is_empty()
            && digest_eq(&sha256_hex(&f.client_secret), &stored)
    });
    if file_ok {
        return Ok(());
    }
    rotate_secret(
        conn,
        state_dir,
        client_id,
        if stored.is_empty() {
            "provision"
        } else {
            "rotate-secret"
        },
    )
}

/// Redirect URIs Go registers for a built-in confidential client.
fn builtin_redirects(client_id: &str) -> &'static str {
    if client_id == OPUTE_CLIENT_ID {
        r#"["https://127.0.0.1/oauth/callback"]"#
    } else {
        "[]"
    }
}

/// Write a fresh secret for a provisioned client and revoke its tokens.
pub fn rotate_secret(
    conn: &Connection,
    state_dir: &Path,
    client_id: &str,
    event: &str,
) -> Result<(), String> {
    let secret = new_secret();
    write_credential(state_dir, client_id, &secret)
        .map_err(|e| format!("write credential for {client_id}: {e}"))?;
    set_client(
        conn,
        client_id,
        &sha256_hex(&secret),
        "confidential",
        true,
        builtin_redirects(client_id),
    )
    .map_err(|e| e.to_string())?;
    let revoked = revoke_client_tokens(conn, client_id).map_err(|e| e.to_string())?;
    audit(
        event,
        &[
            ("client_id", client_id),
            ("revoked_tokens", &revoked.to_string()),
        ],
    );
    Ok(())
}

/// Bring an `authz.sqlite` opened with the Go-compatible schema up to the
/// issuance contract: tables, client types, provisioned secrets, and the
/// one-time revocation of tokens written before this contract.
pub fn prepare(conn: &Connection, state_dir: &Path, opute_secret: &str) -> Result<(), String> {
    conn.execute_batch(ISSUANCE_DDL)
        .map_err(|e| e.to_string())?;
    let migrated: bool = conn
        .query_row(
            "SELECT 1 FROM schema_migrations WHERE id=?",
            [MIGRATION_ID],
            |_| Ok(()),
        )
        .optional()
        .map_err(|e| e.to_string())?
        .is_some();
    if !migrated {
        let revoked = conn
            .execute("UPDATE tokens SET revoked=1 WHERE revoked=0", [])
            .map_err(|e| e.to_string())?;
        conn.execute(
            "INSERT INTO schema_migrations(id, applied_at) VALUES(?, ?)",
            params![MIGRATION_ID, now()],
        )
        .map_err(|e| e.to_string())?;
        audit(
            "migrate",
            &[
                ("migration", MIGRATION_ID),
                ("revoked_tokens", &revoked.to_string()),
            ],
        );
    }
    // The bootstrap client has no secret, so it is public: it may only use
    // operator-approved authorization codes.
    set_client(
        conn,
        BOOTSTRAP_CLIENT_ID,
        "",
        "public",
        false,
        r#"["http://127.0.0.1/callback","http://localhost/callback"]"#,
    )
    .map_err(|e| e.to_string())?;
    let opute_secret = opute_secret.trim();
    if opute_secret.is_empty() {
        provision(conn, state_dir, OPUTE_CLIENT_ID)?;
    } else {
        let hash = sha256_hex(opute_secret);
        let stored = client_secret_hash(conn, OPUTE_CLIENT_ID)
            .map_err(|e| e.to_string())?
            .unwrap_or_default();
        // A generated secret is superseded by the operator's; drop its file.
        let _ = std::fs::remove_file(credential_path(state_dir, OPUTE_CLIENT_ID));
        if stored != hash {
            set_client(
                conn,
                OPUTE_CLIENT_ID,
                &hash,
                "confidential",
                true,
                builtin_redirects(OPUTE_CLIENT_ID),
            )
            .map_err(|e| e.to_string())?;
            revoke_client_tokens(conn, OPUTE_CLIENT_ID).map_err(|e| e.to_string())?;
        }
    }
    provision(conn, state_dir, PROVIDER_CLIENT_ID)?;
    Ok(())
}

// --- failure backoff --------------------------------------------------------------------

/// Exponential backoff after repeated failures, keyed by client and by
/// remote address. Successful grants are never slowed down.
/// (failures in window, window start, blocked until)
type BackoffEntry = (u32, Instant, Option<Instant>);

#[derive(Default)]
pub struct Backoff {
    entries: Mutex<HashMap<String, BackoffEntry>>,
}

impl Backoff {
    const WINDOW: Duration = Duration::from_secs(60);
    const THRESHOLD: u32 = 5;

    pub fn blocked(&self, keys: &[String]) -> bool {
        let now = Instant::now();
        let map = self.entries.lock().expect("backoff lock");
        keys.iter().any(|k| {
            map.get(k)
                .and_then(|e| e.2)
                .is_some_and(|until| until > now)
        })
    }

    pub fn fail(&self, keys: &[String]) {
        let now = Instant::now();
        let mut map = self.entries.lock().expect("backoff lock");
        for k in keys {
            let entry = map.entry(k.clone()).or_insert((0, now, None));
            if now.duration_since(entry.1) > Self::WINDOW && entry.2.is_none_or(|u| u <= now) {
                *entry = (0, now, None);
            }
            entry.0 += 1;
            if entry.0 >= Self::THRESHOLD {
                let exp = (entry.0 - Self::THRESHOLD).min(8);
                entry.2 = Some(now + Duration::from_secs(1 << exp));
            }
        }
    }
}

// --- tokens -------------------------------------------------------------------------------

pub fn insert_token(
    conn: &Connection,
    client_id: &str,
    resource: &str,
    ttl: i64,
) -> rusqlite::Result<String> {
    let token = new_access_token();
    let issued = now();
    conn.execute(
        "INSERT INTO tokens(token_hash, client_id, resource, scope, expires_at, revoked, created_at) VALUES(?,?,?,?,?,0,?)",
        params![sha256_hex(&token), client_id, resource, "mcp", issued + ttl, issued],
    )?;
    // Collect rows expired for more than an hour (as Go does).
    let _ = conn.execute("DELETE FROM tokens WHERE expires_at < ?", [issued - 3600]);
    Ok(token)
}

pub struct ClientRecord {
    pub client_id: String,
    pub secret_hash: String,
    pub client_type: String,
    pub redirect_uris: Vec<String>,
    pub confidential: bool,
}

pub fn client(conn: &Connection, client_id: &str) -> rusqlite::Result<Option<ClientRecord>> {
    conn.query_row(
        "SELECT client_id, secret_hash, client_type, redirect_uris, confidential FROM clients WHERE client_id=?",
        [client_id],
        |r| {
            let raw: String = r.get(3)?;
            Ok(ClientRecord {
                client_id: r.get(0)?,
                secret_hash: r.get(1)?,
                client_type: r.get(2)?,
                redirect_uris: serde_json::from_str(&raw).unwrap_or_default(),
                confidential: r.get::<_, i64>(4)? == 1,
            })
        },
    )
    .optional()
}

/// The outcome of a token request, before HTTP encoding.
#[derive(Debug, PartialEq)]
pub enum Grant {
    Issued {
        token: String,
        resource: String,
    },
    Error {
        status: u16,
        code: &'static str,
        description: String,
    },
}

fn grant_error(status: u16, code: &'static str, description: impl Into<String>) -> Grant {
    Grant::Error {
        status,
        code,
        description: description.into(),
    }
}

/// `client_credentials`: a confidential client authenticating with its
/// registered secret, for a resource this agent serves.
pub fn client_credentials(
    conn: &Connection,
    client_id: &str,
    client_secret: &str,
    resource: &str,
    served: &dyn Fn(&str) -> bool,
) -> Grant {
    if client_id.is_empty() {
        return grant_error(401, "invalid_client", "client authentication is required");
    }
    let Ok(record) = client(conn, client_id) else {
        return grant_error(500, "server_error", "client lookup failed");
    };
    let Some(record) = record else {
        return grant_error(401, "invalid_client", "client authentication failed");
    };
    if !record.confidential || record.client_type == "public" {
        return grant_error(
            400,
            "unauthorized_client",
            "this client may not use client_credentials",
        );
    }
    // A confidential client without a registered secret cannot authenticate.
    if record.secret_hash.is_empty()
        || client_secret.is_empty()
        || !digest_eq(&sha256_hex(client_secret), &record.secret_hash)
    {
        return grant_error(401, "invalid_client", "client authentication failed");
    }
    if resource.is_empty() {
        return grant_error(400, "invalid_request", "resource is required");
    }
    if !served(resource) {
        return grant_error(
            400,
            "invalid_target",
            "resource is not served by this Host Agent",
        );
    }
    match insert_token(conn, &record.client_id, resource, ACCESS_TOKEN_TTL) {
        Ok(token) => Grant::Issued {
            token,
            resource: resource.to_string(),
        },
        Err(_) => grant_error(500, "server_error", "token persist failed"),
    }
}

/// `authorization_code`: redeem a single-use code from an approved request.
pub fn redeem_code(
    conn: &Connection,
    code: &str,
    client_id: &str,
    redirect_uri: &str,
    verifier: &str,
    resource: &str,
) -> Grant {
    if code.is_empty() || client_id.is_empty() || redirect_uri.is_empty() || verifier.is_empty() {
        return grant_error(
            400,
            "invalid_request",
            "code, client_id, redirect_uri and code_verifier are required",
        );
    }
    let hash = sha256_hex(code);
    let row = conn
        .query_row(
            "SELECT client_id, resource, code_challenge, redirect_uri, expires_at, used FROM codes WHERE code=?",
            [&hash],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, i64>(4)?,
                    r.get::<_, i64>(5)?,
                ))
            },
        )
        .optional();
    let Ok(Some((c_client, c_resource, challenge, c_redirect, expires_at, used))) = row else {
        return grant_error(400, "invalid_grant", "authorization code is invalid");
    };
    // Mark used before any other check so a code can never be tried twice.
    let marked = conn
        .execute("UPDATE codes SET used=1 WHERE code=? AND used=0", [&hash])
        .unwrap_or(0);
    if used != 0 || marked != 1 || now() > expires_at {
        return grant_error(
            400,
            "invalid_grant",
            "authorization code is expired or used",
        );
    }
    if c_client != client_id {
        return grant_error(400, "invalid_grant", "client mismatch");
    }
    if c_redirect != redirect_uri {
        return grant_error(400, "invalid_grant", "redirect_uri mismatch");
    }
    if !resource.is_empty() && resource != c_resource {
        return grant_error(400, "invalid_target", "resource mismatch");
    }
    if !digest_eq(&s256(verifier), &challenge) {
        return grant_error(400, "invalid_grant", "pkce verification failed");
    }
    match insert_token(conn, &c_client, &c_resource, ACCESS_TOKEN_TTL) {
        Ok(token) => Grant::Issued {
            token,
            resource: c_resource,
        },
        Err(_) => grant_error(500, "server_error", "token persist failed"),
    }
}

// --- authorization requests ------------------------------------------------------------

/// RFC 8252 §7.3: loopback redirects match on scheme, host and path with any
/// port; every other redirect must match a registered value exactly.
pub fn redirect_allowed(registered: &[String], requested: &str) -> bool {
    if registered.iter().any(|r| r == requested) {
        return true;
    }
    let Some(req) = loopback_parts(requested) else {
        return false;
    };
    registered
        .iter()
        .filter_map(|r| loopback_parts(r))
        .any(|reg| reg.0 == req.0 && reg.2 == req.2)
}

/// (host, port, path-and-query) of an `http://` loopback URI.
fn loopback_parts(uri: &str) -> Option<(String, String, String)> {
    let rest = uri.strip_prefix("http://")?;
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    if authority.contains('@') {
        return None;
    }
    let (host, port) = if let Some(v6) = authority.strip_prefix('[') {
        let end = v6.find(']')?;
        let port = v6[end + 1..].strip_prefix(':').unwrap_or("");
        (format!("[{}]", &v6[..end]), port.to_string())
    } else {
        match authority.rsplit_once(':') {
            Some((h, p)) => (h.to_string(), p.to_string()),
            None => (authority.to_string(), String::new()),
        }
    };
    if !matches!(host.as_str(), "127.0.0.1" | "[::1]" | "localhost")
        || !port.bytes().all(|c| c.is_ascii_digit())
    {
        return None;
    }
    Some((host, port, path.to_string()))
}

pub struct AuthorizeRequest {
    pub client_id: String,
    pub redirect_uri: String,
    pub resource: String,
    pub code_challenge: String,
    pub state: String,
}

#[derive(Debug, PartialEq)]
pub enum AuthorizeOutcome {
    /// Remembered consent: redirect with this code now.
    Code(String),
    /// Waiting for the operator.
    Pending {
        id: String,
        user_code: String,
    },
    Error(&'static str, String),
}

fn issue_code(conn: &Connection, req: &AuthorizeRequest) -> rusqlite::Result<String> {
    let code = random_hex(32);
    conn.execute(
        "INSERT INTO codes(code, client_id, resource, code_challenge, redirect_uri, expires_at, used) VALUES(?,?,?,?,?,?,0)",
        params![sha256_hex(&code), req.client_id, req.resource, req.code_challenge, req.redirect_uri, now() + CODE_TTL],
    )?;
    Ok(code)
}

fn expire_pending(conn: &Connection) {
    let _ = conn.execute(
        "UPDATE pending_authorizations SET status='expired' WHERE status='pending' AND expires_at < ?",
        [now()],
    );
}

/// Record a validated authorization request, or issue a code at once when the
/// operator remembered consent for this client and resource.
pub fn begin_authorization(conn: &Connection, req: &AuthorizeRequest) -> AuthorizeOutcome {
    expire_pending(conn);
    let consented = conn
        .query_row(
            "SELECT 1 FROM consents WHERE client_id=? AND resource=? AND expires_at > ?",
            params![req.client_id, req.resource, now()],
            |_| Ok(()),
        )
        .optional()
        .ok()
        .flatten()
        .is_some();
    if consented {
        audit(
            "approve",
            &[
                ("client_id", &req.client_id),
                ("resource", &req.resource),
                ("decided_by", "consent"),
            ],
        );
        return match issue_code(conn, req) {
            Ok(code) => AuthorizeOutcome::Code(code),
            Err(_) => AuthorizeOutcome::Error("server_error", "code persist failed".into()),
        };
    }
    let count = |sql: &str, p: &[&dyn rusqlite::ToSql]| -> i64 {
        conn.query_row(sql, p, |r| r.get(0)).unwrap_or(i64::MAX)
    };
    let total = count(
        "SELECT COUNT(*) FROM pending_authorizations WHERE status='pending'",
        &[],
    );
    let mine = count(
        "SELECT COUNT(*) FROM pending_authorizations WHERE status='pending' AND client_id=?",
        &[&req.client_id],
    );
    if total >= MAX_PENDING_TOTAL || mine >= MAX_PENDING_PER_CLIENT {
        return AuthorizeOutcome::Error(
            "temporarily_unavailable",
            "too many pending authorization requests".into(),
        );
    }
    let id = random_hex(16);
    for _ in 0..5 {
        let code = user_code();
        let inserted = conn.execute(
            "INSERT INTO pending_authorizations(id, user_code, client_id, redirect_uri, resource, code_challenge, state, status, created_at, expires_at)
             VALUES(?,?,?,?,?,?,?,'pending',?,?)",
            params![id, code, req.client_id, req.redirect_uri, req.resource, req.code_challenge, req.state, now(), now() + PENDING_TTL],
        );
        if inserted.is_ok() {
            audit(
                "pending",
                &[
                    ("client_id", &req.client_id),
                    ("resource", &req.resource),
                    ("user_code", &code),
                ],
            );
            return AuthorizeOutcome::Pending {
                id,
                user_code: code,
            };
        }
    }
    AuthorizeOutcome::Error("server_error", "could not allocate a user code".into())
}

pub struct PendingRecord {
    pub id: String,
    pub user_code: String,
    pub client_id: String,
    pub redirect_uri: String,
    pub resource: String,
    pub code_challenge: String,
    pub state: String,
    pub status: String,
    pub expires_at: i64,
}

const PENDING_COLUMNS: &str =
    "id, user_code, client_id, redirect_uri, resource, code_challenge, state, status, expires_at";

fn pending_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<PendingRecord> {
    Ok(PendingRecord {
        id: r.get(0)?,
        user_code: r.get(1)?,
        client_id: r.get(2)?,
        redirect_uri: r.get(3)?,
        resource: r.get(4)?,
        code_challenge: r.get(5)?,
        state: r.get(6)?,
        status: r.get(7)?,
        expires_at: r.get(8)?,
    })
}

pub fn pending_by_id(conn: &Connection, id: &str) -> Option<PendingRecord> {
    expire_pending(conn);
    conn.query_row(
        &format!("SELECT {PENDING_COLUMNS} FROM pending_authorizations WHERE id=?"),
        [id],
        pending_row,
    )
    .optional()
    .ok()
    .flatten()
}

pub fn pending_list(conn: &Connection) -> Vec<PendingRecord> {
    expire_pending(conn);
    let Ok(mut stmt) = conn.prepare(&format!(
        "SELECT {PENDING_COLUMNS} FROM pending_authorizations WHERE status='pending' ORDER BY created_at"
    )) else {
        return Vec::new();
    };
    stmt.query_map([], pending_row)
        .map(|rows| rows.filter_map(Result::ok).collect())
        .unwrap_or_default()
}

#[derive(Debug, PartialEq)]
pub enum StatusOutcome {
    Pending,
    /// Approved: redirect to this URI (code, iss, state already applied by caller).
    Approved {
        code: String,
        redirect_uri: String,
        state: String,
    },
    Denied {
        redirect_uri: String,
        state: String,
    },
    Expired,
    Unknown,
}

/// Poll a pending request. An approved request yields its code exactly once.
pub fn poll(conn: &Connection, id: &str) -> StatusOutcome {
    let Some(p) = pending_by_id(conn, id) else {
        return StatusOutcome::Unknown;
    };
    match p.status.as_str() {
        "pending" => StatusOutcome::Pending,
        "approved" => {
            let claimed = conn
                .execute(
                    "UPDATE pending_authorizations SET status='redeemed' WHERE id=? AND status='approved'",
                    [id],
                )
                .unwrap_or(0);
            if claimed != 1 {
                return StatusOutcome::Expired;
            }
            let req = AuthorizeRequest {
                client_id: p.client_id,
                redirect_uri: p.redirect_uri.clone(),
                resource: p.resource,
                code_challenge: p.code_challenge,
                state: p.state.clone(),
            };
            match issue_code(conn, &req) {
                Ok(code) => StatusOutcome::Approved {
                    code,
                    redirect_uri: p.redirect_uri,
                    state: p.state,
                },
                Err(_) => StatusOutcome::Expired,
            }
        }
        "denied" => StatusOutcome::Denied {
            redirect_uri: p.redirect_uri,
            state: p.state,
        },
        _ => StatusOutcome::Expired,
    }
}

/// Operator decision on a pending request, by user code.
pub fn decide(
    conn: &Connection,
    user_code: &str,
    approve: bool,
    remember_days: i64,
    decided_by: &str,
) -> Result<PendingRecord, String> {
    expire_pending(conn);
    let code = user_code.trim().to_ascii_uppercase();
    let record = conn
        .query_row(
            &format!("SELECT {PENDING_COLUMNS} FROM pending_authorizations WHERE user_code=?"),
            [&code],
            pending_row,
        )
        .optional()
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("no authorization request with code {code}"))?;
    if record.status != "pending" {
        return Err(format!("authorization request {code} is {}", record.status));
    }
    let status = if approve { "approved" } else { "denied" };
    let changed = conn
        .execute(
            "UPDATE pending_authorizations SET status=?, decided_by=? WHERE id=? AND status='pending'",
            params![status, decided_by, record.id],
        )
        .map_err(|e| e.to_string())?;
    if changed != 1 {
        return Err(format!(
            "authorization request {code} was decided concurrently"
        ));
    }
    if approve && remember_days > 0 {
        conn.execute(
            "INSERT INTO consents(client_id, resource, granted_at, expires_at) VALUES(?,?,?,?)
             ON CONFLICT(client_id, resource) DO UPDATE SET granted_at=excluded.granted_at, expires_at=excluded.expires_at",
            params![record.client_id, record.resource, now(), now() + remember_days * 86_400],
        )
        .map_err(|e| e.to_string())?;
    }
    audit(
        if approve { "approve" } else { "deny" },
        &[
            ("client_id", &record.client_id),
            ("resource", &record.resource),
            ("decided_by", decided_by),
        ],
    );
    Ok(record)
}

/// Remove a remembered consent and revoke that client's tokens for the resource.
pub fn revoke_consent(conn: &Connection, client_id: &str, resource: &str) -> Result<usize, String> {
    conn.execute(
        "DELETE FROM consents WHERE client_id=? AND resource=?",
        params![client_id, resource],
    )
    .map_err(|e| e.to_string())?;
    let revoked = conn
        .execute(
            "UPDATE tokens SET revoked=1 WHERE client_id=? AND resource=? AND revoked=0",
            params![client_id, resource],
        )
        .map_err(|e| e.to_string())?;
    audit(
        "revoke-consent",
        &[
            ("client_id", client_id),
            ("resource", resource),
            ("revoked_tokens", &revoked.to_string()),
        ],
    );
    Ok(revoked)
}

pub fn consent_list(conn: &Connection) -> Vec<(String, String, i64)> {
    let Ok(mut stmt) = conn.prepare("SELECT client_id, resource, expires_at FROM consents WHERE expires_at > ? ORDER BY client_id, resource") else {
        return Vec::new();
    };
    stmt.query_map([now()], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .map(|rows| rows.filter_map(Result::ok).collect())
        .unwrap_or_default()
}

pub fn consent_days(env: &crate::config::Env) -> i64 {
    match env.value("OPUTE_OAUTH_CONSENT_DAYS").trim() {
        "" => DEFAULT_CONSENT_DAYS,
        v => v
            .parse::<i64>()
            .ok()
            .filter(|d| *d >= 0)
            .unwrap_or(DEFAULT_CONSENT_DAYS),
    }
}

// --- approval page -------------------------------------------------------------------------

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// The page shown while a request waits. It deliberately has no approve
/// control: approval only arrives through the operator's own channel.
pub fn approval_page(p: &PendingRecord, status_path: &str, message: &str) -> String {
    let redirect_host = p
        .redirect_uri
        .split("://")
        .nth(1)
        .map(|r| r.split('/').next().unwrap_or(r))
        .unwrap_or(&p.redirect_uri);
    let refresh = if message.is_empty() {
        format!(
            "<meta http-equiv=\"refresh\" content=\"3;url={}\">",
            html_escape(status_path)
        )
    } else {
        String::new()
    };
    let body = if message.is_empty() {
        format!(
            "<p>An application is asking for access to this Host Agent.</p>\
             <dl><dt>Application</dt><dd><code>{client}</code></dd>\
             <dt>Returns to</dt><dd><code>{host}</code></dd>\
             <dt>Resource</dt><dd><code>{resource}</code></dd></dl>\
             <p>Your code: <strong style=\"font-size:1.6em;letter-spacing:.1em\">{code}</strong></p>\
             <p>To allow it, run this on the host:</p>\
             <pre>opute-host-agent oauth approve {code}</pre>\
             <p>Only approve if you started this request and the code matches. This page updates by itself.</p>",
            client = html_escape(&p.client_id),
            host = html_escape(redirect_host),
            resource = html_escape(&p.resource),
            code = html_escape(&p.user_code),
        )
    } else {
        format!("<p>{}</p>", html_escape(message))
    };
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">{refresh}\
         <title>Authorize access</title></head><body style=\"font-family:sans-serif;max-width:36em;margin:2em auto;padding:0 1em\">\
         <h1>Authorize access</h1>{body}</body></html>"
    )
}

/// Security headers for every approval-flow page.
pub fn page_headers() -> [(&'static str, &'static str); 5] {
    [
        ("Cache-Control", "no-store"),
        ("X-Frame-Options", "DENY"),
        (
            "Content-Security-Policy",
            "default-src 'none'; style-src 'unsafe-inline'; frame-ancestors 'none'",
        ),
        ("Referrer-Policy", "no-referrer"),
        ("Content-Type", "text/html; charset=utf-8"),
    ]
}

// --- client ID metadata documents ------------------------------------------------------------

fn ip_forbidden(ip: &std::net::IpAddr) -> bool {
    use std::net::IpAddr;
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_multicast()
                || v4.is_broadcast()
                || v4.octets()[0] == 100 && (64..128).contains(&v4.octets()[1]) // CGNAT
                || *v4 == std::net::Ipv4Addr::new(169, 254, 169, 254)
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return ip_forbidden(&IpAddr::V4(v4));
            }
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (v6.segments()[0] & 0xfe00) == 0xfc00 // unique local
                || (v6.segments()[0] & 0xffc0) == 0xfe80 // link local
        }
    }
}

/// Resolve and vet a metadata host. Every address must be public; the first
/// is the one the fetch will connect to.
pub fn vet_host(host: &str, port: u16) -> Result<std::net::SocketAddr, String> {
    use std::net::ToSocketAddrs;
    let lower = host.to_ascii_lowercase();
    if lower == "localhost" || lower.ends_with(".localhost") {
        return Err("client metadata host is not allowed".into());
    }
    let addrs: Vec<std::net::SocketAddr> = (host.trim_matches(|c| c == '[' || c == ']'), port)
        .to_socket_addrs()
        .map_err(|_| "client metadata host is not resolvable".to_string())?
        .collect();
    if addrs.is_empty() {
        return Err("client metadata host is not resolvable".into());
    }
    if addrs.iter().any(|a| ip_forbidden(&a.ip())) {
        return Err("client metadata host is not allowed".into());
    }
    Ok(addrs[0])
}

/// A validated client ID metadata document.
pub struct ClientMetadata {
    pub client_id: String,
    pub redirect_uris: Vec<String>,
}

/// Fetch a CIMD document over HTTPS, connecting only to the vetted address,
/// following no redirects, bounded in size and time.
pub fn fetch_client_metadata(client_id: &str) -> Result<ClientMetadata, String> {
    let rest = client_id
        .strip_prefix("https://")
        .ok_or_else(|| "client_id must be an https metadata URL".to_string())?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() || authority.contains('@') {
        return Err("client_id must be an https metadata URL".into());
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) if !h.ends_with(']') || authority.starts_with('[') => match p.parse::<u16>() {
            Ok(port) if !h.contains(':') || h.starts_with('[') => (h.to_string(), port),
            _ => (authority.to_string(), 443),
        },
        _ => (authority.to_string(), 443),
    };
    let pinned = vet_host(&host, port)?;
    let agent = ureq::AgentBuilder::new()
        .redirects(0)
        .timeout(Duration::from_secs(5))
        .resolver(move |_: &str| Ok(vec![pinned]))
        .build();
    let response = agent
        .get(client_id)
        .call()
        .map_err(|_| "client metadata fetch failed".to_string())?;
    if response.status() != 200 {
        return Err("client metadata fetch failed".into());
    }
    let mut body = Vec::new();
    response
        .into_reader()
        .take(1 << 20)
        .read_to_end(&mut body)
        .map_err(|_| "client metadata fetch failed".to_string())?;
    validate_client_metadata(client_id, &body)
}

/// Validate a fetched CIMD document.
pub fn validate_client_metadata(client_id: &str, body: &[u8]) -> Result<ClientMetadata, String> {
    let doc: J =
        serde_json::from_slice(body).map_err(|_| "client metadata is not JSON".to_string())?;
    if doc.get("client_id").and_then(J::as_str) != Some(client_id) {
        return Err("client_id does not match metadata URL".into());
    }
    let redirects: Vec<String> = doc
        .get("redirect_uris")
        .and_then(J::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    if redirects.is_empty() {
        return Err("client metadata has no redirect_uris".into());
    }
    for uri in &redirects {
        if !(uri.starts_with("https://") || loopback_parts(uri).is_some()) {
            return Err("redirect_uri must be loopback or https".into());
        }
    }
    Ok(ClientMetadata {
        client_id: client_id.to_string(),
        redirect_uris: redirects,
    })
}

/// Register (or refresh) a CIMD client as a public client.
pub fn upsert_metadata_client(conn: &Connection, meta: &ClientMetadata) -> rusqlite::Result<()> {
    let redirects = serde_json::to_string(&meta.redirect_uris).unwrap_or_else(|_| "[]".into());
    conn.execute(
        "INSERT INTO clients(client_id, secret_hash, client_type, redirect_uris, metadata_url, confidential, created_at)
         VALUES(?,?,?,?,?,0,?)
         ON CONFLICT(client_id) DO UPDATE SET client_type=excluded.client_type, redirect_uris=excluded.redirect_uris,
           metadata_url=excluded.metadata_url, confidential=0",
        params![meta.client_id, "", "public", redirects, meta.client_id, now()],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let conn = Connection::open(dir.path().join("authz.sqlite")).unwrap();
        conn.execute_batch(crate::ddl::AUTHZ_INIT).unwrap();
        prepare(&conn, dir.path(), "").unwrap();
        (dir, conn)
    }

    const RES: &str = "http://127.0.0.1:3014/mcp";

    fn served(r: &str) -> bool {
        r == RES
    }

    #[test]
    fn client_credentials_require_a_registered_secret() {
        let (dir, conn) = store();
        let cred = read_credential(&credential_path(dir.path(), PROVIDER_CLIENT_ID)).unwrap();
        assert!(matches!(
            client_credentials(&conn, PROVIDER_CLIENT_ID, "", RES, &served),
            Grant::Error {
                code: "invalid_client",
                ..
            }
        ));
        assert!(matches!(
            client_credentials(&conn, PROVIDER_CLIENT_ID, "wrong", RES, &served),
            Grant::Error {
                code: "invalid_client",
                ..
            }
        ));
        assert!(matches!(
            client_credentials(&conn, BOOTSTRAP_CLIENT_ID, "anything", RES, &served),
            Grant::Error {
                code: "unauthorized_client",
                ..
            }
        ));
        assert!(matches!(
            client_credentials(
                &conn,
                PROVIDER_CLIENT_ID,
                &cred.client_secret,
                "https://unrelated.example/mcp",
                &served
            ),
            Grant::Error {
                code: "invalid_target",
                ..
            }
        ));
        assert!(matches!(
            client_credentials(&conn, PROVIDER_CLIENT_ID, &cred.client_secret, RES, &served),
            Grant::Issued { .. }
        ));
    }

    #[test]
    fn a_confidential_client_with_an_empty_hash_cannot_authenticate() {
        let (_dir, conn) = store();
        set_client(&conn, "legacy-confidential", "", "confidential", true, "[]").unwrap();
        assert!(matches!(
            client_credentials(&conn, "legacy-confidential", "anything", RES, &served),
            Grant::Error {
                code: "invalid_client",
                ..
            }
        ));
    }

    #[test]
    fn credential_files_are_private_and_hold_the_only_plaintext() {
        let (dir, conn) = store();
        let path = credential_path(dir.path(), PROVIDER_CLIENT_ID);
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(credentials_dir(dir.path()))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        let cred = read_credential(&path).unwrap();
        let stored: String = conn
            .query_row(
                "SELECT secret_hash FROM clients WHERE client_id=?",
                [PROVIDER_CLIENT_ID],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stored, sha256_hex(&cred.client_secret));
        assert!(!stored.contains(&cred.client_secret));
    }

    #[test]
    fn operator_secret_wins_and_writes_no_file() {
        let dir = tempfile::tempdir().unwrap();
        let conn = Connection::open(dir.path().join("authz.sqlite")).unwrap();
        conn.execute_batch(crate::ddl::AUTHZ_INIT).unwrap();
        prepare(&conn, dir.path(), "platform-secret").unwrap();
        assert!(!credential_path(dir.path(), OPUTE_CLIENT_ID).exists());
        assert!(matches!(
            client_credentials(&conn, OPUTE_CLIENT_ID, "platform-secret", RES, &served),
            Grant::Issued { .. }
        ));
    }

    #[test]
    fn migration_revokes_once_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let conn = Connection::open(dir.path().join("authz.sqlite")).unwrap();
        conn.execute_batch(crate::ddl::AUTHZ_INIT).unwrap();
        conn.execute(
            "INSERT INTO tokens VALUES('h','c','r','mcp',4102444800,0,1)",
            [],
        )
        .unwrap();
        prepare(&conn, dir.path(), "").unwrap();
        let revoked: i64 = conn
            .query_row("SELECT revoked FROM tokens WHERE token_hash='h'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(revoked, 1);
        let cred = read_credential(&credential_path(dir.path(), PROVIDER_CLIENT_ID)).unwrap();
        let Grant::Issued { token, .. } =
            client_credentials(&conn, PROVIDER_CLIENT_ID, &cred.client_secret, RES, &served)
        else {
            panic!()
        };
        prepare(&conn, dir.path(), "").unwrap();
        let still: i64 = conn
            .query_row(
                "SELECT revoked FROM tokens WHERE token_hash=?",
                [sha256_hex(&token)],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(still, 0, "a second start revokes nothing");
        assert_eq!(
            read_credential(&credential_path(dir.path(), PROVIDER_CLIENT_ID)).unwrap(),
            cred
        );
    }

    #[test]
    fn approval_flow_issues_one_code_bound_to_the_request() {
        let (_dir, conn) = store();
        let verifier = "v".repeat(50);
        let req = AuthorizeRequest {
            client_id: BOOTSTRAP_CLIENT_ID.into(),
            redirect_uri: "http://127.0.0.1:5555/callback".into(),
            resource: RES.into(),
            code_challenge: s256(&verifier),
            state: "xyz".into(),
        };
        let AuthorizeOutcome::Pending { id, user_code } = begin_authorization(&conn, &req) else {
            panic!()
        };
        assert_eq!(poll(&conn, &id), StatusOutcome::Pending);
        decide(&conn, &user_code, true, 0, "cli").unwrap();
        let StatusOutcome::Approved { code, .. } = poll(&conn, &id) else {
            panic!()
        };
        assert_eq!(
            poll(&conn, &id),
            StatusOutcome::Expired,
            "the code is handed out once"
        );
        assert!(matches!(
            redeem_code(
                &conn,
                &code,
                BOOTSTRAP_CLIENT_ID,
                &req.redirect_uri,
                "wrong-verifier",
                ""
            ),
            Grant::Error {
                code: "invalid_grant",
                ..
            }
        ));
        // The failed attempt consumed the code.
        assert!(matches!(
            redeem_code(
                &conn,
                &code,
                BOOTSTRAP_CLIENT_ID,
                &req.redirect_uri,
                &verifier,
                ""
            ),
            Grant::Error {
                code: "invalid_grant",
                ..
            }
        ));
    }

    #[test]
    fn approved_code_redeems_with_pkce_and_consent_skips_the_wait() {
        let (_dir, conn) = store();
        let verifier = "w".repeat(50);
        let req = AuthorizeRequest {
            client_id: BOOTSTRAP_CLIENT_ID.into(),
            redirect_uri: "http://localhost:9/callback".into(),
            resource: RES.into(),
            code_challenge: s256(&verifier),
            state: String::new(),
        };
        let AuthorizeOutcome::Pending { id, user_code } = begin_authorization(&conn, &req) else {
            panic!()
        };
        decide(&conn, &user_code, true, 30, "cli").unwrap();
        let StatusOutcome::Approved { code, .. } = poll(&conn, &id) else {
            panic!()
        };
        assert!(matches!(
            redeem_code(
                &conn,
                &code,
                BOOTSTRAP_CLIENT_ID,
                &req.redirect_uri,
                &verifier,
                RES
            ),
            Grant::Issued { .. }
        ));
        assert!(matches!(
            begin_authorization(&conn, &req),
            AuthorizeOutcome::Code(_)
        ));
        revoke_consent(&conn, BOOTSTRAP_CLIENT_ID, RES).unwrap();
        assert!(matches!(
            begin_authorization(&conn, &req),
            AuthorizeOutcome::Pending { .. }
        ));
    }

    #[test]
    fn denial_and_limits() {
        let (_dir, conn) = store();
        let req = |n: u8| AuthorizeRequest {
            client_id: BOOTSTRAP_CLIENT_ID.into(),
            redirect_uri: "http://127.0.0.1/callback".into(),
            resource: RES.into(),
            code_challenge: format!("c{n}"),
            state: String::new(),
        };
        let AuthorizeOutcome::Pending { id, user_code } = begin_authorization(&conn, &req(0))
        else {
            panic!()
        };
        decide(&conn, &user_code, false, 30, "cli").unwrap();
        assert!(matches!(poll(&conn, &id), StatusOutcome::Denied { .. }));
        assert!(decide(&conn, &user_code, true, 30, "cli").is_err());
        for n in 1..=3 {
            assert!(matches!(
                begin_authorization(&conn, &req(n)),
                AuthorizeOutcome::Pending { .. }
            ));
        }
        assert_eq!(
            begin_authorization(&conn, &req(9)),
            AuthorizeOutcome::Error(
                "temporarily_unavailable",
                "too many pending authorization requests".into()
            )
        );
    }

    #[test]
    fn redirect_rules() {
        let reg = vec![
            "http://127.0.0.1/callback".to_string(),
            "https://app.example/cb".to_string(),
        ];
        assert!(redirect_allowed(&reg, "http://127.0.0.1:49152/callback"));
        assert!(redirect_allowed(&reg, "https://app.example/cb"));
        assert!(!redirect_allowed(&reg, "https://app.example/cb2"));
        assert!(!redirect_allowed(
            &reg,
            "http://127.0.0.1.attacker.example/callback"
        ));
        assert!(!redirect_allowed(&reg, "http://127.0.0.1:1/other"));
        assert!(!redirect_allowed(&reg, "http://user@127.0.0.1/callback"));
    }

    #[test]
    fn metadata_and_ssrf_rules() {
        assert!(vet_host("localhost", 443).is_err());
        assert!(vet_host("a.localhost", 443).is_err());
        assert!(vet_host("127.0.0.1", 443).is_err());
        assert!(vet_host("10.1.2.3", 443).is_err());
        assert!(vet_host("169.254.169.254", 443).is_err());
        assert!(vet_host("::1", 443).is_err());
        assert!(vet_host("fd00::1", 443).is_err());
        assert!(vet_host("8.8.8.8", 443).is_ok());
        let id = "https://app.example/client.json";
        assert!(validate_client_metadata(
            id,
            br#"{"client_id":"https://other/x","redirect_uris":["https://a/cb"]}"#
        )
        .is_err());
        assert!(validate_client_metadata(id, br#"{"client_id":"https://app.example/client.json","redirect_uris":["http://evil.example/cb"]}"#).is_err());
        assert!(validate_client_metadata(id, br#"{"client_id":"https://app.example/client.json","redirect_uris":["http://127.0.0.1/cb"]}"#).is_ok());
    }

    #[test]
    fn backoff_after_repeated_failures() {
        let b = Backoff::default();
        let keys = vec!["client:x".to_string()];
        for _ in 0..4 {
            b.fail(&keys);
        }
        assert!(!b.blocked(&keys));
        b.fail(&keys);
        assert!(b.blocked(&keys));
    }

    #[test]
    fn approval_page_has_no_control_and_escapes() {
        let p = PendingRecord {
            id: "i".into(),
            user_code: "BCDF-GHJK".into(),
            client_id: "<script>".into(),
            redirect_uri: "http://127.0.0.1/cb".into(),
            resource: RES.into(),
            code_challenge: String::new(),
            state: String::new(),
            status: "pending".into(),
            expires_at: 0,
        };
        let page = approval_page(&p, "/oauth/authorize/status?request=i", "");
        assert!(!page.contains("<form") && !page.contains("<button") && !page.contains("<script>"));
        assert!(page.contains("opute-host-agent oauth approve BCDF-GHJK"));
    }
}
