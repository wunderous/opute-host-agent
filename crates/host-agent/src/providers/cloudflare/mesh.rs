//! Cloudflare Mesh (Go `mesh.go` and `mesh_runtime.go`): mesh-runtime
//! records, WARP connector membership, and HA endpoints served by a
//! dedicated tunnel. Membership and endpoint references live in this
//! process only, as Go's do.

use super::super::support::{
    first_non_empty, http_do, numeric_input, string_input, Args, HostAgent, HttpRequest,
};
use super::{api, connect_host_agent, provider_operation, typed_target_uri, Ctx, Outcome};
use super::{MESH_RUNTIME, NETWORK_OVERLAY};
use crate::goerr::quote;
use crate::provider::definition::{Operation, ResourceBinding};
use crate::resource::Uri;
use serde_json::{json, Map, Value as J};
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

pub(super) const CLOUDFLARED_ARTIFACT_URL: &str =
    "https://github.com/cloudflare/cloudflared/releases/download/2026.8.2/cloudflared-linux-amd64";
pub(super) const CLOUDFLARED_ARTIFACT_SHA: &str =
    "fcfb02b575a52ca1af2e3267af4e1517bcdeb30ac48c834c69abaed3c0576ad2";
const MAX_CLOUDFLARED_ARTIFACT: u64 = 128 << 20;
const CLOUDFLARED_CHUNK_CHARS: usize = 2 << 20;

const MESH_RUNTIME_VALIDATE: &str = "opute.capability.mesh-runtime.validate";
const MESH_RUNTIME_ENSURE_AGENT: &str = "opute.capability.mesh-runtime.ensure-agent";
const MESH_RUNTIME_ENSURE_CONTROL_PLANE: &str = "opute.capability.mesh-runtime.ensure-control-plane";
const MESH_RUNTIME_STATUS: &str = "opute.capability.mesh-runtime.status";
const MESH_MEMBERSHIP_ENROLL: &str = "opute.capability.mesh-membership.enroll";
const MESH_MEMBERSHIP_STATUS: &str = "opute.capability.mesh-membership.status";
const MESH_MEMBERSHIP_LEAVE: &str = "opute.capability.mesh-membership.leave";
const PUBLIC_INGRESS_ENSURE: &str = "opute.capability.public-ingress.ensure";
const PUBLIC_INGRESS_PROMOTE: &str = "opute.capability.public-ingress.promote";
const PUBLIC_INGRESS_PROBE: &str = "opute.capability.public-ingress.probe";
const OVERLAY_VALIDATE: &str = "opute.capability.network-overlay.validate";
const OVERLAY_PREPARE_MEMBERSHIP: &str = "opute.capability.network-overlay.prepare-membership";
const OVERLAY_ATTACH_TARGET: &str = "opute.capability.network-overlay.attach-target";
const OVERLAY_PROBE_REACHABILITY: &str = "opute.capability.network-overlay.probe-reachability";
const OVERLAY_ENSURE_HA_ENDPOINT: &str = "opute.capability.network-overlay.ensure-ha-endpoint";
const OVERLAY_REMOVE_HA_ENDPOINT: &str = "opute.capability.network-overlay.remove-ha-endpoint";
const OVERLAY_REMOVE_MEMBERSHIP: &str = "opute.capability.network-overlay.remove-membership";

#[derive(Clone, Default)]
struct RuntimeRecord {
    target_uri: String,
    agent_ready: bool,
    control_plane_ready: bool,
    agent_version: String,
    control_plane_ref: String,
    ingress_class_name: String,
}

#[derive(Clone)]
struct Binding {
    node_id: String,
    node_token: String,
    instance_uri: String,
}

struct HaEndpoint {
    hostname: String,
    tunnel_name: String,
    service_name: String,
    target_uris: Vec<String>,
}

static RUNTIMES: LazyLock<Mutex<HashMap<String, RuntimeRecord>>> = LazyLock::new(Default::default);
static BINDINGS: LazyLock<Mutex<HashMap<String, Binding>>> = LazyLock::new(Default::default);
static HA_ENDPOINTS: LazyLock<Mutex<HashMap<String, HaEndpoint>>> = LazyLock::new(Default::default);
static ARTIFACT_CACHE: Mutex<String> = Mutex::new(String::new());

// --- the Service Definitions -------------------------------------------------------

fn target_uri_schema() -> J {
    json!({"type": "string", "pattern": "^(vm|container):[a-z][a-z0-9-]{0,31}:.+$"})
}

