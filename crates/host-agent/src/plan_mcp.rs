//! MCP-layer glue for the `host-plan.v1` executor (`internal/hostmcp/plan_run.go`).
//!
//! `validate_host_plan` is pure: decode, run the static validator, hash,
//! and report the topological levels -- no persistence, no task, no
//! dispatch. `run_host_plan`/`get_host_plan_run` are durable and async, and
//! carry the same recipe-metadata-driven redaction Go's
//! `handleRunHostPlanWithMetadata` does, so a host-local recipe's secret
//! inputs never reach the durable store or a task argument unredacted
//! (`host_recipe_mcp.rs` is the recipe-shaped caller of
//! `handle_run_host_plan_with_metadata`). The recipe-metadata-driven
//! provider-activation branches (M7: `activation`, `providerTeardownInputs`,
//! `activateCompletedProviderCandidate`) and the resource-reservation lease
//! binding around a launched run are not yet ported; a plan run here is
//! admitted and audited like any other capability call, but does not yet
//! inherit or hold a reservation of its own.

use crate::catalog::Snapshot;
use crate::plan::graph::topological_levels;
use crate::plan::runner::{DispatchResult, RunCtx, Runner};
use crate::plan::schema::{self as plan, Capability, Document, NodeRunState, RunState};
use crate::plan_evidence::{
    persisted_recipe_hash, recipe_hash, redact_plan_document, redact_plan_run_state,
    redacted_metadata,
};
use crate::store::PlanRecord;
use crate::transport::Server;
use serde_json::{json, Map, Value as J};
use std::collections::BTreeMap;
use std::sync::Arc;

/// `planCapabilitiesFromSnapshot`.
pub fn capabilities_from_snapshot(snapshot: &Snapshot) -> BTreeMap<String, Capability> {
    snapshot
        .tools
        .iter()
        .map(|descriptor| {
            (
                descriptor.name.clone(),
                Capability {
                    name: descriptor.name.clone(),
                    input_schema: descriptor
                        .input_schema
                        .as_object()
                        .cloned()
                        .unwrap_or_default(),
                    output_schema: descriptor
                        .output_schema
                        .as_ref()
                        .and_then(J::as_object)
                        .cloned()
                        .unwrap_or_default(),
                    effect: descriptor.effect.clone(),
                    idempotent: descriptor.idempotent,
                },
            )
        })
        .collect()
}

/// `decodePlanArgument`.
pub fn decode_plan_argument(args: &Map<String, J>) -> Result<Document, String> {
    match args.get("plan") {
        None | Some(J::Null) => Err("plan is required".to_string()),
        Some(raw) => plan::decode_value(raw),
    }
}

/// `validateHostPlanWithSnapshot`.
pub fn validate_host_plan_with_snapshot(doc: &Document, snapshot: &Snapshot) -> Result<J, String> {
    let capabilities = capabilities_from_snapshot(snapshot);
    plan::validate(doc, &capabilities, &snapshot.revision)?;
    let (hash, _) = plan::document_hash(doc)?;
    let levels = topological_levels(doc)?;
    let level_ids: Vec<J> = levels
        .iter()
        .map(|level| {
            J::Array(
                level
                    .iter()
                    .map(|node| J::String(node.id.clone()))
                    .collect(),
            )
        })
        .collect();
    Ok(json!({
        "valid": true,
        "contractVersion": doc.contract_version,
        "planId": doc.plan_id,
        "generation": doc.generation,
        "idempotencyKey": doc.idempotency_key,
        "documentHash": hash,
        "catalogRevision": snapshot.revision,
        "nodeCount": doc.nodes.len(),
        "levels": level_ids,
    }))
}

/// `handleValidateHostPlan`.
pub fn handle_validate_host_plan(args: &Map<String, J>, snapshot: &Snapshot) -> J {
    let doc = match decode_plan_argument(args) {
        Ok(doc) => doc,
        Err(e) => return crate::tools::error_result(&e),
    };
    match validate_host_plan_with_snapshot(&doc, snapshot) {
        Ok(result) => crate::tools::structured_result(result, Some("host plan is valid")),
        Err(e) => crate::tools::error_result(&e),
    }
}

