//! Cloudflare's API (Go `dedicated_tunnel.go` and `cloudflareDelete`): one
//! dedicated tunnel per host, its ingress and its proxied CNAMEs, never a
//! hostname the operator reserved (`OPUTE_RESERVED_HOSTNAMES`).

use super::super::support::{
    first_non_empty, go_any, http_do, string_slice_input, Args, HttpRequest,
};
use super::query_escape;
use crate::goerr::quote;
use serde_json::{json, Map, Value as J};

const API_DEFAULT_BASE: &str = "https://api.cloudflare.com/client/v4";
/// Operator setting: exact hostnames and `*.suffix` patterns, comma-separated.
const RESERVED_ENV: &str = "OPUTE_RESERVED_HOSTNAMES";

#[cfg(test)]
thread_local! {
    static RESERVED_OVERRIDE: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// Runs `body` with `OPUTE_RESERVED_HOSTNAMES` set to `value` on this thread.
#[cfg(test)]
pub(super) fn with_reserved(value: &str, body: impl FnOnce()) {
    RESERVED_OVERRIDE.with(|cell| *cell.borrow_mut() = Some(value.to_string()));
    body();
    RESERVED_OVERRIDE.with(|cell| *cell.borrow_mut() = None);
}

fn reserved_setting() -> String {
    #[cfg(test)]
    if let Some(value) = RESERVED_OVERRIDE.with(|cell| cell.borrow().clone()) {
        return value;
    }
    std::env::var(RESERVED_ENV).unwrap_or_default()
}

/// The reserved pattern a hostname matches, if any: an exact name, or
/// `*.suffix` for any name under `suffix` (never the bare suffix).
fn reserved_match(hostname: &str) -> Option<String> {
    let hostname = hostname.trim().trim_end_matches('.').to_lowercase();
    reserved_setting()
        .split(',')
        .map(|p| p.trim().trim_end_matches('.').to_lowercase())
        .filter(|p| !p.is_empty())
        .find(|pattern| match pattern.strip_prefix("*.") {
            Some(suffix) => hostname.ends_with(&format!(".{suffix}")),
            None => hostname == *pattern,
        })
}

fn reserved_refusal(hostname: &str, pattern: &str) -> String {
    super::super::typed_error(
        "reserved_hostname",
        &format!(
            "hostname {} is reserved by the operator ({RESERVED_ENV})",
            quote(hostname)
        ),
        json!({"hostname": hostname, "pattern": pattern}),
    )
}

pub(super) struct Dedicated {
    pub tunnel_id: String,
    pub dns_record_id: String,
    pub run_token: String,
}

#[derive(Clone, Default)]
pub(super) struct Tunnel {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Default)]
pub(super) struct Record {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub content: String,
}

/// `cloudflareIngressRule`: hostname omitted when empty.
pub(super) struct IngressRule {
    pub hostname: String,
    pub service: String,
    pub origin_request: Option<J>,
}

impl IngressRule {
    fn to_json(&self) -> J {
        // Struct field order: hostname, service, originRequest.
        let mut out = Map::new();
        if !self.hostname.is_empty() {
            out.insert("hostname".into(), J::from(self.hostname.as_str()));
        }
        out.insert("service".into(), J::from(self.service.as_str()));
        if let Some(origin) = self
            .origin_request
            .as_ref()
            .filter(|o| o.as_object().is_some_and(|m| !m.is_empty()))
        {
            out.insert("originRequest".into(), origin.clone());
        }
        J::Object(out)
    }

    fn encode(&self) -> String {
        let mut out = String::from("{");
        if !self.hostname.is_empty() {
            out.push_str("\"hostname\":");
            crate::gojson::encode_string(&self.hostname, &mut out);
            out.push(',');
        }
        out.push_str("\"service\":");
        crate::gojson::encode_string(&self.service, &mut out);
        if let Some(origin) = self.to_json().get("originRequest") {
            out.push_str(",\"originRequest\":");
            crate::gojson::encode(origin, &mut out);
        }
        out.push('}');
        out
    }
}

