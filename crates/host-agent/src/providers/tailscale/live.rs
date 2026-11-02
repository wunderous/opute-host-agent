//! The live backend (Go `live.go`): tailscaled in guests through typed Host
//! Agent callbacks, devices through Tailscale's API, ownership in the same
//! store as the fake backend.

use super::api::{device_ipv4, find_device_by_hostname, Client, Device};
use super::overlay::{
    assert_credential_presence, enrolled, ha_endpoint_removed, insert_ingress, membership_removed,
    mesh_runtime_base, overlay_base, owned_for_removal, parse_peer_uri, parse_target_uri, parses_as_ip,
    path_class, prepared, private_mesh_result, private_service_result, probe_result,
    reject_control_plane_public_target, require_credential_kind, require_host_agent_id,
    require_mesh_agent_ready, require_operator_stable_evidence, store, Membership, CRED_API_KEY,
    CRED_AUTH_KEY, PATH_PRIVATE_MESH, PATH_PUBLIC_INGRESS, PROVIDER_NAME,
};
use super::super::support::{first_non_empty, numeric_input, string_input, Args, HostAgent};
use super::{getenv, op, setenv, Ctx, Outcome, GENERATION, NETWORK_OVERLAY};
use crate::goerr::quote;
use crate::resource::Uri;
use serde_json::{json, Map, Value as J};
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex, Once};
use std::time::{Duration, Instant};

const DEFAULT_ENV_FILE: &str = "/home/houman/.config/opute/tailscale.env";
const DEFAULT_AUTH_KEY_FILE: &str = "/home/houman/.config/opute/tailscale.authkey";
const CGNAT_PREFIX: &str = "100.";

static PENDING_AUTH_KEYS: LazyLock<Mutex<HashMap<String, String>>> = LazyLock::new(Default::default);
static ENV_ONCE: Once = Once::new();

/// `ensureTailscaleEnvLoaded`: once per process, the env file's values and
/// the auth key file fill what the environment leaves unset.
fn ensure_env_loaded() {
    ENV_ONCE.call_once(|| {
        load_env_file(&first_non_empty(&[&getenv("OPUTE_TAILSCALE_ENV_FILE"), DEFAULT_ENV_FILE]));
        if getenv("TAILSCALE_AUTH_KEY").trim().is_empty() {
            let file = first_non_empty(&[&getenv("TAILSCALE_AUTH_KEY_FILE"), DEFAULT_AUTH_KEY_FILE]);
            if let Ok(raw) = std::fs::read(&file) {
                let key = String::from_utf8_lossy(&raw).trim().to_string();
                if !key.is_empty() {
                    setenv("TAILSCALE_AUTH_KEY", &key);
                }
            }
        }
    });
}

fn load_env_file(path: &str) {
    let path = path.trim();
    if path.is_empty() || !getenv("TAILSCALE_API_KEY").trim().is_empty() {
        return;
    }
    let Ok(raw) = std::fs::read(path) else {
        return;
    };
    for line in String::from_utf8_lossy(&raw).split('\n') {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim_matches(['"', '\'']).trim();
        if key.is_empty() || value.is_empty() {
            continue;
        }
        if getenv(key).trim().is_empty() {
            setenv(key, value);
        }
    }
}

pub(super) fn overlay(ctx: &Ctx, operation: &str, args: &Args) -> Outcome {
    ensure_env_loaded();
    match operation {
        op::OVERLAY_VALIDATE => validate(args),
        op::OVERLAY_PREPARE_MEMBERSHIP => prepare_membership(args),
        op::OVERLAY_ATTACH_TARGET => attach_target(ctx, args),
        op::OVERLAY_ENROLL => enroll(ctx, args),
        op::OVERLAY_PROBE_REACHABILITY => {
            let mut args = args.cloned();
            if args.string("pathClass", "").is_empty() {
                args.set("pathClass", J::from(PATH_PRIVATE_MESH));
            }
            probe_path_aware(ctx, &args)
        }
        op::OVERLAY_PROBE => probe_path_aware(ctx, args),
        op::OVERLAY_REPORT_TWO_NODE_READINESS => super::overlay::report_two_node_readiness(args),
        op::OVERLAY_ENSURE_PRIVATE_MESH => ensure_private_mesh(ctx, args),
        op::OVERLAY_ENSURE_PRIVATE_SERVICE => ensure_private_service(ctx, args),
        op::OVERLAY_ENSURE_PUBLIC_INGRESS | op::OVERLAY_ENSURE_HA_ENDPOINT => ensure_public_ingress(ctx, args),
        op::OVERLAY_PROMOTE_PUBLIC_INGRESS => super::overlay::promote(args, true),
        op::OVERLAY_REMOVE_HA_ENDPOINT => remove_ha_endpoint(ctx, args),
        op::OVERLAY_REMOVE_MEMBERSHIP => remove_membership(ctx, args),
        _ => Err(format!("unknown network overlay operation {}", quote(operation))),
    }
}

pub(super) fn mesh_runtime(ctx: &Ctx, operation: &str, args: &Args) -> Outcome {
    ensure_env_loaded();
    match operation {
        op::MESH_RUNTIME_ENSURE_AGENT => ensure_mesh_agent(ctx, args),
        op::MESH_RUNTIME_ENSURE_CONTROL_PLANE => ensure_mesh_control_plane(args),
        _ => mesh_runtime_status(ctx, args),
    }
}

// --- mesh runtime ---------------------------------------------------------------------------

fn agent_version(output: &str) -> Option<String> {
    output
        .split('\n')
        .filter_map(|line| line.trim().strip_prefix("agent_version=").map(str::to_string))
        .next_back()
}

fn command_content(result: &Map<String, J>) -> Option<&Map<String, J>> {
    result.get("structuredContent").and_then(J::as_object)
}

