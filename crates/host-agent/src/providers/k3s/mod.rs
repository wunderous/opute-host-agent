//! `opute-provider-k3s` (Go `plugins/kubernetes/k3s`).
//!
//! Service Definition: the kubernetes capability (31 operations) and the
//! teardown, as the install manifest declares them. Service Provider: the
//! handlers here, which drive the K3s guest through the `incus` CLI on the
//! provider's host, as Go's do. Consumer: the Host Agent's provider binding.
//!
//! ```text
//!  tools/call -> json.Unmarshal(arguments) -> target guard -> operation
//!    cluster   status, manifests, secrets, resources, events     (this file)
//!    join      membership, HA preparation, sealed join, quorum   (membership.rs)
//!    storage   guest disk, image prune, registry garbage collect (storage.rs)
//!    install   verified K3s binary staged from the host          (bootstrap.rs)
//!  incus <args> (stdin when given) -> stdout, or "incus K3s command failed"
//! ```
//!
//! Typed Go documents decode through `decode.rs`, so type errors keep Go's
//! struct field paths.

mod bootstrap;
mod decode;
mod membership;
mod storage;
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
use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const KUBERNETES: &str = "opute.capability.kubernetes.v1";
const VERSION: &str = "1.0.2";
const MAX_MANIFEST_BYTES: usize = 4 * 1024 * 1024;
const DEFAULT_K3S_VERSION: &str = "v1.31.8+k3s1";
const PROVISION_READINESS_TIMEOUT: Duration = Duration::from_secs(3 * 60);
const PROVISION_READINESS_POLL: Duration = Duration::from_secs(2);

/// The capability operation ids.
mod op {
    pub const VALIDATE: &str = "opute.capability.kubernetes.validate";
    pub const PROVISION: &str = "opute.capability.kubernetes.provision";
    pub const STATUS: &str = "opute.capability.kubernetes.status";
    pub const CONFIGURE_REGISTRY: &str = "opute.capability.kubernetes.configure-registry";
    pub const RESTART: &str = "opute.capability.kubernetes.restart";
    pub const REMOVE: &str = "opute.capability.kubernetes.remove";
    pub const APPLY_MANIFEST: &str = "opute.capability.kubernetes.apply-manifest";
    pub const PUT_SECRET: &str = "opute.capability.kubernetes.put-secret";
    pub const GET_RESOURCE: &str = "opute.capability.kubernetes.get-resource";
    pub const DELETE_RESOURCE: &str = "opute.capability.kubernetes.delete-resource";
    pub const GET_RESOURCE_STATUS: &str = "opute.capability.kubernetes.get-resource-status";
    pub const LIST_EVENTS: &str = "opute.capability.kubernetes.list-events";
    pub const LIST_CLUSTERS: &str = "opute.capability.kubernetes.list-clusters";
    pub const GET_CLUSTER_INFO: &str = "opute.capability.kubernetes.get-cluster-info";
    pub const INSPECT_MEMBERSHIP: &str = "opute.capability.kubernetes.inspect-membership";
    pub const PREPARE_HA: &str = "opute.capability.kubernetes.prepare-ha";
    pub const PREPARE_JOIN: &str = "opute.capability.kubernetes.prepare-join";
    pub const GET_JOIN_RECEIVER_KEY: &str = "opute.capability.kubernetes.get-join-receiver-key";
    pub const REDEEM_JOIN: &str = "opute.capability.kubernetes.redeem-join";
    pub const JOIN_NODE: &str = "opute.capability.kubernetes.join-node";
    pub const ENSURE_HA_ENDPOINT: &str = "opute.capability.kubernetes.ensure-ha-endpoint";
    pub const REMOVE_NODE: &str = "opute.capability.kubernetes.remove-node";
    pub const RECOVER_QUORUM: &str = "opute.capability.kubernetes.recover-quorum";
    pub const EXEC_COMMAND: &str = "opute.capability.kubernetes.exec-command";
    pub const INSPECT_GUEST_STORAGE: &str = "opute.capability.kubernetes.inspect-guest-storage";
    pub const PRUNE_UNUSED_IMAGES: &str = "opute.capability.kubernetes.prune-unused-images";
    pub const GARBAGE_COLLECT_REGISTRY: &str = "opute.capability.kubernetes.garbage-collect-registry";
    pub const TRIM_GUEST_STORAGE: &str = "opute.capability.kubernetes.trim-guest-storage";
}

/// A handler's result: a value for `structured`, or a handler error.
type Outcome = Result<J, String>;

pub fn main() -> ! {
    super::run(server(), "4320", |port| format!("Opute K3s provider listening on :{port}/mcp"))
}

pub fn server() -> Server {
    let manifest_value = serde_json::to_value(manifest()).expect("manifest");
    let mut tools = vec![Tool {
        name: "opute.provider.get_install_manifest".into(),
        description: "Read the K3s provider installation manifest".into(),
        input_schema: json!({"type": "object"}),
        output_schema: Some(json!({"type": "object"})),
        handler: Box::new(move |_| structured(manifest_value.clone())),
    }];
    for operation in operations() {
        let id = operation.id.clone();
        tools.push(Tool {
            name: operation.id.clone(),
            description: "K3s Kubernetes provider operation".into(),
            input_schema: J::Object(operation.input_schema.clone().unwrap_or_default()),
            output_schema: operation.output_schema.clone().map(J::Object),
            handler: Box::new(move |call: &Call| {
                // json.Unmarshal into map[string]any, its error unwrapped.
                let args = match Args::decode(call.arguments, "") {
                    Ok(args) => args,
                    Err(e) => return ToolCallOutcome::Protocol(0, e, None),
                };
                respond(dispatch(&id, &args))
            }),
        });
    }
    tools.push(Tool {
        name: "opute.provider.teardown".into(),
        description: "Return a generic cleanup plan for the K3s provider generation".into(),
        input_schema: teardown_schema(),
        output_schema: Some(json!({"type": "object"})),
        handler: Box::new(|call: &Call| respond(teardown(call))),
    });
    Server {
        name: "opute-provider-k3s",
        version: VERSION,
        capabilities: json!({"tools": {"listChanged": true}}),
        tools,
        wrap: Some(json!({"name": "opute-provider-k3s", "version": VERSION})),
    }
}

