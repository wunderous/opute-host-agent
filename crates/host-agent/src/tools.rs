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
use std::sync::{Arc, Mutex};

/// What host capabilities read: the agent's environment and identity, the
/// incus provider, the resource coordinator, and the resource registry.
pub struct Host {
    pub env: crate::config::Env,
    pub incus: crate::incus::Incus,
    pub coordinator: crate::resource::Coordinator,
    pub state: Arc<Mutex<crate::store::StateStore>>,
    pub tenant_id: String,
    pub agent_id: String,
    pub instance_id: String,
    pub instance_root: String,
    pub mcp_port: i64,
}

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

fn dispatch(server: &Server, name: &str, args: &Map<String, J>) -> J {
    let Some(descriptor) = server.catalog.tools.iter().find(|d| d.name == name) else {
        return not_implemented(name);
    };
    // Capabilities that take a canonical resource argument are admitted by
    // resource binding, which arrives with M4; until then they fail closed.
    if !descriptor.requires.is_empty() {
        return not_implemented(name);
    }
    let handler: fn(&Host, &Map<String, J>) -> Result<J, String> = match name {
        "get_host_info" => |host, _| Ok(structured_with_text(&describe_host(host), &HOST_INFO)),
        "list_vms" => |host, args| {
            let fast = args.get("fast").and_then(J::as_bool).unwrap_or(false);
            let register = |uri: &str, coordinates: Map<String, J>| {
                let _ = crate::resource::register(&host.state, &host.tenant_id, uri, &coordinates);
            };
            host.incus
                .list_vms(fast, &register)
                .map(|v| structured_with_text(&v, &VM_LIST))
        },
        "detect_host_platform" => |host, _| {
            let platform = crate::hostobs::detect_platform(&host.env);
            let text = format!(
                "Host platform detected: {} on {}.",
                platform["kind"].as_str().unwrap_or(""),
                platform["cpu"]["architecture"].as_str().unwrap_or("")
            );
            Ok(json!({"content": [{"type": "text", "text": text}], "structuredContent": platform}))
        },
        "get_host_capacity" => |host, _| {
            Ok(json!({
                "content": [{"type": "text", "text": "Host capacity and enforcement state observed."}],
                "structuredContent": host.coordinator.snapshot(),
            }))
        },
        _ => return not_implemented(name),
    };
    // The legacy adapter's declarative gate: arguments against the input
    // schema, then a successful structured result against the output schema.
    let input = descriptor
        .input_schema
        .as_object()
        .cloned()
        .unwrap_or_default();
    let checked = crate::schema::arguments_for_validation(&input, args);
    if let Err(e) = crate::schema::validate(&input, &J::Object(checked)) {
        let message = format!("invalid capability arguments: {e}");
        return capability_error("capability", "invalid_arguments", &message);
    }
    let result = match handler(&server.host, args) {
        Ok(result) => result,
        Err(e) => return error_result(&e),
    };
    let output = descriptor
        .output_schema
        .as_ref()
        .and_then(J::as_object)
        .cloned()
        .unwrap_or_default();
    if result.get("isError") != Some(&J::Bool(true)) {
        let structured = result.get("structuredContent").cloned().unwrap_or(J::Null);
        if let Err(e) = crate::schema::validate(&output, &structured) {
            let message = format!(
                "capability {} returned invalid result: structured result does not match output schema: {e}",
                crate::goerr::quote(name)
            );
            return capability_error("capability", "invalid_result", &message);
        }
    }
    result
}

use crate::gojson::Shape;

