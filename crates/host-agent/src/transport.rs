//! `/health`, `/mcp` and the OAuth resource-server routes, with the Go
//! agent's behaviour (`internal/transport`, `internal/authz`) and the Go MCP
//! SDK's stateless JSON-response handler (go-sdk v1.7.0) behind it.
//!
//! A POST to `/mcp` passes these gates in order. Each gate's reply is the one
//! the Go binary sends, because that is the wire contract:
//!
//! ```text
//!  agent  method (OPTIONS 204 / non-POST 405) -> Origin -> bearer authz
//!         -> body -> envelope -> retired handshake -> ADR 0011 bypass
//!         -> modern header/_meta validation
//!         -> server/discover, tasks/*, resources/*  (agent answers)
//!  sdk    -> DNS-rebinding guard -> MCP-Protocol-Version -> Content-Type
//!         -> Accept -> 4 MiB cap -> JSON-RPC decode -> method table
//!         -> header/_meta consistency -> Mcp-Method/Mcp-Name
//!         -> session dispatch (era gating, params, handler, decoration)
//! ```
//!
//! Token *issuance* (`/oauth/authorize`, `/oauth/token`) is deferred pending
//! an owner design decision; those routes answer 501 in this build.

use std::net::IpAddr;
use std::sync::{Arc, Mutex};

use serde_json::{json, Map, Value as J};

use crate::goerr::quote;
use crate::gojson::{self, Node, Value};
use crate::http1::{self, Body, Request, Response};
use crate::store::AuthzStore;

use crate::mcpsdk::MODERN_VERSION;
const META_VERSION: &str = "io.modelcontextprotocol/protocolVersion";
const META_CLIENT_INFO: &str = "io.modelcontextprotocol/clientInfo";
const META_CAPABILITIES: &str = "io.modelcontextprotocol/clientCapabilities";
const META_SERVER_INFO: &str = "io.modelcontextprotocol/serverInfo";
const TASKS_EXTENSION: &str = "io.modelcontextprotocol/tasks";

/// Everything the HTTP layer needs from the composed runtime.
pub struct Server {
    pub instance_id: String,
    pub local_instance_id: String,
    pub agent_id: String,
    pub tool_prefix: String,
    pub implementation_name: String,
    pub version: String,
    pub fingerprint: Option<(String, String, String)>,
    pub execution_context: Option<(String, String, String)>,
    pub allow_legacy_handshake: bool,
    pub disable_localhost_protection: bool,
    pub bootstrap_token: String,
    pub authz: Mutex<AuthzStore>,
    pub health_observer: Box<dyn Fn() -> Map<String, J> + Send + Sync>,
    pub grant_backoff: crate::oauth::Backoff,
    /// The catalog for this mode, and its `tools/list` projection.
    pub catalog: &'static crate::catalog::Snapshot,
    pub published_tools: Vec<J>,
    pub standalone: bool,
    pub allow_mutations: bool,
    pub host: crate::tools::Host,
}

const ROUTES: [&str; 9] = [
    "/health",
    "/mcp",
    "/.well-known/oauth-protected-resource",
    "/.well-known/oauth-protected-resource/mcp",
    "/.well-known/oauth-authorization-server",
    "/oauth/authorize",
    "/oauth/authorize/status",
    "/oauth/token",
    "/oauth/revoke",
];

/// `serverHandler` + `ServeMux.ServeHTTP` + route handlers.
pub async fn route(server: Arc<Server>, req: Request, mut body: Body) -> (Response, Body) {
    if req.request_uri == "*" {
        if req.method == "OPTIONS" {
            let mut r = Response::new(200);
            r.headers.set("Content-Length", "0");
            return (r, body);
        }
        let mut r = Response::new(400);
        if req.proto_at_least(1, 1) {
            r.headers.set("Connection", "close");
        }
        return (r, body);
    }
    let path = if req.method == "CONNECT" {
        req.escaped_path.clone()
    } else {
        http1::clean_path(&req.escaped_path)
    };
    let matched = ROUTES
        .iter()
        .find(|p| http1::path_matches(&path, p))
        .copied();
    if req.method != "CONNECT" && path != req.escaped_path {
        let mut target = http1::escape_path(&path);
        if !req.raw_query.is_empty() {
            target = format!("{target}?{}", req.raw_query);
        }
        return (redirect(&req, &target, 301), body);
    }
    let response = match matched {
        Some("/health") => health(&server),
        Some("/mcp") => mcp(&server, &req, &mut body).await,
        Some("/.well-known/oauth-protected-resource")
        | Some("/.well-known/oauth-protected-resource/mcp") => prm(&req),
        Some("/.well-known/oauth-authorization-server") => as_metadata(&req),
        Some("/oauth/authorize") if req.method != "GET" && req.method != "POST" => {
            Response::new(405)
        }
        Some("/oauth/token") if req.method != "POST" => Response::new(405),
        Some("/oauth/authorize") => authorize_endpoint(&server, &req, &mut body).await,
        Some("/oauth/authorize/status") => authorize_status(&server, &req),
        Some("/oauth/token") => token_endpoint(&server, &req, &mut body).await,
        Some("/oauth/revoke") => revoke(&server, &req, &mut body).await,
        _ => Response::error("404 page not found", 404),
    };
    (response, body)
}

/// `http.Redirect` for a path-absolute target.
fn redirect(req: &Request, target: &str, code: u16) -> Response {
    let (path_part, query) = match target.find('?') {
        Some(i) => (&target[..i], &target[i..]),
        None => (target, ""),
    };
    let trailing = path_part.ends_with('/');
    let mut url = http1::clean_path(path_part);
    if trailing && !url.ends_with('/') {
        url.push('/');
    }
    url.push_str(query);
    let mut r = Response::new(code);
    r.headers.set("Location", hex_escape_non_ascii(&url));
    if req.method == "GET" || req.method == "HEAD" {
        r.headers.set("Content-Type", "text/html; charset=utf-8");
    }
    if req.method == "GET" {
        r.body = format!(
            "<a href=\"{}\">{}</a>.\n\n",
            html_escape(&url),
            http1::status_text(code)
        )
        .into_bytes();
    }
    r
}