/// `structured(value)`: K3s results carry a fixed text and the value.
fn structured(value: J) -> ToolCallOutcome {
    ToolCallOutcome::Result(json!({
        "content": [{"type": "text", "text": "provider result"}],
        "structuredContent": value,
    }))
}

fn respond(outcome: Outcome) -> ToolCallOutcome {
    match outcome {
        Ok(value) => structured(value),
        Err(e) => ToolCallOutcome::Protocol(0, e, None),
    }
}

// --- the Service Definition ---------------------------------------------------------

fn object(value: J) -> Option<Map<String, J>> {
    match value {
        J::Object(map) => Some(map),
        _ => None,
    }
}

fn manifest() -> InstallManifest {
    InstallManifest {
        schema: "opute-provider-install-manifest.v1".into(),
        provider: ProviderRef { id: "com.opute.k3s".into(), version: VERSION.into() },
        provides: GoSlice(Some(vec![CapabilityRef { id: KUBERNETES.into(), version: 1 }])),
        recipes: GoSlice(Some(vec![RecipeRef {
            id: "com.opute.k3s.managed".into(),
            source: RecipeSource {
                uri: "recipes/kubernetes.yaml".into(),
                revision: "working-tree".into(),
                sha256: "sha256:91f0b596492e3c72fb5eacba4c14c6141e7da584eebfa88bc4ffbc72573c7788".into(),
            },
            mode: "kubernetes".into(),
            inputs: None,
        }])),
        services: GoSlice(Some(vec![ServiceDefinition {
            id: "opute.capability.kubernetes".into(),
            capability_id: KUBERNETES.into(),
            version: 1,
            operations: GoSlice(Some(operations())),
            ..ServiceDefinition::default()
        }])),
        teardown: Some(Operation {
            id: "opute.provider.teardown".into(),
            version: 1,
            input_schema: object(teardown_schema()),
            output_schema: object(json!({"type": "object", "required": ["contractVersion", "plan"]})),
            effect: "destructive".into(),
            resource_kinds: GoSlice(Some(vec!["service".into()])),
            idempotent: true,
            supports_readiness: true,
            task_support: "sync_only".into(),
            resource_cost: Some(ResourceCost { class: "control".into(), ..ResourceCost::default() }),
            ..Operation::default()
        }),
        validation: ValidationRef { capability: KUBERNETES.into(), operation: op::VALIDATE.into() },
        ..InstallManifest::default()
    }
}

/// `providerOperation`: mutations are bridged tasks with a control cost.
fn provider_operation(id: &str, effect: &str, input: J, requires: Vec<ResourceBinding>) -> Operation {
    let read = effect == "read";
    Operation {
        id: id.into(),
        version: 1,
        input_schema: object(input),
        output_schema: object(json!({"type": "object"})),
        effect: effect.into(),
        requires: GoSlice(if requires.is_empty() { None } else { Some(requires) }),
        idempotent: true,
        supports_readiness: !read,
        task_support: if read { "sync_only" } else { "bridged" }.into(),
        resource_cost: (!read).then(|| ResourceCost { class: "control".into(), ..ResourceCost::default() }),
        ..Operation::default()
    }
}

fn binding(resource_type: &str) -> ResourceBinding {
    ResourceBinding {
        argument: "targetUri".into(),
        resource_type: resource_type.into(),
        required: true,
        ..ResourceBinding::default()
    }
}

fn cluster() -> Vec<ResourceBinding> {
    vec![binding("cluster")]
}

fn instance() -> Vec<ResourceBinding> {
    vec![binding("vm"), binding("container")]
}

fn target_schema_with(pattern: &str, properties: J, required: &[&str]) -> J {
    let mut properties = properties.as_object().cloned().unwrap_or_default();
    properties.insert("targetUri".into(), json!({"type": "string", "pattern": pattern}));
    properties.insert(
        "providerInstanceName".into(),
        json!({"type": "string", "minLength": 1, "pattern": "^[A-Za-z0-9][A-Za-z0-9_.-]*$"}),
    );
    properties.insert("instanceType".into(), json!({"type": "string", "enum": ["vm", "container"]}));
    let mut all = vec!["targetUri", "providerInstanceName"];
    all.extend_from_slice(required);
    json!({"type": "object", "required": all, "properties": properties})
}

fn target_schema(properties: J, required: &[&str]) -> J {
    target_schema_with("^cluster:[a-z][a-z0-9-]{0,31}:.+$", properties, required)
}

fn vm_target_schema(properties: J, required: &[&str]) -> J {
    target_schema_with("^(vm|container):[a-z][a-z0-9-]{0,31}:.+$", properties, required)
}

fn resource_fields() -> J {
    json!({"kind": {"type": "string"}, "resourceKind": {"type": "string"}, "resourceName": {"type": "string"}, "namespace": {"type": "string"}})
}

fn membership_expectation_schema() -> J {
    json!({
        "expectedRole": {"type": "string", "enum": ["server"]},
        "expectedServers": {"type": "integer", "minimum": 1, "maximum": 9},
        "requireEmbeddedEtcd": {"type": "boolean"},
        "endpoint": {"type": "string", "minLength": 1, "maxLength": 512},
    })
}