const VM_INFO: Shape = Shape::Struct(&[
    ("uri", Shape::Any),
    ("kind", Shape::Any),
    ("name", Shape::Any),
    ("type", Shape::Any),
    ("status", Shape::Any),
    ("state", Shape::Any),
    ("ipv4", Shape::Any),
    ("release", Shape::Any),
    ("providerId", Shape::Any),
    ("cpus", Shape::Any),
    ("memory", Shape::Any),
    ("disk", Shape::Any),
    ("agentReady", Shape::Any),
    ("hostId", Shape::Any),
]);
const VM_LIST: Shape = Shape::Struct(&[("vms", Shape::List(&VM_INFO))]);
const STALL: Shape = Shape::Struct(&[
    ("someAvg10", Shape::Any),
    ("someAvg60", Shape::Any),
    ("someAvg300", Shape::Any),
    ("someTotalUsec", Shape::Any),
    ("fullAvg10", Shape::Any),
    ("fullAvg60", Shape::Any),
    ("fullAvg300", Shape::Any),
    ("fullTotalUsec", Shape::Any),
]);
const LIMITS: &[(&str, Shape)] = &[
    ("cpuCores", Shape::Any),
    ("memoryBytes", Shape::Any),
    ("diskBytes", Shape::Any),
    ("tasks", Shape::Any),
];
const ADMISSION: Shape = Shape::Keys(&[
    ("effectiveLimits", Shape::Struct(LIMITS)),
    (
        "currentUsage",
        Shape::Struct(&[
            ("cpuCores", Shape::Any),
            ("memoryBytes", Shape::Any),
            ("memoryAvailableBytes", Shape::Any),
            ("diskBytes", Shape::Any),
            ("diskAvailableBytes", Shape::Any),
            ("tasks", Shape::Any),
        ]),
    ),
    (
        "reservations",
        Shape::Struct(&[
            ("count", Shape::Any),
            ("cpuCores", Shape::Any),
            ("memoryBytes", Shape::Any),
            ("diskBytes", Shape::Any),
            ("tasks", Shape::Any),
        ]),
    ),
    (
        "queue",
        Shape::Struct(&[
            ("queued", Shape::Any),
            ("heavyQueued", Shape::Any),
            ("normalActive", Shape::Any),
            ("heavyActive", Shape::Any),
        ]),
    ),
    ("psi", Shape::Map(&STALL)),
]);
const HOST_INFO: Shape = Shape::Struct(&[
    ("uri", Shape::Any),
    ("hostName", Shape::Any),
    ("providerId", Shape::Any),
    ("lxcBinaryPath", Shape::Any),
    ("systemctlPath", Shape::Any),
    ("supportedTools", Shape::Any),
    (
        "capacity",
        Shape::Struct(&[
            ("runningVmCount", Shape::Any),
            ("totalVmCount", Shape::Any),
            ("runningVmCpuLimitCores", Shape::Any),
            ("totalVmCpuLimitCores", Shape::Any),
            ("runningVmMemoryLimitBytes", Shape::Any),
            ("totalVmMemoryLimitBytes", Shape::Any),
            ("runningVmDiskLimitBytes", Shape::Any),
            ("totalVmDiskLimitBytes", Shape::Any),
            ("runningQemuCount", Shape::Any),
            ("totalQemuCount", Shape::Any),
            ("runningContainerCount", Shape::Any),
            ("totalContainerCount", Shape::Any),
        ]),
    ),
    (
        "rootDiskQuota",
        Shape::Struct(&[
            ("pool", Shape::Any),
            ("driver", Shape::Any),
            ("enforced", Shape::Any),
            ("reason", Shape::Any),
        ]),
    ),
    (
        "system",
        Shape::Keys(&[
            ("psi", Shape::Map(&STALL)),
            ("resourceAdmission", ADMISSION),
        ]),
    ),
    (
        "agent",
        Shape::Struct(&[
            ("agentId", Shape::Any),
            ("instanceId", Shape::Any),
            ("instanceRoot", Shape::Any),
            ("environmentFile", Shape::Any),
            ("homeDir", Shape::Any),
            ("serviceScope", Shape::Any),
            ("serviceUnitDir", Shape::Any),
            ("serviceWantedBy", Shape::Any),
            ("mcpEndpoint", Shape::Any),
            ("providerRoot", Shape::Any),
        ]),
    ),
]);

/// tools' `structuredResult(value, "")`: the value, and its JSON (as the Go
/// type `shape` marshals) as text.
fn structured_with_text(value: &J, shape: &Shape) -> J {
    let mut text = String::new();
    crate::gojson::encode_shaped(value, shape, &mut text);
    json!({"content": [{"type": "text", "text": text}], "structuredContent": value})
}

/// `host.Service.DescribeHost`.
fn describe_host(host: &Host) -> J {
    let host_name = nix::unistd::gethostname()
        .map(|h: std::ffi::OsString| h.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut result = Map::new();
    let identity = crate::incus::first_non_empty(&[&host.agent_id, &host_name]).to_string();
    if let Ok(uri) = crate::resource::Uri::new("host", &host.tenant_id, &identity) {
        let uri = uri.to_string();
        let mut coordinates = Map::new();
        coordinates.insert("agentId".into(), J::from(host.agent_id.clone()));
        coordinates.insert("hostName".into(), J::from(host_name.clone()));
        let _ = crate::resource::register(&host.state, &host.tenant_id, &uri, &coordinates);
        result.insert("uri".into(), J::from(uri));
    } else {
        result.insert("uri".into(), J::from(""));
    }
    result.insert("hostName".into(), J::from(host_name));
    result.insert("providerId".into(), J::from("incus"));
    result.insert("lxcBinaryPath".into(), J::from(host.incus.binary.clone()));
    result.insert("systemctlPath".into(), J::from("/usr/bin/systemctl"));
    result.insert("supportedTools".into(), json!(catalog::host_tool_names()));
    if let Ok(capacity) = host.incus.inventory_capacity() {
        result.insert("capacity".into(), capacity);
    }
    if let Ok(quota) = host.incus.root_disk_quota() {
        result.insert("rootDiskQuota".into(), quota);
    }
    result.insert(
        "agent".into(),
        crate::hostobs::agent_installation(
            &host.env,
            &host.agent_id,
            &host.instance_id,
            &host.instance_root,
            host.mcp_port,
        ),
    );
    let paths = crate::hostobs::default_disk_paths(&host.env);
    let mut system = crate::hostobs::HostSystemStats::read(&paths)
        .metadata()
        .unwrap_or_default();
    system.insert("resourceAdmission".into(), host.coordinator.metadata());
    result.insert("system".into(), J::Object(system));
    J::Object(result)
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
        // ClientCapabilities.Roots is a struct, which omitempty never drops.
        Some(json!({"requiredCapabilities": {
            "extensions": {"io.modelcontextprotocol/tasks": {}},
            "roots": {},
        }})),
    )
}

/// `CapabilityCatalogSnapshot` as published.
fn snapshot_json(snapshot: &catalog::Snapshot) -> J {
    let mut s = String::new();
    snapshot.encode(&mut s);
    serde_json::from_str(&s).expect("snapshot encodes as JSON")
}
