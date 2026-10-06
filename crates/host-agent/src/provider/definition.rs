//! Typed provider definitions and validation from contracts/provider.
//! These validators accept typed data; the transport owns wire decoding.
use super::ProviderRef;
use crate::goerr::quote;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CapabilityRef {
    pub id: String,
    pub version: i64,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ServerDescriptor {
    pub transport: String,
    pub endpoint: String,
    pub executable: String,
    pub args: Vec<String>,
    pub sha256: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PluginDescriptor {
    pub schema: String,
    pub plugin_id: String,
    pub version: String,
    pub capabilities: Vec<CapabilityRef>,
    pub requires: Vec<CapabilityRef>,
    pub server: ServerDescriptor,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct HostAgentRequirements {
    pub minimum_version: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RecipeSource {
    pub uri: String,
    pub revision: String,
    pub sha256: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RecipeRef {
    pub id: String,
    pub source: RecipeSource,
    pub mode: String,
    pub inputs: Option<Map<String, Value>>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ValidationRef {
    pub capability: String,
    pub operation: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct InstallManifest {
    pub schema: String,
    pub provider: ProviderRef,
    pub provides: Vec<CapabilityRef>,
    pub requires: Vec<CapabilityRef>,
    pub host_agent: HostAgentRequirements,
    pub recipes: Vec<RecipeRef>,
    pub services: Vec<ServiceDefinition>,
    pub teardown: Option<Operation>,
    pub validation: ValidationRef,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ServiceDefinition {
    pub id: String,
    pub capability_id: String,
    pub version: i64,
    pub operations: Vec<Operation>,
    pub dependencies: Vec<CapabilityRef>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ResultSelector {
    pub id: String,
    pub source_path: String,
    pub cardinality: String,
    pub label_path: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ResultType {
    pub id: String,
    pub version: i64,
    pub selectors: Vec<ResultSelector>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ResourceBinding {
    pub argument: String,
    pub resource_type: String,
    pub source_path: String,
    pub selector_id: String,
    pub required: bool,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ResourceCostArgumentBindings {
    pub cpu_cores: String,
    pub memory_bytes: String,
    pub disk_bytes: String,
    pub tasks: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ResourceCost {
    pub cpu_cores: f64,
    pub memory_bytes: i64,
    pub disk_bytes: i64,
    pub tasks: i64,
    pub class: String,
    pub argument_bindings: Option<ResourceCostArgumentBindings>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Operation {
    pub id: String,
    pub version: i64,
    pub description: String,
    pub input_schema: Option<Map<String, Value>>,
    pub output_schema: Option<Map<String, Value>>,
    pub output_type: String,
    pub result_types: Vec<ResultType>,
    pub validation_schema: String,
    pub observation_schema: String,
    pub effect: String,
    pub resource_kinds: Vec<String>,
    pub requires: Vec<ResourceBinding>,
    pub produces: Vec<ResourceBinding>,
    pub idempotent: bool,
    pub supports_readiness: bool,
    pub resource_cost: Option<ResourceCost>,
    pub supports_streaming: bool,
    pub task_support: String,
}

pub fn service_key(provider: &str, service: &str) -> String {
    format!("{}/{}", provider.trim(), service.trim())
}
fn identifier(s: &str, operation: bool) -> bool {
    let s = s.trim().as_bytes();
    (3..=128).contains(&s.len())
        && s[0].is_ascii_lowercase()
        && s[1..].iter().all(|b| {
            b.is_ascii_lowercase()
                || b.is_ascii_digit()
                || matches!(b, b'.' | b'-')
                || (operation && *b == b'_')
        })
}
fn capability_refs(refs: &[CapabilityRef]) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    for r in refs {
        if !identifier(&r.id, false) || r.version < 1 {
            return Err("invalid capability reference".into());
        }
        if !seen.insert(&r.id) {
            return Err(format!("duplicate capability reference {}", quote(&r.id)));
        }
    }
    Ok(())
}
pub fn validate_descriptor(d: &PluginDescriptor) -> Result<(), String> {
    if d.schema != "opute-provider-plugin.v1" {
        return Err(format!(
            "unsupported provider descriptor schema {}",
            quote(&d.schema)
        ));
    }
    if !identifier(&d.plugin_id, false) {
        return Err("invalid provider pluginId".into());
    }
    if d.version.trim().is_empty() {
        return Err("provider version is required".into());
    }
    if d.server.transport != "streamable_http" {
        return Err("provider transport must be streamable_http".into());
    }
    validate_endpoint(&d.server.endpoint)?;
    if d.capabilities.is_empty() {
        return Err("provider must declare at least one capability".into());
    }
    capability_refs(&d.capabilities)
}
// net/url (Go 1.25) accepts different host spellings from the WHATWG URL
// parser. Keep its validation here; the adapter must still dial explicitly.
fn host_byte_allowed(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"!$&'()*+,;=:[ ]<>\"-_.~".contains(&b) && b != b' '
}
fn unescape_component(raw: &str, mode: u8) -> Option<Vec<u8>> {
    let bytes = raw.as_bytes();
    let mut result = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'%' {
            let digits = bytes.get(i + 1..i + 3)?;
            if !digits.iter().all(u8::is_ascii_hexdigit) {
                return None;
            }
            let value = u8::from_str_radix(std::str::from_utf8(digits).ok()?, 16).ok()?;
            if mode == 1 && value < 128 && value != b'%' {
                return None;
            }
            if mode == 2 && value != b'%' && value != b' ' && !host_byte_allowed(value) {
                return None;
            }
            result.push(value);
            i += 3;
        } else {
            if mode != 0 && b < 128 && !host_byte_allowed(b) {
                return None;
            }
            result.push(b);
            i += 1;
        }
    }
    Some(result)
}
fn optional_port(s: &str) -> bool {
    s.is_empty()
        || s.strip_prefix(':')
            .is_some_and(|p| p.bytes().all(|b| b.is_ascii_digit()))
}
fn validate_endpoint(raw: &str) -> Result<(), String> {
    let invalid = || "provider endpoint must be an absolute http(s) URL".to_string();
    let s = raw.trim();
    if s.bytes().any(|b| b < 32 || b == 127) {
        return Err(invalid());
    }
    let (scheme, rest) = s.split_once(':').ok_or_else(invalid)?;
    if !matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https") {
        return Err(invalid());
    }
    let rest = rest.strip_prefix("//").ok_or_else(invalid)?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    if host.is_empty() {
        return Err(invalid());
    }
    if let Some(open) = host.rfind('[') {
        let close = host.rfind(']').filter(|c| *c > open).ok_or_else(invalid)?;
        if !optional_port(&host[close + 1..]) {
            return Err(invalid());
        }
        let hostname = &host[open + 1..close];
        let decoded = if let Some(zone) = hostname.find("%25") {
            let mut bytes = unescape_component(&hostname[..zone], 1).ok_or_else(invalid)?;
            bytes.extend(unescape_component(&hostname[zone..], 2).ok_or_else(invalid)?);
            bytes
        } else {
            unescape_component(hostname, 1).ok_or_else(invalid)?
        };
        let decoded = std::str::from_utf8(&decoded).map_err(|_| invalid())?;
        let (address, zone) = decoded
            .split_once('%')
            .map_or((decoded, None), |(a, z)| (a, Some(z)));
        if zone.is_some_and(str::is_empty) || address.parse::<std::net::Ipv6Addr>().is_err() {
            return Err(invalid());
        }
    } else {
        if let Some(colon) = host.rfind(':') {
            if !optional_port(&host[colon..]) {
                return Err(invalid());
            }
        }
        unescape_component(host, 1).ok_or_else(invalid)?;
    }
    let path_and_query = &rest[authority.len()..];
    let path = path_and_query.split(['?', '#']).next().unwrap_or_default();
    unescape_component(path, 0).ok_or_else(invalid)?;
    if let Some((_, fragment)) = rest.split_once('#') {
        unescape_component(fragment, 0).ok_or_else(invalid)?;
    }
    if let Some((user, _)) = authority.rsplit_once('@') {
        if !user
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-._:~!$&'()*+,;=%@".contains(&b))
        {
            return Err(invalid());
        }
        unescape_component(user, 0).ok_or_else(invalid)?;
        return Err("provider endpoint must not embed credentials".into());
    }
    Ok(())
}
pub fn validate_install_manifest(
    m: &InstallManifest,
    expected: &ProviderRef,
) -> Result<(), String> {
    if m.schema != "opute-provider-install-manifest.v1" {
        return Err(format!(
            "unsupported provider install manifest schema {}",
            quote(&m.schema)
        ));
    }
    if &m.provider != expected {
        return Err("install manifest provider identity does not match trusted descriptor".into());
    }
    if m.provides.is_empty() {
        return Err("install manifest must provide at least one capability".into());
    }
    capability_refs(&m.provides)?;
    if m.validation.capability.trim().is_empty() || m.validation.operation.trim().is_empty() {
        return Err("install manifest validation capability and operation are required".into());
    }
    for r in &m.recipes {
        if r.id.trim().is_empty() || r.source.uri.trim().is_empty() {
            return Err("install manifest recipes require id and source.uri".into());
        }
        if r.source.revision.trim().is_empty() || r.source.sha256.trim().is_empty() {
            return Err(format!(
                "install manifest recipe {} must pin revision and sha256",
                quote(&r.id)
            ));
        }
    }
    validate_services(&m.services)?;
    if let Some(t) = &m.teardown {
        if t.id != "opute.provider.teardown" {
            return Err("provider teardown operation must be opute.provider.teardown".into());
        }
        validate_operation(t, "provider teardown")?;
        if t.effect != "destructive" {
            return Err("provider teardown operation must be destructive".into());
        }
    }
    Ok(())
}
fn validate_services(services: &[ServiceDefinition]) -> Result<(), String> {
    if services.is_empty() {
        return Err("provider manifest must declare at least one service".into());
    }
    let mut seen_services = BTreeSet::new();
    let mut seen_ops = BTreeSet::new();
    for s in services {
        if !identifier(&s.id, false) || s.version < 1 {
            return Err("invalid provider service definition".into());
        }
        if !identifier(&s.capability_id, false) {
            return Err(format!(
                "provider service {} must declare a neutral capabilityId",
                quote(&s.id)
            ));
        }
        if !seen_services.insert(&s.id) {
            return Err(format!("duplicate provider service {}", quote(&s.id)));
        }
        for d in &s.dependencies {
            if !identifier(&d.id, false) || d.version < 1 {
                return Err(format!(
                    "provider service {} declares an invalid dependency",
                    quote(&s.id)
                ));
            }
            if d.id.trim() == s.capability_id.trim() {
                return Err(format!(
                    "provider service {} depends on itself",
                    quote(&s.id)
                ));
            }
        }
        for o in &s.operations {
            validate_operation(o, &format!("provider service {}", s.id))?;
            if !seen_ops.insert(&o.id) {
                return Err(format!("duplicate provider operation {}", quote(&o.id)));
            }
        }
    }
    Ok(())
}
fn result_types(o: &Operation) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    for t in &o.result_types {
        if t.id.trim().is_empty() || t.version < 1 || !seen.insert(&t.id) {
            return Err("result types require unique ids and positive versions".into());
        }
        let mut selectors = BTreeSet::new();
        for s in &t.selectors {
            if s.id.trim().is_empty() || s.source_path.trim().is_empty() || !selectors.insert(&s.id)
            {
                return Err(format!(
                    "result type {} contains an invalid selector",
                    quote(&t.id)
                ));
            }
            let cardinality = if s.cardinality.trim().is_empty() {
                "one"
            } else {
                &s.cardinality
            };
            if !matches!(cardinality, "one" | "many") {
                return Err(format!(
                    "selector {} has unsupported cardinality {}",
                    quote(&s.id),
                    quote(&s.cardinality)
                ));
            }
        }
    }
    if !o.output_type.is_empty() && !seen.contains(&o.output_type) {
        return Err(format!(
            "output type {} is not declared",
            quote(&o.output_type)
        ));
    }
    if !o.result_types.is_empty() && o.output_type.is_empty() {
        return Err("result selectors require an output type".into());
    }
    Ok(())
}
fn schema_path<'a>(schema: &'a Map<String, Value>, path: &str) -> Option<&'a Map<String, Value>> {
    if path.starts_with('.') || path.ends_with('.') || path.contains(['[', ']', '/']) {
        return None;
    }
    let mut current = schema;
    for segment in path.split('.') {
        if segment.is_empty() {
            return None;
        }
        current = current
            .get("properties")?
            .as_object()?
            .get(segment)?
            .as_object()?;
    }
    Some(current)
}
fn resource_cost(o: &Operation) -> Result<(), String> {
    let Some(c) = &o.resource_cost else {
        return Ok(());
    };
    let id = quote(&o.id);
    if !c.cpu_cores.is_finite() || c.cpu_cores < 0.0 {
        return Err(format!(
            "provider operation {id} has an invalid resourceCost.cpuCores"
        ));
    }
    // Go iterates a map here. With multiple negatives, the specific field is
    // nondeterministic; no public invariant depends on its iteration order.
    for (field, value) in [
        ("memoryBytes", c.memory_bytes),
        ("diskBytes", c.disk_bytes),
        ("tasks", c.tasks),
    ] {
        if value < 0 {
            return Err(format!(
                "provider operation {id} has a negative resourceCost.{field}"
            ));
        }
    }
    if !matches!(c.class.trim(), "" | "control" | "normal" | "heavy") {
        return Err(format!(
            "provider operation {id} has unsupported resourceCost.class {}",
            quote(&c.class)
        ));
    }
    if let Some(b) = &c.argument_bindings {
        for (field, path, string_allowed, fallback) in [
            ("cpuCores", &b.cpu_cores, false, c.cpu_cores > 0.0),
            ("memoryBytes", &b.memory_bytes, true, c.memory_bytes > 0),
            ("diskBytes", &b.disk_bytes, true, true),
            ("tasks", &b.tasks, false, c.tasks > 0),
        ] {
            let path = path.trim();
            if path.is_empty() {
                continue;
            }
            if !fallback {
                return Err(format!("provider operation {id} resourceCost.{field} requires a positive static fallback"));
            }
            let property=schema_path(o.input_schema.as_ref().expect("validated schema"),path).ok_or_else(||format!("provider operation {id} resourceCost.{field} binding path {} is absent from inputSchema",quote(path)))?;
            let kind = property
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if !(matches!(kind, "integer" | "number") || string_allowed && kind == "string") {
                return Err(format!("provider operation {id} resourceCost.{field} binding path {} has unsupported type {}",quote(path),quote(kind)));
            }
        }
    }
    Ok(())
}
fn validate_operation(o: &Operation, owner: &str) -> Result<(), String> {
    if !identifier(&o.id, true)
        || o.version < 1
        || o.input_schema.is_none()
        || o.output_schema.is_none()
    {
        return Err(format!("{owner} contains an invalid operation"));
    }
    result_types(o).map_err(|e| format!("{owner} result selectors: {e}"))?;
    if !matches!(
        o.effect.as_str(),
        "read" | "mutation" | "destructive" | "credential_bearing"
    ) {
        return Err(format!(
            "provider operation {} has unsupported effect {}",
            quote(&o.id),
            quote(&o.effect)
        ));
    }
    if !matches!(o.task_support.as_str(), "" | "sync_only" | "bridged") {
        return Err(format!(
            "provider operation {} has unsupported taskSupport {}",
            quote(&o.id),
            quote(&o.task_support)
        ));
    }
    resource_cost(o)?;
    for b in &o.requires {
        if b.resource_type.trim().is_empty() {
            return Err(format!(
                "provider operation {} has a resource binding without resourceType",
                quote(&o.id)
            ));
        }
        if b.argument.trim().is_empty() && b.source_path.trim().is_empty() {
            return Err(format!(
                "provider operation {} has a resource binding without argument or sourcePath",
                quote(&o.id)
            ));
        }
        if !b.selector_id.is_empty() {
            return Err(format!(
                "provider operation {} requires bindings cannot reference result selectors",
                quote(&o.id)
            ));
        }
    }
    for b in &o.produces {
        if b.resource_type.trim().is_empty() {
            return Err(format!(
                "provider operation {} has a resource binding without resourceType",
                quote(&o.id)
            ));
        }
        if b.source_path.trim().is_empty() {
            return Err(format!(
                "provider operation {} has a produced binding without sourcePath",
                quote(&o.id)
            ));
        }
        let known = o
            .result_types
            .iter()
            .filter(|t| t.id == o.output_type)
            .any(|t| t.selectors.iter().any(|s| s.id == b.selector_id));
        if !b.selector_id.is_empty() && (o.output_type.is_empty() || !known) {
            return Err(format!(
                "provider operation {} references unknown selector {}",
                quote(&o.id),
                quote(&b.selector_id)
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires the pinned Go manifest differential driver"]
    fn definition_fixture() {
        let input =
            std::fs::read(std::env::var("PARITY_PROVIDER_DEFINITION_INPUT").unwrap()).unwrap();
        let rows: Vec<Value> = serde_json::from_slice(&input).unwrap();
        let out: Vec<Value> = rows
            .into_iter()
            .map(|r| {
                let result = if r["kind"] == "descriptor" {
                    validate_descriptor(&serde_json::from_value(r["value"].clone()).unwrap())
                } else {
                    validate_install_manifest(
                        &serde_json::from_value(r["value"].clone()).unwrap(),
                        &serde_json::from_value(r["expected"].clone()).unwrap(),
                    )
                };
                match result {
                    Ok(()) => serde_json::json!({"valid":true}),
                    Err(e) => serde_json::json!({"error":e}),
                }
            })
            .collect();
        std::fs::write(
            std::env::var("PARITY_PROVIDER_DEFINITION_OUTPUT").unwrap(),
            serde_json::to_vec_pretty(&out).unwrap(),
        )
        .unwrap();
    }
}
