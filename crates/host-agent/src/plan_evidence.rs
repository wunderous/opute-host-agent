//! Port of `internal/hostmcp/evidence_redaction.go`'s plan-specific pieces:
//! projecting a plan document, a live plan-run task value, and durable
//! `RunState` through the capability catalog and a recipe's declared secret
//! input names. Secret handling is owned by `writeOnly` schema fields or an
//! explicit secret-input name; it never infers sensitivity from argument
//! names, and a value with no schema to project through is replaced, not
//! copied, so no unmarked content reaches a durable sink or task argument
//! verbatim.

use crate::plan::schema::{Capability, ContextEntry, RunState};
use serde_json::{json, Map, Value as J};
use std::collections::{BTreeMap, BTreeSet};

const REDACTED: &str = "[redacted]";

/// `redactEvidenceBySchema`.
pub fn redact_evidence_by_schema(value: &J, schema: Option<&Map<String, J>>) -> J {
    if let Some(schema) = schema {
        if schema.get("writeOnly") == Some(&J::Bool(true)) {
            return J::String(REDACTED.to_string());
        }
    }
    match value {
        J::Object(object) => {
            let properties = schema
                .and_then(|s| s.get("properties"))
                .and_then(J::as_object);
            let additional = schema
                .and_then(|s| s.get("additionalProperties"))
                .and_then(J::as_object);
            let mut out = Map::with_capacity(object.len());
            for (key, child) in object {
                let child_schema = properties
                    .and_then(|p| p.get(key))
                    .and_then(J::as_object)
                    .or(additional);
                out.insert(key.clone(), redact_evidence_by_schema(child, child_schema));
            }
            J::Object(out)
        }
        J::Array(items) => {
            let item_schema = schema.and_then(|s| s.get("items")).and_then(J::as_object);
            J::Array(
                items
                    .iter()
                    .map(|child| redact_evidence_by_schema(child, item_schema))
                    .collect(),
            )
        }
        other => other.clone(),
    }
}

/// `recipeSecretNameSet`.
pub fn recipe_secret_name_set(metadata: Option<&Map<String, J>>) -> BTreeSet<String> {
    let mut result = BTreeSet::new();
    let Some(metadata) = metadata else {
        return result;
    };
    if let Some(J::Array(values)) = metadata.get("secretInputs") {
        for value in values {
            if let Some(name) = value.as_str() {
                result.insert(name.to_string());
            }
        }
    }
    result
}