pub(super) fn identifier_ok(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

fn dedicated_tunnel_name(args: &Args) -> String {
    args.string("tunnelName", "").trim().to_string()
}

pub(super) fn require_tunnel_name(name: &str) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("tunnelName is required".into());
    }
    Ok(())
}

pub(super) fn refuse_reserved_hostname(hostname: &str) -> Result<(), String> {
    let trimmed = hostname.trim().to_lowercase();
    if trimmed.is_empty() {
        return Err("hostname is required".into());
    }
    match reserved_match(&trimmed) {
        Some(pattern) => Err(reserved_refusal(&trimmed, &pattern)),
        None => Ok(()),
    }
}

pub(super) fn refuse_reserved_ingress(rules: &[IngressRule]) -> Result<(), String> {
    for rule in rules {
        let hostname = rule.hostname.trim().to_lowercase();
        if let Some(pattern) = reserved_match(&hostname) {
            return Err(reserved_refusal(&hostname, &pattern));
        }
    }
    Ok(())
}

fn parse_tunnel_id_from_cname(content: &str) -> String {
    let lowered = content.trim().to_lowercase();
    let trimmed = lowered.strip_suffix('.').unwrap_or(&lowered);
    match trimmed.strip_suffix(".cfargotunnel.com") {
        Some(id) if identifier_ok(id) => id.to_string(),
        _ => String::new(),
    }
}

pub(super) fn api_base() -> String {
    first_non_empty(&[
        &std::env::var("CLOUDFLARE_API_BASE").unwrap_or_default(),
        API_DEFAULT_BASE,
    ])
    .trim_end_matches('/')
    .to_string()
}

pub(super) fn creds() -> Result<(String, String, String), String> {
    let get = |k: &str| std::env::var(k).unwrap_or_default().trim().to_string();
    let (account, zone, token) = (
        get("CLOUDFLARE_ACCOUNT_ID"),
        get("CLOUDFLARE_ZONE_ID"),
        get("CLOUDFLARE_API_TOKEN"),
    );
    if account.is_empty() || zone.is_empty() || token.is_empty() {
        return Err("dedicated Cloudflare tunnel requires CLOUDFLARE_ACCOUNT_ID, CLOUDFLARE_ZONE_ID, and CLOUDFLARE_API_TOKEN".into());
    }
    Ok((account, zone, token))
}

pub(super) fn ensure_dedicated_tunnel(args: &Args) -> Result<Dedicated, String> {
    let hostnames = dedicated_hostnames(args);
    if hostnames.is_empty() {
        return Err("hostname is required".into());
    }
    for hostname in &hostnames {
        refuse_reserved_hostname(hostname)?;
    }
    let tunnel_name = dedicated_tunnel_name(args);
    require_tunnel_name(&tunnel_name)?;
    let local_target = args.string("localTarget", "");
    if local_target.is_empty() {
        return Err("localTarget is required".into());
    }
    let (account, zone, token) = creds()?;
    let tunnel = ensure_tunnel_by_name(&token, &account, &tunnel_name)?;
    let rules = tunnel_ingress(&token, &account, &tunnel.id)?;
    refuse_reserved_ingress(&rules)?;
    let mut ingress: Vec<IngressRule> = hostnames
        .iter()
        .map(|h| IngressRule {
            hostname: h.clone(),
            service: local_target.clone(),
            origin_request: None,
        })
        .collect();
    ingress.push(IngressRule {
        hostname: String::new(),
        service: "http_status:404".into(),
        origin_request: None,
    });
    put_tunnel_ingress(&token, &account, &tunnel.id, &ingress)?;
    let mut primary: Option<Record> = None;
    for hostname in &hostnames {
        let record = upsert_cname(&token, &zone, hostname, &tunnel.id)?;
        primary.get_or_insert(record);
    }
    let run_token = tunnel_token(&token, &account, &tunnel.id)?;
    Ok(Dedicated {
        tunnel_id: tunnel.id,
        dns_record_id: primary.map(|r| r.id).unwrap_or_default(),
        run_token,
    })
}

