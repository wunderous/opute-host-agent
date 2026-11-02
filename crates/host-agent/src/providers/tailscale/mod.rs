//! `opute-provider-tailscale` (Go `plugins/tunneling/tailscale`).
//!
//! Service Definitions: mesh-runtime, mesh-membership, private-mesh and
//! public-ingress, and the teardown, as the install manifest declares them.
//! Service Provider: an in-process ownership store driven by the fake
//! backend (the default) or by the live backend, which reaches guests only
//! through typed Host Agent callbacks and devices through Tailscale's API.
//! Consumer: the Host Agent's provider binding.
//!
//! ```text
//!  tools/call -> requestArguments -> seam operation -> network-overlay id
//!    OPUTE_TAILSCALE_BACKEND unset or "fake"  overlay.rs (store only)
//!    otherwise                                 live.rs (callbacks + API)
//! ```

mod api;
mod live;
mod overlay;
#[cfg(test)]
mod tests;

use super::support::{first_non_empty, string_input, teardown_input, Args};
use super::{Call, Server, Tool};
use crate::goerr::quote;
use crate::mcpsdk::ToolCallOutcome;
use crate::provider::definition::{
    CapabilityRef, GoSlice, InstallManifest, Operation, RecipeRef, RecipeSource,
    ResourceBinding, ResourceCost, ServiceDefinition, ValidationRef,
};
use crate::provider::ProviderRef;
use serde_json::{json, Map, Value as J};
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

const TUNNELING: &str = "opute.capability.tunneling.v1";
const GENERATION: &str = "com.opute.tailscale@1.0.0";
const PLUGIN_ID: &str = "com.opute.tailscale";
const VERSION: &str = "1.0.0";
const MESH_RUNTIME: &str = "opute.capability.mesh-runtime.v1";
const MESH_MEMBERSHIP: &str = "opute.capability.mesh-membership.v1";
const PRIVATE_MESH: &str = "opute.capability.private-mesh.v1";
const PUBLIC_INGRESS: &str = "opute.capability.public-ingress.v1";
const NETWORK_OVERLAY: &str = "opute.capability.network-overlay.v1";

/// The capability operation ids this provider routes.
mod op {
    pub const MESH_RUNTIME_VALIDATE: &str = "opute.capability.mesh-runtime.validate";
    pub const MESH_RUNTIME_ENSURE_AGENT: &str = "opute.capability.mesh-runtime.ensure-agent";
    pub const MESH_RUNTIME_ENSURE_CONTROL_PLANE: &str = "opute.capability.mesh-runtime.ensure-control-plane";
    pub const MESH_RUNTIME_STATUS: &str = "opute.capability.mesh-runtime.status";
    pub const MEMBERSHIP_ENROLL: &str = "opute.capability.mesh-membership.enroll";
    pub const MEMBERSHIP_STATUS: &str = "opute.capability.mesh-membership.status";
    pub const MEMBERSHIP_LEAVE: &str = "opute.capability.mesh-membership.leave";
    pub const PRIVATE_MESH_ENSURE: &str = "opute.capability.private-mesh.ensure";
    pub const PRIVATE_MESH_ENSURE_SERVICE: &str = "opute.capability.private-mesh.ensure-service";
    pub const PRIVATE_MESH_PROBE: &str = "opute.capability.private-mesh.probe";
    pub const PUBLIC_INGRESS_ENSURE: &str = "opute.capability.public-ingress.ensure";
    pub const PUBLIC_INGRESS_PROMOTE: &str = "opute.capability.public-ingress.promote";
    pub const PUBLIC_INGRESS_PROBE: &str = "opute.capability.public-ingress.probe";
    pub const OVERLAY_VALIDATE: &str = "opute.capability.network-overlay.validate";
    pub const OVERLAY_PREPARE_MEMBERSHIP: &str = "opute.capability.network-overlay.prepare-membership";
    pub const OVERLAY_ATTACH_TARGET: &str = "opute.capability.network-overlay.attach-target";
    pub const OVERLAY_ENROLL: &str = "opute.capability.network-overlay.enroll";
    pub const OVERLAY_PROBE_REACHABILITY: &str = "opute.capability.network-overlay.probe-reachability";
    pub const OVERLAY_PROBE: &str = "opute.capability.network-overlay.probe";
    pub const OVERLAY_REPORT_TWO_NODE_READINESS: &str = "opute.capability.network-overlay.report-two-node-readiness";
    pub const OVERLAY_ENSURE_PRIVATE_MESH: &str = "opute.capability.network-overlay.ensure-private-mesh";
    pub const OVERLAY_ENSURE_PRIVATE_SERVICE: &str = "opute.capability.network-overlay.ensure-private-service";
    pub const OVERLAY_ENSURE_PUBLIC_INGRESS: &str = "opute.capability.network-overlay.ensure-public-ingress";
    pub const OVERLAY_ENSURE_HA_ENDPOINT: &str = "opute.capability.network-overlay.ensure-ha-endpoint";
    pub const OVERLAY_PROMOTE_PUBLIC_INGRESS: &str = "opute.capability.network-overlay.promote-public-ingress";
    pub const OVERLAY_REMOVE_HA_ENDPOINT: &str = "opute.capability.network-overlay.remove-ha-endpoint";
    pub const OVERLAY_REMOVE_MEMBERSHIP: &str = "opute.capability.network-overlay.remove-membership";
}

