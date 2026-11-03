//! `opute-provider-cloudflare` (Go `plugins/tunneling/cloudflare`).
//!
//! Service Definitions: tunneling (13 operations, including the legacy
//! aliases), mesh-runtime, mesh-membership and public-ingress, and the
//! teardown operation, as the install manifest declares them. Service
//! Provider: the handlers here, which reach the host only through typed Host
//! Agent callbacks and Cloudflare's API. Consumer: the Host Agent's provider
//! binding.
//!
//! ```text
//!  tools/call -> requestArguments (map[string]any) -> dispatch by operation
//!    tunneling  ensure / probe / remove host tunnel, connectors   (this file)
//!    dedicated  Cloudflare API tunnels, ingress, CNAMEs           (api.rs)
//!    mesh       mesh-runtime records, Mesh nodes, HA endpoints    (mesh.rs)
//!  callbacks -> OPUTE_HOST_AGENT_ENDPOINT (+ bearer) -> host tools
//! ```

mod api;
mod mesh;

use super::support::{first_non_empty, string_input, Args, HostAgent, HttpRequest};
use super::{Call, Server, Tool};
use crate::goerr::quote;
use crate::mcpsdk::ToolCallOutcome;
use crate::provider::definition::{
    CapabilityRef, GoSlice, InstallManifest, Operation, RecipeRef, RecipeSource, ResourceBinding,
    ResourceCost, ServiceDefinition, ValidationRef,
};
use crate::provider::ProviderRef;
use crate::resource::Uri;
use serde_json::{json, Map, Value as J};
use std::time::Duration;

const VERSION: &str = "1.0.1";
const TUNNELING: &str = "opute.capability.tunneling.v1";
const MESH_RUNTIME: &str = "opute.capability.mesh-runtime.v1";
const MESH_MEMBERSHIP: &str = "opute.capability.mesh-membership.v1";
const PUBLIC_INGRESS: &str = "opute.capability.public-ingress.v1";
const NETWORK_OVERLAY: &str = "opute.capability.network-overlay.v1";
const PUBLIC_INGRESS_PROBE: &str = "opute.capability.public-ingress.probe";
const DEFAULT_FORWARDER_IMAGE: &str = "alpine/socat:1.8.0.0";

/// A handler's result: a value for `structured`, or a handler error.
type Outcome = Result<J, String>;

pub fn main() -> ! {
    super::run(server(), "4319", |port| {
        format!("Opute Cloudflare provider listening on :{port}/mcp")
    })
}

