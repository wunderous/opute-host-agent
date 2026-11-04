//! The ownership store and the fake backend (Go `overlay.go`): memberships,
//! mesh edges, private services, public ingresses and mesh runtimes, owned
//! by this process and this provider generation.

use super::super::support::{first_non_empty, Args};
use super::{getenv, live, op, Ctx, Outcome, GENERATION, MESH_RUNTIME, NETWORK_OVERLAY, PLUGIN_ID};
use crate::goerr::quote;
use crate::resource::Uri;
use serde_json::{json, Map, Value as J};
use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard};

pub(super) const PATH_PRIVATE_MESH: &str = "private-mesh";
pub(super) const PATH_PUBLIC_INGRESS: &str = "public-ingress";
pub(super) const CRED_API_KEY: &str = "api-key";
pub(super) const CRED_AUTH_KEY: &str = "auth-key";
pub(super) const PROVIDER_NAME: &str = "network-overlay";

#[derive(Clone, Default)]
pub(super) struct Membership {
    pub reference: String,
    pub target_uri: String,
    pub overlay_uri: String,
    pub node_id: String,
    pub node_name: String,
    pub mesh_ip: String,
    pub mesh_interface: String,
    pub generation: String,
    pub attached: bool,
}

#[derive(Clone, Default)]
pub(super) struct MeshEdge {
    pub source_uri: String,
    pub peer_uri: String,
    pub peer_mesh_ip: String,
    pub ready: bool,
    pub generation: String,
}

#[derive(Clone)]
pub(super) struct PrivateService {
    pub target_uri: String,
    pub generation: String,
}

#[derive(Clone, Default)]
pub(super) struct PublicIngress {
    pub overlay_uri: String,
    pub target_uri: String,
    pub endpoint: String,
    pub stable: bool,
    pub generation: String,
    pub hostname: String,
}

#[derive(Clone, Default)]
pub(super) struct Runtime {
    pub target_uri: String,
    pub agent_ready: bool,
    pub control_plane_ready: bool,
    pub agent_version: String,
    pub control_plane_ref: String,
    pub ingress_class_name: String,
    pub generation: String,
}

#[derive(Default)]
pub(super) struct Store {
    pub memberships: HashMap<String, Membership>,
    pub by_target: HashMap<String, String>,
    pub mesh_edges: HashMap<String, MeshEdge>,
    pub services: HashMap<String, PrivateService>,
    pub ingresses: HashMap<String, PublicIngress>,
    pub runtimes: HashMap<String, Runtime>,
}

static STORE: Mutex<Option<Store>> = Mutex::new(None);

/// `ownershipStore.Lock()`.
pub(super) fn store() -> StoreGuard {
    let mut guard = STORE.lock().unwrap_or_else(|e| e.into_inner());
    guard.get_or_insert_with(Store::default);
    StoreGuard(guard)
}