fn ensure_mesh_agent(ctx: &Ctx, args: &Args) -> Outcome {
    require_host_agent_id(args)?;
    let uri = parse_target_uri(args)?;
    let client = connect_host_agent(ctx)?;
    wait_for_guest_exec(&client, &uri)?;
    let result = client.call_host(
        "run_instance_command",
        json!({"uri": uri.to_string(), "command": "bash", "args": ["-lc", ENSURE_AGENT_SCRIPT], "timeoutMs": 10 * 60 * 1000}),
    )?;
    let content = command_content(&result)
        .ok_or_else(|| "mesh-runtime.ensure-agent returned no structured command result".to_string())?;
    let output = string_input(content, "stdout", "");
    let stderr = string_input(content, "stderr", "");
    let (code, present) = numeric_input(content, "exitCode");
    if present && code != 0 {
        return Err(format!("mesh-runtime.ensure-agent failed with exit code {code}: {output}\n{stderr}"));
    }
    let version = agent_version(&output).unwrap_or_else(|| "unknown".into());
    let target = uri.to_string();
    let mut store = store();
    let rec = store.runtimes.entry(target.clone()).or_default();
    rec.target_uri = target.clone();
    rec.agent_ready = true;
    rec.agent_version = version;
    rec.generation = GENERATION.into();
    Ok(mesh_runtime_base(&target, rec, true))
}

fn mesh_runtime_status(ctx: &Ctx, args: &Args) -> Outcome {
    require_host_agent_id(args)?;
    let uri = parse_target_uri(args)?;
    let client = connect_host_agent(ctx)?;
    wait_for_guest_exec(&client, &uri)?;
    let result = client.call_host(
        "run_instance_command",
        json!({"uri": uri.to_string(), "command": "bash", "args": ["-lc", AGENT_STATUS_SCRIPT], "timeoutMs": 60 * 1000}),
    )?;
    let content = command_content(&result)
        .ok_or_else(|| "mesh-runtime.status returned no structured command result".to_string())?;
    let output = string_input(content, "stdout", "");
    let ready = output.contains("agent_ready=true");
    let version = agent_version(&output).unwrap_or_default();
    let target = uri.to_string();
    let mut store = store();
    let rec = store.runtimes.entry(target.clone()).or_default();
    rec.target_uri = target.clone();
    rec.agent_ready = ready;
    if !version.is_empty() {
        rec.agent_version = version;
    }
    Ok(mesh_runtime_base(&target, rec, ready))
}

fn ensure_mesh_control_plane(args: &Args) -> Outcome {
    require_host_agent_id(args)?;
    let uri = parse_target_uri(args)?;
    let target = uri.to_string();
    if !store().runtimes.get(&target).is_some_and(|r| r.agent_ready) {
        return Err("mesh-runtime.ensure-control-plane requires mesh-runtime.ensure-agent first".into());
    }
    let ingress_class = first_non_empty(&[&args.string("ingressClassName", ""), "tailscale"]);
    let evidence = args.string("operatorEvidence", "");
    let operator_mode = args.bool("operatorMode", true);
    if operator_mode && evidence.is_empty() && ingress_class != "tailscale" {
        return Err("mesh-runtime.ensure-control-plane operatorMode requires operatorEvidence or ingressClassName=tailscale".into());
    }
    let mut store = store();
    let rec = store.runtimes.entry(target.clone()).or_default();
    rec.target_uri = target.clone();
    rec.control_plane_ready = true;
    rec.ingress_class_name = ingress_class;
    rec.control_plane_ref = first_non_empty(&[&args.string("controlPlaneRef", ""), "tailscale-operator"]);
    rec.generation = GENERATION.into();
    Ok(mesh_runtime_base(&target, rec, true))
}

// --- credentials ---------------------------------------------------------------------------

fn api_client() -> Result<Client, String> {
    ensure_env_loaded();
    let api_key = getenv("TAILSCALE_API_KEY").trim().to_string();
    if api_key.is_empty() {
        return Err("TAILSCALE_API_KEY is required for the live Tailscale backend".into());
    }
    Ok(Client::new(&api_key, &getenv("TAILSCALE_TAILNET"), &getenv("TAILSCALE_API_BASE")))
}

fn resolve_auth_key(args: &Args) -> Result<String, String> {
    ensure_env_loaded();
    let key = first_non_empty(&[&args.string("authKey", ""), &args.string("credential", ""), &getenv("TAILSCALE_AUTH_KEY")]);
    if !key.is_empty() {
        return Ok(key);
    }
    let client = api_client().map_err(|e| format!("auth key unavailable and API client failed: {e}"))?;
    client.create_auth_key(24 * 60 * 60)
}

fn resolve_api_key(args: &Args) -> String {
    ensure_env_loaded();
    first_non_empty(&[&args.string("apiKey", ""), &args.string("credential", ""), &getenv("TAILSCALE_API_KEY")])
}

// --- membership ---------------------------------------------------------------------------

fn validate(args: &Args) -> Outcome {
    let uri = parse_target_uri(args)?;
    let kind = require_credential_kind(args, "")?;
    assert_credential_presence(args, &kind)?;
    if kind == CRED_API_KEY || kind.is_empty() {
        let key = resolve_api_key(args);
        if !key.is_empty() {
            setenv("TAILSCALE_API_KEY", &key);
        }
    }
    let devices = api_client()?.list_devices()?;
    Ok(json!({
        "contractVersion": NETWORK_OVERLAY,
        "ready": true,
        "provider": PROVIDER_NAME,
        "generation": GENERATION,
        "targetUri": uri.to_string(),
        "nodeCount": devices.len(),
        "credentialKind": first_non_empty(&[&kind, CRED_API_KEY]),
    }))
}

