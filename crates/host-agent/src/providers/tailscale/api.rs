//! Tailscale's API (Go `tailscale_api.go`): devices and auth keys of one
//! tailnet, with a 60-second client.

use super::super::support::{go_any, http_do, HttpRequest};
use serde_json::{json, Value as J};
use std::time::Duration;

const DEFAULT_BASE: &str = "https://api.tailscale.com";
const DEFAULT_TAILNET: &str = "-";

#[derive(Clone, Default)]
pub(super) struct Device {
    pub id: String,
    pub name: String,
    pub hostname: String,
    pub addresses: Vec<String>,
}

pub(super) struct Client {
    api_key: String,
    tailnet: String,
    base_url: String,
}

impl Client {
    pub fn new(api_key: &str, tailnet: &str, base_url: &str) -> Client {
        let tailnet = match tailnet.trim() {
            "" => DEFAULT_TAILNET,
            t => t,
        };
        let base_url = match base_url.trim().trim_end_matches('/') {
            "" => DEFAULT_BASE,
            b => b,
        };
        Client {
            api_key: api_key.trim().into(),
            tailnet: tailnet.into(),
            base_url: base_url.into(),
        }
    }

    pub fn list_devices(&self) -> Result<Vec<Device>, String> {
        if self.api_key.is_empty() {
            return Err("tailscale API key is required".into());
        }
        let endpoint = format!(
            "{}/api/v2/tailnet/{}/devices",
            self.base_url,
            path_escape(&self.tailnet)
        );
        let payload = self
            .do_json("GET", &endpoint, None, true)
            .map_err(|e| format!("list Tailscale devices: {e}"))?;
        decode_devices(payload.as_ref())
            .map_err(|e| format!("list Tailscale devices: decode Tailscale API response: {e}"))
    }

    /// A reusable, preauthorized auth key.
    pub fn create_auth_key(&self, expiry_seconds: i64) -> Result<String, String> {
        if self.api_key.is_empty() {
            return Err("tailscale API key is required".into());
        }
        let expiry_seconds = if expiry_seconds <= 0 {
            24 * 60 * 60
        } else {
            expiry_seconds
        };
        let endpoint = format!(
            "{}/api/v2/tailnet/{}/keys",
            self.base_url,
            path_escape(&self.tailnet)
        );
        let body = crate::gojson::encode_to_string(&json!({
            "capabilities": {"devices": {"create": {"reusable": true, "ephemeral": false, "preauthorized": true}}},
            "expirySeconds": expiry_seconds,
        }));
        let payload = self
            .do_json("POST", &endpoint, Some(body), true)
            .map_err(|e| format!("create Tailscale auth key: {e}"))?;
        let fields = string_fields(payload.as_ref(), "TailscaleAuthKey", "", &["id", "key"])
            .map_err(|e| {
                format!("create Tailscale auth key: decode Tailscale API response: {e}")
            })?;
        if fields[1].trim().is_empty() {
            return Err("Tailscale auth key response did not include a key".into());
        }
        Ok(fields[1].clone())
    }

    pub fn delete_device(&self, device_id: &str) -> Result<(), String> {
        if self.api_key.is_empty() {
            return Err("tailscale API key is required".into());
        }
        let device_id = device_id.trim();
        if device_id.is_empty() {
            return Err("device id is required".into());
        }
        let endpoint = format!("{}/api/v2/device/{}", self.base_url, path_escape(device_id));
        self.do_json("DELETE", &endpoint, None, false)
            .map_err(|e| format!("delete Tailscale device: {e}"))?;
        Ok(())
    }

    /// `doJSON`: the decoded payload when the caller wants one and the
    /// response carried one (not empty, not `null`).
    fn do_json(
        &self,
        method: &str,
        endpoint: &str,
        body: Option<String>,
        want: bool,
    ) -> Result<Option<J>, String> {
        let body = body.filter(|b| !b.is_empty());
        let mut headers = vec![("Authorization", format!("Bearer {}", self.api_key))];
        if body.is_some() {
            headers.push(("Content-Type", "application/json".into()));
        }
        let response = http_do(HttpRequest {
            method,
            url: endpoint,
            headers,
            body: body.map(String::into_bytes),
            timeout: Some(Duration::from_secs(60)),
            https_redirects: None,
            limit: 4 << 20,
        })?;
        if response.status == 404 && method == "DELETE" {
            return Ok(None);
        }
        if !(200..300).contains(&response.status) {
            return Err(format!(
                "HTTP {}: {}",
                response.status,
                String::from_utf8_lossy(&response.body).trim()
            ));
        }
        if !want || response.body.is_empty() || response.body == b"null" {
            return Ok(None);
        }
        if let Some(e) = crate::gojson::syntax_error(&response.body) {
            return Err(format!("decode Tailscale API response: {e}"));
        }
        let node = crate::gojson::parse(&response.body).map_err(|_| {
            "decode Tailscale API response: unexpected end of JSON input".to_string()
        })?;
        Ok(Some(go_any(&node)))
    }
}