pub(super) struct StoreGuard(MutexGuard<'static, Option<Store>>);

impl std::ops::Deref for StoreGuard {
    type Target = Store;
    fn deref(&self) -> &Store {
        self.0.as_ref().expect("store")
    }
}

impl std::ops::DerefMut for StoreGuard {
    fn deref_mut(&mut self) -> &mut Store {
        self.0.as_mut().expect("store")
    }
}

// --- dispatch ----------------------------------------------------------------------------------

fn use_fake_backend() -> bool {
    let backend = getenv("OPUTE_TAILSCALE_BACKEND");
    let backend = backend.trim();
    backend.is_empty() || backend.eq_ignore_ascii_case("fake")
}

/// `normalizeSeamOperation`: the three-role seam ids onto the
/// network-overlay operations both backends implement.
fn normalize(operation: &str) -> &str {
    match operation {
        op::MEMBERSHIP_ENROLL => op::OVERLAY_ENROLL,
        op::MEMBERSHIP_STATUS | op::PRIVATE_MESH_PROBE | op::PUBLIC_INGRESS_PROBE => {
            op::OVERLAY_PROBE
        }
        op::MEMBERSHIP_LEAVE => op::OVERLAY_REMOVE_MEMBERSHIP,
        op::PRIVATE_MESH_ENSURE => op::OVERLAY_ENSURE_PRIVATE_MESH,
        op::PRIVATE_MESH_ENSURE_SERVICE => op::OVERLAY_ENSURE_PRIVATE_SERVICE,
        op::PUBLIC_INGRESS_ENSURE => op::OVERLAY_ENSURE_PUBLIC_INGRESS,
        op::PUBLIC_INGRESS_PROMOTE => op::OVERLAY_PROMOTE_PUBLIC_INGRESS,
        other => other,
    }
}

pub(super) fn dispatch(ctx: &Ctx, original: &str, args: Args) -> Outcome {
    // A nil argument map is an empty one here.
    let mut args = if args.nil {
        Args::from_map(Map::new())
    } else {
        args
    };
    if matches!(
        original,
        op::MESH_RUNTIME_VALIDATE
            | op::MESH_RUNTIME_ENSURE_AGENT
            | op::MESH_RUNTIME_ENSURE_CONTROL_PLANE
            | op::MESH_RUNTIME_STATUS
    ) {
        return if use_fake_backend() {
            fake_mesh_runtime(original, &args)
        } else {
            live::mesh_runtime(ctx, original, &args)
        };
    }
    let operation = normalize(original);
    match original {
        op::MEMBERSHIP_STATUS if args.get("pathClass").is_none() => {
            args.set("pathClass", J::from(PATH_PRIVATE_MESH))
        }
        op::PRIVATE_MESH_PROBE => args.set("pathClass", J::from(PATH_PRIVATE_MESH)),
        op::PUBLIC_INGRESS_PROBE => args.set("pathClass", J::from(PATH_PUBLIC_INGRESS)),
        _ => {}
    }
    if use_fake_backend() {
        fake_overlay(operation, &args)
    } else {
        live::overlay(ctx, operation, &args)
    }
}

fn fake_overlay(operation: &str, args: &Args) -> Outcome {
    match operation {
        op::OVERLAY_VALIDATE => validate(args),
        op::OVERLAY_PREPARE_MEMBERSHIP => prepare_membership(args),
        op::OVERLAY_ATTACH_TARGET => attach_target(args),
        op::OVERLAY_ENROLL => enroll(args),
        op::OVERLAY_PROBE_REACHABILITY => {
            let mut args = args.cloned();
            if args.string("pathClass", "").is_empty() {
                args.set("pathClass", J::from(PATH_PRIVATE_MESH));
            }
            probe_path_aware(&args)
        }
        op::OVERLAY_PROBE => probe_path_aware(args),
        op::OVERLAY_REPORT_TWO_NODE_READINESS => report_two_node_readiness(args),
        op::OVERLAY_ENSURE_PRIVATE_MESH => ensure_private_mesh(args),
        op::OVERLAY_ENSURE_PRIVATE_SERVICE => ensure_private_service(args),
        op::OVERLAY_ENSURE_PUBLIC_INGRESS | op::OVERLAY_ENSURE_HA_ENDPOINT => {
            ensure_public_ingress(args)
        }
        op::OVERLAY_PROMOTE_PUBLIC_INGRESS => promote_public_ingress(args),
        op::OVERLAY_REMOVE_HA_ENDPOINT => remove_ha_endpoint(args),
        op::OVERLAY_REMOVE_MEMBERSHIP => remove_membership(args),
        _ => Err(format!(
            "unknown network overlay operation {}",
            quote(operation)
        )),
    }
}

// --- shared checks ------------------------------------------------------------------------

pub(super) fn require_host_agent_id(args: &Args) -> Result<String, String> {
    let host_id = args.string("hostAgentId", "").trim().to_string();
    if host_id.is_empty() {
        return Err(
            "hostAgentId is required and must be the exact opaque Host Agent identity".into(),
        );
    }
    if host_id.contains(['.', '/', ':']) {
        return Err(
            "hostAgentId must be an opaque Host Agent identity, not a network address".into(),
        );
    }
    Ok(host_id)
}

pub(super) fn parse_target_uri(args: &Args) -> Result<Uri, String> {
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

/// The peer's canonical VM or container URI.
pub(super) fn parse_peer_uri(args: &Args) -> Result<Uri, String> {
    let peer = Uri::parse(args.string("peerTargetUri", "").trim()).map_err(|e| {
        format!("peerTargetUri must be a canonical VM or container resource URI: {e}")
    })?;
    if peer.resource_type != "vm" && peer.resource_type != "container" {
        return Err("peerTargetUri requires resource type \"vm\" or \"container\"".into());
    }
    Ok(peer)
}

/// `net.ParseIP(s) != nil`: an IPv4 or IPv6 address.
pub(super) fn parses_as_ip(text: &str) -> bool {
    text.parse::<std::net::IpAddr>().is_ok()
}

pub(super) fn require_credential_kind(args: &Args, expected: &str) -> Result<String, String> {
    let kind = args.string("credentialKind", "").trim().to_string();
    if kind.is_empty() {
        if !expected.is_empty() {
            return Err("credentialKind is required".into());
        }
        // validate may omit the kind when no credential is supplied.
        if has_any_credential(args) {
            return Err("credentialKind is required when a credential is supplied".into());
        }
        return Ok(String::new());
    }
    if kind != CRED_API_KEY && kind != CRED_AUTH_KEY {
        return Err(format!("unsupported credentialKind {}", quote(&kind)));
    }
    if !expected.is_empty() && kind != expected {
        return Err(format!(
            "credential kind mismatch: operation requires {expected}"
        ));
    }
    Ok(kind)
}

pub(super) fn assert_credential_presence(args: &Args, kind: &str) -> Result<(), String> {
    let cred = args.string("credential", "");
    let api_key = args.string("apiKey", "");
    let auth_key = args.string("authKey", "");
    match kind {
        CRED_API_KEY => {
            if !auth_key.is_empty() && api_key.is_empty() {
                return Err("credential kind mismatch: api-key required, auth-key supplied".into());
            }
        }
        CRED_AUTH_KEY => {
            if !api_key.is_empty() && auth_key.is_empty() && cred.is_empty() {
                return Err("credential kind mismatch: auth-key required, api-key supplied".into());
            }
        }
        _ => {}
    }
    Ok(())
}

fn has_any_credential(args: &Args) -> bool {
    !args.string("credential", "").is_empty()
        || !args.string("apiKey", "").is_empty()
        || !args.string("authKey", "").is_empty()
}

pub(super) fn reject_control_plane_public_target(
    local_target: &str,
    target_kind: &str,
) -> Result<(), String> {
    let combined = format!("{local_target} {target_kind}").to_lowercase();
    for forbidden in ["etcd", "k8s-api", "kubernetes-api", "cni", "host-admin"] {
        if combined.contains(forbidden) {
            return Err(format!(
                "public ingress rejects control-plane target {}",
                quote(forbidden)
            ));
        }
    }
    if combined.contains("remote://") {
        return Err("public ingress rejects remote arbitrary targets".into());
    }
    Ok(())
}

/// `requireOperatorStableEvidence`: a stable operator claim needs evidence
/// and an operator-managed endpoint that is not a node-local Funnel name.
pub(super) fn require_operator_stable_evidence(args: &Args) -> Result<(), String> {
    if !args.bool("operatorMode", false) {
        return Ok(());
    }
    let evidence = args.string("operatorEvidence", "");
    let ingress_class = args.string("ingressClassName", "");
    let endpoint = args.string("endpoint", &args.string("hostname", ""));
    if evidence.is_empty() && ingress_class != "tailscale" {
        return Err("operatorMode/stable=true requires operatorEvidence or ingressClassName=tailscale with an operator-managed endpoint".into());
    }
    if endpoint.is_empty() {
        return Err(
            "operatorMode requires endpoint/hostname for the operator-managed public URL".into(),
        );
    }
    let lower = endpoint.to_lowercase();
    if lower.contains("opute-ha-a.") || lower.contains("opute-ha-b.") {
        return Err(format!(
            "operatorMode refuses node-local Funnel hostname {} as a stable cluster endpoint",
            quote(&endpoint)
        ));
    }
    Ok(())
}

pub(super) fn new_opaque_ref(prefix: &str) -> Result<String, String> {
    let mut raw = [0u8; 12];
    getrandom::getrandom(&mut raw).map_err(|e| format!("create opaque reference: {e}"))?;
    Ok(format!("{prefix}-{}", hex(&raw)))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn sha256(text: &str) -> [u8; 32] {
    use sha2::Digest as _;
    sha2::Sha256::digest(text.as_bytes()).into()
}

pub(super) fn mesh_key(source: &str, peer: &str) -> String {
    format!("{source}->{peer}")
}

/// The fake backend's deterministic address for a target.
fn allocate_mesh_ip(target_uri: &str) -> String {
    let sum = sha256(target_uri);
    format!(
        "100.64.{}.{}",
        sum[0] as u32 % 250 + 1,
        sum[1] as u32 % 250 + 1
    )
}

/// `overlayBase`: the membership's identity, then the operation's fields.
pub(super) fn overlay_base(record: &Membership, extra: J) -> J {
    let mut out = json!({
        "contractVersion": NETWORK_OVERLAY,
        "provider": PROVIDER_NAME,
        "generation": GENERATION,
        "targetUri": record.target_uri,
        "overlayUri": record.overlay_uri,
    });
    if let (Some(map), J::Object(extra)) = (out.as_object_mut(), extra) {
        map.extend(extra);
    }
    out
}

pub(super) fn mesh_runtime_base(target_uri: &str, rec: &Runtime, ready: bool) -> J {
    json!({
        "contractVersion": MESH_RUNTIME,
        "provider": PLUGIN_ID,
        "targetUri": target_uri,
        "agentReady": rec.agent_ready,
        "controlPlaneReady": rec.control_plane_ready,
        "agentVersion": rec.agent_version,
        "controlPlaneRef": rec.control_plane_ref,
        "ingressClassName": rec.ingress_class_name,
        "generation": first_non_empty(&[&rec.generation, GENERATION]),
        "probe": {"agentReady": rec.agent_ready, "controlPlaneReady": rec.control_plane_ready},
        "ready": ready,
    })
}

pub(super) fn require_mesh_agent_ready(target_uri: &str) -> Result<(), String> {
    if !store()
        .runtimes
        .get(target_uri)
        .is_some_and(|r| r.agent_ready)
    {
        return Err(format!(
            "mesh agent not ready on {target_uri}; call opute.capability.mesh-runtime.ensure-agent first"
        ));
    }
    Ok(())
}

// --- store operations ------------------------------------------------------------------

impl Store {
    pub(super) fn upsert_membership(
        &mut self,
        target_uri: &str,
        name: &str,
        attached: bool,
    ) -> Result<Membership, String> {
        if let Some(existing) = self.by_target.get(target_uri).cloned() {
            let record = self.memberships.entry(existing).or_default();
            if record.generation != GENERATION {
                return Err("target already owned by a foreign generation".into());
            }
            if !name.is_empty() {
                record.node_name = name.to_string();
            }
            if attached {
                record.attached = true;
                if record.mesh_ip.is_empty() {
                    record.mesh_ip = allocate_mesh_ip(target_uri);
                    record.mesh_interface = "tailscale0".into();
                }
            }
            return Ok(record.clone());
        }
        let reference = new_opaque_ref("mesh")?;
        let node_id = new_opaque_ref("node")?;
        let mut record = Membership {
            reference: reference.clone(),
            target_uri: target_uri.to_string(),
            overlay_uri: format!("overlay:local:{}", hex(&sha256(target_uri)[..8])),
            node_id,
            node_name: name.to_string(),
            generation: GENERATION.into(),
            attached,
            ..Membership::default()
        };
        if attached {
            record.mesh_ip = allocate_mesh_ip(target_uri);
            record.mesh_interface = "tailscale0".into();
        }
        self.memberships.insert(reference.clone(), record.clone());
        self.by_target.insert(target_uri.to_string(), reference);
        Ok(record)
    }

    pub(super) fn require_owned_membership(
        &self,
        args: &Args,
        target_uri: &str,
    ) -> Result<Membership, String> {
        let hint = args.string("membershipRef", "");
        let reference = self
            .by_target
            .get(target_uri)
            .ok_or_else(|| "target is not enrolled".to_string())?;
        if !hint.is_empty() && &hint != reference {
            return Err("membershipRef does not match owned enrollment".into());
        }
        let record = self.memberships.get(reference).cloned().unwrap_or_default();
        if record.generation != GENERATION {
            return Err("stale provider generation for membership".into());
        }
        if !record.attached {
            return Err("membership is not attached".into());
        }
        Ok(record)
    }

    pub(super) fn private_mesh_ready(&self, source_uri: &str, peer_mesh_ip: &str) -> bool {
        for edge in self.mesh_edges.values() {
            if edge.generation == GENERATION
                && edge.source_uri == source_uri
                && edge.peer_mesh_ip == peer_mesh_ip
                && edge.ready
            {
                // The reverse edge is required too.
                let reverse = self
                    .mesh_edges
                    .get(&mesh_key(&edge.peer_uri, &edge.source_uri))
                    .cloned()
                    .unwrap_or_default();
                return reverse.ready && reverse.generation == GENERATION;
            }
        }
        false
    }

    pub(super) fn public_ready(&self, target_uri: &str) -> bool {
        self.ingresses.values().any(|i| {
            i.target_uri == target_uri && i.generation == GENERATION && !i.endpoint.is_empty()
        })
    }

    /// The first owned ingress of the target, in map order as Go's range.
    pub(super) fn ingress_of(&self, target_uri: &str) -> Option<&PublicIngress> {
        self.ingresses
            .values()
            .find(|i| i.target_uri == target_uri && i.generation == GENERATION)
    }

    fn count_peers_with_mesh_ip(&self, mesh_ip: &str) -> usize {
        self.memberships
            .values()
            .filter(|r| r.mesh_ip == mesh_ip && r.generation == GENERATION)
            .count()
    }

    pub(super) fn set_edges(&mut self, source: &str, source_ip: &str, peer: &str, peer_ip: &str) {
        let edge = |s: &str, p: &str, ip: &str| MeshEdge {
            source_uri: s.into(),
            peer_uri: p.into(),
            peer_mesh_ip: ip.into(),
            ready: true,
            generation: GENERATION.into(),
        };
        self.mesh_edges
            .insert(mesh_key(source, peer), edge(source, peer, peer_ip));
        self.mesh_edges
            .insert(mesh_key(peer, source), edge(peer, source, source_ip));
    }

    pub(super) fn delete_owned_for_target(&mut self, target_uri: &str, reference: &str) {
        self.memberships.remove(reference);
        if self
            .by_target
            .get(target_uri)
            .is_some_and(|r| r == reference)
        {
            self.by_target.remove(target_uri);
        }
        self.mesh_edges.retain(|_, e| {
            !(e.generation == GENERATION
                && (e.source_uri == target_uri || e.peer_uri == target_uri))
        });
        self.services
            .retain(|_, s| !(s.generation == GENERATION && s.target_uri == target_uri));
        self.ingresses
            .retain(|_, i| !(i.generation == GENERATION && i.target_uri == target_uri));
    }

    pub(super) fn delete_generation(&mut self, generation: &str) {
        let owned: Vec<(String, String)> = self
            .memberships
            .iter()
            .filter(|(_, r)| r.generation == generation)
            .map(|(k, r)| (k.clone(), r.target_uri.clone()))
            .collect();
        for (reference, target) in owned {
            self.delete_owned_for_target(&target, &reference);
        }
    }
}

fn membership_fields(record: &Membership, with_mesh: bool) -> J {
    let mut out = json!({
        "ready": true,
        "membershipRef": record.reference,
        "nodeId": record.node_id,
        "nodeName": record.node_name,
        "overlayUri": record.overlay_uri,
    });
    if with_mesh {
        out["meshIp"] = J::from(record.mesh_ip.as_str());
        out["meshInterface"] = J::from(record.mesh_interface.as_str());
    }
    out
}

pub(super) fn enrolled(record: &Membership) -> J {
    overlay_base(record, membership_fields(record, true))
}

pub(super) fn prepared(record: &Membership) -> J {
    overlay_base(record, membership_fields(record, false))
}

// --- the fake backend -------------------------------------------------------------------

fn validate(args: &Args) -> Outcome {
    let uri = parse_target_uri(args)?;
    let kind = require_credential_kind(args, "")?;
    assert_credential_presence(args, &kind)?;
    let node_count = store().memberships.len();
    Ok(json!({
        "contractVersion": NETWORK_OVERLAY,
        "ready": true,
        "provider": PROVIDER_NAME,
        "generation": GENERATION,
        "targetUri": uri.to_string(),
        "nodeCount": node_count,
        "credentialKind": kind,
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
    let record = store().upsert_membership(&uri.to_string(), &name, false)?;
    Ok(prepared(&record))
}

fn attach_target(args: &Args) -> Outcome {
    require_host_agent_id(args)?;
    let uri = parse_target_uri(args)?;
    let reference = args.string("membershipRef", "");
    if reference.is_empty() {
        return Err("membershipRef is required".into());
    }
    let mut store = store();
    let mut record =
        store.memberships.get(&reference).cloned().ok_or_else(|| {
            "network overlay membership reference is unknown or expired".to_string()
        })?;
    if record.generation != GENERATION {
        return Err("stale provider generation for membership".into());
    }
    if record.target_uri != uri.to_string() {
        return Err("network overlay membership is bound to another instance".into());
    }
    record.attached = true;
    if record.mesh_ip.is_empty() {
        record.mesh_ip = allocate_mesh_ip(&record.target_uri);
        record.mesh_interface = "tailscale0".into();
    }
    store.memberships.insert(reference.clone(), record.clone());
    store.by_target.insert(record.target_uri.clone(), reference);
    Ok(overlay_base(
        &record,
        json!({
            "ready": true,
            "membershipRef": record.reference,
            "nodeId": record.node_id,
            "meshIp": record.mesh_ip,
            "meshInterface": record.mesh_interface,
            "overlayUri": record.overlay_uri,
        }),
    ))
}

fn enroll(args: &Args) -> Outcome {
    require_host_agent_id(args)?;
    let uri = parse_target_uri(args)?;
    require_credential_kind(args, CRED_AUTH_KEY)?;
    assert_credential_presence(args, CRED_AUTH_KEY)?;
    let target = uri.to_string();
    require_mesh_agent_ready(&target)?;
    let name = first_non_empty(&[&args.string("name", ""), "node"]);
    let hint = args.string("membershipRef", "");
    let mut store = store();
    if let Some(existing) = store.by_target.get(&target).cloned() {
        let mut record = store
            .memberships
            .get(&existing)
            .cloned()
            .unwrap_or_default();
        if record.generation != GENERATION {
            return Err("stale provider generation for membership".into());
        }
        if !hint.is_empty() && hint != existing {
            return Err("membershipRef does not match owned enrollment for target".into());
        }
        record.attached = true;
        if record.mesh_ip.is_empty() {
            record.mesh_ip = allocate_mesh_ip(&record.target_uri);
            record.mesh_interface = "tailscale0".into();
        }
        if record.node_name.is_empty() {
            record.node_name = name;
        }
        store.memberships.insert(existing, record.clone());
        return Ok(enrolled(&record));
    }
    let record = store.upsert_membership(&target, &name, true)?;
    Ok(enrolled(&record))
}

/// The probe result before its path-specific fields.
pub(super) fn probe_result(record: &Membership, path_class: &str) -> (J, Map<String, J>) {
    let result = overlay_base(record, json!({"pathClass": path_class}));
    let mut probe = Map::new();
    probe.insert("pathClass".into(), J::from(path_class));
    (result, probe)
}

pub(super) fn path_class(args: &Args) -> Result<String, String> {
    let class = args.string("pathClass", "");
    if class != PATH_PRIVATE_MESH && class != PATH_PUBLIC_INGRESS {
        return Err(format!(
            "pathClass must be {} or {}",
            quote(PATH_PRIVATE_MESH),
            quote(PATH_PUBLIC_INGRESS)
        ));
    }
    Ok(class)
}

fn probe_path_aware(args: &Args) -> Outcome {
    let uri = parse_target_uri(args)?;
    let target = uri.to_string();
    let class = path_class(args)?;
    let peer_mesh_ip = args.string("peerMeshIp", "");
    let store = store();
    let reference = store
        .by_target
        .get(&target)
        .ok_or_else(|| "target is not enrolled in the overlay".to_string())?;
    let record = store
        .memberships
        .get(reference)
        .cloned()
        .unwrap_or_default();
    if record.generation != GENERATION {
        return Err("stale provider generation for membership".into());
    }
    let (mut result, mut probe) = probe_result(&record, &class);
    if class == PATH_PRIVATE_MESH {
        if peer_mesh_ip.is_empty() {
            return Err("peerMeshIp is required for private-mesh probes".into());
        }
        if !parses_as_ip(&peer_mesh_ip) {
            return Err("peerMeshIp must be an IPv4 address".into());
        }
        let ready = store.private_mesh_ready(&target, &peer_mesh_ip);
        result["ready"] = J::Bool(ready);
        result["peerMeshIp"] = J::from(peer_mesh_ip.as_str());
        probe.insert("ready".into(), J::Bool(ready));
        probe.insert("peerMeshIp".into(), J::from(peer_mesh_ip.as_str()));
        if !ready {
            // Public ingress presence never satisfies a private probe.
            if store.public_ready(&target) {
                probe.insert("publicIngressPresent".into(), J::Bool(true));
                probe.insert("privateSatisfiedByPublic".into(), J::Bool(false));
            }
            result["error"] = J::from("private-mesh peer reachability is not ready");
        }
    } else {
        let ready = store.public_ready(&target);
        result["ready"] = J::Bool(ready);
        result["stable"] = J::Bool(false);
        if ready {
            if let Some(ingress) = store.ingress_of(&target) {
                result["endpoint"] = J::from(ingress.endpoint.as_str());
                result["stable"] = J::Bool(ingress.stable);
            }
        }
        probe.insert("ready".into(), J::Bool(ready));
        if !ready {
            result["error"] = J::from("public-ingress endpoint is not ready");
        }
    }
    result["probe"] = J::Object(probe);
    Ok(result)
}

pub(super) fn report_two_node_readiness(args: &Args) -> Outcome {
    require_host_agent_id(args)?;
    let uri = parse_target_uri(args)?;
    let peer_uri = args.string("peerTargetUri", "");
    if peer_uri.is_empty() {
        return Err("peerTargetUri is required".into());
    }
    let datastore_mode = args.string("datastoreMode", "");
    if datastore_mode != "external-datastore" && datastore_mode != "embedded-etcd-two-node" {
        return Err("datastoreMode must be external-datastore or embedded-etcd-two-node".into());
    }
    let store = store();
    let record = store.require_owned_membership(args, &uri.to_string())?;
    let peer_ref = store
        .by_target
        .get(&peer_uri)
        .ok_or_else(|| "peer target is not enrolled".to_string())?;
    let peer = store.memberships.get(peer_ref).cloned().unwrap_or_default();
    let edge = |a: &str, b: &str| {
        store
            .mesh_edges
            .get(&mesh_key(a, b))
            .is_some_and(|e| e.ready)
    };
    let mesh_ready =
        edge(&record.target_uri, &peer.target_uri) && edge(&peer.target_uri, &record.target_uri);
    let mut funnel_stable = None;
    let mut public_ready = false;
    for ingress in store.ingresses.values() {
        if ingress.generation != GENERATION {
            continue;
        }
        if ingress.target_uri == record.target_uri || ingress.target_uri == peer.target_uri {
            public_ready = true;
            funnel_stable = Some(ingress.stable);
        }
    }
    let (mut availability, mut recovery, mut api_write_ready, mut durable_ready) = (
        "serving-continuity-only",
        "acknowledge-single-member-reset",
        false,
        false,
    );
    if datastore_mode == "external-datastore" {
        if !args.bool("datastoreEvidence", false) {
            return Err("external-datastore mode requires datastoreEvidence=true with independent datastore proof".into());
        }
        availability = "control-plane-write-continuity";
        recovery = "datastore-failover";
        api_write_ready = mesh_ready;
        durable_ready = true;
    }
    let ready = mesh_ready && (datastore_mode != "external-datastore" || durable_ready);
    let mut out = overlay_base(
        &record,
        json!({
            "ready": ready,
            "datastoreMode": datastore_mode,
            "availabilityClass": availability,
            "recoveryPolicy": recovery,
            "membership": {"ready": true, "sourceUri": record.target_uri, "destUri": peer.target_uri, "generation": GENERATION},
            "privateMesh": {"ready": mesh_ready, "pathClass": PATH_PRIVATE_MESH, "sourceUri": record.target_uri, "destUri": peer.target_uri, "generation": GENERATION},
            "publicIngress": {"ready": public_ready, "pathClass": PATH_PUBLIC_INGRESS, "generation": GENERATION},
            "durableStore": {"ready": durable_ready, "generation": GENERATION},
            "apiWriteAvailable": {"ready": api_write_ready, "generation": GENERATION},
        }),
    );
    if let Some(stable) = funnel_stable {
        out["funnelStable"] = J::Bool(stable);
    }
    if !mesh_ready {
        out["error"] = J::from("private mesh is not ready in both directions");
    }
    Ok(out)
}

pub(super) fn private_mesh_result(source: &Membership, peer_ip: &str, args: &Args) -> J {
    overlay_base(
        source,
        json!({
            "ready": true,
            "pathClass": PATH_PRIVATE_MESH,
            "peerMeshIp": peer_ip,
            "overlayUri": source.overlay_uri,
            "datastoreMode": args.string("datastoreMode", "external-datastore"),
            "availabilityClass": args.string("availabilityClass", "serving-continuity"),
            "recoveryPolicy": args.string("recoveryPolicy", "explicit-acknowledgement"),
            "bothDirectionsReady": true,
        }),
    )
}

fn ensure_private_mesh(args: &Args) -> Outcome {
    require_host_agent_id(args)?;
    let uri = parse_target_uri(args)?;
    let peer_uri = parse_peer_uri(args)?;
    let peer_mesh_ip = args.string("peerMeshIp", "");
    if !parses_as_ip(&peer_mesh_ip) {
        return Err("peerMeshIp must be an IPv4 address".into());
    }
    let (target, peer_target) = (uri.to_string(), peer_uri.to_string());
    let mut store = store();
    let source_ref = store
        .by_target
        .get(&target)
        .ok_or_else(|| "source target is not enrolled".to_string())?;
    let source = store
        .memberships
        .get(source_ref)
        .cloned()
        .unwrap_or_default();
    if source.generation != GENERATION || !source.attached {
        return Err("source membership is not attached for this generation".into());
    }
    let peer_ref = store
        .by_target
        .get(&peer_target)
        .ok_or_else(|| "peer target is not enrolled".to_string())?;
    let peer = store.memberships.get(peer_ref).cloned().unwrap_or_default();
    if peer.generation != GENERATION || !peer.attached {
        return Err("peer membership is not attached for this generation".into());
    }
    if peer.mesh_ip != peer_mesh_ip {
        return Err("ambiguous peer: peerMeshIp does not match enrolled peer".into());
    }
    if store.count_peers_with_mesh_ip(&peer_mesh_ip) != 1 {
        return Err("ambiguous peer mesh address".into());
    }
    store.set_edges(&target, &source.mesh_ip, &peer_target, &peer.mesh_ip);
    Ok(private_mesh_result(&source, &peer.mesh_ip, args))
}

pub(super) fn private_service_result(record: &Membership, local_target: &str) -> J {
    overlay_base(
        record,
        json!({"ready": true, "pathClass": PATH_PRIVATE_MESH, "endpoint": local_target, "stable": true}),
    )
}

fn ensure_private_service(args: &Args) -> Outcome {
    require_host_agent_id(args)?;
    let uri = parse_target_uri(args)?;
    let local_target = args.string("localTarget", "");
    if local_target.is_empty() {
        return Err("localTarget is required".into());
    }
    let mut store = store();
    let record = store.require_owned_membership(args, &uri.to_string())?;
    store.services.insert(
        record.overlay_uri.clone(),
        PrivateService {
            target_uri: record.target_uri.clone(),
            generation: GENERATION.into(),
        },
    );
    Ok(private_service_result(&record, &local_target))
}

pub(super) fn insert_ingress(
    store: &mut Store,
    record: &Membership,
    endpoint: &str,
    stable: bool,
    hostname: &str,
) -> Result<String, String> {
    let endpoint_ref = new_opaque_ref("ingress")?;
    store.ingresses.insert(
        endpoint_ref.clone(),
        PublicIngress {
            overlay_uri: record.overlay_uri.clone(),
            target_uri: record.target_uri.clone(),
            endpoint: endpoint.into(),
            stable,
            generation: GENERATION.into(),
            hostname: hostname.into(),
        },
    );
    Ok(endpoint_ref)
}

fn ensure_public_ingress(args: &Args) -> Outcome {
    require_host_agent_id(args)?;
    let uri = parse_target_uri(args)?;
    let local_target = args.string("localTarget", "");
    if local_target.is_empty() {
        return Err("localTarget is required".into());
    }
    reject_control_plane_public_target(&local_target, &args.string("targetKind", ""))?;
    let mut store = store();
    let record = store.require_owned_membership(args, &uri.to_string())?;
    let operator_mode = args.bool("operatorMode", false);
    require_operator_stable_evidence(args)?;
    let hostname = first_non_empty(&[&args.string("hostname", ""), "node.ingress.example"]);
    let endpoint = format!("https://{hostname}");
    let endpoint_ref = insert_ingress(&mut store, &record, &endpoint, operator_mode, &hostname)?;
    Ok(overlay_base(
        &record,
        json!({
            "ready": true,
            "pathClass": PATH_PUBLIC_INGRESS,
            "endpoint": endpoint,
            "endpointRef": endpoint_ref,
            "stable": operator_mode,
        }),
    ))
}

/// `promotePublicIngress`, shared by both backends (the live one checks the
/// operator evidence first).
pub(super) fn promote(args: &Args, check_evidence: bool) -> Outcome {
    require_host_agent_id(args)?;
    let uri = parse_target_uri(args)?;
    let endpoint_ref = args.string("endpointRef", "");
    if endpoint_ref.is_empty() {
        return Err("endpointRef is required".into());
    }
    let mut store = store();
    let mut ingress = store
        .ingresses
        .get(&endpoint_ref)
        .cloned()
        .ok_or_else(|| "endpoint reference is unknown".to_string())?;
    if ingress.generation != GENERATION {
        return Err("stale provider generation for endpoint".into());
    }
    let record = store.require_owned_membership(args, &uri.to_string())?;
    let operator_mode = args.bool("operatorMode", false);
    if check_evidence {
        require_operator_stable_evidence(args)?;
    }
    ingress.target_uri = record.target_uri.clone();
    ingress.overlay_uri = record.overlay_uri.clone();
    ingress.endpoint = format!(
        "https://{}/promoted",
        first_non_empty(&[&ingress.hostname, "node.ingress.example"])
    );
    ingress.stable = operator_mode;
    let endpoint = ingress.endpoint.clone();
    store.ingresses.insert(endpoint_ref.clone(), ingress);
    Ok(overlay_base(
        &record,
        json!({
            "ready": true,
            "pathClass": PATH_PUBLIC_INGRESS,
            "endpoint": endpoint,
            "endpointRef": endpoint_ref,
            "stable": operator_mode,
        }),
    ))
}

fn promote_public_ingress(args: &Args) -> Outcome {
    promote(args, false)
}

pub(super) fn ha_endpoint_removed(target_uri: &str, endpoint_ref: Option<&str>) -> J {
    let mut out = json!({
        "contractVersion": NETWORK_OVERLAY,
        "ready": true,
        "provider": PROVIDER_NAME,
        "generation": GENERATION,
        "deleted": true,
        "targetUri": target_uri,
    });
    if let Some(reference) = endpoint_ref {
        out["endpointRef"] = J::from(reference);
    }
    out
}

fn remove_ha_endpoint(args: &Args) -> Outcome {
    let endpoint_ref = args.string("endpointRef", "");
    if endpoint_ref.is_empty() {
        return Err("endpointRef is required".into());
    }
    let mut store = store();
    let Some(ingress) = store.ingresses.get(&endpoint_ref).cloned() else {
        return Ok(ha_endpoint_removed("vm:local:placeholder", None));
    };
    if ingress.generation != GENERATION {
        return Err("refusing to delete endpoint owned by a foreign generation".into());
    }
    store.ingresses.remove(&endpoint_ref);
    Ok(ha_endpoint_removed(
        &ingress.target_uri,
        Some(&endpoint_ref),
    ))
}

pub(super) fn membership_removed(reference: &str, target_uri: &str) -> J {
    json!({
        "contractVersion": NETWORK_OVERLAY,
        "ready": true,
        "provider": PROVIDER_NAME,
        "generation": GENERATION,
        "deleted": true,
        "membershipRef": reference,
        "targetUri": target_uri,
    })
}

/// The owned membership a leave names, checked against its target.
pub(super) fn owned_for_removal(args: &Args) -> Result<(Uri, String, Membership), String> {
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
        return Err("refusing to remove membership owned by a foreign generation".into());
    }
    if record.target_uri != uri.to_string() {
        return Err("network overlay membership is bound to another instance".into());
    }
    Ok((uri, reference, record))
}

fn remove_membership(args: &Args) -> Outcome {
    let (uri, reference, record) = owned_for_removal(args)?;
    store().delete_owned_for_target(&record.target_uri, &reference);
    Ok(membership_removed(&reference, &uri.to_string()))
}

/// The teardown's finalize phase: everything this generation owns.
pub(super) fn finalize_owned_teardown(ctx: &Ctx, inputs: &Map<String, J>) -> Result<(), String> {
    if !use_fake_backend() {
        return live::finalize_owned_teardown(ctx, inputs);
    }
    let generation = first_non_empty(&[&super::string_input(inputs, "generation", ""), GENERATION]);
    if generation != GENERATION {
        return Err("refusing teardown for foreign provider generation".into());
    }
    store().delete_generation(&generation);
    Ok(())
}

// --- the fake mesh runtime -----------------------------------------------------------------

fn fake_mesh_runtime(operation: &str, args: &Args) -> Outcome {
    require_host_agent_id(args)?;
    let uri = parse_target_uri(args)?;
    let target = uri.to_string();
    match operation {
        op::MESH_RUNTIME_ENSURE_AGENT => {
            let mut store = store();
            let rec = store.runtimes.entry(target.clone()).or_default();
            rec.target_uri = target.clone();
            rec.agent_ready = true;
            rec.agent_version = "fake-tailscale".into();
            rec.generation = GENERATION.into();
            Ok(mesh_runtime_base(&target, rec, true))
        }
        op::MESH_RUNTIME_ENSURE_CONTROL_PLANE => {
            let ingress_class =
                first_non_empty(&[&args.string("ingressClassName", ""), "tailscale"]);
            let evidence = args.string("operatorEvidence", "");
            let operator_mode = args.bool("operatorMode", true);
            let mut store = store();
            if !store.runtimes.get(&target).is_some_and(|r| r.agent_ready) {
                return Err(
                    "mesh-runtime.ensure-control-plane requires mesh-runtime.ensure-agent first"
                        .into(),
                );
            }
            if operator_mode && evidence.is_empty() && ingress_class != "tailscale" {
                return Err(
                    "operatorMode requires operatorEvidence or ingressClassName=tailscale".into(),
                );
            }
            let rec = store.runtimes.entry(target.clone()).or_default();
            rec.target_uri = target.clone();
            rec.control_plane_ready = true;
            rec.ingress_class_name = ingress_class;
            rec.control_plane_ref = first_non_empty(&[
                &args.string("controlPlaneRef", ""),
                "fake-tailscale-operator",
            ]);
            rec.generation = GENERATION.into();
            Ok(mesh_runtime_base(&target, rec, true))
        }
        _ => {
            let rec = store().runtimes.get(&target).cloned().unwrap_or_default();
            let ready = rec.agent_ready
                && (rec.control_plane_ready || !args.bool("requireControlPlane", false));
            Ok(mesh_runtime_base(&target, &rec, ready))
        }
    }
}