fn prepare_membership(args: &Args) -> Outcome {
    require_host_agent_id(args)?;
    let uri = parse_target_uri(args)?;
    require_credential_kind(args, CRED_AUTH_KEY)?;
    assert_credential_presence(args, CRED_AUTH_KEY)?;
    let name = args.string("name", "");
    if name.is_empty() {
        return Err("name is required".into());
    }
    let auth_key = resolve_auth_key(args)?;
    let record = store().upsert_membership(&uri.to_string(), &name, false)?;
    PENDING_AUTH_KEYS
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(record.reference.clone(), auth_key);
    Ok(prepared(&record))
}

fn attach_target(ctx: &Ctx, args: &Args) -> Outcome {
    require_host_agent_id(args)?;
    let uri = parse_target_uri(args)?;
    let reference = args.string("membershipRef", "");
    if reference.is_empty() {
        return Err("membershipRef is required".into());
    }
    let record = store()
        .memberships
        .get(&reference)
        .cloned()
        .ok_or_else(|| "network overlay membership reference is unknown or expired".to_string())?;
    if record.generation != GENERATION {
        return Err("stale provider generation for membership".into());
    }
    if record.target_uri != uri.to_string() {
        return Err("network overlay membership is bound to another instance".into());
    }
    let pending = PENDING_AUTH_KEYS.lock().unwrap_or_else(|e| e.into_inner()).get(&reference).cloned();
    let auth_key = match pending.filter(|k| !k.is_empty()) {
        Some(key) => key,
        None => resolve_auth_key(args)?,
    };
    enroll_guest(ctx, &uri, &record.node_name, &reference, &auth_key)
}

fn enroll(ctx: &Ctx, args: &Args) -> Outcome {
    require_host_agent_id(args)?;
    let uri = parse_target_uri(args)?;
    require_credential_kind(args, CRED_AUTH_KEY)?;
    assert_credential_presence(args, CRED_AUTH_KEY)?;
    let name = first_non_empty(&[&args.string("name", ""), "node"]);
    let mut hint = args.string("membershipRef", "");
    let target = uri.to_string();
    let existing = {
        let store = store();
        store
            .by_target
            .get(&target)
            .map(|r| (r.clone(), store.memberships.get(r).cloned().unwrap_or_default()))
    };
    if let Some((existing_ref, existing)) = existing {
        if existing.generation != GENERATION {
            return Err("stale provider generation for membership".into());
        }
        if !hint.is_empty() && hint != existing_ref {
            return Err("membershipRef does not match owned enrollment for target".into());
        }
        if existing.attached && !existing.mesh_ip.is_empty() && !existing.node_id.is_empty() && !existing.node_id.starts_with("node-") {
            let mut record = existing.clone();
            record.mesh_interface = first_non_empty(&[&existing.mesh_interface, "tailscale0"]);
            return Ok(enrolled(&record));
        }
        hint = existing.reference.clone();
    }
    let auth_key = resolve_auth_key(args)?;
    if hint.is_empty() {
        hint = store().upsert_membership(&target, &name, false)?.reference;
    }
    enroll_guest(ctx, &uri, &name, &hint, &auth_key)
}

fn enroll_guest(ctx: &Ctx, uri: &Uri, hostname: &str, membership_ref: &str, auth_key: &str) -> Outcome {
    let hostname = sanitize_hostname(hostname);
    if hostname.is_empty() {
        return Err("name is required".into());
    }
    let target = uri.to_string();
    require_mesh_agent_ready(&target)?;
    let client = connect_host_agent(ctx)?;
    wait_for_guest_exec(&client, uri)?;
    let result = client.call_host(
        "run_instance_command",
        json!({
            "uri": target,
            "command": "bash",
            "args": ["-lc", enroll_script(&hostname)],
            "stdin": format!("{auth_key}\n"),
            "timeoutMs": 10 * 60 * 1000,
        }),
    )?;
    let content = command_content(&result)
        .ok_or_else(|| "Tailscale enrollment returned no structured command result".to_string())?;
    let output = string_input(content, "stdout", "");
    let stderr = string_input(content, "stderr", "");
    let combined = format!("{output}\n{stderr}");
    let (code, present) = numeric_input(content, "exitCode");
    if present && code != 0 {
        return Err(format!(
            "Tailscale guest enrollment failed with exit code {code}: {}",
            redact_secret(&combined, auth_key)
        ));
    }
    let mut mesh_ip = find_tailscale_ipv4(&output);
    if mesh_ip.is_empty() {
        return Err(format!(
            "Tailscale node connected but no Mesh IP was observed: {}",
            redact_secret(&combined, auth_key)
        ));
    }
    let api = api_client()?;
    let deadline = Instant::now() + Duration::from_secs(2 * 60);
    let device: Device = loop {
        let devices = api.list_devices()?;
        if let Some(found) = find_device_by_hostname(&devices, &hostname) {
            break found;
        }
        if Instant::now() > deadline {
            return Err(format!(
                "Tailscale device with hostname {} was not observed in the API after enrollment",
                quote(&hostname)
            ));
        }
        std::thread::sleep(Duration::from_secs(2));
    };
    if device.id.is_empty() {
        return Err("Tailscale device response did not include an id".into());
    }
    let api_ip = device_ipv4(&device);
    if !api_ip.is_empty() {
        mesh_ip = api_ip;
    }
    let mut store = store();
    let mut membership_ref = membership_ref.to_string();
    let mut record = match store.memberships.get(&membership_ref).cloned() {
        Some(record) => record,
        None => {
            let created = store.upsert_membership(&target, &hostname, true)?;
            membership_ref = created.reference.clone();
            created
        }
    };
    if record.target_uri != target {
        return Err("network overlay membership is bound to another instance".into());
    }
    record.node_name = hostname;
    record.node_id = device.id;
    record.mesh_ip = mesh_ip;
    record.mesh_interface = "tailscale0".into();
    record.attached = true;
    record.generation = GENERATION.into();
    store.memberships.insert(membership_ref.clone(), record.clone());
    store.by_target.insert(record.target_uri.clone(), membership_ref.clone());
    PENDING_AUTH_KEYS.lock().unwrap_or_else(|e| e.into_inner()).remove(&membership_ref);
    Ok(enrolled(&record))
}