fn hex_escape_non_ascii(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b >= 0x80 {
                format!("%{b:02x}")
            } else {
                (b as char).to_string()
            }
        })
        .collect()
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&#34;")
        .replace('\'', "&#39;")
}

fn json_response(status: u16, value: &J) -> Response {
    Response::json(status, go_marshal(value))
}

/// Go's `json.Marshal` escapes <, > and & as <, >, &.
pub fn go_marshal(value: &J) -> Vec<u8> {
    let text = serde_json::to_string(value).expect("JSON values serialize");
    text.replace('<', "\\u003c")
        .replace('>', "\\u003e")
        .replace('&', "\\u0026")
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
        .into_bytes()
}

// --- /health -----------------------------------------------------------------

fn health(server: &Server) -> Response {
    let mut payload = Map::new();
    payload.insert("ok".into(), J::Bool(true));
    let mut put = |k: &str, v: &str| {
        if !v.is_empty() {
            payload.insert(k.into(), J::String(v.into()));
        }
    };
    put("instanceId", &server.instance_id);
    put("localInstanceId", &server.local_instance_id);
    put("agentId", &server.agent_id);
    put("mcpToolNamePrefix", &server.tool_prefix);
    if let Some((fp, version, source)) = &server.fingerprint {
        if !fp.is_empty() {
            payload.insert("fingerprint".into(), J::String(fp.clone()));
            payload.insert("fingerprintVersion".into(), J::String(version.clone()));
            payload.insert("fingerprintSource".into(), J::String(source.clone()));
        }
    }
    if let Some((id, kind, display)) = &server.execution_context {
        if !id.is_empty() {
            payload.insert(
                "executionContext".into(),
                json!({"id": id, "kind": kind, "displayName": display}),
            );
        }
    }
    let mut extra = (server.health_observer)();
    if let Some(capabilities) = extra.remove("capabilities") {
        payload.insert("capabilities".into(), capabilities);
    }
    if !extra.is_empty() {
        payload.insert("capacity".into(), J::Object(extra));
    }
    json_response(200, &J::Object(payload))
}

// --- authz -------------------------------------------------------------------

fn request_scheme(req: &Request) -> &'static str {
    if req
        .headers
        .get("X-Forwarded-Proto")
        .eq_ignore_ascii_case("https")
    {
        "https"
    } else {
        "http"
    }
}

fn request_origin(req: &Request) -> String {
    format!("{}://{}", request_scheme(req), req.host)
}

fn canonical_mcp_resource(req: &Request) -> String {
    format!("{}://{}/mcp", request_scheme(req), req.host.trim())
}

/// `net.SplitHostPort`, returning only success.
pub fn split_host_port(hostport: &str) -> Option<(String, String)> {
    let (host, port) = if let Some(rest) = hostport.strip_prefix('[') {
        let end = rest.find(']')?;
        let after = &rest[end + 1..];
        let port = after.strip_prefix(':')?;
        if rest[..end].contains('[') || port.contains(']') || port.contains('[') {
            return None;
        }
        (rest[..end].to_string(), port.to_string())
    } else {
        let i = hostport.rfind(':')?;
        let host = &hostport[..i];
        if host.contains(':') || host.contains('[') || host.contains(']') {
            return None;
        }
        (host.to_string(), hostport[i + 1..].to_string())
    };
    if port.contains('[') || port.contains(']') {
        return None;
    }
    Some((host, port))
}

/// `net.ParseIP` (IPv4 dotted quad or IPv6, no zone).
fn parse_ip(s: &str) -> Option<IpAddr> {
    s.parse::<IpAddr>().ok()
}

fn is_loopback(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback(),
        IpAddr::V6(v6) => {
            v6.is_loopback() || v6.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback())
        }
    }
}

fn same_ip(a: &IpAddr, b: &IpAddr) -> bool {
    let norm = |ip: &IpAddr| match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(*ip, IpAddr::V4),
        _ => *ip,
    };
    norm(a) == norm(b)
}

fn interface_addrs() -> Vec<IpAddr> {
    nix::ifaddrs::getifaddrs()
        .map(|it| {
            it.filter_map(|ifa| {
                let addr = ifa.address?;
                if let Some(v4) = addr.as_sockaddr_in() {
                    Some(IpAddr::V4(v4.ip()))
                } else {
                    addr.as_sockaddr_in6().map(|v6| IpAddr::V6(v6.ip()))
                }
            })
            .collect()
        })
        .unwrap_or_default()
}

/// `authz.IsLocalHostAddress`.
pub fn is_local_host_address(host: &str) -> bool {
    let hostname = split_host_port(host).map_or(host.to_string(), |(h, _)| h);
    let hostname = hostname.trim_matches(|c| c == '[' || c == ']');
    if hostname.eq_ignore_ascii_case("localhost") || hostname == "127.0.0.1" || hostname == "::1" {
        return true;
    }
    let Some(ip) = parse_ip(hostname) else {
        return false;
    };
    if is_loopback(&ip) {
        return true;
    }
    interface_addrs().iter().any(|a| same_ip(a, &ip))
}

fn origin_allowed(req: &Request) -> bool {
    let origin = req.headers.get("Origin").trim();
    if origin.is_empty() {
        return true;
    }
    if is_local_host_address(&req.host) {
        let origin = origin.strip_suffix('/').unwrap_or(origin);
        return [
            "http://127.0.0.1",
            "https://127.0.0.1",
            "http://localhost",
            "https://localhost",
            "http://[::1]",
            "https://[::1]",
        ]
        .iter()
        .any(|allowed| origin == *allowed || origin.starts_with(&format!("{allowed}:")));
    }
    let origin = origin.strip_suffix('/').unwrap_or(origin);
    let scheme = request_scheme(req);
    let want = format!("{scheme}://{}", req.host);
    if let Some(i) = req.host.find(':') {
        if origin == format!("{scheme}://{}", &req.host[..i]) || origin == want {
            return true;
        }
    }
    origin == want
}

