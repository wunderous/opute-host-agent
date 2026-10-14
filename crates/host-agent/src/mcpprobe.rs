//! The authenticated MCP exposure probe (`internal/mcpprobe/probe.go`): an
//! OAuth client-credentials token minted for the resource, then a Streamable
//! HTTP `tools/list` with it. Secrets live only in memory and never appear in
//! the observation or an error.
//!
//! URLs are parsed, escaped and printed as Go's `net/url` does, so a
//! normalized endpoint is byte-identical to Go's.

use std::time::Duration;

use serde_json::{json, Value as J};

use crate::gojson::{equal_fold, first_value, kind_name, parse, Node, Value};

pub const PROTOCOL_VERSION: &str = "2026-07-28";

/// The parts of a Go `*url.URL` these callers read or print. `host`, `path`
/// and `fragment` hold unescaped bytes; `raw_path` keeps a path spelling that
/// differs from the default escaping, as `URL.RawPath` does.
#[derive(Clone, Debug, Default)]
pub struct GoUrl {
    pub scheme: String,
    pub has_user: bool,
    pub user: String,
    pub host: Vec<u8>,
    pub path: Vec<u8>,
    pub raw_path: String,
    pub raw_query: String,
    pub force_query: bool,
    pub fragment: Vec<u8>,
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Path,
    Host,
    Fragment,
}

/// `shouldEscape` for the modes these URLs use.
fn should_escape(c: u8, mode: Mode) -> bool {
    if c.is_ascii_alphanumeric() {
        return false;
    }
    if mode == Mode::Host && b"!$&'()*+,;=:[]<>\"".contains(&c) {
        return false;
    }
    match c {
        b'-' | b'_' | b'.' | b'~' => return false,
        b'$' | b'&' | b'+' | b',' | b'/' | b':' | b';' | b'=' | b'?' | b'@' => {
            return match mode {
                Mode::Path => c == b'?',
                Mode::Fragment => false,
                Mode::Host => true,
            }
        }
        _ => {}
    }
    if mode == Mode::Fragment && b"!()*".contains(&c) {
        return false;
    }
    true
}

fn unhex(c: u8) -> Option<u8> {
    (c as char).to_digit(16).map(|d| d as u8)
}

/// `unescape`: percent escapes decoded; a bad escape (or, in a host, a
/// byte that must be escaped) is an error.
fn unescape(s: &str, mode: Mode) -> Option<Vec<u8>> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' => {
                let hi = b.get(i + 1).copied().and_then(unhex)?;
                let lo = b.get(i + 2).copied().and_then(unhex)?;
                if mode == Mode::Host && hi < 8 && &s[i..i + 3] != "%25" {
                    return None;
                }
                out.push(hi << 4 | lo);
                i += 3;
            }
            c => {
                if mode == Mode::Host && c < 0x80 && should_escape(c, mode) {
                    return None;
                }
                out.push(c);
                i += 1;
            }
        }
    }
    Some(out)
}

/// `escape` (never in query mode, so a space is `%20`).
fn escape(s: &[u8], mode: Mode) -> String {
    let mut out = String::with_capacity(s.len());
    for &c in s {
        if should_escape(c, mode) {
            out.push_str(&format!("%{c:02X}"));
        } else {
            out.push(char::from(c));
        }
    }
    out
}

/// `validEncoded(s, encodePath)`.
fn valid_encoded(s: &str) -> bool {
    s.bytes()
        .all(|c| b"!$&'()*+,;=:@[]%".contains(&c) || !should_escape(c, Mode::Path))
}