// --- private mesh ---------------------------------------------------------------------------

fn ensure_private_mesh(ctx: &Ctx, args: &Args) -> Outcome {
    require_host_agent_id(args)?;
    let uri = parse_target_uri(args)?;
    let peer_uri = parse_peer_uri(args)?;
    let peer_mesh_ip = args.string("peerMeshIp", "");
    if !parses_as_ip(&peer_mesh_ip) {
        return Err("peerMeshIp must be an IPv4 address".into());
    }
    let (target, peer_target) = (uri.to_string(), peer_uri.to_string());
    let (source, mut peer) = {
        let store = store();
        let source_ref = store.by_target.get(&target).ok_or_else(|| "source target is not enrolled".to_string())?;
        let source = store.memberships.get(source_ref).cloned().unwrap_or_default();
        let peer = store.by_target.get(&peer_target).map(|r| store.memberships.get(r).cloned().unwrap_or_default());
        (source, peer)
    };
    let need_adopt = match &peer {
        None => true,
        Some(p) => p.generation != GENERATION || !p.attached || (!p.mesh_ip.is_empty() && p.mesh_ip != peer_mesh_ip),
    };
    if need_adopt {
        let mut adopt = Map::new();
        adopt.insert("hostAgentId".into(), J::from(args.string("hostAgentId", "")));
        adopt.insert("peerTargetUri".into(), J::from(peer_target.as_str()));
        adopt.insert("peerMeshIp".into(), J::from(peer_mesh_ip.as_str()));
        adopt.insert("name".into(), J::from(peer_uri.resource_id.as_str()));
        adopt_remote_peer(&Args::from_map(adopt))
            .map_err(|e| format!("peer target is not enrolled and adopt failed: {e}"))?;
        let store = store();
        peer = Some(
            store
                .by_target
                .get(&peer_target)
                .and_then(|r| store.memberships.get(r).cloned())
                .unwrap_or_default(),
        );
    }
    let peer = peer.unwrap_or_default();
    if source.generation != GENERATION || !source.attached {
        return Err("source membership is not attached for this generation".into());
    }
    if peer.generation != GENERATION || !peer.attached {
        return Err("peer membership is not attached for this generation".into());
    }
    if peer.mesh_ip != peer_mesh_ip {
        return Err("ambiguous peer: peerMeshIp does not match enrolled peer".into());
    }
    let client = connect_host_agent(ctx)?;
    ping_from_guest(&client, &uri, &peer.mesh_ip).map_err(|e| format!("forward private-mesh probe failed: {e}"))?;
    let mut reverse_method = "guest-ping";
    if let Err(e) = ping_from_guest(&client, &peer_uri, &source.mesh_ip) {
        // The peer may live on a remote Incus host; prove the reverse path
        // with a Tailscale ping from the source.
        if let Err(e2) = tailscale_ping_from_guest(&client, &uri, &peer.mesh_ip) {
            return Err(format!("reverse private-mesh probe failed: guest={e} tailscale-ping={e2}"));
        }
        reverse_method = "tailscale-ping-from-source";
    }
    store().set_edges(&target, &source.mesh_ip, &peer_target, &peer.mesh_ip);
    let mut out = private_mesh_result(&source, &peer.mesh_ip, args);
    out["reverseProbeMethod"] = J::from(reverse_method);
    Ok(out)
}

fn adopt_remote_peer(args: &Args) -> Outcome {
    require_host_agent_id(args)?;
    let raw = args.string("peerTargetUri", &args.string("targetUri", ""));
    let peer_uri = Uri::parse(raw.trim())
        .map_err(|e| format!("peerTargetUri must be a canonical resource URI: {e}"))?;
    let mut peer_mesh_ip = args.string("peerMeshIp", "");
    let hostname = sanitize_hostname(&first_non_empty(&[&args.string("name", ""), &peer_uri.resource_id]));
    let devices = api_client()?.list_devices()?;
    let mut matched = None;
    for device in &devices {
        let ip = device_ipv4(device);
        let host = first_non_empty(&[&device.hostname, &device.name]).to_lowercase();
        let host = host.strip_suffix('.').unwrap_or(&host).to_string();
        if !peer_mesh_ip.is_empty() && ip == peer_mesh_ip {
            matched = Some(device.clone());
            break;
        }
        if !hostname.is_empty() && (host == hostname || host.starts_with(&format!("{hostname}."))) {
            matched = Some(device.clone());
            if peer_mesh_ip.is_empty() {
                peer_mesh_ip = ip;
            }
            break;
        }
    }
    let Some(device) = matched else {
        return Err(format!("remote Tailscale peer not found for hostname={hostname} meshIp={peer_mesh_ip}"));
    };
    if peer_mesh_ip.is_empty() {
        peer_mesh_ip = device_ipv4(&device);
    }
    if peer_mesh_ip.is_empty() {
        return Err("remote peer has no IPv4 address".into());
    }
    let peer_target = peer_uri.to_string();
    let mut store = store();
    let mut record = store.upsert_membership(&peer_target, &first_non_empty(&[&hostname, &device.hostname, "peer"]), true)?;
    record.attached = true;
    record.mesh_ip = peer_mesh_ip;
    record.node_id = first_non_empty(&[&device.id, &record.node_id]);
    record.node_name = first_non_empty(&[&hostname, &device.hostname, &record.node_name]);
    record.mesh_interface = "tailscale0".into();
    store.memberships.insert(record.reference.clone(), record.clone());
    store.by_target.insert(peer_target, record.reference.clone());
    Ok(overlay_base(&record, json!({
        "ready": true, "membershipRef": record.reference, "nodeId": record.node_id,
        "nodeName": record.node_name, "meshIp": record.mesh_ip, "adopted": true,
        "overlayUri": record.overlay_uri,
    })))
}