struct Decision {
    allowed: bool,
    status: u16,
    www_auth: String,
}

fn www_authenticate(req: &Request) -> String {
    let metadata = format!(
        "{}/.well-known/oauth-protected-resource/mcp",
        request_origin(req)
    );
    format!(
        "Bearer resource_metadata={}, scope={}",
        quote(&metadata),
        quote("mcp")
    )
}

fn constant_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn authorize(server: &Server, req: &Request) -> Decision {
    let deny = |status| Decision {
        allowed: false,
        status,
        www_auth: www_authenticate(req),
    };
    let auth = req.headers.get("Authorization");
    let token = auth.strip_prefix("Bearer ").map(str::trim).unwrap_or("");
    if token.is_empty() {
        return deny(401);
    }
    let bootstrap = server.bootstrap_token.trim();
    if !bootstrap.is_empty() && constant_eq(token, bootstrap) {
        if !is_local_host_address(&req.host) {
            return deny(403);
        }
        return Decision {
            allowed: true,
            status: 200,
            www_auth: String::new(),
        };
    }
    let hash = hex::encode(<sha2::Sha256 as sha2::Digest>::digest(token));
    let record = server
        .authz
        .lock()
        .ok()
        .and_then(|s| s.token_by_hash(&hash).ok().flatten());
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let Some(record) = record else {
        return deny(401);
    };
    if record.revoked || now > record.expires_at {
        return deny(401);
    }
    if record.resource != canonical_mcp_resource(req) {
        return deny(403);
    }
    if !record.scope.is_empty() && record.scope != "mcp" {
        return deny(403);
    }
    Decision {
        allowed: true,
        status: 200,
        www_auth: String::new(),
    }
}

fn prm(req: &Request) -> Response {
    if req.method != "GET" {
        return Response::new(405);
    }
    let issuer = request_origin(req);
    json_response(
        200,
        &json!({
            "resource": canonical_mcp_resource(req),
            "authorization_servers": [issuer],
            "scopes_supported": ["mcp"],
            "bearer_methods_supported": ["header"],
            "resource_documentation": format!("{issuer}/mcp"),
        }),
    )
}

fn as_metadata(req: &Request) -> Response {
    if req.method != "GET" {
        return Response::new(405);
    }
    let issuer = request_origin(req);
    json_response(
        200,
        &json!({
            "issuer": issuer,
            "authorization_endpoint": format!("{issuer}/oauth/authorize"),
            "token_endpoint": format!("{issuer}/oauth/token"),
            "revocation_endpoint": format!("{issuer}/oauth/revoke"),
            "code_challenge_methods_supported": ["S256"],
            "response_types_supported": ["code"],
            "grant_types_supported": ["authorization_code", "client_credentials"],
            "token_endpoint_auth_methods_supported": ["client_secret_basic", "client_secret_post", "none"],
            "authorization_response_iss_parameter_supported": true,
            "client_id_metadata_document_supported": true,
            "scopes_supported": ["mcp"],
        }),
    )
}

/// `Request.ParseForm` for POST/PUT/PATCH bodies, then `firstForm`.
async fn parse_form(req: &Request, body: &mut Body) -> Result<String, ()> {
    if !matches!(req.method.as_str(), "POST" | "PUT" | "PATCH") {
        return Ok(String::new());
    }
    let ct = req.headers.get("Content-Type");
    let ct = if ct.is_empty() {
        "application/octet-stream"
    } else {
        ct
    };
    let media = parse_media_type(ct).ok_or(())?;
    if media != "application/x-www-form-urlencoded" {
        return Ok(String::new());
    }
    let raw = body.read_all(Some(10 << 20)).await.map_err(|_| ())?;
    Ok(String::from_utf8_lossy(&raw).into_owned())
}

fn first_form(req: &Request, post: &str, key: &str) -> String {
    let from_post = http1::form_get(post, key);
    let value = if from_post.is_empty() {
        req.query_get(key)
    } else {
        from_post
    };
    let value = value.trim().to_string();
    if !value.is_empty() {
        return value;
    }
    req.query_get(key).trim().to_string()
}

async fn revoke(server: &Server, req: &Request, body: &mut Body) -> Response {
    if req.method != "POST" {
        return Response::new(405);
    }
    let post = parse_form(req, body).await.unwrap_or_default();
    let token = first_form(req, &post, "token");
    if !token.is_empty() {
        let hash = hex::encode(<sha2::Sha256 as sha2::Digest>::digest(token.as_str()));
        if let Ok(store) = server.authz.lock() {
            let _ = store.revoke_hash(&hash);
        }
    }
    Response::new(200)
}

// --- OAuth issuance (decision D8: stricter than the Go baseline) ---------------

fn oauth_json(status: u16, value: &J) -> Response {
    let mut r = json_response(status, value);
    // RFC 6749 §5.1: token responses are never cached.
    r.headers.set("Cache-Control", "no-store");
    r.headers.set("Pragma", "no-cache");
    r
}

fn oauth_error(status: u16, code: &str, description: &str) -> Response {
    oauth_json(
        status,
        &json!({"error": code, "error_description": description}),
    )
}

/// `Request.BasicAuth`: `Basic base64(id:secret)`, prefix case-insensitive.
fn basic_auth(req: &Request) -> Option<(String, String)> {
    use base64::Engine;
    let auth = req.headers.get("Authorization");
    if auth.len() < 6 || !auth[..6].eq_ignore_ascii_case("basic ") {
        return None;
    }
    let raw = base64::engine::general_purpose::STANDARD
        .decode(auth[6..].trim())
        .ok()?;
    let text = String::from_utf8(raw).ok()?;
    let (id, secret) = text.split_once(':')?;
    Some((id.to_string(), secret.to_string()))
}

fn remote_ip(req: &Request) -> String {
    req.remote_addr.ip().to_string()
}