fn operations() -> Vec<Operation> {
    let p = provider_operation;
    let heavy = |id: &str, input: J| {
        let mut operation = p(id, "destructive", input, cluster());
        operation.resource_cost = Some(ResourceCost {
            class: "heavy".into(),
            cpu_cores: 2.0,
            memory_bytes: 2_147_483_648,
            tasks: 8,
            ..ResourceCost::default()
        });
        operation
    };
    let string_list = || json!({"type": "array", "items": {"type": "string", "minLength": 1}});
    vec![
        p(op::VALIDATE, "read", json!({"type": "object"}), vec![]),
        p(op::PROVISION, "mutation", vm_target_schema(json!({
            "version": {"type": "string", "minLength": 1, "maxLength": 64},
            "clusterInit": {"type": "boolean", "const": true},
            "nodeIp": {"type": "string", "format": "ipv4"},
            "advertiseAddress": {"type": "string", "format": "ipv4"},
            "flannelIface": {"type": "string", "minLength": 1, "maxLength": 15},
            "tlsSan": {"type": "string", "format": "ipv4"},
        }), &["version"]), instance()),
        p(op::STATUS, "read", target_schema(json!({}), &[]), cluster()),
        p(op::CONFIGURE_REGISTRY, "mutation", target_schema(json!({
            "endpoint": {"type": "string", "minLength": 1, "maxLength": 512},
            "registry": {"type": "string", "minLength": 1, "maxLength": 256},
            "insecure": {"type": "boolean"},
        }), &["endpoint"]), cluster()),
        p(op::RESTART, "mutation", target_schema(json!({}), &[]), cluster()),
        p(op::REMOVE, "destructive", target_schema(json!({}), &[]), cluster()),
        p(op::APPLY_MANIFEST, "mutation", target_schema(json!({"manifest": {"type": "string", "minLength": 1, "maxLength": MAX_MANIFEST_BYTES}}), &["manifest"]), cluster()),
        p(op::PUT_SECRET, "mutation", target_schema(json!({
            "namespace": {"type": "string"},
            "name": {"type": "string"},
            "data": {"type": "object", "writeOnly": true, "additionalProperties": {"type": "string"}},
        }), &["namespace", "name", "data"]), cluster()),
        p(op::GET_RESOURCE, "read", target_schema(resource_fields(), &["kind", "resourceName"]), cluster()),
        p(op::DELETE_RESOURCE, "destructive", target_schema(resource_fields(), &["kind", "resourceName"]), cluster()),
        p(op::GET_RESOURCE_STATUS, "read", target_schema(resource_fields(), &["resourceKind", "resourceName"]), cluster()),
        p(op::LIST_EVENTS, "read", target_schema(json!({
            "namespace": {"type": "string"},
            "limit": {"type": "integer", "minimum": 1, "maximum": 200},
        }), &[]), cluster()),
        p(op::LIST_CLUSTERS, "read", json!({"type": "object", "properties": {"source": {"type": "string"}}}), vec![]),
        p(op::GET_CLUSTER_INFO, "read", target_schema(json!({}), &[]), cluster()),
        p(op::INSPECT_MEMBERSHIP, "read", target_schema(membership_expectation_schema(), &[]), cluster()),
        p(op::PREPARE_HA, "mutation", target_schema(membership_expectation_schema(), &[]), cluster()),
        p(op::PREPARE_JOIN, "mutation", target_schema(json!({
            "destinationHostId": {"type": "string", "minLength": 1},
            "destinationPublicKey": {"type": "string", "minLength": 1, "writeOnly": true},
            "role": {"type": "string", "enum": ["server"]},
            "expiresInMs": {"type": "integer", "minimum": 1000, "maximum": 86_400_000},
        }), &["destinationHostId"]), cluster()),
        p(op::GET_JOIN_RECEIVER_KEY, "read", vm_target_schema(json!({}), &[]), instance()),
        p(op::REDEEM_JOIN, "mutation", vm_target_schema(json!({
            "redemptionRef": {"type": "string", "minLength": 1, "writeOnly": true},
            "destinationHostId": {"type": "string", "minLength": 1},
            "sealedJoinMaterial": {"type": "string", "minLength": 1, "writeOnly": true},
            "sourceTargetUri": {"type": "string", "minLength": 1},
        }), &["redemptionRef"]), instance()),
        p(op::JOIN_NODE, "mutation", vm_target_schema(json!({
            "redemptionRef": {"type": "string", "minLength": 1, "writeOnly": true},
            "serverAddress": {"type": "string", "minLength": 1},
            "joinToken": {"type": "string", "minLength": 1, "writeOnly": true},
            "version": {"type": "string", "minLength": 1, "maxLength": 64},
            "clusterInit": {"type": "boolean", "const": false},
            "role": {"type": "string", "enum": ["server"]},
            "nodeIp": {"type": "string", "format": "ipv4"},
            "advertiseAddress": {"type": "string", "format": "ipv4"},
            "flannelIface": {"type": "string", "minLength": 1, "maxLength": 15},
            "tlsSan": {"type": "string", "format": "ipv4"},
        }), &["redemptionRef", "serverAddress", "clusterInit", "role"]), instance()),
        p(op::ENSURE_HA_ENDPOINT, "mutation", target_schema(json!({"endpoint": {"type": "string", "minLength": 1, "maxLength": 512}}), &["endpoint"]), cluster()),
        p(op::REMOVE_NODE, "destructive", target_schema(json!({
            "nodeName": {"type": "string", "minLength": 1},
            "role": {"type": "string", "enum": ["server"]},
        }), &["nodeName", "role"]), cluster()),
        p(op::RECOVER_QUORUM, "destructive", target_schema(json!({"acknowledgeSingleMemberReset": {"type": "boolean", "const": true}}), &["acknowledgeSingleMemberReset"]), cluster()),
        p(op::EXEC_COMMAND, "read", target_schema(json!({
            "kubectlArgs": {"type": "array", "minItems": 1, "items": {"type": "string", "minLength": 1}},
            "stdin": {"type": "string", "writeOnly": true},
        }), &["kubectlArgs"]), cluster()),
        p(op::INSPECT_GUEST_STORAGE, "read", target_schema(json!({"includeRegistry": {"type": "boolean"}, "extraKeepTags": string_list()}), &[]), cluster()),
        heavy(op::PRUNE_UNUSED_IMAGES, target_schema(json!({
            "dryRun": {"type": "boolean"},
            "minAgeSeconds": {"type": "integer", "minimum": 3600},
            "extraKeepTags": string_list(),
        }), &[])),
        heavy(op::GARBAGE_COLLECT_REGISTRY, target_schema(json!({
            "dryRun": {"type": "boolean"},
            "includeRegistry": {"type": "boolean"},
            "extraKeepTags": string_list(),
            "registryNamespace": {"type": "string", "minLength": 1},
            "registryName": {"type": "string", "minLength": 1},
        }), &[])),
        p(op::TRIM_GUEST_STORAGE, "mutation", target_schema(json!({}), &[]), cluster()),
    ]
}

fn teardown_schema() -> J {
    json!({"type": "object", "required": ["inputs"], "properties": {"inputs": {"type": "object", "properties": {
        "phase": {"type": "string", "enum": ["prepare", "finalize"]},
        "serviceName": {"type": "string"},
        "serviceFile": {"type": "string"},
        "scope": {"type": "string", "enum": ["user", "system"]},
    }}}})
}