/// A handler's result: a value for `structured`, or a handler error.
type Outcome = Result<J, String>;

/// The call context every callback uses: the request's resource delegation.
struct Ctx {
    delegation: String,
}

pub fn main() -> ! {
    super::run(server(), "4321", |port| {
        format!("Opute Tailscale provider listening on :{port}/mcp")
    })
}

pub fn server() -> Server {
    let manifest = manifest();
    let manifest_text = super::go_text(serde_json::to_string(&manifest).expect("manifest"));
    let manifest_value = serde_json::to_value(&manifest).expect("manifest");
    let mut tools = vec![Tool {
        name: "opute.provider.get_install_manifest".into(),
        description: "Read the Tailscale provider installation manifest".into(),
        input_schema: json!({"type": "object"}),
        output_schema: Some(json!({"type": "object"})),
        handler: Box::new(move |_| {
            super::structured(manifest_text.clone(), manifest_value.clone())
        }),
    }];
    for operation in all_operations() {
        let id = operation.id.clone();
        tools.push(Tool {
            name: operation.id.clone(),
            description: "Network overlay provider operation".into(),
            input_schema: J::Object(operation.input_schema.clone().unwrap_or_default()),
            output_schema: operation.output_schema.clone().map(J::Object),
            handler: Box::new(move |call: &Call| {
                let args = match Args::decode(call.arguments, "decode provider arguments: ") {
                    Ok(args) => args,
                    Err(e) => return ToolCallOutcome::Protocol(0, e, None),
                };
                let ctx = Ctx { delegation: call.delegation() };
                respond(dispatch(&ctx, &id, args))
            }),
        });
    }
    tools.push(Tool {
        name: "opute.provider.teardown".into(),
        description: "Return a cleanup plan and finalize owned overlay resources".into(),
        input_schema: teardown_schema(),
        output_schema: Some(json!({"type": "object"})),
        handler: Box::new(|call: &Call| {
            let ctx = Ctx { delegation: call.delegation() };
            respond(teardown(&ctx, call))
        }),
    });
    Server {
        name: "opute-provider-tailscale",
        version: VERSION,
        capabilities: json!({"tools": {"listChanged": true}}),
        tools,
        wrap: Some(json!({"name": "opute-provider-tailscale", "version": VERSION})),
    }
}

/// `structured(value)`, or the handler's error.
fn respond(outcome: Outcome) -> ToolCallOutcome {
    match outcome {
        Ok(value) => {
            let mut text = String::new();
            crate::gojson::encode(&value, &mut text);
            super::structured(super::go_text(text), value)
        }
        Err(e) => ToolCallOutcome::Protocol(0, e, None),
    }
}

// --- the process environment ---------------------------------------------------------

/// The live backend writes the process environment (`os.Setenv`); those
/// writes live here, read before the real environment.
static ENV: LazyLock<Mutex<HashMap<String, String>>> = LazyLock::new(Default::default);

/// `os.Getenv`.
fn getenv(key: &str) -> String {
    let written = ENV.lock().unwrap_or_else(|e| e.into_inner()).get(key).cloned();
    written.unwrap_or_else(|| std::env::var(key).unwrap_or_default())
}