/// Cloudflare Mesh runs in the target's network namespace, which a VM or a
/// system container provides; either canonical form is accepted.
fn mesh_target_binding() -> Vec<ResourceBinding> {
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

fn mesh_operation(id: &str, effect: &str, input: J) -> Operation {
    provider_operation(id, effect, input, json!({"type": "object"}), &["host", "network"], mesh_target_binding())
}

pub(super) fn runtime_operations() -> Vec<Operation> {
    let props = json!({
        "targetUri": target_uri_schema(),
        "hostAgentId": {"type": "string", "minLength": 1},
        "operatorMode": {"type": "boolean"},
        "ingressClassName": {"type": "string"},
        "operatorEvidence": {"type": "string"},
        "controlPlaneRef": {"type": "string"},
    });
    let input = || json!({"type": "object", "required": ["targetUri"], "properties": props.clone()});
    vec![
        mesh_operation(MESH_RUNTIME_VALIDATE, "read", input()),
        mesh_operation(MESH_RUNTIME_ENSURE_AGENT, "mutation", input()),
        mesh_operation(MESH_RUNTIME_ENSURE_CONTROL_PLANE, "mutation", input()),
        mesh_operation(MESH_RUNTIME_STATUS, "read", input()),
    ]
}

pub(super) fn membership_operations() -> Vec<Operation> {
    let membership_ref = json!({"type": "string", "minLength": 1, "writeOnly": true});
    vec![
        mesh_operation(MESH_MEMBERSHIP_ENROLL, "mutation", json!({
            "type": "object",
            "required": ["targetUri", "name"],
            "properties": {
                "targetUri": target_uri_schema(),
                "name": {"type": "string", "pattern": "^[A-Za-z0-9][A-Za-z0-9_.-]{0,62}$"},
                "membershipRef": membership_ref,
            },
        })),
        mesh_operation(MESH_MEMBERSHIP_STATUS, "read", json!({
            "type": "object",
            "required": ["targetUri"],
            "properties": {"targetUri": target_uri_schema()},
        })),
        mesh_operation(MESH_MEMBERSHIP_LEAVE, "destructive", json!({
            "type": "object",
            "required": ["membershipRef", "targetUri"],
            "properties": {"membershipRef": membership_ref, "targetUri": target_uri_schema()},
        })),
    ]
}

pub(super) fn public_ingress_operations() -> Vec<Operation> {
    let membership_ref = json!({"type": "string", "minLength": 1, "writeOnly": true});
    vec![
        mesh_operation(PUBLIC_INGRESS_ENSURE, "mutation", json!({
            "type": "object",
            "required": ["targetUri", "membershipRef"],
            "properties": {
                "targetUri": target_uri_schema(),
                "membershipRef": membership_ref,
                "localTarget": {"type": "string", "format": "uri"},
                "hostname": {"type": "string", "pattern": "^[a-z0-9][a-z0-9.-]{0,252}$"},
                "endpoint": {"type": "string"},
                "tunnelName": {"type": "string", "pattern": "^[A-Za-z0-9][A-Za-z0-9_.-]{0,62}$"},
                "peerTargetUris": {"type": "array", "items": target_uri_schema()},
                "operatorMode": {"type": "boolean"},
                "ingressClassName": {"type": "string"},
                "operatorEvidence": {"type": "string"},
            },
        })),
        mesh_operation(PUBLIC_INGRESS_PROMOTE, "mutation", json!({
            "type": "object",
            "required": ["targetUri", "membershipRef"],
            "properties": {"targetUri": target_uri_schema(), "membershipRef": membership_ref},
        })),
        mesh_operation(PUBLIC_INGRESS_PROBE, "read", json!({
            "type": "object",
            "required": ["targetUri"],
            "properties": {"targetUri": target_uri_schema(), "peerMeshIp": {"type": "string", "format": "ipv4"}},
        })),
    ]
}

// --- dispatch ----------------------------------------------------------------------

/// The mesh half of Go's `dispatchCloudflareOperation`; `None` for an
/// operation that is not a mesh one.
pub(super) fn dispatch(ctx: &Ctx, operation: &str, args: &Args) -> Option<Outcome> {
    Some(match operation {
        MESH_RUNTIME_VALIDATE | MESH_RUNTIME_ENSURE_AGENT | MESH_RUNTIME_ENSURE_CONTROL_PLANE | MESH_RUNTIME_STATUS => {
            runtime(operation, args)
        }
        MESH_MEMBERSHIP_ENROLL | OVERLAY_PREPARE_MEMBERSHIP => prepare(args),
        MESH_MEMBERSHIP_STATUS | OVERLAY_VALIDATE => validate(args),
        MESH_MEMBERSHIP_LEAVE | OVERLAY_REMOVE_MEMBERSHIP => remove(ctx, args),
        // Cloudflare HA endpoints are already the stable public form.
        PUBLIC_INGRESS_ENSURE | PUBLIC_INGRESS_PROMOTE | OVERLAY_ENSURE_HA_ENDPOINT => ensure_ha_endpoint(ctx, args),
        PUBLIC_INGRESS_PROBE | OVERLAY_PROBE_REACHABILITY => probe(ctx, args),
        OVERLAY_ATTACH_TARGET => attach(ctx, args),
        OVERLAY_REMOVE_HA_ENDPOINT => remove_ha_endpoint(ctx, args),
        _ => return None,
    })
}

fn runtime(operation: &str, args: &Args) -> Outcome {
    let target = args.string("targetUri", "").trim().to_string();
    if target.is_empty() {
        return Err("targetUri is required".into());
    }
    let mut runtimes = RUNTIMES.lock().unwrap_or_else(|e| e.into_inner());
    match operation {
        MESH_RUNTIME_ENSURE_AGENT => {
            let rec = runtimes.entry(target.clone()).or_default();
            rec.target_uri = target;
            rec.agent_ready = true;
            rec.agent_version = "cloudflared".into();
            Ok(runtime_out(rec, true))
        }
        MESH_RUNTIME_ENSURE_CONTROL_PLANE => {
            if !runtimes.get(&target).is_some_and(|r| r.agent_ready) {
                return Err("mesh-runtime.ensure-control-plane requires mesh-runtime.ensure-agent first".into());
            }
            // The host-tunnel path has no separate operator control plane;
            // it is ready once the agent is.
            let rec = runtimes.entry(target).or_default();
            rec.control_plane_ready = true;
            rec.control_plane_ref = first_non_empty(&[&args.string("controlPlaneRef", ""), "cloudflare-edge"]);
            rec.ingress_class_name = args.string("ingressClassName", "");
            Ok(runtime_out(rec, true))
        }
        _ => {
            let rec = runtimes.get(&target).cloned().unwrap_or_default();
            Ok(runtime_out(&rec, rec.agent_ready))
        }
    }
}

fn runtime_out(rec: &RuntimeRecord, ready: bool) -> J {
    json!({
        "contractVersion": MESH_RUNTIME,
        "provider": "com.opute.cloudflare",
        "targetUri": rec.target_uri,
        "ready": ready,
        "agentReady": rec.agent_ready,
        "controlPlaneReady": rec.control_plane_ready,
        "agentVersion": rec.agent_version,
        "controlPlaneRef": rec.control_plane_ref,
        "ingressClassName": rec.ingress_class_name,
        "generation": "com.opute.cloudflare@1.0.1",
        "probe": {"agentReady": rec.agent_ready, "controlPlaneReady": rec.control_plane_ready},
    })
}

// --- membership ----------------------------------------------------------------------

/// `parseTargetURI`: a canonical VM or container URI.
fn parse_target_uri(args: &Args) -> Result<Uri, String> {
    let uri = Uri::parse(args.string("targetUri", "").trim())
        .map_err(|e| format!("targetUri must be a canonical VM or container resource URI: {e}"))?;
    if uri.resource_type != "vm" && uri.resource_type != "container" {
        return Err(format!(
            "targetUri requires resource type \"vm\" or \"container\", got {}",
            quote(&uri.resource_type)
        ));
    }
    Ok(uri)
}

/// A Cloudflare object decoded into a Go struct of string fields.
fn string_fields(value: Option<&J>, ty: &str, fields: &[&str]) -> Result<Vec<String>, String> {
    let mut out = vec![String::new(); fields.len()];
    let Some(value) = value else {
        return Ok(out);
    };
    let Some(map) = value.as_object() else {
        return Err(format!(
            "decode Cloudflare result: json: cannot unmarshal {} into Go value of type main.{ty}",
            kind(value)
        ));
    };
    for (key, item) in map {
        let Some(i) = fields.iter().position(|f| crate::gojson::equal_fold(f, key)) else {
            continue;
        };
        match item {
            J::String(s) => out[i] = s.clone(),
            J::Null => {}
            other => {
                return Err(format!(
                    "decode Cloudflare result: json: cannot unmarshal {} into Go struct field {ty}.{} of type string",
                    kind(other),
                    fields[i]
                ))
            }
        }
    }
    Ok(out)
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

const NODE_FIELDS: [&str; 5] = ["id", "name", "status", "ip", "token"];

fn validate(args: &Args) -> Outcome {
    let uri = parse_target_uri(args)?;
    let (account, _, token) = api::creds()?;
    let endpoint = format!("{}/accounts/{account}/warp_connector", api::api_base());
    let result = api::cloudflare_json(&token, "GET", &endpoint, None)
        .map_err(|e| format!("list Cloudflare Mesh nodes: {e}"))?;
    let nodes = match &result {
        None => Vec::new(),
        Some(J::Array(items)) => items
            .iter()
            .map(|n| string_fields(Some(n), "meshNode", &NODE_FIELDS))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("list Cloudflare Mesh nodes: {e}"))?,
        Some(other) => {
            return Err(format!(
                "list Cloudflare Mesh nodes: decode Cloudflare result: json: cannot unmarshal {} into Go value of type []main.meshNode",
                kind(other)
            ))
        }
    };
    let summaries: Vec<J> = nodes
        .iter()
        .map(|n| json!({"id": n[0], "name": n[1], "status": n[2]}))
        .collect();
    Ok(json!({
        "contractVersion": NETWORK_OVERLAY,
        "ready": true,
        "targetUri": uri.to_string(),
        "provider": "cloudflare-mesh",
        "nodeCount": nodes.len(),
        "nodes": summaries,
    }))
}

fn prepare(args: &Args) -> Outcome {
    let uri = parse_target_uri(args)?;
    let name = args.string("name", "").trim().to_string();
    if name.is_empty() {
        return Err("name is required".into());
    }
    let (account, _, token) = api::creds()?;
    ensure_mesh_connectivity(&account, &token)?;
    let base = api::api_base();
    let body = crate::gojson::encode_to_string(&json!({"name": name, "ha": false}));
    let created = api::cloudflare_json(&token, "POST", &format!("{base}/accounts/{account}/warp_connector"), Some(body))
        .map_err(|e| format!("create Cloudflare Mesh node: {e}"))?;
    let node = string_fields(created.as_ref(), "meshNode", &NODE_FIELDS)
        .map_err(|e| format!("create Cloudflare Mesh node: {e}"))?;
    let node_id = node[0].clone();
    let mut node_token = node[4].clone();
    if node_id.is_empty() {
        return Err("Cloudflare Mesh node response did not include an id".into());
    }
    if node_token.is_empty() {
        let endpoint = format!("{base}/accounts/{account}/warp_connector/{node_id}/token");
        match api::cloudflare_json(&token, "GET", &endpoint, None)
            .map_err(|e| format!("retrieve Cloudflare Mesh node token: {e}"))?
        {
            None => {}
            Some(J::String(s)) => node_token = s,
            Some(other) => {
                return Err(format!(
                    "retrieve Cloudflare Mesh node token: decode Cloudflare result: json: cannot unmarshal {} into Go value of type string",
                    kind(&other)
                ))
            }
        }
    }
    if node_token.trim().is_empty() {
        return Err("Cloudflare Mesh node response did not include a token".into());
    }
    let reference = new_membership_ref()?;
    BINDINGS.lock().unwrap_or_else(|e| e.into_inner()).insert(
        reference.clone(),
        Binding { node_id: node_id.clone(), node_token, instance_uri: uri.to_string() },
    );
    Ok(json!({
        "contractVersion": NETWORK_OVERLAY,
        "ready": true,
        "provider": "cloudflare-mesh",
        "membershipRef": reference,
        "nodeId": node_id,
        "nodeName": name,
        "targetUri": uri.to_string(),
        "tokenExposed": false,
    }))
}

/// `ensureMeshConnectivity`: turn on only the two account settings a
/// WARP-to-WARP overlay needs, with a sparse PATCH.
fn ensure_mesh_connectivity(account: &str, token: &str) -> Result<(), String> {
    let endpoint = format!("{}/accounts/{account}/zerotrust/connectivity_settings", api::api_base());
    let current = api::cloudflare_json(token, "GET", &endpoint, None)
        .map_err(|e| format!("read Cloudflare Mesh connectivity settings: {e}"))?;
    let (offramp, icmp) = connectivity_flags(current.as_ref())
        .map_err(|e| format!("read Cloudflare Mesh connectivity settings: {e}"))?;
    let mut patch = Map::new();
    if !offramp {
        patch.insert("offramp_warp_enabled".into(), J::Bool(true));
    }
    if !icmp {
        patch.insert("icmp_proxy_enabled".into(), J::Bool(true));
    }
    if patch.is_empty() {
        return Ok(());
    }
    let body = crate::gojson::encode_to_string(&J::Object(patch));
    let updated = api::cloudflare_json(token, "PATCH", &endpoint, Some(body))
        .map_err(|e| format!("enable Cloudflare Mesh connectivity settings: {e}"))?;
    let (offramp, icmp) = connectivity_flags(updated.as_ref())
        .map_err(|e| format!("enable Cloudflare Mesh connectivity settings: {e}"))?;
    if !offramp || !icmp {
        return Err(format!(
            "Cloudflare Mesh connectivity settings were not enabled: offramp_warp_enabled={offramp} icmp_proxy_enabled={icmp}"
        ));
    }
    Ok(())
}

fn connectivity_flags(value: Option<&J>) -> Result<(bool, bool), String> {
    let Some(value) = value else {
        return Ok((false, false));
    };
    let Some(map) = value.as_object() else {
        return Err(format!(
            "decode Cloudflare result: json: cannot unmarshal {} into Go value of type main.meshConnectivitySettings",
            kind(value)
        ));
    };
    let mut flags = (false, false);
    for (key, item) in map {
        let slot = if crate::gojson::equal_fold(key, "offramp_warp_enabled") {
            &mut flags.0
        } else if crate::gojson::equal_fold(key, "icmp_proxy_enabled") {
            &mut flags.1
        } else {
            continue;
        };
        match item {
            J::Bool(b) => *slot = *b,
            J::Null => {}
            other => {
                let field = if crate::gojson::equal_fold(key, "offramp_warp_enabled") {
                    "offramp_warp_enabled"
                } else {
                    "icmp_proxy_enabled"
                };
                return Err(format!(
                    "decode Cloudflare result: json: cannot unmarshal {} into Go struct field meshConnectivitySettings.{field} of type bool",
                    kind(other)
                ));
            }
        }
    }
    Ok(flags)
}

fn structured_content(result: &Map<String, J>) -> Option<&Map<String, J>> {
    result.get("structuredContent").and_then(J::as_object)
}

fn attach(ctx: &Ctx, args: &Args) -> Outcome {
    let uri = parse_target_uri(args)?;
    let reference = args.string("membershipRef", "").trim().to_string();
    if reference.is_empty() {
        return Err("membershipRef is required".into());
    }
    let binding = BINDINGS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&reference)
        .cloned()
        .ok_or_else(|| "network overlay membership reference is unknown or expired".to_string())?;
    if binding.instance_uri != uri.to_string() {
        return Err("network overlay membership is bound to another instance".into());
    }
    let client = connect_host_agent(ctx)?;
    wait_for_guest_exec(&client, &uri)?;
    let result = client.call_host(
        "run_instance_command",
        json!({
            "uri": uri.to_string(),
            "command": "bash",
            "args": ["-lc", MESH_INSTALL_SCRIPT],
            "stdin": format!("{}\n", binding.node_token),
            "timeoutMs": 10 * 60 * 1000,
        }),
    )?;
    let content = structured_content(&result)
        .ok_or_else(|| "Mesh enrollment returned no structured command result".to_string())?;
    let output = string_input(content, "stdout", "");
    let stderr = string_input(content, "stderr", "");
    let combined = format!("{output}\n{stderr}");
    let (exit_code, present) = numeric_input(content, "exitCode");
    if present && exit_code != 0 {
        return Err(format!(
            "Cloudflare Mesh guest enrollment failed with exit code {exit_code}: {}",
            command_diagnostics(&combined, &binding.node_token)
        ));
    }
    let mesh_ip = find_mesh_ip(&output);
    if mesh_ip.is_empty() {
        return Err(format!(
            "Cloudflare Mesh node connected but no Mesh IP was observed: {}",
            command_diagnostics(&combined, &binding.node_token)
        ));
    }
    let mesh_interface = find_mesh_interface(&output, &mesh_ip);
    if mesh_interface.is_empty() {
        return Err(format!(
            "Cloudflare Mesh node connected but no Mesh interface was observed: {}",
            command_diagnostics(&combined, &binding.node_token)
        ));
    }
    Ok(json!({
        "contractVersion": NETWORK_OVERLAY,
        "ready": true,
        "provider": "cloudflare-mesh",
        "membershipRef": reference,
        "nodeId": binding.node_id,
        "targetUri": uri.to_string(),
        "meshIp": mesh_ip,
        "meshInterface": mesh_interface,
    }))
}