impl GoUrl {
    /// `url.Parse`, reporting only whether it failed.
    pub fn parse(raw: &str) -> Option<GoUrl> {
        let (rest, fragment) = match raw.split_once('#') {
            Some((r, f)) => (r, Some(f)),
            None => (raw, None),
        };
        let mut url = GoUrl::default();
        if let Some(fragment) = fragment {
            url.fragment = unescape(fragment, Mode::Fragment)?;
        }
        if rest.bytes().any(|b| b < 0x20 || b == 0x7f) {
            return None;
        }
        // getScheme.
        let mut rest = rest;
        for (i, c) in rest.bytes().enumerate() {
            if c.is_ascii_alphabetic() {
                continue;
            }
            if i > 0 && (c.is_ascii_digit() || b"+-.".contains(&c)) {
                continue;
            }
            if c == b':' {
                if i == 0 {
                    return None; // missing protocol scheme
                }
                url.scheme = rest[..i].to_ascii_lowercase();
                rest = &rest[i + 1..];
            }
            break;
        }
        if let Some((r, q)) = rest.split_once('?') {
            url.raw_query = q.to_string();
            url.force_query = q.is_empty();
            rest = r;
        }
        if !rest.starts_with('/') {
            if !url.scheme.is_empty() {
                // An opaque URL (`mailto:x`): no host, which every caller
                // rejects.
                return Some(url);
            }
            if let Some(colon) = rest.find(':') {
                if rest.find('/').is_none_or(|slash| colon < slash) {
                    return None; // first path segment cannot contain colon
                }
            }
        }
        if (!url.scheme.is_empty() || !rest.starts_with("///")) && rest.starts_with("//") {
            let after = &rest[2..];
            let end = after.find('/').unwrap_or(after.len());
            let authority = &after[..end];
            rest = &after[end..];
            let host = match authority.rfind('@') {
                Some(at) => {
                    url.has_user = true;
                    url.user = authority[..at].to_string();
                    &authority[at + 1..]
                }
                None => authority,
            };
            url.host = parse_host(host)?;
        }
        url.path = unescape(rest, Mode::Path)?;
        if escape(&url.path, Mode::Path) != rest {
            url.raw_path = rest.to_string();
        }
        Some(url)
    }

    /// `URL.Hostname`: the host without port or IPv6 brackets.
    pub fn hostname(&self) -> String {
        let host = String::from_utf8_lossy(&self.host);
        let host = match host.rfind(':') {
            Some(i) if !host[i..].contains(']') => &host[..i],
            _ => &host[..],
        };
        match host.strip_prefix('[') {
            Some(inner) => inner.strip_suffix(']').unwrap_or(inner).to_string(),
            None => host.to_string(),
        }
    }

    /// `URL.EscapedPath`.
    fn escaped_path(&self) -> String {
        if !self.raw_path.is_empty()
            && valid_encoded(&self.raw_path)
            && unescape(&self.raw_path, Mode::Path).as_deref() == Some(&self.path[..])
        {
            return self.raw_path.clone();
        }
        if self.path == b"*" {
            return "*".into();
        }
        escape(&self.path, Mode::Path)
    }

    /// Replaces the path, as assigning `Path` and clearing `RawPath` does.
    pub fn set_path(&mut self, path: &str) {
        self.path = path.as_bytes().to_vec();
        self.raw_path.clear();
    }

    /// `URL.String` for the URL shapes the callers build.
    pub fn string(&self) -> String {
        let mut out = String::new();
        if !self.scheme.is_empty() {
            out.push_str(&self.scheme);
            out.push(':');
        }
        if !self.scheme.is_empty() || !self.host.is_empty() || self.has_user {
            if !self.host.is_empty() || !self.path.is_empty() || self.has_user {
                out.push_str("//");
            }
            if self.has_user {
                out.push_str(&self.user);
                out.push('@');
            }
            out.push_str(&escape(&self.host, Mode::Host));
        }
        let path = self.escaped_path();
        if !path.is_empty() && !path.starts_with('/') && !self.host.is_empty() {
            out.push('/');
        }
        out.push_str(&path);
        if self.force_query || !self.raw_query.is_empty() {
            out.push('?');
            out.push_str(&self.raw_query);
        }
        if !self.fragment.is_empty() {
            out.push('#');
            out.push_str(&escape(&self.fragment, Mode::Fragment));
        }
        out
    }
}