/// `initialPlanState`.
fn initial_plan_state(run_id: &str, doc: &Document) -> RunState {
    let mut nodes = BTreeMap::new();
    for node in &doc.nodes {
        nodes.insert(
            node.id.clone(),
            NodeRunState {
                id: node.id.clone(),
                status: plan::STATUS_PENDING.to_string(),
                ..Default::default()
            },
        );
    }
    RunState {
        run_id: run_id.to_string(),
        plan_id: doc.plan_id.clone(),
        generation: doc.generation,
        status: "pending".to_string(),
        nodes,
        ..Default::default()
    }
}

/// `taskCapabilityDescriptor`-keyed-by-node, for `redact_plan_run_state`:
/// each action/validate node's declared capability output schema, so a
/// node's durable `output`/`observed` evidence is projected through it
/// rather than copied verbatim.
fn node_output_schemas(
    doc: &Document,
    capabilities: &BTreeMap<String, Capability>,
) -> BTreeMap<String, Map<String, J>> {
    let mut out = BTreeMap::new();
    for node in &doc.nodes {
        let tool = node
            .action
            .as_ref()
            .map(|a| a.tool.as_str())
            .or_else(|| node.validate.as_ref().map(|v| v.tool.as_str()));
        if let Some(schema) = tool
            .and_then(|t| capabilities.get(t))
            .map(|c| &c.output_schema)
        {
            out.insert(node.id.clone(), schema.clone());
        }
    }
    out
}

/// `marshalPlanState`: the durable `RunState` projection through the plan's
/// own node output schemas and each context entry's `secret` marker --
/// bare `run_host_plan` (`metadata: None`) has no secret input names to add
/// on top, but node-output and context-secret projection still apply.
fn marshal_plan_state(
    state: &RunState,
    doc: &Document,
    capabilities: &BTreeMap<String, Capability>,
) -> String {
    let schemas = node_output_schemas(doc, capabilities);
    go_json_string(&redact_plan_run_state(state, &schemas)).unwrap_or_else(|_| {
        r#"{"status":"unknown","error":"failed to encode plan state"}"#.to_string()
    })
}

/// `json.Marshal` as a `String`: `serde_json` alone does not HTML-escape
/// `<`, `>` and `&`, so this is required everywhere Rust output must match
/// Go's byte-for-byte -- a durable column, a canonical hash input, or a
/// value compared directly against a Go reference run.
fn go_json_string<T: serde::Serialize>(value: &T) -> Result<String, String> {
    let encoded = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    String::from_utf8(crate::gojson::html_escape_json_bytes(encoded)).map_err(|e| e.to_string())
}

/// `redactedPlanIdentity`: makes idempotency stable when a recipe refreshes
/// a secret input (a short-lived MCP bearer, say) -- secret rotation must
/// not make the same declarative plan look like a different document, and
/// the unredacted value must never enter the durable document hash.
fn redacted_plan_identity(
    doc: &Document,
    metadata: Option<&Map<String, J>>,
    capabilities: &BTreeMap<String, Capability>,
) -> Result<(String, String), String> {
    let (_, encoded) = plan::document_hash(doc)?;
    let document: J =
        serde_json::from_slice(&encoded).map_err(|e| format!("decode plan for redaction: {e}"))?;
    let redacted_document = redact_plan_document(&document, metadata, capabilities);
    let redacted_plan = go_json_string(&redacted_document)?;
    let redacted_typed: Document = serde_json::from_str(&redacted_plan)
        .map_err(|e| format!("decode redacted plan identity: {e}"))?;
    let (hash, _) = plan::document_hash(&redacted_typed)?;
    Ok((hash, redacted_plan))
}

/// `ensurePlanDocumentHash`: a record created under an older canonical
/// encoding is migrated forward once its content is confirmed unchanged;
/// a genuine content mismatch under the same idempotency key is refused.
fn ensure_plan_document_hash(
    state: &crate::store::StateStore,
    record: &mut PlanRecord,
    expected: &str,
) -> Result<(), String> {
    if record.document_hash == expected {
        return Ok(());
    }
    let mismatch = || {
        format!(
            "idempotency key already belongs to a different plan document: {}",
            record.run_id
        )
    };
    let persisted: Document = serde_json::from_str(&record.plan_json).map_err(|_| mismatch())?;
    let (persisted_hash, _) = plan::document_hash(&persisted).map_err(|_| mismatch())?;
    if persisted_hash != expected {
        return Err(mismatch());
    }
    state
        .update_plan_document_hash(&record.run_id, expected)
        .map_err(|e| format!("migrate persisted plan document hash: {e}"))?;
    record.document_hash = expected.to_string();
    Ok(())
}

