//! MCP-layer glue for the `host-recipe.v1` host-local family
//! (`internal/hostmcp/host_recipe_run.go`): sourcing with a pinned hash,
//! input resolution, compatibility, and the refusal to execute anything the
//! contract says belongs to the Platform. Execution itself is the exact
//! same durable `host-plan.v1` runner every other recipe kind uses
//! (`plan_mcp::handle_run_host_plan_with_metadata`), so idempotency,
//! readiness, recovery and resume are the existing ones rather than a
//! second implementation.

use crate::plan::interpolate::{interpolate_args, EvalContext};
use crate::plan_mcp::{
    capabilities_from_snapshot, handle_run_host_plan_with_metadata,
    validate_host_plan_with_snapshot,
};
use crate::recipe::host_recipe::{self as host_recipe, HostLoaded};
use crate::recipe::source::{validate_host_agent_version, SourceRequest};
use crate::transport::Server;
use serde_json::{json, Map, Value as J};
use std::sync::Arc;

fn string_field(args: &Map<String, J>, key: &str) -> String {
    args.get(key)
        .and_then(J::as_str)
        .unwrap_or("")
        .trim()
        .to_string()
}

fn bool_field(args: &Map<String, J>, key: &str) -> bool {
    args.get(key).and_then(J::as_bool).unwrap_or(false)
}

fn source_request(args: &Map<String, J>, require_hash: bool) -> SourceRequest {
    SourceRequest {
        source: string_field(args, "source"),
        revision: string_field(args, "revision"),
        sha256: string_field(args, "sha256"),
        require_sha256: require_hash,
    }
}

fn input_values(args: &Map<String, J>) -> Result<Map<String, J>, String> {
    match args.get("inputs") {
        None | Some(J::Null) => Ok(Map::new()),
        Some(J::Object(values)) => Ok(values.clone()),
        Some(_) => Err("inputs must be an object".to_string()),
    }
}