fn ping_with(client: &HostAgent, guest: &Uri, command: &str, args: J, timeout_ms: i64, label: &str, peer_ip: &str) -> Result<(), String> {
    let result = client.call_host(
        "run_instance_command",
        json!({"uri": guest.to_string(), "command": command, "args": args, "timeoutMs": timeout_ms}),
    )?;
    let content = command_content(&result).ok_or_else(|| format!("{label} returned no structured command result"))?;
    let (code, present) = numeric_input(content, "exitCode");
    if !present || code != 0 {
        return Err(format!("{label} {peer_ip} from {guest} failed with exit code {code}"));
    }
    Ok(())
}

fn ping_from_guest(client: &HostAgent, guest: &Uri, peer_ip: &str) -> Result<(), String> {
    ping_with(client, guest, "ping", json!(["-c", "1", "-W", "3", peer_ip]), 15 * 1000, "ping", peer_ip)
}

fn tailscale_ping_from_guest(client: &HostAgent, guest: &Uri, peer_ip: &str) -> Result<(), String> {
    ping_with(client, guest, "tailscale", json!(["ping", "-c", "1", peer_ip]), 30 * 1000, "tailscale ping", peer_ip)
}

fn ensure_private_service(ctx: &Ctx, args: &Args) -> Outcome {
    require_host_agent_id(args)?;
    let uri = parse_target_uri(args)?;
    let local_target = args.string("localTarget", "");
    if local_target.is_empty() {
        return Err("localTarget is required".into());
    }
    let record = store().require_owned_membership(args, &uri.to_string())?;
    let serve_url = normalize_loopback_http(&local_target)?;
    let client = connect_host_agent(ctx)?;
    let script = format!(
        "set -euo pipefail\ntimeout 60s tailscale serve --bg {}\ntailscale serve status || true\n",
        shell_quote(&serve_url)
    );
    let result = client.call_host(
        "run_instance_command",
        json!({"uri": uri.to_string(), "command": "bash", "args": ["-lc", script], "timeoutMs": 90 * 1000}),
    )?;
    if let Some(content) = command_content(&result) {
        let (code, present) = numeric_input(content, "exitCode");
        if present && code != 0 {
            return Err(format!(
                "tailscale serve failed with exit code {code}: {} {}",
                string_input(content, "stderr", ""),
                string_input(content, "stdout", "")
            ));
        }
    }
    store().services.insert(
        record.overlay_uri.clone(),
        super::overlay::PrivateService { target_uri: record.target_uri.clone(), generation: GENERATION.into() },
    );
    Ok(private_service_result(&record, &local_target))
}

// --- public ingress ---------------------------------------------------------------------------

const FUNNEL_SCRIPT: &str = r#"set -euo pipefail
# Reset then enable Funnel to the scoped local gateway/ingress target.
tailscale serve reset >/dev/null 2>&1 || true
tailscale funnel reset >/dev/null 2>&1 || true
timeout 60s tailscale funnel --bg --yes TARGET_URL
DNS_NAME="$(tailscale status --json 2>/dev/null | python3 -c 'import json,sys; d=json.load(sys.stdin); print((d.get("Self") or {}).get("DNSName") or "")' 2>/dev/null || true)"
if [ -z "$DNS_NAME" ]; then
  DNS_NAME="$(tailscale status --json 2>/dev/null | sed -n 's/.*"DNSName"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n1)"
fi
printf 'dns_name=%s\n' "$DNS_NAME"
printf 'local_target=%s\n' TARGET_URL
tailscale funnel status 2>/dev/null || true
tailscale serve status --json 2>/dev/null || true
"#;

fn ensure_public_ingress(ctx: &Ctx, args: &Args) -> Outcome {
    require_host_agent_id(args)?;
    let uri = parse_target_uri(args)?;
    let local_target = args.string("localTarget", "");
    if local_target.is_empty() {
        return Err("localTarget is required".into());
    }
    reject_control_plane_public_target(&local_target, &args.string("targetKind", ""))?;
    let record = store().require_owned_membership(args, &uri.to_string())?;
    let operator_mode = args.bool("operatorMode", false);
    require_operator_stable_evidence(args)?;
    if operator_mode {
        // An operator-managed stable ingress is recorded without the node's
        // Funnel CLI.
        let mut endpoint = args.string("endpoint", "");
        let mut hostname = args.string("hostname", "");
        if endpoint.is_empty() {
            hostname = hostname.strip_suffix('.').unwrap_or(&hostname).to_string();
            if hostname.is_empty() {
                return Err("operatorMode requires endpoint or hostname".into());
            }
            endpoint = if hostname.contains("://") { hostname.clone() } else { format!("https://{hostname}") };
        }
        if hostname.is_empty() {
            let rest = endpoint.strip_prefix("https://").unwrap_or(&endpoint);
            let rest = rest.strip_prefix("http://").unwrap_or(rest);
            hostname = rest.split('/').next().unwrap_or("").to_string();
        }
        let mut store = store();
        let endpoint_ref = insert_ingress(&mut store, &record, &endpoint, true, &hostname)?;
        return Ok(overlay_base(&record, json!({
            "ready": true, "pathClass": PATH_PUBLIC_INGRESS, "endpoint": endpoint,
            "endpointRef": endpoint_ref, "stable": true, "operatorMode": true,
            "operatorEvidence": args.string("operatorEvidence", ""),
            "ingressClassName": args.string("ingressClassName", ""),
        })));
    }
    let serve_url = normalize_loopback_http(&local_target)?;
    let client = connect_host_agent(ctx)?;
    let script = FUNNEL_SCRIPT.replace("TARGET_URL", &shell_quote(&serve_url));
    let result = client.call_host(
        "run_instance_command",
        json!({"uri": uri.to_string(), "command": "bash", "args": ["-lc", script], "timeoutMs": 120 * 1000}),
    )?;
    let empty = Map::new();
    let content = command_content(&result).unwrap_or(&empty);
    let (code, present) = numeric_input(content, "exitCode");
    if present && code != 0 {
        return Err(format!(
            "tailscale funnel failed with exit code {code}: {} {}",
            string_input(content, "stderr", ""),
            string_input(content, "stdout", "")
        ));
    }
    let hostname = first_non_empty(&[
        &args.string("hostname", ""),
        &find_dns_name(&string_input(content, "stdout", "")),
        &format!("{}.ts.net", record.node_name),
    ]);
    let hostname = hostname.strip_suffix('.').unwrap_or(&hostname).to_string();
    let endpoint = format!("https://{hostname}");
    let mut store = store();
    let endpoint_ref = insert_ingress(&mut store, &record, &endpoint, operator_mode, &hostname)?;
    Ok(overlay_base(&record, json!({
        "ready": true, "pathClass": PATH_PUBLIC_INGRESS, "endpoint": endpoint,
        "endpointRef": endpoint_ref, "stable": operator_mode,
    })))
}

