//! Test-only process seam for deterministic M5 crashes in actual store writes.
//! No endpoint, CLI mode, or production capability is added.
use crate::store::{ActiveCapabilityRecord, PlanRecord, StateStore};
use serde_json::json;
use std::io::{BufRead, Write};
use std::path::Path;

#[test]
#[ignore = "requires an isolated parity driver and state directory"]
fn crash_writer() {
    let directory = std::env::var("PARITY_STATE_DIR").expect("isolated state directory");
    let action = std::env::var("PARITY_STATE_ACTION").expect("fixture action");
    let task = std::env::var("PARITY_TASK_ID").expect("accepted task identity");
    let mut store = StateStore::open(Path::new(&directory)).expect("open fixture store");
    for (id, status) in [("old", "completed"), ("new", "running")] {
        store
            .create_plan(&PlanRecord {
                run_id: id.into(),
                plan_id: id.into(),
                generation: 1,
                idempotency_key: id.into(),
                document_hash: "sha256:fixture".into(),
                catalog_revision: "sha256:fixture".into(),
                status: status.into(),
                plan_json: "{}".into(),
                recipe_json: "{}".into(),
                state_json: r#"{"checkpoint":"original"}"#.into(),
                ..Default::default()
            })
            .expect("seed actual plan creation");
    }
    let active = |id: &str| ActiveCapabilityRecord {
        capability: "fixture".into(),
        serving_contract: "fixture.v1".into(),
        provider: "fixture".into(),
        recipe_id: id.into(),
        recipe_version: "1".into(),
        recipe_hash: "sha256:fixture".into(),
        run_id: id.into(),
        input_bindings_json: "{}".into(),
        observation_json: "{}".into(),
        ..Default::default()
    };
    store
        .complete_plan_with_active_capability("old", r#"{"checkpoint":"original"}"#, &active("old"))
        .expect("seed previous active selection");
    println!("PARITY_SETUP");
    std::io::stdout().flush().unwrap();
    let mut command = String::new();
    std::io::stdin().lock().read_line(&mut command).unwrap();
    assert_eq!(command.trim(), "GO");
    let result = match action.as_str() {
        "operation-create" => store.create_operation(&task, "fixture", "replacement"),
        "operation-complete" => store.complete_operation(&task, &json!({"checkpoint":"changed"})),
        "operation-fail" => store.fail_operation(&task, "changed"),
        "operation-cancel" => store.cancel_operation(&task),
        "task-snapshot" => store.save_registry_task_snapshot(&task, "request_task_input", "changed", &json!({
            "taskId": task, "status":"completed", "resultType":"complete", "toolName":"request_task_input",
            "createdAt":"2026-10-04T00:00:00Z", "lastUpdatedAt":"2026-10-04T00:00:00Z",
            "result":{"checkpoint":"changed"}, "toolArgs":{}
        })),
        "plan-update" => store.update_plan("new", "completed", r#"{"checkpoint":"changed"}"#, ""),
        "plan-complete" => store.complete_plan_with_active_capability("new", r#"{"checkpoint":"changed"}"#, &active("new")),
        _ => panic!("unknown fixture action"),
    };
    result.expect("actual storage write");
    println!("PARITY_COMMITTED");
}