/// `planRunResult`, minus the M7 active-runtime/active-capability
/// projection: `record.recipe_json`'s own fields (recipe id/version,
/// execution, redacted inputs, the redacted expanded plan, ...) are still
/// surfaced, since those are M6-scoped and already redacted at write time.
fn plan_run_result(record: &PlanRecord) -> J {
    let mut result = json!({
        "runId": record.run_id, "planId": record.plan_id, "generation": record.generation,
        "idempotencyKey": record.idempotency_key, "documentHash": record.document_hash,
        "catalogRevision": record.catalog_revision, "status": record.status,
        "createdAt": record.created_at, "updatedAt": record.updated_at,
    });
    if !record.state_json.trim().is_empty() {
        if let Ok(state_value) = serde_json::from_str::<J>(&record.state_json) {
            if let Some(nodes) = state_value.get("nodes") {
                result["nodes"] = nodes.clone();
            }
            result["state"] = state_value;
        }
    }
    if result.get("nodes").is_none() {
        result["nodes"] = json!({});
    }
    if !record.error_message.is_empty() {
        result["error"] = J::String(record.error_message.clone());
    }
    if !record.recipe_json.trim().is_empty() {
        if let Ok(recipe_value) = serde_json::from_str::<J>(&record.recipe_json) {
            result["recipe"] = recipe_value;
        }
    }
    crate::tools::structured_result(result, None)
}

/// `handleRunHostPlan`: the bare path, with no recipe metadata to redact
/// against or persist.
pub fn handle_run_host_plan(server: &Arc<Server>, args: &Map<String, J>) -> J {
    handle_run_host_plan_with_metadata(
        server,
        args,
        None,
        "run_host_plan",
        "Executing host plan...",
    )
}

/// `handleRunHostPlanWithMetadata`. The recipe-metadata-driven
/// provider-activation branches (M7: `activation`, `providerTeardownInputs`,
/// `activateCompletedProviderCandidate`) and the resource-reservation lease
/// binding around a launched run are not yet ported (see the module-level
/// comment); the durable wait -> `tasks/input_required` escalation is a
/// separate, also-not-yet-wired follow-up -- a plan that reaches a durable
/// wait still records `status: "waiting"` correctly in `get_host_plan_run`,
/// it just does not yet surface as a live MCP task input request.
pub fn handle_run_host_plan_with_metadata(
    server: &Arc<Server>,
    args: &Map<String, J>,
    recipe_metadata: Option<&Map<String, J>>,
    task_name: &str,
    task_description: &str,
) -> J {
    let doc = match decode_plan_argument(args) {
        Ok(doc) => doc,
        Err(e) => return crate::tools::error_result(&e),
    };
    let snapshot = server.catalog;
    if let Err(e) = validate_host_plan_with_snapshot(&doc, snapshot) {
        return crate::tools::error_result(&e);
    }
    let capabilities = capabilities_from_snapshot(snapshot);
    let (hash, redacted_plan) = match redacted_plan_identity(&doc, recipe_metadata, &capabilities) {
        Ok(v) => v,
        Err(e) => return crate::tools::error_result(&format!("hash plan: {e}")),
    };

    let record = match resolve_plan_record(
        server,
        &doc,
        &hash,
        &redacted_plan,
        recipe_metadata,
        &capabilities,
    ) {
        Ok(ResolvedRecord::Terminal(result)) => return result,
        Ok(ResolvedRecord::Fresh(record)) => *record,
        Err(e) => return crate::tools::error_result(&e),
    };

    if let Some(existing) = server.tasks.get(&record.run_id) {
        if existing.status == "working" || existing.status == "input_required" {
            return plan_run_result(&record);
        }
    }

    let mut state_value = if record.state_json.trim().is_empty() {
        initial_plan_state(&record.run_id, &doc)
    } else {
        match serde_json::from_str::<RunState>(&record.state_json) {
            Ok(v) => v,
            Err(e) => {
                return crate::tools::error_result(&format!("decode persisted plan state: {e}"))
            }
        }
    };
    state_value.run_id = record.run_id.clone();
    state_value.plan_id = doc.plan_id.clone();
    state_value.generation = doc.generation;
    state_value.status = "running".to_string();
    {
        let state = match server.host.state.lock() {
            Ok(s) => s,
            Err(_) => return crate::tools::error_result("durable plan state is unavailable"),
        };
        if let Err(e) = state.update_plan(
            &record.run_id,
            "running",
            &marshal_plan_state(&state_value, &doc, &capabilities),
            "",
        ) {
            return crate::tools::error_result(&format!("start plan run: {e}"));
        }
    }

    let redacted_task_plan = redact_plan_document(
        &serde_json::from_str::<J>(&redacted_plan).unwrap_or(J::Null),
        recipe_metadata,
        &capabilities,
    );
    let resume = args.get("resume").and_then(J::as_bool).unwrap_or(false);
    let mut task_args = json!({"plan": redacted_task_plan, "resume": resume});
    if let Some(metadata) = redacted_metadata(recipe_metadata) {
        task_args["recipe"] = J::Object(metadata);
    }
    let (_rec, cancelled) =
        server
            .tasks
            .create_with_id(&record.run_id, task_name, task_description, task_args);
    spawn_plan_execution(
        Arc::clone(server),
        record.run_id.clone(),
        doc,
        state_value,
        snapshot.revision.clone(),
        cancelled,
    );
    plan_run_result(&record)
}

