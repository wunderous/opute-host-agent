//! The Go MCP SDK's stateless, JSON-response Streamable HTTP handler
//! (go-sdk v1.7.0) for the methods this server registers.
//!
//! The Go agent delegates every method it does not answer itself to the SDK,
//! so the SDK's behaviour is part of the wire contract: which era a request
//! belongs to, which methods exist in that era, how params are decoded and
//! what the decode errors say, and which HTTP status a JSON-RPC error gets.
//!
//! ```text
//!  guard   DNS rebinding -> MCP-Protocol-Version -> Content-Type -> Accept
//!          -> 4 MiB cap
//!  POST    JSON-RPC decode -> method table (id / params rules)
//!          -> header vs _meta version -> Mcp-Method / Mcp-Name
//!          -> notification? 202
//!  session _meta (clientInfo, clientCapabilities) -> era gating
//!          -> typed params (segmentio errors) -> handler -> decoration
//!  status  2026-07-28 header: -32601 -> 404; -32602/-32021/-32022 -> 400
//! ```
//!
//! Params are decoded with segmentio/encoding, whose type errors read
//! `json: cannot unmarshal "<next 32 bytes>" into Go struct field
//! <Struct...>.<key path> of type <T>`; `Ty` reproduces that.

use serde_json::{json, Map, Value as J};

use crate::goerr::quote;
use crate::gojson::{self, Node, Value};
use crate::http1::{Request, Response};
use crate::transport::{go_marshal, parse_media_type};

pub const MODERN_VERSION: &str = "2026-07-28";
const MAX_BODY: u64 = 4 << 20;
pub const SUPPORTED_VERSIONS: [&str; 5] = [
    "2026-07-28",
    "2025-11-25",
    "2025-06-18",
    "2025-03-26",
    "2024-11-05",
];
const META_VERSION: &str = "io.modelcontextprotocol/protocolVersion";
const META_CLIENT_INFO: &str = "io.modelcontextprotocol/clientInfo";
const META_CAPABILITIES: &str = "io.modelcontextprotocol/clientCapabilities";
const META_SERVER_INFO: &str = "io.modelcontextprotocol/serverInfo";

/// What the SDK server knows about itself.
pub struct Sdk<'a> {
    pub implementation_name: &'a str,
    pub version: &'a str,
    /// Published tool names, sorted (catalog content arrives in M3).
    pub tools: &'a [J],
}

// --- segmentio typed decoding ------------------------------------------------