/// `k3sServiceNamePattern`: `^[A-Za-z0-9_.@:-]+\.service$`.
fn service_name_ok(name: &str) -> bool {
    name.strip_suffix(".service").is_some_and(|stem| {
        !stem.is_empty()
            && stem
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'@' | b':' | b'-'))
    })
}

fn teardown(call: &Call) -> Outcome {
    let (phase, inputs) = teardown_input(call.arguments)?;
    if first_non_empty(&[&phase, &string_input(&inputs, "phase", "")]) == "finalize" {
        return Ok(json!({"completed": true}));
    }
    let service_name = first_non_empty(&[&string_input(&inputs, "serviceName", ""), "opute-provider-k3s.service"]);
    let scope = first_non_empty(&[&string_input(&inputs, "scope", ""), "user"]);
    if scope != "user" && scope != "system" {
        return Err(format!("unsupported host service scope {}", quote(&scope)));
    }
    let default_file = if scope == "system" {
        format!("/etc/systemd/system/{service_name}")
    } else {
        format!("~/.config/systemd/user/{service_name}")
    };
    let service_file = first_non_empty(&[&string_input(&inputs, "serviceFile", ""), &default_file]);
    if !service_name_ok(&service_name) {
        return Err("serviceName must name a .service unit".into());
    }
    let file_name = service_file.rsplit_once('/').map_or(service_file.as_str(), |(_, n)| n);
    if !service_name_ok(file_name) {
        return Err("K3s service file must name a .service unit".into());
    }
    if scope == "system" && !service_file.strip_prefix("/etc/systemd/system/").is_some_and(service_name_ok) {
        return Err("system-scoped K3s service file must be one .service unit beneath /etc/systemd/system".into());
    }
    let service_uri = first_non_empty(&[
        &string_input(&inputs, "serviceUri", ""),
        &format!("host-service:local:{scope}/{service_name}"),
    ]);
    let plan_id = "com.opute.k3s.teardown";
    // Teardown is two phase: the provider stays reachable for its finalize
    // callback, so the plan only records the declared service identity.
    Ok(json!({
        "contractVersion": "host-plan.v1",
        "plan": {
            "contractVersion": "host-plan.v1",
            "planId": plan_id,
            "generation": 1,
            "idempotencyKey": format!("{plan_id}-{service_name}-{service_file}"),
            "nodes": [{"id": "inspect-service", "action": {"tool": "inspect_host_service", "args": {"uri": service_uri, "scope": scope}}}],
        },
    }))
}

// --- dispatch ----------------------------------------------------------------------------------

fn dispatch(operation: &str, args: &Args) -> Outcome {
    let guarded = |check: fn(&Args) -> Result<(), String>, f: fn(&Args) -> Outcome| {
        check(args)?;
        f(args)
    };
    match operation {
        op::VALIDATE => Ok(json!({"contractVersion": KUBERNETES, "ready": true})),
        op::PROVISION => guarded(require_provision_target, provision),
        op::STATUS | op::GET_CLUSTER_INFO => guarded(require_target, get_cluster_info),
        op::CONFIGURE_REGISTRY => guarded(require_target, configure_registry),
        op::RESTART => guarded(require_target, restart),
        op::REMOVE => guarded(require_target, remove),
        op::APPLY_MANIFEST => guarded(require_target, apply_manifest),
        op::PUT_SECRET => guarded(require_target, put_secret),
        op::GET_RESOURCE => guarded(require_target, get_resource),
        op::DELETE_RESOURCE => guarded(require_target, delete_resource),
        op::GET_RESOURCE_STATUS => guarded(require_target, get_status),
        op::LIST_EVENTS => guarded(require_target, list_events),
        op::LIST_CLUSTERS => list_clusters(args),
        op::INSPECT_MEMBERSHIP => guarded(require_target, membership::inspect_membership),
        op::PREPARE_HA => guarded(require_target, membership::prepare_ha),
        op::PREPARE_JOIN => guarded(require_target, membership::prepare_join),
        op::GET_JOIN_RECEIVER_KEY => guarded(require_provision_target, membership::get_join_receiver_key),
        op::REDEEM_JOIN => guarded(require_provision_target, membership::redeem_join),
        op::JOIN_NODE => guarded(require_provision_target, membership::join_node),
        op::ENSURE_HA_ENDPOINT => guarded(require_target, membership::ensure_ha_endpoint),
        op::REMOVE_NODE => guarded(require_target, membership::remove_node),
        op::RECOVER_QUORUM => guarded(require_target, membership::recover_quorum),
        op::EXEC_COMMAND => guarded(require_target, exec_command),
        op::INSPECT_GUEST_STORAGE => guarded(require_target, storage::inspect_guest_storage),
        op::PRUNE_UNUSED_IMAGES => guarded(require_target, storage::prune_unused_images),
        op::GARBAGE_COLLECT_REGISTRY => guarded(require_target, storage::garbage_collect_registry),
        op::TRIM_GUEST_STORAGE => guarded(require_target, storage::trim_guest_storage),
        _ => Err(format!("unknown K3s provider operation {}", quote(operation))),
    }
}

fn s(args: &Args, key: &str) -> String {
    args.string(key, "")
}

fn has_unsafe(value: &str, set: &str) -> bool {
    value.chars().any(|c| set.contains(c))
}

const UNSAFE_VERSION: &str = "\0\r\n ';&|`$()";

fn provision(args: &Args) -> Outcome {
    let instance = s(args, "providerInstanceName");
    let version = first_non_empty(&[&s(args, "version"), DEFAULT_K3S_VERSION]);
    if has_unsafe(&version, UNSAFE_VERSION) {
        return Err("version contains unsafe characters".into());
    }
    if s(args, "instanceType") == "container" {
        configure_container(&instance)?;
    }
    if let Err(e) = run_command(&["start", &instance], None) {
        if !e.to_lowercase().contains("already running") {
            return Err(format!("start Kubernetes target: {e}"));
        }
    }
    let finish = |mut object: J| {
        object["provisioned"] = J::Bool(true);
        object["version"] = J::from(version.as_str());
        Ok(object)
    };
    if run_command(&["exec", &instance, "--", "test", "-x", "/usr/local/bin/k3s"], None).is_ok() {
        let result = wait_for_cluster_info(args).map_err(|e| format!("observe existing Kubernetes runtime: {e}"))?;
        return finish(result);
    }
    let install_exec = server_install_exec(args, args.bool("clusterInit", false))?;
    bootstrap::install_k3s_server(&instance, &version, &install_exec, "", "")
        .map_err(|e| format!("provision Kubernetes runtime: {e}"))?;
    let result = wait_for_cluster_info(args).map_err(|e| format!("verify Kubernetes runtime: {e}"))?;
    finish(result)
}