/// `parseHost`: a bracketed IPv6 literal or a name, an optional numeric
/// port, then host unescaping.
fn parse_host(host: &str) -> Option<Vec<u8>> {
    let port_ok = |p: &str| {
        p.is_empty()
            || p.strip_prefix(':')
                .is_some_and(|d| d.bytes().all(|b| b.is_ascii_digit()))
    };
    if host.starts_with('[') {
        let close = host.rfind(']')?;
        if !port_ok(&host[close + 1..]) {
            return None;
        }
    } else if let Some(i) = host.rfind(':') {
        if !port_ok(&host[i..]) {
            return None;
        }
    }
    unescape(host, Mode::Host)
}

/// `net.ParseIP(host).IsLoopback()`.
pub fn is_loopback_ip(host: &str) -> bool {
    match host.parse::<std::net::IpAddr>() {
        Ok(std::net::IpAddr::V4(ip)) => ip.is_loopback(),
        Ok(std::net::IpAddr::V6(ip)) => {
            ip.is_loopback() || ip.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback())
        }
        Err(_) => false,
    }
}

/// `probeURLs`: the resource URL (path without trailing slashes, ending in
/// `/mcp`, no fragment) and its sibling token endpoint.
fn probe_urls(endpoint: &str) -> Result<(String, String), String> {
    let invalid = || "MCP endpoint must be an absolute HTTP(S) URL without userinfo".to_string();
    let mut parsed = GoUrl::parse(endpoint.trim()).ok_or_else(invalid)?;
    if parsed.host.is_empty()
        || (parsed.scheme != "http" && parsed.scheme != "https")
        || parsed.has_user
    {
        return Err(invalid());
    }
    let trimmed = parsed.path.len() - parsed.path.iter().rev().take_while(|&&b| b == b'/').count();
    parsed.path.truncate(trimmed);
    if !parsed.path.ends_with(b"/mcp") {
        return Err("MCP endpoint must end with /mcp".into());
    }
    parsed.fragment.clear();
    let resource = parsed.string();
    let mut token = parsed.clone();
    token.path = [&parsed.path[..parsed.path.len() - 4], b"/oauth/token"].concat();
    token.raw_query.clear();
    Ok((resource, token.string()))
}

/// `ValidateEndpoint`.
pub fn validate_endpoint(endpoint: &str) -> Result<String, String> {
    let (resource, _) = probe_urls(endpoint)?;
    let parsed = GoUrl::parse(&resource).ok_or("MCP endpoint is not a valid URL")?;
    if !parsed.raw_query.is_empty() || !parsed.fragment.is_empty() {
        return Err("MCP endpoint must not contain a query or fragment".into());
    }
    Ok(resource)
}

/// `url.QueryEscape`.
fn query_escape(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(b))
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(15))
        .tls_config(crate::hostartifact::tls_config())
        .build()
}

fn post_error(what: &str, url: &str, err: &ureq::Transport) -> String {
    let cause = crate::hostartifact::get_error(url, err);
    format!(
        "{what}: Post{}",
        cause.strip_prefix("Get").unwrap_or(&cause)
    )
}

fn read_limited(response: ureq::Response, limit: u64) -> Vec<u8> {
    use std::io::Read;
    let mut body = Vec::new();
    let _ = response.into_reader().take(limit).read_to_end(&mut body);
    body
}