fn dedicated_hostnames(args: &Args) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for value in super::hostnames_from_args(args) {
        let hostname = value.trim().to_lowercase();
        if hostname.is_empty() || out.contains(&hostname) {
            continue;
        }
        out.push(hostname);
    }
    out
}

pub(super) fn unpublish_dedicated_tunnel(args: &Args) -> Result<(), String> {
    let hostnames = dedicated_hostnames(args);
    if hostnames.is_empty() {
        return Err("hostname is required".into());
    }
    for hostname in &hostnames {
        refuse_reserved_hostname(hostname)?;
    }
    let tunnel_name = dedicated_tunnel_name(args);
    require_tunnel_name(&tunnel_name)?;
    let (account, zone, token) = creds()?;
    let tunnel = find_tunnel_by_name(&token, &account, &tunnel_name)?;
    if let Some(tunnel) = &tunnel {
        let rules = tunnel_ingress(&token, &account, &tunnel.id)?;
        refuse_reserved_ingress(&rules)?;
    }
    let base = api_base();
    for hostname in &hostnames {
        let Some(record) = find_hostname_cname(&token, &zone, hostname)? else {
            continue;
        };
        let target_id = parse_tunnel_id_from_cname(&record.content);
        if !target_id.is_empty() {
            if let Some(target) = get_tunnel(&token, &account, &target_id)? {
                refuse_reserved_ingress(&tunnel_ingress(&token, &account, &target.id)?)?;
            }
        }
        delete(
            &token,
            &format!("{base}/zones/{zone}/dns_records/{}", record.id),
        )
        .map_err(|e| format!("delete dedicated CNAME: {e}"))?;
    }
    if let Some(tunnel) = tunnel {
        delete(
            &token,
            &format!(
                "{base}/accounts/{account}/cfd_tunnel/{}?force=true",
                tunnel.id
            ),
        )
        .map_err(|e| format!("delete dedicated tunnel: {e}"))?;
    }
    Ok(())
}

/// `probeTunnelRouting`: the hostnames the named tunnel's ingress serves
/// (nil when none was read), whether every requested one is among them, and
/// the tunnel id.
pub(super) fn probe_tunnel_routing(
    args: &Args,
    hostnames: &[String],
) -> (Option<Vec<String>>, bool, String) {
    let tunnel_name = dedicated_tunnel_name(args);
    if tunnel_name.is_empty() || hostnames.is_empty() {
        return (None, false, String::new());
    }
    let Ok((account, _, token)) = creds() else {
        return (None, false, String::new());
    };
    let tunnel = match find_tunnel_by_name(&token, &account, &tunnel_name) {
        Ok(Some(tunnel)) => tunnel,
        _ => return (None, false, String::new()),
    };
    let rules = match tunnel_ingress(&token, &account, &tunnel.id) {
        Ok(rules) => rules,
        Err(_) => return (None, false, tunnel.id),
    };
    let mut routed_hostnames: Option<Vec<String>> = None;
    let mut served = Vec::new();
    for rule in &rules {
        let name = rule.hostname.trim().to_lowercase();
        if name.is_empty() {
            continue;
        }
        served.push(name.clone());
        routed_hostnames.get_or_insert_with(Vec::new).push(name);
    }
    let routed = hostnames
        .iter()
        .all(|h| served.contains(&h.trim().to_lowercase()));
    (routed_hostnames, routed, tunnel.id)
}

pub(super) fn ensure_tunnel_by_name(
    token: &str,
    account: &str,
    name: &str,
) -> Result<Tunnel, String> {
    if let Some(existing) = find_tunnel_by_name(token, account, name)? {
        return Ok(existing);
    }
    let mut body = String::new();
    crate::gojson::encode(
        &json!({"name": name, "config_src": "cloudflare"}),
        &mut body,
    );
    let created = cloudflare_json(
        token,
        "POST",
        &format!("{}/accounts/{account}/cfd_tunnel", api_base()),
        Some(body),
    )
    .map_err(|e| format!("create Cloudflare tunnel: {e}"))?;
    let created = created
        .map(|r| decode_tunnel(&r))
        .transpose()
        .map_err(|e| format!("create Cloudflare tunnel: {e}"))?
        .unwrap_or_default();
    if created.id.is_empty() {
        return Err("create Cloudflare tunnel: missing id".into());
    }
    Ok(created)
}