/// The provider's guest-runtime contract (Go `cloudflareMeshInstallScript`),
/// byte for byte.
pub(super) const MESH_INSTALL_SCRIPT: &str = r#"set -euo pipefail
IFS= read -r mesh_token || true
if [ -z "$mesh_token" ]; then
  echo 'Cloudflare Mesh connector token was not received on stdin' >&2
  exit 1
fi
ipv6_disabled="$(cat /proc/sys/net/ipv6/conf/all/disable_ipv6 2>/dev/null || printf 1)"
if [ ! -r /proc/net/if_inet6 ] || [ "$ipv6_disabled" != "0" ]; then
  echo 'Cloudflare Mesh requires an IPv6-capable guest kernel; this target has IPv6 disabled' >&2
  exit 78
fi
if ! command -v warp-cli >/dev/null 2>&1; then
  export DEBIAN_FRONTEND=noninteractive
  timeout 180s apt-get update -qq
  timeout 180s apt-get install -y -qq ca-certificates curl gnupg
  install -d -m 0755 /usr/share/keyrings
  timeout 60s curl -fsSL https://pkg.cloudflareclient.com/pubkey.gpg | gpg --yes --dearmor -o /usr/share/keyrings/cloudflare-warp-archive-keyring.gpg
  . /etc/os-release
  printf 'deb [signed-by=/usr/share/keyrings/cloudflare-warp-archive-keyring.gpg] https://pkg.cloudflareclient.com/ %s main\n' "$VERSION_CODENAME" > /etc/apt/sources.list.d/cloudflare-client.list
  timeout 180s apt-get update -qq
  timeout 180s apt-get install -y -qq cloudflare-warp