enum ResolvedRecord {
    Terminal(J),
    Fresh(Box<PlanRecord>),
}

/// The `FindPlan`/`CreatePlan` idempotency dance: an in-flight or
/// already-terminal run under the same `(planId, generation,
/// idempotencyKey)` short-circuits to its existing result; a genuinely new
/// run gets a fresh durable record. A recipe-metadata-bearing request whose
/// idempotency key already belongs to a *different* recipe document (same
/// plan identity, different `recipeHash`) is a hard conflict, same as a
/// plan-document mismatch.
fn resolve_plan_record(
    server: &Arc<Server>,
    doc: &Document,
    hash: &str,
    redacted_plan: &str,
    recipe_metadata: Option<&Map<String, J>>,
    capabilities: &BTreeMap<String, Capability>,
) -> Result<ResolvedRecord, String> {
    let state = server
        .host
        .state
        .lock()
        .map_err(|_| "durable plan state is unavailable".to_string())?;
    let found = state
        .find_plan(&doc.plan_id, doc.generation, &doc.idempotency_key)
        .map_err(|e| format!("find plan run: {e}"))?;
    if let Some(mut record) = found {
        ensure_plan_document_hash(&state, &mut record, hash)?;
        if let Some(metadata) = recipe_metadata {
            let stored = persisted_recipe_hash(&record.recipe_json);
            if !stored.is_empty() && stored != recipe_hash(metadata) {
                return Err(format!(
                    "idempotency key already belongs to a different runtime recipe: {}",
                    record.run_id
                ));
            }
        }
        if let Some(result) =
            short_circuit_if_live_or_reconcile(&state, &mut record, &server.catalog.revision)?
        {
            return Ok(ResolvedRecord::Terminal(result));
        }
        return Ok(ResolvedRecord::Fresh(Box::new(record)));
    }
    let new_record = PlanRecord {
        run_id: crate::tasks::new_task_id(),
        plan_id: doc.plan_id.clone(),
        generation: doc.generation,
        idempotency_key: doc.idempotency_key.clone(),
        document_hash: hash.to_string(),
        catalog_revision: server.catalog.revision.clone(),
        status: "working".to_string(),
        plan_json: redacted_plan.to_string(),
        recipe_json: redacted_metadata(recipe_metadata)
            .map(J::Object)
            .and_then(|v| go_json_string(&v).ok())
            .unwrap_or_default(),
        state_json: marshal_plan_state(&initial_plan_state("", doc), doc, capabilities),
        ..Default::default()
    };
    let (mut record, created) = state
        .create_plan(&new_record)
        .map_err(|e| format!("persist plan run: {e}"))?;
    ensure_plan_document_hash(&state, &mut record, hash)?;
    if !created {
        if let Some(result) =
            short_circuit_if_live_or_reconcile(&state, &mut record, &server.catalog.revision)?
        {
            return Ok(ResolvedRecord::Terminal(result));
        }
    }
    Ok(ResolvedRecord::Fresh(Box::new(record)))
}