/// `os.Setenv`.
fn setenv(key: &str, value: &str) {
    ENV.lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(key.to_string(), value.to_string());
}

// --- the Service Definition ---------------------------------------------------------------

fn object(value: J) -> Option<Map<String, J>> {
    match value {
        J::Object(map) => Some(map),
        _ => None,
    }
}

fn recipe(id: &str, uri: &str, sha: &str, mode: &str) -> RecipeRef {
    RecipeRef {
        id: id.into(),
        source: RecipeSource {
            uri: uri.into(),
            revision: "working-tree".into(),
            sha256: sha.into(),
        },
        mode: mode.into(),
        inputs: None,
    }
}

fn manifest() -> InstallManifest {
    let capability = |id: &str| CapabilityRef { id: id.into(), version: 1 };
    let service = |id: &str, capability_id: &str, operations: Vec<Operation>| ServiceDefinition {
        id: id.into(),
        capability_id: capability_id.into(),
        version: 1,
        operations: GoSlice(Some(operations)),
        ..ServiceDefinition::default()
    };
    InstallManifest {
        schema: "opute-provider-install-manifest.v1".into(),
        provider: ProviderRef { id: PLUGIN_ID.into(), version: VERSION.into() },
        provides: GoSlice(Some(vec![
            capability(MESH_RUNTIME),
            capability(MESH_MEMBERSHIP),
            capability(PRIVATE_MESH),
            capability(PUBLIC_INGRESS),
        ])),
        recipes: GoSlice(Some(vec![
            recipe("com.opute.tailscale.activate", "recipes/activate.yaml", "sha256:f878248c0cd8c9035a2ab3d2e7f92ca80aeef33b82747d96b7f4053249722204", "activate"),
            recipe("com.opute.tailscale.overlay-mesh", "recipes/overlay-mesh.yaml", "sha256:3c783ab82f5e1cafdc657e29ab569fef7c906d61a351554efbc814ebfbc2a27c", "private-mesh"),
            recipe("com.opute.tailscale.public-ingress", "recipes/public-ingress.yaml", "sha256:97528bf47b0f350ca4f57cf5ed957e99ba2c5f3027d1573d1e38e0fd5d9b2da6", "public-ingress"),
            recipe("com.opute.tailscale.ha-network-bundle", "recipes/ha-network-bundle.yaml", "sha256:7ede770cca2ee6ed3fa350b768cc2a75a9f9a63b78f2a99f5a7939aba238d6cc", "vendor-bundle"),
            recipe("com.opute.tailscale.install", "recipes/install.yaml", "sha256:b7c3560f037fe7f493f483c70944c6f97a52485b49d189b05134b115e3129dba", "install"),
        ])),
        services: GoSlice(Some(vec![
            service("opute.capability.mesh-runtime", MESH_RUNTIME, mesh_runtime_operations()),
            service("opute.capability.mesh-membership", MESH_MEMBERSHIP, mesh_membership_operations()),
            service("opute.capability.private-mesh", PRIVATE_MESH, private_mesh_operations()),
            service("opute.capability.public-ingress", PUBLIC_INGRESS, public_ingress_operations()),
        ])),
        teardown: Some(Operation {
            id: "opute.provider.teardown".into(),
            version: 1,
            input_schema: object(teardown_schema()),
            output_schema: object(json!({"type": "object", "required": ["contractVersion", "plan"]})),
            effect: "destructive".into(),
            resource_kinds: GoSlice(Some(vec!["service".into(), "network".into()])),
            idempotent: true,
            supports_readiness: true,
            task_support: "sync_only".into(),
            resource_cost: Some(ResourceCost { class: "control".into(), ..ResourceCost::default() }),
            ..Operation::default()
        }),
        validation: ValidationRef { capability: PUBLIC_INGRESS.into(), operation: op::PUBLIC_INGRESS_PROBE.into() },
        ..InstallManifest::default()
    }
}