/// The runner checks each target again at dispatch. This check exists
/// because by then the earlier nodes have already mutated the host: a
/// recipe addressed to a peer should be refused before the first of them
/// runs, not halfway through.
fn assert_host_local_targets(server: &Server, loaded: &HostLoaded) -> Result<(), String> {
    let agent_id = server.host.agent_id.trim();
    if agent_id.is_empty() {
        return Ok(());
    }
    for node in &loaded.expanded_plan.nodes {
        let Some(target) = &node.target else { continue };
        let context = EvalContext {
            variables: loaded.expanded_plan.variables.clone(),
            ..Default::default()
        };
        let args: Map<String, J> = [("hostRef".to_string(), J::String(target.host_ref.clone()))]
            .into_iter()
            .collect();
        let resolved = interpolate_args(&args, &context)
            .map_err(|e| format!("resolve host-local target for node \"{}\": {e}", node.id))?;
        let host_ref = resolved
            .get("hostRef")
            .and_then(J::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if host_ref != agent_id {
            return Err(format!(
                "host-local node \"{}\" targets Host Agent \"{host_ref}\", not this one (\"{agent_id}\"); a host-local recipe may only act on the host executing it",
                node.id
            ));
        }
    }
    Ok(())
}

fn host_local_recipe_metadata(loaded: &HostLoaded) -> Map<String, J> {
    let mut metadata = Map::new();
    metadata.insert(
        "contractVersion".to_string(),
        J::String(host_recipe::HOST_CONTRACT_VERSION.to_string()),
    );
    metadata.insert(
        "recipeKind".to_string(),
        J::String("host-local".to_string()),
    );
    metadata.insert(
        "recipeId".to_string(),
        J::String(loaded.document.recipe_id.clone()),
    );
    metadata.insert(
        "recipeVersion".to_string(),
        J::String(loaded.document.recipe_version.clone()),
    );
    metadata.insert(
        "execution".to_string(),
        serde_json::to_value(&loaded.document.execution).unwrap_or(J::Null),
    );
    metadata.insert(
        "source".to_string(),
        serde_json::to_value(&loaded.source).unwrap_or(J::Null),
    );
    metadata.insert("inputs".to_string(), J::Object(loaded.redacted_inputs()));
    metadata.insert(
        "secretInputs".to_string(),
        J::Array(
            loaded
                .secret_input_names()
                .into_iter()
                .map(J::String)
                .collect(),
        ),
    );
    metadata.insert(
        "recipeHash".to_string(),
        J::String(loaded.source.recipe_hash.clone()),
    );
    metadata.insert(
        "recipeDocument".to_string(),
        serde_json::to_value(&loaded.document).unwrap_or(J::Null),
    );
    metadata.insert(
        "outputMapping".to_string(),
        serde_json::to_value(&loaded.document.output_mapping).unwrap_or(J::Null),
    );
    metadata.insert(
        "expandedPlan".to_string(),
        serde_json::to_value(&loaded.expanded_plan).unwrap_or(J::Null),
    );
    metadata
}

/// `loadHostLocalRecipe`.
fn load_host_local_recipe(
    server: &Server,
    args: &Map<String, J>,
    require_hash: bool,
) -> Result<HostLoaded, String> {
    let sourced = host_recipe::load_host(&source_request(args, require_hash))?;
    if !sourced.document.execution.is_host_local() {
        return Err(format!(
            "host recipe \"{}\" declares coordinator={}/mode={}; submit it to the Platform coordinator, which owns the wait fences, resume revisions and cross-host dispatch a Host Agent cannot provide",
            sourced.document.recipe_id, sourced.document.execution.coordinator, sourced.document.execution.mode
        ));
    }
    validate_host_agent_version(
        &sourced.document.compatibility.min_host_agent_version,
        env!("CARGO_PKG_VERSION"),
    )?;
    let values = input_values(args)?;
    let mut resolved = host_recipe::resolve_host_inputs(sourced.document, &values)?;
    resolved.source = sourced.source;
    resolved.raw = sourced.raw;
    let capabilities = capabilities_from_snapshot(server.catalog);
    resolved.validate(&capabilities, &server.catalog.revision)?;
    assert_host_local_targets(server, &resolved)?;
    Ok(resolved)
}

/// `handleValidateHostLocalRecipe`.
pub fn handle_validate_host_local_recipe(server: &Server, args: &Map<String, J>) -> J {
    let loaded = match load_host_local_recipe(server, args, false) {
        Ok(l) => l,
        Err(e) => return crate::tools::error_result(&e),
    };
    let plan_result = match validate_host_plan_with_snapshot(&loaded.expanded_plan, server.catalog)
    {
        Ok(r) => r,
        Err(e) => return crate::tools::error_result(&e),
    };
    crate::tools::structured_result(
        json!({
            "valid": true,
            "contractVersion": host_recipe::HOST_CONTRACT_VERSION,
            "recipeId": loaded.document.recipe_id,
            "recipeVersion": loaded.document.recipe_version,
            "execution": loaded.document.execution,
            "hostAgentId": server.host.agent_id,
            "source": loaded.source,
            "inputs": loaded.redacted_inputs(),
            "recipeHash": loaded.source.recipe_hash,
            "rawSha256": loaded.source.raw_sha256,
            "plan": plan_result,
        }),
        Some("host-local recipe is valid"),
    )
}

/// `handleRunHostLocalRecipe`.
pub fn handle_run_host_local_recipe(server: &Arc<Server>, args: &Map<String, J>) -> J {
    let loaded = match load_host_local_recipe(server, args, true) {
        Ok(l) => l,
        Err(e) => return crate::tools::error_result(&e),
    };
    let metadata = host_local_recipe_metadata(&loaded);
    let plan_args: Map<String, J> = [
        (
            "plan".to_string(),
            serde_json::to_value(&loaded.expanded_plan).unwrap_or(J::Null),
        ),
        ("resume".to_string(), J::Bool(bool_field(args, "resume"))),
    ]
    .into_iter()
    .collect();
    handle_run_host_plan_with_metadata(
        server,
        &plan_args,
        Some(&metadata),
        "run_host_local_recipe",
        "Executing host-local recipe...",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A standalone `transport::Server`, built the same way
    /// `plan_mcp::tests::test_server` is: through the real startup path, so
    /// `agent_id` is a known value (`"agent-under-test"`) a recipe's target
    /// can address.
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

    fn write_recipe(dir: &std::path::Path, document: &J) -> String {
        let path = dir.join("recipe.json");
        std::fs::write(&path, serde_json::to_vec(document).unwrap()).unwrap();
        path.to_str().unwrap().to_string()
    }

    fn host_local_document(secret: bool) -> J {
        let mut inputs = Map::new();
        inputs.insert(
            "agentRef".to_string(),
            json!({"default": "agent-under-test"}),
        );
        if secret {
            inputs.insert(
                "token".to_string(),
                json!({"secret": true, "required": true}),
            );
        }
        json!({
            "contractVersion": "host-recipe.v1",
            "recipeId": "example.host-local",
            "recipeVersion": "1.0.0",
            "execution": {"coordinator": "host-agent", "mode": "local"},
            "inputs": inputs,
            "plan": {
                "contractVersion": crate::plan::schema::CONTRACT_VERSION,
                "planId": "host-local-plan",
                "idempotencyKey": "k1",
                "generation": 1,
                "nodes": [
                    {
                        "id": "n1",
                        "target": {"hostRef": "${vars.inputs.agentRef}"},
                        "action": {"tool": "get_host_info", "args": {}},
                    }
                ],
            },
        })
    }

    fn poll_until_terminal(server: &Server, run_id: &str) -> J {
        for _ in 0..200 {
            let result = crate::plan_mcp::handle_get_host_plan_run(
                server,
                &[("runId".to_string(), J::String(run_id.to_string()))]
                    .into_iter()
                    .collect(),
            );
            let status = result["structuredContent"]["status"].as_str().unwrap_or("");
            if status != "working" && status != "running" && status != "pending" {
                return result;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("host-local recipe run {run_id} did not reach a terminal status in time");
    }

    #[test]
    fn validate_reports_the_recipe_and_its_expanded_plan() {
        let dir = tempfile::tempdir().unwrap();
        let server = test_server(dir.path());
        let path = write_recipe(dir.path(), &host_local_document(false));
        let args: Map<String, J> = [("source".to_string(), J::String(path))]
            .into_iter()
            .collect();
        let result = handle_validate_host_local_recipe(&server, &args);
        assert_eq!(result["structuredContent"]["valid"], J::Bool(true));
        assert_eq!(
            result["structuredContent"]["recipeId"],
            J::String("example.host-local".to_string())
        );
        assert_eq!(result["structuredContent"]["plan"]["valid"], J::Bool(true));
    }

    #[test]
    fn validate_rejects_a_recipe_targeting_a_different_host_agent() {
        let dir = tempfile::tempdir().unwrap();
        let server = test_server(dir.path());
        let path = write_recipe(dir.path(), &host_local_document(false));
        let args: Map<String, J> = [
            ("source".to_string(), J::String(path)),
            (
                "inputs".to_string(),
                json!({"agentRef": "some-other-agent"}),
            ),
        ]
        .into_iter()
        .collect();
        let result = handle_validate_host_local_recipe(&server, &args);
        assert_eq!(result["isError"], J::Bool(true));
    }

    #[test]
    fn run_host_local_recipe_executes_and_redacts_its_secret_input() {
        let dir = tempfile::tempdir().unwrap();
        let server = test_server(dir.path());
        let path = write_recipe(dir.path(), &host_local_document(true));
        let args: Map<String, J> = [
            ("source".to_string(), J::String(path)),
            ("inputs".to_string(), json!({"token": "super-secret-value"})),
        ]
        .into_iter()
        .collect();
        let run_result = handle_run_host_local_recipe(&server, &args);
        let run_id = run_result["structuredContent"]["runId"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(!run_result.to_string().contains("super-secret-value"));

        let final_result = poll_until_terminal(&server, &run_id);
        assert_eq!(
            final_result["structuredContent"]["status"],
            J::String("completed".to_string())
        );
        let final_json = final_result.to_string();
        assert!(
            !final_json.contains("super-secret-value"),
            "run result leaked a secret: {final_json}"
        );
        assert_eq!(
            final_result["structuredContent"]["recipe"]["recipeId"],
            J::String("example.host-local".to_string())
        );
    }
}