fi
if command -v systemctl >/dev/null 2>&1; then
  timeout 30s systemctl enable --now warp-svc.service >/dev/null 2>&1 || true
fi
printf 'net.ipv4.ip_forward = 1\nnet.ipv6.conf.all.forwarding = 1\nnet.ipv6.conf.all.accept_ra = 2\n' > /etc/sysctl.d/99-zzz-cloudflare-warp-connector.conf
sysctl --system >/dev/null 2>&1 || true
# Readiness is the Mesh address, not the status text.
#
# The packaged 2026.7.x client prints nothing for status or registration
# show under a non-interactive exec: three runs failed the bounded wait as
# "did not reach Connected" against guests that already held CloudflareWARP
# 100.96.0.131/32 and 100.96.0.132/32, and the failure diagnostics -- which run
# the same commands with 2>&1 -- captured no status line either, only settings
# output and the addresses. Gating enrollment on text this client does not emit
# made a healthy connector unprovable.
#
# The address is the signal the overlay actually owns: WARP assigns one in
# 100.96.0.0/12 only after the connector registers with Zero Trust and the
# tunnel is up, and probe-reachability then proves the peer path end to end
# before any K3s traffic uses it. A guest address such as 10.0.100.225 contains
# "100.", so the address test stays anchored on the CIDR field rather than on a
# substring.
mesh_address() {
  ip -4 -o addr show scope global 2>/dev/null \
    | awk '$4 ~ /^100\.(9[6-9]|10[0-9]|11[01])\./ { print $4; exit }'
}
mesh_ready() {
  [ -n "$(mesh_address)" ]
}
if ! mesh_ready; then
  timeout 45s warp-cli --accept-tos connector new "$mesh_token"
  timeout 45s warp-cli --accept-tos connect