/// `providerOperation`: every overlay operation is synchronous, so recipes
/// validate readiness against the same in-process store; an output schema
/// that is only `{"type":"object"}` becomes the overlay output schema.
fn provider_operation(id: &str, effect: &str, input: J, output: J) -> Operation {
    let output = if output == json!({"type": "object"}) { overlay_output_schema() } else { output };
    Operation {
        id: id.into(),
        version: 1,
        input_schema: object(input),
        output_schema: object(output),
        effect: effect.into(),
        resource_kinds: GoSlice(Some(vec!["host".into(), "network".into()])),
        requires: GoSlice(Some(target_binding())),
        idempotent: true,
        supports_readiness: effect != "read",
        task_support: "sync_only".into(),
        resource_cost: (effect != "read").then(|| ResourceCost { class: "control".into(), ..ResourceCost::default() }),
        ..Operation::default()
    }
}

fn overlay_output_schema() -> J {
    let string = || json!({"type": "string"});
    let boolean = || json!({"type": "boolean"});
    json!({"type": "object", "properties": {
        "membershipRef": string(), "ready": boolean(), "meshIp": string(), "overlayUri": string(),
        "targetUri": string(), "endpoint": string(), "stable": boolean(), "pathClass": string(),
        "endpointRef": string(), "operatorMode": boolean(), "operatorEvidence": string(),
        "ingressClassName": string(), "probe": {"type": "object"}, "contractVersion": string(),
        "generation": string(), "provider": string(), "error": string(),
    }})
}

fn mesh_runtime_output_schema() -> J {
    let string = || json!({"type": "string"});
    let boolean = || json!({"type": "boolean"});
    json!({"type": "object", "properties": {
        "contractVersion": string(), "ready": boolean(), "provider": string(), "targetUri": string(),
        "agentReady": boolean(), "controlPlaneReady": boolean(), "agentVersion": string(),
        "controlPlaneRef": string(), "ingressClassName": string(), "probe": {"type": "object"},
        "error": string(), "generation": string(),
    }})
}

/// The mesh target is a VM or a system container, either canonical form.
fn target_binding() -> Vec<ResourceBinding> {
    ["vm", "container"]
        .iter()
        .map(|kind| ResourceBinding {
            argument: "targetUri".into(),
            resource_type: (*kind).into(),
            required: true,
            ..ResourceBinding::default()
        })
        .collect()
}

fn target_uri_schema() -> J {
    json!({"type": "string", "pattern": "^(vm|container):[a-z][a-z0-9-]{0,31}:.+$"})
}

/// The operation's own properties merged with the overlay credentials.
fn input(required: &[&str], props: J) -> J {
    let mut properties = props.as_object().cloned().unwrap_or_default();
    for (k, v) in [
        ("hostAgentId", json!({"type": "string", "minLength": 1})),
        ("credentialKind", json!({"type": "string", "enum": ["api-key", "auth-key"]})),
        ("credential", json!({"type": "string", "writeOnly": true})),
        ("apiKey", json!({"type": "string", "writeOnly": true})),
        ("authKey", json!({"type": "string", "writeOnly": true})),
    ] {
        properties.insert(k.into(), v);
    }
    json!({"type": "object", "required": required, "properties": properties})
}

fn membership_ref_schema() -> J {
    json!({"type": "string", "minLength": 1, "writeOnly": true})
}

fn mesh_runtime_operations() -> Vec<Operation> {
    let target = || json!({"targetUri": target_uri_schema()});
    let out = mesh_runtime_output_schema;
    vec![
        provider_operation(op::MESH_RUNTIME_VALIDATE, "read", input(&["targetUri"], target()), out()),
        provider_operation(op::MESH_RUNTIME_ENSURE_AGENT, "mutation", input(&["targetUri"], json!({
            "targetUri": target_uri_schema(),
            "agentMode": {"type": "string", "enum": ["node-agent", "host-agent"]},
        })), out()),
        provider_operation(op::MESH_RUNTIME_ENSURE_CONTROL_PLANE, "mutation", input(&["targetUri"], json!({
            "targetUri": target_uri_schema(),
            "operatorMode": {"type": "boolean"},
            "ingressClassName": {"type": "string"},
            "operatorEvidence": {"type": "string"},
            "controlPlaneRef": {"type": "string"},
        })), out()),
        provider_operation(op::MESH_RUNTIME_STATUS, "read", input(&["targetUri"], target()), out()),
    ]
}