/// `url.PathEscape`.
fn path_escape(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        let keep = b.is_ascii_alphanumeric()
            || matches!(
                b,
                b'-' | b'_' | b'.' | b'~' | b'$' | b'&' | b'+' | b':' | b'=' | b'@'
            );
        if keep {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn kind(v: &J) -> &'static str {
    match v {
        J::Object(_) => "object",
        J::Array(_) => "array",
        J::String(_) => "string",
        J::Bool(_) => "bool",
        J::Null => "null",
        J::Number(_) => "number",
    }
}

/// A Go struct of string fields: names matched without case, a mistyped
/// field is the first `UnmarshalTypeError` (`path` is the field path of the
/// struct within the payload, empty at the top).
fn string_fields(
    value: Option<&J>,
    ty: &str,
    path: &str,
    fields: &[&str],
) -> Result<Vec<String>, String> {
    let mut out = vec![String::new(); fields.len()];
    let Some(value) = value else {
        return Ok(out);
    };
    let Some(map) = value.as_object() else {
        if value.is_null() {
            return Ok(out);
        }
        return Err(if path.is_empty() {
            format!(
                "json: cannot unmarshal {} into Go value of type main.{ty}",
                kind(value)
            )
        } else {
            format!(
                "json: cannot unmarshal {} into Go struct field .{path} of type main.{ty}",
                kind(value)
            )
        });
    };
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
                let field = if path.is_empty() {
                    fields[i].to_string()
                } else {
                    format!("{path}.{}", fields[i])
                };
                return Err(format!(
                    "json: cannot unmarshal {} into Go struct field {ty}.{field} of type string",
                    kind(other)
                ));
            }
        }
    }
    Ok(out)
}

/// The devices payload: `struct { Devices []TailscaleDevice }`.
fn decode_devices(payload: Option<&J>) -> Result<Vec<Device>, String> {
    let Some(payload) = payload else {
        return Ok(Vec::new());
    };
    let Some(map) = payload.as_object() else {
        return Err(format!(
            "json: cannot unmarshal {} into Go value of type struct {{ Devices []main.TailscaleDevice \"json:\\\"devices\\\"\" }}",
            kind(payload)
        ));
    };
    let mut devices = Vec::new();
    for (key, value) in map {
        if !crate::gojson::equal_fold(key, "devices") {
            continue;
        }
        devices = match value {
            J::Null => Vec::new(),
            J::Array(items) => items.iter().map(decode_device).collect::<Result<_, _>>()?,
            other => {
                return Err(format!(
                    "json: cannot unmarshal {} into Go struct field .devices of type []main.TailscaleDevice",
                    kind(other)
                ))
            }
        };
    }
    Ok(devices)
}

fn decode_device(value: &J) -> Result<Device, String> {
    let fields = ["id", "name", "hostname", "nodeKey", "user", "os"];
    let mut stripped = value.clone();
    let mut addresses = Vec::new();
    if let Some(map) = stripped.as_object_mut() {
        let keys: Vec<String> = map
            .keys()
            .filter(|k| crate::gojson::equal_fold(k, "addresses"))
            .cloned()
            .collect();
        for key in keys {
            match map.remove(&key) {
                Some(J::Null) | None => addresses = Vec::new(),
                Some(J::Array(items)) => {
                    addresses = Vec::new();
                    for item in items {
                        match item {
                            J::String(s) => addresses.push(s),
                            J::Null => addresses.push(String::new()),
                            other => {
                                return Err(format!(
                                    "json: cannot unmarshal {} into Go struct field TailscaleDevice.devices.addresses of type string",
                                    kind(&other)
                                ))
                            }
                        }
                    }
                }
                Some(other) => {
                    return Err(format!(
                        "json: cannot unmarshal {} into Go struct field TailscaleDevice.devices.addresses of type []string",
                        kind(&other)
                    ))
                }
            }
        }
    }
    let f = string_fields(Some(&stripped), "TailscaleDevice", "devices", &fields)?;
    Ok(Device {
        id: f[0].clone(),
        name: f[1].clone(),
        hostname: f[2].clone(),
        addresses,
    })
}

/// `findDeviceByHostname`: the first device whose hostname or name is the
/// hostname, starts with it and a dot, or has it as its first label.
pub(super) fn find_device_by_hostname(devices: &[Device], hostname: &str) -> Option<Device> {
    let hostname = hostname.trim().to_lowercase();
    if hostname.is_empty() {
        return None;
    }
    for device in devices {
        for candidate in [&device.hostname, &device.name] {
            let candidate = candidate.trim().to_lowercase();
            if candidate.is_empty() {
                continue;
            }
            if candidate == hostname
                || candidate.starts_with(&format!("{hostname}."))
                || candidate.split('.').next() == Some(hostname.as_str())
            {
                return Some(device.clone());
            }
        }
    }
    None
}

/// `deviceIPv4`: the first address that looks like dotted IPv4.
pub(super) fn device_ipv4(device: &Device) -> String {
    for address in &device.addresses {
        let ip = address.split('/').next().unwrap_or("").trim();
        let parsed = parse_ipv4(ip);
        if !parsed.is_empty() {
            return parsed;
        }
    }
    String::new()
}

fn parse_ipv4(raw: &str) -> String {
    let raw = raw.trim();
    if raw.is_empty() || raw.contains(':') || raw.split('.').count() != 4 {
        return String::new();
    }
    raw.to_string()
}