/// `mintProbeToken`.
fn mint_token(token_url: &str, resource: &str) -> Result<String, String> {
    let form = format!(
        "client_id=opute-mcp-host&grant_type=client_credentials&resource={}",
        query_escape(resource)
    );
    let response = match agent()
        .post(token_url)
        .set("Content-Type", "application/x-www-form-urlencoded")
        .send_string(&form)
    {
        Ok(response) => response,
        Err(ureq::Error::Status(code, _)) => {
            return Err(format!("MCP OAuth token request returned HTTP {code}"))
        }
        Err(ureq::Error::Transport(t)) => {
            return Err(post_error("MCP OAuth token request failed", token_url, &t))
        }
    };
    let body = read_limited(response, 64 * 1024);
    let node = decode(&body).map_err(|e| format!("decode MCP OAuth token response: {e}"))?;
    let mut token = String::new();
    let mut first: Option<String> = None;
    match &node.value {
        Value::Null => {}
        Value::Object(members) => {
            for (key, value) in members {
                if !equal_fold(key, "access_token") {
                    continue;
                }
                match &value.value {
                    Value::Null => {}
                    Value::String(text) => token = text.clone(),
                    other => {
                        first.get_or_insert_with(|| {
                            format!(
                                "json: cannot unmarshal {} into Go struct field .access_token of type string",
                                kind_name(other)
                            )
                        });
                    }
                }
            }
        }
        other => {
            first = Some(format!(
                "json: cannot unmarshal {} into Go value of type struct {{ AccessToken string \"json:\\\"access_token\\\"\" }}",
                kind_name(other)
            ))
        }
    }
    if let Some(e) = first {
        return Err(format!("decode MCP OAuth token response: {e}"));
    }
    if token.trim().is_empty() {
        return Err("MCP OAuth token response did not contain access_token".into());
    }
    Ok(token.trim().to_string())
}

/// `json.NewDecoder(r).Decode`: the first JSON value of the body.
fn decode(body: &[u8]) -> Result<Node, String> {
    let end = first_value(body)?;
    parse(&body[..end]).map_err(|_| "unexpected EOF".to_string())
}

const RPC_TYPE: &str = "struct { Result *struct { Tools []json.RawMessage \"json:\\\"tools\\\"\" } \"json:\\\"result\\\"\"; Error *struct { Code int \"json:\\\"code\\\"\" } \"json:\\\"error\\\"\" }";

/// Decodes the JSON-RPC envelope into Go's anonymous `rpc` struct: the tool
/// count of a non-null `result` and the code of a non-null `error`, or the
/// first `UnmarshalTypeError` (decoding continues past one, as in Go).
#[allow(clippy::type_complexity)]
fn decode_rpc(node: &Node) -> Result<(Option<usize>, Option<i64>), String> {
    let members = match &node.value {
        Value::Null => return Ok((None, None)),
        Value::Object(members) => members,
        other => {
            return Err(format!(
                "json: cannot unmarshal {} into Go value of type {RPC_TYPE}",
                kind_name(other)
            ))
        }
    };
    let mut first: Option<String> = None;
    let mut type_error = |field: &str, kind: String, ty: &str| {
        first.get_or_insert_with(|| {
            format!("json: cannot unmarshal {kind} into Go struct field .{field} of type {ty}")
        });
    };
    let mut result = None;
    let mut error = None;
    for (key, value) in members {
        if equal_fold(key, "result") {
            match &value.value {
                Value::Null => result = None,
                Value::Object(fields) => {
                    let mut count = result.unwrap_or(0);
                    for (k, tools) in fields {
                        if !equal_fold(k, "tools") {
                            continue;
                        }
                        match &tools.value {
                            Value::Null => count = 0,
                            Value::Array(items) => count = items.len(),
                            other => type_error(
                                "result.tools",
                                kind_name(other).into(),
                                "[]json.RawMessage",
                            ),
                        }
                    }
                    result = Some(count);
                }
                other => type_error(
                    "result",
                    kind_name(other).into(),
                    "struct { Tools []json.RawMessage \"json:\\\"tools\\\"\" }",
                ),
            }
        } else if equal_fold(key, "error") {
            match &value.value {
                Value::Null => error = None,
                Value::Object(fields) => {
                    let mut code = error.unwrap_or(0);
                    for (k, value) in fields {
                        if !equal_fold(k, "code") {
                            continue;
                        }
                        match &value.value {
                            Value::Null => {}
                            Value::Number(text) => match text.parse::<i64>() {
                                Ok(n) => code = n,
                                Err(_) => type_error("error.code", format!("number {text}"), "int"),
                            },
                            other => type_error("error.code", kind_name(other).into(), "int"),
                        }
                    }
                    error = Some(code);
                }
                other => type_error(
                    "error",
                    kind_name(other).into(),
                    "struct { Code int \"json:\\\"code\\\"\" }",
                ),
            }
        }
    }
    match first {
        Some(e) => Err(e),
        None => Ok((result, error)),
    }
}