pub enum Ty {
    /// A string-kinded type, by its Go name ("string", "mcp.LoggingLevel").
    Str(&'static str),
    Bool,
    Raw,
    /// A map[string]any-kinded type ("mcp.Meta", "map[string]interface {}").
    MapAny(&'static str),
    /// map[string]string
    MapStr,
    /// []string
    SliceStr,
    /// A struct or a pointer to one (both report the struct type).
    Struct(&'static StructDef),
    /// A slice of structs.
    SliceOf(&'static StructDef),
}

pub struct StructDef {
    pub name: &'static str,
    pub fields: &'static [(&'static str, Ty)],
}

struct TypeError {
    value: String,
    ty: String,
    strukt: String,
    field: String,
}

impl TypeError {
    fn message(&self) -> String {
        if !self.strukt.is_empty() || !self.field.is_empty() {
            format!(
                "json: cannot unmarshal {} into Go struct field {}.{} of type {}",
                self.value, self.strukt, self.field, self.ty
            )
        } else {
            format!(
                "json: cannot unmarshal {} into Go value of type {}",
                self.value, self.ty
            )
        }
    }
}

/// segmentio `prefix`: the next 32 bytes of input, then "...".
fn prefix(rest: &[u8]) -> String {
    if rest.len() < 32 {
        String::from_utf8_lossy(rest).into_owned()
    } else {
        format!("{}...", String::from_utf8_lossy(&rest[..32]))
    }
}

fn leaf(node: &Node, doc: &[u8], ty: &str) -> TypeError {
    TypeError {
        value: quote(&prefix(&doc[node.offset..])),
        ty: ty.to_string(),
        strukt: String::new(),
        field: String::new(),
    }
}

fn prepend_field(key: &str, field: &str) -> String {
    if field.is_empty() {
        key.to_string()
    } else {
        format!("{key}.{field}")
    }
}

fn decode(node: &Node, ty: &Ty, doc: &[u8]) -> Result<(), TypeError> {
    if node.is_null() {
        return Ok(());
    }
    match ty {
        Ty::Raw => Ok(()),
        Ty::Str(name) => match node.value {
            Value::String(_) => Ok(()),
            _ => Err(leaf(node, doc, name)),
        },
        Ty::Bool => match node.value {
            Value::Bool(_) => Ok(()),
            _ => Err(leaf(node, doc, "bool")),
        },
        Ty::MapAny(name) => match node.value {
            Value::Object(_) => Ok(()),
            _ => Err(leaf(node, doc, name)),
        },
        Ty::MapStr => {
            let Value::Object(members) = &node.value else {
                return Err(leaf(node, doc, "map[string]string"));
            };
            for (k, v) in members {
                if !v.is_null() && !matches!(v.value, Value::String(_)) {
                    let mut e = leaf(v, doc, "string");
                    e.strukt = format!("map[string]string{}", e.strukt);
                    e.field = prepend_field(k, &e.field);
                    return Err(e);
                }
            }
            Ok(())
        }
        Ty::SliceStr => {
            let Value::Array(items) = &node.value else {
                return Err(leaf(node, doc, "[]string"));
            };
            for (i, item) in items.iter().enumerate() {
                if !item.is_null() && !matches!(item.value, Value::String(_)) {
                    let mut e = leaf(item, doc, "string");
                    e.field = prepend_field(&i.to_string(), &e.field);
                    return Err(e);
                }
            }
            Ok(())
        }
        Ty::SliceOf(def) => {
            let Value::Array(items) = &node.value else {
                return Err(leaf(node, doc, &format!("[]{}", def.name)));
            };
            for (i, item) in items.iter().enumerate() {
                if let Err(mut e) = decode(item, &Ty::Struct(def), doc) {
                    e.field = prepend_field(&i.to_string(), &e.field);
                    return Err(e);
                }
            }
            Ok(())
        }
        Ty::Struct(def) => {
            let Value::Object(members) = &node.value else {
                return Err(leaf(node, doc, def.name));
            };
            for (k, v) in members {
                let lower = k.to_lowercase();
                let field = def.fields.iter().find(|(name, _)| *name == k).or_else(|| {
                    def.fields
                        .iter()
                        .find(|(name, _)| name.to_lowercase() == lower)
                });
                let Some((_, fty)) = field else {
                    continue;
                };
                if let Err(mut e) = decode(v, fty, doc) {
                    e.strukt = format!("{}{}", def.name, e.strukt);
                    e.field = prepend_field(k, &e.field);
                    return Err(e);
                }
            }
            Ok(())
        }
    }
}

const META: Ty = Ty::MapAny("mcp.Meta");

static ICON: StructDef = StructDef {
    name: "mcp.Icon",
    fields: &[
        ("src", Ty::Str("string")),
        ("mimeType", Ty::Str("string")),
        ("sizes", Ty::SliceStr),
        ("theme", Ty::Str("mcp.IconTheme")),
    ],
};
static IMPLEMENTATION: StructDef = StructDef {
    name: "mcp.Implementation",
    fields: &[
        ("name", Ty::Str("string")),
        ("title", Ty::Str("string")),
        ("description", Ty::Str("string")),
        ("version", Ty::Str("string")),
        ("websiteUrl", Ty::Str("string")),
        ("icons", Ty::SliceOf(&ICON)),
    ],
};
static ROOT_CAPABILITIES: StructDef = StructDef {
    name: "mcp.RootCapabilities",
    fields: &[("listChanged", Ty::Bool)],
};
static EMPTY_SAMPLING_CONTEXT: StructDef = StructDef {
    name: "mcp.SamplingContextCapabilities",
    fields: &[],
};
static EMPTY_SAMPLING_TOOLS: StructDef = StructDef {
    name: "mcp.SamplingToolsCapabilities",
    fields: &[],
};
static SAMPLING: StructDef = StructDef {
    name: "mcp.SamplingCapabilities",
    fields: &[
        ("context", Ty::Struct(&EMPTY_SAMPLING_CONTEXT)),
        ("tools", Ty::Struct(&EMPTY_SAMPLING_TOOLS)),
    ],
};
static EMPTY_FORM: StructDef = StructDef {
    name: "mcp.FormElicitationCapabilities",
    fields: &[],
};
static EMPTY_URL: StructDef = StructDef {
    name: "mcp.URLElicitationCapabilities",
    fields: &[],
};
static ELICITATION: StructDef = StructDef {
    name: "mcp.ElicitationCapabilities",
    fields: &[
        ("form", Ty::Struct(&EMPTY_FORM)),
        ("url", Ty::Struct(&EMPTY_URL)),
    ],
};
static CLIENT_CAPABILITIES_V2: StructDef = StructDef {
    name: "mcp.clientCapabilitiesV2",
    fields: &[
        ("experimental", Ty::MapAny("map[string]interface {}")),
        ("extensions", Ty::MapAny("map[string]interface {}")),
        ("roots", Ty::Struct(&ROOT_CAPABILITIES)),
        ("sampling", Ty::Struct(&SAMPLING)),
        ("elicitation", Ty::Struct(&ELICITATION)),
    ],
};
static INITIALIZE_PARAMS_V2: StructDef = StructDef {
    name: "mcp.initializeParamsV2",
    fields: &[
        ("_meta", META),
        ("capabilities", Ty::Struct(&CLIENT_CAPABILITIES_V2)),
        ("clientInfo", Ty::Struct(&IMPLEMENTATION)),
        ("protocolVersion", Ty::Str("string")),
    ],
};
static PING_PARAMS: StructDef = StructDef {
    name: "mcp.PingParams",
    fields: &[("_meta", META)],
};
static LIST_TOOLS_PARAMS: StructDef = StructDef {
    name: "mcp.ListToolsParams",
    fields: &[("_meta", META), ("cursor", Ty::Str("string"))],
};
static LIST_PROMPTS_PARAMS: StructDef = StructDef {
    name: "mcp.ListPromptsParams",
    fields: &[("_meta", META), ("cursor", Ty::Str("string"))],
};
static CALL_TOOL_PARAMS: StructDef = StructDef {
    name: "mcp.CallToolParamsRaw",
    fields: &[
        ("_meta", META),
        ("name", Ty::Str("string")),
        ("arguments", Ty::Raw),
        ("inputResponses", Ty::Raw),
        ("requestState", Ty::Str("string")),
    ],
};
static GET_PROMPT_PARAMS: StructDef = StructDef {
    name: "mcp.GetPromptParams",
    fields: &[
        ("_meta", META),
        ("arguments", Ty::MapStr),
        ("name", Ty::Str("string")),
        ("inputResponses", Ty::Raw),
        ("requestState", Ty::Str("string")),
    ],
};
static COMPLETE_ARGUMENT: StructDef = StructDef {
    name: "mcp.CompleteParamsArgument",
    fields: &[("name", Ty::Str("string")), ("value", Ty::Str("string"))],
};
static COMPLETE_CONTEXT: StructDef = StructDef {
    name: "mcp.CompleteContext",
    fields: &[("arguments", Ty::MapStr)],
};
static COMPLETE_REFERENCE: StructDef = StructDef {
    name: "mcp.CompleteReference",
    fields: &[
        ("type", Ty::Str("string")),
        ("name", Ty::Str("string")),
        ("uri", Ty::Str("string")),
    ],
};
static COMPLETE_PARAMS: StructDef = StructDef {
    name: "mcp.CompleteParams",
    fields: &[
        ("_meta", META),
        ("argument", Ty::Struct(&COMPLETE_ARGUMENT)),
        ("context", Ty::Struct(&COMPLETE_CONTEXT)),
        ("ref", Ty::Struct(&COMPLETE_REFERENCE)),
    ],
};
static NOTIFICATION_SUBSCRIPTIONS: StructDef = StructDef {
    name: "mcp.NotificationSubscriptions",
    fields: &[
        ("toolsListChanged", Ty::Bool),
        ("promptsListChanged", Ty::Bool),
        ("resourcesListChanged", Ty::Bool),
        ("resourceSubscriptions", Ty::SliceStr),
    ],
};
static SUBSCRIPTIONS_LISTEN_PARAMS: StructDef = StructDef {
    name: "mcp.SubscriptionsListenParams",
    fields: &[
        ("_meta", META),
        ("notifications", Ty::Struct(&NOTIFICATION_SUBSCRIPTIONS)),
    ],
};
static SET_LEVEL_PARAMS: StructDef = StructDef {
    name: "mcp.SetLoggingLevelParams",
    fields: &[("_meta", META), ("level", Ty::Str("mcp.LoggingLevel"))],
};

/// `decodeMetaValue`: present, non-null, and decodable as `def`.
fn meta_value_ok(meta: &Node, key: &str, def: &'static StructDef) -> bool {
    match meta.key(key) {
        None => false,
        Some(v) if v.is_null() => false,
        Some(v) => {
            // remarshal: json.Marshal of the decoded value, then decode.
            let doc = v.raw.clone();
            match gojson::parse(&doc) {
                Ok(node) => {
                    decode(&node, &Ty::Struct(def), &doc).is_ok() && node.as_object().is_some()
                }
                Err(_) => false,
            }
        }
    }
}

// --- errors -------------------------------------------------------------------

struct RpcError {
    code: i64,
    message: String,
    data: Option<J>,
}

fn rpc_err(code: i64, message: impl Into<String>) -> RpcError {
    RpcError {
        code,
        message: message.into(),
        data: None,
    }
}

fn error_body(id: &Option<J>, code: i64, message: &str, data: Option<J>) -> Vec<u8> {
    let mut e = Map::new();
    e.insert("code".into(), J::from(code));
    e.insert("message".into(), J::String(message.into()));
    if let Some(d) = data {
        e.insert("data".into(), d);
    }
    let mut envelope = Map::new();
    envelope.insert("jsonrpc".into(), J::String("2.0".into()));
    if let Some(id) = id {
        envelope.insert("id".into(), id.clone());
    }
    envelope.insert("error".into(), J::Object(e));
    go_marshal(&J::Object(envelope))
}

/// `writeJSONRPCError` (no Cache-Control: no stream exists yet).
fn json_error(status: u16, id: &Option<J>, code: i64, message: &str) -> Response {
    let mut r = Response::new(status);
    r.headers.set("Content-Type", "application/json");
    r.body = error_body(id, code, message, None);
    r
}

// --- request guards -----------------------------------------------------------

/// go-sdk `util.IsLoopback`.
pub fn is_loopback_host(addr: &str) -> bool {
    let host = crate::transport::split_host_port(addr).map_or_else(
        || addr.trim_matches(|c| c == '[' || c == ']').to_string(),
        |(h, _)| h,
    );
    if host == "localhost" {
        return true;
    }
    host.parse::<std::net::IpAddr>().is_ok_and(|ip| match ip {
        std::net::IpAddr::V4(v4) => v4.is_loopback(),
        std::net::IpAddr::V6(v6) => v6.is_loopback(),
    })
}

fn accepts(values: &[String]) -> (bool, bool) {
    let (mut json_ok, mut stream_ok) = (false, false);
    for value in values {
        for raw in value.split(',') {
            let base = raw
                .trim()
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_lowercase();
            match base.as_str() {
                "application/json" | "application/*" => json_ok = true,
                "text/event-stream" | "text/*" => stream_ok = true,
                "*/*" => {
                    json_ok = true;
                    stream_ok = true;
                }
                _ => {}
            }
        }
    }
    (json_ok, stream_ok)
}

// --- JSON-RPC decoding ----------------------------------------------------------

struct Message {
    method: String,
    has_method: bool,
    id: Option<J>,
    params: Vec<u8>,
}

/// jsonrpc2 `DecodeMessage`.
fn decode_message(node: &Node, doc: &[u8]) -> Result<Message, String> {
    if node.as_object().is_none() {
        let e = leaf(node, doc, "jsonrpc2.wireDecode");
        return Err(format!("unmarshaling jsonrpc message: {}", e.message()));
    }
    let version = match node.field("jsonrpc") {
        None => String::new(),
        Some(n) => match &n.value {
            Value::String(s) => s.clone(),
            Value::Null => String::new(),
            _ => {
                let mut e = leaf(n, doc, "string");
                e.strukt = "jsonrpc2.wireDecode".into();
                e.field = "jsonrpc".into();
                return Err(format!("unmarshaling jsonrpc message: {}", e.message()));
            }
        },
    };
    if version != "2.0" {
        return Err(format!(
            "invalid message version tag {}; expected \"2.0\"",
            quote(&version)
        ));
    }
    let id = match node.field("id") {
        None => None,
        Some(n) => match &n.value {
            Value::Null => None,
            Value::String(s) => Some(J::String(s.clone())),
            Value::Number(text) => Some(J::from(text.parse::<f64>().unwrap_or(0.0) as i64)),
            Value::Bool(_) => return Err("parse error: invalid ID type bool".into()),
            Value::Object(_) => {
                return Err("parse error: invalid ID type map[string]interface {}".into())
            }
            Value::Array(_) => return Err("parse error: invalid ID type []interface {}".into()),
        },
    };
    let (method, has_method) = match node.field("method").map(|n| &n.value) {
        Some(Value::String(s)) => (s.clone(), !s.is_empty()),
        _ => (String::new(), false),
    };
    let params = node
        .field("params")
        .map(|n| n.raw.clone())
        .unwrap_or_default();
    Ok(Message {
        method,
        has_method,
        id,
        params,
    })
}

#[derive(Clone, Copy)]
struct MethodInfo {
    notification: bool,
    missing_params_ok: bool,
}

/// `serverMethodInfos` as registered for this server.
fn method_info(method: &str) -> Option<MethodInfo> {
    let (notification, missing_params_ok) = match method {
        "completion/complete"
        | "initialize"
        | "prompts/get"
        | "tools/call"
        | "resources/read"
        | "logging/setLevel"
        | "resources/subscribe"
        | "subscriptions/listen"
        | "resources/unsubscribe" => (false, false),
        "server/discover"
        | "ping"
        | "prompts/list"
        | "tools/list"
        | "resources/list"
        | "resources/templates/list" => (false, true),
        "notifications/cancelled"
        | "notifications/initialized"
        | "notifications/roots/list_changed" => (true, true),
        "notifications/progress" => (true, false),
        _ => return None,
    };
    Some(MethodInfo {
        notification,
        missing_params_ok,
    })
}

fn request_meta(params: &[u8]) -> Option<Node> {
    let n = gojson::parse(params).ok()?;
    n.as_object()?;
    let meta = n.field("_meta")?;
    meta.as_object()?;
    Some(meta.clone())
}

// --- the handler ------------------------------------------------------------------

/// `StreamableHTTPHandler.ServeHTTP` for a POST the agent has already read.
pub fn serve(sdk: &Sdk, req: &Request, raw: &[u8], protected: bool) -> Response {
    if protected && is_loopback_host(&req.local_addr.to_string()) && !is_loopback_host(&req.host) {
        return Response::error(
            &format!("Forbidden: invalid Host header {}", quote(&req.host)),
            403,
        );
    }
    let header_version = req.headers.get("Mcp-Protocol-Version").to_string();
    if !header_version.is_empty()
        && !SUPPORTED_VERSIONS.contains(&header_version.as_str())
        && header_version.as_str() < MODERN_VERSION
    {
        return Response::error(
            &format!(
                "Bad Request: Unsupported protocol version (supported versions: {})",
                SUPPORTED_VERSIONS.join(",")
            ),
            400,
        );
    }
    if parse_media_type(req.headers.get("Content-Type")).as_deref() != Some("application/json") {
        return Response::error("Content-Type must be 'application/json'", 415);
    }
    let (json_ok, stream_ok) = accepts(req.headers.values("Accept"));
    if !json_ok || !stream_ok {
        return Response::error(
            "Accept must contain both 'application/json' and 'text/event-stream'",
            400,
        );
    }
    if raw.len() as u64 > MAX_BODY {
        return Response::error(&format!("request body exceeds {MAX_BODY} bytes"), 413);
    }
    let effective_version = if header_version.is_empty() {
        "2025-03-26".to_string()
    } else {
        header_version.clone()
    };
    let new_protocol = header_version.as_str() >= MODERN_VERSION;
    let doc = match gojson::parse(raw) {
        Ok(d) => d,
        Err(_) => return Response::error("malformed payload: invalid character", 400),
    };
    if let Value::Array(_) = doc.value {
        if effective_version.as_str() >= "2025-06-18" {
            return Response::error(
                &format!(
                    "JSON-RPC batching is not supported in 2025-06-18 and later (request version: {effective_version})"
                ),
                400,
            );
        }
    }
    let msg = match decode_message(&doc, raw) {
        Ok(m) => m,
        Err(e) => return Response::error(&format!("malformed payload: {e}"), 400),
    };
    if !msg.has_method {
        // A response or an empty method: nothing to dispatch.
        return Response::new(202);
    }
    let is_call = msg.id.is_some();
    let Some(info) = method_info(&msg.method) else {
        let text = format!("JSON RPC not handled: {} unsupported", quote(&msg.method));
        if new_protocol && is_call {
            return json_error(404, &msg.id, -32601, &text);
        }
        return Response::error(&text, 400);
    };
    if info.notification && is_call {
        return Response::error(
            &format!("invalid request: unexpected id for {}", quote(&msg.method)),
            400,
        );
    }
    if !info.notification && !is_call {
        return Response::error(
            &format!("invalid request: missing id for {}", quote(&msg.method)),
            400,
        );
    }
    if !info.missing_params_ok && msg.params.is_empty() {
        return Response::error("invalid request: missing required \"params\"", 400);
    }
    let meta_version = request_meta(&msg.params)
        .and_then(|m| {
            m.key(META_VERSION)
                .and_then(|v| v.as_str().map(String::from))
        })
        .unwrap_or_default();
    if new_protocol || !meta_version.is_empty() {
        if header_version.is_empty() {
            return json_error(
                400,
                &msg.id,
                -32020,
                &format!(
                    "Mcp-Protocol-Version header is required for requests carrying {}",
                    quote(META_VERSION)
                ),
            );
        }
        if meta_version.is_empty() {
            return json_error(
                400,
                &msg.id,
                -32602,
                &format!("missing or invalid _meta field {}", quote(META_VERSION)),
            );
        }
        if header_version != meta_version {
            return json_error(
                400,
                &msg.id,
                -32020,
                &format!(
                    "Mcp-Protocol-Version header {} does not match request {} {}",
                    quote(&header_version),
                    META_VERSION,
                    quote(&meta_version)
                ),
            );
        }
    }
    if let Err(text) = validate_mcp_headers(req, &header_version, &msg) {
        return json_error(400, &msg.id, -32020, &text);
    }
    if !is_call {
        return Response::new(202);
    }
    let (status, body) = match handle(sdk, &msg) {
        Ok(result) => (
            200,
            go_marshal(
                &json!({"jsonrpc": "2.0", "id": msg.id.clone().unwrap_or(J::Null), "result": result}),
            ),
        ),
        Err(e) => {
            let status = if new_protocol {
                match e.code {
                    -32601 => 404,
                    -32602 | -32022 | -32021 => 400,
                    _ => 200,
                }
            } else {
                200
            };
            (status, error_body(&msg.id, e.code, &e.message, e.data))
        }
    };
    let mut r = Response::new(status);
    r.headers.set("Cache-Control", "no-cache, no-transform");
    r.headers.set("Content-Type", "application/json");
    r.body = body;
    r
}

/// `validateMcpHeaders` (raw header values; no RFC 2047 decoding).
fn validate_mcp_headers(req: &Request, version: &str, msg: &Message) -> Result<(), String> {
    if version.is_empty() || version < MODERN_VERSION {
        return Ok(());
    }
    let in_header = req.headers.get("Mcp-Method");
    if in_header.is_empty() {
        return Err("missing required Mcp-Method header".into());
    }
    if in_header != msg.method {
        return Err(format!(
            "header mismatch: Mcp-Method header value '{in_header}' does not match body value '{}'",
            msg.method
        ));
    }
    let (def, field): (&StructDef, &str) = match msg.method.as_str() {
        "tools/call" => (&CALL_TOOL_PARAMS, "name"),
        "prompts/get" => (&GET_PROMPT_PARAMS, "name"),
        "resources/read" => return Ok(()),
        _ => return Ok(()),
    };
    let name_header = req.headers.get("Mcp-Name");
    if name_header.is_empty() {
        return Err(format!(
            "missing required Mcp-Name header for method {}",
            quote(&msg.method)
        ));
    }
    // extractName: a full typed decode of the params.
    let parsed = gojson::parse(&msg.params).ok();
    let decoded_ok = parsed
        .as_ref()
        .is_some_and(|p| decode(p, &Ty::Struct(def), &msg.params).is_ok());
    if !decoded_ok {
        return Err(format!(
            "failed to extract name from parameters for method {}",
            quote(&msg.method)
        ));
    }
    let name = parsed
        .as_ref()
        .and_then(|p| p.field(field))
        .and_then(|n| n.as_str())
        .unwrap_or("")
        .to_string();
    if name_header != name {
        return Err(format!(
            "header mismatch: Mcp-Name header value '{name_header}' does not match body value '{name}'"
        ));
    }
    Ok(())
}

/// `unmarshalParams` + `handleReceive`'s error wrapping.
fn unmarshal_params(
    method: &str,
    raw: &[u8],
    def: &'static StructDef,
    missing_ok: bool,
) -> Result<Option<Node>, RpcError> {
    let wrap = |e: RpcError| RpcError {
        code: e.code,
        message: format!("handling '{method}': {}", e.message),
        data: e.data,
    };
    if raw.is_empty() || raw == b"null" {
        if missing_ok {
            return Ok(None);
        }
        if method == "initialize" {
            return Err(wrap(rpc_err(0, "missing required \"params\"")));
        }
        return Err(wrap(rpc_err(
            -32600,
            "invalid request: missing required \"params\"",
        )));
    }
    let node = gojson::parse(raw).map_err(|_| wrap(rpc_err(-32602, "invalid params")))?;
    if let Err(e) = decode(&node, &Ty::Struct(def), raw) {
        let text = String::from_utf8_lossy(raw).into_owned();
        let message = format!(
            "unmarshaling {} into a *{}: {}",
            quote(&text),
            def.name,
            e.message()
        );
        if method == "initialize" {
            return Err(wrap(rpc_err(0, message)));
        }
        return Err(wrap(rpc_err(-32602, format!("invalid params: {message}"))));
    }
    Ok(Some(node))
}

/// The canonical gob encoding of `pageToken{LastUID}` that `encodeCursor`
/// produces; any other cursor fails `decodeCursor`.
fn decode_cursor(cursor: &str) -> Option<String> {
    use base64::Engine;
    const PREFIX: [u8; 35] = [
        0x22, 0x7f, 0x03, 0x01, 0x01, 0x09, 0x70, 0x61, 0x67, 0x65, 0x54, 0x6f, 0x6b, 0x65, 0x6e,
        0x01, 0xff, 0x80, 0x00, 0x01, 0x01, 0x01, 0x07, 0x4c, 0x61, 0x73, 0x74, 0x55, 0x49, 0x44,
        0x01, 0x0c, 0x00, 0x00, 0x00,
    ];
    let bytes = base64::engine::general_purpose::URL_SAFE
        .decode(cursor)
        .ok()?;
    let rest = bytes.strip_prefix(&PREFIX[..])?;
    let (len, rest) = gob_uint(rest)?;
    if rest.len() as u64 != len {
        return None;
    }
    let rest = rest.strip_prefix(&[0xff, 0x80][..])?;
    if rest == [0x00] {
        return Some(String::new());
    }
    let rest = rest.strip_prefix(&[0x01][..])?;
    let (n, rest) = gob_uint(rest)?;
    let (s, tail) = rest.split_at_checked(n as usize)?;
    if tail != [0x00] {
        return None;
    }
    String::from_utf8(s.to_vec()).ok()
}

fn gob_uint(b: &[u8]) -> Option<(u64, &[u8])> {
    let first = *b.first()?;
    if first < 0x80 {
        return Some((first as u64, &b[1..]));
    }
    let n = (!first).wrapping_add(1) as usize;
    if n > 8 || b.len() < 1 + n {
        return None;
    }
    let v = b[1..=n].iter().fold(0u64, |acc, x| acc << 8 | *x as u64);
    Some((v, &b[1 + n..]))
}

/// `ServerSession.handle` and the registered method handlers.
fn handle(sdk: &Sdk, msg: &Message) -> Result<J, RpcError> {
    let meta = request_meta(&msg.params);
    let mut uses_new = false;
    let mut meta_version = String::new();
    if let Some(meta) = &meta {
        if let Some(v) = meta.key(META_VERSION).and_then(|v| v.as_str()) {
            if v >= MODERN_VERSION {
                uses_new = true;
                meta_version = v.to_string();
                if meta.key(META_CLIENT_INFO).is_some()
                    && !meta_value_ok(meta, META_CLIENT_INFO, &IMPLEMENTATION)
                {
                    return Err(rpc_err(
                        -32602,
                        format!("invalid _meta field {}", quote(META_CLIENT_INFO)),
                    ));
                }
                if !meta_value_ok(meta, META_CAPABILITIES, &CLIENT_CAPABILITIES_V2) {
                    return Err(rpc_err(
                        -32602,
                        format!(
                            "missing or invalid _meta field {}",
                            quote(META_CAPABILITIES)
                        ),
                    ));
                }
            }
        }
    }
    if uses_new && !SUPPORTED_VERSIONS.contains(&meta_version.as_str()) {
        return Err(RpcError {
            code: -32022,
            message: "unsupported protocol version".into(),
            data: Some(json!({"supported": SUPPORTED_VERSIONS, "requested": meta_version})),
        });
    }
    // jsonrpc2 rewrites every method-not-found error to this text.
    let not_found = || rpc_err(-32601, format!("method not found: {}", quote(&msg.method)));
    if uses_new
        && matches!(
            msg.method.as_str(),
            "initialize"
                | "ping"
                | "logging/setLevel"
                | "resources/subscribe"
                | "resources/unsubscribe"
        )
    {
        return Err(not_found());
    }
    let raw = &msg.params;
    let mut result = match msg.method.as_str() {
        "initialize" => {
            let params = unmarshal_params("initialize", raw, &INITIALIZE_PARAMS_V2, false)?;
            let requested = params
                .as_ref()
                .and_then(|p| p.field("protocolVersion"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let negotiated = if SUPPORTED_VERSIONS.contains(&requested.as_str())
                && requested.as_str() < MODERN_VERSION
            {
                requested
            } else {
                "2025-11-25".to_string()
            };
            return Ok(json!({
                "capabilities": {"resources": {"listChanged": true}, "tools": {"listChanged": true}},
                "protocolVersion": negotiated,
                "serverInfo": {"name": sdk.implementation_name, "version": sdk.version},
            }));
        }
        "ping" => {
            unmarshal_params("ping", raw, &PING_PARAMS, true)?;
            return Ok(json!({}));
        }
        "prompts/list" => {
            let params = unmarshal_params("prompts/list", raw, &LIST_PROMPTS_PARAMS, true)?;
            check_cursor(params.as_ref())?;
            json!({"prompts": [], "ttlMs": 0, "cacheScope": "public"})
        }
        "tools/list" => {
            let params = unmarshal_params("tools/list", raw, &LIST_TOOLS_PARAMS, true)?;
            let after = check_cursor(params.as_ref())?;
            let tools: Vec<J> = sdk
                .tools
                .iter()
                .filter(|t| {
                    after
                        .as_deref()
                        .is_none_or(|a| t.get("name").and_then(J::as_str).is_some_and(|n| n > a))
                })
                .cloned()
                .collect();
            json!({"tools": tools, "ttlMs": 0, "cacheScope": "public"})
        }
        "tools/call" => {
            let params = unmarshal_params("tools/call", raw, &CALL_TOOL_PARAMS, false)?;
            let name = params
                .as_ref()
                .and_then(|p| p.field("name"))
                .and_then(|n| n.as_str())
                .unwrap_or("");
            return Err(rpc_err(-32602, format!("unknown tool {}", quote(name))));
        }
        "prompts/get" => {
            let params = unmarshal_params("prompts/get", raw, &GET_PROMPT_PARAMS, false)?;
            let name = params
                .as_ref()
                .and_then(|p| p.field("name"))
                .and_then(|n| n.as_str())
                .unwrap_or("");
            return Err(rpc_err(-32602, format!("unknown prompt {}", quote(name))));
        }
        "completion/complete" => {
            let params = unmarshal_params("completion/complete", raw, &COMPLETE_PARAMS, false)?;
            let has_ref = params
                .as_ref()
                .and_then(|p| p.field("ref"))
                .is_some_and(|r| !r.is_null());
            if !has_ref {
                return Err(rpc_err(
                    -32602,
                    "invalid params: missing required 'ref' field",
                ));
            }
            return Err(not_found());
        }
        "subscriptions/listen" => {
            let params = unmarshal_params(
                "subscriptions/listen",
                raw,
                &SUBSCRIPTIONS_LISTEN_PARAMS,
                false,
            )?;
            let has = params
                .as_ref()
                .and_then(|p| p.field("notifications"))
                .is_some_and(|r| !r.is_null());
            if !has {
                return Err(rpc_err(
                    -32602,
                    "invalid params: missing required 'notifications' field",
                ));
            }
            // A listen stream is long-lived server-sent events; it is not
            // part of this build's surface yet.
            return Err(not_found());
        }
        "logging/setLevel" => {
            unmarshal_params("logging/setLevel", raw, &SET_LEVEL_PARAMS, false)?;
            return Ok(json!({}));
        }
        _ => return Err(not_found()),
    };
    if uses_new {
        if let Some(obj) = result.as_object_mut() {
            obj.insert("resultType".into(), J::String("complete".into()));
            obj.insert(
                "_meta".into(),
                json!({META_SERVER_INFO: {"name": sdk.implementation_name, "version": sdk.version}}),
            );
        }
    }
    Ok(result)
}

/// `paginateList` cursor handling: `Some(last)` to resume after `last`.
fn check_cursor(params: Option<&Node>) -> Result<Option<String>, RpcError> {
    let cursor = params
        .and_then(|p| p.field("cursor"))
        .and_then(|c| c.as_str())
        .unwrap_or("");
    if cursor.is_empty() {
        return Ok(None);
    }
    decode_cursor(cursor)
        .map(Some)
        .ok_or_else(|| rpc_err(-32602, "invalid params"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_round_trips_go_encodings() {
        assert_eq!(
            decode_cursor("In8DAQEJcGFnZVRva2VuAf-AAAEBAQdMYXN0VUlEAQwAAAAD_4AA").as_deref(),
            Some("")
        );
        assert_eq!(
            decode_cursor("In8DAQEJcGFnZVRva2VuAf-AAAEBAQdMYXN0VUlEAQwAAAAI_4ABA2FiYwA=")
                .as_deref(),
            Some("abc")
        );
        assert_eq!(decode_cursor("bogus"), None);
    }

    #[test]
    fn segmentio_messages() {
        let raw = br#"{"protocolVersion":"x","clientInfo":1}"#;
        let node = gojson::parse(raw).unwrap();
        let e = decode(&node, &Ty::Struct(&INITIALIZE_PARAMS_V2), raw).unwrap_err();
        assert_eq!(
            e.message(),
            r#"json: cannot unmarshal "1}" into Go struct field mcp.initializeParamsV2.clientInfo of type mcp.Implementation"#
        );
        let raw = b"[]";
        let node = gojson::parse(raw).unwrap();
        let e = decode(&node, &Ty::Struct(&PING_PARAMS), raw).unwrap_err();
        assert_eq!(
            e.message(),
            r#"json: cannot unmarshal "[]" into Go value of type mcp.PingParams"#
        );
    }
}