async fn token_endpoint(server: &Server, req: &Request, body: &mut Body) -> Response {
    let Ok(post) = parse_form(req, body).await else {
        return oauth_error(400, "invalid_request", "malformed body");
    };
    let grant = first_form(req, &post, "grant_type");
    let (client_id, secret) = basic_auth(req).unwrap_or_else(|| {
        (
            first_form(req, &post, "client_id"),
            first_form(req, &post, "client_secret"),
        )
    });
    let keys = vec![
        format!("client:{client_id}"),
        format!("remote:{}", remote_ip(req)),
    ];
    let resource = first_form(req, &post, "resource");
    let canonical = canonical_mcp_resource(req);
    let served = |r: &str| r == canonical;
    let outcome = {
        let Ok(store) = server.authz.lock() else {
            return oauth_error(500, "server_error", "store unavailable");
        };
        let conn = store.conn();
        match grant.as_str() {
            "client_credentials" => {
                crate::oauth::client_credentials(conn, &client_id, &secret, &resource, &served)
            }
            "authorization_code" => crate::oauth::redeem_code(
                conn,
                &first_form(req, &post, "code"),
                &client_id,
                &first_form(req, &post, "redirect_uri"),
                &first_form(req, &post, "code_verifier"),
                &resource,
            ),
            _ => crate::oauth::Grant::Error {
                status: 400,
                code: "unsupported_grant_type",
                description: "use authorization_code or client_credentials".into(),
            },
        }
    };
    match outcome {
        crate::oauth::Grant::Issued { token, resource } => {
            crate::oauth::audit(
                "issue",
                &[
                    ("client_id", &client_id),
                    ("grant", &grant),
                    ("resource", &resource),
                    ("outcome", "issued"),
                    ("remote", &remote_ip(req)),
                ],
            );
            oauth_json(
                200,
                &json!({
                    "access_token": token,
                    "token_type": "Bearer",
                    "expires_in": crate::oauth::ACCESS_TOKEN_TTL,
                    "scope": "mcp",
                    "resource": resource,
                }),
            )
        }
        crate::oauth::Grant::Error {
            status,
            code,
            description,
        } => {
            // Only failing requests are throttled: secrets and codes carry 256
            // bits, so slowing a valid grant adds no protection, and behind a
            // tunnel every caller shares the loopback address.
            if server.grant_backoff.blocked(&keys) {
                crate::oauth::audit(
                    "deny",
                    &[
                        ("client_id", &client_id),
                        ("grant", &grant),
                        ("resource", &resource),
                        ("outcome", "backoff"),
                        ("remote", &remote_ip(req)),
                    ],
                );
                return oauth_error(429, "slow_down", "too many failed requests; retry later");
            }
            if status < 500 {
                server.grant_backoff.fail(&keys);
            }
            crate::oauth::audit(
                "deny",
                &[
                    ("client_id", &client_id),
                    ("grant", &grant),
                    ("resource", &resource),
                    ("outcome", code),
                    ("remote", &remote_ip(req)),
                ],
            );
            let mut r = oauth_error(status, code, &description);
            if status == 401 {
                r.headers
                    .set("WWW-Authenticate", "Basic realm=\"host-agent\"");
            }
            r
        }
    }
}

/// `application/x-www-form-urlencoded` encoding of one value.
fn form_encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            b' ' => "+".to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Append query parameters to a redirect URI, keeping its own query.
fn with_query(uri: &str, params: &[(&str, &str)]) -> String {
    let (base, fragment) = match uri.split_once('#') {
        Some((b, _)) => (b, ""),
        None => (uri, ""),
    };
    let mut out = base.to_string();
    for (k, v) in params {
        if v.is_empty() && *k == "state" {
            continue;
        }
        out.push(if out.contains('?') { '&' } else { '?' });
        out.push_str(&format!("{k}={}", form_encode(v)));
    }
    out + fragment
}

fn redirect_to(location: &str) -> Response {
    let mut r = Response::new(302);
    r.headers.set("Location", location.to_string());
    r.headers.set("Cache-Control", "no-store");
    r
}

fn html_page(status: u16, html: String) -> Response {
    let mut r = Response::new(status);
    for (k, v) in crate::oauth::page_headers() {
        r.headers.set(k, v);
    }
    r.body = html.into_bytes();
    r
}

async fn authorize_endpoint(server: &Server, req: &Request, body: &mut Body) -> Response {
    let Ok(post) = parse_form(req, body).await else {
        return Response::error("invalid request", 400);
    };
    let field = |k: &str| first_form(req, &post, k);
    let (client_id, redirect_uri, resource) =
        (field("client_id"), field("redirect_uri"), field("resource"));
    let (challenge, method, state) = (
        field("code_challenge"),
        field("code_challenge_method"),
        field("state"),
    );
    if field("response_type") != "code"
        || client_id.is_empty()
        || redirect_uri.is_empty()
        || resource.is_empty()
        || challenge.is_empty()
        || method != "S256"
    {
        return Response::error("invalid_request", 400);
    }
    if resource != canonical_mcp_resource(req) {
        return Response::error("invalid_target", 400);
    }
    let registered = {
        let Ok(store) = server.authz.lock() else {
            return Response::error("server_error", 500);
        };
        crate::oauth::client(store.conn(), &client_id)
            .ok()
            .flatten()
    };
    let client = match registered {
        Some(c) => c,
        None if client_id.starts_with("https://") => {
            let id = client_id.clone();
            let fetched =
                tokio::task::spawn_blocking(move || crate::oauth::fetch_client_metadata(&id))
                    .await
                    .unwrap_or_else(|_| Err("client metadata fetch failed".into()));
            let meta = match fetched {
                Ok(m) => m,
                Err(e) => return Response::error(&e, 400),
            };
            let Ok(store) = server.authz.lock() else {
                return Response::error("server_error", 500);
            };
            if crate::oauth::upsert_metadata_client(store.conn(), &meta).is_err() {
                return Response::error("server_error", 500);
            }
            match crate::oauth::client(store.conn(), &client_id)
                .ok()
                .flatten()
            {
                Some(c) => c,
                None => return Response::error("server_error", 500),
            }
        }
        None => return Response::error("unknown client_id", 400),
    };
    if client.confidential {
        return Response::error("unauthorized_client", 400);
    }
    if !crate::oauth::redirect_allowed(&client.redirect_uris, &redirect_uri) {
        return Response::error("invalid redirect_uri", 400);
    }
    let request = crate::oauth::AuthorizeRequest {
        client_id: client.client_id.clone(),
        redirect_uri: redirect_uri.clone(),
        resource,
        code_challenge: challenge,
        state: state.clone(),
    };
    let outcome = {
        let Ok(store) = server.authz.lock() else {
            return Response::error("server_error", 500);
        };
        crate::oauth::begin_authorization(store.conn(), &request)
    };
    let iss = request_origin(req);
    match outcome {
        crate::oauth::AuthorizeOutcome::Code(code) => redirect_to(&with_query(
            &redirect_uri,
            &[("code", &code), ("iss", &iss), ("state", &state)],
        )),
        crate::oauth::AuthorizeOutcome::Pending { id, .. } => {
            let Ok(store) = server.authz.lock() else {
                return Response::error("server_error", 500);
            };
            match crate::oauth::pending_by_id(store.conn(), &id) {
                Some(p) => html_page(200, crate::oauth::approval_page(&p, &status_path(&id), "")),
                None => Response::error("server_error", 500),
            }
        }
        crate::oauth::AuthorizeOutcome::Error(code, _) => redirect_to(&with_query(
            &redirect_uri,
            &[("error", code), ("iss", &iss), ("state", &state)],
        )),
    }
}