fn remove_ha_endpoint(ctx: &Ctx, args: &Args) -> Outcome {
    let endpoint_ref = args.string("endpointRef", "");
    if endpoint_ref.is_empty() {
        return Err("endpointRef is required".into());
    }
    let ingress = store().ingresses.remove(&endpoint_ref);
    let Some(ingress) = ingress else {
        return Ok(ha_endpoint_removed("vm:local:placeholder", None));
    };
    if ingress.generation != GENERATION {
        return Err("refusing to delete endpoint owned by a foreign generation".into());
    }
    if let Ok(client) = connect_host_agent(ctx) {
        let _ = client.call_host(
            "run_instance_command",
            json!({"uri": ingress.target_uri, "command": "bash", "args": ["-lc", "tailscale funnel --bg=false || true; tailscale serve reset || true"], "timeoutMs": 30 * 1000}),
        );
    }
    Ok(ha_endpoint_removed(&ingress.target_uri, Some(&endpoint_ref)))
}

fn logout_and_delete(client: Option<&HostAgent>, api: Option<&Client>, record: &Membership) {
    if let Some(client) = client {
        let _ = client.call_host(
            "run_instance_command",
            json!({"uri": record.target_uri, "command": "bash", "args": ["-lc", "tailscale logout || true"], "timeoutMs": 30 * 1000}),
        );
    }
    if let Some(api) = api {
        if !record.node_id.is_empty() && !record.node_id.starts_with("node-") {
            let _ = api.delete_device(&record.node_id);
        }
    }
}

fn remove_membership(ctx: &Ctx, args: &Args) -> Outcome {
    let (uri, reference, record) = owned_for_removal(args)?;
    let client = connect_host_agent(ctx).ok();
    let api = api_client().ok();
    logout_and_delete(client.as_ref(), api.as_ref(), &record);
    store().delete_owned_for_target(&record.target_uri, &reference);
    Ok(membership_removed(&reference, &uri.to_string()))
}

fn probe_path_aware(ctx: &Ctx, args: &Args) -> Outcome {
    let uri = parse_target_uri(args)?;
    let target = uri.to_string();
    let class = path_class(args)?;
    let peer_mesh_ip = args.string("peerMeshIp", "");
    let not_ready = |error: &str| {
        Ok(json!({
            "contractVersion": NETWORK_OVERLAY,
            "ready": false,
            "provider": PROVIDER_NAME,
            "generation": GENERATION,
            "targetUri": target,
            "pathClass": class,
            "probe": {"pathClass": class, "ready": false},
            "error": error,
            "stable": false,
        }))
    };
    let record = {
        let store = store();
        match store.by_target.get(&target) {
            None => return not_ready("target is not enrolled in the overlay"),
            Some(r) => store.memberships.get(r).cloned().unwrap_or_default(),
        }
    };
    if record.generation != GENERATION {
        return not_ready("stale provider generation for membership");
    }
    let (mut result, mut probe) = probe_result(&record, &class);
    if class == PATH_PRIVATE_MESH {
        if peer_mesh_ip.is_empty() {
            // A membership-presence probe: enroll readiness without a peer.
            result["ready"] = J::Bool(true);
            probe.insert("ready".into(), J::Bool(true));
            probe.insert("membershipOnly".into(), J::Bool(true));
            result["probe"] = J::Object(probe);
            return Ok(result);
        }
        if !parses_as_ip(&peer_mesh_ip) {
            return Err("peerMeshIp must be an IPv4 address".into());
        }
        let mut ready = store().private_mesh_ready(&target, &peer_mesh_ip);
        if !ready {
            if let Ok(client) = connect_host_agent(ctx) {
                if ping_from_guest(&client, &uri, &peer_mesh_ip).is_ok() {
                    ready = true;
                }
            }
        }
        result["ready"] = J::Bool(ready);
        result["peerMeshIp"] = J::from(peer_mesh_ip.as_str());
        probe.insert("ready".into(), J::Bool(ready));
        probe.insert("peerMeshIp".into(), J::from(peer_mesh_ip.as_str()));
        if !ready {
            if store().public_ready(&target) {
                probe.insert("publicIngressPresent".into(), J::Bool(true));
                probe.insert("privateSatisfiedByPublic".into(), J::Bool(false));
            }
            result["error"] = J::from("private-mesh peer reachability is not ready");
        }
    } else {
        let store = store();
        let ready = store.public_ready(&target);
        let (mut stable, mut endpoint) = (false, String::new());
        if ready {
            if let Some(ingress) = store.ingress_of(&target) {
                endpoint = ingress.endpoint.clone();
                stable = ingress.stable;
            }
        }
        result["ready"] = J::Bool(ready);
        result["stable"] = J::Bool(stable);
        if !endpoint.is_empty() {
            result["endpoint"] = J::from(endpoint);
        }
        probe.insert("ready".into(), J::Bool(ready));
        if !ready {
            result["error"] = J::from("public-ingress endpoint is not ready");
        }
    }
    result["probe"] = J::Object(probe);
    Ok(result)
}

