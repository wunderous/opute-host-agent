//! The optional cluster endpoint (Q19, decided by the owner on 2026-11-08;
//! OpenSpec change `typed-ha-endpoint`).
//!
//! ```text
//!   OPUTE_K3S_CLUSTER_ENDPOINT=enabled   publishes ensure/remove-cluster-endpoint
//!                                        and ensure-ha-endpoint's caBundleRef
//!   otherwise                            catalog unchanged; calls refused with
//!                                        capability_unavailable; no hook runs
//!
//!   opt in   tls-san drop-in on each Ready etcd server, restarted one at a
//!            time; host-record lines in the bridge's raw.dnsmasq; the record
//!            (kube-system ConfigMap opute-cluster-endpoint)
//!   opt out  drop-in, k3s-serving secret and dynamic-cert.json removed, each
//!            server restarted; DNS block and record removed
//!   hooks    join / remove-node / quorum recovery update the DNS block and
//!            record (join also the new server's TLS name) when a record exists
//! ```
//!
//! Every Incus call goes through an [`Incus`] runner, so the tests script the
//! cluster and see every effect. The CA never leaves the servers: results
//! carry `cluster-endpoint:<host>:<cluster>`, which `ensure-ha-endpoint`
//! resolves by reading `server-ca.crt` from a Ready member.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value as J};

use super::super::support::Args;

pub(super) const ENV: &str = "OPUTE_K3S_CLUSTER_ENDPOINT";
pub(super) const DROP_IN: &str = "/etc/rancher/k3s/config.yaml.d/50-opute-cluster-endpoint.yaml";
pub(super) const RECORD: &str = "opute-cluster-endpoint";
const SERVER_CA: &str = "/var/lib/rancher/k3s/server/tls/server-ca.crt";
const DYNAMIC_CERT: &str = "/var/lib/rancher/k3s/server/tls/dynamic-cert.json";
const API_PORT: u16 = 6443;
const PROBE_TIMEOUT: Duration = Duration::from_secs(20);

#[cfg(test)]
use super::super::TYPED;

/// One `incus <args>` call: stdout, or the failure text.
pub(super) type Incus<'a> = &'a mut dyn FnMut(&[&str], Option<&[u8]>) -> Result<Vec<u8>, String>;

/// How long a restarted server may take to be Ready again, and how a
/// rollout sees that it serves the TLS name.
#[derive(Clone, Copy)]
pub(super) struct Rollout {
    pub attempts: u32,
    pub poll: Duration,
    /// Whether the server at `address` presents a certificate the cluster CA
    /// signs for `hostname`.
    pub serves: fn(hostname: &str, address: &str, ca: &[u8]) -> bool,
}

pub(super) const ROLLOUT: Rollout = Rollout {
    attempts: 60,
    poll: Duration::from_secs(3),
    serves: serves_name,
};

/// A TLS handshake with the server's own address under `hostname`, trusting
/// only the cluster CA: it succeeds only for a certificate listing the name.
fn serves_name(hostname: &str, address: &str, ca: &[u8]) -> bool {
    probe(hostname, &[address.to_string()], ca).is_ok()
}

/// What a restart must show besides a Ready Node: the hostname a server must
/// serve, its address, and the cluster CA.
type Serving<'a> = Option<(&'a str, &'a str, &'a [u8])>;