fn mesh_membership_operations() -> Vec<Operation> {
    let object = || json!({"type": "object"});
    vec![
        provider_operation(op::MEMBERSHIP_ENROLL, "mutation", input(&["targetUri", "name"], json!({
            "targetUri": target_uri_schema(),
            "name": {"type": "string", "pattern": "^[A-Za-z0-9][A-Za-z0-9_.-]{0,62}$"},
            "membershipRef": membership_ref_schema(),
        })), object()),
        provider_operation(op::MEMBERSHIP_STATUS, "read", input(&["targetUri"], json!({"targetUri": target_uri_schema()})), object()),
        provider_operation(op::MEMBERSHIP_LEAVE, "mutation", input(&["targetUri", "membershipRef"], json!({
            "targetUri": target_uri_schema(),
            "membershipRef": membership_ref_schema(),
        })), object()),
    ]
}

fn private_mesh_operations() -> Vec<Operation> {
    let object = || json!({"type": "object"});
    vec![
        provider_operation(op::PRIVATE_MESH_ENSURE, "mutation", input(&["targetUri", "membershipRef", "peerTargetUri"], json!({
            "targetUri": target_uri_schema(),
            "membershipRef": membership_ref_schema(),
            "peerTargetUri": target_uri_schema(),
            "peerMeshIp": {"type": "string", "format": "ipv4"},
        })), object()),
        provider_operation(op::PRIVATE_MESH_ENSURE_SERVICE, "mutation", input(&["targetUri", "membershipRef", "serviceName", "port"], json!({
            "targetUri": target_uri_schema(),
            "membershipRef": membership_ref_schema(),
            "serviceName": {"type": "string", "minLength": 1},
            "port": {"type": "integer", "minimum": 1, "maximum": 65535},
        })), object()),
        provider_operation(op::PRIVATE_MESH_PROBE, "read", input(&["targetUri"], json!({
            "targetUri": target_uri_schema(),
            "peerMeshIp": {"type": "string", "format": "ipv4"},
        })), object()),
    ]
}

fn public_ingress_operations() -> Vec<Operation> {
    let object = || json!({"type": "object"});
    vec![
        provider_operation(op::PUBLIC_INGRESS_ENSURE, "mutation", input(&["targetUri", "membershipRef", "localTarget"], json!({
            "targetUri": target_uri_schema(),
            "membershipRef": membership_ref_schema(),
            "localTarget": {"type": "string", "format": "uri"},
            "hostname": {"type": "string"},
            "endpoint": {"type": "string"},
            "operatorMode": {"type": "boolean"},
            "ingressClassName": {"type": "string"},
            "operatorEvidence": {"type": "string"},
        })), object()),
        provider_operation(op::PUBLIC_INGRESS_PROMOTE, "mutation", input(&["targetUri", "membershipRef"], json!({
            "targetUri": target_uri_schema(),
            "membershipRef": membership_ref_schema(),
        })), object()),
        provider_operation(op::PUBLIC_INGRESS_PROBE, "read", input(&["targetUri"], json!({"targetUri": target_uri_schema()})), object()),
    ]
}

fn all_operations() -> Vec<Operation> {
    mesh_runtime_operations()
        .into_iter()
        .chain(mesh_membership_operations())
        .chain(private_mesh_operations())
        .chain(public_ingress_operations())
        .collect()
}

fn teardown_schema() -> J {
    json!({"type": "object", "required": ["inputs"], "properties": {"inputs": {"type": "object", "properties": {
        "phase": {"type": "string", "enum": ["prepare", "finalize"]},
        "serviceName": {"type": "string"},
        "serviceFile": {"type": "string"},
        "scope": {"type": "string", "enum": ["user", "system"]},
        "generation": {"type": "string"},
    }}}})
}

// --- dispatch ----------------------------------------------------------------------------------

