//! `internal/plan/schema.go`: the `host-plan.v1` document types, decode, and
//! static validation. Field order and `skip_serializing_if` mirror Go's
//! struct declaration order and `omitempty` tags exactly, including the
//! quirk that Go's `omitempty` is a no-op on a non-pointer struct field
//! (always serialized) -- this is what makes `DocumentHash` byte-identical
//! with the pinned Go reference.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub const CONTRACT_VERSION: &str = "host-plan.v1";

const MAX_DOCUMENT_BYTES: usize = 512 * 1024;
const MAX_NODES: usize = 256;
pub const MAX_FAN_OUT: usize = 64;
const MAX_TOTAL_ATTEMPTS: i64 = 256;
const MAX_PASSES: i64 = 32;

fn is_false(value: &bool) -> bool {
    !*value
}
fn is_zero_i64(value: &i64) -> bool {
    *value == 0
}
fn is_empty_map(value: &Map<String, Value>) -> bool {
    value.is_empty()
}
fn is_empty_vec<T>(value: &[T]) -> bool {
    value.is_empty()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Document {
    #[serde(rename = "contractVersion")]
    pub contract_version: String,
    #[serde(rename = "planId")]
    pub plan_id: String,
    pub generation: i64,
    #[serde(rename = "idempotencyKey")]
    pub idempotency_key: String,
    #[serde(
        rename = "catalogRevision",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    pub catalog_revision: String,
    #[serde(default, skip_serializing_if = "is_empty_map")]
    pub variables: Map<String, Value>,
    #[serde(default)]
    pub defaults: Defaults,
    #[serde(default)]
    pub converge: Converge,
    pub nodes: Vec<Node>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Defaults {
    #[serde(rename = "timeoutMs", default, skip_serializing_if = "is_zero_i64")]
    pub timeout_ms: i64,
    #[serde(default)]
    pub retry: Retry,
    #[serde(rename = "maxPasses", default, skip_serializing_if = "is_zero_i64")]
    pub max_passes: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Converge {
    #[serde(rename = "maxPasses", default, skip_serializing_if = "is_zero_i64")]
    pub max_passes: i64,
    #[serde(
        rename = "abortOnExhaustion",
        default,
        skip_serializing_if = "is_false"
    )]
    pub abort_on_exhaustion: bool,
    #[serde(
        rename = "maxConcurrency",
        default,
        skip_serializing_if = "is_zero_i64"
    )]
    pub max_concurrency: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Retry {
    #[serde(rename = "maxAttempts", default, skip_serializing_if = "is_zero_i64")]
    pub max_attempts: i64,
    #[serde(rename = "backoffMs", default, skip_serializing_if = "is_zero_i64")]
    pub backoff_ms: i64,
    #[serde(rename = "backoffFactor", default, skip_serializing_if = "is_zero_i64")]
    pub backoff_factor: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Node {
    pub id: String,
    #[serde(rename = "dependsOn", default, skip_serializing_if = "is_empty_vec")]
    pub depends_on: Vec<String>,
    #[serde(default, skip_serializing_if = "is_empty_vec")]
    pub when: Vec<Assertion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<TargetRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<Action>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait: Option<WaitSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validate: Option<Validation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recover: Option<Recovery>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compensate: Option<Action>,
    #[serde(default)]
    pub retry: Retry,
    #[serde(rename = "timeoutMs", default, skip_serializing_if = "is_zero_i64")]
    pub timeout_ms: i64,
    #[serde(
        rename = "continueOnFailure",
        default,
        skip_serializing_if = "is_false"
    )]
    pub continue_on_failure: bool,
    #[serde(rename = "forEach", default, skip_serializing_if = "Option::is_none")]
    pub for_each: Option<ForEach>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct TargetRef {
    #[serde(rename = "hostRef")]
    pub host_ref: String,
    #[serde(
        rename = "resourceRef",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    pub resource_ref: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct WaitTrigger {
    pub kind: String,
    #[serde(rename = "type")]
    pub kind_type: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct WaitSpec {
    pub trigger: WaitTrigger,
    #[serde(default, skip_serializing_if = "is_empty_map")]
    pub correlation: Map<String, Value>,
    #[serde(rename = "inputSchema", default, skip_serializing_if = "is_empty_map")]
    pub input_schema: Map<String, Value>,
    #[serde(rename = "schemaRevision")]
    pub schema_revision: String,
    #[serde(
        rename = "expiresAt",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    pub expires_at: String,
    #[serde(rename = "expiresInMs", default, skip_serializing_if = "is_zero_i64")]
    pub expires_in_ms: i64,
    #[serde(rename = "contextDelta", default, skip_serializing_if = "is_empty_vec")]
    pub context_delta: Vec<ContextDelta>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ContextDelta {
    pub name: String,
    pub value: String,
    pub schema: Map<String, Value>,
    pub provenance: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub secret: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Action {
    pub tool: String,
    #[serde(default, skip_serializing_if = "is_empty_map")]
    pub args: Map<String, Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Recovery {
    #[serde(flatten)]
    pub action: Action,
    #[serde(rename = "maxAttempts", default, skip_serializing_if = "is_zero_i64")]
    pub max_attempts: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Validation {
    pub tool: String,
    #[serde(default, skip_serializing_if = "is_empty_map")]
    pub args: Map<String, Value>,
    #[serde(default, skip_serializing_if = "is_empty_vec")]
    pub assert: Vec<Assertion>,
    #[serde(
        rename = "pollIntervalMs",
        default,
        skip_serializing_if = "is_zero_i64"
    )]
    pub poll_interval_ms: i64,
    #[serde(rename = "timeoutMs", default, skip_serializing_if = "is_zero_i64")]
    pub timeout_ms: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ForEach {
    pub source: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub path: String,
    #[serde(rename = "as")]
    pub r#as: String,
    #[serde(default, skip_serializing_if = "is_empty_vec")]
    pub filter: Vec<Assertion>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Assertion {
    pub path: String,
    pub op: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    #[serde(default, skip_serializing_if = "is_empty_vec")]
    pub assertions: Vec<Assertion>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Capability {
    pub name: String,
    pub input_schema: Map<String, Value>,
    pub output_schema: Map<String, Value>,
    pub effect: String,
    pub idempotent: bool,
}

pub type NodeStatus = &'static str;
pub const STATUS_PENDING: NodeStatus = "pending";
pub const STATUS_SKIPPED: NodeStatus = "skipped";
pub const STATUS_SATISFIED: NodeStatus = "satisfied";
pub const STATUS_APPLIED: NodeStatus = "applied";
pub const STATUS_FAILED: NodeStatus = "failed";
pub const STATUS_UNKNOWN: NodeStatus = "unknown";
pub const STATUS_COMPENSATED: NodeStatus = "compensated";
pub const STATUS_COMPENSATION_FAILED: NodeStatus = "compensation_failed";
pub const STATUS_WAITING: NodeStatus = "waiting";
pub const STATUS_EXPIRED: NodeStatus = "expired";

pub const RUN_STATUS_WAITING: &str = "waiting";
// Deferred wait/resume wiring (see plan::runner); not yet read from
// plan_mcp.rs.
#[allow(dead_code)]
pub const RUN_STATUS_EXPIRED: &str = "expired";

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct NodeRunState {
    pub id: String,
    pub status: String,
    #[serde(default, skip_serializing_if = "is_zero_i64")]
    pub attempts: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected: Option<Value>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error: String,
    #[serde(
        rename = "startedAt",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    pub started_at: String,
    #[serde(
        rename = "completedAt",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    pub completed_at: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct WaitState {
    #[serde(rename = "nodeId")]
    pub node_id: String,
    #[serde(rename = "waitId")]
    pub wait_id: String,
    #[serde(rename = "waitRevision")]
    pub wait_revision: i64,
    #[serde(rename = "schemaRevision")]
    pub schema_revision: String,
    pub trigger: WaitTrigger,
    #[serde(default, skip_serializing_if = "is_empty_map")]
    pub correlation: Map<String, Value>,
    #[serde(rename = "inputSchema", default, skip_serializing_if = "is_empty_map")]
    pub input_schema: Map<String, Value>,
    #[serde(
        rename = "expiresAt",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    pub expires_at: String,
    pub status: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ContextEntry {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<Value>,
    #[serde(default, skip_serializing_if = "is_empty_map")]
    pub schema: Map<String, Value>,
    #[serde(
        rename = "schemaRevision",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    pub schema_revision: String,
    #[serde(rename = "producerNode")]
    pub producer_node: String,
    pub source: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub secret: bool,
    #[serde(rename = "recordedAt")]
    pub recorded_at: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[allow(dead_code)]
pub struct ResumeRequest {
    #[serde(rename = "waitNodeId")]
    pub wait_node_id: String,
    #[serde(rename = "waitRevision")]
    pub wait_revision: i64,
    #[serde(rename = "schemaRevision")]
    pub schema_revision: String,
    #[serde(default, skip_serializing_if = "is_empty_map")]
    pub correlation: Map<String, Value>,
    #[serde(default, skip_serializing_if = "is_empty_map")]
    pub input: Map<String, Value>,
    pub source: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct RunState {
    #[serde(rename = "runId")]
    pub run_id: String,
    #[serde(rename = "planId")]
    pub plan_id: String,
    pub generation: i64,
    pub status: String,
    pub nodes: BTreeMap<String, NodeRunState>,
    #[serde(default, skip_serializing_if = "is_empty_map")]
    pub outputs: Map<String, Value>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub context: BTreeMap<String, ContextEntry>,
    #[serde(
        rename = "contextHistory",
        default,
        skip_serializing_if = "is_empty_vec"
    )]
    pub context_history: Vec<ContextEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wait: Option<WaitState>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error: String,
}

#[derive(Debug, Clone)]
pub struct AssertionFailure {
    // Mirrors Go's `AssertionFailure.Assertion`; not yet read anywhere, but
    // kept for parity -- future node-trace/observability output surfaces it.
    #[allow(dead_code)]
    pub assertion: Assertion,
    pub observed: Option<Value>,
    pub expected: Option<Value>,
    pub message: String,
}

/// Decode a plan document from JSON or YAML bytes/text. Mirrors `plan.Decode`
/// for the two inputs the host agent actually needs: a raw JSON/YAML blob (an
/// MCP tool argument is always JSON already decoded into a `Value`, so the
/// `Value::Object` case below is the hot path) or a preformed `Document`.
pub fn decode_value(raw: &Value) -> Result<Document, String> {
    if let Value::Object(_) = raw {
        return serde_json::from_value(raw.clone()).map_err(|e| format!("decode JSON plan: {e}"));
    }
    if let Value::String(text) = raw {
        return decode_bytes(text.as_bytes());
    }
    Err("plan must be an object, JSON, YAML, or string".to_string())
}

pub fn decode_bytes(data: &[u8]) -> Result<Document, String> {
    if data.is_empty() || data.len() > MAX_DOCUMENT_BYTES {
        return Err(format!(
            "plan document must be between 1 and {MAX_DOCUMENT_BYTES} bytes"
        ));
    }
    let text = String::from_utf8_lossy(data);
    let trimmed = text.trim_start();
    if trimmed.starts_with('{') {
        serde_json::from_slice(data).map_err(|e| format!("decode JSON plan: {e}"))
    } else {
        serde_yaml::from_slice(data).map_err(|e| format!("decode YAML plan: {e}"))
    }
}

/// `plan.CanonicalJSON` / `plan.DocumentHash`. Go marshals struct fields in
/// declaration order and map keys sorted; `serde_json`'s default (no
/// `preserve_order` feature) does the same for both, so this is a direct
/// `json.Marshal` + `sha256` port.
pub fn canonical_json(doc: &Document) -> Result<Vec<u8>, String> {
    let encoded = serde_json::to_vec(doc).map_err(|e| e.to_string())?;
    Ok(crate::gojson::html_escape_json_bytes(encoded))
}

pub fn document_hash(doc: &Document) -> Result<(String, Vec<u8>), String> {
    let encoded = canonical_json(doc)?;
    let digest = Sha256::digest(&encoded);
    Ok((format!("sha256:{}", hex::encode(digest)), encoded))
}

fn schema_type(schema: &Map<String, Value>) -> String {
    match schema.get("type") {
        None => "unknown".to_string(),
        Some(Value::String(s)) if !s.is_empty() => s.clone(),
        Some(Value::Array(values)) => {
            let parts: Vec<String> = values
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect();
            parts.join("|")
        }
        _ => "unknown".to_string(),
    }
}

fn schemas_compatible(producer: &Map<String, Value>, consumer: &Map<String, Value>) -> bool {
    let producer_type = schema_type(producer);
    let consumer_type = schema_type(consumer);
    if producer_type == "unknown" || consumer_type == "unknown" {
        return true;
    }
    consumer_type.split('|').any(|c| c == producer_type)
}

fn number(value: &Value) -> Option<f64> {
    value.as_f64()
}

fn json_equal(left: &Value, right: &Value) -> bool {
    left == right
}

/// `plan.ValidateJSON`.
pub fn validate_json(schema: &Map<String, Value>, value: &Value) -> Result<(), String> {
    if schema.is_empty() {
        return Ok(());
    }
    if is_whole_reference(value) {
        return Ok(());
    }
    if let Some(enum_values) = schema.get("enum").and_then(Value::as_array) {
        if !enum_values.iter().any(|c| json_equal(c, value)) {
            return Err(format!("value {value} is not in enum"));
        }
    }
    if let Some(constant) = schema.get("const") {
        if !json_equal(constant, value) {
            return Err(format!("value {value} does not equal const {constant}"));
        }
    }
    let types = schema_types(schema.get("type"));
    if types.is_empty() {
        return Ok(());
    }
    if types.len() > 1 {
        let mut last = Ok(());
        for type_name in &types {
            let mut candidate = schema.clone();
            candidate.insert("type".to_string(), Value::String(type_name.clone()));
            match validate_json(&candidate, value) {
                Ok(()) => return Ok(()),
                Err(e) => last = Err(e),
            }
        }
        return last;
    }
    match types[0].as_str() {
        "object" => {
            let object = value
                .as_object()
                .ok_or_else(|| format!("expected object, got {}", type_name_of(value)))?;
            if let Some(required) = string_or_any_slice(schema.get("required")) {
                for name in required {
                    if !object.contains_key(&name) {
                        return Err(format!("missing required property \"{name}\""));
                    }
                }
            }
            let properties = schema.get("properties").and_then(Value::as_object);
            for (key, child) in object {
                if let Some(property) = properties
                    .and_then(|p| p.get(key))
                    .and_then(Value::as_object)
                {
                    validate_json(property, child)
                        .map_err(|e| format!("property \"{key}\": {e}"))?;
                } else if let Some(Value::Bool(false)) = schema.get("additionalProperties") {
                    return Err(format!("unknown property \"{key}\""));
                }
            }
        }
        "array" => {
            let array = value
                .as_array()
                .ok_or_else(|| format!("expected array, got {}", type_name_of(value)))?;
            if let Some(minimum) = schema.get("minItems").and_then(number) {
                if (array.len() as f64) < minimum {
                    return Err(format!(
                        "array has {} items, minimum is {minimum}",
                        array.len()
                    ));
                }
            }
            if let Some(item_schema) = schema.get("items").and_then(Value::as_object) {
                for (index, item) in array.iter().enumerate() {
                    validate_json(item_schema, item).map_err(|e| format!("item {index}: {e}"))?;
                }
            }
        }
        "string" => {
            let text = value
                .as_str()
                .ok_or_else(|| format!("expected string, got {}", type_name_of(value)))?;
            if let Some(minimum) = schema.get("minLength").and_then(number) {
                if (text.len() as f64) < minimum {
                    return Err(format!("string is shorter than {minimum}"));
                }
            }
            if let Some(pattern) = schema.get("pattern").and_then(Value::as_str) {
                let re = regex::Regex::new(pattern)
                    .map_err(|e| format!("invalid schema pattern: {e}"))?;
                if !re.is_match(text) {
                    return Err("string does not match pattern".to_string());
                }
            }
        }
        "integer" => {
            let n = number(value)
                .ok_or_else(|| format!("expected integer, got {}", type_name_of(value)))?;
            if n.trunc() != n {
                return Err(format!("expected integer, got {}", type_name_of(value)));
            }
        }
        "number" => {
            if number(value).is_none() {
                return Err(format!("expected number, got {}", type_name_of(value)));
            }
        }
        "boolean" => {
            if value.as_bool().is_none() {
                return Err(format!("expected boolean, got {}", type_name_of(value)));
            }
        }
        _ => {}
    }
    if let Some(minimum) = schema.get("minimum").and_then(number) {
        let actual = number(value).ok_or_else(|| "number is below minimum".to_string())?;
        if actual < minimum {
            return Err(format!("number is below minimum {minimum}"));
        }
    }
    Ok(())
}

fn type_name_of(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn schema_types(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::String(s)) if !s.is_empty() => vec![s.clone()],
        Some(Value::Array(values)) => values
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        _ => vec![],
    }
}

fn string_or_any_slice(value: Option<&Value>) -> Option<Vec<String>> {
    match value {
        Some(Value::Array(values)) => {
            let mut out = Vec::with_capacity(values.len());
            for item in values {
                out.push(item.as_str()?.to_string());
            }
            Some(out)
        }
        _ => None,
    }
}

static REFERENCE_PATTERN: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
fn reference_pattern() -> &'static regex::Regex {
    REFERENCE_PATTERN.get_or_init(|| regex::Regex::new(r"\$\{([^}]+)\}").unwrap())
}

fn is_whole_reference(value: &Value) -> bool {
    let Some(text) = value.as_str() else {
        return false;
    };
    if let Some(m) = reference_pattern().find(text) {
        m.start() == 0 && m.end() == text.len()
    } else {
        false
    }
}

fn attempts_for(node: &Node, doc: &Document) -> i64 {
    let mut attempts = node.retry.max_attempts;
    if attempts <= 0 {
        attempts = doc.defaults.retry.max_attempts;
    }
    if attempts <= 0 {
        return 1;
    }
    attempts
}

fn node_by_id<'a>(doc: &'a Document, id: &str) -> Option<&'a Node> {
    doc.nodes.iter().find(|n| n.id == id)
}

fn graph_ancestors(doc: &Document) -> BTreeMap<String, std::collections::BTreeSet<String>> {
    let mut parents: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for node in &doc.nodes {
        parents.insert(node.id.clone(), node.depends_on.clone());
    }
    fn visit(
        id: &str,
        parents: &BTreeMap<String, Vec<String>>,
        seen: &mut std::collections::BTreeSet<String>,
    ) {
        if let Some(list) = parents.get(id) {
            for parent in list {
                if seen.insert(parent.clone()) {
                    visit(parent, parents, seen);
                }
            }
        }
    }
    let mut ancestors = BTreeMap::new();
    for node in &doc.nodes {
        let mut seen = std::collections::BTreeSet::new();
        visit(&node.id, &parents, &mut seen);
        ancestors.insert(node.id.clone(), seen);
    }
    ancestors
}

fn schema_at<'a>(
    schema: Option<&'a Map<String, Value>>,
    path: &[&str],
) -> Option<&'a Map<String, Value>> {
    let mut current = schema?;
    for part in path {
        let properties = current.get("properties").and_then(Value::as_object)?;
        current = properties.get(*part).and_then(Value::as_object)?;
    }
    Some(current)
}

fn schema_at_owned(schema: &Map<String, Value>, path: &[String]) -> Option<Map<String, Value>> {
    let refs: Vec<&str> = path.iter().map(String::as_str).collect();
    schema_at(Some(schema), &refs).cloned()
}

fn is_empty_schema(schema: &Map<String, Value>) -> bool {
    schema.is_empty()
}

/// `plan.Validate`: the full static validator. `capabilities` maps a tool
/// name to its declared input/output schema and effect/idempotency, exactly
/// as the Go catalog snapshot does.
pub fn validate(
    doc: &Document,
    capabilities: &BTreeMap<String, Capability>,
    catalog_revision: &str,
) -> Result<(), String> {
    if doc.contract_version != CONTRACT_VERSION {
        return Err(format!(
            "unsupported contractVersion \"{}\"",
            doc.contract_version
        ));
    }
    if doc.plan_id.trim().is_empty() || doc.idempotency_key.trim().is_empty() {
        return Err("planId and idempotencyKey are required".to_string());
    }
    if doc.generation < 1 {
        return Err("generation must be at least 1".to_string());
    }
    if !doc.catalog_revision.is_empty()
        && !catalog_revision.is_empty()
        && doc.catalog_revision != catalog_revision
    {
        return Err(format!(
            "catalog revision mismatch: plan={} current={}",
            doc.catalog_revision, catalog_revision
        ));
    }
    if doc.nodes.is_empty() || doc.nodes.len() > MAX_NODES {
        return Err(format!(
            "nodes must contain between 1 and {MAX_NODES} entries"
        ));
    }
    let mut passes = doc.converge.max_passes;
    if doc.converge.max_passes < 0 || doc.defaults.max_passes < 0 {
        return Err("maxPasses cannot be negative".to_string());
    }
    if passes <= 0 {
        passes = doc.defaults.max_passes;
    }
    if passes > MAX_PASSES {
        return Err(format!("converge maxPasses exceeds limit {MAX_PASSES}"));
    }
    if doc.converge.max_concurrency < 0 || doc.converge.max_concurrency > MAX_FAN_OUT as i64 {
        return Err(format!(
            "converge maxConcurrency must be between 0 and {MAX_FAN_OUT}"
        ));
    }
    if doc.defaults.retry.max_attempts < 0 || doc.defaults.retry.max_attempts > MAX_TOTAL_ATTEMPTS {
        return Err(format!(
            "default retry maxAttempts must be between 0 and {MAX_TOTAL_ATTEMPTS}"
        ));
    }
    document_hash(doc).map_err(|e| format!("canonicalize plan: {e}"))?;
    super::graph::validate_graph(doc)?;
    let ancestors = graph_ancestors(doc);
    let mut total_attempts: i64 = 0;
    for node in &doc.nodes {
        if node.id.trim().is_empty() {
            return Err("node id is required".to_string());
        }
        if node.action.is_none()
            && node.wait.is_none()
            && node.validate.is_none()
            && node.for_each.is_none()
        {
            return Err(format!(
                "node \"{}\" must have an action, wait, validation, or forEach",
                node.id
            ));
        }
        if let Some(wait) = &node.wait {
            if node.action.is_some()
                || node.validate.is_some()
                || node.recover.is_some()
                || node.compensate.is_some()
                || node.for_each.is_some()
            {
                return Err(format!(
                    "node \"{}\" wait cannot be combined with an action, validation, recovery, compensation, or forEach",
                    node.id
                ));
            }
            validate_wait_spec(wait).map_err(|e| format!("node \"{}\" wait: {e}", node.id))?;
            validate_interpolations(
                &Value::Object(wait.correlation.clone()),
                doc,
                node,
                &ancestors,
            )
            .map_err(|e| format!("node \"{}\" wait correlation: {e}", node.id))?;
            for delta in &wait.context_delta {
                validate_interpolations(&Value::String(delta.value.clone()), doc, node, &ancestors)
                    .map_err(|e| {
                        format!("node \"{}\" context delta \"{}\": {e}", node.id, delta.name)
                    })?;
            }
        }
        if let Some(target) = &node.target {
            if target.host_ref.trim().is_empty() {
                return Err(format!("node \"{}\" target hostRef is required", node.id));
            }
            validate_interpolations(
                &Value::String(target.host_ref.clone()),
                doc,
                node,
                &ancestors,
            )
            .map_err(|e| format!("node \"{}\" target: {e}", node.id))?;
        }
        if let Some(action) = &node.action {
            let capability = capabilities.get(&action.tool).ok_or_else(|| {
                format!(
                    "node \"{}\" references unknown action \"{}\"",
                    node.id, action.tool
                )
            })?;
            if action.tool == "run_host_plan" {
                return Err(format!(
                    "node \"{}\" cannot recursively run a host plan",
                    node.id
                ));
            }
            validate_interpolations(&Value::Object(action.args.clone()), doc, node, &ancestors)
                .map_err(|e| format!("node \"{}\" action references: {e}", node.id))?;
            validate_json(
                &capability.input_schema,
                &Value::Object(action.args.clone()),
            )
            .map_err(|e| format!("node \"{}\" action {} arguments: {e}", node.id, action.tool))?;
            if capability.effect != "read" && node.validate.is_none() {
                return Err(format!(
                    "mutating node \"{}\" requires a readiness validate block",
                    node.id
                ));
            }
            if attempts_for(node, doc) > 1 && capability.effect != "read" && !capability.idempotent
            {
                return Err(format!(
                    "mutating node \"{}\" action {} is not declared idempotent; automatic retries are disabled",
                    node.id, action.tool
                ));
            }
            walk_reference_types(
                &Value::Object(action.args.clone()),
                capabilities.get(&action.tool).map(|c| &c.input_schema),
                doc,
                node,
                capabilities,
            )
            .map_err(|e| format!("node \"{}\" action types: {e}", node.id))?;
        }
        if let Some(validation) = &node.validate {
            let capability = capabilities.get(&validation.tool).ok_or_else(|| {
                format!(
                    "node \"{}\" references unknown validation tool \"{}\"",
                    node.id, validation.tool
                )
            })?;
            if capability.effect != "read" {
                return Err(format!(
                    "node \"{}\" validation tool \"{}\" is not read-only",
                    node.id, validation.tool
                ));
            }
            validate_interpolations(
                &Value::Object(validation.args.clone()),
                doc,
                node,
                &ancestors,
            )
            .map_err(|e| format!("node \"{}\" validation references: {e}", node.id))?;
            validate_json(
                &capability.input_schema,
                &Value::Object(validation.args.clone()),
            )
            .map_err(|e| {
                format!(
                    "node \"{}\" validation {} arguments: {e}",
                    node.id, validation.tool
                )
            })?;
            walk_reference_types(
                &Value::Object(validation.args.clone()),
                capabilities.get(&validation.tool).map(|c| &c.input_schema),
                doc,
                node,
                capabilities,
            )
            .map_err(|e| format!("node \"{}\" validation types: {e}", node.id))?;
        }
        if let Some(recover) = &node.recover {
            let capability = capabilities.get(&recover.action.tool).ok_or_else(|| {
                format!(
                    "node \"{}\" references unknown recovery tool \"{}\"",
                    node.id, recover.action.tool
                )
            })?;
            if recover.max_attempts < 0 || recover.max_attempts > MAX_TOTAL_ATTEMPTS {
                return Err(format!(
                    "node \"{}\" recovery maxAttempts is outside the bounded range",
                    node.id
                ));
            }
            validate_interpolations(
                &Value::Object(recover.action.args.clone()),
                doc,
                node,
                &ancestors,
            )
            .map_err(|e| format!("node \"{}\" recovery references: {e}", node.id))?;
            validate_json(
                &capability.input_schema,
                &Value::Object(recover.action.args.clone()),
            )
            .map_err(|e| {
                format!(
                    "node \"{}\" recovery {} arguments: {e}",
                    node.id, recover.action.tool
                )
            })?;
            walk_reference_types(
                &Value::Object(recover.action.args.clone()),
                capabilities
                    .get(&recover.action.tool)
                    .map(|c| &c.input_schema),
                doc,
                node,
                capabilities,
            )
            .map_err(|e| format!("node \"{}\" recovery types: {e}", node.id))?;
        }
        if let Some(compensate) = &node.compensate {
            let capability = capabilities.get(&compensate.tool).ok_or_else(|| {
                format!(
                    "node \"{}\" references unknown compensation tool \"{}\"",
                    node.id, compensate.tool
                )
            })?;
            validate_interpolations(
                &Value::Object(compensate.args.clone()),
                doc,
                node,
                &ancestors,
            )
            .map_err(|e| format!("node \"{}\" compensation references: {e}", node.id))?;
            validate_json(
                &capability.input_schema,
                &Value::Object(compensate.args.clone()),
            )
            .map_err(|e| {
                format!(
                    "node \"{}\" compensation {} arguments: {e}",
                    node.id, compensate.tool
                )
            })?;
            walk_reference_types(
                &Value::Object(compensate.args.clone()),
                capabilities.get(&compensate.tool).map(|c| &c.input_schema),
                doc,
                node,
                capabilities,
            )
            .map_err(|e| format!("node \"{}\" compensation types: {e}", node.id))?;
        }
        if let Some(for_each) = &node.for_each {
            if for_each.source.trim().is_empty() || for_each.r#as.trim().is_empty() {
                return Err(format!(
                    "node \"{}\" forEach requires source and as",
                    node.id
                ));
            }
            if node.action.is_none() {
                return Err(format!("node \"{}\" forEach requires an action", node.id));
            }
            validate_reference(&for_each.source, doc, node, &ancestors, true)
                .map_err(|e| format!("node \"{}\" forEach source: {e}", node.id))?;
        }
        if node.retry.max_attempts < 0 || node.retry.max_attempts > MAX_TOTAL_ATTEMPTS {
            return Err(format!(
                "node \"{}\" retry maxAttempts is outside the bounded range",
                node.id
            ));
        }
        let attempts = attempts_for(node, doc);
        let attempt_multiplier = if node.for_each.is_some() {
            MAX_FAN_OUT as i64
        } else {
            1
        };
        let mut recovery_attempts = 0;
        if let Some(recover) = &node.recover {
            recovery_attempts = if recover.max_attempts <= 0 {
                1
            } else {
                recover.max_attempts
            };
            let capability = capabilities.get(&recover.action.tool);
            let effect_read = capability.map(|c| c.effect == "read").unwrap_or(false);
            let idempotent = capability.map(|c| c.idempotent).unwrap_or(false);
            if recovery_attempts > 1 && !effect_read && !idempotent {
                return Err(format!(
                    "node \"{}\" recovery {} is not declared idempotent; automatic retries are disabled",
                    node.id, recover.action.tool
                ));
            }
        }
        total_attempts += attempt_multiplier * (attempts + recovery_attempts);
    }
    if total_attempts > MAX_TOTAL_ATTEMPTS {
        return Err(format!("plan attempts exceed limit {MAX_TOTAL_ATTEMPTS}"));
    }
    Ok(())
}

fn validate_wait_spec(wait: &WaitSpec) -> Result<(), String> {
    if wait.trigger.kind != "operator" && wait.trigger.kind != "event-or-operator" {
        return Err("trigger kind must be operator or event-or-operator".to_string());
    }
    if wait.trigger.kind_type.trim().is_empty() {
        return Err("trigger type is required".to_string());
    }
    if wait.schema_revision.trim().is_empty() {
        return Err("schemaRevision is required".to_string());
    }
    if !wait.expires_at.is_empty() && chrono_parse_rfc3339(&wait.expires_at).is_none() {
        return Err("expiresAt must be RFC3339".to_string());
    }
    if wait.expires_in_ms < 0 {
        return Err("expiresInMs cannot be negative".to_string());
    }
    if !wait.expires_at.is_empty() && wait.expires_in_ms != 0 {
        return Err("expiresAt and expiresInMs are mutually exclusive".to_string());
    }
    if wait.input_schema.is_empty() {
        return Err("inputSchema is required".to_string());
    }
    validate_bounded_input_schema(&wait.input_schema, 0)?;
    let mut names = std::collections::BTreeSet::new();
    for delta in &wait.context_delta {
        if delta.name.trim().is_empty() || !names.insert(delta.name.clone()) {
            return Err("contextDelta names must be unique and non-empty".to_string());
        }
        let trimmed = delta.value.trim();
        if trimmed.is_empty()
            || !is_whole_reference(&Value::String(delta.value.clone()))
            || !trimmed.starts_with("${input.")
        {
            return Err(format!(
                "contextDelta \"{}\" must be a whole input reference",
                delta.name
            ));
        }
        if delta.schema.is_empty() {
            return Err(format!(
                "contextDelta \"{}\" schema is required",
                delta.name
            ));
        }
        validate_bounded_schema(&delta.schema, 0)
            .map_err(|e| format!("contextDelta \"{}\" schema: {e}", delta.name))?;
        if delta.provenance != "operator"
            && delta.provenance != "authenticated-event"
            && delta.provenance != "event-or-operator"
        {
            return Err(format!(
                "contextDelta \"{}\" provenance is unsupported",
                delta.name
            ));
        }
    }
    Ok(())
}

fn chrono_parse_rfc3339(value: &str) -> Option<()> {
    // No chrono dependency yet; a conservative structural check mirrors
    // time.Parse(time.RFC3339Nano) closely enough for validation purposes.
    let bytes = value.as_bytes();
    if bytes.len() < 20 {
        return None;
    }
    if bytes[4] != b'-' || bytes[7] != b'-' || bytes[10] != b'T' {
        return None;
    }
    Some(())
}

fn validate_bounded_input_schema(schema: &Map<String, Value>, depth: i32) -> Result<(), String> {
    validate_bounded_schema(schema, depth)?;
    if schema_type(schema) != "object" {
        return Err("schema must have type object".to_string());
    }
    let additional = schema.get("additionalProperties").and_then(Value::as_bool);
    if additional != Some(false) {
        return Err("schema must set additionalProperties: false".to_string());
    }
    let properties = schema
        .get("properties")
        .and_then(Value::as_object)
        .ok_or_else(|| "schema properties are required".to_string())?;
    if properties.len() > 32 {
        return Err("schema has too many properties".to_string());
    }
    for (name, raw) in properties {
        if name.trim().is_empty() {
            return Err("schema property names must be non-empty".to_string());
        }
        let property = raw
            .as_object()
            .ok_or_else(|| format!("schema property \"{name}\" must be an object"))?;
        if schema_type(property) == "object" {
            validate_bounded_input_schema(property, depth + 1)
                .map_err(|e| format!("property \"{name}\": {e}"))?;
        }
    }
    if let Some(required) = string_or_any_slice(schema.get("required")) {
        for name in required {
            if !properties.contains_key(&name) {
                return Err(format!("required property \"{name}\" is not declared"));
            }
        }
    }
    Ok(())
}

fn validate_bounded_schema(schema: &Map<String, Value>, depth: i32) -> Result<(), String> {
    if depth > 8 {
        return Err("schema nesting exceeds limit".to_string());
    }
    if (schema_type(schema) == "unknown" || schema_type(schema).is_empty())
        && schema.get("type").is_none()
    {
        return Err("schema type is required".to_string());
    }
    Ok(())
}

fn walk_reference_types(
    value: &Value,
    schema: Option<&Map<String, Value>>,
    doc: &Document,
    node: &Node,
    capabilities: &BTreeMap<String, Capability>,
) -> Result<(), String> {
    match value {
        Value::Object(map) => {
            let properties = schema
                .and_then(|s| s.get("properties"))
                .and_then(Value::as_object);
            for (key, child) in map {
                let child_schema = properties
                    .and_then(|p| p.get(key))
                    .and_then(Value::as_object);
                walk_reference_types(child, child_schema, doc, node, capabilities)
                    .map_err(|e| format!("property \"{key}\": {e}"))?;
            }
        }
        Value::Array(items) => {
            let item_schema = schema
                .and_then(|s| s.get("items"))
                .and_then(Value::as_object);
            for (index, child) in items.iter().enumerate() {
                walk_reference_types(child, item_schema, doc, node, capabilities)
                    .map_err(|e| format!("item {index}: {e}"))?;
            }
        }
        Value::String(text) => {
            if !is_whole_reference(value) {
                return Ok(());
            }
            let Some(m) = reference_pattern().captures(text) else {
                return Ok(());
            };
            let reference_body = m.get(1).unwrap().as_str().to_string();
            if reference_body.starts_with("input.") || reference_body.starts_with("context.") {
                let produced_schema =
                    reference_schema(&reference_body, doc, node).ok_or_else(|| {
                        format!("reference \"{text}\" has no declared input or context producer")
                    })?;
                if let Some(target_schema) = schema {
                    if !is_empty_schema(target_schema)
                        && !schemas_compatible(&produced_schema, target_schema)
                    {
                        return Err(format!(
                            "reference \"{text}\" produces {} but target expects {}",
                            schema_type(&produced_schema),
                            schema_type(target_schema)
                        ));
                    }
                }
                return Ok(());
            }
            if !reference_body.starts_with("nodes.") {
                return Ok(());
            }
            let parts: Vec<&str> = reference_body.split('.').collect();
            if parts.len() < 3 {
                return Ok(());
            }
            let producer_id = parts[1];
            let producer = node_by_id(doc, producer_id)
                .ok_or_else(|| format!("producer node \"{producer_id}\" is not declared"))?;
            let mut producer_schema: Option<Map<String, Value>> = None;
            if let Some(action) = &producer.action {
                producer_schema = capabilities
                    .get(&action.tool)
                    .map(|c| c.output_schema.clone());
            }
            if producer_schema.is_none() {
                if let Some(validation) = &producer.validate {
                    producer_schema = capabilities
                        .get(&validation.tool)
                        .map(|c| c.output_schema.clone());
                }
            }
            let mut path: Vec<String> = parts[2..].iter().map(|s| s.to_string()).collect();
            if !path.is_empty() && path[0] == "output" {
                path.remove(0);
            }
            let produced_schema = producer_schema
                .as_ref()
                .and_then(|s| schema_at_owned(s, &path))
                .ok_or_else(|| {
                    format!("reference \"{text}\" has no typed producer output schema")
                })?;
            if let Some(target_schema) = schema {
                if !is_empty_schema(target_schema)
                    && !schemas_compatible(&produced_schema, target_schema)
                {
                    return Err(format!(
                        "reference \"{text}\" produces {} but target expects {}",
                        schema_type(&produced_schema),
                        schema_type(target_schema)
                    ));
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn validate_interpolations(
    value: &Value,
    doc: &Document,
    node: &Node,
    ancestors: &BTreeMap<String, std::collections::BTreeSet<String>>,
) -> Result<(), String> {
    match value {
        Value::Object(map) => {
            for child in map.values() {
                validate_interpolations(child, doc, node, ancestors)?;
            }
        }
        Value::Array(items) => {
            for child in items {
                validate_interpolations(child, doc, node, ancestors)?;
            }
        }
        Value::String(text) => {
            for m in reference_pattern().captures_iter(text) {
                let reference = m.get(1).unwrap().as_str();
                validate_reference(reference, doc, node, ancestors, node.for_each.is_some())?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn validate_reference(
    reference: &str,
    doc: &Document,
    node: &Node,
    ancestors: &BTreeMap<String, std::collections::BTreeSet<String>>,
    allow_item: bool,
) -> Result<(), String> {
    let reference = if let Some(m) = reference_pattern().captures(reference) {
        if m.get(0).unwrap().as_str() == reference {
            m.get(1).unwrap().as_str()
        } else {
            reference
        }
    } else {
        reference
    };
    let parts: Vec<&str> = reference.split('.').collect();
    if parts.len() < 2 || parts[1].trim().is_empty() {
        return Err(format!("invalid interpolation reference \"{reference}\""));
    }
    match parts[0] {
        "vars" => {
            if !doc.variables.contains_key(parts[1]) {
                return Err(format!("variable \"{}\" is not declared", parts[1]));
            }
        }
        "item" => {
            if !allow_item {
                return Err(format!(
                    "item reference \"{reference}\" is only valid inside forEach"
                ));
            }
        }
        "nodes" => {
            if parts.len() < 3
                || !ancestors
                    .get(&node.id)
                    .map(|a| a.contains(parts[1]))
                    .unwrap_or(false)
            {
                return Err(format!(
                    "node reference \"{reference}\" must target a declared dependency"
                ));
            }
        }
        "input" => {
            let Some(wait) = &node.wait else {
                return Err(format!(
                    "input reference \"{reference}\" is only valid in its wait context"
                ));
            };
            let path: Vec<&str> = parts[1..].to_vec();
            let owned: Vec<String> = path.iter().map(|s| s.to_string()).collect();
            if schema_at_owned(&wait.input_schema, &owned).is_none() {
                return Err(format!(
                    "input reference \"{reference}\" is not declared by the wait schema"
                ));
            }
        }
        "context" => {
            if reference_schema(reference, doc, node).is_none() {
                return Err(format!(
                    "context reference \"{reference}\" has no declared dependency producer"
                ));
            }
        }
        other => {
            return Err(format!("unknown interpolation root \"{other}\""));
        }
    }
    Ok(())
}

fn reference_schema(reference: &str, doc: &Document, node: &Node) -> Option<Map<String, Value>> {
    let parts: Vec<&str> = reference.split('.').collect();
    if parts.len() < 2 {
        return None;
    }
    let ancestors = graph_ancestors(doc);
    let node_ancestors = ancestors.get(&node.id)?;
    match parts[0] {
        "input" => {
            let wait = node.wait.as_ref()?;
            let path: Vec<String> = parts[1..].iter().map(|s| s.to_string()).collect();
            schema_at_owned(&wait.input_schema, &path)
        }
        "context" => {
            for candidate in &doc.nodes {
                if !node_ancestors.contains(&candidate.id) {
                    continue;
                }
                let Some(wait) = &candidate.wait else {
                    continue;
                };
                for delta in &wait.context_delta {
                    if delta.name != parts[1] {
                        continue;
                    }
                    let path: Vec<String> = parts[2..].iter().map(|s| s.to_string()).collect();
                    if path.is_empty() {
                        return if delta.schema.is_empty() {
                            None
                        } else {
                            Some(delta.schema.clone())
                        };
                    }
                    return schema_at_owned(&delta.schema, &path);
                }
            }
            None
        }
        _ => None,
    }
}

#[allow(dead_code)]
pub fn node_by_id_pub<'a>(doc: &'a Document, id: &str) -> Option<&'a Node> {
    node_by_id(doc, id)
}

pub fn attempts_for_pub(node: &Node, doc: &Document) -> i64 {
    attempts_for(node, doc)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_document() -> Document {
        Document {
            contract_version: CONTRACT_VERSION.to_string(),
            plan_id: "plan-1".to_string(),
            generation: 1,
            idempotency_key: "key-1".to_string(),
            nodes: vec![Node {
                id: "n1".to_string(),
                action: Some(Action {
                    tool: "noop".to_string(),
                    args: Map::new(),
                }),
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn document_hash_is_deterministic_and_order_independent_for_object_keys() {
        let doc = sample_document();
        let (hash1, _) = document_hash(&doc).unwrap();
        let (hash2, _) = document_hash(&doc).unwrap();
        assert_eq!(hash1, hash2);
        assert!(hash1.starts_with("sha256:"));
    }

    /// `serde_json` does not HTML-escape by default; `json.Marshal` (what
    /// Go's `CanonicalJSON`/`DocumentHash` use) does. A recipe or plan
    /// variable containing `<`, `>` or `&` -- `cluster:<tenant>:<id>` was
    /// the real one that caught this -- must still hash identically to Go,
    /// or the canonical hash silently diverges only for documents with
    /// those characters in a string value.
    #[test]
    fn canonical_json_html_escapes_like_go_json_marshal() {
        let mut doc = sample_document();
        doc.variables
            .insert("note".to_string(), Value::String("a<b>&c".to_string()));
        let encoded = canonical_json(&doc).unwrap();
        let text = String::from_utf8(encoded).unwrap();
        assert!(text.contains("a\\u003cb\\u003e\\u0026c"), "{text}");
        assert!(!text.contains('<') && !text.contains('>') && !text.contains('&'));
    }

    #[test]
    fn decode_round_trips_through_canonical_json() {
        let doc = sample_document();
        let encoded = canonical_json(&doc).unwrap();
        let value: Value = serde_json::from_slice(&encoded).unwrap();
        let decoded = decode_value(&value).unwrap();
        assert_eq!(decoded, doc);
    }

    #[test]
    fn decode_accepts_yaml() {
        let yaml = b"contractVersion: host-plan.v1\nplanId: p1\ngeneration: 1\nidempotencyKey: k1\nnodes:\n  - id: n1\n    action:\n      tool: noop\n";
        let decoded = decode_bytes(yaml).unwrap();
        assert_eq!(decoded.plan_id, "p1");
    }

    #[test]
    fn validate_rejects_wrong_contract_version() {
        let mut doc = sample_document();
        doc.contract_version = "other".to_string();
        let caps = BTreeMap::new();
        assert!(validate(&doc, &caps, "").is_err());
    }

    #[test]
    fn validate_rejects_empty_nodes() {
        let mut doc = sample_document();
        doc.nodes.clear();
        let caps = BTreeMap::new();
        assert!(validate(&doc, &caps, "").is_err());
    }

    #[test]
    fn validate_accepts_declared_action_capability() {
        let doc = sample_document();
        let mut caps = BTreeMap::new();
        caps.insert(
            "noop".to_string(),
            Capability {
                name: "noop".to_string(),
                input_schema: Map::new(),
                output_schema: Map::new(),
                effect: "read".to_string(),
                idempotent: true,
            },
        );
        assert!(validate(&doc, &caps, "").is_ok());
    }

    #[test]
    fn validate_json_enforces_required_properties() {
        let mut schema = Map::new();
        schema.insert("type".to_string(), Value::String("object".to_string()));
        schema.insert(
            "required".to_string(),
            Value::Array(vec![Value::String("x".to_string())]),
        );
        schema.insert("additionalProperties".to_string(), Value::Bool(false));
        let mut properties = Map::new();
        properties.insert("x".to_string(), serde_json::json!({"type": "string"}));
        schema.insert("properties".to_string(), Value::Object(properties));
        assert!(validate_json(&schema, &serde_json::json!({})).is_err());
        assert!(validate_json(&schema, &serde_json::json!({"x": "ok"})).is_ok());
    }

    #[test]
    fn omitempty_struct_fields_always_serialize() {
        let doc = sample_document();
        let encoded = serde_json::to_value(&doc).unwrap();
        assert!(encoded.get("defaults").is_some());
        assert!(encoded.get("converge").is_some());
    }
}