pub(super) fn find_tunnel_by_name(
    token: &str,
    account: &str,
    name: &str,
) -> Result<Option<Tunnel>, String> {
    let endpoint = format!(
        "{}/accounts/{account}/cfd_tunnel?is_deleted=false&name={}",
        api_base(),
        query_escape(name)
    );
    let result = cloudflare_json(token, "GET", &endpoint, None)
        .map_err(|e| format!("list Cloudflare tunnels: {e}"))?;
    let tunnels = match result {
        Some(r) => decode_list(&r, decode_tunnel, "[]main.cloudflareTunnel")
            .map_err(|e| format!("list Cloudflare tunnels: {e}"))?,
        None => Vec::new(),
    };
    Ok(tunnels
        .into_iter()
        .find(|t| t.name == name && !t.id.is_empty()))
}

pub(super) fn get_tunnel(
    token: &str,
    account: &str,
    tunnel_id: &str,
) -> Result<Option<Tunnel>, String> {
    if !identifier_ok(tunnel_id) {
        return Err("invalid Cloudflare tunnel id".into());
    }
    match cloudflare_json(
        token,
        "GET",
        &format!("{}/accounts/{account}/cfd_tunnel/{tunnel_id}", api_base()),
        None,
    ) {
        Err(e) if is_not_found(&e) => Ok(None),
        Err(e) => Err(e),
        Ok(None) => Ok(None),
        Ok(Some(r)) => {
            let tunnel = decode_tunnel(&r)?;
            Ok((!tunnel.id.is_empty()).then_some(tunnel))
        }
    }
}

pub(super) fn tunnel_ingress(
    token: &str,
    account: &str,
    tunnel_id: &str,
) -> Result<Vec<IngressRule>, String> {
    let result = match cloudflare_json(
        token,
        "GET",
        &format!(
            "{}/accounts/{account}/cfd_tunnel/{tunnel_id}/configurations",
            api_base()
        ),
        None,
    ) {
        Err(e) if is_not_found(&e) => return Ok(Vec::new()),
        Err(e) => return Err(format!("read Cloudflare tunnel ingress: {e}")),
        Ok(r) => r,
    };
    let rules = result
        .as_ref()
        .and_then(|r| r.get("config"))
        .and_then(|c| c.get("ingress"))
        .and_then(J::as_array)
        .cloned()
        .unwrap_or_default();
    Ok(rules
        .iter()
        .map(|r| IngressRule {
            hostname: r
                .get("hostname")
                .and_then(J::as_str)
                .unwrap_or("")
                .to_string(),
            service: r
                .get("service")
                .and_then(J::as_str)
                .unwrap_or("")
                .to_string(),
            origin_request: r.get("originRequest").cloned(),
        })
        .collect())
}

pub(super) fn put_tunnel_ingress(
    token: &str,
    account: &str,
    tunnel_id: &str,
    rules: &[IngressRule],
) -> Result<(), String> {
    let body = format!(
        "{{\"config\":{{\"ingress\":[{}]}}}}",
        rules
            .iter()
            .map(IngressRule::encode)
            .collect::<Vec<_>>()
            .join(",")
    );
    cloudflare_json(
        token,
        "PUT",
        &format!(
            "{}/accounts/{account}/cfd_tunnel/{tunnel_id}/configurations",
            api_base()
        ),
        Some(body),
    )
    .map_err(|e| format!("put Cloudflare tunnel ingress: {e}"))?;
    Ok(())
}

pub(super) fn tunnel_token(token: &str, account: &str, tunnel_id: &str) -> Result<String, String> {
    let result = cloudflare_json(
        token,
        "GET",
        &format!(
            "{}/accounts/{account}/cfd_tunnel/{tunnel_id}/token",
            api_base()
        ),
        None,
    )
    .map_err(|e| format!("mint Cloudflare tunnel token: {e}"))?;
    let run_token = match result {
        None => String::new(),
        Some(J::String(s)) => s,
        Some(other) => {
            return Err(format!(
                "mint Cloudflare tunnel token: decode Cloudflare result: json: cannot unmarshal {} into Go value of type string",
                json_kind(&other)
            ))
        }
    };
    if run_token.trim().is_empty() {
        return Err("mint Cloudflare tunnel token: empty result".into());
    }
    Ok(run_token)
}