fn status_path(id: &str) -> String {
    format!("/oauth/authorize/status?request={}", form_encode(id))
}

fn authorize_status(server: &Server, req: &Request) -> Response {
    if req.method != "GET" && req.method != "HEAD" {
        return Response::new(405);
    }
    let id = req.query_get("request");
    let Ok(store) = server.authz.lock() else {
        return Response::error("server_error", 500);
    };
    let conn = store.conn();
    let iss = request_origin(req);
    match crate::oauth::poll(conn, &id) {
        crate::oauth::StatusOutcome::Pending => match crate::oauth::pending_by_id(conn, &id) {
            Some(p) => html_page(200, crate::oauth::approval_page(&p, &status_path(&id), "")),
            None => html_page(404, crate::oauth::approval_page(&unknown_pending(), "", "This authorization request does not exist.")),
        },
        crate::oauth::StatusOutcome::Approved { code, redirect_uri, state } => {
            redirect_to(&with_query(&redirect_uri, &[("code", &code), ("iss", &iss), ("state", &state)]))
        }
        crate::oauth::StatusOutcome::Denied { redirect_uri, state } => {
            redirect_to(&with_query(&redirect_uri, &[("error", "access_denied"), ("iss", &iss), ("state", &state)]))
        }
        crate::oauth::StatusOutcome::Expired => html_page(
            200,
            crate::oauth::approval_page(&unknown_pending(), "", "This authorization request has expired or was already used. Start again from the application."),
        ),
        crate::oauth::StatusOutcome::Unknown => html_page(
            404,
            crate::oauth::approval_page(&unknown_pending(), "", "This authorization request does not exist."),
        ),
    }
}

fn unknown_pending() -> crate::oauth::PendingRecord {
    crate::oauth::PendingRecord {
        id: String::new(),
        user_code: String::new(),
        client_id: String::new(),
        redirect_uri: String::new(),
        resource: String::new(),
        code_challenge: String::new(),
        state: String::new(),
        status: String::new(),
        expires_at: 0,
    }
}

// --- /mcp: agent layer -----------------------------------------------------

struct ProtocolError {
    code: i64,
    message: String,
    data: Option<J>,
}

fn perr(code: i64, message: impl Into<String>) -> ProtocolError {
    ProtocolError {
        code,
        message: message.into(),
        data: None,
    }
}

fn header_mismatch() -> ProtocolError {
    perr(-32020, "HeaderMismatch")
}

/// The id echoed by the agent's own writers: Go decoded it into `any`.
fn agent_id_value(id: Option<&Node>) -> J {
    match id {
        None => J::Null,
        Some(n) => go_any(n),
    }
}

/// A decoded `any`, re-encoded the way Go's `json.Marshal` prints it.
pub(crate) fn go_any(n: &Node) -> J {
    match &n.value {
        Value::Null => J::Null,
        Value::Bool(b) => J::Bool(*b),
        Value::Number(text) => {
            let f: f64 = text.parse().unwrap_or(0.0);
            go_float(f)
        }
        Value::String(s) => J::String(s.clone()),
        Value::Array(items) => J::Array(items.iter().map(go_any).collect()),
        Value::Object(members) => {
            let mut map = Map::new();
            for (k, v) in members {
                map.insert(k.clone(), go_any(v));
            }
            J::Object(map)
        }
    }
}

/// float64 as encoding/json prints it (integral values without a fraction).
fn go_float(f: f64) -> J {
    if f.fract() == 0.0 && f.abs() < 1e21 {
        if f.abs() <= 9.007_199_254_740_992e15 {
            return J::from(f as i64);
        }
        return serde_json::from_str(&format!("{f:.0}")).unwrap_or(J::Null);
    }
    serde_json::Number::from_f64(f).map_or(J::Null, J::Number)
}

fn write_protocol_error(id: &J, err: ProtocolError) -> Response {
    let status = if err.code == -32601 { 404 } else { 400 };
    let mut e = Map::new();
    e.insert("code".into(), J::from(err.code));
    e.insert("message".into(), J::String(err.message));
    if let Some(d) = err.data {
        e.insert("data".into(), d);
    }
    json_response(
        status,
        &json!({"jsonrpc": "2.0", "id": id, "error": J::Object(e)}),
    )
}

fn write_rpc_result(id: &J, result: J) -> Response {
    json_response(200, &json!({"jsonrpc": "2.0", "id": id, "result": result}))
}