/// K3s in a system container needs the container's user namespace: an
/// unprivileged, nested container.
fn configure_container(instance: &str) -> Result<(), String> {
    let privileged = run_command(&["config", "get", instance, "security.privileged"], None)
        .map_err(|e| format!("inspect K3s container privilege: {e}"))?;
    let nesting = run_command(&["config", "get", instance, "security.nesting"], None)
        .map_err(|e| format!("inspect K3s container nesting: {e}"))?;
    let text = |b: &[u8]| String::from_utf8_lossy(b).trim().to_string();
    if text(&privileged).eq_ignore_ascii_case("false") && text(&nesting).eq_ignore_ascii_case("true") {
        return Ok(());
    }
    if let Err(e) = run_command(&["stop", instance], None) {
        if !e.to_lowercase().contains("not running") {
            return Err(format!("stop K3s container before privilege configuration: {e}"));
        }
    }
    run_command(&["config", "set", instance, "security.nesting=true", "security.privileged=false"], None)
        .map_err(|e| format!("configure unprivileged nested K3s container: {e}"))?;
    Ok(())
}

/// `waitForClusterInfo`: the cluster read on a two-second ticker until a node
/// is ready or three minutes pass.
fn wait_for_cluster_info(args: &Args) -> Outcome {
    let start = Instant::now();
    let mut ticks = 0u32;
    loop {
        let last_err = match get_cluster_info(args) {
            Ok(result) if result.get("ready").and_then(J::as_bool) == Some(true) => return Ok(result),
            Ok(_) => "cluster has no ready nodes".to_string(),
            Err(e) => e,
        };
        ticks += 1;
        let next = PROVISION_READINESS_POLL * ticks;
        if start.elapsed() >= PROVISION_READINESS_TIMEOUT {
            return Err(format!("timed out waiting for Kubernetes readiness: {last_err}"));
        }
        let wait = next.min(PROVISION_READINESS_TIMEOUT).saturating_sub(start.elapsed());
        std::thread::sleep(wait);
        if start.elapsed() >= PROVISION_READINESS_TIMEOUT {
            return Err(format!("timed out waiting for Kubernetes readiness: {last_err}"));
        }
    }
}

fn configure_registry(args: &Args) -> Outcome {
    let instance = s(args, "providerInstanceName");
    let endpoint = s(args, "endpoint").trim_end_matches('/').to_string();
    let invalid = || "endpoint must be an absolute http or https URL".to_string();
    let parsed = crate::mcpprobe::GoUrl::parse(&endpoint).ok_or_else(invalid)?;
    let host = String::from_utf8_lossy(&parsed.host).into_owned();
    if (parsed.scheme != "http" && parsed.scheme != "https") || host.is_empty() || has_unsafe(&endpoint, "\0\r\n") {
        return Err(invalid());
    }
    let registry = first_non_empty(&[&s(args, "registry"), &host]);
    if has_unsafe(&registry, UNSAFE_VERSION) {
        return Err("registry contains unsafe characters".into());
    }
    let protocol = if args.get("insecure").and_then(J::as_bool) == Some(true) { "http" } else { "https" };
    let config = format!("mirrors:\n  {registry}:\n    endpoint:\n      - {protocol}://{host}\nconfigs: {{}}\n");
    use base64::Engine as _;
    let encoded = base64::engine::general_purpose::STANDARD.encode(config);
    let write = format!(
        "mkdir -p /etc/rancher/k3s; printf '%s' {} | base64 -d > /etc/rancher/k3s/registries.yaml",
        shell_quote(&encoded)
    );
    run_command(&["exec", &instance, "--", "bash", "-lc", &write], None)
        .map_err(|e| format!("configure Kubernetes registry: {e}"))?;
    run_command(&["exec", &instance, "--", "systemctl", "restart", "k3s"], None)
        .map_err(|e| format!("restart Kubernetes runtime after registry configuration: {e}"))?;
    let mut object = wait_for_cluster_info(args)
        .map_err(|e| format!("verify Kubernetes runtime after registry configuration: {e}"))?;
    object["targetUri"] = J::from(s(args, "targetUri"));
    object["registry"] = J::from(registry);
    object["endpoint"] = J::from(format!("{protocol}://{host}"));
    object["configured"] = J::Bool(true);
    Ok(object)
}

fn restart(args: &Args) -> Outcome {
    let instance = s(args, "providerInstanceName");
    run_command(&["exec", &instance, "--", "systemctl", "restart", "k3s"], None)
        .map_err(|e| format!("restart Kubernetes runtime: {e}"))?;
    // A restart is not readiness: poll the cluster read before reporting it.
    let mut object = wait_for_cluster_info(args).map_err(|e| format!("verify Kubernetes runtime after restart: {e}"))?;
    object["restarted"] = J::Bool(true);
    Ok(object)
}

fn remove(args: &Args) -> Outcome {
    let instance = s(args, "providerInstanceName");
    run_command(&["exec", &instance, "--", "bash", "-lc", "/usr/local/bin/k3s-uninstall.sh"], None)
        .map_err(|e| format!("remove Kubernetes runtime: {e}"))?;
    Ok(json!({"targetUri": s(args, "targetUri"), "removed": true}))
}

fn apply_manifest(args: &Args) -> Outcome {
    let manifest = s(args, "manifest");
    if manifest.is_empty() {
        return Err("manifest is required".into());
    }
    if manifest.len() > MAX_MANIFEST_BYTES {
        return Err("manifest exceeds the 4 MiB limit".into());
    }
    run_kubectl(args, &["apply", "-f", "-"], Some(manifest.replace("__OPUTE_DOLLAR__", "$").as_bytes()))?;
    Ok(json!({"targetUri": s(args, "targetUri"), "applied": true}))
}

