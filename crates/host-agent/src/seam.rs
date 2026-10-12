//! The seam verifier (decision D14, `three-role-capability-seam`).
//!
//! It builds the real composition root (`app::compose`) in both serving
//! modes, walks the one service context, and classifies every name the
//! descriptor source defines:
//!
//! - `compliant`: a mounted service offers it, it has a consumer, and its
//!   Service Definition comes from the descriptor source;
//! - `unported`: no service offers it and every executor answers with the
//!   typed `not_implemented` error (M8 work, published for catalog parity);
//! - `gap`: no service offers it, yet some executor answers otherwise, so an
//!   implementation is reached outside the service context.
//!
//! The computed gaps must equal `seam-gaps.json`, the committed list of
//! M1–M7 capabilities not yet revised. Published names are cross-checked
//! against Go's captured `tools/list` for each mode. With `SEAM_REPORT` set,
//! the report is written there.

use crate::admission::Binding;
use crate::plan::runner::RunCtx;
use crate::transport::Server;
use serde_json::{json, Map, Value as J};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{atomic::AtomicBool, Arc};

const GAPS: &str = include_str!("../seam-gaps.json");

fn server(dir: &std::path::Path, mode: &str) -> Arc<Server> {
    let empty = dir.join("empty-path");
    std::fs::create_dir_all(&empty).unwrap();
    let env = crate::config::Env::from_pairs([
        ("HOME", dir.join("home").to_str().unwrap()),
        ("XDG_CONFIG_HOME", dir.join("xdg").to_str().unwrap()),
        ("XDG_RUNTIME_DIR", dir.join("run").to_str().unwrap()),
        ("PATH", empty.to_str().unwrap()),
        ("OPUTE_REMOTE_AGENT_ID", "seam-verifier"),
        ("OPUTE_AGENT_MODE", mode),
        ("OPUTE_STANDALONE_ALLOW_MUTATIONS", "true"),
        (
            "OPUTE_STANDALONE_STATE_DIR",
            dir.join("state").to_str().unwrap(),
        ),
        (
            "OPUTE_INSTANCE_ROOT",
            dir.join("instance").to_str().unwrap(),
        ),
        (
            "OPUTE_HOST_RESOURCE_LOCK_DIR",
            dir.join("locks").to_str().unwrap(),
        ),
        ("HOST_MCP_BIND_HOST", "127.0.0.1"),
        ("HOST_MCP_PORT", "1"),
    ]);
    let runtime = crate::app::new_runtime(&env).unwrap();
    let cfg = runtime.config.clone();
    let authz_dir = if cfg.standalone_state_dir.as_os_str().is_empty() {
        cfg.instance_root.clone()
    } else {
        cfg.standalone_state_dir.clone()
    };
    std::fs::create_dir_all(&authz_dir).unwrap();
    let authz = crate::store::AuthzStore::open(&authz_dir, &cfg.opute_client_secret).unwrap();
    crate::app::compose(&cfg, authz, runtime.state.clone(), None).unwrap()
}

fn not_implemented(result: &J) -> bool {
    result
        .pointer("/structuredContent/code")
        .and_then(J::as_str)
        == Some("not_implemented")
}

/// What each executor answers for a name no mounted service offers. Only
/// the typed `not_implemented` error proves nothing was reached.
fn executor_answers(server: &Arc<Server>, name: &str) -> Vec<(&'static str, J)> {
    let args = Map::new();
    let binding = Binding::default();
    let snapshot = server.catalog.snapshot();
    let mut answers = vec![
        (
            "dispatch",
            crate::tools::invoke(server, &RunCtx::background(), name, &args, &binding),
        ),
        (
            "task-input",
            crate::tools::run_task_input(server, name, &args),
        ),
    ];
    if crate::tools::LIFECYCLE.contains(&name) {
        // The executor is probed directly, not through admission: an
        // inherited reservation, whose release is a no-op, stands in.
        let reservation = crate::resource::Reservation {
            id: "seam-verifier".into(),
            request: Default::default(),
            inherited: true,
            created_at: None,
            expires_at: None,
        };
        let ctx = RunCtx::background().with_reservation(reservation.clone());
        let claimed = Arc::new(AtomicBool::new(false));
        let call = crate::builtin::Invocation {
            ctx: &ctx,
            args: &args,
            binding: &binding,
            snapshot: &snapshot,
            claimed: Some(&claimed),
        };
        answers.push((
            "lifecycle",
            crate::tools::run_lifecycle(server, name, &call),
        ));
    }
    answers
}

fn route(name: &str) -> &'static str {
    if crate::tools::LIFECYCLE.contains(&name) {
        "lifecycle"
    } else if name == "request_task_input" {
        "task-input"
    } else {
        "dispatch"
    }
}