/// `writeJSONRPCError`: a plain error is -32603 on 200; a protocol or
/// jsonrpc error keeps its code, with 404 for -32601.
fn write_rpc_error(id: &J, err: ExtError) -> Response {
    let (code, message, status) = match err {
        ExtError::Plain(m) => (-32603, m, 200),
        ExtError::Rpc(code, m) => (code, m, if code == -32601 { 404 } else { 200 }),
    };
    json_response(
        status,
        &json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}),
    )
}

enum ExtError {
    Plain(String),
    Rpc(i64, String),
}

/// RFC 2047 encoded-word decoding as `decodeRFC2047` does it.
fn decode_rfc2047(value: &str) -> String {
    let value = value.trim();
    if !value.starts_with("=?") || !value.ends_with("?=") {
        return value.to_string();
    }
    let parts: Vec<&str> = value.split('?').collect();
    if parts.len() != 5 {
        return value.to_string();
    }
    let (charset, encoding, payload) = (parts[1].to_lowercase(), parts[2].to_lowercase(), parts[3]);
    if charset != "utf-8" && charset != "us-ascii" {
        return value.to_string();
    }
    match encoding.as_str() {
        "b" => base64::Engine::decode(&base64::engine::general_purpose::STANDARD, payload)
            .map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_else(|_| value.to_string()),
        "q" => decode_q(payload).unwrap_or_else(|| value.to_string()),
        _ => value.to_string(),
    }
}

fn decode_q(value: &str) -> Option<String> {
    let b = value.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'_' => out.push(b' '),
            b'=' => {
                if i + 2 >= b.len() {
                    return None;
                }
                let hex = std::str::from_utf8(&b[i + 1..i + 3]).ok()?;
                out.push(u8::from_str_radix(hex, 16).ok()?);
                i += 2;
            }
            c => out.push(c),
        }
        i += 1;
    }
    Some(String::from_utf8_lossy(&out).into_owned())
}

fn is_modern_only(method: &str) -> bool {
    method == "server/discover" || method.starts_with("tasks/") || method.starts_with("resources/")
}

fn is_legacy_compatible(method: &str) -> bool {
    matches!(
        method,
        "initialize"
            | "notifications/initialized"
            | "notifications/cancelled"
            | "ping"
            | "tools/list"
            | "tools/call"
            | "prompts/list"
            | "prompts/get"
    )
}

fn is_retired_handshake(method: &str) -> bool {
    method == "initialize" || method == "notifications/initialized"
}

/// Parse params the way `json.Unmarshal(raw, &map[string]json.RawMessage)`
/// does: `None` when raw is empty, `null`, or not an object.
fn params_object(raw: &[u8]) -> Option<Node> {
    if raw.is_empty() || raw == b"null" {
        return None;
    }
    let n = gojson::parse(raw).ok()?;
    n.as_object()?;
    Some(n)
}

fn is_modern_request(raw: &[u8]) -> bool {
    let Some(params) = params_object(raw) else {
        return false;
    };
    let Some(meta) = params.key("_meta") else {
        return false;
    };
    meta.as_object().is_some() && meta.key(META_VERSION).is_some()
}

fn skip_modern_validation(server: &Server, method: &str, raw: &[u8]) -> bool {
    server.allow_legacy_handshake
        && is_legacy_compatible(method)
        && (is_retired_handshake(method) || !is_modern_request(raw))
}

fn tasks_capability_error() -> ProtocolError {
    ProtocolError {
        code: -32003,
        message: "Missing required client capability".into(),
        data: Some(json!({"requiredCapabilities": {"extensions": {TASKS_EXTENSION: {}}}})),
    }
}

/// Go's `json.Unmarshal(raw, &s)` into a string: only a JSON string sets it.
fn unmarshal_string(node: Option<&Node>) -> String {
    node.and_then(|n| n.as_str()).unwrap_or("").to_string()
}

fn validate_modern(req: &Request, method: &str, raw: &[u8]) -> Result<(), ProtocolError> {
    let header_version = decode_rfc2047(req.headers.get("MCP-Protocol-Version").trim());
    let header_method = decode_rfc2047(req.headers.get("Mcp-Method").trim());
    let header_name = decode_rfc2047(req.headers.get("Mcp-Name").trim());
    let params = params_object(raw).ok_or_else(header_mismatch)?;
    let meta = params.key("_meta").ok_or_else(header_mismatch)?;
    if meta.is_null() {
        // json.Unmarshal of null into a map leaves it nil: no keys.
        return Err(header_mismatch());
    }
    meta.as_object().ok_or_else(header_mismatch)?;
    let requested = unmarshal_string(meta.key(META_VERSION));
    if requested.is_empty() {
        return Err(header_mismatch());
    }
    if requested != MODERN_VERSION
        || (!header_version.is_empty() && header_version != MODERN_VERSION)
    {
        return Err(ProtocolError {
            code: -32022,
            message: "Unsupported protocol version".into(),
            data: Some(json!({"supported": [MODERN_VERSION], "requested": requested})),
        });
    }
    if header_version.is_empty() || header_version != requested {
        return Err(header_mismatch());
    }
    if header_method.is_empty() || header_method != method {
        return Err(header_mismatch());
    }
    for key in [META_CLIENT_INFO, META_CAPABILITIES] {
        if meta.key(key).is_none() {
            return Err(header_mismatch());
        }
    }
    if method == "tools/call" {
        let name = unmarshal_string(params.key("name"));
        if header_name.is_empty() || header_name != name {
            return Err(header_mismatch());
        }
    }
    if method.starts_with("tasks/") {
        let caps = meta
            .key(META_CAPABILITIES)
            .filter(|c| c.as_object().is_some() || c.is_null());
        let ext = caps
            .and_then(|c| c.key("extensions"))
            .filter(|e| e.as_object().is_some() || e.is_null());
        let Some(ext) = ext else {
            return Err(tasks_capability_error());
        };
        if ext.key(TASKS_EXTENSION).is_none() {
            return Err(tasks_capability_error());
        }
        if matches!(method, "tasks/get" | "tasks/update" | "tasks/cancel") {
            let task_id = match params.key("taskId") {
                Some(n) => match &n.value {
                    Value::String(s) => Some(s.clone()),
                    Value::Null => Some(String::new()),
                    _ => None,
                },
                None => None,
            };
            match task_id {
                Some(t) if !t.trim().is_empty() => {
                    if header_name.is_empty() || header_name != t {
                        return Err(header_mismatch());
                    }
                }
                _ => return Err(perr(-32602, format!("{method} requires params.taskId"))),
            }
        }
    }
    Ok(())
}