fi
# A cold guest installs the package, registers with Zero Trust, and only then
# receives its Mesh address; two minutes was inside that window on a loaded
# host, and the enrollment failed while the connector was still coming up.
connected=0
for attempt in $(seq 1 150); do
  if mesh_ready; then
    connected=1
    break
  fi
  sleep 2
done
if [ "$connected" -ne 1 ]; then
  echo 'Cloudflare Mesh connector received no address in 100.96.0.0/12 before the bounded wait' >&2
  # Report what the client will say about itself, the registration mode, and
  # every address the guest actually holds, so the next attempt starts from
  # evidence. An empty status is itself a finding -- that is how this client
  # behaves under a non-interactive exec.
  warp-cli --accept-tos status 2>&1 || true
  warp-cli --accept-tos registration show 2>&1 || true
  warp-cli --accept-tos settings 2>&1 || true
  echo 'warp-interface addresses:'
  ip -4 -o addr show scope global 2>&1 || true
  exit 1
fi
# Same Mesh CIDR test the readiness predicate uses. A bare "100." prefix also
# matches the rest of the shared CGNAT space (100.64/10), so a guest that holds
# a carrier-grade address would have had the Mesh route published on the wrong
# interface.
mesh_iface="$(ip -4 -o addr show scope global 2>/dev/null | awk '$4 ~ /^100\.(9[6-9]|10[0-9]|11[01])\./ {print $2; exit}' | sed 's/:$//' )"
if [ -z "$mesh_iface" ]; then
  echo 'Cloudflare Mesh interface could not be identified for the main-table route' >&2
  exit 1
fi
# WARP normally installs its broad routes in a policy table selected by a
# firewall mark. Native daemons such as k3s do not inherit that mark, so their
# ordinary TCP sockets otherwise resolve a Mesh peer through the Incus bridge.
# Publish the provider-owned Mesh CIDR in the main table as well; this is the
# declared overlay route that makes the endpoint usable by k3s and etcd.
ip route replace 100.96.0.0/12 dev "$mesh_iface"
warp-cli --accept-tos status
ip -4 -o addr show scope global"#;

fn probe(ctx: &Ctx, args: &Args) -> Outcome {
    let uri = parse_target_uri(args)?;
    let peer = args
        .string("peerMeshIp", "")
        .trim()
        .parse::<std::net::IpAddr>()
        .ok()
        .and_then(|ip| match ip {
            std::net::IpAddr::V4(v4) => Some(v4),
            std::net::IpAddr::V6(v6) => v6.to_ipv4_mapped(),
        })
        .ok_or_else(|| "peerMeshIp must be an IPv4 address".to_string())?;
    let client = connect_host_agent(ctx)?;
    let result = client.call_host(
        "run_instance_command",
        json!({
            "uri": uri.to_string(),
            "command": "ping",
            "args": ["-c", "1", "-W", "3", peer.to_string()],
            "timeoutMs": 15 * 1000,
        }),
    )?;
    let content = structured_content(&result)
        .ok_or_else(|| "Cloudflare Mesh reachability probe returned no structured command result".to_string())?;
    let (exit_code, present) = numeric_input(content, "exitCode");
    if !present || exit_code != 0 {
        return Err(format!(
            "Cloudflare Mesh reachability probe failed with exit code {exit_code}: {}",
            command_diagnostics(
                &format!("{}\n{}", string_input(content, "stdout", ""), string_input(content, "stderr", "")),
                ""
            )
        ));
    }
    Ok(json!({
        "contractVersion": NETWORK_OVERLAY,
        "ready": true,
        "provider": "cloudflare-mesh",
        "targetUri": uri.to_string(),
        "peerMeshIp": peer.to_string(),
        "probe": J::Object(content.clone()),
    }))
}

fn remove(ctx: &Ctx, args: &Args) -> Outcome {
    let uri = parse_target_uri(args)?;
    let reference = args.string("membershipRef", "").trim().to_string();
    if reference.is_empty() {
        return Err("membershipRef is required".into());
    }
    let binding = BINDINGS.lock().unwrap_or_else(|e| e.into_inner()).get(&reference).cloned();
    let Some(binding) = binding else {
        return Ok(json!({"contractVersion": NETWORK_OVERLAY, "ready": true, "deleted": true, "membershipRef": reference}));
    };
    if binding.instance_uri != uri.to_string() {
        return Err("network overlay membership is bound to another instance".into());
    }
    let client = connect_host_agent(ctx)?;
    client.call_host(
        "run_instance_command",
        json!({
            "uri": uri.to_string(),
            "command": "bash",
            "args": ["-lc", "warp-cli disconnect || true; warp-cli registration delete || true"],
            "timeoutMs": 30 * 1000,
        }),
    )?;
    let (account, _, token) = api::creds()?;
    let endpoint = format!("{}/accounts/{account}/warp_connector/{}", api::api_base(), binding.node_id);
    api::cloudflare_json(&token, "DELETE", &endpoint, None).map_err(|e| format!("delete Cloudflare Mesh node: {e}"))?;
    BINDINGS.lock().unwrap_or_else(|e| e.into_inner()).remove(&reference);
    Ok(json!({
        "contractVersion": NETWORK_OVERLAY,
        "ready": true,
        "deleted": true,
        "membershipRef": reference,
        "targetUri": uri.to_string(),
    }))
}