pub fn server() -> Server {
    let manifest = manifest();
    let manifest_text = super::go_text(serde_json::to_string(&manifest).expect("manifest"));
    let manifest_value = serde_json::to_value(&manifest).expect("manifest");
    let mut tools = vec![Tool {
        name: "opute.provider.get_install_manifest".into(),
        description: "Read the Cloudflare provider installation manifest".into(),
        input_schema: json!({"type": "object"}),
        output_schema: Some(json!({"type": "object"})),
        handler: Box::new(move |_| {
            super::structured(manifest_text.clone(), manifest_value.clone())
        }),
    }];
    let operations = cloudflare_operations()
        .into_iter()
        .chain(mesh::runtime_operations())
        .chain(mesh::membership_operations())
        .chain(mesh::public_ingress_operations());
    for operation in operations {
        let id = operation.id.clone();
        tools.push(Tool {
            name: operation.id.clone(),
            description: "Cloudflare tunneling provider operation".into(),
            input_schema: J::Object(operation.input_schema.clone().unwrap_or_default()),
            output_schema: operation.output_schema.clone().map(J::Object),
            handler: Box::new(move |call: &Call| {
                let args = match Args::decode(call.arguments, "decode provider arguments: ") {
                    Ok(args) => args,
                    Err(e) => return ToolCallOutcome::Protocol(0, e, None),
                };
                respond(dispatch(&call.delegation(), &id, args))
            }),
        });
    }
    tools.push(Tool {
        name: "opute.provider.teardown".into(),
        description: "Return a generic cleanup plan and finalize Cloudflare API resources".into(),
        input_schema: teardown_schema(),
        output_schema: Some(json!({"type": "object"})),
        handler: Box::new(|call: &Call| respond(teardown(call))),
    });
    Server {
        name: "opute-provider-cloudflare",
        version: VERSION,
        capabilities: json!({"tools": {"listChanged": true}}),
        tools,
        wrap: Some(json!({"name": "opute-provider-cloudflare", "version": VERSION})),
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

// --- the Service Definition ------------------------------------------------------

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
    let capability = |id: &str| CapabilityRef {
        id: id.into(),
        version: 1,
    };
    let service = |id: &str, capability_id: &str, operations: Vec<Operation>| ServiceDefinition {
        id: id.into(),
        capability_id: capability_id.into(),
        version: 1,
        operations: GoSlice(Some(operations)),
        ..ServiceDefinition::default()
    };
    InstallManifest {
        schema: "opute-provider-install-manifest.v1".into(),
        provider: ProviderRef {
            id: "com.opute.cloudflare".into(),
            version: VERSION.into(),
        },
        provides: GoSlice(Some(vec![
            capability(TUNNELING),
            capability(MESH_RUNTIME),
            capability(MESH_MEMBERSHIP),
            capability(PUBLIC_INGRESS),
        ])),
        recipes: GoSlice(Some(vec![
            recipe(
                "com.opute.cloudflare.activate-tunneling",
                "recipes/activate-tunneling.yaml",
                "sha256:6fec9957909058f39bc48bd35becae2e46be55a06b9b2ff4a9928a608772f460",
                "activate",
            ),
            recipe(
                "com.opute.cloudflare.activate",
                "recipes/activate.yaml",
                "sha256:2c9d9ad41f1cc0c836196e6c0e56014c6eab4fde0f1eed2e746eaeb946cad0c4",
                "activate",
            ),
            recipe(
                "com.opute.cloudflare.tunneling",
                "recipes/tunneling.yaml",
                "sha256:2f404972cbe5c463b8fe501973894c241341b2621e5941fad06af1434a958bc7",
                "tunnel",
            ),
            recipe(
                "com.opute.cloudflare.tunneling.managed",
                "recipes/tunneling-managed.yaml",
                "sha256:de45303f69256b664ec2e137f14e98ae3113ceb935c1fb6da34f85b54758fcae",
                "managed",
            ),
            recipe(
                "com.opute.cloudflare.tunneling.public-host",
                "recipes/tunneling-public-host.yaml",
                "sha256:db9d4e1a82d376daeaa1ed2d4448f80e5ad2e6063886395c4de60864a70b840a",
                "public-host",
            ),
        ])),
        services: GoSlice(Some(vec![
            service(
                "opute.capability.tunneling",
                TUNNELING,
                cloudflare_operations(),
            ),
            service(
                "opute.capability.mesh-runtime",
                MESH_RUNTIME,
                mesh::runtime_operations(),
            ),
            service(
                "opute.capability.mesh-membership",
                MESH_MEMBERSHIP,
                mesh::membership_operations(),
            ),
            service(
                "opute.capability.public-ingress",
                PUBLIC_INGRESS,
                mesh::public_ingress_operations(),
            ),
        ])),
        teardown: Some(Operation {
            id: "opute.provider.teardown".into(),
            version: 1,
            input_schema: object(teardown_schema()),
            output_schema: object(
                json!({"type": "object", "required": ["contractVersion", "plan"]}),
            ),
            effect: "destructive".into(),
            resource_kinds: GoSlice(Some(vec!["service".into(), "tunnel".into()])),
            idempotent: true,
            supports_readiness: true,
            task_support: "sync_only".into(),
            resource_cost: Some(ResourceCost {
                class: "control".into(),
                ..ResourceCost::default()
            }),
            ..Operation::default()
        }),
        validation: ValidationRef {
            capability: PUBLIC_INGRESS.into(),
            operation: PUBLIC_INGRESS_PROBE.into(),
        },
        ..InstallManifest::default()
    }
}

/// `providerOperation`: mutating operations are bridged tasks with a typed
/// control-class cost; reads are synchronous.
fn provider_operation(
    id: &str,
    effect: &str,
    input: J,
    output: J,
    resources: &[&str],
    requires: Vec<ResourceBinding>,
) -> Operation {
    let read = effect == "read";
    Operation {
        id: id.into(),
        version: 1,
        input_schema: object(input),
        output_schema: object(output),
        effect: effect.into(),
        resource_kinds: GoSlice(if resources.is_empty() {
            None
        } else {
            Some(resources.iter().map(|r| r.to_string()).collect())
        }),
        requires: GoSlice(if requires.is_empty() {
            None
        } else {
            Some(requires)
        }),
        idempotent: true,
        supports_readiness: !read,
        task_support: if read { "sync_only" } else { "bridged" }.into(),
        resource_cost: (!read).then(|| ResourceCost {
            class: "control".into(),
            ..ResourceCost::default()
        }),
        ..Operation::default()
    }
}

fn cluster_binding() -> Vec<ResourceBinding> {
    vec![ResourceBinding {
        argument: "targetUri".into(),
        resource_type: "cluster".into(),
        required: true,
        ..ResourceBinding::default()
    }]
}

fn cloudflare_operations() -> Vec<Operation> {
    let host_tunnel = ["host", "tunnel"];
    let cluster_tunnel = ["cluster", "tunnel"];
    let op = |id: &str, effect: &str, input: J, output: J, resources: &[&str]| {
        provider_operation(id, effect, input, output, resources, Vec::new())
    };
    let mut ops = vec![
        op(
            "opute.capability.tunneling.validate",
            "read",
            validation_schema(),
            json!({"type": "object"}),
            &[],
        ),
        op(
            "opute.capability.tunneling.ensure-host-tunnel",
            "mutation",
            tunnel_schema(),
            tunnel_output_schema(),
            &host_tunnel,
        ),
        op(
            "opute.capability.tunneling.probe-host-tunnel",
            "read",
            tunnel_schema(),
            tunnel_output_schema(),
            &host_tunnel,
        ),
        op(
            "opute.capability.tunneling.remove-host-tunnel",
            "destructive",
            tunnel_schema(),
            tunnel_output_schema(),
            &host_tunnel,
        ),
        provider_operation(
            "opute.capability.tunneling.install-kubernetes-connector",
            "mutation",
            connector_schema_requiring(&["token", "namespace", "targetUri"]),
            connector_output_schema(),
            &cluster_tunnel,
            cluster_binding(),
        ),
        provider_operation(
            "opute.capability.tunneling.delete-kubernetes-connector",
            "destructive",
            connector_schema_requiring(&["namespace", "targetUri"]),
            connector_output_schema(),
            &cluster_tunnel,
            cluster_binding(),
        ),
        op(
            "ensure_cloudflared_tunnel",
            "mutation",
            tunnel_schema(),
            tunnel_output_schema(),
            &host_tunnel,
        ),
    ];
    for id in ["probe_host_exposure", "get_cloudflare_tunnel_status"] {
        ops.push(op(
            id,
            "read",
            tunnel_schema(),
            tunnel_output_schema(),
            &host_tunnel,
        ));
    }
    for id in [
        "remove_local_llm_cloudflared_tunnel",
        "remove_host_exposure",
    ] {
        ops.push(op(
            id,
            "destructive",
            tunnel_schema(),
            tunnel_output_schema(),
            &host_tunnel,
        ));
    }
    ops.push(op(
        "install_cloudflared_connector",
        "mutation",
        connector_schema_requiring(&["token", "namespace"]),
        connector_output_schema(),
        &cluster_tunnel,
    ));
    ops.push(op(
        "delete_cloudflared_connector",
        "destructive",
        connector_schema_requiring(&["namespace"]),
        connector_output_schema(),
        &cluster_tunnel,
    ));
    ops
}

fn validation_schema() -> J {
    json!({"type": "object", "required": ["bindings"], "properties": {
        "bindings": {"type": "array"},
        "placement": {"type": "string", "enum": ["host", "kubernetes", "container"]},
    }})
}

const CONTAINER_ARTIFACT_NOTE: &str =
    "For container placement, use remotely managed cloudflared 2025.4.0 or later, which supports --token-file.";

fn tunnel_schema() -> J {
    json!({"type": "object", "required": ["bindingId", "localTarget"], "properties": {
        "bindingId": {"type": "string", "minLength": 1},
        "endpoint": {"type": "string", "format": "uri"},
        "hostname": {"type": "string"},
        "hostnames": {"type": "array", "items": {"type": "string", "minLength": 1}},
        "localTarget": {"type": "string"},
        "runToken": {"type": "string", "writeOnly": true},
        "tunnelName": {"type": "string"},
        "tokenFile": {"type": "string"},
        "manageHostConnector": {"type": "boolean"},
        "connector": {"type": "string"},
        "originHostId": {"type": "string", "minLength": 1},
        "placement": {"type": "string", "enum": ["host", "container", "kubernetes"]},
        "targetUri": {"type": "string"},
        "artifactUri": {"type": "string", "description": CONTAINER_ARTIFACT_NOTE},
        "artifactSha256": {"type": "string"},
        "artifactPath": {"type": "string"},
        "serviceName": {"type": "string"},
        "serviceFile": {"type": "string"},
        "scope": {"type": "string", "enum": ["user", "system"]},
    }})
}

fn connector_schema_requiring(required: &[&str]) -> J {
    json!({"type": "object", "required": required, "properties": {
        "token": {"type": "string", "minLength": 1, "writeOnly": true},
        "namespace": {"type": "string", "minLength": 1},
        "name": {"type": "string"},
        "image": {"type": "string", "description": "For Kubernetes placement, use cloudflared 2025.4.0 or later to support --token-file; the default image meets this requirement."},
        "replicas": {"type": "integer", "minimum": 1},
        "targetUri": {"type": "string"},
        "placement": {"type": "string", "enum": ["kubernetes", "container"]},
        "artifactUri": {"type": "string", "description": CONTAINER_ARTIFACT_NOTE},
        "artifactSha256": {"type": "string"},
        "localTargets": local_targets_schema(),
        "forwarderImage": {"type": "string"},
    }})
}

fn local_targets_schema() -> J {
    json!({"type": "array", "items": {"type": "object", "required": ["localPort", "target"], "properties": {
        "localPort": {"type": "integer", "minimum": 1, "maximum": 65535},
        "target": {"type": "string", "minLength": 3, "description": "host:port the connector pod should reach on this loopback port."},
    }}})
}

fn teardown_schema() -> J {
    json!({"type": "object", "required": ["inputs"], "properties": {"inputs": {"type": "object", "properties": {
        "phase": {"type": "string", "enum": ["prepare", "finalize"]},
        "serviceName": {"type": "string"},
        "serviceFile": {"type": "string"},
        "scope": {"type": "string", "enum": ["user", "system"]},
        "tunnelId": {"type": "string", "pattern": "^[A-Za-z0-9_-]+$"},
        "dnsRecordIds": {"type": "array", "items": {"type": "string", "pattern": "^[A-Za-z0-9_-]+$"}},
    }}}})
}

fn tunnel_output_schema() -> J {
    json!({"type": "object", "required": ["contractVersion", "ready", "bindingId", "placement"], "properties": {
        "contractVersion": {"type": "string"},
        "ready": {"type": "boolean"},
        "bindingId": {"type": "string"},
        "placement": {"type": "string"},
        "hostname": {"type": "string"},
        "hostnames": {"type": "array"},
        "tunnelId": {"type": "string"},
        "dnsRecordId": {"type": "string"},
        "deleted": {"type": "boolean"},
        "runToken": {"type": "string", "writeOnly": true},
    }})
}

fn connector_output_schema() -> J {
    json!({"type": "object", "required": ["contractVersion", "ready", "placement"]})
}

// --- dispatch --------------------------------------------------------------------

/// The one call context every callback uses: the request's resource
/// delegation.
struct Ctx {
    delegation: String,
}

fn dispatch(delegation: &str, operation: &str, mut args: Args) -> Outcome {
    let ctx = Ctx {
        delegation: delegation.to_string(),
    };
    if let Some(outcome) = mesh::dispatch(&ctx, operation, &args) {
        return outcome;
    }
    match operation {
        "opute.capability.tunneling.validate" => {
            let bindings = args
                .get("bindings")
                .and_then(J::as_array)
                .cloned()
                .unwrap_or_default();
            if bindings.is_empty() {
                return Err("bindings must contain at least one declared binding".into());
            }
            let placement = placement_input(&args)?;
            Ok(
                json!({"contractVersion": TUNNELING, "ready": true, "bindings": bindings, "placement": placement}),
            )
        }
        "opute.capability.tunneling.ensure-host-tunnel"
        | "ensure_cloudflared_tunnel"
        | "create_cloudflare_tunnel" => ensure_tunnel(&ctx, &mut args),
        "opute.capability.tunneling.probe-host-tunnel"
        | "probe_host_exposure"
        | "get_cloudflare_tunnel_status" => probe_tunnel(&ctx, &args),
        "opute.capability.tunneling.remove-host-tunnel"
        | "remove_local_llm_cloudflared_tunnel"
        | "remove_host_exposure"
        | "delete_cloudflare_tunnel" => remove_tunnel(&ctx, &args),
        "opute.capability.tunneling.install-kubernetes-connector"
        | "install_cloudflared_connector" => install_connector(&ctx, &args),
        "opute.capability.tunneling.delete-kubernetes-connector"
        | "delete_cloudflared_connector" => delete_connector(&ctx, &args),
        _ => Err(format!(
            "unknown Cloudflare provider operation {}",
            quote(operation)
        )),
    }
}

fn ensure_tunnel(ctx: &Ctx, args: &mut Args) -> Outcome {
    let placement = placement_input(args)?;
    // The legacy host alias predates the neutral placement field; its one
    // safe connector value is derived here.
    if placement == "host" && args.string("connector", "").is_empty() {
        args.set("connector", J::from("host"));
    }
    if args.string("bindingId", "").is_empty() || args.string("localTarget", "").is_empty() {
        return Err("bindingId and localTarget are required".into());
    }
    validate_local_target(
        &args.string("localTarget", ""),
        &args.string("connector", ""),
        &args.string("originHostId", ""),
    )?;
    let hostname = args.string("hostname", "");
    // A hostname without a caller-supplied run token is the dedicated-host
    // path: the provider mints the tunnel and its CNAME.
    let dedicated = !hostname.is_empty() && args.string("runToken", "").is_empty();
    let mut provisioned = None;
    if dedicated {
        let result = api::ensure_dedicated_tunnel(args)?;
        args.set("runToken", J::from(result.run_token.clone()));
        provisioned = Some(result);
    }
    let manage_connector = args.bool(
        "manageHostConnector",
        !dedicated || !args.string("endpoint", "").is_empty(),
    );
    let token_file = args.string("tokenFile", "");
    if let (Some(provisioned), false) = (&provisioned, token_file.is_empty()) {
        let client = connect_host_agent(ctx)?;
        client.call_host(
            "ensure_host_file",
            json!({"path": token_file, "content": provisioned.run_token, "mode": 0o600}),
        )?;
    }
    let mut out = json!({
        "contractVersion": TUNNELING,
        "ready": true,
        "bindingId": args.string("bindingId", ""),
        "placement": placement,
        "hostname": hostname,
        "hostnames": hostnames_from_args(args),
    });
    if manage_connector {
        let client = connect_host_agent(ctx)?;
        match placement.as_str() {
            "host" => reconcile_host_tunnel(&client, args)?,
            "container" => reconcile_container_tunnel(&client, args)?,
            _ => {
                // ensureDedicatedTunnel mints runToken; the Kubernetes
                // connector reads token.
                if args.string("token", "").is_empty() {
                    let token = args.string("runToken", "");
                    args.set("token", J::from(token));
                }
                install_kubernetes_connector(&client, args)?;
            }
        }
    }
    if let Some(provisioned) = provisioned {
        out["tunnelId"] = J::from(provisioned.tunnel_id);
        out["dnsRecordId"] = J::from(provisioned.dns_record_id);
        out["runToken"] = J::from(provisioned.run_token);
    }
    Ok(out)
}

fn reconcile_host_tunnel(client: &HostAgent, args: &Args) -> Result<(), String> {
    let scope = host_service_scope(args)?;
    let run_token = args.string("runToken", "");
    if run_token.is_empty() {
        return Err("runToken is required for host tunnel placement".into());
    }
    // Only the public Host Agent onboarding path supplies an endpoint; other
    // host-native consumers keep their established service contract.
    if args.string("endpoint", "").is_empty() {
        return reconcile_legacy_host_tunnel(client, args, &scope, &run_token);
    }
    let hostname = args.string("hostname", "");
    let endpoint = first_non_empty(&[
        &args.string("endpoint", ""),
        &format!("https://{hostname}/mcp"),
    ]);
    let mut call = json!({
        "bindingId": args.string("bindingId", ""),
        "endpoint": endpoint,
        "localTarget": args.string("localTarget", ""),
        "tunnelToken": run_token,
        "scope": scope,
    });
    let origin = args.string("originHostId", "");
    if !origin.is_empty() {
        call["originHostId"] = J::from(origin);
    }
    for key in [
        "artifactUri",
        "artifactSha256",
        "artifactPath",
        "tokenFile",
        "serviceName",
        "serviceFile",
    ] {
        let value = args.string(key, "");
        if !value.is_empty() {
            call[key] = J::from(value);
        }
    }
    client.call_host("ensure_public_mcp_tunnel", call)?;
    Ok(())
}

fn reconcile_legacy_host_tunnel(
    client: &HostAgent,
    args: &Args,
    scope: &str,
    run_token: &str,
) -> Result<(), String> {
    let service_name = first_non_empty(&[
        &args.string("serviceName", ""),
        "opute-cloudflare-tunnel.service",
    ]);
    let service_file = first_non_empty(&[
        &args.string("serviceFile", ""),
        &default_host_service_file(scope, &service_name),
    ]);
    let artifact_path = first_non_empty(&[
        &args.string("artifactPath", ""),
        default_cloudflared_artifact_path(scope),
    ]);
    let artifact_uri = first_non_empty(&[
        &args.string("artifactUri", ""),
        mesh::CLOUDFLARED_ARTIFACT_URL,
    ]);
    let artifact_sha = first_non_empty(&[
        &args.string("artifactSha256", ""),
        mesh::CLOUDFLARED_ARTIFACT_SHA,
    ]);
    client.call_host(
        "ensure_host_artifact",
        json!({"uri": artifact_uri, "destination": artifact_path, "sha256": artifact_sha, "executable": true}),
    )?;
    let executable = match artifact_path.strip_prefix('~') {
        Some(rest) => format!("%h{rest}"),
        None => artifact_path.clone(),
    };
    let wanted_by = if scope == "system" {
        "multi-user.target"
    } else {
        "default.target"
    };
    let unit = format!(
        "[Unit]\nDescription=Opute Cloudflare tunnel\nAfter=network-online.target\n\n[Service]\nExecStart={executable} tunnel --no-autoupdate run --token {}\nRestart=on-failure\n\n[Install]\nWantedBy={wanted_by}\n",
        shell_quote(run_token)
    );
    client.call_host(
        "ensure_host_file",
        json!({"path": service_file, "content": unit, "mode": 0o600}),
    )?;
    client.call_host("ensure_host_service_supervisor", json!({"scope": scope}))?;
    client.call_host(
        "set_host_service_state",
        json!({"uri": host_service_uri(scope, &service_name), "serviceName": service_name, "state": "start", "scope": scope}),
    )?;
    let target = args.string("localTarget", "");
    if !target.is_empty() {
        client.call_host(
            "probe_http_endpoint",
            json!({"endpoint": target, "acceptAuthenticationChallenge": true}),
        )?;
    }
    Ok(())
}

const CONTAINER_TUNNEL_SCRIPT: &str = "set -eu; curl -fsSL --proto =https --tlsv1.2 \"$1\" -o /tmp/cloudflared; echo \"$2  /tmp/cloudflared\" | sha256sum -c -; install -m 0755 /tmp/cloudflared /usr/local/bin/cloudflared; umask 077; mkdir -p /etc/opute-cloudflare; cat > /etc/opute-cloudflare/tunnel-token; chmod 0600 /etc/opute-cloudflare/tunnel-token; chmod 0700 /etc/opute-cloudflare; cat > /etc/systemd/system/opute-cloudflare-tunnel.service <<'UNIT'\n[Unit]\nAfter=network-online.target\n\n[Service]\nExecStart=/usr/local/bin/cloudflared tunnel --no-autoupdate run --token-file /etc/opute-cloudflare/tunnel-token\nRestart=on-failure\n\n[Install]\nWantedBy=multi-user.target\nUNIT\nchmod 0644 /etc/systemd/system/opute-cloudflare-tunnel.service; systemctl daemon-reload; systemctl enable --now opute-cloudflare-tunnel.service";

fn reconcile_container_tunnel(client: &HostAgent, args: &Args) -> Result<(), String> {
    reconcile_container_tunnel_with(
        &mut |name, arguments| client.call_host(name, arguments).map(|_| ()),
        args,
    )
}

fn reconcile_container_tunnel_with(
    call: &mut dyn FnMut(&str, J) -> Result<(), String>,
    args: &Args,
) -> Result<(), String> {
    let target = typed_target_uri(&args.string("targetUri", ""), "container")?;
    let token = args.string("runToken", "");
    let artifact_uri = args.string("artifactUri", "");
    let artifact_sha = args.string("artifactSha256", "");
    let artifact_sha = artifact_sha
        .strip_prefix("sha256:")
        .unwrap_or(&artifact_sha)
        .to_string();
    if token.is_empty() || artifact_uri.is_empty() || artifact_sha.is_empty() {
        return Err(
            "container placement requires runToken, artifactUri, and artifactSha256".into(),
        );
    }
    call(
        "provision_container",
        json!({"containerName": target.resource_id, "image": args.string("image", ""), "nesting": true}),
    )?;
    call(
        "run_instance_command",
        json!({
            "uri": target.to_string(),
            "command": "sh",
            "args": ["-lc", CONTAINER_TUNNEL_SCRIPT, "cloudflared", artifact_uri, artifact_sha],
            "stdin": token,
            "timeoutMs": 120000,
        }),
    )
}

fn install_connector(ctx: &Ctx, args: &Args) -> Outcome {
    let placement = first_non_empty(&[&args.string("placement", ""), "kubernetes"]);
    if placement == "container" {
        let client = connect_host_agent(ctx)?;
        let mut container_args = args.cloned();
        if container_args.string("runToken", "").is_empty() {
            let token = container_args.string("token", "");
            container_args.set("runToken", J::from(token));
        }
        reconcile_container_tunnel(&client, &container_args)?;
        return Ok(json!({"contractVersion": TUNNELING, "ready": true, "placement": placement}));
    }
    if placement != "kubernetes" {
        return Err("connector placement must be kubernetes or container".into());
    }
    let client = connect_host_agent(ctx)?;
    install_kubernetes_connector(&client, args)?;
    Ok(json!({"contractVersion": TUNNELING, "ready": true, "placement": placement}))
}

fn install_kubernetes_connector(client: &HostAgent, args: &Args) -> Result<(), String> {
    install_kubernetes_connector_with(
        &mut |name, arguments| client.call_host(name, arguments).map(|_| ()),
        args,
    )
}

/// Only `put_k8s_secret` receives the token, in its write-only data field;
/// manifests carry resource configuration only.
fn install_kubernetes_connector_with(
    call: &mut dyn FnMut(&str, J) -> Result<(), String>,
    args: &Args,
) -> Result<(), String> {
    let target = typed_target_uri(&args.string("targetUri", ""), "cluster")?;
    let namespace = first_non_empty(&[&args.string("namespace", ""), "cloudflare-connector"]);
    let name = first_non_empty(&[&args.string("name", ""), "cloudflared"]);
    let image = first_non_empty(&[&args.string("image", ""), "cloudflare/cloudflared:2026.7.2"]);
    let local_targets = parse_local_targets(args.get("localTargets"))?;
    let forwarder_image =
        first_non_empty(&[&args.string("forwarderImage", ""), DEFAULT_FORWARDER_IMAGE]);
    let token = first_non_empty(&[&args.string("token", ""), &args.string("runToken", "")]);
    if token.is_empty() {
        return Err("Cloudflare connector token is required".into());
    }
    crate::clusterops::validate_name(&namespace, "namespace")?;
    crate::clusterops::validate_name(&name, "name")?;
    let secret_name = format!("{name}-token");
    crate::clusterops::validate_name(&secret_name, "name")?;
    call(
        "apply_manifest",
        json!({"uri": target.to_string(), "manifest": namespace_manifest(&namespace)}),
    )?;
    call(
        "put_k8s_secret",
        json!({"uri": target.to_string(), "namespace": namespace, "name": secret_name, "data": {"token": token}}),
    )?;
    call(
        "apply_manifest",
        json!({"uri": target.to_string(), "manifest": cloudflared_manifest(&namespace, &name, &image, args.int("replicas", 1), &local_targets, &forwarder_image)}),
    )
}

fn delete_connector(ctx: &Ctx, args: &Args) -> Outcome {
    let client = connect_host_agent(ctx)?;
    let target = typed_target_uri(&args.string("targetUri", ""), "cluster")?;
    client.call_host(
        "delete_k8s_resource",
        json!({"uri": target.to_string(), "kind": "namespace", "resourceName": first_non_empty(&[&args.string("namespace", ""), "cloudflare-connector"])}),
    )?;
    Ok(
        json!({"contractVersion": TUNNELING, "ready": true, "deleted": true, "placement": "kubernetes"}),
    )
}

fn probe_tunnel(ctx: &Ctx, args: &Args) -> Outcome {
    let client = connect_host_agent(ctx)?;
    let endpoint = args.string("localTarget", "");
    if endpoint.is_empty() {
        return Err("localTarget is required".into());
    }
    let result = client.call_host(
        "probe_http_endpoint",
        json!({"endpoint": endpoint, "acceptAuthenticationChallenge": true}),
    )?;
    let hostnames = hostnames_from_args(args);
    let (routed_hostnames, routed, tunnel_id) = api::probe_tunnel_routing(args, &hostnames);
    let mut probes = vec![result.get("structuredContent").cloned().unwrap_or(J::Null)];
    let mut ready = result.get("isError").and_then(J::as_bool) != Some(true);
    for hostname in &hostnames {
        let public_url = format!("https://{hostname}/mcp");
        match authenticated_public_tools_list(&public_url) {
            Ok(listed) => probes.push(listed),
            Err(e) => {
                ready = false;
                probes.push(json!({"endpoint": public_url, "ready": false, "error": e}));
            }
        }
    }
    Ok(json!({
        "contractVersion": TUNNELING,
        "ready": ready,
        "bindingId": args.string("bindingId", ""),
        "placement": first_non_empty(&[&args.string("placement", ""), "host"]),
        "hostname": args.string("hostname", ""),
        "hostnames": hostnames,
        // The tunnel's configuration, not whether its connector is up.
        "routed": routed,
        "routedHostnames": routed_hostnames.map_or(J::Null, |h| json!(h)),
        "tunnelId": tunnel_id,
        "probe": probes,
    }))
}

const PUBLIC_LIST_BODY: &str = r#"{"jsonrpc":"2.0","id":"1","method":"tools/list","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientInfo":{"name":"cloudflare-tunnel-probe","version":"1.0.0"},"io.modelcontextprotocol/clientCapabilities":{}}}}"#;

fn authenticated_public_tools_list(resource_url: &str) -> Result<J, String> {
    let token = mint_public_host_agent_token(resource_url)?;
    let response = super::support::http_do(HttpRequest {
        method: "POST",
        url: resource_url,
        headers: vec![
            ("Authorization", format!("Bearer {token}")),
            ("Content-Type", "application/json".into()),
            ("Accept", "application/json, text/event-stream".into()),
            ("MCP-Protocol-Version", "2026-07-28".into()),
            ("Mcp-Method", "tools/list".into()),
        ],
        body: Some(PUBLIC_LIST_BODY.as_bytes().to_vec()),
        timeout: Some(Duration::from_secs(15)),
        https_redirects: None,
        limit: 1 << 20,
    })?;
    if response.status >= 300 {
        return Err(format!(
            "public tools/list failed: {}",
            response.status_line
        ));
    }
    let doc: J = api::decode_first(&response.body)?;
    if let Some(error) = doc.get("error").filter(|e| !e.is_null()) {
        let message = error.get("message").and_then(J::as_str).unwrap_or("");
        return Err(format!("public tools/list: {message}"));
    }
    let tools = doc
        .get("result")
        .and_then(|r| r.get("tools"))
        .cloned()
        .unwrap_or(J::Null);
    Ok(json!({"endpoint": resource_url, "ready": true, "tools": tools}))
}

/// The audience-bound credential an HTTPS Host Agent endpoint requires; a
/// local MCP_AUTH_TOKEN is never sent to a public route.
fn mint_public_host_agent_token(resource_url: &str) -> Result<String, String> {
    let token_url = format!(
        "{}/oauth/token",
        resource_url.strip_suffix("/mcp").unwrap_or(resource_url)
    );
    let form = format!(
        "grant_type=client_credentials&client_id=opute-mcp-host&resource={}",
        query_escape(resource_url)
    );
    let response = super::support::http_do(HttpRequest {
        method: "POST",
        url: &token_url,
        headers: vec![("Content-Type", "application/x-www-form-urlencoded".into())],
        body: Some(form.into_bytes()),
        timeout: Some(Duration::from_secs(15)),
        https_redirects: None,
        limit: 64 * 1024,
    })?;
    if response.status >= 300 {
        return Err(format!(
            "public token mint failed: {}",
            response.status_line
        ));
    }
    api::decode_first(&response.body)
        .ok()
        .and_then(|doc| {
            doc.get("access_token")
                .and_then(J::as_str)
                .map(String::from)
        })
        .filter(|t| !t.is_empty())
        .ok_or_else(|| "public token mint missing access_token".to_string())
}

/// `url.QueryEscape`.
pub(super) fn query_escape(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn remove_tunnel(ctx: &Ctx, args: &Args) -> Outcome {
    let scope = host_service_scope(args)?;
    let hostname = args.string("hostname", "");
    let dedicated = !hostname.is_empty() || !args.string("tunnelName", "").is_empty();
    let service_name = first_non_empty(&[
        &args.string("serviceName", ""),
        "opute-cloudflare-tunnel.service",
    ]);
    let service_file = first_non_empty(&[
        &args.string("serviceFile", ""),
        &default_host_service_file(&scope, &service_name),
    ]);
    let client = match connect_host_agent(ctx) {
        Ok(client) => Some(client),
        Err(e) if !dedicated => return Err(e),
        Err(_) => None,
    };
    if let Some(client) = client {
        let uri = host_service_uri(&scope, &service_name);
        let calls = [
            (
                "set_host_service_state",
                json!({"uri": uri, "serviceName": service_name, "state": "stop", "scope": scope}),
            ),
            (
                "set_host_service_state",
                json!({"uri": uri, "serviceName": service_name, "state": "disable", "scope": scope}),
            ),
            (
                "remove_host_file",
                json!({"path": service_file, "confirm": true, "scope": scope}),
            ),
        ];
        for (name, arguments) in calls {
            if let Err(e) = client.call_host(name, arguments) {
                if !dedicated {
                    return Err(e);
                }
            }
        }
    }
    if dedicated {
        api::unpublish_dedicated_tunnel(args)?;
    }
    Ok(json!({
        "contractVersion": TUNNELING,
        "ready": true,
        "deleted": true,
        "bindingId": args.string("bindingId", ""),
        "placement": first_non_empty(&[&args.string("placement", ""), "host"]),
        "hostname": hostname,
    }))
}

// --- teardown ---------------------------------------------------------------------

fn teardown(call: &Call) -> Outcome {
    // json.Unmarshal into struct{Phase string; Inputs map[string]any}.
    let (phase, inputs) = super::support::teardown_input(call.arguments)?;
    let ctx = Ctx {
        delegation: call.delegation(),
    };
    let _ = &ctx;
    if first_non_empty(&[&phase, &string_input(&inputs, "phase", "")]) == "finalize" {
        api::cleanup_external_resources(&inputs)?;
        return Ok(json!({"completed": true}));
    }
    let args = Args::from_map(inputs.clone());
    let service_name = first_non_empty(&[
        &args.string("serviceName", ""),
        "opute-cloudflare-tunnel.service",
    ]);
    let scope = host_service_scope(&args)?;
    if !service_name_ok(&service_name) {
        return Err("serviceName must name a .service unit".into());
    }
    let service_file = first_non_empty(&[
        &args.string("serviceFile", ""),
        &default_host_service_file(&scope, &service_name),
    ]);
    validate_service_file(&scope, &service_file)?;
    let service_uri = first_non_empty(&[
        &args.string("serviceUri", ""),
        &host_service_uri(&scope, &service_name),
    ]);
    let cleanup_key = format!(
        "{}-{}",
        args.string("tunnelId", ""),
        args.strings("dnsRecordIds").join(",")
    );
    let plan_id = "com.opute.cloudflare.teardown";
    Ok(json!({"contractVersion": "host-plan.v1", "plan": {
        "contractVersion": "host-plan.v1",
        "planId": plan_id,
        "generation": 1,
        "idempotencyKey": format!("{plan_id}-{service_name}-{service_file}-{cleanup_key}"),
        "nodes": [{"id": "inspect-service", "action": {"tool": "inspect_host_service", "args": {"uri": service_uri, "scope": scope}}}],
    }}))
}

fn service_name_ok(name: &str) -> bool {
    name.strip_suffix(".service").is_some_and(|stem| {
        !stem.is_empty()
            && stem
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '@' | ':' | '-'))
    })
}