fn go_published(mode: &str) -> BTreeSet<String> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../baseline/inventory/catalog")
        .join(format!("{mode}.json"));
    let doc: J = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    doc["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        // D19: Rust publishes the neutral name in place of each mapped Go name.
        .map(|name| {
            crate::catalog::neutral_name(name)
                .unwrap_or(name)
                .to_string()
        })
        .collect()
}

/// The verifier's report for the current build.
pub(crate) fn report() -> J {
    let mut capabilities: BTreeMap<String, Map<String, J>> = BTreeMap::new();
    let mut services = Vec::new();
    let internal: BTreeSet<String> = crate::catalog::internal()
        .tools
        .iter()
        .map(|d| d.name.clone())
        .collect();
    let mut problems = Vec::new();
    for mode in ["platform", "standalone"] {
        let dir = tempfile::tempdir().unwrap();
        let server = server(dir.path(), mode);
        let published: BTreeSet<String> = server
            .catalog
            .snapshot()
            .tools
            .iter()
            .map(|d| d.name.clone())
            .collect();
        if published != go_published(mode) {
            problems.push(format!(
                "{mode}: published names differ from Go's captured tools/list under the D19 map"
            ));
        }
        let mounts = &server.providers.activation.mounts;
        let mounted =
            mounts.generation_services(&crate::builtin::provider_id(), crate::builtin::GENERATION);
        if mode == "platform" {
            for service in &mounted {
                services.push(json!({
                    "key": service.key,
                    "capability": service.capability_id,
                    "inject": service.inject,
                    "operations": service.definition.operations.iter().map(|o| o.id.clone()).collect::<Vec<_>>(),
                }));
            }
        }
        for name in crate::catalog::source_names() {
            let in_mode = published.contains(&name) || internal.contains(&name);
            let entry = capabilities.entry(name.clone()).or_insert_with(|| {
                let mut entry = Map::new();
                entry.insert("route".into(), J::from(route(&name)));
                entry.insert("internal".into(), J::Bool(internal.contains(&name)));
                entry.insert("published".into(), json!({}));
                entry
            });
            entry["published"][mode] = J::Bool(published.contains(&name));
            if !in_mode {
                continue;
            }
            let service = crate::builtin::resolve(&server, &name);
            let status = match &service {
                Some(service) => {
                    entry.insert("service".into(), J::from(service.key.clone()));
                    let mut consumers = vec!["tools/call"];
                    if route(&name) == "dispatch" {
                        consumers.push("plan executor");
                    }
                    entry.insert("consumers".into(), json!(consumers));
                    "compliant"
                }
                None => {
                    let reached: Vec<&str> = executor_answers(&server, &name)
                        .into_iter()
                        .filter(|(_, answer)| !not_implemented(answer))
                        .map(|(executor, _)| executor)
                        .collect();
                    if reached.is_empty() {
                        "unported"
                    } else {
                        entry.insert("reachedBy".into(), json!(reached));
                        "gap"
                    }
                }
            };
            // A gap in any mode is a gap. A mounted service is mounted in
            // every mode, so compliance cannot differ between modes.
            let combined = match (entry.get("status").and_then(J::as_str), status) {
                (None, status) => status,
                (Some("gap"), _) | (_, "gap") => "gap",
                (Some(previous), status) if previous == status => status,
                (Some(previous), status) => {
                    problems.push(format!(
                        "{name}: {previous} in one mode, {status} in the other"
                    ));
                    status
                }
            };
            entry.insert("status".into(), J::from(combined));
        }
        crate::builtin::dispose(mounts);
        if !mounts
            .generation_services(&crate::builtin::provider_id(), crate::builtin::GENERATION)
            .is_empty()
        {
            problems.push(format!("{mode}: built-in services survive disposal"));
        }
    }
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut gaps = Vec::new();
    for (name, entry) in &capabilities {
        let status = entry
            .get("status")
            .and_then(J::as_str)
            .unwrap_or("unpublished");
        *counts.entry(status.into()).or_default() += 1;
        if status == "gap" {
            gaps.push(name.clone());
        }
    }
    json!({
        "schemaVersion": "opute-seam-report.v1",
        "decision": "D14",
        "provider": crate::builtin::provider_id(),
        "generation": crate::builtin::GENERATION,
        "services": services,
        "counts": counts,
        "gaps": gaps,
        "problems": problems,
        "capabilities": capabilities,
    })
}

#[test]
fn seam_verifier() {
    let report = report();
    if let Ok(path) = std::env::var("SEAM_REPORT") {
        std::fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    }
    let problems = report["problems"].as_array().unwrap();
    assert!(problems.is_empty(), "seam problems: {problems:?}");
    let expected: Vec<String> = serde_json::from_str(GAPS).unwrap();
    let computed: Vec<String> = serde_json::from_value(report["gaps"].clone()).unwrap();
    assert_eq!(
        computed, expected,
        "seam gaps differ from seam-gaps.json: a capability is reached outside the service context, or a revised one is still listed"
    );
}