#[cfg(test)]
thread_local! {
    /// Tests set the deployment switch per thread.
    pub(super) static ENABLED_OVERRIDE: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

pub(super) fn enabled() -> bool {
    #[cfg(test)]
    if let Some(value) = ENABLED_OVERRIDE.with(|o| o.get()) {
        return value;
    }
    std::env::var(ENV).as_deref() == Ok("enabled")
}

/// A typed error: `structuredContent.code` and its fields, before any effect
/// when the caller says so.
pub(super) fn typed(code: &str, message: &str, fields: J) -> String {
    super::super::typed_error(code, message, fields)
}

pub(super) fn unavailable(operation: &str) -> String {
    typed(
        "capability_unavailable",
        &format!("{operation} is not enabled on this host ({ENV} is not \"enabled\")"),
        json!({}),
    )
}

/// An explicit DNS hostname: lowercase labels, at least two, at most 253
/// characters. Nothing else is ever used as a name.
pub(super) fn valid_hostname(name: &str) -> bool {
    let label = |l: &str| {
        !l.is_empty()
            && l.len() <= 63
            && l.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            && !l.starts_with('-')
            && !l.ends_with('-')
    };
    name.len() <= 253 && name.split('.').count() >= 2 && name.split('.').all(label)
}

/// `cluster:<host>:<name>` -> `cluster-endpoint:<host>:<name>`.
pub(super) fn ca_reference(target_uri: &str) -> String {
    format!(
        "cluster-endpoint:{}",
        target_uri.strip_prefix("cluster:").unwrap_or(target_uri)
    )
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Server {
    pub name: String,
    pub ready: bool,
    pub address: String,
}

/// The cluster's etcd members (`kubectl get nodes -o json`), by name.
pub(super) fn servers(incus: Incus, via: &str) -> Result<Vec<Server>, String> {
    let raw = incus(
        &[
            "exec", via, "--", "k3s", "kubectl", "get", "nodes", "-o", "json",
        ],
        None,
    )
    .map_err(|e| format!("read K3s nodes: {e}"))?;
    let doc: J = serde_json::from_slice(&raw).map_err(|e| format!("decode K3s nodes: {e}"))?;
    let mut out = Vec::new();
    for item in doc["items"].as_array().into_iter().flatten() {
        let labels = &item["metadata"]["labels"];
        if labels["node-role.kubernetes.io/etcd"].as_str() != Some("true") {
            continue;
        }
        let ready = item["status"]["conditions"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|c| c["type"] == "Ready" && c["status"] == "True");
        let address = item["status"]["addresses"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|a| a["type"] == "InternalIP")
            .and_then(|a| a["address"].as_str())
            .unwrap_or("")
            .to_string();
        let name = item["metadata"]["name"].as_str().unwrap_or("").to_string();
        if !name.is_empty() {
            out.push(Server {
                name,
                ready,
                address,
            });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct Record {
    pub hostname: String,
    pub network: String,
    /// The cluster URI the DNS block is keyed by.
    pub cluster: String,
}

/// The cluster's record, if it opted in.
pub(super) fn record(incus: Incus, via: &str) -> Result<Option<Record>, String> {
    let raw = incus(
        &[
            "exec",
            via,
            "--",
            "sh",
            "-c",
            &format!(
                "k3s kubectl -n kube-system get configmap {RECORD} -o json 2>/dev/null || true"
            ),
        ],
        None,
    )
    .map_err(|e| format!("read the cluster endpoint record: {e}"))?;
    if raw.iter().all(u8::is_ascii_whitespace) {
        return Ok(None);
    }
    let doc: J = serde_json::from_slice(&raw)
        .map_err(|e| format!("decode the cluster endpoint record: {e}"))?;
    let data = &doc["data"];
    Ok(Some(Record {
        hostname: data["hostname"].as_str().unwrap_or("").to_string(),
        network: data["network"].as_str().unwrap_or("").to_string(),
        cluster: data["cluster"].as_str().unwrap_or("").to_string(),
    }))
}

fn write_record(
    incus: Incus,
    via: &str,
    record: &Record,
    members: &[String],
) -> Result<(), String> {
    let manifest = json!({
        "apiVersion": "v1", "kind": "ConfigMap",
        "metadata": {"name": RECORD, "namespace": "kube-system"},
        "data": {"hostname": record.hostname, "network": record.network, "cluster": record.cluster,
                 "members": members.join(",")},
    });
    incus(
        &["exec", via, "--", "k3s", "kubectl", "apply", "-f", "-"],
        Some(manifest.to_string().as_bytes()),
    )
    .map(|_| ())
    .map_err(|e| format!("write the cluster endpoint record: {e}"))
}

fn delete_record(incus: Incus, via: &str) -> Result<(), String> {
    incus(
        &[
            "exec",
            via,
            "--",
            "k3s",
            "kubectl",
            "-n",
            "kube-system",
            "delete",
            "configmap",
            RECORD,
            "--ignore-not-found",
        ],
        None,
    )
    .map(|_| ())
    .map_err(|e| format!("delete the cluster endpoint record: {e}"))
}

/// The Incus network the instance's `eth0` is attached to.
/// The Incus managed network an instance's eth0 is on: the device's
/// `network`, or the `parent` of a bridged NIC when that bridge is a managed
/// network (the default profile attaches eth0 that way). Only a managed
/// network takes `raw.dnsmasq`.
pub(super) fn network_of(incus: Incus, instance: &str) -> Result<String, String> {
    let raw = incus(&["list", instance, "--format", "json"], None)
        .map_err(|e| format!("inspect {instance}'s network: {e}"))?;
    let doc: J = serde_json::from_slice(&raw).map_err(|e| format!("decode {instance}: {e}"))?;
    let eth0 = doc
        .as_array()
        .into_iter()
        .flatten()
        .find(|i| i["name"] == instance)
        .map(|i| i["expanded_devices"]["eth0"].clone())
        .unwrap_or(J::Null);
    let refused = || format!("{instance} has no eth0 on an Incus managed network");
    if let Some(network) = eth0["network"].as_str().filter(|n| !n.is_empty()) {
        return Ok(network.to_string());
    }
    let parent = match (eth0["nictype"].as_str(), eth0["parent"].as_str()) {
        (Some("bridged"), Some(parent)) if !parent.is_empty() => parent.to_string(),
        _ => return Err(refused()),
    };
    let raw = incus(&["network", "list", "--format", "json"], None)
        .map_err(|e| format!("list Incus networks: {e}"))?;
    let networks: J =
        serde_json::from_slice(&raw).map_err(|e| format!("decode Incus networks: {e}"))?;
    let managed = networks
        .as_array()
        .into_iter()
        .flatten()
        .any(|n| n["name"] == parent.as_str() && n["managed"] == true);
    if managed {
        Ok(parent)
    } else {
        Err(refused())
    }
}

fn begin(cluster: &str) -> String {
    format!("# opute-cluster-endpoint {cluster} begin")
}

fn end(cluster: &str) -> String {
    format!("# opute-cluster-endpoint {cluster} end")
}

/// `existing` raw.dnsmasq with this cluster's block replaced by `block`
/// (removed when `None`); every other line is kept as it was.
pub(super) fn merge_dnsmasq(
    existing: &str,
    cluster: &str,
    hostname: &str,
    addresses: Option<&[String]>,
) -> String {
    let (b, e) = (begin(cluster), end(cluster));
    let mut out: Vec<String> = Vec::new();
    let mut inside = false;
    for line in existing.lines() {
        if line == b {
            inside = true;
            continue;
        }
        if line == e {
            inside = false;
            continue;
        }
        if !inside {
            out.push(line.to_string());
        }
    }
    while out.last().is_some_and(|l| l.trim().is_empty()) {
        out.pop();
    }
    if let Some(addresses) = addresses {
        out.push(b);
        out.extend(
            addresses
                .iter()
                .map(|a| format!("host-record={hostname},{a}")),
        );
        out.push(e);
    }
    out.join("\n")
}

fn dnsmasq(incus: Incus, network: &str) -> Result<String, String> {
    incus(&["network", "get", network, "raw.dnsmasq"], None)
        .map(|b| {
            String::from_utf8_lossy(&b)
                .trim_end_matches('\n')
                .to_string()
        })
        .map_err(|e| format!("read {network}'s DNS configuration: {e}"))
}

/// Point the hostname at `addresses` (none: remove the block). Reports
/// whether the network changed.
fn set_resolution(
    incus: Incus,
    network: &str,
    cluster: &str,
    hostname: &str,
    addresses: Option<&[String]>,
) -> Result<bool, String> {
    let current = dnsmasq(incus, network)?;
    let wanted = merge_dnsmasq(&current, cluster, hostname, addresses);
    if wanted == current {
        return Ok(false);
    }
    let result = if wanted.is_empty() {
        incus(&["network", "unset", network, "raw.dnsmasq"], None)
    } else {
        incus(
            &["network", "set", network, &format!("raw.dnsmasq={wanted}")],
            None,
        )
    };
    result
        .map(|_| true)
        .map_err(|e| format!("update {network}'s DNS configuration: {e}"))
}

fn drop_in(hostname: &str) -> String {
    format!("# Opute cluster endpoint (typed-ha-endpoint); written by the K3s provider.\ntls-san:\n  - \"{hostname}\"\n")
}

fn read_file(incus: Incus, instance: &str, path: &str) -> Result<String, String> {
    incus(
        &[
            "exec",
            instance,
            "--",
            "sh",
            "-c",
            &format!("cat {path} 2>/dev/null || true"),
        ],
        None,
    )
    .map(|b| String::from_utf8_lossy(&b).into_owned())
    .map_err(|e| format!("read {path} on {instance}: {e}"))
}

/// Restart K3s on one server and wait until its own API reports it Ready.
/// Restart K3s on `server` and wait until its Node is Ready and, when
/// `serving` is given, it serves the hostname: right after a restart the Node
/// can still carry its earlier Ready condition.
fn restart(incus: Incus, server: &str, rollout: Rollout, serving: Serving) -> Result<(), String> {
    incus(&["exec", server, "--", "systemctl", "restart", "k3s"], None)
        .map_err(|e| format!("restart K3s on {server}: {e}"))?;
    for attempt in 0..rollout.attempts {
        if attempt > 0 {
            std::thread::sleep(rollout.poll);
        }
        let status = incus(
            &[
                "exec",
                server,
                "--",
                "k3s",
                "kubectl",
                "get",
                "node",
                server,
                "-o",
                "jsonpath={.status.conditions[?(@.type==\"Ready\")].status}",
            ],
            None,
        );
        let ready = status.is_ok_and(|s| String::from_utf8_lossy(&s).trim() == "True");
        if ready && serving.is_none_or(|(name, address, ca)| (rollout.serves)(name, address, ca)) {
            return Ok(());
        }
    }
    Err(typed(
        "rollout_incomplete",
        &format!(
            "K3s server {server} did not return to Ready serving its TLS names after its restart"
        ),
        json!({"server": server}),
    ))
}

fn addresses(servers: &[Server]) -> Vec<String> {
    let mut out: Vec<String> = servers
        .iter()
        .filter(|s| s.ready && !s.address.is_empty())
        .map(|s| s.address.clone())
        .collect();
    out.sort();
    out.dedup();
    out
}

/// `ensure-cluster-endpoint`.
pub(super) fn ensure(args: &Args, incus: Incus, rollout: Rollout) -> Result<J, String> {
    let target_uri = args.string("targetUri", "");
    let via = args.string("providerInstanceName", "");
    let hostname = args.string("hostname", "");
    if !valid_hostname(&hostname) {
        return Err(
            "hostname must be an explicit DNS name of lowercase labels, such as k3s.example.test"
                .into(),
        );
    }
    let servers = servers(incus, &via)?;
    if !servers.iter().any(|s| s.ready) {
        return Err(
            "a cluster endpoint requires an embedded-etcd cluster with at least one Ready server"
                .into(),
        );
    }
    let existing = record(incus, &via)?;
    if let Some(existing) = &existing {
        if existing.hostname != hostname {
            return Err(format!(
                "the cluster already has the endpoint {}; remove it before choosing another hostname",
                existing.hostname
            ));
        }
    }
    let network = network_of(incus, &via)?;
    let mut restarted = Vec::new();
    let wanted = drop_in(&hostname);
    let ready: Vec<&Server> = servers.iter().filter(|s| s.ready).collect();
    let ca = cluster_ca(incus, &ready)?;
    for server in &ready {
        if read_file(incus, &server.name, DROP_IN)? == wanted {
            continue;
        }
        incus(
            &[
                "exec",
                &server.name,
                "--",
                "sh",
                "-c",
                &format!("mkdir -p $(dirname {DROP_IN}) && cat > {DROP_IN}"),
            ],
            Some(wanted.as_bytes()),
        )
        .map_err(|e| format!("write the TLS names on {}: {e}", server.name))?;
        let serving = Some((hostname.as_str(), server.address.as_str(), ca.as_slice()));
        if let Err(error) = restart(incus, &server.name, rollout, serving) {
            // An endpoint that already resolves keeps resolving only to
            // servers known Ready, never the one that did not return; a first
            // opt-in creates no resolution.
            if existing.is_some() {
                let mut still: Vec<Server> = servers.clone();
                still.retain(|s| s.name != server.name);
                let _ = set_resolution(
                    incus,
                    &network,
                    &target_uri,
                    &hostname,
                    Some(&addresses(&still)),
                );
            }
            return Err(error);
        }
        restarted.push(server.name.clone());
    }
    let members = addresses(&servers);
    let dns_changed = set_resolution(incus, &network, &target_uri, &hostname, Some(&members))?;
    let record_value = Record {
        hostname: hostname.clone(),
        network,
        cluster: target_uri.clone(),
    };
    write_record(incus, &via, &record_value, &members)?;
    Ok(json!({
        "targetUri": target_uri,
        "hostname": hostname,
        "endpoint": format!("https://{hostname}:{API_PORT}"),
        "caBundleRef": ca_reference(&target_uri),
        "resolvesTo": members,
        "restartedServers": restarted,
        "changed": dns_changed || !restarted.is_empty(),
    }))
}

/// `remove-cluster-endpoint`.
pub(super) fn remove(args: &Args, incus: Incus, rollout: Rollout) -> Result<J, String> {
    let target_uri = args.string("targetUri", "");
    let via = args.string("providerInstanceName", "");
    let servers = servers(incus, &via)?;
    let found = record(incus, &via)?;
    let mut carrying = Vec::new();
    for server in servers.iter().filter(|s| s.ready) {
        if !read_file(incus, &server.name, DROP_IN)?.is_empty() {
            carrying.push(server.name.clone());
        }
    }
    if found.is_none() && carrying.is_empty() {
        return Ok(
            json!({"targetUri": target_uri, "removed": false, "changed": false, "restartedServers": []}),
        );
    }
    let hostname = found
        .as_ref()
        .map(|r| r.hostname.clone())
        .unwrap_or_default();
    let network = match &found {
        Some(r) if !r.network.is_empty() => r.network.clone(),
        _ => network_of(incus, &via)?,
    };
    // Resolution first: no client is sent to a server mid-restart for a name
    // it is about to drop.
    set_resolution(incus, &network, &target_uri, &hostname, None)?;
    let mut restarted = Vec::new();
    if !carrying.is_empty() {
        for name in &carrying {
            incus(
                &["exec", name, "--", "rm", "-f", DROP_IN, DYNAMIC_CERT],
                None,
            )
            .map_err(|e| format!("remove the TLS names on {name}: {e}"))?;
        }
        incus(
            &[
                "exec",
                &via,
                "--",
                "k3s",
                "kubectl",
                "-n",
                "kube-system",
                "delete",
                "secret",
                "k3s-serving",
                "--ignore-not-found",
            ],
            None,
        )
        .map_err(|e| format!("drop the serving certificate: {e}"))?;
        for name in &carrying {
            restart(incus, name, rollout, None)?;
            restarted.push(name.clone());
        }
    }
    delete_record(incus, &via)?;
    Ok(
        json!({"targetUri": target_uri, "removed": true, "changed": true, "hostname": hostname,
              "restartedServers": restarted}),
    )
}

/// After a membership change: resolution and the record follow the Ready
/// members, less `excluded` (servers a quorum reset left out of etcd). A
/// cluster without a record is not touched beyond the read.
pub(super) fn follow(
    incus: Incus,
    via: &str,
    excluded: &std::collections::BTreeSet<String>,
) -> Result<Option<Vec<String>>, String> {
    let Some(found) = record(incus, via)? else {
        return Ok(None);
    };
    let mut servers = servers(incus, via)?;
    servers.retain(|s| !excluded.contains(&s.name));
    let members = addresses(&servers);
    let network = if found.network.is_empty() {
        network_of(incus, via)?
    } else {
        found.network.clone()
    };
    set_resolution(
        incus,
        &network,
        &found.cluster,
        &found.hostname,
        Some(&members),
    )?;
    write_record(incus, via, &Record { network, ..found }, &members)?;
    Ok(Some(members))
}

/// A server that just joined an opted-in cluster takes the TLS name, then
/// resolution follows. `joined` is the new server's instance.
pub(super) fn follow_join(
    incus: Incus,
    joined: &str,
    rollout: Rollout,
) -> Result<Option<Vec<String>>, String> {
    let Some(found) = record(incus, joined)? else {
        return Ok(None);
    };
    let wanted = drop_in(&found.hostname);
    if read_file(incus, joined, DROP_IN)? != wanted {
        incus(
            &[
                "exec",
                joined,
                "--",
                "sh",
                "-c",
                &format!("mkdir -p $(dirname {DROP_IN}) && cat > {DROP_IN}"),
            ],
            Some(wanted.as_bytes()),
        )
        .map_err(|e| format!("write the TLS names on {joined}: {e}"))?;
        let servers = servers(incus, joined)?;
        let ready: Vec<&Server> = servers.iter().filter(|s| s.ready).collect();
        let ca = cluster_ca(incus, &ready)?;
        let address = servers
            .iter()
            .find(|s| s.name == joined)
            .map(|s| s.address.clone())
            .unwrap_or_default();
        restart(
            incus,
            joined,
            rollout,
            Some((found.hostname.as_str(), address.as_str(), ca.as_slice())),
        )?;
    }
    follow(incus, joined, &Default::default())
}

/// The cluster CA, read from the first Ready server that has it.
fn cluster_ca(incus: Incus, ready: &[&Server]) -> Result<Vec<u8>, String> {
    let mut last = String::from("no Ready server");
    for server in ready {
        match incus(&["exec", &server.name, "--", "cat", SERVER_CA], None) {
            Ok(pem) if !pem.is_empty() => return Ok(pem),
            Ok(_) => last = format!("{} has an empty CA", server.name),
            Err(e) => last = e,
        }
    }
    Err(format!("read the cluster CA: {last}"))
}

/// The CA a `caBundleRef` names, read from a Ready member; never returned.
pub(super) fn resolve_ca(
    incus: Incus,
    via: &str,
    target_uri: &str,
    reference: &str,
) -> Result<(Vec<u8>, String, Vec<Server>), String> {
    if reference != ca_reference(target_uri) {
        return Err(format!(
            "caBundleRef {reference} does not name this cluster's endpoint"
        ));
    }
    let Some(found) = record(incus, via)? else {
        return Err(format!(
            "caBundleRef {reference}: the cluster has no endpoint"
        ));
    };
    let servers = servers(incus, via)?;
    let mut last = String::from("no Ready server");
    for server in servers.iter().filter(|s| s.ready) {
        match incus(&["exec", &server.name, "--", "cat", SERVER_CA], None) {
            Ok(pem) if !pem.is_empty() => return Ok((pem, found.hostname, servers)),
            Ok(_) => last = format!("{} has an empty CA", server.name),
            Err(e) => last = e,
        }
    }
    Err(format!(
        "caBundleRef {reference}: read the cluster CA: {last}"
    ))
}

/// The hostname as a client on the bridge resolves it (`getent` in a Ready
/// server, which uses the bridge's DNS).
pub(super) fn resolve(
    incus: Incus,
    servers: &[Server],
    hostname: &str,
) -> Result<Vec<String>, String> {
    let mut last = String::from("no Ready server");
    for server in servers.iter().filter(|s| s.ready) {
        match incus(
            &["exec", &server.name, "--", "getent", "ahostsv4", hostname],
            None,
        ) {
            Ok(raw) => {
                let mut out: Vec<String> = String::from_utf8_lossy(&raw)
                    .lines()
                    .filter_map(|l| l.split_whitespace().next())
                    .filter(|a| a.parse::<std::net::Ipv4Addr>().is_ok())
                    .map(str::to_string)
                    .collect();
                out.sort();
                out.dedup();
                return Ok(out);
            }
            Err(e) => last = e,
        }
    }
    Err(format!("resolve {hostname}: {last}"))
}

/// GET /version on the hostname at each resolved address, trusting only the
/// cluster CA; the first answer wins.
pub(super) fn probe(hostname: &str, addresses: &[String], ca_pem: &[u8]) -> Result<u16, String> {
    use rustls::pki_types::{pem::PemObject, CertificateDer};
    let mut roots = rustls::RootCertStore::empty();
    for cert in CertificateDer::pem_slice_iter(ca_pem) {
        let cert = cert.map_err(|e| format!("decode the cluster CA: {e}"))?;
        roots
            .add(cert)
            .map_err(|e| format!("trust the cluster CA: {e}"))?;
    }
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_root_certificates(roots)
        .with_no_client_auth();
    let mut last = String::from("the hostname resolves to no address");
    for address in addresses {
        let pinned: std::net::SocketAddr = format!("{address}:{API_PORT}")
            .parse()
            .map_err(|e| format!("address {address}: {e}"))?;
        let agent = ureq::AgentBuilder::new()
            .timeout(PROBE_TIMEOUT)
            .tls_config(Arc::new(config.clone()))
            .resolver(move |_: &str| Ok(vec![pinned]))
            .build();
        match agent
            .get(&format!("https://{hostname}:{API_PORT}/version"))
            .call()
        {
            Ok(response) => return Ok(response.status()),
            Err(ureq::Error::Status(code, _)) if code == 401 || code == 403 => return Ok(code),
            Err(e) => last = format!("{address}: {e}"),
        }
    }
    Err(last)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    const CA: &str = "-----BEGIN CERTIFICATE-----\nMIIBfake\n-----END CERTIFICATE-----\n";

    /// A scripted Incus host with three K3s servers on incusbr0.
    #[derive(Default)]
    struct Cluster {
        calls: Vec<String>,
        files: BTreeMap<(String, String), String>,
        dnsmasq: String,
        record: Option<J>,
        ready: BTreeMap<String, bool>,
        /// Servers that never come back after a restart.
        stuck: Vec<String>,
        /// Every instance's eth0 device.
        eth0: J,
    }

    impl Cluster {
        fn new() -> Self {
            let mut c = Cluster::default();
            for name in ["e2e-k1", "e2e-k2", "e2e-k3"] {
                c.ready.insert(name.into(), true);
            }
            c.dnsmasq = "dhcp-option=6,1.1.1.1".into();
            // As Incus's default profile attaches it.
            c.eth0 =
                json!({"name": "eth0", "nictype": "bridged", "parent": "incusbr0", "type": "nic"});
            c
        }

        fn nodes(&self) -> String {
            let items: Vec<J> = self.ready.iter().enumerate().map(|(i, (name, ready))| json!({
                "metadata": {"name": name, "labels": {"node-role.kubernetes.io/etcd": "true"}},
                "status": {"conditions": [{"type": "Ready", "status": if *ready { "True" } else { "False" }}],
                           "addresses": [{"type": "InternalIP", "address": format!("10.0.100.{}", 11 + i)}]},
            })).collect();
            json!({"items": items}).to_string()
        }

        fn run(&mut self, args: &[&str], input: Option<&[u8]>) -> Result<Vec<u8>, String> {
            self.calls.push(args.join(" "));
            let text = |s: &str| Ok(s.as_bytes().to_vec());
            match args {
                ["exec", _, "--", "k3s", "kubectl", "get", "nodes", "-o", "json"] => {
                    text(&self.nodes())
                }
                ["exec", _, "--", "sh", "-c", cmd] if cmd.contains("get configmap") => {
                    text(&self.record.as_ref().map(J::to_string).unwrap_or_default())
                }
                ["exec", _, "--", "k3s", "kubectl", "apply", "-f", "-"] => {
                    self.record = Some(serde_json::from_slice(input.unwrap()).unwrap());
                    text("configured")
                }
                ["exec", _, "--", "k3s", "kubectl", "-n", "kube-system", "delete", "configmap", ..] =>
                {
                    self.record = None;
                    text("deleted")
                }
                ["exec", _, "--", "k3s", "kubectl", "-n", "kube-system", "delete", "secret", ..] => {
                    text("deleted")
                }
                ["list", name, "--format", "json"] => text(
                    &json!([{"name": name, "expanded_devices": {"eth0": self.eth0}}]).to_string(),
                ),
                ["network", "list", "--format", "json"] => text(
                    &json!([{"name": "incusbr0", "managed": true},
                            {"name": "br-host", "managed": false}])
                    .to_string(),
                ),
                ["network", "get", "incusbr0", "raw.dnsmasq"] => {
                    text(&format!("{}\n", self.dnsmasq))
                }
                ["network", "set", "incusbr0", value] => {
                    self.dnsmasq = value.strip_prefix("raw.dnsmasq=").unwrap().to_string();
                    text("")
                }
                ["network", "unset", "incusbr0", "raw.dnsmasq"] => {
                    self.dnsmasq.clear();
                    text("")
                }
                ["exec", server, "--", "sh", "-c", cmd] if cmd.starts_with("cat ") => {
                    let path = cmd.split_whitespace().nth(1).unwrap();
                    text(
                        self.files
                            .get(&(server.to_string(), path.to_string()))
                            .map(String::as_str)
                            .unwrap_or(""),
                    )
                }
                ["exec", server, "--", "sh", "-c", cmd] if cmd.contains("cat > ") => {
                    self.files.insert(
                        (server.to_string(), DROP_IN.into()),
                        String::from_utf8(input.unwrap().to_vec()).unwrap(),
                    );
                    text("")
                }
                ["exec", server, "--", "rm", "-f", paths @ ..] => {
                    for path in paths {
                        self.files.remove(&(server.to_string(), path.to_string()));
                    }
                    text("")
                }
                ["exec", server, "--", "systemctl", "restart", "k3s"] => {
                    if self.stuck.contains(&server.to_string()) {
                        self.ready.insert(server.to_string(), false);
                    }
                    text("")
                }
                ["exec", server, "--", "k3s", "kubectl", "get", "node", ..] => {
                    text(if self.ready[*server] { "True" } else { "False" })
                }
                ["exec", _, "--", "cat", path] if *path == SERVER_CA => text(CA),
                other => Err(format!("unscripted incus {other:?}")),
            }
        }

        fn effects(&self) -> Vec<&String> {
            self.calls
                .iter()
                .filter(|c| {
                    c.contains("cat > ")
                        || c.contains("restart")
                        || c.starts_with("network set")
                        || c.starts_with("network unset")
                        || c.contains(" apply ")
                        || c.contains(" delete ")
                        || c.contains(" rm -f ")
                })
                .collect()
        }
    }

    const FAST: Rollout = Rollout {
        attempts: 2,
        poll: Duration::from_millis(1),
        serves: serves_scripted,
    };

    thread_local! {
        /// Per server address: how many more TLS checks see the old
        /// certificate (`u32::MAX`: never the new one).
        static OLD_CERT: std::cell::RefCell<BTreeMap<String, u32>> = Default::default();
        static CHECKS: std::cell::RefCell<Vec<String>> = Default::default();
    }

    fn serves_scripted(hostname: &str, address: &str, ca: &[u8]) -> bool {
        assert_eq!((hostname, ca), ("k3s.e2e.test", CA.as_bytes()));
        CHECKS.with(|c| c.borrow_mut().push(address.to_string()));
        OLD_CERT.with(|m| match m.borrow_mut().get_mut(address) {
            Some(0) | None => true,
            Some(n) => {
                *n = n.saturating_sub(if *n == u32::MAX { 0 } else { 1 });
                false
            }
        })
    }

    fn args(extra: J) -> Args {
        let mut value =
            json!({"targetUri": "cluster:local:e2e-k1", "providerInstanceName": "e2e-k1"});
        value
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().cloned().unwrap_or_default());
        Args::from_map(value.as_object().cloned().unwrap())
    }

    fn ensure_on(c: &mut Cluster) -> Result<J, String> {
        ensure(
            &args(json!({"hostname": "k3s.e2e.test"})),
            &mut |a, i| c.run(a, i),
            FAST,
        )
    }

    fn remove_on(c: &mut Cluster) -> Result<J, String> {
        remove(&args(json!({})), &mut |a, i| c.run(a, i), FAST)
    }

    #[test]
    fn opting_in_sets_tls_names_and_resolution_together() {
        let mut c = Cluster::new();
        let out = ensure_on(&mut c).unwrap();
        for server in ["e2e-k1", "e2e-k2", "e2e-k3"] {
            assert!(
                c.files[&(server.to_string(), DROP_IN.to_string())].contains("\"k3s.e2e.test\"")
            );
        }
        assert_eq!(
            out["restartedServers"],
            json!(["e2e-k1", "e2e-k2", "e2e-k3"])
        );
        assert_eq!(
            out["resolvesTo"],
            json!(["10.0.100.11", "10.0.100.12", "10.0.100.13"])
        );
        assert_eq!(c.dnsmasq, "dhcp-option=6,1.1.1.1\n# opute-cluster-endpoint cluster:local:e2e-k1 begin\n\
            host-record=k3s.e2e.test,10.0.100.11\nhost-record=k3s.e2e.test,10.0.100.12\n\
            host-record=k3s.e2e.test,10.0.100.13\n# opute-cluster-endpoint cluster:local:e2e-k1 end");
        assert_eq!(
            c.record.as_ref().unwrap()["data"]["hostname"],
            "k3s.e2e.test"
        );
        assert_eq!(out["endpoint"], "https://k3s.e2e.test:6443");
        assert_eq!(out["changed"], true);
        // One server at a time: each restart is followed by its own Ready check
        // before the next server's TLS names are written.
        let order: Vec<&String> = c
            .calls
            .iter()
            .filter(|x| x.contains("restart") || x.contains("cat > ") || x.contains("get node "))
            .collect();
        let servers: Vec<&str> = order
            .iter()
            .map(|x| x.split_whitespace().nth(1).unwrap())
            .collect();
        assert_eq!(
            servers,
            [
                "e2e-k1", "e2e-k1", "e2e-k1", "e2e-k2", "e2e-k2", "e2e-k2", "e2e-k3", "e2e-k3",
                "e2e-k3"
            ]
        );
    }

    #[test]
    fn a_repeat_with_unchanged_membership_restarts_nothing() {
        let mut c = Cluster::new();
        ensure_on(&mut c).unwrap();
        c.calls.clear();
        let out = ensure_on(&mut c).unwrap();
        assert_eq!(out["changed"], false);
        assert_eq!(out["restartedServers"], json!([]));
        assert!(
            !c.calls
                .iter()
                .any(|x| x.contains("restart") || x.starts_with("network set")),
            "{:?}",
            c.calls
        );
    }

    #[test]
    fn opting_out_removes_tls_names_resolution_and_the_record() {
        let mut c = Cluster::new();
        ensure_on(&mut c).unwrap();
        let out = remove_on(&mut c).unwrap();
        assert_eq!(
            out["restartedServers"],
            json!(["e2e-k1", "e2e-k2", "e2e-k3"])
        );
        assert!(c.files.keys().all(|(_, path)| path != DROP_IN));
        assert_eq!(c.dnsmasq, "dhcp-option=6,1.1.1.1");
        assert!(c.record.is_none());
        assert!(c
            .calls
            .iter()
            .any(|x| x.contains("delete secret k3s-serving")));
        assert!(c.calls.iter().any(|x| x.contains(DYNAMIC_CERT)));
        c.calls.clear();
        let again = remove_on(&mut c).unwrap();
        assert_eq!(again["changed"], false);
        assert!(c.effects().is_empty(), "{:?}", c.effects());
    }

    #[test]
    fn opting_out_of_a_bridge_with_no_other_dns_unsets_it() {
        let mut c = Cluster::new();
        c.dnsmasq.clear();
        ensure_on(&mut c).unwrap();
        remove_on(&mut c).unwrap();
        assert!(c
            .calls
            .iter()
            .any(|x| x == "network unset incusbr0 raw.dnsmasq"));
    }

    #[test]
    fn a_server_that_does_not_return_stops_the_rollout() {
        let mut c = Cluster::new();
        c.stuck.push("e2e-k2".into());
        let err = ensure_on(&mut c).unwrap_err();
        let typed: J = serde_json::from_str(err.strip_prefix(TYPED).expect("typed")).unwrap();
        assert_eq!(typed["code"], "rollout_incomplete");
        assert_eq!(typed["server"], "e2e-k2");
        assert!(
            !c.calls.iter().any(|x| x.starts_with("exec e2e-k3")),
            "{:?}",
            c.calls
        );
        // A first opt-in that did not finish creates no resolution and no record.
        assert_eq!(c.dnsmasq, "dhcp-option=6,1.1.1.1");
        assert!(c.record.is_none());
    }

    #[test]
    fn an_endpoint_that_resolves_keeps_resolving_only_to_ready_servers() {
        let mut c = Cluster::new();
        ensure_on(&mut c).unwrap();
        c.files.remove(&("e2e-k2".to_string(), DROP_IN.to_string()));
        c.stuck.push("e2e-k2".into());
        let err = ensure_on(&mut c).unwrap_err();
        assert!(err.contains("rollout_incomplete"));
        assert!(!c.dnsmasq.contains("10.0.100.12"), "{}", c.dnsmasq);
        assert!(c.dnsmasq.contains("10.0.100.11") && c.dnsmasq.contains("10.0.100.13"));
    }

    #[test]
    fn without_a_record_membership_hooks_change_nothing() {
        let mut c = Cluster::new();
        assert_eq!(
            follow(&mut |a, i| c.run(a, i), "e2e-k1", &Default::default()).unwrap(),
            None
        );
        assert_eq!(
            follow_join(&mut |a, i| c.run(a, i), "e2e-k2", FAST).unwrap(),
            None
        );
        assert!(c.effects().is_empty(), "{:?}", c.effects());
        assert!(c.files.is_empty());
        assert_eq!(c.dnsmasq, "dhcp-option=6,1.1.1.1");
    }

    #[test]
    fn a_retired_server_leaves_the_endpoint() {
        let mut c = Cluster::new();
        ensure_on(&mut c).unwrap();
        c.ready.remove("e2e-k3");
        let members = follow(&mut |a, i| c.run(a, i), "e2e-k1", &Default::default()).unwrap();
        assert_eq!(
            members,
            Some(vec!["10.0.100.11".to_string(), "10.0.100.12".to_string()])
        );
        assert!(!c.dnsmasq.contains("10.0.100.13"));
        let excluded = ["e2e-k2".to_string()].into_iter().collect();
        follow(&mut |a, i| c.run(a, i), "e2e-k1", &excluded).unwrap();
        assert!(
            !c.dnsmasq.contains("10.0.100.12"),
            "a reset left e2e-k2 out of etcd"
        );
    }

    #[test]
    fn a_joined_server_takes_the_tls_name_then_resolves() {
        let mut c = Cluster::new();
        c.ready.remove("e2e-k3");
        ensure_on(&mut c).unwrap();
        c.ready.insert("e2e-k3".into(), true);
        let members = follow_join(&mut |a, i| c.run(a, i), "e2e-k3", FAST)
            .unwrap()
            .unwrap();
        assert!(c.files[&("e2e-k3".to_string(), DROP_IN.to_string())].contains("k3s.e2e.test"));
        assert!(c
            .calls
            .iter()
            .any(|x| x == "exec e2e-k3 -- systemctl restart k3s"));
        assert_eq!(members.len(), 3);
        assert!(c.dnsmasq.contains("10.0.100.13"));
    }

    #[test]
    fn the_ca_is_only_ever_a_reference() {
        let mut c = Cluster::new();
        let out = ensure_on(&mut c).unwrap();
        assert_eq!(out["caBundleRef"], "cluster-endpoint:local:e2e-k1");
        assert!(!out.to_string().contains("BEGIN CERTIFICATE"));
        assert!(!c
            .record
            .as_ref()
            .unwrap()
            .to_string()
            .contains("BEGIN CERTIFICATE"));
        assert!(!c.dnsmasq.contains("BEGIN CERTIFICATE"));
        let (pem, hostname, _) = resolve_ca(
            &mut |a, i| c.run(a, i),
            "e2e-k1",
            "cluster:local:e2e-k1",
            "cluster-endpoint:local:e2e-k1",
        )
        .unwrap();
        assert_eq!(
            (pem, hostname.as_str()),
            (CA.as_bytes().to_vec(), "k3s.e2e.test")
        );
        let removed = remove_on(&mut c).unwrap();
        assert!(!removed.to_string().contains("BEGIN CERTIFICATE"));
        let err = resolve_ca(
            &mut |a, i| c.run(a, i),
            "e2e-k1",
            "cluster:local:e2e-k1",
            "cluster-endpoint:local:e2e-k1",
        )
        .unwrap_err();
        assert!(err.contains("no endpoint"), "{err}");
        assert!(resolve_ca(
            &mut |a, i| c.run(a, i),
            "e2e-k1",
            "cluster:local:e2e-k1",
            "cluster-endpoint:local:other"
        )
        .is_err());
    }

    #[test]
    fn only_an_explicit_hostname_is_accepted() {
        let mut c = Cluster::new();
        for bad in ["", "k3s", "K3S.e2e.test", "-a.test", "a..test", "a.test;rm"] {
            let err = ensure(
                &args(json!({"hostname": bad})),
                &mut |a, i| c.run(a, i),
                FAST,
            )
            .unwrap_err();
            assert!(err.contains("explicit DNS name"), "{bad}: {err}");
        }
        assert!(c.calls.is_empty(), "refused before any call");
    }

    #[test]
    fn other_dnsmasq_lines_are_kept() {
        let merged = merge_dnsmasq("a=1\nb=2", "c", "h.test", Some(&["10.0.0.1".to_string()]));
        assert_eq!(merge_dnsmasq(&merged, "c", "h.test", None), "a=1\nb=2");
        let other = merge_dnsmasq(&merged, "d", "i.test", Some(&["10.0.0.2".to_string()]));
        assert!(merge_dnsmasq(&other, "c", "h.test", None).contains("host-record=i.test,10.0.0.2"));
    }

    #[test]
    fn the_network_is_the_nics_managed_network_or_bridge() {
        let mut cluster = Cluster::new();
        let mut incus = |a: &[&str], i: Option<&[u8]>| cluster.run(a, i);
        assert_eq!(network_of(&mut incus, "e2e-k1").unwrap(), "incusbr0");

        let mut cluster = Cluster::new();
        cluster.eth0 = json!({"network": "incusbr0", "type": "nic"});
        let mut incus = |a: &[&str], i: Option<&[u8]>| cluster.run(a, i);
        assert_eq!(network_of(&mut incus, "e2e-k1").unwrap(), "incusbr0");

        // A host bridge Incus does not manage has no dnsmasq to configure.
        for eth0 in [
            json!({"nictype": "bridged", "parent": "br-host", "type": "nic"}),
            json!({"nictype": "macvlan", "parent": "incusbr0", "type": "nic"}),
            J::Null,
        ] {
            let mut cluster = Cluster::new();
            cluster.eth0 = eth0;
            let mut incus = |a: &[&str], i: Option<&[u8]>| cluster.run(a, i);
            let error = network_of(&mut incus, "e2e-k1").unwrap_err();
            assert!(
                error.contains("no eth0 on an Incus managed network"),
                "{error}"
            );
        }
    }

    /// A stale Ready condition right after a restart is not a rollout: a
    /// server counts once it serves a certificate for the hostname.
    #[test]
    fn a_server_counts_as_rolled_out_once_it_serves_the_name() {
        let mut c = Cluster::new();
        OLD_CERT.with(|m| m.borrow_mut().insert("10.0.100.12".into(), 1));
        CHECKS.with(|c| c.borrow_mut().clear());
        ensure_on(&mut c).unwrap();
        let k2 = CHECKS.with(|c| c.borrow().iter().filter(|a| *a == "10.0.100.12").count());
        assert_eq!(k2, 2, "e2e-k2 was checked until it served the name");

        let mut c = Cluster::new();
        OLD_CERT.with(|m| m.borrow_mut().insert("10.0.100.12".into(), u32::MAX));
        let err = ensure_on(&mut c).unwrap_err();
        let typed: J = serde_json::from_str(err.strip_prefix(TYPED).expect("typed")).unwrap();
        assert_eq!(typed["code"], "rollout_incomplete");
        assert_eq!(typed["server"], "e2e-k2");
        assert!(
            !c.calls.iter().any(|x| x.starts_with("exec e2e-k3")),
            "{:?}",
            c.calls
        );
        OLD_CERT.with(|m| m.borrow_mut().clear());
    }
}
