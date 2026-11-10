//! What the Go provider executables share beyond the SDK: `mcphttp`'s
//! provider handler wrapper, `encoding/json` arguments as `map[string]any`,
//! the Host Agent callback client (`pkg/hostagentclient`), and `net/http`'s
//! client with Go's proxy rules and error text.

use crate::gojson::{float_value, Node, Value};
use crate::http1::{Request, Response};
use crate::plan::runner::RunCtx;
use crate::provider::adapter::Client;
use serde_json::{json, Map, Value as J};
use std::io::Read;
use std::time::Duration;

// --- mcphttp.WrapProviderHandler ----------------------------------------------

/// `mcphttp.WrapProviderHandler`: GET and DELETE on `/mcp` or `/` are refused,
/// and a POSTed `initialize`, `notifications/initialized` or `server/discover`
/// is answered before the SDK sees it. `None` hands the request on.
pub(super) fn wrap_provider_handler(info: &J, req: &Request, raw: &[u8]) -> Option<Response> {
    if req.method == "GET" || req.method == "DELETE" {
        let path = req.escaped_path.as_str();
        if path == "/mcp" || path == "/" || path.is_empty() {
            return Some(Response::error("method not allowed", 405));
        }
    }
    if req.method != "POST" {
        return None;
    }
    // json.Unmarshal into struct{ID any; Method string}: keys match without
    // case, the last one wins, and any decode error hands the request on.
    let doc = crate::gojson::parse(raw).ok()?;
    let entries = doc.as_object()?;
    let mut id = J::Null;
    let mut method = String::new();
    for (key, value) in entries {
        if crate::gojson::equal_fold(key, "id") {
            id = go_any(value);
        } else if crate::gojson::equal_fold(key, "method") {
            match &value.value {
                Value::String(s) => method = s.clone(),
                Value::Null => {}
                _ => return None,
            }
        }
    }
    let encode = |body: J| {
        let mut out = String::new();
        crate::gojson::encode(&body, &mut out);
        crate::gojson::html_escape_json_bytes(out.into_bytes())
    };
    if method == "initialize" || method == "notifications/initialized" {
        return Some(Response::json(
            404,
            encode(json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {"code": -32601, "message": format!("Method not found: {method}"),
                          "data": {"supported": ["2026-07-28"]}},
            })),
        ));
    }
    if method == "server/discover" {
        return Some(Response::json(
            200,
            encode(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "resultType": "complete",
                    "supportedVersions": ["2026-07-28"],
                    "capabilities": {"tools": {}, "extensions": {"io.modelcontextprotocol/tasks": {}}},
                    "_meta": {"io.modelcontextprotocol/serverInfo": info},
                },
            })),
        ));
    }
    None
}

// --- encoding/json values ----------------------------------------------------------

/// A JSON value as `encoding/json` decodes it into `any`: numbers become
/// float64 and a repeated key keeps its last value.
pub fn go_any(node: &Node) -> J {
    match &node.value {
        Value::Null => J::Null,
        Value::Bool(b) => J::Bool(*b),
        Value::Number(text) => float_value(text.parse::<f64>().unwrap_or(0.0)),
        Value::String(s) => J::String(s.clone()),
        Value::Array(items) => J::Array(items.iter().map(go_any).collect()),
        Value::Object(entries) => {
            let mut map = Map::new();
            for (k, v) in entries {
                map.insert(k.clone(), go_any(v));
            }
            J::Object(map)
        }
    }
}

/// A Go runtime panic the handler does not recover: the process ends with
/// Go's panic line and exit status 2.
pub fn go_panic(message: &str) -> ! {
    eprintln!("panic: {message}\n\ngoroutine 1 [running]:");
    std::process::exit(2)
}

/// A tool call's `map[string]any` arguments. A JSON `null` decodes to a nil
/// map: reads see it empty, and a write panics as it does in Go.
#[derive(Clone, Default)]
pub struct Args {
    pub map: Map<String, J>,
    pub nil: bool,
}