/// `redactPlanAction`.
fn redact_plan_action(action: &mut Map<String, J>, capabilities: &BTreeMap<String, Capability>) {
    let tool = action
        .get("tool")
        .and_then(J::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let Some(args) = action.get("args").and_then(J::as_object).cloned() else {
        return;
    };
    if tool.is_empty() {
        return;
    }
    let redacted = match capabilities.get(&tool) {
        Some(capability) => {
            redact_evidence_by_schema(&J::Object(args), Some(&capability.input_schema))
        }
        None => json!({"redacted": true}),
    };
    action.insert("tool".to_string(), J::String(tool));
    action.insert("args".to_string(), redacted);
}

/// `redactPlanEvidence`: projects both the executable arguments and the
/// durable observations of a host plan through the capability catalog. A
/// plan is validated before it reaches this function, but the fail-closed
/// fallback is intentional: durable evidence must not become a second path
/// around a capability's declared secret boundary.
pub fn redact_plan_evidence(
    value: &J,
    secret_names: &BTreeSet<String>,
    capabilities: &BTreeMap<String, Capability>,
) -> J {
    let Some(root) = value.as_object() else {
        return json!({"redacted": true});
    };
    let mut projected = root.clone();
    if let Some(J::Object(inputs)) = projected
        .get_mut("variables")
        .and_then(J::as_object_mut)
        .and_then(|v| v.get_mut("inputs"))
    {
        for name in secret_names {
            if inputs.contains_key(name) {
                inputs.insert(name.clone(), J::String(REDACTED.to_string()));
            }
        }
    }
    if let Some(J::Array(nodes)) = projected.get_mut("nodes") {
        for node in nodes {
            let Some(node) = node.as_object_mut() else {
                continue;
            };
            for key in ["action", "validate", "compensate", "recover"] {
                if let Some(J::Object(action)) = node.get_mut(key) {
                    redact_plan_action(action, capabilities);
                }
            }
        }
    }
    J::Object(projected)
}

/// `redactRecipeMetadataSecrets`: the recipe-metadata-embedded expanded
/// plan's `variables.inputs` is redacted the same way the plan document's
/// are -- the metadata carries its own copy for display, not a reference.
pub fn redact_recipe_metadata_secrets(metadata: &mut Map<String, J>) {
    let secret_names = recipe_secret_name_set(Some(metadata));
    if let Some(J::Object(inputs)) = metadata
        .get_mut("expandedPlan")
        .and_then(J::as_object_mut)
        .and_then(|v| v.get_mut("variables"))
        .and_then(J::as_object_mut)
        .and_then(|v| v.get_mut("inputs"))
    {
        for name in &secret_names {
            if inputs.contains_key(name) {
                inputs.insert(name.clone(), J::String(REDACTED.to_string()));
            }
        }
    }
}

/// `redactPlanDocument`: the redacted plan document re-encoded, used to
/// derive the stable, secret-free identity hash and the durable record.
pub fn redact_plan_document(
    document: &J,
    metadata: Option<&Map<String, J>>,
    capabilities: &BTreeMap<String, Capability>,
) -> J {
    redact_plan_evidence(document, &recipe_secret_name_set(metadata), capabilities)
}

/// `redactedMetadata`: a deep copy of the recipe metadata with every
/// secret-input-named value masked. `None` (the bare `run_host_plan` path)
/// has nothing to redact and is never persisted.
pub fn redacted_metadata(metadata: Option<&Map<String, J>>) -> Option<Map<String, J>> {
    let metadata = metadata?;
    let mut clone = metadata.clone();
    redact_recipe_metadata_secrets(&mut clone);
    Some(clone)
}

/// `recipeHash`.
pub fn recipe_hash(metadata: &Map<String, J>) -> String {
    metadata
        .get("recipeHash")
        .and_then(J::as_str)
        .unwrap_or("")
        .trim()
        .to_string()
}

/// `persistedRecipeHash`.
pub fn persisted_recipe_hash(encoded: &str) -> String {
    if encoded.trim().is_empty() {
        return String::new();
    }
    serde_json::from_str::<Map<String, J>>(encoded)
        .ok()
        .map(|m| recipe_hash(&m))
        .unwrap_or_default()
}

fn clone_evidence_value(value: &J) -> J {
    value.clone()
}

/// `redactPlanRunState`: the live/persisted run-state projection, with each
/// node's `output`/`observed` redacted through that node's own capability
/// output schema (falling back to fail-closed `{"redacted": true}` for any
/// output with no node schema to project through, matching Go's handling of
/// activation/recipe-observation outputs) and every `secret`-marked context
/// entry's value withheld entirely.
pub fn redact_plan_run_state(
    state: &RunState,
    node_schemas: &BTreeMap<String, Map<String, J>>,
) -> J {
    let mut nodes = Map::new();
    for (id, node_value) in &state.nodes {
        let mut node = Map::new();
        node.insert("id".to_string(), J::String(node_value.id.clone()));
        node.insert("status".to_string(), J::String(node_value.status.clone()));
        if node_value.attempts != 0 {
            node.insert("attempts".to_string(), J::from(node_value.attempts));
        }
        let schema = node_schemas.get(id);
        if let Some(output) = &node_value.output {
            node.insert(
                "output".to_string(),
                redact_evidence_by_schema(output, schema),
            );
        }
        if let Some(observed) = &node_value.observed {
            node.insert(
                "observed".to_string(),
                redact_evidence_by_schema(observed, schema),
            );
        }
        if let Some(expected) = &node_value.expected {
            node.insert("expected".to_string(), clone_evidence_value(expected));
        }
        if !node_value.error.is_empty() {
            node.insert("error".to_string(), J::String(node_value.error.clone()));
        }
        if !node_value.started_at.is_empty() {
            node.insert(
                "startedAt".to_string(),
                J::String(node_value.started_at.clone()),
            );
        }
        if !node_value.completed_at.is_empty() {
            node.insert(
                "completedAt".to_string(),
                J::String(node_value.completed_at.clone()),
            );
        }
        nodes.insert(id.clone(), J::Object(node));
    }
    let mut outputs = Map::new();
    for (id, output) in &state.outputs {
        outputs.insert(
            id.clone(),
            match node_schemas.get(id) {
                Some(schema) => redact_evidence_by_schema(output, Some(schema)),
                None => json!({"redacted": true}),
            },
        );
    }
    let mut context = Map::new();
    for (name, entry) in &state.context {
        context.insert(name.clone(), context_entry_projection(entry));
    }
    let mut result = Map::new();
    result.insert("runId".to_string(), J::String(state.run_id.clone()));
    result.insert("planId".to_string(), J::String(state.plan_id.clone()));
    result.insert("generation".to_string(), J::from(state.generation));
    result.insert("status".to_string(), J::String(state.status.clone()));
    result.insert("nodes".to_string(), J::Object(nodes));
    result.insert("outputs".to_string(), J::Object(outputs));
    result.insert("error".to_string(), J::String(state.error.clone()));
    result.insert("context".to_string(), J::Object(context));
    if !state.context_history.is_empty() {
        result.insert(
            "contextHistory".to_string(),
            J::Array(
                state
                    .context_history
                    .iter()
                    .map(context_entry_projection)
                    .collect(),
            ),
        );
    }
    if let Some(wait) = &state.wait {
        result.insert(
            "wait".to_string(),
            json!({
                "nodeId": wait.node_id, "waitId": wait.wait_id, "waitRevision": wait.wait_revision,
                "schemaRevision": wait.schema_revision, "trigger": wait.trigger,
                "correlation": clone_evidence_value(&J::Object(wait.correlation.clone())),
                "inputSchema": clone_evidence_value(&J::Object(wait.input_schema.clone())),
                "expiresAt": wait.expires_at, "status": wait.status,
            }),
        );
    }
    J::Object(result)
}

fn context_entry_projection(entry: &ContextEntry) -> J {
    let mut projected = Map::new();
    projected.insert("name".to_string(), J::String(entry.name.clone()));
    projected.insert(
        "schema".to_string(),
        clone_evidence_value(&J::Object(entry.schema.clone())),
    );
    projected.insert(
        "schemaRevision".to_string(),
        J::String(entry.schema_revision.clone()),
    );
    projected.insert(
        "producerNode".to_string(),
        J::String(entry.producer_node.clone()),
    );
    projected.insert("source".to_string(), J::String(entry.source.clone()));
    projected.insert("secret".to_string(), J::Bool(entry.secret));
    projected.insert(
        "recordedAt".to_string(),
        J::String(entry.recorded_at.clone()),
    );
    if !entry.secret {
        if let Some(value) = &entry.value {
            projected.insert("value".to_string(), clone_evidence_value(value));
        }
    }
    J::Object(projected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::schema::NodeRunState;

    fn capability(name: &str, schema: J) -> (String, Capability) {
        (
            name.to_string(),
            Capability {
                name: name.to_string(),
                input_schema: schema.as_object().cloned().unwrap_or_default(),
                output_schema: Map::new(),
                effect: "mutate".to_string(),
                idempotent: false,
            },
        )
    }

    #[test]
    fn write_only_schema_redacts_regardless_of_field_name() {
        let schema: Map<String, J> = json!({"writeOnly": true}).as_object().unwrap().clone();
        assert_eq!(
            redact_evidence_by_schema(&json!("secret-value"), Some(&schema)),
            J::String(REDACTED.to_string())
        );
    }

    #[test]
    fn nested_object_projects_by_properties_schema() {
        let schema: Map<String, J> =
            json!({"properties": {"token": {"writeOnly": true}, "keep": {}}})
                .as_object()
                .unwrap()
                .clone();
        let value = json!({"token": "t", "keep": 1});
        assert_eq!(
            redact_evidence_by_schema(&value, Some(&schema)),
            json!({"token": "[redacted]", "keep": 1})
        );
    }

    #[test]
    fn redact_plan_evidence_masks_secret_variables_and_unknown_tool_args() {
        let capabilities: BTreeMap<String, Capability> =
            [capability("known_tool", json!({"properties": {"x": {}}}))]
                .into_iter()
                .collect();
        let secret_names: BTreeSet<String> = ["token".to_string()].into_iter().collect();
        let document = json!({
            "variables": {"inputs": {"token": "abc", "other": "ok"}},
            "nodes": [
                {"id": "n1", "action": {"tool": "known_tool", "args": {"x": 1}}},
                {"id": "n2", "action": {"tool": "unknown_tool", "args": {"x": 1}}},
            ],
        });
        let redacted = redact_plan_evidence(&document, &secret_names, &capabilities);
        assert_eq!(
            redacted["variables"]["inputs"]["token"],
            J::String(REDACTED.to_string())
        );
        assert_eq!(
            redacted["variables"]["inputs"]["other"],
            J::String("ok".to_string())
        );
        assert_eq!(redacted["nodes"][0]["action"]["args"], json!({"x": 1}));
        assert_eq!(
            redacted["nodes"][1]["action"]["args"],
            json!({"redacted": true})
        );
    }

    #[test]
    fn redact_plan_run_state_fails_closed_on_an_output_with_no_node_schema() {
        let mut state = RunState {
            run_id: "r1".to_string(),
            plan_id: "p1".to_string(),
            generation: 1,
            status: "completed".to_string(),
            ..Default::default()
        };
        state.nodes.insert(
            "n1".to_string(),
            NodeRunState {
                id: "n1".to_string(),
                status: "applied".to_string(),
                ..Default::default()
            },
        );
        state
            .outputs
            .insert("n1".to_string(), json!({"secret": "leak"}));
        let redacted = redact_plan_run_state(&state, &BTreeMap::new());
        assert_eq!(redacted["outputs"]["n1"], json!({"redacted": true}));
    }

    #[test]
    fn redact_plan_run_state_withholds_secret_context_values() {
        let mut state = RunState {
            run_id: "r1".to_string(),
            plan_id: "p1".to_string(),
            generation: 1,
            status: "running".to_string(),
            ..Default::default()
        };
        state.context.insert(
            "token".to_string(),
            ContextEntry {
                name: "token".to_string(),
                value: Some(json!("abc")),
                secret: true,
                ..Default::default()
            },
        );
        let redacted = redact_plan_run_state(&state, &BTreeMap::new());
        assert_eq!(redacted["context"]["token"].get("value"), None);
        assert_eq!(redacted["context"]["token"]["secret"], J::Bool(true));
    }
}