fn validate_service_file(scope: &str, service_file: &str) -> Result<String, String> {
    let service_file = service_file.trim();
    let name = service_file
        .rsplit_once('/')
        .map_or(service_file, |(_, n)| n);
    if !service_name_ok(name) {
        return Err("Cloudflare service file must name a .service unit".into());
    }
    if scope == "system"
        && !service_file
            .strip_prefix("/etc/systemd/system/")
            .is_some_and(service_name_ok)
    {
        return Err("system-scoped Cloudflare service file must be one .service unit beneath /etc/systemd/system".into());
    }
    Ok(service_file.to_string())
}

// --- helpers ------------------------------------------------------------------------

fn connect_host_agent(ctx: &Ctx) -> Result<HostAgent, String> {
    let endpoint = std::env::var("OPUTE_HOST_AGENT_ENDPOINT").unwrap_or_default();
    let endpoint = endpoint.trim();
    if endpoint.is_empty() {
        return Err(
            "OPUTE_HOST_AGENT_ENDPOINT is required for Cloudflare provider callbacks".into(),
        );
    }
    let mut token = first_non_empty(&[
        &std::env::var("OPUTE_HOST_AGENT_BEARER_TOKEN").unwrap_or_default(),
        &std::env::var("MCP_AUTH_TOKEN").unwrap_or_default(),
    ]);
    if url::Url::parse(endpoint).is_ok_and(|u| u.scheme().eq_ignore_ascii_case("https")) {
        token = mint_public_host_agent_token(endpoint)
            .map_err(|e| format!("mint public Host Agent callback token: {e}"))?;
    }
    HostAgent::connect(endpoint, &token, &ctx.delegation)
}