fn find_hostname_cname(token: &str, zone: &str, hostname: &str) -> Result<Option<Record>, String> {
    Ok(list_hostname_records(token, zone, hostname)?
        .into_iter()
        .find(|r| r.kind.eq_ignore_ascii_case("CNAME")))
}

fn list_hostname_records(token: &str, zone: &str, hostname: &str) -> Result<Vec<Record>, String> {
    let endpoint = format!(
        "{}/zones/{zone}/dns_records?name={}",
        api_base(),
        query_escape(hostname)
    );
    let result = cloudflare_json(token, "GET", &endpoint, None)
        .map_err(|e| format!("list Cloudflare DNS records: {e}"))?;
    let records = match result {
        Some(r) => decode_list(&r, decode_record, "[]main.cloudflareDNSRecord")
            .map_err(|e| format!("list Cloudflare DNS records: {e}"))?,
        None => Vec::new(),
    };
    Ok(records
        .into_iter()
        .filter(|r| r.name.eq_ignore_ascii_case(hostname))
        .collect())
}

pub(super) fn upsert_cname(
    token: &str,
    zone: &str,
    hostname: &str,
    tunnel_id: &str,
) -> Result<Record, String> {
    let desired = format!("{tunnel_id}.cfargotunnel.com");
    let base = api_base();
    let mut matching = None;
    for record in list_hostname_records(token, zone, hostname)? {
        if record.kind.eq_ignore_ascii_case("CNAME")
            && record
                .content
                .strip_suffix('.')
                .unwrap_or(&record.content)
                .eq_ignore_ascii_case(&desired)
        {
            matching = Some(record);
            continue;
        }
        delete(
            token,
            &format!("{base}/zones/{zone}/dns_records/{}", record.id),
        )
        .map_err(|e| {
            format!(
                "delete conflicting DNS {} {}: {e}",
                record.kind, record.name
            )
        })?;
    }
    if let Some(record) = matching {
        return Ok(record);
    }
    let mut body = String::new();
    crate::gojson::encode(
        &json!({"type": "CNAME", "name": hostname, "content": desired, "proxied": true, "ttl": 1}),
        &mut body,
    );
    let created = cloudflare_json(
        token,
        "POST",
        &format!("{base}/zones/{zone}/dns_records"),
        Some(body),
    )
    .map_err(|e| format!("create dedicated CNAME: {e}"))?;
    let created = created
        .map(|r| decode_record(&r))
        .transpose()
        .map_err(|e| format!("create dedicated CNAME: {e}"))?
        .unwrap_or_default();
    if created.id.is_empty() {
        return Err("create dedicated CNAME: missing id".into());
    }
    Ok(created)
}

fn is_not_found(e: &str) -> bool {
    e.contains("cloudflare not found")
}

fn json_kind(v: &J) -> &'static str {
    match v {
        J::Object(_) => "object",
        J::Array(_) => "array",
        J::String(_) => "string",
        J::Bool(_) => "bool",
        J::Null => "null",
        J::Number(_) => "number",
    }
}

/// A Cloudflare object decoded into a Go struct of string fields: a
/// mistyped field is Go's `UnmarshalTypeError`.
fn string_fields(value: &J, ty: &str, fields: &[&str]) -> Result<Vec<String>, String> {
    let Some(map) = value.as_object() else {
        if value.is_null() {
            return Ok(vec![String::new(); fields.len()]);
        }
        return Err(format!(
            "decode Cloudflare result: json: cannot unmarshal {} into Go value of type main.{ty}",
            json_kind(value)
        ));
    };
    let mut out = vec![String::new(); fields.len()];
    for (key, item) in map {
        let Some(i) = fields
            .iter()
            .position(|f| crate::gojson::equal_fold(f, key))
        else {
            continue;
        };
        match item {
            J::String(s) => out[i] = s.clone(),
            J::Null => {}
            other => {
                return Err(format!(
                    "decode Cloudflare result: json: cannot unmarshal {} into Go struct field {ty}.{} of type string",
                    json_kind(other),
                    fields[i]
                ))
            }
        }
    }
    Ok(out)
}

