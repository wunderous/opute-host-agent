//! Schema-owned evidence projection, shared by tasks and durable observations.
use serde_json::{Map, Value as J};
use std::collections::BTreeSet;

/// Go's `redactEvidenceBySchema`: the one projection a caller's own result
/// is delivered through, whether synchronously or later through a task
/// poll, live or restored after a restart. The caller already owns this
/// data — a secret `writeOnly` field is the only thing ever hidden; an
/// open or undeclared field passes through exactly as Go returns it, since
/// withholding it here has no security benefit and is pure functionality
/// loss.
pub fn redact_for_delivery(value: &J, schema: Option<&Map<String, J>>) -> J {
    let Some(schema) = schema else {
        return value.clone();
    };
    if schema.get("writeOnly") == Some(&J::Bool(true)) {
        return J::from("[redacted]");
    }
    match value {
        J::Object(object) => {
            let properties = schema.get("properties").and_then(J::as_object);
            let additional = schema.get("additionalProperties").and_then(J::as_object);
            J::Object(
                object
                    .iter()
                    .map(|(key, child)| {
                        let child_schema = properties
                            .and_then(|p| p.get(key))
                            .and_then(J::as_object)
                            .or(additional);
                        (key.clone(), redact_for_delivery(child, child_schema))
                    })
                    .collect(),
            )
        }
        J::Array(items) => {
            let item_schema = schema.get("items").and_then(J::as_object);
            J::Array(
                items
                    .iter()
                    .map(|i| redact_for_delivery(i, item_schema))
                    .collect(),
            )
        }
        other => other.clone(),
    }
}

/// D13 (`redact-unmarked-projections`): a declared, scoped divergence from
/// Go's `redactEvidenceBySchema`, used only for what reaches durable
/// storage (an audit row, or a stored echo of the caller's own arguments)
/// -- never for what is delivered back to a caller; see
/// [`redact_for_delivery`] for that. Go preserves a value whenever no
/// schema entry marks it write-only, including one admitted only by a bare
/// `additionalProperties: true` or one with no enclosing schema at all.
/// This storage projection instead fails closed: a value is projected in
/// its original form only when a `properties` entry or a typed (object)
/// `additionalProperties` schema names it explicitly. Anything else --
/// including open-but-untyped content -- is replaced wholesale rather than
/// recursed into, so no unmarked content reaches a durable sink verbatim.
/// An unknown *capability* must still be handled by the caller with a
/// wholesale redacted projection.
pub fn redact_for_storage(value: &J, schema: Option<&Map<String, J>>) -> J {
    let Some(schema) = schema else {
        return J::from("[redacted]");
    };
    if schema.get("writeOnly") == Some(&J::Bool(true)) {
        return J::from("[redacted]");
    }
    match value {
        J::Object(object) => {
            let properties = schema.get("properties").and_then(J::as_object);
            let additional = schema.get("additionalProperties").and_then(J::as_object);
            J::Object(
                object
                    .iter()
                    .map(|(key, child)| {
                        let child_schema = properties
                            .and_then(|p| p.get(key))
                            .and_then(J::as_object)
                            .or(additional);
                        (key.clone(), redact_for_storage(child, child_schema))
                    })
                    .collect(),
            )
        }
        J::Array(items) => {
            let item_schema = schema.get("items").and_then(J::as_object);
            J::Array(
                items
                    .iter()
                    .map(|i| redact_for_storage(i, item_schema))
                    .collect(),
            )
        }
        other => other.clone(),
    }
}

/// Capability facts and diagnostic evidence are opaque and have no field
/// schema. Go retains their provenance and replaces each present value.
#[allow(dead_code)] // Provider-owned observations are wired in M7.
pub fn redact_observation(value: &Map<String, J>, schema: Option<&Map<String, J>>) -> J {
    let mut projected = value.clone();
    if let Some(structured) = projected.get("structured") {
        projected.insert("structured".into(), redact_for_storage(structured, schema));
    }
    for key in ["facts", "evidence"] {
        if let Some(entries) = projected.get_mut(key).and_then(J::as_array_mut) {
            for entry in entries {
                if let Some(entry) = entry.as_object_mut() {
                    if entry.contains_key("value") {
                        entry.insert("value".into(), J::from("[redacted]"));
                    }
                }
            }
        }
    }
    J::Object(projected)
}

/// Go's redactPlanEvidence: project executable arguments through their
/// declared capabilities and hide recipe inputs explicitly marked secret.
#[allow(dead_code)] // M5 projection boundary; the M6 executor supplies documents.
pub fn redact_plan_document(
    value: &J,
    secret_names: &BTreeSet<String>,
    catalog: &crate::catalog::Snapshot,
) -> J {
    let Some(mut root) = value.as_object().cloned() else {
        return serde_json::json!({"redacted": true});
    };
    if let Some(inputs) = root
        .get_mut("variables")
        .and_then(J::as_object_mut)
        .and_then(|variables| variables.get_mut("inputs"))
        .and_then(J::as_object_mut)
    {
        for name in secret_names {
            if inputs.contains_key(name) {
                inputs.insert(name.clone(), J::from("[redacted]"));
            }
        }
    }
    if let Some(nodes) = root.get_mut("nodes").and_then(J::as_array_mut) {
        for node in nodes {
            let Some(node) = node.as_object_mut() else {
                continue;
            };
            for key in ["action", "validate", "compensate", "recover"] {
                let Some(action) = node.get_mut(key).and_then(J::as_object_mut) else {
                    continue;
                };
                let tool = action.get("tool").and_then(J::as_str).unwrap_or("");
                let Some(args) = action.get("args").filter(|args| args.is_object()) else {
                    continue;
                };
                if tool.trim().is_empty() {
                    continue;
                }
                let projected = match catalog
                    .tools
                    .iter()
                    .find(|descriptor| descriptor.name == tool)
                {
                    Some(descriptor) => {
                        redact_for_storage(args, descriptor.input_schema.as_object())
                    }
                    None => serde_json::json!({"redacted": true}),
                };
                action.insert("args".into(), projected);
            }
        }
    }
    J::Object(root)
}