/// `HandleExtensionMethod` for the M2 surface. Task state belongs to M4: with
/// no task store every id is unknown, which is what an empty Go state reports.
fn extension(server: &Server, method: &str, raw: &[u8]) -> Result<J, ExtError> {
    let task_id = || {
        gojson::parse(raw)
            .ok()
            .and_then(|n| n.field("taskId").and_then(|t| t.as_str().map(String::from)))
            .unwrap_or_default()
    };
    match method {
        "server/discover" => Ok(json!({
            "resultType": "complete",
            "supportedVersions": [MODERN_VERSION],
            "capabilities": {"tools": {}, "extensions": {TASKS_EXTENSION: {}}},
            "_meta": {META_SERVER_INFO: {"name": server.implementation_name, "version": server.version}},
        })),
        "tasks/get" => Err(ExtError::Plain(format!("task not found: {}", task_id()))),
        "tasks/cancel" => Err(ExtError::Plain(format!(
            "cannot cancel task: {}",
            task_id()
        ))),
        "tasks/update" => {
            let parsed = gojson::parse(raw).ok();
            let id = task_id();
            if id.trim().is_empty() {
                return Err(ExtError::Plain("tasks/update requires taskId".into()));
            }
            let has_inputs = parsed
                .as_ref()
                .and_then(|n| n.field("inputResponses"))
                .is_some_and(|v| v.as_object().is_some());
            if !has_inputs {
                return Err(ExtError::Plain(
                    "tasks/update requires inputResponses".into(),
                ));
            }
            Err(ExtError::Plain(format!("task not found: {id}")))
        }
        "resources/list" | "resources/read" | "tasks/list" => {
            Err(ExtError::Rpc(-32601, format!("Method not found: {method}")))
        }
        _ => Err(ExtError::Plain(format!(
            "unsupported extension method: {method}"
        ))),
    }
}

async fn mcp(server: &Arc<Server>, req: &Request, body: &mut Body) -> Response {
    match req.method.as_str() {
        "OPTIONS" => return Response::new(204),
        "POST" => {}
        _ => {
            let mut r = Response::error("method not allowed", 405);
            r.headers.set("Allow", "POST, OPTIONS");
            return r;
        }
    }
    if !origin_allowed(req) {
        return Response::error("forbidden origin", 403);
    }
    let decision = authorize(server, req);
    if !decision.allowed {
        let status = if decision.status == 0 {
            401
        } else {
            decision.status
        };
        let mut r = Response::error(http1::status_text(status), status);
        if !decision.www_auth.is_empty() {
            r.headers.set("WWW-Authenticate", decision.www_auth);
        }
        return r;
    }
    let raw = match body.read_all(None).await {
        Ok(b) => b,
        Err(e) => return Response::error(&e.to_string(), 400),
    };
    if raw.is_empty() {
        return Response::error("empty body", 400);
    }
    // json.Unmarshal into struct{Method string; Params RawMessage; ID any}.
    let Ok(doc) = gojson::parse(&raw) else {
        return Response::error("invalid json-rpc", 400);
    };
    let (method, params_raw, id) = match &doc.value {
        Value::Null => (String::new(), Vec::new(), J::Null),
        Value::Object(_) => {
            let Ok(method) = gojson::go_string(doc.field("method")) else {
                return Response::error("invalid json-rpc", 400);
            };
            let params = doc
                .field("params")
                .map(|n| n.raw.clone())
                .unwrap_or_default();
            (method, params, agent_id_value(doc.field("id")))
        }
        _ => return Response::error("invalid json-rpc", 400),
    };
    if is_retired_handshake(&method) && !server.allow_legacy_handshake {
        return write_protocol_error(
            &id,
            ProtocolError {
                code: -32601,
                message: format!("Method not found: {method}"),
                data: Some(json!({"supported": [MODERN_VERSION]})),
            },
        );
    }
    if !skip_modern_validation(server, &method, &params_raw) {
        if let Err(e) = validate_modern(req, &method, &params_raw) {
            return write_protocol_error(&id, e);
        }
    }
    if is_modern_only(&method) {
        return match extension(server, &method, &params_raw) {
            Ok(result) => write_rpc_result(&id, result),
            Err(e) => write_rpc_error(&id, e),
        };
    }
    let protected = !server.disable_localhost_protection && is_local_host_address(req.host.trim());
    let serve = move |server: &Server, req: &Request, raw: &[u8]| {
        let call = |name: &str, params: Option<&Node>| crate::tools::call(server, name, params);
        let sdk = crate::mcpsdk::Sdk {
            implementation_name: &server.implementation_name,
            version: &server.version,
            tools: &server.published_tools,
            call_tool: &call,
        };
        crate::mcpsdk::serve(&sdk, req, raw, protected)
    };
    let mut response = if method == "tools/call" {
        let (mut req, mut raw) = (req.clone(), raw.clone());
        rewrite_tool_call_name(server, &mut req, &mut raw);
        // Tool handlers read the host and run commands; keep them off the
        // connection-serving thread.
        let server = Arc::clone(server);
        tokio::task::spawn_blocking(move || serve(&server, &req, &raw))
            .await
            .unwrap_or_else(|_| Response::error("internal error", 500))
    } else {
        serve(server, req, &raw)
    };
    if method == "tools/call" {
        response.body = normalize_task_creation(&response.body);
        response.headers.del("Content-Length");
    }
    response
}

/// `ResolveIncomingToolCallName`: with prefixing on, an unprefixed catalog
/// name is mapped to its wire name so callers can keep using catalog names.
fn resolve_tool_name(server: &Server, name: &str) -> String {
    let name = name.trim();
    let prefix = server.tool_prefix.as_str();
    if prefix.is_empty() || name.is_empty() || name.starts_with(&format!("{prefix}_")) {
        return name.to_string();
    }
    if server.catalog.tools.iter().any(|d| d.name == name) {
        return crate::catalog::wire_name(prefix, name);
    }
    name.to_string()
}