/// `ProbeAuthenticatedMCPEndpoint`.
pub fn probe_authenticated(endpoint: &str, bearer: &str) -> Result<J, String> {
    let (resource, token_url) = probe_urls(endpoint)?;
    let (token, mode) = if bearer.trim().is_empty() {
        (mint_token(&token_url, &resource)?, "client-credentials")
    } else {
        (bearer.trim().to_string(), "provided-bearer")
    };
    // `json.Marshal` of the request map: keys sorted.
    let body = format!(
        r#"{{"id":"host-agent-mcp-probe","jsonrpc":"2.0","method":"tools/list","params":{{"_meta":{{"io.modelcontextprotocol/clientCapabilities":{{}},"io.modelcontextprotocol/clientInfo":{{"name":"opute-host-agent-mcp-probe","version":"1.0.0"}},"io.modelcontextprotocol/protocolVersion":"{PROTOCOL_VERSION}"}}}}}}"#
    );
    let response = match agent()
        .post(&resource)
        .set("Authorization", &format!("Bearer {token}"))
        .set("Content-Type", "application/json")
        .set("Accept", "application/json, text/event-stream")
        .set("MCP-Protocol-Version", PROTOCOL_VERSION)
        .set("Mcp-Method", "tools/list")
        .send_string(&body)
    {
        Ok(response) => response,
        Err(ureq::Error::Status(code, _)) => {
            return Err(format!("MCP tools/list probe returned HTTP {code}"))
        }
        Err(ureq::Error::Transport(t)) => {
            return Err(post_error("MCP tools/list probe failed", &resource, &t))
        }
    };
    let status = response.status();
    let body = read_limited(response, 1 << 20);
    let node = decode(&body).map_err(|e| format!("decode MCP tools/list probe: {e}"))?;
    let (result, error) =
        decode_rpc(&node).map_err(|e| format!("decode MCP tools/list probe: {e}"))?;
    if let Some(code) = error {
        return Err(format!(
            "MCP tools/list probe returned JSON-RPC error {code}"
        ));
    }
    let Some(tool_count) = result else {
        return Err("MCP tools/list probe returned no result".into());
    };
    Ok(json!({
        "servingContract": "mcp-exposure.v1",
        "endpoint": resource,
        "ready": true,
        "authenticated": true,
        "authorizationMode": mode,
        "protocolVersion": PROTOCOL_VERSION,
        "statusCode": status,
        "toolCount": tool_count,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoints_normalize_like_go() {
        assert_eq!(
            probe_urls("https://Host.example:8443/a/mcp//#frag").unwrap(),
            (
                "https://Host.example:8443/a/mcp".to_string(),
                "https://Host.example:8443/a/oauth/token".to_string()
            )
        );
        assert!(validate_endpoint("https://h/mcp?x=1").is_err());
        assert!(validate_endpoint("https://u@h/mcp").is_err());
        assert!(validate_endpoint("ftp://h/mcp").is_err());
        assert!(validate_endpoint("https://h/other").is_err());
        assert!(
            is_loopback_ip("127.0.0.2") && is_loopback_ip("::1") && !is_loopback_ip("10.0.0.1")
        );
        assert_eq!(query_escape("https://h/mcp"), "https%3A%2F%2Fh%2Fmcp");
    }
}