impl Args {
    /// `requestArguments`: absent arguments are an empty map.
    pub fn decode(arguments: Option<&Node>, prefix: &str) -> Result<Args, String> {
        let Some(node) = arguments.filter(|n| !n.raw.is_empty()) else {
            return Ok(Args::default());
        };
        match &node.value {
            Value::Null => Ok(Args {
                map: Map::new(),
                nil: true,
            }),
            Value::Object(_) => Ok(Args {
                map: go_any(node).as_object().cloned().unwrap_or_default(),
                nil: false,
            }),
            other => Err(format!(
                "{prefix}json: cannot unmarshal {} into Go value of type map[string]interface {{}}",
                crate::gojson::kind_name(other)
            )),
        }
    }

    pub fn from_map(map: Map<String, J>) -> Args {
        Args { map, nil: false }
    }

    /// `cloneMap`: a copy, never nil.
    pub fn cloned(&self) -> Args {
        Args::from_map(self.map.clone())
    }

    pub fn get(&self, key: &str) -> Option<&J> {
        self.map.get(key)
    }

    pub fn set(&mut self, key: &str, value: J) {
        if self.nil {
            go_panic("assignment to entry in nil map");
        }
        self.map.insert(key.into(), value);
    }

    /// `stringInput`: a non-blank string, trimmed, else the fallback.
    pub fn string(&self, key: &str, fallback: &str) -> String {
        string_input(&self.map, key, fallback)
    }

    /// `boolInput`.
    pub fn bool(&self, key: &str, fallback: bool) -> bool {
        self.map.get(key).and_then(J::as_bool).unwrap_or(fallback)
    }

    /// `intInput`: a float64 truncated toward zero.
    pub fn int(&self, key: &str, fallback: i64) -> i64 {
        self.map
            .get(key)
            .and_then(J::as_f64)
            .map_or(fallback, |f| f as i64)
    }

    /// `stringSliceInput`.
    pub fn strings(&self, key: &str) -> Vec<String> {
        string_slice_input(&self.map, key)
    }
}

/// `stringInput` over any map.
pub fn string_input(map: &Map<String, J>, key: &str, fallback: &str) -> String {
    match map.get(key).and_then(J::as_str) {
        Some(value) if !value.trim().is_empty() => value.trim().to_string(),
        _ => fallback.to_string(),
    }
}