pub(super) fn finalize_owned_teardown(ctx: &Ctx, inputs: &Map<String, J>) -> Result<(), String> {
    let generation = first_non_empty(&[&string_input(inputs, "generation", ""), GENERATION]);
    if generation != GENERATION {
        return Err("refusing teardown for foreign provider generation".into());
    }
    ensure_env_loaded();
    let records: Vec<Membership> = store()
        .memberships
        .values()
        .filter(|r| r.generation == generation)
        .cloned()
        .collect();
    let client = if records.is_empty() { None } else { connect_host_agent(ctx).ok() };
    let api = api_client().ok();
    for record in &records {
        logout_and_delete(client.as_ref(), api.as_ref(), record);
    }
    store().delete_generation(&generation);
    Ok(())
}

// --- callbacks ---------------------------------------------------------------------------------

fn connect_host_agent(ctx: &Ctx) -> Result<HostAgent, String> {
    let endpoint = getenv("OPUTE_HOST_AGENT_ENDPOINT").trim().to_string();
    if endpoint.is_empty() {
        return Err("OPUTE_HOST_AGENT_ENDPOINT is required for Tailscale provider callbacks".into());
    }
    let token = first_non_empty(&[&getenv("OPUTE_HOST_AGENT_BEARER_TOKEN"), &getenv("MCP_AUTH_TOKEN")]);
    HostAgent::connect(&endpoint, &token, &ctx.delegation)
}

/// `waitForGuestExec`: `true` in the guest every two seconds, for up to
/// five minutes, until it exits 0.
fn wait_for_guest_exec(client: &HostAgent, uri: &Uri) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(5 * 60);
    while Instant::now() < deadline {
        if let Ok(result) = client.call_host(
            "run_instance_command",
            json!({"uri": uri.to_string(), "command": "true", "timeoutMs": 30 * 1000}),
        ) {
            if let Some(content) = command_content(&result) {
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

// --- helpers --------------------------------------------------------------------------------------

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r"'\''"))
}

/// `sanitizeHostname`: lower case, every other rune `-`, outer `-` and `.`
/// trimmed.
pub(super) fn sanitize_hostname(name: &str) -> String {
    let lowered = name.trim().to_lowercase();
    let mapped: String = lowered
        .chars()
        .map(|c| if c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_' | '.') { c } else { '-' })
        .collect();
    mapped.trim_matches(['-', '.']).to_string()
}

/// `net.ParseIP(s).To4()`, printed as Go prints it.
fn ipv4_of(text: &str) -> Option<std::net::Ipv4Addr> {
    match text.parse::<std::net::IpAddr>().ok()? {
        std::net::IpAddr::V4(v4) => Some(v4),
        std::net::IpAddr::V6(v6) => v6.to_ipv4_mapped(),
    }
}

/// `findTailscaleIPv4`: the script's `mesh_ip=` line, else the first field
/// that is an IPv4 address in 100.0.0.0/8.
pub(super) fn find_tailscale_ipv4(output: &str) -> String {
    for line in output.split('\n') {
        if let Some(ip) = line.trim().strip_prefix("mesh_ip=") {
            let ip = ip.trim();
            if ipv4_of(ip).is_some() {
                return ip.to_string();
            }
        }
    }
    for field in output.split_whitespace() {
        let candidate = field.trim_matches(['(', ')', ',', '=']);
        if let Some(ip) = ipv4_of(candidate.split('/').next().unwrap_or("")) {
            let text = ip.to_string();
            if text.starts_with(CGNAT_PREFIX) {
                return text;
            }
        }
    }
    String::new()
}

fn find_dns_name(output: &str) -> String {
    for line in output.split('\n') {
        if let Some(name) = line.trim().strip_prefix("dns_name=") {
            let name = name.trim();
            return name.strip_suffix('.').unwrap_or(name).to_string();
        }
    }
    String::new()
}

/// `normalizeLoopbackHTTP`: a bare port (or service name) becomes a
/// loopback URL; a URL must be HTTP(S) on loopback.
pub(super) fn normalize_loopback_http(local_target: &str) -> Result<String, String> {
    let local_target = local_target.trim();
    if local_target.is_empty() {
        return Err("localTarget is required".into());
    }
    if !local_target.contains("://") {
        let digits = local_target.bytes().all(|b| b.is_ascii_digit());
        if super::super::lookup_port(local_target).is_ok() || digits {
            return Ok(format!("http://127.0.0.1:{local_target}"));
        }
        return Err("localTarget must be an HTTP URL or port".into());
    }
    let (scheme, host) = go_request_uri_host(local_target).ok_or_else(|| "localTarget must be an HTTP(S) URL".to_string())?;
    if scheme != "http" && scheme != "https" {
        return Err("localTarget must be an HTTP(S) URL".into());
    }
    if host != "127.0.0.1" && host != "localhost" && host != "::1" {
        return Err("localTarget must target loopback".into());
    }
    Ok(local_target.to_string())
}

/// `url.ParseRequestURI(raw)` then `Scheme` and `Hostname()`: the scheme
/// lowered, the host exactly as written, `None` where Go errors.
fn go_request_uri_host(raw: &str) -> Option<(String, String)> {
    if raw.bytes().any(|b| b < 0x20 || b == 0x7f) {
        return None;
    }
    // getScheme: a letter, then letters, digits, '+', '-' or '.', then ':'.
    let colon = raw.find(':')?;
    let scheme = &raw[..colon];
    let first_ok = scheme.bytes().next().is_some_and(|b| b.is_ascii_alphabetic());
    if !first_ok || !scheme.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.')) {
        return None;
    }
    let rest = raw[colon + 1..].split('?').next().unwrap_or("");
    let Some(after) = rest.strip_prefix("//") else {
        // An opaque URL (or an absolute path) has no host.
        return Some((scheme.to_ascii_lowercase(), String::new()));
    };
    let authority = after.split('/').next().unwrap_or("");
    let host = match authority.rfind('@') {
        Some(i) => {
            let userinfo = &authority[..i];
            let valid = userinfo.bytes().all(|b| {
                b.is_ascii_alphanumeric() || b"-._:~!$&'()*+,;=%@".contains(&b)
            });
            if !valid {
                return None;
            }
            &authority[i + 1..]
        }
        None => authority,
    };
    let port_ok = |p: &str| p.is_empty() || p.strip_prefix(':').is_some_and(|d| d.bytes().all(|b| b.is_ascii_digit()));
    let hostname = if let Some(inner) = host.strip_prefix('[') {
        let close = inner.find(']')?;
        if !port_ok(&inner[close + 1..]) {
            return None;
        }
        inner[..close].to_string()
    } else {
        let (name, port) = match host.rfind(':') {
            Some(i) => (&host[..i], &host[i..]),
            None => (host, ""),
        };
        if !port_ok(port) {
            return None;
        }
        let allowed = |b: u8| b >= 0x80 || b.is_ascii_alphanumeric() || b"-_.~!$&'()*+,;=:[]<>\"%".contains(&b);
        if !name.bytes().all(allowed) {
            return None;
        }
        name.to_string()
    };
    Some((scheme.to_ascii_lowercase(), hostname))
}