fn typed_target_uri(raw: &str, expected: &str) -> Result<Uri, String> {
    let target = Uri::parse(raw.trim())
        .map_err(|e| format!("targetUri must be a canonical {expected} resource URI: {e}"))?;
    if target.resource_type != expected {
        return Err(format!(
            "targetUri requires resource type {}, got {}",
            quote(expected),
            quote(&target.resource_type)
        ));
    }
    Ok(target)
}

fn placement_input(args: &Args) -> Result<String, String> {
    let placement = first_non_empty(&[&args.string("placement", ""), "host"]);
    match placement.as_str() {
        "host" | "container" | "kubernetes" => Ok(placement),
        _ => Err(format!(
            "unsupported Cloudflare placement {}",
            quote(&placement)
        )),
    }
}

/// `validateLocalTarget`: an HTTP(S) URL with a host; a host connector
/// needs an explicit port, and a non-loopback one an origin host.
fn validate_local_target(raw: &str, connector: &str, origin_host_id: &str) -> Result<(), String> {
    let mut trimmed = raw.trim().to_string();
    if trimmed == "host-agent-mcp" {
        trimmed = "http://127.0.0.1:3004/mcp".into();
    }
    let invalid = || "localTarget must be an HTTP(S) URL".to_string();
    // url.ParseRequestURI: an absolute URI without a fragment.
    if raw.contains(['\r', '\n', '\0']) || trimmed.contains('#') {
        return Err(invalid());
    }
    let (scheme, rest) = trimmed.split_once("://").ok_or_else(invalid)?;
    if scheme != "http" && scheme != "https" {
        return Err(invalid());
    }
    let authority = rest.split(['/', '?']).next().unwrap_or("");
    let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    if host_port.is_empty() {
        return Err(invalid());
    }
    let (hostname, port) = if let Some(inner) = host_port.strip_prefix('[') {
        let (h, after) = inner.split_once(']').ok_or_else(invalid)?;
        (
            h.to_string(),
            after.strip_prefix(':').unwrap_or("").to_string(),
        )
    } else {
        match host_port.split_once(':') {
            Some((h, p)) => (h.to_string(), p.to_string()),
            None => (host_port.to_string(), String::new()),
        }
    };
    let loopback = hostname == "127.0.0.1" || hostname == "localhost";
    if connector == "host" && port.is_empty() {
        return Err("host connector requires an explicit loopback localTarget port".into());
    }
    if connector == "host" && !loopback && origin_host_id.trim().is_empty() {
        return Err("remote host connector localTarget requires an explicit originHostId".into());
    }
    Ok(())
}