// --- HA endpoints -----------------------------------------------------------------------

fn ensure_ha_endpoint(ctx: &Ctx, args: &Args) -> Outcome {
    let targets = target_uris_input(args)?;
    let hostname = args.string("hostname", "").trim().to_lowercase();
    api::refuse_platform_hostname(&hostname)?;
    let tunnel_name = args.string("tunnelName", "").trim().to_string();
    api::refuse_platform_tunnel_name(&tunnel_name)?;
    let (account, zone, token) = api::creds()?;
    let tunnel = api::ensure_tunnel_by_name(&token, &account, &tunnel_name)?;
    let rules = api::tunnel_ingress(&token, &account, &tunnel.id)?;
    api::refuse_platform_ingress(&rules)?;
    api::put_tunnel_ingress(
        &token,
        &account,
        &tunnel.id,
        &[
            api::IngressRule {
                hostname: hostname.clone(),
                service: "https://127.0.0.1:6443".into(),
                origin_request: Some(json!({"noTLSVerify": true})),
            },
            api::IngressRule { hostname: String::new(), service: "http_status:404".into(), origin_request: None },
        ],
    )?;
    api::upsert_cname(&token, &account, &zone, &hostname, &tunnel.id)?;
    let tunnel_token = api::tunnel_token(&token, &account, &tunnel.id)?;
    let service_name = format!("opute-cloudflare-mesh-ha-{tunnel_name}.service");
    let unpublish = || {
        let mut map = Map::new();
        map.insert("hostname".into(), J::from(hostname.as_str()));
        map.insert("tunnelName".into(), J::from(tunnel_name.as_str()));
        let _ = api::unpublish_dedicated_tunnel(&Args::from_map(map));
    };
    let client = connect_host_agent(ctx)?;
    let mut installed: Vec<&Uri> = Vec::new();
    for target in &targets {
        if let Err(e) = install_ha_connector(&client, target, &service_name, &tunnel_token) {
            for done in &installed {
                let _ = remove_ha_connector(&client, done, &service_name);
            }
            unpublish();
            return Err(e);
        }
        installed.push(target);
    }
    let endpoint = format!("https://{hostname}");
    let status = match wait_for_ha_endpoint(&endpoint, Duration::from_secs(3 * 60)) {
        Ok(status) => status,
        Err(e) => {
            if let Ok(cleanup) = connect_host_agent(ctx) {
                for done in &installed {
                    let _ = remove_ha_connector(&cleanup, done, &service_name);
                }
            }
            unpublish();
            return Err(e);
        }
    };
    let endpoint_ref = new_membership_ref()?;
    HA_ENDPOINTS.lock().unwrap_or_else(|e| e.into_inner()).insert(
        endpoint_ref.clone(),
        HaEndpoint {
            hostname,
            tunnel_name,
            service_name,
            target_uris: targets.iter().map(Uri::to_string).collect(),
        },
    );
    Ok(json!({
        "contractVersion": NETWORK_OVERLAY,
        "ready": true,
        "provider": "cloudflare-tunnel",
        "endpoint": endpoint,
        "endpointRef": endpoint_ref,
        "targetCount": targets.len(),
        "statusCode": status,
    }))
}

fn remove_ha_endpoint(ctx: &Ctx, args: &Args) -> Outcome {
    let reference = args.string("endpointRef", "").trim().to_string();
    if reference.is_empty() {
        return Err("endpointRef is required".into());
    }
    let binding = HA_ENDPOINTS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&reference)
        .ok_or_else(|| "network overlay HA endpoint reference is unknown or expired".to_string())?;
    let client = connect_host_agent(ctx)?;
    // errors.Join: every failure, one per line.
    let mut errors = Vec::new();
    for target in &binding.target_uris {
        match Uri::parse(target) {
            Err(e) => errors.push(e),
            Ok(parsed) => {
                if let Err(e) = remove_ha_connector(&client, &parsed, &binding.service_name) {
                    errors.push(e);
                }
            }
        }
    }
    let mut map = Map::new();
    map.insert("hostname".into(), J::from(binding.hostname));
    map.insert("tunnelName".into(), J::from(binding.tunnel_name));
    if let Err(e) = api::unpublish_dedicated_tunnel(&Args::from_map(map)) {
        errors.push(e);
    }
    if !errors.is_empty() {
        return Err(errors.join("\n"));
    }
    Ok(json!({"contractVersion": NETWORK_OVERLAY, "ready": true, "deleted": true}))
}

/// `targetURIsInput`: the target and its peers, at least two distinct VMs.
fn target_uris_input(args: &Args) -> Result<Vec<Uri>, String> {
    let mut raw_values = Vec::new();
    let primary = args.string("targetUri", "").trim().to_string();
    if !primary.is_empty() {
        raw_values.push(primary);
    }
    if let Some(J::Array(values)) = args.get("peerTargetUris") {
        for value in values {
            let text = value.as_str().ok_or_else(|| "targetUris must contain strings".to_string())?;
            raw_values.push(text.to_string());
        }
    }
    if raw_values.len() < 2 {
        return Err("targetUri and peerTargetUris must contain at least two VM resource URIs".into());
    }
    let mut targets: Vec<Uri> = Vec::with_capacity(raw_values.len());
    for raw in &raw_values {
        let target = typed_target_uri(raw, "vm")?;
        if targets.iter().any(|t| t.to_string() == target.to_string()) {
            return Err("targetUris must not contain duplicates".into());
        }
        targets.push(target);
    }
    Ok(targets)
}