fn delete_resource(args: &Args) -> Outcome {
    let (kind, name, namespace) = resource_arguments(args)?;
    let mut command = vec!["delete", kind.as_str(), name.as_str()];
    if !namespace.is_empty() {
        command.extend(["-n", namespace.as_str()]);
    }
    command.push("--ignore-not-found=true");
    run_kubectl(args, &command, None)?;
    Ok(json!({"targetUri": s(args, "targetUri"), "kind": kind, "resourceName": name, "namespace": namespace, "deleted": true}))
}

fn exec_command(args: &Args) -> Outcome {
    let kubectl_args = string_slice_argument(args, "kubectlArgs")?;
    let refs: Vec<&str> = kubectl_args.iter().map(String::as_str).collect();
    let stdout = run_kubectl(args, &refs, Some(s(args, "stdin").as_bytes()))?;
    Ok(json!({"targetUri": s(args, "targetUri"), "stdout": String::from_utf8_lossy(&stdout), "exitCode": 0}))
}

/// `k3sServerInstallExec`: the server flags the installer runs K3s with.
fn server_install_exec(args: &Args, cluster_init: bool) -> Result<String, String> {
    let mut flags = vec!["server".to_string()];
    if cluster_init {
        flags.push("--cluster-init".into());
    }
    for (name, flag) in [("nodeIp", "--node-ip"), ("advertiseAddress", "--advertise-address"), ("tlsSan", "--tls-san")] {
        let value = s(args, name);
        if value.is_empty() {
            continue;
        }
        let v4 = match value.parse::<std::net::IpAddr>() {
            Ok(std::net::IpAddr::V4(v4)) => Some(v4),
            Ok(std::net::IpAddr::V6(v6)) => v6.to_ipv4_mapped(),
            Err(_) => None,
        };
        let Some(v4) = v4 else {
            return Err(format!("{name} must be an IPv4 address"));
        };
        flags.push(flag.into());
        flags.push(v4.to_string());
    }
    let iface = s(args, "flannelIface");
    if !iface.is_empty() {
        if iface.len() > 15 || has_unsafe(&iface, "\0\r\n \t;&|`$()") {
            return Err("flannelIface contains unsafe characters".into());
        }
        flags.push("--flannel-iface".into());
        flags.push(iface);
    }
    if s(args, "instanceType") == "container" {
        // A WSL-backed container exposes read-only host sysctls; kubelet's
        // namespace-aware mode tolerates them.
        flags.push("--kubelet-arg=feature-gates=KubeletInUserNamespace=true".into());
    }
    Ok(flags.join(" "))
}

fn put_secret(args: &Args) -> Outcome {
    let namespace = s(args, "namespace");
    let name = s(args, "name");
    if namespace.is_empty() || name.is_empty() {
        return Err("namespace and name are required".into());
    }
    let data = args.get("data").and_then(J::as_object).filter(|m| !m.is_empty());
    let Some(data) = data else {
        return Err("data is required".into());
    };
    use base64::Engine as _;
    let mut encoded = Map::new();
    for (key, raw) in data {
        match raw.as_str() {
            Some(value) if !key.trim().is_empty() => {
                encoded.insert(key.clone(), J::from(base64::engine::general_purpose::STANDARD.encode(value)));
            }
            _ => return Err("secret data must contain string values".into()),
        }
    }
    let manifest = json!({"apiVersion": "v1", "kind": "Secret", "metadata": {"name": name, "namespace": namespace}, "type": "Opaque", "data": encoded});
    let body = crate::gojson::encode_to_string(&manifest);
    run_kubectl(args, &["apply", "-f", "-"], Some(body.as_bytes()))?;
    Ok(json!({"targetUri": s(args, "targetUri"), "namespace": namespace, "name": name, "configured": true}))
}

fn get_resource(args: &Args) -> Outcome {
    let (kind, name, namespace) = resource_arguments(args)?;
    let mut command = vec!["get", kind.as_str(), name.as_str(), "-o", "json"];
    if !namespace.is_empty() {
        command.extend(["-n", namespace.as_str()]);
    }
    let raw = run_kubectl(args, &command, None)?;
    let object = decode::unmarshal(&raw, &decode::Shape::MapAny)
        .map_err(|e| format!("invalid Kubernetes resource response: {e}"))?;
    let text = String::from_utf8_lossy(&raw).into_owned();
    Ok(json!({
        "targetUri": s(args, "targetUri"), "kind": kind, "resourceName": name, "namespace": namespace,
        "resource": object, "json": text, "yaml": text,
    }))
}

fn get_status(args: &Args) -> Outcome {
    let mut object = match get_resource(args) {
        Ok(object) => object,
        Err(e) if e.to_lowercase().contains("not found") => {
            return Ok(json!({"targetUri": s(args, "targetUri"), "status": "missing", "message": e}));
        }
        Err(e) => return Err(e),
    };
    let resource = object.get("resource").cloned().unwrap_or(J::Null);
    let mut status = "pending";
    match first_non_empty(&[&s(args, "kind"), &s(args, "resourceKind")]).to_lowercase().as_str() {
        "service" => status = "ready",
        "pod" => {
            if matches!(nested_string(&resource, &["status", "phase"]).as_deref(), Some("Running" | "Succeeded")) {
                status = "ready";
            }
        }
        "deployment" => {
            let mut desired = nested_number(&resource, &["spec", "replicas"]);
            let ready = nested_number(&resource, &["status", "readyReplicas"]);
            if desired == 0.0 {
                desired = 1.0;
            }
            if ready >= desired {
                status = "ready";
            }
        }
        "persistentvolumeclaim" | "pvc" => {
            if nested_string(&resource, &["status", "phase"]).as_deref() == Some("Bound") {
                status = "ready";
            }
        }
        _ => {}
    }
    object["status"] = J::from(status);
    Ok(object)
}

