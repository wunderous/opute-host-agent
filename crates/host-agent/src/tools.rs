//! `tools/call`: the Go server's `handleToolCall` pipeline.
//!
//! The SDK layer has already decoded and validated the request envelope. This
//! module resolves the wire name to a registered capability, decodes the
//! arguments the way `json.Unmarshal` into `map[string]any` does, checks the
//! requested catalog revision, applies the standalone mutation gate, and
//! dispatches. Capabilities whose handlers have not been ported yet fail
//! closed with a typed `not_implemented` capability error; the catalog still
//! publishes them, so `tools/list` stays identical to Go.

use crate::catalog;
use crate::gojson::{Node, Value};
use crate::mcpsdk::ToolCallOutcome;
use crate::transport::Server;
use serde_json::{json, Map, Value as J};

/// Resolves a wire name to the catalog name it was registered under, if any.
fn registered(server: &Server, wire: &str) -> Option<String> {
    let name = if server.tool_prefix.trim().is_empty() {
        wire.to_string()
    } else {
        wire.strip_prefix(&format!("{}_", server.tool_prefix.trim()))?
            .to_string()
    };
    let known = server.catalog.tools.iter().any(|d| d.name == name)
        || catalog::internal().tools.iter().any(|d| d.name == name);
    (known && catalog::wire_name(&server.tool_prefix, &name) == wire).then_some(name)
}

/// `tools.ErrorResult` for a plain error.
pub fn error_result(message: &str) -> J {
    json!({"content": [{"type": "text", "text": format!("Error: {message}")}], "isError": true})
}

/// `tools.ErrorResult` for a typed `CapabilityError`.
pub fn capability_error(owner: &str, code: &str, message: &str) -> J {
    json!({
        "content": [{"type": "text", "text": format!("Error: {message}")}],
        "structuredContent": {"code": code, "owner": owner, "message": message},
        "isError": true,
    })
}

/// hostmcp's `structuredResult`: text content only when text is given.
pub fn structured_result(value: J, text: Option<&str>) -> J {
    let content: Vec<J> = text
        .map(|t| json!({"type": "text", "text": t}))
        .into_iter()
        .collect();
    json!({"content": content, "structuredContent": value})
}

/// `json.Unmarshal(arguments, &map[string]any{})`.
fn decode_arguments(params: Option<&Node>) -> Result<Map<String, J>, String> {
    let Some(raw) = params.and_then(|p| p.field("arguments")) else {
        return Ok(Map::new());
    };
    let kind = match &raw.value {
        Value::Null => return Ok(Map::new()),
        Value::Object(_) => {
            return Ok(match crate::transport::go_any(raw) {
                J::Object(m) => m,
                _ => Map::new(),
            })
        }
        Value::Array(_) => "array",
        Value::String(_) => "string",
        Value::Number(_) => "number",
        Value::Bool(_) => "bool",
    };
    Err(format!(
        "json: cannot unmarshal {kind} into Go value of type map[string]interface {{}}"
    ))
}

fn requested_revision(params: Option<&Node>) -> String {
    params
        .and_then(|p| p.field("_meta"))
        .and_then(|m| m.key("catalogRevision"))
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

pub fn call(server: &Server, wire: &str, params: Option<&Node>) -> ToolCallOutcome {
    let Some(name) = registered(server, wire) else {
        return ToolCallOutcome::Unknown;
    };
    let requested = requested_revision(params);
    if !requested.is_empty() && requested != server.catalog.revision {
        return ToolCallOutcome::Result(capability_error(
            "lifecycle",
            "catalog_revision_stale",
            &format!(
                "catalog revision {} is stale; current revision is {}",
                crate::goerr::quote(&requested),
                crate::goerr::quote(&server.catalog.revision)
            ),
        ));
    }
    let args = match decode_arguments(params) {
        Ok(a) => a,
        Err(e) => return ToolCallOutcome::Result(error_result(&format!("invalid arguments: {e}"))),
    };
    if server.standalone && catalog::is_standalone_mutation(&name) && !server.allow_mutations {
        return ToolCallOutcome::Result(error_result(
            "standalone mutations are disabled; set OPUTE_STANDALONE_ALLOW_MUTATIONS=true",
        ));
    }
    // Lifecycle tools are routed before the task boundary.
    if name == "get_capability_catalog" {
        return ToolCallOutcome::Result(structured_result(snapshot_json(server.catalog), None));
    }
    if catalog::is_task_aware(&name) && !LIFECYCLE.contains(&name.as_str()) {
        if !task_extension_declared(params) {
            return missing_tasks_capability();
        }
        // The MCP Tasks lifecycle arrives with M4.
        return ToolCallOutcome::Result(not_implemented(&name));
    }
    ToolCallOutcome::Result(dispatch(server, &name, &args))
}

/// `isLifecycleTool`: transport-owned operations routed before tasks.
const LIFECYCLE: &[&str] = &[
    "validate_host_plan",
    "run_host_plan",
    "get_host_plan_run",
    "validate_host_local_recipe",
    "run_host_local_recipe",
    "validate_runtime_recipe",
    "run_runtime_recipe",
    "get_runtime_recipe_run",
    "validate_tunnel_recipe",
    "run_tunnel_recipe",
    "get_tunnel_run",
    "opute.provider.install",
    "opute.provider.validate",
    "opute.provider.status",
    "opute.provider.reload",
    "opute.provider.teardown",
    "get_capability_catalog",
    "open_assistant_session",
];

/// A capability whose handler this build does not carry yet. It fails
/// closed with a typed error rather than doing anything.
fn not_implemented(name: &str) -> J {
    capability_error(
        "capability",
        "not_implemented",
        &format!("{name} is not implemented by this Host Agent build"),
    )
}

fn dispatch(_server: &Server, name: &str, _args: &Map<String, J>) -> J {
    not_implemented(name)
}

/// `taskExtensionDeclared`.
fn task_extension_declared(params: Option<&Node>) -> bool {
    let Some(meta) = params.and_then(|p| p.field("_meta")) else {
        return false;
    };
    let capabilities = meta
        .key("io.modelcontextprotocol/clientCapabilities")
        .filter(|c| c.as_object().is_some())
        .or_else(|| meta.key("clientCapabilities"));
    capabilities
        .and_then(|c| c.key("extensions"))
        .filter(|e| e.as_object().is_some())
        .and_then(|e| e.key("io.modelcontextprotocol/tasks"))
        .is_some()
}

/// `missingTasksCapabilityError`: the Tasks extension's -32003.
fn missing_tasks_capability() -> ToolCallOutcome {
    ToolCallOutcome::Protocol(
        -32003,
        "Missing required client capability".into(),
        Some(
            json!({"requiredCapabilities": {"extensions": {"io.modelcontextprotocol/tasks": {}}}}),
        ),
    )
}

/// `CapabilityCatalogSnapshot` as published.
fn snapshot_json(snapshot: &catalog::Snapshot) -> J {
    let mut s = String::new();
    snapshot.encode(&mut s);
    serde_json::from_str(&s).expect("snapshot encodes as JSON")
}