fn decode_tunnel(value: &J) -> Result<Tunnel, String> {
    let f = string_fields(value, "cloudflareTunnel", &["id", "name"])?;
    Ok(Tunnel {
        id: f[0].clone(),
        name: f[1].clone(),
    })
}

fn decode_record(value: &J) -> Result<Record, String> {
    let f = string_fields(
        value,
        "cloudflareDNSRecord",
        &["id", "type", "name", "content"],
    )?;
    Ok(Record {
        id: f[0].clone(),
        kind: f[1].clone(),
        name: f[2].clone(),
        content: f[3].clone(),
    })
}

fn decode_list<T>(
    value: &J,
    item: fn(&J) -> Result<T, String>,
    ty: &str,
) -> Result<Vec<T>, String> {
    match value {
        J::Null => Ok(Vec::new()),
        J::Array(items) => items.iter().map(item).collect(),
        other => Err(format!(
            "decode Cloudflare result: json: cannot unmarshal {} into Go value of type {ty}",
            json_kind(other)
        )),
    }
}

/// `json.NewDecoder(r).Decode(&v)` into a generic value: the first JSON
/// value, with the decoder's errors.
pub(crate) fn decode_first(body: &[u8]) -> Result<J, String> {
    let end = crate::gojson::first_value(body)?;
    let node = crate::gojson::parse(&body[..end]).map_err(|_| "unexpected EOF".to_string())?;
    Ok(go_any(&node))
}

/// `json.Unmarshal` into a struct: Go's syntax error text first.
fn unmarshal(payload: &[u8]) -> Result<J, String> {
    if let Some(e) = crate::gojson::syntax_error(payload) {
        return Err(e);
    }
    crate::gojson::parse(payload)
        .map(|n| go_any(&n))
        .map_err(|_| "unexpected end of JSON input".to_string())
}

/// `cloudflareJSON`: one authenticated call; 404 is "cloudflare not found";
/// the envelope must report success. The result is absent when Cloudflare
/// sent none (or null).
pub(super) fn cloudflare_json(
    token: &str,
    method: &str,
    endpoint: &str,
    body: Option<String>,
) -> Result<Option<J>, String> {
    let mut headers = vec![("Authorization", format!("Bearer {token}"))];
    let body = body.filter(|b| !b.is_empty());
    if body.is_some() {
        headers.push(("Content-Type", "application/json".into()));
    }
    let response = http_do(HttpRequest {
        method,
        url: endpoint,
        headers,
        body: body.map(String::into_bytes),
        timeout: None,
        https_redirects: None,
        limit: 256 * 1024,
    })?;
    if response.status == 404 {
        return Err(format!("cloudflare not found: {endpoint}"));
    }
    if !(200..300).contains(&response.status) {
        return Err(format!(
            "HTTP {}: {}",
            response.status,
            String::from_utf8_lossy(&response.body).trim()
        ));
    }
    let envelope =
        unmarshal(&response.body).map_err(|e| format!("decode Cloudflare response: {e}"))?;
    let (success, errors) =
        envelope_fields(&envelope).map_err(|e| format!("decode Cloudflare response: {e}"))?;
    if !success {
        if let Some((code, message)) = errors.first() {
            return Err(format!("Cloudflare API error {code}: {message}"));
        }
        return Err("Cloudflare API reported failure".into());
    }
    Ok(envelope.get("result").filter(|r| !r.is_null()).cloned())
}