const HA_CONNECTOR_SCRIPT: &str = "set -eu
IFS= read -r tunnel_token || true
if [ -z \"$tunnel_token\" ]; then
  echo 'Cloudflare Tunnel token was not received on stdin' >&2
  exit 1
fi
if ! command -v cloudflared >/dev/null 2>&1; then
\tinstall -m 0755 /tmp/opute-cloudflared /usr/local/bin/cloudflared
fi
rm -f /tmp/opute-cloudflared
printf '%s\\n' '[Unit]' 'After=network-online.target' '' '[Service]' \"ExecStart=/usr/local/bin/cloudflared tunnel --no-autoupdate run --token $tunnel_token\" 'Restart=on-failure' > /etc/systemd/system/SERVICE_NAME
unset tunnel_token
systemctl daemon-reload
systemctl enable --now SERVICE_NAME
cloudflared --version >/dev/null";

fn install_ha_connector(client: &HostAgent, target: &Uri, service_name: &str, token: &str) -> Result<(), String> {
    wait_for_guest_exec(client, target)?;
    let artifact = verified_cloudflared_artifact()?;
    stage_cloudflared_artifact(client, target, &artifact)
        .map_err(|e| format!("stage verified Cloudflare connector in guest: {e}"))?;
    let script = HA_CONNECTOR_SCRIPT.replace("SERVICE_NAME", service_name);
    let result = client.call_host(
        "run_instance_command",
        json!({"uri": target.to_string(), "command": "bash", "args": ["-lc", script], "stdin": format!("{token}\n"), "timeoutMs": 10 * 60 * 1000}),
    )?;
    match result.get("structuredContent") {
        None | Some(J::Null) => return Err("Cloudflare HA connector returned no command result".into()),
        Some(J::Object(output)) => {
            let (code, present) = numeric_input(output, "exitCode");
            if present && code != 0 {
                return Err(format!(
                    "Cloudflare HA connector failed with exit code {code}: {}",
                    command_diagnostics(
                        &format!("{}\n{}", string_input(output, "stdout", ""), string_input(output, "stderr", "")),
                        token
                    )
                ));
            }
        }
        Some(_) => {}
    }
    Ok(())
}

fn command_exit(result: &Map<String, J>) -> Option<i64> {
    structured_content(result).and_then(|content| {
        let (code, present) = numeric_input(content, "exitCode");
        (present && code != 0).then_some(code)
    })
}

fn stage_cloudflared_artifact(client: &HostAgent, target: &Uri, encoded: &str) -> Result<(), String> {
    let mut start = 0;
    while start < encoded.len() {
        let end = (start + CLOUDFLARED_CHUNK_CHARS).min(encoded.len());
        let redirect = if start > 0 { ">>" } else { ">" };
        let result = client.call_host(
            "run_instance_command",
            json!({
                "uri": target.to_string(),
                "command": "bash",
                "args": ["-lc", format!("set -euo pipefail; base64 --decode {redirect} /tmp/opute-cloudflared.gz")],
                "stdin": &encoded[start..end],
                "timeoutMs": 10 * 60 * 1000,
            }),
        )?;
        if let Some(code) = command_exit(&result) {
            return Err(format!(
                "Cloudflare connector artifact chunk {} failed with exit code {code}",
                start / CLOUDFLARED_CHUNK_CHARS + 1
            ));
        }
        start += CLOUDFLARED_CHUNK_CHARS;
    }
    let result = client.call_host(
        "run_instance_command",
        json!({
            "uri": target.to_string(),
            "command": "bash",
            "args": ["-lc", format!("set -euo pipefail; gzip --decompress -c /tmp/opute-cloudflared.gz > /tmp/opute-cloudflared; rm -f /tmp/opute-cloudflared.gz; observed=$(sha256sum /tmp/opute-cloudflared | awk '{{print $1}}'); [ \"$observed\" = \"{CLOUDFLARED_ARTIFACT_SHA}\" ]; chmod 0755 /tmp/opute-cloudflared")],
            "timeoutMs": 10 * 60 * 1000,
        }),
    )?;
    if let Some(code) = command_exit(&result) {
        return Err(format!("Cloudflare connector artifact reconstruction failed with exit code {code}"));
    }
    Ok(())
}

/// `verifiedCloudflaredArtifact`: the pinned connector, downloaded once,
/// checked against its digest, gzipped and base64-encoded for staging. The
/// gzip stream is flate2's, not Go's; the guest checks the decompressed
/// digest, so only the staged bytes differ.
fn verified_cloudflared_artifact() -> Result<String, String> {
    {
        let cached = ARTIFACT_CACHE.lock().unwrap_or_else(|e| e.into_inner());
        if !cached.is_empty() {
            return Ok(cached.clone());
        }
    }
    let response = http_do(HttpRequest {
        method: "GET",
        url: CLOUDFLARED_ARTIFACT_URL,
        headers: Vec::new(),
        body: None,
        timeout: Some(Duration::from_secs(5 * 60)),
        https_redirects: Some("Cloudflare connector artifact redirect must remain HTTPS"),
        limit: MAX_CLOUDFLARED_ARTIFACT + 1,
    })
    .map_err(|e| format!("download Cloudflare connector artifact: {e}"))?;
    if !(200..300).contains(&response.status) {
        return Err(format!("download Cloudflare connector artifact: HTTP {}", response.status_line));
    }
    let artifact = response.body;
    if artifact.len() as u64 > MAX_CLOUDFLARED_ARTIFACT {
        return Err(format!("Cloudflare connector artifact exceeds {MAX_CLOUDFLARED_ARTIFACT} bytes"));
    }
    use sha2::Digest as _;
    let observed = hex(&sha2::Sha256::digest(&artifact));
    if !observed.eq_ignore_ascii_case(CLOUDFLARED_ARTIFACT_SHA) {
        return Err(format!(
            "Cloudflare connector artifact checksum mismatch: expected {CLOUDFLARED_ARTIFACT_SHA}, observed {observed}"
        ));
    }
    use std::io::Write as _;
    let mut zipper = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::new(6));
    zipper
        .write_all(&artifact)
        .map_err(|e| format!("compress Cloudflare connector artifact: {e}"))?;
    let compressed = zipper
        .finish()
        .map_err(|e| format!("finish Cloudflare connector artifact compression: {e}"))?;
    use base64::Engine as _;
    let encoded = base64::engine::general_purpose::STANDARD.encode(compressed);
    let mut cached = ARTIFACT_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if cached.is_empty() {
        *cached = encoded;
    }
    Ok(cached.clone())
}