/// A working/running/waiting record answers at once; a terminal record's
/// stale catalog revision is migrated forward so a later node reconcile
/// sees the current one. Returns the short-circuit result, if any.
fn short_circuit_if_live_or_reconcile(
    state: &crate::store::StateStore,
    record: &mut PlanRecord,
    current_revision: &str,
) -> Result<Option<J>, String> {
    if record.status == "working"
        || record.status == "running"
        || record.status == plan::RUN_STATUS_WAITING
    {
        return Ok(Some(plan_run_result(record)));
    }
    if !record.catalog_revision.is_empty() && record.catalog_revision != current_revision {
        state
            .update_plan_catalog_revision(&record.run_id, current_revision)
            .map_err(|e| format!("update resumed plan catalog revision: {e}"))?;
        record.catalog_revision = current_revision.to_string();
    }
    Ok(None)
}

fn to_dispatch_result(value: J) -> DispatchResult {
    let is_error = value.get("isError") == Some(&J::Bool(true));
    let structured_content = value
        .get("structuredContent")
        .cloned()
        .filter(|v| !v.is_null());
    let text = value
        .get("content")
        .and_then(J::as_array)
        .into_iter()
        .flatten()
        .find_map(|c| {
            if c.get("type").and_then(J::as_str) != Some("text") {
                return None;
            }
            c.get("text")
                .and_then(J::as_str)
                .map(str::to_string)
                .filter(|s| !s.trim().is_empty())
        })
        .unwrap_or_else(|| "operation failed".to_string());
    DispatchResult {
        is_error,
        structured_content,
        text,
    }
}

/// `executeHostPlan` with `recipeMetadata == nil`: runs the plan to
/// completion on its own thread, persisting every state transition via
/// the runner's `Sink`, then completes or fails the task. Each node's
/// tool call goes through the exact same `tools::dispatch` admission and
/// audit path a direct capability call takes, under no task identity (as
/// Go's `DispatchTool` does for plan nodes too) -- the plan's own task
/// identity is for `tasks/get`, not for attributing individual node calls.
fn spawn_plan_execution(
    server: Arc<Server>,
    run_id: String,
    doc: Document,
    state_value: RunState,
    catalog_revision: String,
    cancelled: Arc<std::sync::atomic::AtomicBool>,
) {
    let capabilities = capabilities_from_snapshot(server.catalog);
    let sink_capabilities = capabilities.clone();
    let final_capabilities = capabilities.clone();
    let host_agent_id = server.host.agent_id.clone();
    let dispatch_server = Arc::clone(&server);
    let sink_server = Arc::clone(&server);
    let sink_run_id = run_id.clone();
    let sink_doc = doc.clone();
    let final_doc = doc.clone();
    let workers_server = Arc::clone(&server);
    let handle = std::thread::spawn(move || {
        let runner = Runner {
            capabilities,
            catalog_revision,
            host_agent_id,
            dispatch: Arc::new(
                move |ctx: &RunCtx, name: &str, call_args: &Map<String, J>| {
                    Ok(to_dispatch_result(crate::tools::dispatch(
                        &dispatch_server,
                        name,
                        call_args,
                        None,
                        Some(ctx),
                    )))
                },
            ),
            sink: Some(Box::new(move |state: &RunState| {
                if let Ok(store) = sink_server.host.state.lock() {
                    let _ = store.update_plan(
                        &sink_run_id,
                        &state.status,
                        &marshal_plan_state(state, &sink_doc, &sink_capabilities),
                        &state.error,
                    );
                }
                Ok(())
            })),
        };
        let ctx = RunCtx::from_flag(cancelled);
        let (final_state, run_err) = runner.run(&ctx, &doc, state_value);
        let status = if final_state.status.is_empty() {
            "failed".to_string()
        } else {
            final_state.status.clone()
        };
        if let Ok(store) = server.host.state.lock() {
            let _ = store.update_plan(
                &run_id,
                &status,
                &marshal_plan_state(&final_state, &final_doc, &final_capabilities),
                &final_state.error,
            );
        }
        match run_err {
            Err(e) => server.tasks.fail(&run_id, &e),
            Ok(()) => {
                let structured = serde_json::to_value(&final_state).ok();
                server.tasks.complete(
                    &run_id,
                    crate::tasks::ToolResult {
                        structured_content: structured,
                        ..Default::default()
                    },
                );
            }
        }
    });
    if let Ok(mut workers) = workers_server.task_workers.lock() {
        workers.retain(|worker| !worker.is_finished());
        workers.push(handle);
    };
}

