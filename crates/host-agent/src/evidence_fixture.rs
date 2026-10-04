//! Test-only full-catalog projection and actual durable sink sweep.
use super::*;
use crate::store::{ActiveCapabilityRecord, AuthzStore, PlanRecord, StateStore};
use std::collections::BTreeSet;
use std::io::{BufRead, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};

#[test]
#[ignore = "requires the isolated M5 secret-canary driver"]
fn durable_projection() {
    let input = std::env::var("PARITY_EVIDENCE_SPEC").expect("isolated fixture specification");
    let spec: J = serde_json::from_slice(&std::fs::read(input).unwrap()).unwrap();
    let env = crate::config::Env::from_process();
    let cfg = crate::config::Config::load(&env);
    let directory = std::env::var("OPUTE_STANDALONE_STATE_DIR").unwrap();
    let state = Arc::new(Mutex::new(StateStore::open(Path::new(&directory)).unwrap()));
    let authz = AuthzStore::open(Path::new(&directory), "fixture-auth-secret").unwrap();
    let server = crate::app::http_server(&cfg, authz, state).unwrap();
    let mut projected = Map::new();
    projected.insert("_catalog".into(), snapshot_json(server.catalog));
    for case in spec["cases"].as_array().unwrap() {
        let id = case["id"].as_str().unwrap();
        let tool = case["tool"].as_str().unwrap();
        let args = case["arguments"].as_object().unwrap();
        let task_args = redact_task_args(&server, tool, args);
        let (task, _) = server.tasks.create(tool, "fixture", task_args.clone());
        server
            .host
            .state
            .lock()
            .unwrap()
            .create_operation(&task.task_id, tool, "fixture")
            .unwrap();
        persist_task(&server, &task);
        server.tasks.complete(
            &task.task_id,
            crate::tasks::ToolResult {
                structured_content: Some(json!({"marker":"safe"})),
                ..Default::default()
            },
        );
        persist_task(&server, &server.tasks.get(&task.task_id).unwrap());
        let reservation = crate::resource::Reservation {
            id: "control".into(),
            request: Default::default(),
        };
        record_invocation(
            &server,
            tool,
            args,
            &Binding::default(),
            &reservation,
            &json!({"structuredContent":{"marker":"safe"},"isError":false,"content":[]}),
        );
        let secrets = BTreeSet::from(["recipeSecret".into()]);
        let document =
            crate::evidence::redact_plan_document(&case["document"], &secrets, server.catalog);
        let run_state = crate::evidence::redact_plan_run_state(
            case["state"].as_object().unwrap(),
            case["document"].as_object(),
            server.catalog,
        );
        let mut state = server.host.state.lock().unwrap();
        state
            .create_plan(&PlanRecord {
                run_id: id.into(),
                plan_id: id.into(),
                generation: 1,
                idempotency_key: id.into(),
                document_hash: "sha256:fixture".into(),
                catalog_revision: server.catalog.revision.clone(),
                status: "completed".into(),
                plan_json: document.to_string(),
                recipe_json: document.to_string(),
                state_json: run_state.to_string(),
                ..Default::default()
            })
            .unwrap();
        state
            .complete_plan_with_active_capability(
                id,
                &run_state.to_string(),
                &ActiveCapabilityRecord {
                    capability: id.into(),
                    serving_contract: "fixture.v1".into(),
                    provider: "fixture".into(),
                    recipe_id: id.into(),
                    recipe_version: "1".into(),
                    recipe_hash: "sha256:fixture".into(),
                    run_id: id.into(),
                    input_bindings_json: task_args.to_string(),
                    observation_json: run_state.to_string(),
                    ..Default::default()
                },
            )
            .unwrap();
        projected.insert(
            id.into(),
            json!({"arguments":task_args,"document":document,"state":run_state}),
        );
        projected[id]["taskId"] = J::from(task.task_id);
    }
    std::fs::write(
        spec["output"].as_str().unwrap(),
        J::Object(projected).to_string(),
    )
    .unwrap();
    println!("PARITY_EVIDENCE_READY");
    std::io::stdout().flush().unwrap();
    let mut command = String::new();
    std::io::stdin().lock().read_line(&mut command).unwrap();
    assert_eq!(command.trim(), "GO");
}