fn hostnames_from_args(args: &Args) -> Vec<String> {
    let mut hostnames = args.strings("hostnames");
    let hostname = args.string("hostname", "");
    if !hostname.is_empty() && !hostnames.contains(&hostname) {
        hostnames.push(hostname);
    }
    hostnames
}

fn host_service_uri(scope: &str, service_name: &str) -> String {
    format!(
        "host-service:local:{}/{}",
        scope.trim(),
        service_name.trim()
    )
}

fn host_service_scope(args: &Args) -> Result<String, String> {
    let scope = first_non_empty(&[&args.string("scope", ""), "user"]);
    if scope != "user" && scope != "system" {
        return Err(format!("unsupported host service scope {}", quote(&scope)));
    }
    Ok(scope)
}

fn default_host_service_file(scope: &str, service_name: &str) -> String {
    if scope == "system" {
        format!("/etc/systemd/system/{service_name}")
    } else {
        format!("~/.config/systemd/user/{service_name}")
    }
}

fn default_cloudflared_artifact_path(scope: &str) -> &'static str {
    if scope == "system" {
        "/opt/opute/cloudflared"
    } else {
        "~/.local/share/opute/providers/com.opute.cloudflare/bin/cloudflared"
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// `json.Marshal` of a string, as the manifests quote YAML scalars.
fn yaml_quote(value: &str) -> String {
    let mut out = String::new();
    crate::gojson::encode_string(value, &mut out);
    out
}

fn namespace_manifest(namespace: &str) -> String {
    format!(
        "apiVersion: v1\nkind: Namespace\nmetadata:\n  name: {}\n",
        yaml_quote(namespace)
    )
}

struct LocalTarget {
    local_port: i64,
    target: String,
}

fn cloudflared_manifest(
    namespace: &str,
    name: &str,
    image: &str,
    replicas: i64,
    targets: &[LocalTarget],
    forwarder_image: &str,
) -> String {
    let replicas = replicas.max(1);
    let (n, ns, img, secret) = (
        yaml_quote(name),
        yaml_quote(namespace),
        yaml_quote(image),
        yaml_quote(&format!("{name}-token")),
    );
    format!("apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: {n}\n  namespace: {ns}\nspec:\n  replicas: {replicas}\n  selector:\n    matchLabels:\n      app: {n}\n  template:\n    metadata:\n      labels:\n        app: {n}\n    spec:\n      containers:\n      - name: cloudflared\n        image: {img}\n        args: [tunnel, --no-autoupdate, run, --token-file, /etc/cloudflared/tunnel-token]\n        volumeMounts:\n        - name: tunnel-token\n          mountPath: /etc/cloudflared/tunnel-token\n          subPath: tunnel-token\n          readOnly: true\n        readinessProbe:\n          exec:\n            command: [cloudflared, --version]\n          initialDelaySeconds: 5\n          periodSeconds: 10\n      volumes:\n      - name: tunnel-token\n        secret:\n          secretName: {secret}\n          defaultMode: 0444\n          items:\n          - key: token\n            path: tunnel-token\n")
        + &forwarder_containers(targets, forwarder_image)
}

/// `parseLocalTargets`: `[{localPort, target}]`, each target `host:port`,
/// each port bound once.
fn parse_local_targets(raw: Option<&J>) -> Result<Vec<LocalTarget>, String> {
    let Some(raw) = raw.filter(|r| !r.is_null()) else {
        return Ok(Vec::new());
    };
    let Some(entries) = raw.as_array() else {
        return Err("localTargets must be an array of {localPort, target}".into());
    };
    let mut targets: Vec<LocalTarget> = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let Some(fields) = entry.as_object() else {
            return Err(format!(
                "localTargets[{index}] must be an object with localPort and target"
            ));
        };
        let port = Args::from_map(fields.clone()).int("localPort", 0);
        if !(1..=65535).contains(&port) {
            return Err(format!(
                "localTargets[{index}].localPort must be a TCP port between 1 and 65535, got {}",
                go_v(fields.get("localPort"))
            ));
        }
        let target = string_input(fields, "target", "");
        if target.is_empty() {
            return Err(format!("localTargets[{index}].target is required"));
        }
        let split = split_host_port(&target);
        if !split.is_some_and(|(h, p)| !h.trim().is_empty() && !p.trim().is_empty()) {
            return Err(format!(
                "localTargets[{index}].target must be host:port, got {}",
                quote(&target)
            ));
        }
        if let Some(previous) = targets.iter().find(|t| t.local_port == port) {
            return Err(format!(
                "localTargets binds localhost:{port} to both {} and {}",
                quote(&previous.target),
                quote(&target)
            ));
        }
        targets.push(LocalTarget {
            local_port: port,
            target,
        });
    }
    Ok(targets)
}