fn remove_ha_connector(client: &HostAgent, target: &Uri, service_name: &str) -> Result<(), String> {
    client.call_host(
        "run_instance_command",
        json!({
            "uri": target.to_string(),
            "command": "bash",
            "args": ["-lc", format!("systemctl disable --now {service_name} >/dev/null 2>&1 || true; rm -f /etc/systemd/system/{service_name}; systemctl daemon-reload")],
            "timeoutMs": 120_000,
        }),
    )?;
    Ok(())
}

/// `waitForHAEndpoint`: `GET <endpoint>/version` every five seconds until an
/// acceptable status or the deadline.
fn wait_for_ha_endpoint(endpoint: &str, timeout: Duration) -> Result<u16, String> {
    let deadline = Instant::now() + timeout;
    let url = format!("{}/version", endpoint.trim_end_matches('/'));
    let mut last_err = None;
    while Instant::now() < deadline {
        match http_do(HttpRequest {
            method: "GET",
            url: &url,
            headers: Vec::new(),
            body: None,
            timeout: None,
            https_redirects: None,
            limit: 4096,
        }) {
            Ok(response) if acceptable_status(response.status) => return Ok(response.status),
            Ok(response) => last_err = Some(format!("stable endpoint returned HTTP {}", response.status)),
            Err(e) => last_err = Some(e),
        }
        std::thread::sleep(Duration::from_secs(5));
    }
    let last_err = last_err.unwrap_or_else(|| "stable endpoint did not become reachable".into());
    Err(format!("stable endpoint {endpoint}: {last_err}"))
}

fn acceptable_status(status: u16) -> bool {
    (200..400).contains(&status) || status == 401 || status == 403
}

/// `waitForGuestExec`: `true` in the guest every two seconds, for up to five
/// minutes, until it exits 0.
fn wait_for_guest_exec(client: &HostAgent, uri: &Uri) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(5 * 60);
    while Instant::now() < deadline {
        if let Ok(result) = client.call_host(
            "run_instance_command",
            json!({"uri": uri.to_string(), "command": "true", "timeoutMs": 30 * 1000}),
        ) {
            if let Some(content) = structured_content(&result) {
                let (code, present) = numeric_input(content, "exitCode");
                if present && code == 0 {
                    return Ok(());
                }
            }
        }
        std::thread::sleep(Duration::from_secs(2));
    }
    Err(format!("timed out waiting for Incus guest agent on {uri}"))
}

// --- helpers --------------------------------------------------------------------------------

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn new_membership_ref() -> Result<String, String> {
    let mut raw = [0u8; 18];
    getrandom::getrandom(&mut raw)
        .map_err(|e| format!("create network overlay membership reference: {e}"))?;
    Ok(format!("mesh-{}", hex(&raw)))
}

/// `net.ParseIP` of an IPv4 (or IPv4-mapped) address, as Go prints it.
fn parse_ipv4(text: &str) -> Option<std::net::Ipv4Addr> {
    match text.parse::<std::net::IpAddr>().ok()? {
        std::net::IpAddr::V4(v4) => Some(v4),
        std::net::IpAddr::V6(v6) => v6.to_ipv4_mapped(),
    }
}

/// `findMeshIP`: the first field that is an address in 100.96.0.0/12.
pub(super) fn find_mesh_ip(output: &str) -> String {
    for field in output.split_whitespace() {
        let candidate = field.trim_matches(['(', ')', ',']);
        let address = candidate.split('/').next().unwrap_or("");
        if let Some(ip) = parse_ipv4(address) {
            let o = ip.octets();
            if o[0] == 100 && (o[1] & 0xf0) == 96 {
                return ip.to_string();
            }
        }
    }
    String::new()
}

/// `findMeshInterface`: the `ip -o addr` line that carries the address.
pub(super) fn find_mesh_interface(output: &str, mesh_ip: &str) -> String {
    for line in output.split('\n') {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 4 || fields[2] != "inet" || !fields[3].starts_with(&format!("{mesh_ip}/")) {
            continue;
        }
        return fields[1].strip_suffix(':').unwrap_or(fields[1]).to_string();
    }
    String::new()
}

const DIAGNOSTIC_LINE_BUDGET: usize = 16;

/// `meshCommandDiagnostics`: the last sixteen telling lines, secret
/// redacted, each cut at 240 bytes.
pub(super) fn command_diagnostics(output: &str, secret: &str) -> String {
    const WORDS: [&str; 11] = [
        "status", "connected", "inet", "interface", "warp", "error", "failed", "unable", "cannot", "registration",
        "terms",
    ];
    let mut lines: Vec<String> = Vec::new();
    for line in output.split('\n') {
        let line = line.trim();
        let lower = line.to_lowercase();
        if line.is_empty() || !WORDS.iter().any(|w| lower.contains(w)) {
            continue;
        }
        let mut line = line.to_string();
        if !secret.is_empty() {
            line = line.replace(secret, "[redacted]");
        }
        lines.push(truncate_bytes(&line, 240));
        if lines.len() > DIAGNOSTIC_LINE_BUDGET {
            lines.drain(..lines.len() - DIAGNOSTIC_LINE_BUDGET);
        }
    }
    if lines.is_empty() {
        return "guest command returned no diagnostic lines".into();
    }
    format!("guest diagnostics={}", lines.join(" | "))
}

/// Go's `line[:n]`: a byte cut; a split rune's bytes each encode as U+FFFD.
fn truncate_bytes(line: &str, n: usize) -> String {
    if line.len() <= n {
        return line.to_string();
    }
    let bytes = &line.as_bytes()[..n];
    let valid = match std::str::from_utf8(bytes) {
        Ok(s) => return s.to_string(),
        Err(e) => e.valid_up_to(),
    };
    let mut out = String::from_utf8_lossy(&bytes[..valid]).into_owned();
    out.extend(std::iter::repeat_n('\u{FFFD}', n - valid));
    out
}