/// `rewriteIncomingToolCallName`: the body is decoded into a map, the name
/// replaced, and the map re-encoded (sorted keys, float64 numbers); the
/// `Mcp-Name` header follows when it was empty or named the same tool.
fn rewrite_tool_call_name(server: &Server, req: &mut Request, raw: &mut Vec<u8>) {
    let Ok(doc) = crate::gojson::parse(raw) else {
        return;
    };
    let J::Object(mut envelope) = go_any(&doc) else {
        return;
    };
    let Some(J::Object(mut params)) = envelope.get("params").cloned() else {
        return;
    };
    let Some(name) = params.get("name").and_then(J::as_str).map(str::to_string) else {
        return;
    };
    if name.trim().is_empty() {
        return;
    }
    let resolved = resolve_tool_name(server, &name);
    if resolved.is_empty() || resolved == name {
        return;
    }
    params.insert("name".into(), J::from(resolved.clone()));
    envelope.insert("params".into(), J::Object(params));
    let mut out = String::new();
    crate::gojson::encode(&J::Object(envelope), &mut out);
    *raw = out.into_bytes();
    let header = req.headers.get("Mcp-Name").to_string();
    if header.is_empty() || header == name {
        req.headers.set("Mcp-Name", resolved);
    }
}

/// `normalizeTaskCreationResponse`: a task-creating tools/call result is
/// flattened so the task handle is the result.
fn normalize_task_creation(body: &[u8]) -> Vec<u8> {
    let Ok(mut envelope) = serde_json::from_slice::<J>(body) else {
        return body.to_vec();
    };
    let Some(result) = envelope.get("result").and_then(J::as_object).cloned() else {
        return body.to_vec();
    };
    let Some(structured) = result
        .get("structuredContent")
        .and_then(J::as_object)
        .cloned()
    else {
        return body.to_vec();
    };
    if structured.get("resultType") != Some(&J::String("task".into())) {
        return body.to_vec();
    }
    let mut flat = structured;
    if let Some(meta) = result.get("_meta") {
        flat.insert("_meta".into(), meta.clone());
    }
    envelope["result"] = J::Object(flat);
    go_marshal(&envelope)
}

// --- media types -------------------------------------------------------------

/// `mime.ParseMediaType`, returning only the lowercased media type.
pub fn parse_media_type(v: &str) -> Option<String> {
    let (base, mut rest) = match v.find(';') {
        Some(i) => (&v[..i], &v[i..]),
        None => (v, ""),
    };
    let media = base.trim().to_lowercase();
    let token = |s: &str| !s.is_empty() && s.bytes().all(is_token_byte);
    match media.split_once('/') {
        Some((a, b)) if token(a) && token(b) => {}
        None if token(&media) => {}
        _ => return None,
    }
    let mut seen = std::collections::HashSet::new();
    loop {
        rest = rest.trim_start();
        if rest.is_empty() {
            break;
        }
        rest = rest.strip_prefix(';')?.trim_start();
        if rest.is_empty() {
            break;
        }
        let eq = rest.find('=')?;
        let key = rest[..eq].trim().to_lowercase();
        if !token(&key) {
            return None;
        }
        let after = rest[eq + 1..].trim_start();
        let (consumed, ok) = if let Some(q) = after.strip_prefix('"') {
            let mut i = 0;
            let b = q.as_bytes();
            let mut escaped = false;
            let mut end = None;
            while i < b.len() {
                if escaped {
                    escaped = false;
                } else if b[i] == b'\\' {
                    escaped = true;
                } else if b[i] == b'"' {
                    end = Some(i);
                    break;
                }
                i += 1;
            }
            match end {
                Some(e) => (e + 2, true),
                None => (0, false),
            }
        } else {
            let n = after.bytes().take_while(|c| is_token_byte(*c)).count();
            (n, n > 0)
        };
        if !ok || !seen.insert(key) {
            return None;
        }
        rest = &after[consumed..];
    }
    Some(media)
}

fn is_token_byte(c: u8) -> bool {
    c.is_ascii_alphanumeric()
        || matches!(
            c,
            b'!' | b'#'
                | b'$'
                | b'%'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'.'
                | b'^'
                | b'_'
                | b'`'
                | b'|'
                | b'~'
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_type_rules_match_go() {
        assert_eq!(
            parse_media_type("application/json; charset=utf-8").as_deref(),
            Some("application/json")
        );
        assert_eq!(
            parse_media_type("Application/JSON").as_deref(),
            Some("application/json")
        );
        assert_eq!(parse_media_type("application/json;charset"), None);
        assert_eq!(parse_media_type("application/json, text/plain"), None);
        assert_eq!(parse_media_type(""), None);
    }

    #[test]
    fn local_host_rules() {
        assert!(is_local_host_address("127.0.0.1:3014"));
        assert!(is_local_host_address("localhost"));
        assert!(is_local_host_address("[::1]:9"));
        assert!(!is_local_host_address("example.com:80"));
        assert!(crate::mcpsdk::is_loopback_host("[::1]:9"));
        assert!(!crate::mcpsdk::is_loopback_host("example.com"));
    }

    #[test]
    fn rfc2047_decoding_matches_go() {
        assert_eq!(decode_rfc2047("=?UTF-8?B?cGluZw==?="), "ping");
        assert_eq!(decode_rfc2047("=?utf-8?q?tools=2Flist?="), "tools/list");
        assert_eq!(
            decode_rfc2047("=?iso-8859-1?B?cGluZw==?="),
            "=?iso-8859-1?B?cGluZw==?="
        );
    }

    #[test]
    fn go_float_formatting() {
        assert_eq!(go_float(1.5), json!(1.5));
        assert_eq!(go_float(1000.0), json!(1000));
        assert_eq!(go_float(9007199254740993.0), json!(9007199254740992_i64));
    }
}