/// `fmt.Sprint("%v")` of a decoded JSON value.
fn go_v(value: Option<&J>) -> String {
    match value {
        None | Some(J::Null) => "<nil>".into(),
        Some(J::String(s)) => s.clone(),
        Some(J::Number(n)) => {
            let f = n.as_f64().unwrap_or(0.0);
            // %v of a float64 uses %g: shortest digits, exponent past 21.
            if f.fract() == 0.0 && f.abs() < 1e21 {
                format!("{}", f as i64)
            } else {
                let mut s = String::new();
                crate::gojson::encode_float(f, &mut s);
                s
            }
        }
        Some(J::Bool(b)) => b.to_string(),
        Some(J::Array(items)) => format!(
            "[{}]",
            items
                .iter()
                .map(|i| go_v(Some(i)))
                .collect::<Vec<_>>()
                .join(" ")
        ),
        Some(J::Object(map)) => format!(
            "map[{}]",
            map.iter()
                .map(|(k, v)| format!("{k}:{}", go_v(Some(v))))
                .collect::<Vec<_>>()
                .join(" ")
        ),
    }
}

/// `net.SplitHostPort`.
fn split_host_port(hostport: &str) -> Option<(String, String)> {
    if let Some(rest) = hostport.strip_prefix('[') {
        let (host, after) = rest.split_once(']')?;
        let port = after.strip_prefix(':')?;
        if port.contains(':') || host.contains('[') {
            return None;
        }
        return Some((host.to_string(), port.to_string()));
    }
    let (host, port) = hostport.rsplit_once(':')?;
    if host.contains(':') {
        return None;
    }
    Some((host.to_string(), port.to_string()))
}

fn forwarder_containers(targets: &[LocalTarget], image: &str) -> String {
    let mut out = String::new();
    for target in targets {
        let listen = format!(
            "TCP-LISTEN:{},fork,reuseaddr,bind=127.0.0.1",
            target.local_port
        );
        let connect = format!("TCP:{}", target.target);
        out.push_str(&format!("      - name: forward-{}\n", target.local_port));
        out.push_str(&format!("        image: {}\n", yaml_quote(image)));
        out.push_str(&format!(
            "        args: [{}, {}]\n",
            yaml_quote(&listen),
            yaml_quote(&connect)
        ));
        out.push_str("        resources:\n          requests:\n            cpu: 10m\n            memory: 16Mi\n          limits:\n            memory: 64Mi\n");
    }
    out
}

#[cfg(test)]
mod tests;