fn list_events(args: &Args) -> Outcome {
    let mut limit = 50i64;
    if let Some(raw) = args.get("limit").and_then(J::as_f64) {
        if raw as i64 > 0 {
            limit = raw as i64;
        }
    }
    limit = limit.min(200);
    let namespace = s(args, "namespace");
    let mut command = vec!["get", "events", "-o", "json"];
    if !namespace.is_empty() {
        command.extend(["-n", namespace.as_str()]);
    }
    let raw = run_kubectl(args, &command, None)?;
    let shape = decode::anon(vec![decode::field("Items", "items", decode::slice(decode::Shape::MapAny))]);
    let list = decode::unmarshal(&raw, &shape).map_err(|e| format!("invalid Kubernetes events response: {e}"))?;
    let mut items = decode::items(&list, "items").to_vec();
    let timestamp = |item: &J| {
        ["eventTime", "lastTimestamp", "firstTimestamp"]
            .iter()
            .find_map(|k| item.get(*k).and_then(J::as_str).filter(|v| !v.is_empty()))
            .unwrap_or("")
            .to_string()
    };
    items.sort_by_key(|item| std::cmp::Reverse(timestamp(item)));
    items.truncate(limit as usize);
    let events: Vec<J> = items
        .iter()
        .map(|item| {
            let get = |k: &str| item.get(k).cloned().unwrap_or(J::Null);
            let metadata = get("metadata");
            let meta = |k: &str| metadata.as_object().and_then(|m| m.get(k)).cloned().unwrap_or(J::Null);
            json!({
                "type": get("type"), "reason": get("reason"), "message": get("message"), "count": get("count"),
                "firstTimestamp": get("firstTimestamp"), "lastTimestamp": get("lastTimestamp"), "eventTime": get("eventTime"),
                "source": get("source"), "involvedObject": get("involvedObject"), "name": meta("name"), "namespace": meta("namespace"),
            })
        })
        .collect();
    Ok(json!({"targetUri": s(args, "targetUri"), "namespace": namespace, "limit": limit, "events": events}))
}

fn list_clusters(_args: &Args) -> Outcome {
    let raw = run_command(&["list", "--format", "json"], None)?;
    // The whole inventory decodes before any guest is asked anything.
    let inventory = decode::unmarshal(&raw, &decode::slice(decode::Shape::MapAny))
        .map_err(|e| format!("invalid Incus inventory response: {e}"))?;
    let instances = inventory.as_array().cloned().unwrap_or_default();
    let mut clusters = Vec::new();
    for instance in &instances {
        let name = instance.get("name").and_then(J::as_str).unwrap_or("");
        if name.is_empty() {
            continue;
        }
        let mut instance_type = instance.get("type").and_then(J::as_str).unwrap_or("").to_string();
        if instance_type == "virtual-machine" {
            instance_type = "vm".into();
        }
        if instance_type != "vm" && instance_type != "container" {
            continue;
        }
        let Ok(version_output) = run_command(&["exec", name, "--", "k3s", "--version"], None) else {
            continue;
        };
        let mut cluster = json!({
            "name": name,
            "status": instance.get("status").cloned().unwrap_or(J::Null),
            "instanceType": instance_type,
            "provider": "k3s",
            "version": parse_k3s_version(&String::from_utf8_lossy(&version_output)),
        });
        if let Ok(nodes_output) = run_command(&["exec", name, "--", "k3s", "kubectl", "get", "nodes", "-o", "json"], None) {
            let (nodes, ready) = membership::parse_cluster_node_snapshot(&nodes_output)
                .map_err(|e| format!("parse Kubernetes nodes for cluster {}: {e}", quote(name)))?;
            cluster["nodeCount"] = J::from(nodes.len());
            cluster["nodes"] = J::Array(nodes);
            cluster["readyNodeCount"] = J::from(ready);
            cluster["ready"] = J::Bool(ready > 0);
        }
        clusters.push(cluster);
    }
    Ok(json!({"clusters": clusters}))
}

fn get_cluster_info(args: &Args) -> Outcome {
    let instance = s(args, "providerInstanceName");
    let version_output = run_command(&["exec", &instance, "--", "k3s", "--version"], None)?;
    let nodes_output = run_command(&["exec", &instance, "--", "k3s", "kubectl", "get", "nodes", "-o", "json"], None)?;
    let (nodes, ready) =
        membership::parse_cluster_node_snapshot(&nodes_output).map_err(|e| format!("parse Kubernetes nodes: {e}"))?;
    Ok(json!({
        "targetUri": s(args, "targetUri"),
        "version": parse_k3s_version(&String::from_utf8_lossy(&version_output)),
        "nodeCount": nodes.len(),
        "nodes": nodes,
        "readyNodeCount": ready,
        "ready": ready > 0,
    }))
}

/// `parseK3sVersion`: the release token of `k3s --version`'s first line.
fn parse_k3s_version(output: &str) -> String {
    let mut first = output.split('\n').next().unwrap_or("").trim().to_string();
    const PREFIX: &str = "k3s version ";
    if first.to_lowercase().starts_with(PREFIX) {
        // Go slices the original bytes at the prefix's length.
        first = String::from_utf8_lossy(first.as_bytes().get(PREFIX.len()..).unwrap_or_default()).trim().to_string();
    }
    first.split_whitespace().next().unwrap_or("").to_string()
}

fn run_kubectl(args: &Args, kubectl_args: &[&str], input: Option<&[u8]>) -> Result<Vec<u8>, String> {
    require_target(args)?;
    let instance = s(args, "providerInstanceName");
    let mut command = vec!["exec", instance.as_str(), "--", "k3s", "kubectl"];
    command.extend_from_slice(kubectl_args);
    run_command(&command, input)
}

/// Go's `syscall.Signal.String()` for the signals a command dies of.
fn signal_name(signal: i32) -> String {
    match signal {
        1 => "hangup".into(),
        2 => "interrupt".into(),
        3 => "quit".into(),
        6 => "aborted".into(),
        9 => "killed".into(),
        11 => "segmentation fault".into(),
        13 => "broken pipe".into(),
        15 => "terminated".into(),
        n => format!("signal {n}"),
    }
}

/// `runCommand`: `incus <args>`, stdin when given, stdout on success.
fn run_command(args: &[&str], input: Option<&[u8]>) -> Result<Vec<u8>, String> {
    run_program("incus", args, input, None).map_err(|(err, message)| format!("incus K3s command failed: {err}: {message}"))
}