/// `stringSliceInput` over any map.
pub fn string_slice_input(map: &Map<String, J>, key: &str) -> Vec<String> {
    map.get(key)
        .and_then(J::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(J::as_str)
                .filter(|s| !s.trim().is_empty())
                .map(|s| s.trim().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// `firstNonEmpty`: the first non-blank value, trimmed.
pub fn first_non_empty(values: &[&str]) -> String {
    values
        .iter()
        .map(|v| v.trim())
        .find(|v| !v.is_empty())
        .unwrap_or("")
        .to_string()
}

/// `numericInput`: a number truncated to int, and whether the key held any
/// non-null value.
pub fn numeric_input(map: &Map<String, J>, key: &str) -> (i64, bool) {
    match map.get(key) {
        Some(J::Number(n)) => (n.as_f64().map_or(0, |f| f as i64), true),
        Some(J::Null) | None => (0, false),
        Some(_) => (0, true),
    }
}

/// Run a future to completion on the calling (blocking) thread.
pub fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("start runtime")
        .block_on(future)
}

/// The teardown tool's own decode: `phase` and `inputs`, matched without
/// case; a mistyped field is Go's `UnmarshalTypeError`.
pub fn teardown_input(
    arguments: Option<&crate::gojson::Node>,
) -> Result<(String, Map<String, J>), String> {
    use crate::gojson::Value;
    let mut phase = String::new();
    let mut inputs = Map::new();
    // Absent arguments are an empty `json.RawMessage`, which Unmarshal refuses.
    let Some(node) = arguments.filter(|n| !n.raw.is_empty()) else {
        return Err("unexpected end of JSON input".into());
    };
    let entries = match &node.value {
        Value::Null => return Ok((phase, inputs)),
        Value::Object(entries) => entries,
        other => {
            return Err(format!(
                "json: cannot unmarshal {} into Go value of type struct {{ Phase string \"json:\\\"phase\\\"\"; Inputs map[string]interface {{}} \"json:\\\"inputs\\\"\" }}",
                crate::gojson::kind_name(other)
            ))
        }
    };
    let mut first_error = None;
    for (key, item) in entries {
        if crate::gojson::equal_fold(key, "phase") {
            match &item.value {
                Value::Null => {}
                Value::String(s) => phase = s.clone(),
                other => {
                    first_error.get_or_insert_with(|| {
                        format!(
                            "json: cannot unmarshal {} into Go struct field .phase of type string",
                            crate::gojson::kind_name(other)
                        )
                    });
                }
            }
        } else if crate::gojson::equal_fold(key, "inputs") {
            match &item.value {
                Value::Null => inputs = Map::new(),
                Value::Object(_) => {
                    // Decoding into an existing map merges keys.
                    for (k, v) in go_any(item).as_object().cloned().unwrap_or_default() {
                        inputs.insert(k, v);
                    }
                }
                other => {
                    first_error.get_or_insert_with(|| {
                        format!("json: cannot unmarshal {} into Go struct field .inputs of type map[string]interface {{}}", crate::gojson::kind_name(other))
                    });
                }
            }
        }
    }
    first_error.map_or(Ok((phase, inputs)), Err)
}

// --- pkg/hostagentclient ------------------------------------------------------------

/// `hostagentclient.Client`: modern-only MCP calls to the Host Agent, each
/// carrying the calling request's resource delegation.
pub struct HostAgent {
    client: Client,
    delegation: String,
}

impl HostAgent {
    /// `hostagentclient.Connect`: the endpoint (default the local agent) and
    /// one `server/discover` to prove it answers.
    pub fn connect(endpoint: &str, token: &str, delegation: &str) -> Result<HostAgent, String> {
        let mut endpoint = endpoint.trim().to_string();
        if endpoint.is_empty() {
            endpoint = "http://127.0.0.1:3014/mcp".into();
        }
        let client = Client::new(
            endpoint,
            token.to_string(),
            "opute-host-agent-client".into(),
            "0.1.0".into(),
        )?;
        let agent = HostAgent {
            client,
            delegation: delegation.trim().to_string(),
        };
        block_on(agent.client.call(
            &RunCtx::background(),
            "server/discover",
            "",
            Map::new(),
            &agent.delegation,
        ))
        .map_err(|e| format!("connect host agent: {e}"))?;
        Ok(agent)
    }

    /// `Client.Call`: the tool result as a `CallToolResult` map.
    pub fn call(&self, name: &str, arguments: Map<String, J>) -> Result<Map<String, J>, String> {
        block_on(self.client.call_tool(
            &RunCtx::background(),
            name,
            Some(arguments),
            &self.delegation,
        ))
    }

    /// `callHost`: a callback whose error result is an error.
    pub fn call_host(&self, name: &str, arguments: J) -> Result<Map<String, J>, String> {
        let arguments = arguments.as_object().cloned().unwrap_or_default();
        let result = self
            .call(name, arguments)
            .map_err(|e| format!("host callback {name}: {e}"))?;
        if result.get("isError").and_then(J::as_bool) == Some(true) {
            let detail = result
                .get("content")
                .and_then(J::as_array)
                .into_iter()
                .flatten()
                .filter(|c| c.get("type").and_then(J::as_str) == Some("text"))
                .filter_map(|c| c.get("text").and_then(J::as_str))
                .map(str::trim)
                .find(|t| !t.is_empty())
                .unwrap_or("host callback returned an error result")
                .to_string();
            return Err(format!("host callback {name} failed: {detail}"));
        }
        Ok(result)
    }
}

// --- net/http client ----------------------------------------------------------------

/// A completed HTTP exchange: Go's `StatusCode`, `Status` and the body.
pub struct HttpResponse {
    pub status: u16,
    pub status_line: String,
    pub body: Vec<u8>,
}

/// One request as Go's `http.Client` sends it.
pub struct HttpRequest<'a> {
    pub method: &'a str,
    pub url: &'a str,
    pub headers: Vec<(&'a str, String)>,
    pub body: Option<Vec<u8>>,
    /// `http.Client.Timeout`; none for `http.DefaultClient`.
    pub timeout: Option<Duration>,
    /// Refuse a redirect to a non-HTTPS URL with this error.
    pub https_redirects: Option<&'a str>,
    /// Read at most this many body bytes (`io.LimitReader`).
    pub limit: u64,
}

fn env(key: &str) -> String {
    std::env::var(key).unwrap_or_default()
}

/// `httpproxy.FromEnvironment` + `ProxyFunc`: the proxy URL Go uses for
/// `url`, if any. Loopback and `localhost` are never proxied.
/// Go's `http.ProxyFromEnvironment` (`httpproxy.Config.ProxyFunc`): the
/// proxy for a request, never for localhost or a loopback address.
pub(crate) fn proxy_for(scheme: &str, host: &str, port: u16) -> Option<String> {
    let pick = |upper: &str, lower: &str| {
        let v = env(upper);
        if v.is_empty() {
            env(lower)
        } else {
            v
        }
    };
    select_proxy(
        scheme,
        host,
        port,
        &pick("HTTP_PROXY", "http_proxy"),
        &pick("HTTPS_PROXY", "https_proxy"),
        &pick("NO_PROXY", "no_proxy"),
    )
}

/// `proxy_for` over explicit settings.
fn select_proxy(
    scheme: &str,
    host: &str,
    port: u16,
    http_proxy: &str,
    https_proxy: &str,
    no_proxy: &str,
) -> Option<String> {
    let proxy = if scheme == "https" {
        https_proxy.to_string()
    } else {
        http_proxy.to_string()
    };
    if proxy.is_empty() {
        return None;
    }
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    let ip = bare.parse::<std::net::IpAddr>().ok();
    if bare.eq_ignore_ascii_case("localhost") || ip.is_some_and(|ip| ip.is_loopback()) {
        return None;
    }
    let host = bare.trim().to_ascii_lowercase();
    let port = port.to_string();
    for entry in no_proxy.split(',') {
        let entry = entry.trim().to_ascii_lowercase();
        if entry.is_empty() {
            continue;
        }
        if entry == "*" {
            return None;
        }
        // IPv4/CIDR, IPv6/CIDR: for an IP host only.
        if let Some((net, bits)) = entry.split_once('/') {
            if let (Ok(net), Ok(bits)) = (net.parse::<std::net::IpAddr>(), bits.parse::<u32>()) {
                if ip.is_some_and(|ip| in_cidr(ip, net, bits)) {
                    return None;
                }
                continue;
            }
        }
        // `host:port`, `[v6]:port`, or the whole entry (`net.SplitHostPort`).
        let (name, entry_port) = split_host_port(&entry);
        if name.is_empty() {
            continue;
        }
        let port_matches = entry_port.is_none_or(|p| p == port);
        // An IP entry matches an IP host by value.
        if let Ok(entry_ip) = name.parse::<std::net::IpAddr>() {
            if ip.is_some_and(|ip| same_ip(ip, entry_ip)) && port_matches {
                return None;
            }
            continue;
        }
        // A name never matches an IP host.
        if ip.is_some() {
            continue;
        }
        // `*.name` and `.name` match subdomains only; `name` also itself.
        let name = name
            .strip_prefix('*')
            .filter(|n| n.starts_with('.'))
            .unwrap_or(name);
        let (suffix, match_host) = match name.strip_prefix('.') {
            Some(_) => (name.to_string(), false),
            None => (format!(".{name}"), true),
        };
        if (host.ends_with(&suffix) || (match_host && host == suffix[1..])) && port_matches {
            return None;
        }
    }
    Some(if proxy.contains("://") {
        proxy
    } else {
        format!("http://{proxy}")
    })
}

/// Go's `net.SplitHostPort` as `config.init` uses it on a NO_PROXY entry:
/// `host:port` or `[v6]:port`, else the whole entry as the host.
fn split_host_port(entry: &str) -> (&str, Option<&str>) {
    if let Some(rest) = entry.strip_prefix('[') {
        if let Some((host, port)) = rest.split_once("]:") {
            return (host, Some(port));
        }
        return (entry, None);
    }
    match entry.split_once(':') {
        Some((host, port)) if !port.contains(':') => (host, Some(port)),
        _ => (entry, None),
    }
}

/// `net.IP.Equal`: an IPv4 address equals its IPv4-mapped IPv6 form.
fn same_ip(a: std::net::IpAddr, b: std::net::IpAddr) -> bool {
    a.to_canonical() == b.to_canonical()
}

/// D28 (Q7): `builder` with the proxy Go's default transport would use for
/// `url`, if any. ureq picks one proxy per agent, so a redirect to another
/// host keeps the first host's choice.
pub(crate) fn env_proxy(builder: ureq::AgentBuilder, url: &str) -> ureq::AgentBuilder {
    let Some((scheme, authority)) = crate::hostread::http_authority(url) else {
        return builder;
    };
    let address = crate::hostread::dial_address(&scheme, &authority);
    let (host, port) = address
        .rsplit_once(':')
        .map(|(h, p)| (h.to_string(), p.parse::<u16>().unwrap_or(0)))
        .unwrap_or((address.clone(), 0));
    match proxy_for(&scheme, &host, port).and_then(|p| ureq::Proxy::new(p).ok()) {
        Some(proxy) => builder.proxy(proxy),
        None => builder,
    }
}

pub(super) fn in_cidr(ip: std::net::IpAddr, net: std::net::IpAddr, bits: u32) -> bool {
    match (ip, net) {
        (std::net::IpAddr::V4(a), std::net::IpAddr::V4(b)) if bits <= 32 => {
            let mask = if bits == 0 {
                0
            } else {
                u32::MAX << (32 - bits)
            };
            u32::from(a) & mask == u32::from(b) & mask
        }
        (std::net::IpAddr::V6(a), std::net::IpAddr::V6(b)) if bits <= 128 => {
            let mask = if bits == 0 {
                0
            } else {
                u128::MAX << (128 - bits)
            };
            u128::from(a) & mask == u128::from(b) & mask
        }
        _ => false,
    }
}

/// Go's `url.Error` operation name: the method with only its first letter
/// upper case.
fn op_name(method: &str) -> String {
    let lower = method.to_ascii_lowercase();
    let mut chars = lower.chars();
    chars
        .next()
        .map(|c| c.to_ascii_uppercase().to_string() + chars.as_str())
        .unwrap_or_default()
}

/// The first resolver Go's DNS client asks, for a lookup error.
fn resolver() -> String {
    std::fs::read_to_string("/etc/resolv.conf")
        .ok()
        .and_then(|text| {
            text.lines()
                .filter_map(|l| l.trim().strip_prefix("nameserver"))
                .map(|s| s.trim().to_string())
                .find(|s| !s.is_empty())
        })
        .map(|ns| {
            if ns.contains(':') {
                format!("[{ns}]:53")
            } else {
                format!("{ns}:53")
            }
        })
        .unwrap_or_else(|| "127.0.0.1:53".into())
}

/// What Go's dialer reports for a connection it could not make.
fn dial_error(host: &str, port: u16, proxy: Option<&str>) -> Option<String> {
    use std::net::ToSocketAddrs;
    if let Some(proxy) = proxy {
        let (scheme, authority) = crate::hostread::http_authority(proxy)?;
        let address = crate::hostread::dial_address(&scheme, &authority);
        let resolved = address.to_socket_addrs().ok()?.next()?;
        return match std::net::TcpStream::connect_timeout(&resolved, Duration::from_secs(5)) {
            Ok(_) => None,
            Err(e) => Some(format!(
                "proxyconnect tcp: dial tcp {resolved}: connect: {}",
                crate::goerr::errno_text(&e)
            )),
        };
    }
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    let resolved = match (bare, port).to_socket_addrs() {
        Ok(mut addrs) => addrs.next()?,
        Err(_) => {
            return Some(format!(
                "dial tcp: lookup {bare} on {}: no such host",
                resolver()
            ))
        }
    };
    match std::net::TcpStream::connect_timeout(&resolved, Duration::from_secs(5)) {
        Ok(_) => None,
        Err(e) => Some(format!(
            "dial tcp {resolved}: connect: {}",
            crate::goerr::errno_text(&e)
        )),
    }
}

/// `http.Client.Do` (and reading the body): Go's error text on failure.
pub fn http_do(request: HttpRequest) -> Result<HttpResponse, String> {
    let op = op_name(request.method);
    let quoted = crate::goerr::quote(request.url);
    let Some((scheme, authority)) = crate::hostread::http_authority(request.url) else {
        return Err(format!(
            "{op} {quoted}: unsupported protocol scheme {}",
            crate::goerr::quote(request.url.split(':').next().unwrap_or(""))
        ));
    };
    let address = crate::hostread::dial_address(&scheme, &authority);
    let (host, port) = address
        .rsplit_once(':')
        .map(|(h, p)| (h.to_string(), p.parse::<u16>().unwrap_or(0)))
        .unwrap_or((address.clone(), 0));
    let proxy = proxy_for(&scheme, &host, port);
    let mut builder = ureq::AgentBuilder::new().redirects(if request.https_redirects.is_some() {
        0
    } else {
        10
    });
    if let Some(timeout) = request.timeout {
        builder = builder.timeout(timeout);
    }
    if let Some(proxy) = &proxy {
        if let Ok(p) = ureq::Proxy::new(proxy) {
            builder = builder.proxy(p);
        }
    }
    let agent = builder.build();
    let mut current = request.url.to_string();
    for _ in 0..=10 {
        let mut call = agent.request(request.method, &current);
        for (k, v) in &request.headers {
            call = call.set(k, v);
        }
        let outcome = match &request.body {
            Some(body) => call.send_bytes(body),
            None => call.call(),
        };
        let response = match outcome {
            Ok(r) | Err(ureq::Error::Status(_, r)) => r,
            Err(ureq::Error::Transport(t)) => {
                use std::error::Error as _;
                let mut source = t.source();
                while let Some(e) = source {
                    if let Some(io) = e.downcast_ref::<std::io::Error>() {
                        if matches!(
                            io.kind(),
                            std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                        ) {
                            return Err(format!("{op} {quoted}: context deadline exceeded (Client.Timeout exceeded while awaiting headers)"));
                        }
                    }
                    source = e.source();
                }
                let text =
                    dial_error(&host, port, proxy.as_deref()).unwrap_or_else(|| t.to_string());
                return Err(format!("{op} {quoted}: {text}"));
            }
        };
        let status = response.status();
        if let Some(refusal) = request.https_redirects {
            if (300..400).contains(&status) && status != 304 {
                if let Some(location) = response.header("Location") {
                    let next = url::Url::parse(&current)
                        .and_then(|base| base.join(location))
                        .map(|u| u.to_string())
                        .unwrap_or_else(|_| location.to_string());
                    if !next.starts_with("https:") {
                        return Err(format!("{op} {}: {refusal}", crate::goerr::quote(&next)));
                    }
                    current = next;
                    continue;
                }
            }
        }
        let status_line = format!("{status} {}", response.status_text());
        let mut body = Vec::new();
        response
            .into_reader()
            .take(request.limit)
            .read_to_end(&mut body)
            .map_err(|e| e.to_string())?;
        return Ok(HttpResponse {
            status,
            status_line,
            body,
        });
    }
    Err(format!("{op} {quoted}: stopped after 10 redirects"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_decode_as_a_go_map() {
        let parse = |s: &str| crate::gojson::parse(s.as_bytes()).unwrap();
        assert!(Args::decode(None, "").unwrap().map.is_empty());
        assert!(Args::decode(Some(&parse("null")), "").unwrap().nil);
        assert_eq!(
            Args::decode(Some(&parse("[1]")), "decode provider arguments: ").err().unwrap(),
            "decode provider arguments: json: cannot unmarshal array into Go value of type map[string]interface {}"
        );
        let args =
            Args::decode(Some(&parse(r#"{"a":" x ","n":2.0,"a2":[" y ",3,""]}"#)), "").unwrap();
        assert_eq!(args.string("a", "f"), "x");
        assert_eq!(args.int("n", 0), 2);
        assert_eq!(args.strings("a2"), vec!["y"]);
        assert_eq!(args.get("n"), Some(&J::from(2)));
    }

    #[test]
    fn go_names_its_operations() {
        assert_eq!(op_name("POST"), "Post");
        assert_eq!(op_name("DELETE"), "Delete");
    }

    /// Go's `httpproxy` rules (`config.init`, `useProxy`, `domainMatch`,
    /// `ipMatch`), one row each: (NO_PROXY, scheme, host, port, proxied).
    #[test]
    fn no_proxy_follows_gos_rules() {
        let rows: &[(&str, &str, &str, u16, bool)] = &[
            ("", "http", "example.com", 80, true),
            // HTTPS uses HTTPS_PROXY only.
            ("", "https", "example.com", 443, false),
            // A name matches itself and its subdomains, not a longer name.
            ("example.com", "http", "example.com", 80, false),
            ("example.com", "http", "a.example.com", 80, false),
            ("example.com", "http", "notexample.com", 80, true),
            ("Example.COM", "http", "EXAMPLE.com", 80, false),
            // A leading dot or `*.` matches subdomains only.
            (".example.com", "http", "a.example.com", 80, false),
            (".example.com", "http", "example.com", 80, true),
            ("*.example.com", "http", "a.example.com", 80, false),
            ("*.example.com", "http", "example.com", 80, true),
            // Only a leading `*.` is a wildcard.
            ("*example.com", "http", "example.com", 80, true),
            // An entry's port must be the request's.
            ("example.com:8080", "http", "example.com", 80, true),
            ("example.com:8080", "http", "example.com", 8080, false),
            // An IP host matches IP and CIDR entries by value, never a name.
            ("0.0.5", "http", "10.0.0.5", 80, true),
            ("10.0.0.5", "http", "10.0.0.5", 80, false),
            ("10.0.0.5:81", "http", "10.0.0.5", 80, true),
            ("10.0.0.0/8", "http", "10.1.2.3", 80, false),
            ("10.0.0.0/8", "http", "11.0.0.1", 80, true),
            ("2001:DB8:0::1", "http", "2001:db8::1", 80, false),
            ("[2001:db8::1]:8080", "http", "[2001:db8::1]", 8080, false),
            ("[2001:db8::1]:8080", "http", "2001:db8::1", 80, true),
            // `*` alone, malformed entries, blanks.
            ("a.test, * ", "http", "example.com", 80, false),
            (":80,,  ", "http", "example.com", 80, true),
            // Never localhost or a loopback address.
            ("", "http", "localhost", 80, false),
            ("", "http", "127.0.0.1", 80, false),
            ("", "http", "[::1]", 80, false),
        ];
        for (no_proxy, scheme, host, port, proxied) in rows {
            let chosen = select_proxy(scheme, host, *port, "http://proxy.test:3128", "", no_proxy);
            assert_eq!(
                chosen.is_some(),
                *proxied,
                "NO_PROXY={no_proxy:?} {scheme}://{host}:{port}"
            );
        }
        // HTTPS_PROXY for https; a bare host:port proxy gets a scheme.
        assert_eq!(
            select_proxy("https", "example.com", 443, "", "proxy.test:3128", ""),
            Some("http://proxy.test:3128".to_string())
        );
    }
}