fn redact_secret(text: &str, secret: &str) -> String {
    if secret.is_empty() {
        text.to_string()
    } else {
        text.replace(secret, "[redacted]")
    }
}

const ENSURE_AGENT_SCRIPT: &str = r#"set -euo pipefail
if command -v tailscale >/dev/null 2>&1 && command -v tailscaled >/dev/null 2>&1; then
  if command -v systemctl >/dev/null 2>&1; then
    timeout 30s systemctl enable --now tailscaled >/dev/null 2>&1 || true
  fi
  if ! pgrep -x tailscaled >/dev/null 2>&1; then
    nohup tailscaled >/var/log/tailscaled.log 2>&1 &
    sleep 2
  fi
  ver="$(tailscale version 2>/dev/null | head -n1 | tr -d '\r' || true)"
  printf 'agent_ready=true\nagent_version=%s\n' "${ver:-unknown}"
  exit 0
fi
export DEBIAN_FRONTEND=noninteractive
if command -v apt-get >/dev/null 2>&1; then
  timeout 180s apt-get update -qq || true
  timeout 180s apt-get install -y -qq curl ca-certificates gnupg || true
fi
timeout 180s curl -fsSL https://tailscale.com/install.sh | sh
if command -v systemctl >/dev/null 2>&1; then
  timeout 30s systemctl enable --now tailscaled >/dev/null 2>&1 || true
fi
if ! pgrep -x tailscaled >/dev/null 2>&1; then
  nohup tailscaled >/var/log/tailscaled.log 2>&1 &
  sleep 2
fi
if ! command -v tailscale >/dev/null 2>&1 || ! command -v tailscaled >/dev/null 2>&1; then
  echo 'mesh-runtime.ensure-agent failed: tailscale/tailscaled not installed' >&2
  exit 1
fi
ver="$(tailscale version 2>/dev/null | head -n1 | tr -d '\r' || true)"
printf 'agent_ready=true\nagent_version=%s\n' "${ver:-unknown}"
"#;

const AGENT_STATUS_SCRIPT: &str = r#"set -euo pipefail
if ! command -v tailscale >/dev/null 2>&1 || ! command -v tailscaled >/dev/null 2>&1; then
  printf 'agent_ready=false\n'
  exit 0
fi
running=false
if pgrep -x tailscaled >/dev/null 2>&1; then
  running=true
fi
ver="$(tailscale version 2>/dev/null | head -n1 | tr -d '\r' || true)"
printf 'agent_ready=%s\nagent_version=%s\n' "$running" "${ver:-unknown}"
"#;

fn enroll_script(hostname: &str) -> String {
    format!(
        r#"set -euo pipefail
IFS= read -r auth_key || true
if [ -z "$auth_key" ]; then
  echo 'Tailscale auth key was not received on stdin' >&2
  exit 1
fi
if ! command -v tailscale >/dev/null 2>&1 || ! command -v tailscaled >/dev/null 2>&1; then
  echo 'mesh agent not installed; call opute.capability.mesh-runtime.ensure-agent first' >&2
  exit 1
fi
if ! pgrep -x tailscaled >/dev/null 2>&1; then
  echo 'tailscaled is not running; call opute.capability.mesh-runtime.ensure-agent first' >&2
  exit 1
fi
timeout 120s tailscale up --authkey="$auth_key" --hostname={} --accept-routes=false --reset
mesh_ip=""
for attempt in $(seq 1 60); do
  mesh_ip="$(tailscale ip -4 2>/dev/null | head -n1 | tr -d '[:space:]' || true)"
  if [ -n "$mesh_ip" ]; then
    break
  fi
  sleep 2
done
if [ -z "$mesh_ip" ]; then
  echo 'Tailscale did not assign an IPv4 address before the bounded wait' >&2
  tailscale status 2>&1 || true
  exit 1
fi
printf 'mesh_ip=%s\n' "$mesh_ip"
ip -4 -o addr show tailscale0 2>/dev/null || true
tailscale status 2>/dev/null || true
"#,
        shell_quote(hostname)
    )
}