/// `handleGetHostPlanRun`.
pub fn handle_get_host_plan_run(server: &Server, args: &Map<String, J>) -> J {
    let run_id = args
        .get("runId")
        .and_then(J::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if run_id.is_empty() {
        return crate::tools::error_result("runId and durable plan state are required");
    }
    let state = match server.host.state.lock() {
        Ok(s) => s,
        Err(_) => return crate::tools::error_result("runId and durable plan state are required"),
    };
    match state.get_plan(&run_id) {
        Err(e) => crate::tools::error_result(&format!("get plan run: {e}")),
        Ok(None) => crate::tools::error_result(&format!("host plan run not found: {run_id}")),
        Ok(Some(record)) => plan_run_result(&record),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::schema::{Action, Node};

    /// A real read-effect, idempotent tool from the standalone catalog, so
    /// tests exercise the actual `Descriptor` shape rather than a
    /// hand-built stand-in (its fields are mostly private to this crate's
    /// catalog-construction code).
    fn sample_plan() -> Document {
        Document {
            contract_version: plan::CONTRACT_VERSION.to_string(),
            plan_id: "p1".to_string(),
            idempotency_key: "k1".to_string(),
            generation: 1,
            nodes: vec![Node {
                id: "n1".to_string(),
                action: Some(Action {
                    tool: "get_host_info".to_string(),
                    args: Map::new(),
                }),
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn validate_reports_levels_and_hash_for_a_valid_plan() {
        let snapshot = crate::catalog::for_mode(true);
        let doc = sample_plan();
        let result = validate_host_plan_with_snapshot(&doc, snapshot).unwrap();
        assert_eq!(result["valid"], J::Bool(true));
        assert_eq!(result["nodeCount"], J::from(1));
        assert_eq!(result["levels"], json!([["n1"]]));
        assert!(result["documentHash"]
            .as_str()
            .unwrap()
            .starts_with("sha256:"));
        assert_eq!(
            result["catalogRevision"],
            J::String(snapshot.revision.clone())
        );
    }

    #[test]
    fn validate_rejects_a_cycle_before_hashing() {
        let snapshot = crate::catalog::for_mode(true);
        let mut doc = sample_plan();
        doc.nodes[0].depends_on.push("n1".to_string());
        assert!(validate_host_plan_with_snapshot(&doc, snapshot).is_err());
    }

    #[test]
    fn handle_validate_host_plan_requires_a_plan_argument() {
        let snapshot = crate::catalog::for_mode(true);
        let result = handle_validate_host_plan(&Map::new(), snapshot);
        assert_eq!(result["isError"], J::Bool(true));
    }

    /// A standalone `transport::Server` wired through the real startup path
    /// (`app::new_runtime` + `AuthzStore::open` + `app::http_server`), so
    /// `run_host_plan`/`get_host_plan_run` exercise the actual durable store
    /// and task registry rather than a stand-in.
    fn test_server(dir: &std::path::Path) -> Arc<Server> {
        let env = crate::config::Env::from_pairs([
            ("HOME", dir.join("home").to_str().unwrap()),
            ("XDG_CONFIG_HOME", dir.join("xdg").to_str().unwrap()),
            ("OPUTE_REMOTE_AGENT_ID", "agent-under-test"),
            ("OPUTE_AGENT_MODE", "standalone"),
            (
                "OPUTE_STANDALONE_STATE_DIR",
                dir.join("state").to_str().unwrap(),
            ),
            ("HOST_MCP_BIND_HOST", "127.0.0.1"),
            ("HOST_MCP_PORT", "1"),
        ]);
        let runtime = crate::app::new_runtime(&env).unwrap();
        let cfg = runtime.config.clone();
        let authz =
            crate::store::AuthzStore::open(&cfg.standalone_state_dir, &cfg.opute_client_secret)
                .unwrap();
        Arc::new(crate::app::http_server(&cfg, authz, runtime.state.clone()).unwrap())
    }

    fn poll_until_terminal(server: &Server, run_id: &str) -> J {
        for _ in 0..200 {
            let result =
                handle_get_host_plan_run(server, &map(&[("runId", J::String(run_id.to_string()))]));
            let status = result["structuredContent"]["status"].as_str().unwrap_or("");
            if status != "working" && status != "running" && status != "pending" {
                return result;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("plan run {run_id} did not reach a terminal status in time");
    }

    fn map(pairs: &[(&str, J)]) -> Map<String, J> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    fn plan_args(doc: &Document) -> Map<String, J> {
        map(&[("plan", serde_json::to_value(doc).unwrap())])
    }

    #[test]
    fn run_host_plan_executes_a_single_node_plan_to_completion() {
        let dir = tempfile::tempdir().unwrap();
        let server = test_server(dir.path());
        let doc = sample_plan();
        let run_result = handle_run_host_plan(&server, &plan_args(&doc));
        let run_id = run_result["structuredContent"]["runId"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(!run_id.is_empty());

        let final_result = poll_until_terminal(&server, &run_id);
        assert_eq!(
            final_result["structuredContent"]["status"],
            J::String("completed".to_string())
        );
        assert_eq!(
            final_result["structuredContent"]["runId"],
            J::String(run_id)
        );
    }

    #[test]
    fn run_host_plan_is_idempotent_on_the_same_plan_identity() {
        let dir = tempfile::tempdir().unwrap();
        let server = test_server(dir.path());
        let doc = sample_plan();
        let first = handle_run_host_plan(&server, &plan_args(&doc));
        let first_run_id = first["structuredContent"]["runId"]
            .as_str()
            .unwrap()
            .to_string();

        let second = handle_run_host_plan(&server, &plan_args(&doc));
        let second_run_id = second["structuredContent"]["runId"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(first_run_id, second_run_id);

        poll_until_terminal(&server, &first_run_id);
    }

    #[test]
    fn run_host_plan_with_metadata_redacts_a_declared_secret_input_everywhere() {
        let dir = tempfile::tempdir().unwrap();
        let server = test_server(dir.path());
        let mut doc = sample_plan();
        doc.variables = map(&[("inputs", json!({"token": "super-secret-value"}))]);
        let metadata = map(&[
            ("secretInputs", json!(["token"])),
            ("recipeHash", J::String("rh1".to_string())),
        ]);

        let run_result = handle_run_host_plan_with_metadata(
            &server,
            &plan_args(&doc),
            Some(&metadata),
            "run_host_local_recipe",
            "Executing host-local recipe...",
        );
        let run_id = run_result["structuredContent"]["runId"]
            .as_str()
            .unwrap()
            .to_string();

        // The task's own stored arguments never carry the secret value.
        let task = server.tasks.get(&run_id).expect("task recorded");
        let task_json = serde_json::to_string(&task.tool_args).unwrap();
        assert!(
            !task_json.contains("super-secret-value"),
            "task args leaked a secret: {task_json}"
        );

        let final_result = poll_until_terminal(&server, &run_id);
        let final_json = final_result.to_string();
        assert!(
            !final_json.contains("super-secret-value"),
            "run result leaked a secret: {final_json}"
        );
        assert_eq!(
            final_result["structuredContent"]["recipe"]["secretInputs"],
            json!(["token"])
        );

        // The durable plan record's own stored document is redacted too.
        let stored = {
            let state = server.host.state.lock().unwrap();
            state.get_plan(&run_id).unwrap().unwrap()
        };
        assert!(
            !stored.plan_json.contains("super-secret-value"),
            "persisted plan document leaked a secret: {}",
            stored.plan_json
        );
        assert!(
            !stored.recipe_json.contains("super-secret-value"),
            "persisted recipe metadata leaked a secret: {}",
            stored.recipe_json
        );
    }

    #[test]
    fn get_host_plan_run_reports_not_found_for_an_unknown_run_id() {
        let dir = tempfile::tempdir().unwrap();
        let server = test_server(dir.path());
        let result = handle_get_host_plan_run(
            &server,
            &map(&[("runId", J::String("does-not-exist".to_string()))]),
        );
        assert_eq!(result["isError"], J::Bool(true));
    }

    #[test]
    fn get_host_plan_run_requires_a_run_id() {
        let dir = tempfile::tempdir().unwrap();
        let server = test_server(dir.path());
        let result = handle_get_host_plan_run(&server, &Map::new());
        assert_eq!(result["isError"], J::Bool(true));
    }
}
