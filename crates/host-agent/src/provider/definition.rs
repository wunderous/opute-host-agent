//! Typed provider definitions and validation from contracts/provider.
//! Go-compatible typed decoding retains null slices and canonical fields.
use super::ProviderRef;
use crate::goerr::quote;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeSet;
use std::ops::Deref;

/// A Go slice retains nil separately from an allocated empty slice.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct GoSlice<T>(pub Option<Vec<T>>);
impl<T> Deref for GoSlice<T> {
    type Target = [T];
    fn deref(&self) -> &[T] {
        self.0.as_deref().unwrap_or_default()
    }
}
impl<'a, T> IntoIterator for &'a GoSlice<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

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
    #[serde(skip_serializing_if = "String::is_empty")]
    pub executable: String,
    #[serde(skip_serializing_if = "slice_empty")]
    pub args: GoSlice<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub sha256: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PluginDescriptor {
    pub schema: String,
    pub plugin_id: String,
    pub version: String,
    pub capabilities: GoSlice<CapabilityRef>,
    #[serde(skip_serializing_if = "slice_empty")]
    pub requires: GoSlice<CapabilityRef>,
    pub server: ServerDescriptor,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct HostAgentRequirements {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub minimum_version: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RecipeSource {
    pub uri: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub revision: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub sha256: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RecipeRef {
    pub id: String,
    pub source: RecipeSource,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub mode: String,
    #[serde(skip_serializing_if = "map_empty")]
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
    pub provides: GoSlice<CapabilityRef>,
    #[serde(skip_serializing_if = "slice_empty")]
    pub requires: GoSlice<CapabilityRef>,
    pub host_agent: HostAgentRequirements,
    pub recipes: GoSlice<RecipeRef>,
    #[serde(skip_serializing_if = "slice_empty")]
    pub services: GoSlice<ServiceDefinition>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub teardown: Option<Operation>,
    pub validation: ValidationRef,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ServiceDefinition {
    pub id: String,
    pub capability_id: String,
    pub version: i64,
    pub operations: GoSlice<Operation>,
    #[serde(skip_serializing_if = "slice_empty")]
    pub dependencies: GoSlice<CapabilityRef>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ResultSelector {
    pub id: String,
    pub source_path: String,
    pub cardinality: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub label_path: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ResultType {
    pub id: String,
    pub version: i64,
    #[serde(skip_serializing_if = "slice_empty")]
    pub selectors: GoSlice<ResultSelector>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ResourceBinding {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub argument: String,
    pub resource_type: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub source_path: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub selector_id: String,
    #[serde(skip_serializing_if = "is_false")]
    pub required: bool,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ResourceCostArgumentBindings {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub cpu_cores: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub memory_bytes: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub disk_bytes: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub tasks: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ResourceCost {
    #[serde(skip_serializing_if = "is_zero_f64")]
    pub cpu_cores: f64,
    #[serde(skip_serializing_if = "is_zero_i64")]
    pub memory_bytes: i64,
    #[serde(skip_serializing_if = "is_zero_i64")]
    pub disk_bytes: i64,
    #[serde(skip_serializing_if = "is_zero_i64")]
    pub tasks: i64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub class: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub argument_bindings: Option<ResourceCostArgumentBindings>,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Operation {
    pub id: String,
    pub version: i64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub description: String,
    pub input_schema: Option<Map<String, Value>>,
    pub output_schema: Option<Map<String, Value>>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub output_type: String,
    #[serde(skip_serializing_if = "slice_empty")]
    pub result_types: GoSlice<ResultType>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub validation_schema: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub observation_schema: String,
    pub effect: String,
    #[serde(skip_serializing_if = "slice_empty")]
    pub resource_kinds: GoSlice<String>,
    #[serde(skip_serializing_if = "slice_empty")]
    pub requires: GoSlice<ResourceBinding>,
    #[serde(skip_serializing_if = "slice_empty")]
    pub produces: GoSlice<ResourceBinding>,
    pub idempotent: bool,
    pub supports_readiness: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_cost: Option<ResourceCost>,
    #[serde(skip_serializing_if = "is_false")]
    pub supports_streaming: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub task_support: String,
}

#[derive(Clone, Copy)]
enum Ty {
    String(&'static str),
    Int(&'static str),
    Float,
    Bool,
    Map,
    Struct(&'static str, bool),
    Slice(&'static str),
}
fn fields(name: &str) -> &'static [(&'static str, Ty)] {
    match name {
        "CapabilityRef" => &[("id", Ty::String("string")), ("version", Ty::Int("int"))],
        "ServerDescriptor" => &[
            ("transport", Ty::String("string")),
            ("endpoint", Ty::String("string")),
            ("executable", Ty::String("string")),
            ("args", Ty::Slice("String")),
            ("sha256", Ty::String("string")),
        ],
        "PluginDescriptor" => &[
            ("schema", Ty::String("string")),
            ("pluginId", Ty::String("string")),
            ("version", Ty::String("string")),
            ("capabilities", Ty::Slice("CapabilityRef")),
            ("requires", Ty::Slice("CapabilityRef")),
            ("server", Ty::Struct("ServerDescriptor", false)),
        ],
        "HostAgentRequirements" => &[("minimumVersion", Ty::String("string"))],
        "RecipeSource" => &[
            ("uri", Ty::String("string")),
            ("revision", Ty::String("string")),
            ("sha256", Ty::String("string")),
        ],
        "RecipeRef" => &[
            ("id", Ty::String("string")),
            ("source", Ty::Struct("RecipeSource", false)),
            ("mode", Ty::String("string")),
            ("inputs", Ty::Map),
        ],
        "ValidationRef" => &[
            ("capability", Ty::String("string")),
            ("operation", Ty::String("string")),
        ],
        "InstallManifest" => &[
            ("schema", Ty::String("string")),
            ("provider", Ty::Struct("ProviderRef", false)),
            ("provides", Ty::Slice("CapabilityRef")),
            ("requires", Ty::Slice("CapabilityRef")),
            ("hostAgent", Ty::Struct("HostAgentRequirements", false)),
            ("recipes", Ty::Slice("RecipeRef")),
            ("services", Ty::Slice("ServiceDefinition")),
            ("teardown", Ty::Struct("Operation", true)),
            ("validation", Ty::Struct("ValidationRef", false)),
        ],
        "ServiceDefinition" => &[
            ("id", Ty::String("string")),
            ("capabilityId", Ty::String("string")),
            ("version", Ty::Int("int")),
            ("operations", Ty::Slice("Operation")),
            ("dependencies", Ty::Slice("CapabilityRef")),
        ],
        "ResultSelector" => &[
            ("id", Ty::String("string")),
            ("sourcePath", Ty::String("string")),
            ("cardinality", Ty::String("capability.Cardinality")),
            ("labelPath", Ty::String("string")),
        ],
        "ResultType" => &[
            ("id", Ty::String("string")),
            ("version", Ty::Int("int")),
            ("selectors", Ty::Slice("ResultSelector")),
        ],
        "ResourceBinding" => &[
            ("argument", Ty::String("string")),
            ("resourceType", Ty::String("string")),
            ("sourcePath", Ty::String("string")),
            ("selectorId", Ty::String("string")),
            ("required", Ty::Bool),
        ],
        "ResourceCostArgumentBindings" => &[
            ("cpuCores", Ty::String("string")),
            ("memoryBytes", Ty::String("string")),
            ("diskBytes", Ty::String("string")),
            ("tasks", Ty::String("string")),
        ],
        "ResourceCost" => &[
            ("cpuCores", Ty::Float),
            ("memoryBytes", Ty::Int("int64")),
            ("diskBytes", Ty::Int("int64")),
            ("tasks", Ty::Int("int64")),
            ("class", Ty::String("string")),
            (
                "argumentBindings",
                Ty::Struct("ResourceCostArgumentBindings", true),
            ),
        ],
        "Operation" => &[
            ("id", Ty::String("string")),
            ("version", Ty::Int("int")),
            ("description", Ty::String("string")),
            ("inputSchema", Ty::Map),
            ("outputSchema", Ty::Map),
            ("outputType", Ty::String("string")),
            ("resultTypes", Ty::Slice("ResultType")),
            ("validationSchema", Ty::String("string")),
            ("observationSchema", Ty::String("string")),
            ("effect", Ty::String("string")),
            ("resourceKinds", Ty::Slice("String")),
            ("requires", Ty::Slice("ResourceBinding")),
            ("produces", Ty::Slice("ResourceBinding")),
            ("idempotent", Ty::Bool),
            ("supportsReadiness", Ty::Bool),
            ("resourceCost", Ty::Struct("ResourceCost", true)),
            ("supportsStreaming", Ty::Bool),
            ("taskSupport", Ty::String("string")),
        ],
        "ProviderRef" => &[
            ("id", Ty::String("string")),
            ("version", Ty::String("string")),
        ],
        _ => unreachable!("unknown provider definition"),
    }
}
fn go_type(name: &str) -> String {
    if name == "String" {
        return "string".into();
    }
    format!(
        "{}.{}",
        if matches!(name, "ResultType" | "ResultSelector") {
            "capability"
        } else {
            "provider"
        },
        name
    )
}
fn type_error(node: &crate::gojson::Node, ty: &str, owner: &str, path: &str) -> String {
    use crate::gojson::Value as G;
    let value = match &node.value {
        G::Bool(_) => "bool".into(),
        G::Number(n) if matches!(ty, "int" | "int64" | "float64") => format!("number {n}"),
        G::Number(_) => "number".into(),
        G::String(_) => "string".into(),
        G::Array(_) => "array".into(),
        G::Object(_) => "object".into(),
        G::Null => "null".into(),
    };
    if path.is_empty() {
        format!("json: cannot unmarshal {value} into Go value of type {ty}")
    } else {
        format!("json: cannot unmarshal {value} into Go struct field {owner}.{path} of type {ty}")
    }
}
fn dynamic(node: &crate::gojson::Node, owner: &str, path: &str) -> Result<Value, String> {
    use crate::gojson::Value as G;
    Ok(match &node.value {
        G::Null => Value::Null,
        G::Bool(b) => Value::Bool(*b),
        G::String(s) => Value::String(s.clone()),
        G::Number(n) => {
            let f = n
                .parse::<f64>()
                .ok()
                .filter(|f| f.is_finite())
                .ok_or_else(|| type_error(node, "float64", owner, path))?;
            crate::gojson::float_value(f)
        }
        G::Array(a) => Value::Array(
            a.iter()
                .map(|n| dynamic(n, owner, path))
                .collect::<Result<_, _>>()?,
        ),
        G::Object(o) => {
            let mut m = Map::new();
            for (k, n) in o {
                m.insert(k.clone(), dynamic(n, owner, path)?);
            }
            Value::Object(m)
        }
    })
}
#[derive(Default)]
struct DecodeState {
    slices: std::collections::BTreeMap<String, Vec<Value>>,
}

fn decode_struct(
    node: &crate::gojson::Node,
    name: &str,
    current: Value,
    path: &str,
    owner: &str,
    state: &mut DecodeState,
    location: &str,
) -> Result<Value, String> {
    if node.is_null() {
        return Ok(current);
    }
    let entries = node
        .as_object()
        .ok_or_else(|| type_error(node, &go_type(name), owner, path))?;
    let mut value = current.as_object().cloned().unwrap_or_default();
    for (key, node) in entries {
        let Some((field, ty)) = fields(name)
            .iter()
            .find(|(field, _)| *field == key)
            .or_else(|| {
                fields(name)
                    .iter()
                    .find(|(field, _)| crate::gojson::equal_fold(field, key))
            })
        else {
            continue;
        };
        let path = if path.is_empty() {
            field.to_string()
        } else {
            format!("{path}.{field}")
        };
        let previous = value.remove(*field).unwrap_or(Value::Null);
        value.insert(
            field.to_string(),
            decode_field(
                node,
                *ty,
                previous,
                name,
                &path,
                state,
                &format!("{location}.{field}"),
            )?,
        );
    }
    Ok(Value::Object(value))
}
fn decode_field(
    node: &crate::gojson::Node,
    ty: Ty,
    previous: Value,
    owner: &str,
    path: &str,
    state: &mut DecodeState,
    location: &str,
) -> Result<Value, String> {
    use crate::gojson::Value as G;
    if node.is_null() {
        return Ok(match ty {
            Ty::Map | Ty::Slice(_) | Ty::Struct(_, true) => Value::Null,
            Ty::String(_) => {
                if previous.is_null() {
                    Value::String(String::new())
                } else {
                    previous
                }
            }
            Ty::Int(_) => {
                if previous.is_null() {
                    Value::from(0)
                } else {
                    previous
                }
            }
            Ty::Float => {
                if previous.is_null() {
                    Value::from(0.0)
                } else {
                    previous
                }
            }
            Ty::Bool => {
                if previous.is_null() {
                    Value::Bool(false)
                } else {
                    previous
                }
            }
            Ty::Struct(_, false) => {
                if previous.is_null() {
                    Value::Object(Map::new())
                } else {
                    previous
                }
            }
        });
    }
    let wrong = |name: &str| type_error(node, name, owner, path);
    match ty {
        Ty::String(name) => match &node.value {
            G::String(s) => Ok(Value::String(s.clone())),
            _ => Err(wrong(name)),
        },
        Ty::Bool => match node.value {
            G::Bool(b) => Ok(Value::Bool(b)),
            _ => Err(wrong("bool")),
        },
        Ty::Int(name) => match &node.value {
            G::Number(n) => n.parse::<i64>().map(Value::from).map_err(|_| wrong(name)),
            _ => Err(wrong(name)),
        },
        Ty::Float => match &node.value {
            G::Number(n) => n
                .parse::<f64>()
                .ok()
                .filter(|f| f.is_finite())
                .map(crate::gojson::float_value)
                .ok_or_else(|| wrong("float64")),
            _ => Err(wrong("float64")),
        },
        Ty::Struct(name, _) => decode_struct(node, name, previous, path, owner, state, location),
        Ty::Slice(name) => {
            let G::Array(items) = &node.value else {
                return Err(wrong(&format!("[]{}", go_type(name))));
            };
            // encoding/json reuses the backing slice, including elements
            // outside its current length after a preceding duplicate shrank it.
            let mut backing = if previous.is_array() {
                state
                    .slices
                    .remove(location)
                    .unwrap_or_else(|| previous.as_array().unwrap().clone())
            } else {
                Vec::new()
            };
            for (i, node) in items.iter().enumerate() {
                let prior = backing.get(i).cloned().unwrap_or_else(|| {
                    if name == "String" {
                        Value::String(String::new())
                    } else {
                        Value::Object(Map::new())
                    }
                });
                let value = if name == "String" {
                    decode_field(
                        node,
                        Ty::String("string"),
                        prior,
                        owner,
                        path,
                        state,
                        &format!("{location}[{i}]"),
                    )?
                } else {
                    decode_struct(
                        node,
                        name,
                        prior,
                        path,
                        owner,
                        state,
                        &format!("{location}[{i}]"),
                    )?
                };
                if i < backing.len() {
                    backing[i] = value;
                } else {
                    backing.push(value);
                }
            }
            let value = Value::Array(backing[..items.len()].to_vec());
            if items.is_empty() {
                backing.clear();
            }
            state.slices.insert(location.to_string(), backing);
            Ok(value)
        }

        Ty::Map => {
            let entries = node
                .as_object()
                .ok_or_else(|| wrong("map[string]interface {}"))?;
            let mut map = previous.as_object().cloned().unwrap_or_default();
            for (k, n) in entries {
                map.insert(k.clone(), dynamic(n, owner, path)?);
            }
            Ok(Value::Object(map))
        }
    }
}
// Field order is part of json.Marshal's output and therefore manifest identity.
fn encode_definition(name: &str, value: &Value, output: &mut String) {
    let Some(map) = value.as_object() else {
        crate::gojson::encode(value, output);
        return;
    };
    output.push('{');
    let mut first = true;
    for (field, ty) in fields(name) {
        let Some(value) = map.get(*field) else {
            continue;
        };
        if !first {
            output.push(',');
        }
        first = false;
        crate::gojson::encode_string(field, output);
        output.push(':');
        match ty {
            Ty::Struct(name, _) => encode_definition(name, value, output),
            Ty::Slice(name) if *name != "String" && value.is_array() => {
                output.push('[');
                for (i, item) in value.as_array().unwrap().iter().enumerate() {
                    if i > 0 {
                        output.push(',');
                    }
                    encode_definition(name, item, output);
                }
                output.push(']');
            }
            _ => crate::gojson::encode(value, output),
        }
    }
    output.push('}');
}
pub fn encode_descriptor(descriptor: &PluginDescriptor) -> String {
    let mut output = String::new();
    encode_definition(
        "PluginDescriptor",
        &serde_json::to_value(descriptor).expect("finite descriptor"),
        &mut output,
    );
    output
}
pub fn encode_manifest(manifest: &InstallManifest) -> Result<String, String> {
    for operation in manifest
        .services
        .iter()
        .flat_map(|s| s.operations.iter())
        .chain(manifest.teardown.iter())
    {
        if let Some(cost) = &operation.resource_cost {
            if !cost.cpu_cores.is_finite() {
                let name = if cost.cpu_cores.is_nan() {
                    "NaN"
                } else if cost.cpu_cores.is_sign_positive() {
                    "+Inf"
                } else {
                    "-Inf"
                };
                return Err(format!("json: unsupported value: {name}"));
            }
        }
    }
    let mut output = String::new();
    encode_definition(
        "InstallManifest",
        &serde_json::to_value(manifest).expect("finite manifest"),
        &mut output,
    );
    Ok(output)
}

pub fn decode_descriptor(node: &crate::gojson::Node) -> Result<PluginDescriptor, String> {
    let value = decode_struct(
        node,
        "PluginDescriptor",
        Value::Object(Map::new()),
        "",
        "",
        &mut DecodeState::default(),
        "",
    )?;
    serde_json::from_value(value).map_err(|e| format!("internal provider descriptor decoding: {e}"))
}
pub fn decode_manifest(node: &crate::gojson::Node) -> Result<InstallManifest, String> {
    let value = decode_struct(
        node,
        "InstallManifest",
        Value::Object(Map::new()),
        "",
        "",
        &mut DecodeState::default(),
        "",
    )?;
    serde_json::from_value(value).map_err(|e| format!("internal provider manifest decoding: {e}"))
}

fn map_empty(value: &Option<Map<String, Value>>) -> bool {
    value.as_ref().is_none_or(Map::is_empty)
}

fn is_zero_i64(value: &i64) -> bool {
    *value == 0
}
fn is_zero_f64(value: &f64) -> bool {
    *value == 0.0
}
fn is_false(value: &bool) -> bool {
    !value
}
fn slice_empty<T>(value: &GoSlice<T>) -> bool {
    value.is_empty()
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
                if matches!(
                    r["kind"].as_str(),
                    Some("wire-descriptor" | "wire-manifest")
                ) {
                    let node =
                        crate::gojson::parse(r["value"].as_str().unwrap().as_bytes()).unwrap();
                    let decoded = if r["kind"] == "wire-descriptor" {
                        decode_descriptor(&node)
                            .map(|d| (serde_json::to_value(&d).unwrap(), encode_descriptor(&d)))
                    } else {
                        decode_manifest(&node).map(|m| {
                            (
                                serde_json::to_value(&m).unwrap(),
                                encode_manifest(&m).unwrap(),
                            )
                        })
                    };
                    return match decoded {
                        Ok((d, encoded)) => serde_json::json!({"decoded":d,"encoded":encoded}),
                        Err(e) => serde_json::json!({"error":e}),
                    };
                }
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