fn dispatch(ctx: &Ctx, operation: &str, args: Args) -> Outcome {
    match operation {
        "opute.capability.tunneling.validate" => {
            let bindings = args.get("bindings").and_then(J::as_array).cloned().unwrap_or_default();
            if bindings.is_empty() {
                return Err("bindings must contain at least one declared binding".into());
            }
            let placement = first_non_empty(&[&args.string("placement", ""), "host"]);
            if !matches!(placement.as_str(), "host" | "container" | "kubernetes") {
                return Err(format!("unsupported placement {}", quote(&placement)));
            }
            Ok(json!({"contractVersion": TUNNELING, "ready": true, "bindings": bindings, "placement": placement}))
        }
        op::MESH_RUNTIME_VALIDATE
        | op::MESH_RUNTIME_ENSURE_AGENT
        | op::MESH_RUNTIME_ENSURE_CONTROL_PLANE
        | op::MESH_RUNTIME_STATUS
        | op::MEMBERSHIP_ENROLL
        | op::MEMBERSHIP_STATUS
        | op::MEMBERSHIP_LEAVE
        | op::PRIVATE_MESH_ENSURE
        | op::PRIVATE_MESH_ENSURE_SERVICE
        | op::PRIVATE_MESH_PROBE
        | op::PUBLIC_INGRESS_ENSURE
        | op::PUBLIC_INGRESS_PROMOTE
        | op::PUBLIC_INGRESS_PROBE
        | op::OVERLAY_VALIDATE
        | op::OVERLAY_PREPARE_MEMBERSHIP
        | op::OVERLAY_ATTACH_TARGET
        | op::OVERLAY_ENROLL
        | op::OVERLAY_PROBE_REACHABILITY
        | op::OVERLAY_PROBE
        | op::OVERLAY_ENSURE_PRIVATE_MESH
        | op::OVERLAY_ENSURE_PRIVATE_SERVICE
        | op::OVERLAY_ENSURE_PUBLIC_INGRESS
        | op::OVERLAY_ENSURE_HA_ENDPOINT
        | op::OVERLAY_PROMOTE_PUBLIC_INGRESS
        | op::OVERLAY_REMOVE_HA_ENDPOINT
        | op::OVERLAY_REMOVE_MEMBERSHIP => overlay::dispatch(ctx, operation, args),
        _ => Err(format!("unknown provider operation {}", quote(operation))),
    }
}

/// `serviceNamePattern`: `^[A-Za-z0-9_.@:-]+\.service$`.
fn service_name_ok(name: &str) -> bool {
    name.strip_suffix(".service").is_some_and(|stem| {
        !stem.is_empty()
            && stem
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'@' | b':' | b'-'))
    })
}

fn teardown(ctx: &Ctx, call: &Call) -> Outcome {
    let (phase, inputs) = teardown_input(call.arguments)?;
    if first_non_empty(&[&phase, &string_input(&inputs, "phase", "")]) == "finalize" {
        overlay::finalize_owned_teardown(ctx, &inputs)?;
        return Ok(json!({"completed": true}));
    }
    let service_name = first_non_empty(&[&string_input(&inputs, "serviceName", ""), "opute-provider-tailscale.service"]);
    let scope = first_non_empty(&[&string_input(&inputs, "scope", ""), "user"]);
    if scope != "user" && scope != "system" {
        return Err(format!("unsupported service scope {}", quote(&scope)));
    }
    if !service_name_ok(&service_name) {
        return Err("serviceName must name a .service unit".into());
    }
    let default_file = if scope == "system" {
        format!("/etc/systemd/system/{service_name}")
    } else {
        format!("~/.config/systemd/user/{service_name}")
    };
    let service_file = first_non_empty(&[&string_input(&inputs, "serviceFile", ""), &default_file]);
    if scope == "system"
        && !service_file
            .strip_prefix("/etc/systemd/system/")
            .is_some_and(service_name_ok)
    {
        return Err("system-scoped service file must be one .service unit beneath /etc/systemd/system".into());
    }
    let service_uri = first_non_empty(&[
        &string_input(&inputs, "serviceUri", ""),
        &format!("host-service:local:{scope}/{service_name}"),
    ]);
    let generation = first_non_empty(&[&string_input(&inputs, "generation", ""), GENERATION]);
    let plan_id = "com.opute.tailscale.teardown";
    Ok(json!({
        "contractVersion": "host-plan.v1",
        "plan": {
            "contractVersion": "host-plan.v1",
            "planId": plan_id,
            "generation": 1,
            "idempotencyKey": format!("{plan_id}-{service_name}-{service_file}-{generation}"),
            "nodes": [{
                "id": "inspect-service",
                "action": {"tool": "inspect_host_service", "args": {"uri": service_uri, "scope": scope}},
            }],
        },
    }))
}