/// Go's redactPlanRunState. Unknown derived outputs fail closed, while
/// capability-owned outputs use their schema; secret context entries retain
/// identity/provenance metadata but omit their values.
#[allow(dead_code)] // M5 projection boundary; the M6 executor supplies state.
pub fn redact_plan_run_state(
    value: &Map<String, J>,
    document: Option<&Map<String, J>>,
    catalog: &crate::catalog::Snapshot,
) -> J {
    let text = |key: &str| value.get(key).cloned().unwrap_or_else(|| J::from(""));
    let mut result = serde_json::json!({
        "runId": text("runId"), "planId": text("planId"),
        "generation": value.get("generation").cloned().unwrap_or(J::from(0)),
        "status": text("status"), "error": text("error"), "nodes": {}, "outputs": {},
    });
    let mut schemas = std::collections::BTreeMap::new();
    if let Some(nodes) = document
        .and_then(|doc| doc.get("nodes"))
        .and_then(J::as_array)
    {
        for node in nodes {
            let action = node
                .get("action")
                .filter(|v| !v.is_null())
                .or_else(|| node.get("validate").filter(|v| !v.is_null()));
            let tool = action
                .and_then(|action| action.get("tool"))
                .and_then(J::as_str);
            let id = node.get("id").and_then(J::as_str).unwrap_or("");
            if let Some(descriptor) =
                tool.and_then(|tool| catalog.tools.iter().find(|d| d.name == tool))
            {
                schemas.insert(id, descriptor.output_schema.as_ref().and_then(J::as_object));
            }
        }
    }
    if let Some(nodes) = value.get("nodes").and_then(J::as_object) {
        for (id, node) in nodes {
            let mut projected = serde_json::json!({
                "id": node.get("id").cloned().unwrap_or(J::from("")),
                "status": node.get("status").cloned().unwrap_or(J::from("")),
            });
            if node.get("attempts").and_then(J::as_i64).unwrap_or(0) != 0 {
                projected["attempts"] = node["attempts"].clone();
            }
            for key in ["output", "observed"] {
                if let Some(field) = node.get(key).filter(|v| !v.is_null()) {
                    projected[key] =
                        redact_for_storage(field, schemas.get(id.as_str()).copied().flatten());
                }
            }
            if let Some(expected) = node.get("expected").filter(|v| !v.is_null()) {
                projected["expected"] = expected.clone();
            }
            for key in ["error", "startedAt", "completedAt"] {
                if node
                    .get(key)
                    .and_then(J::as_str)
                    .is_some_and(|s| !s.is_empty())
                {
                    projected[key] = node[key].clone();
                }
            }
            result["nodes"][id] = projected;
        }
    }
    if let Some(outputs) = value.get("outputs").and_then(J::as_object) {
        for (id, output) in outputs {
            result["outputs"][id] = match schemas.get(id.as_str()) {
                Some(schema) => redact_for_storage(output, *schema),
                None => serde_json::json!({"redacted": true}),
            };
        }
    }
    let project_context = |entry: &J| {
        let mut projected = serde_json::json!({});
        for key in [
            "name",
            "schema",
            "schemaRevision",
            "producerNode",
            "source",
            "secret",
            "recordedAt",
        ] {
            projected[key] = entry.get(key).cloned().unwrap_or(match key {
                "secret" => J::Bool(false),
                "schema" => J::Null,
                _ => J::from(""),
            });
        }
        if entry.get("secret") != Some(&J::Bool(true)) {
            projected["value"] = entry.get("value").cloned().unwrap_or(J::Null);
        }
        projected
    };
    result["context"] = serde_json::json!({});
    if let Some(context) = value.get("context").and_then(J::as_object) {
        for (name, entry) in context {
            result["context"][name] = project_context(entry);
        }
    }
    if let Some(history) = value
        .get("contextHistory")
        .and_then(J::as_array)
        .filter(|v| !v.is_empty())
    {
        result["contextHistory"] = J::Array(history.iter().map(project_context).collect());
    }
    if let Some(wait) = value.get("wait").filter(|v| !v.is_null()) {
        let mut projected = serde_json::json!({});
        for key in [
            "nodeId",
            "waitId",
            "waitRevision",
            "schemaRevision",
            "trigger",
            "correlation",
            "inputSchema",
            "expiresAt",
            "status",
        ] {
            projected[key] = wait.get(key).cloned().unwrap_or(match key {
                "waitRevision" => J::from(0),
                "correlation" | "inputSchema" => J::Null,
                _ => J::from(""),
            });
        }
        result["wait"] = projected;
    }
    result
}