/// `cloudflareEnvelope`'s success flag and errors.
fn envelope_fields(envelope: &J) -> Result<(bool, Vec<(i64, String)>), String> {
    let Some(map) = envelope.as_object() else {
        if envelope.is_null() {
            return Ok((false, Vec::new()));
        }
        return Err(format!(
            "json: cannot unmarshal {} into Go value of type main.cloudflareEnvelope",
            json_kind(envelope)
        ));
    };
    let mut success = false;
    let mut errors = Vec::new();
    for (key, value) in map {
        if crate::gojson::equal_fold(key, "success") {
            match value {
                J::Bool(b) => success = *b,
                J::Null => {}
                other => return Err(format!("json: cannot unmarshal {} into Go struct field cloudflareEnvelope.success of type bool", json_kind(other))),
            }
        } else if crate::gojson::equal_fold(key, "errors") {
            errors.clear();
            for item in value.as_array().cloned().unwrap_or_default() {
                let code = item.get("code").and_then(J::as_f64).map_or(0, |f| f as i64);
                let message = item
                    .get("message")
                    .and_then(J::as_str)
                    .unwrap_or("")
                    .to_string();
                errors.push((code, message));
            }
        }
    }
    Ok((success, errors))
}

/// `cloudflareDelete`: 404 is success; a non-2xx status or an unsuccessful
/// envelope is an error.
pub(super) fn delete(token: &str, endpoint: &str) -> Result<(), String> {
    let response = http_do(HttpRequest {
        method: "DELETE",
        url: endpoint,
        headers: vec![("Authorization", format!("Bearer {token}"))],
        body: None,
        timeout: None,
        https_redirects: None,
        limit: 64 * 1024,
    })?;
    if response.status == 404 {
        return Ok(());
    }
    if !(200..300).contains(&response.status) {
        return Err(format!(
            "HTTP {}: {}",
            response.status,
            String::from_utf8_lossy(&response.body).trim()
        ));
    }
    let envelope =
        unmarshal(&response.body).map_err(|e| format!("decode Cloudflare delete response: {e}"))?;
    let (success, errors) = envelope_fields(&envelope)
        .map_err(|e| format!("decode Cloudflare delete response: {e}"))?;
    if !success {
        if let Some((code, message)) = errors.first() {
            return Err(format!("Cloudflare API error {code}: {message}"));
        }
        return Err("Cloudflare API reported delete failure".into());
    }
    Ok(())
}

/// `cleanupExternalResources`: the teardown's finalize phase deletes the
/// tunnel and DNS records it is given, collecting every failure.
pub(super) fn cleanup_external_resources(inputs: &Map<String, J>) -> Result<(), String> {
    let tunnel_id = super::super::support::string_input(inputs, "tunnelId", "");
    let record_ids = string_slice_input(inputs, "dnsRecordIds");
    if tunnel_id.is_empty() && record_ids.is_empty() {
        return Ok(());
    }
    let get = |k: &str| std::env::var(k).unwrap_or_default().trim().to_string();
    let (account, zone, token) = (
        get("CLOUDFLARE_ACCOUNT_ID"),
        get("CLOUDFLARE_ZONE_ID"),
        get("CLOUDFLARE_API_TOKEN"),
    );
    if account.is_empty() || zone.is_empty() || token.is_empty() {
        return Err("external Cloudflare cleanup requires CLOUDFLARE_ACCOUNT_ID, CLOUDFLARE_ZONE_ID, and CLOUDFLARE_API_TOKEN".into());
    }
    let mut errors = Vec::new();
    if !tunnel_id.is_empty() {
        if !identifier_ok(&tunnel_id) {
            errors.push("invalid Cloudflare tunnel id".to_string());
        } else if let Err(e) = delete(&token, &format!("https://api.cloudflare.com/client/v4/accounts/{account}/cfd_tunnel/{tunnel_id}?force=true")) {
            errors.push(format!("delete Cloudflare tunnel: {e}"));
        }
    }
    for record_id in record_ids {
        if !identifier_ok(&record_id) {
            errors.push("invalid Cloudflare DNS record id".to_string());
            continue;
        }
        if let Err(e) = delete(
            &token,
            &format!("https://api.cloudflare.com/client/v4/zones/{zone}/dns_records/{record_id}"),
        ) {
            errors.push(format!("delete Cloudflare DNS record: {e}"));
        }
    }
    // errors.Join: one error per line.
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("\n"))
    }
}