/// `exec.CommandContext(...).Run()` with captured output: the exec error
/// (`exit status N`, `signal: killed`, a lookup failure) and the trimmed
/// stderr (else stdout) on failure. A deadline kills the process.
fn run_program(program: &str, args: &[&str], input: Option<&[u8]>, deadline: Option<Duration>) -> Result<Vec<u8>, (String, String)> {
    let output = spawn_and_wait(program, args, input, deadline, false)?;
    Ok(output.0)
}

/// The process's stdout and stderr (or both combined), or its exec error and
/// trimmed diagnostic text.
fn spawn_and_wait(
    program: &str,
    args: &[&str],
    input: Option<&[u8]>,
    deadline: Option<Duration>,
    combined: bool,
) -> Result<(Vec<u8>, Vec<u8>), (String, String)> {
    let mut command = Command::new(program);
    command.args(args).stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() });
    // CombinedOutput: both streams share one pipe, interleaved as written.
    let mut combined_reader = None;
    if combined {
        let (reader, writer) = nix::unistd::pipe().map_err(|e| (e.desc().to_string(), String::new()))?;
        let second = writer.try_clone().map_err(|e| (e.to_string(), String::new()))?;
        command.stdout(Stdio::from(writer)).stderr(Stdio::from(second));
        combined_reader = Some(std::fs::File::from(reader));
    } else {
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err((format!("exec: {}: executable file not found in $PATH", quote(program)), String::new()))
        }
        Err(e) => return Err((format!("fork/exec {program}: {}", crate::goerr::errno_text(&e)), String::new())),
    };
    let writer = input.map(|bytes| {
        let bytes = bytes.to_vec();
        let mut stdin = child.stdin.take().expect("stdin");
        std::thread::spawn(move || {
            let _ = stdin.write_all(&bytes);
        })
    });
    // The parent's copies of the shared pipe's write end close with
    // `command` so the reader sees EOF when the child exits.
    drop(command);
    fn drain<R: Read + Send + 'static>(pipe: Option<R>) -> std::thread::JoinHandle<Vec<u8>> {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_end(&mut buf);
            }
            buf
        })
    }
    let out_reader = match combined_reader {
        Some(reader) => drain(Some(reader)),
        None => drain(child.stdout.take()),
    };
    let err_reader = drain(child.stderr.take());
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(e) => return Err((e.to_string(), String::new())),
        }
        if deadline.is_some_and(|d| start.elapsed() >= d) {
            let _ = child.kill();
            match child.wait() {
                Ok(status) => break status,
                Err(e) => return Err((e.to_string(), String::new())),
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    if let Some(writer) = writer {
        let _ = writer.join();
    }
    let stdout = out_reader.join().unwrap_or_default();
    let stderr = err_reader.join().unwrap_or_default();
    if status.success() {
        return Ok((stdout, stderr));
    }
    use std::os::unix::process::ExitStatusExt as _;
    let err = match (status.code(), status.signal()) {
        (Some(code), _) => format!("exit status {code}"),
        (None, Some(signal)) => format!("signal: {}", signal_name(signal)),
        _ => "exit status -1".into(),
    };
    let message = if combined {
        String::from_utf8_lossy(&stdout).trim().to_string()
    } else {
        let e = String::from_utf8_lossy(&stderr).trim().to_string();
        if e.is_empty() {
            String::from_utf8_lossy(&stdout).trim().to_string()
        } else {
            e
        }
    };
    Err((err, message))
}

fn require_target(args: &Args) -> Result<(), String> {
    if !s(args, "targetUri").starts_with("cluster:") {
        return Err("targetUri must be a canonical cluster URI".into());
    }
    if s(args, "providerInstanceName").is_empty() {
        return Err("providerInstanceName is required".into());
    }
    let kind = s(args, "instanceType");
    if !kind.is_empty() && kind != "vm" && kind != "container" {
        return Err(format!("unsupported instanceType {}", quote(&kind)));
    }
    Ok(())
}

fn require_provision_target(args: &Args) -> Result<(), String> {
    let uri = s(args, "targetUri");
    if !uri.starts_with("vm:") && !uri.starts_with("container:") {
        return Err("targetUri must be a canonical VM or container URI for Kubernetes provisioning".into());
    }
    if s(args, "providerInstanceName").is_empty() {
        return Err("providerInstanceName is required".into());
    }
    let kind = s(args, "instanceType");
    if !kind.is_empty() && kind != "vm" && kind != "container" {
        return Err(format!("unsupported instanceType {}", quote(&kind)));
    }
    Ok(())
}

fn resource_arguments(args: &Args) -> Result<(String, String, String), String> {
    let kind = first_non_empty(&[&s(args, "kind"), &s(args, "resourceKind")]);
    let name = s(args, "resourceName");
    if kind.is_empty() || name.is_empty() {
        return Err("kind and resourceName are required".into());
    }
    Ok((kind, name, s(args, "namespace")))
}

fn nested_value<'a>(object: &'a J, path: &[&str]) -> Option<&'a J> {
    path.iter().try_fold(object, |current, part| current.as_object().and_then(|m| m.get(*part)))
}

fn nested_string(object: &J, path: &[&str]) -> Option<String> {
    nested_value(object, path).and_then(J::as_str).map(str::to_string)
}

fn nested_number(object: &J, path: &[&str]) -> f64 {
    nested_value(object, path).and_then(J::as_f64).unwrap_or(0.0)
}

/// The K3s provider's `shellQuote` (its own, unlike the others').
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\\"'\\\"'"))
}

fn string_slice_argument(args: &Args, key: &str) -> Result<Vec<String>, String> {
    let raw = args.get(key).and_then(J::as_array).filter(|a| !a.is_empty());
    let Some(raw) = raw else {
        return Err(format!("{key} must contain at least one argument"));
    };
    raw.iter()
        .map(|value| match value.as_str() {
            Some(text) if !text.trim().is_empty() && !has_unsafe(text, "\0\r\n") => Ok(text.to_string()),
            _ => Err(format!("{key} must contain non-empty safe strings")),
        })
        .collect()
}

/// `optionalStringSliceInput`: the safe, non-blank strings of a list.
fn optional_string_slice(args: &Args, key: &str) -> Vec<String> {
    args.get(key)
        .and_then(J::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(J::as_str)
                .map(str::trim)
                .filter(|t| !t.is_empty() && !has_unsafe(t, "\0\r\n"))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}
