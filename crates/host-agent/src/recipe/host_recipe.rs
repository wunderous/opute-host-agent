//! Port of `internal/recipe/host_recipe.go`: the `host-recipe.v1` envelope
//! -- the recipe family M6 names explicitly (`host_recipe_run`). Shares the
//! plan executor and source-loading machinery with every other recipe
//! family; it never gets a second execution engine.

use super::source::{
    canonical_hash, decode_value, load_source, reserved_plan_variables, resolve_inputs,
    resolve_plan_identity, sorted_strings, CompatibilitySpec, InputSpec, SourceMetadata,
    SourceRequest,
};
use crate::plan::schema::{self as plan, Capability, Document as PlanDocument};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

pub const HOST_CONTRACT_VERSION: &str = "host-recipe.v1";

pub const HOST_COORDINATOR_PLATFORM: &str = "platform";
pub const HOST_MODE_DISTRIBUTED: &str = "distributed";
pub const HOST_COORDINATOR_HOST_AGENT: &str = "host-agent";
pub const HOST_MODE_LOCAL: &str = "local";

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct HostExecution {
    pub coordinator: String,
    pub mode: String,
}

impl HostExecution {
    pub fn is_host_local(&self) -> bool {
        self.coordinator == HOST_COORDINATOR_HOST_AGENT && self.mode == HOST_MODE_LOCAL
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct HostDocument {
    #[serde(rename = "contractVersion")]
    pub contract_version: String,
    #[serde(rename = "recipeId")]
    pub recipe_id: String,
    #[serde(rename = "recipeVersion")]
    pub recipe_version: String,
    pub execution: HostExecution,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub inputs: BTreeMap<String, InputSpec>,
    #[serde(default)]
    pub compatibility: CompatibilitySpec,
    pub plan: PlanDocument,
    #[serde(
        rename = "outputMapping",
        default,
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub output_mapping: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default)]
pub struct HostLoaded {
    pub document: HostDocument,
    pub expanded_plan: PlanDocument,
    pub inputs: Map<String, Value>,
    pub source: SourceMetadata,
    pub raw: Vec<u8>,
}

impl HostLoaded {
    pub fn redacted_inputs(&self) -> Map<String, Value> {
        let mut redacted = Map::with_capacity(self.inputs.len());
        for (name, value) in &self.inputs {
            let secret = self
                .document
                .inputs
                .get(name)
                .map(|spec| spec.secret)
                .unwrap_or(false);
            redacted.insert(
                name.clone(),
                if secret {
                    Value::String("[redacted]".to_string())
                } else {
                    value.clone()
                },
            );
        }
        redacted
    }

    pub fn secret_input_names(&self) -> Vec<String> {
        let names: Vec<String> = self
            .document
            .inputs
            .iter()
            .filter(|(_, spec)| spec.secret)
            .map(|(name, _)| name.clone())
            .collect();
        sorted_strings(names)
    }

    pub fn validate(
        &self,
        capabilities: &BTreeMap<String, Capability>,
        catalog_revision: &str,
    ) -> Result<(), String> {
        validate_host_envelope(&self.document)?;
        if self.expanded_plan.contract_version.is_empty() {
            return Err("host recipe plan is not resolved; resolve inputs first".to_string());
        }
        for name in &self.document.compatibility.required_tools {
            if !capabilities.contains_key(name) {
                return Err(format!(
                    "host recipe requires unsupported host-agent capability \"{name}\""
                ));
            }
        }
        reject_nested_plan_runs(&self.expanded_plan)?;
        validate_host_targets(&self.expanded_plan, self.document.execution.is_host_local())?;
        plan::validate(&self.expanded_plan, capabilities, catalog_revision)
            .map_err(|e| format!("validate host recipe plan: {e}"))?;
        Ok(())
    }
}

pub fn load_host(request: &SourceRequest) -> Result<HostLoaded, String> {
    let (raw, mut metadata) = load_source(request)?;
    let document: HostDocument = decode_value(&raw, "host recipe")?;
    validate_host_envelope(&document)?;
    reject_host_local_event_bindings(&raw)?;
    let hash = canonical_hash(&document).map_err(|e| format!("hash host recipe: {e}"))?;
    metadata.recipe_hash = hash;
    Ok(HostLoaded {
        document,
        source: metadata,
        raw,
        ..Default::default()
    })
}

// Mirrors Go's exported `DecodeHost`; no caller needs a decode-without-load
// path yet (`load_host` always decodes as part of fetching the source).
#[allow(dead_code)]
pub fn decode_host(raw: &[u8]) -> Result<HostDocument, String> {
    decode_value(raw, "host recipe")
}

pub fn resolve_host_inputs(
    document: HostDocument,
    values: &Map<String, Value>,
) -> Result<HostLoaded, String> {
    validate_host_envelope(&document)?;
    let resolved = resolve_inputs(&document.inputs, values)?;
    let mut variables = reserved_plan_variables(&document.plan.variables);
    variables.insert("inputs".to_string(), Value::Object(resolved.clone()));
    let mut expanded = document.plan.clone();
    expanded.variables = variables.clone();
    resolve_plan_identity(&mut expanded, &variables)?;
    Ok(HostLoaded {
        document,
        expanded_plan: expanded,
        inputs: resolved,
        ..Default::default()
    })
}

pub fn validate_host_envelope(document: &HostDocument) -> Result<(), String> {
    if document.contract_version != HOST_CONTRACT_VERSION {
        return Err(format!(
            "unsupported host recipe contractVersion \"{}\"",
            document.contract_version
        ));
    }
    if document.recipe_id.trim().is_empty() || document.recipe_version.trim().is_empty() {
        return Err("recipeId and recipeVersion are required".to_string());
    }
    let distributed = document.execution.coordinator == HOST_COORDINATOR_PLATFORM
        && document.execution.mode == HOST_MODE_DISTRIBUTED;
    if !distributed && !document.execution.is_host_local() {
        return Err(format!(
            "execution must be coordinator={HOST_COORDINATOR_PLATFORM}/mode={HOST_MODE_DISTRIBUTED} or coordinator={HOST_COORDINATOR_HOST_AGENT}/mode={HOST_MODE_LOCAL}"
        ));
    }
    if document.plan.contract_version != plan::CONTRACT_VERSION {
        return Err(format!(
            "host recipe plan must use {}",
            plan::CONTRACT_VERSION
        ));
    }
    if document.plan.nodes.is_empty() {
        return Err("host recipe plan must contain at least one node".to_string());
    }
    if document.execution.is_host_local() {
        return validate_host_local_plan(&document.plan);
    }
    Ok(())
}

/// The three restrictions that keep host-local execution from becoming a
/// second coordinator with none of the durable machinery that makes the
/// Platform one: a single host, no waits, and no emitted events.
fn validate_host_local_plan(document: &PlanDocument) -> Result<(), String> {
    let mut seen = std::collections::BTreeSet::new();
    let mut ordered = Vec::new();
    for node in &document.nodes {
        if node.wait.is_some() {
            return Err(format!("host-local node \"{}\" declares a wait; a Host Agent has no durable resume channel for one", node.id));
        }
        let Some(target) = &node.target else { continue };
        let reference = target.host_ref.trim().to_string();
        if seen.insert(reference.clone()) {
            ordered.push(reference);
        }
    }
    if ordered.len() > 1 {
        let sorted = sorted_strings(ordered.clone());
        return Err(format!(
            "host-local recipe targets {} hosts ({}); it may only act on the host executing it",
            ordered.len(),
            sorted.join(", ")
        ));
    }
    Ok(())
}

/// Reads the raw document because `plan::Node` carries no `emits` field: a
/// Host Agent has no event channel, so an `emits` declaration would
/// otherwise be silently dropped in decoding and the author would believe
/// an event was published.
fn reject_host_local_event_bindings(raw: &[u8]) -> Result<(), String> {
    #[derive(Deserialize, Default)]
    struct GenericNode {
        id: String,
        #[serde(default)]
        emits: Map<String, Value>,
    }
    #[derive(Deserialize, Default)]
    struct GenericPlan {
        #[serde(default)]
        nodes: Vec<GenericNode>,
    }
    #[derive(Deserialize, Default)]
    struct Generic {
        execution: HostExecution,
        #[serde(default)]
        plan: GenericPlan,
    }
    let generic: Generic = decode_value(raw, "host recipe")?;
    if !generic.execution.is_host_local() {
        return Ok(());
    }
    for node in &generic.plan.nodes {
        if !node.emits.is_empty() {
            return Err(format!("host-local node \"{}\" emits an event; authenticated events exist to satisfy waits on other hosts", node.id));
        }
    }
    Ok(())
}

fn reject_nested_plan_runs(doc: &PlanDocument) -> Result<(), String> {
    for node in &doc.nodes {
        let mut actions = Vec::new();
        if let Some(action) = &node.action {
            actions.push(action);
        }
        if let Some(compensate) = &node.compensate {
            actions.push(compensate);
        }
        if let Some(recover) = &node.recover {
            actions.push(&recover.action);
        }
        for action in actions {
            if action.tool == "run_host_plan" || action.tool == "run_runtime_recipe" {
                return Err(format!(
                    "recipe node \"{}\" cannot recursively run {}",
                    node.id, action.tool
                ));
            }
        }
        if let Some(validation) = &node.validate {
            if validation.tool == "run_host_plan" || validation.tool == "run_runtime_recipe" {
                return Err(format!(
                    "recipe node \"{}\" cannot use {} as validation",
                    node.id, validation.tool
                ));
            }
        }
    }
    Ok(())
}

/// A distributed recipe must say which host every action belongs to -- the
/// Platform is dispatching across several and an unbound node has no
/// answer. A host-local recipe has exactly one host by construction (the
/// agent executing it), so an action node MAY omit its target there, and
/// omitting it means this host; a target that IS present is still pinned
/// to an exact `vars.inputs` reference.
fn validate_host_targets(document: &PlanDocument, host_local: bool) -> Result<(), String> {
    for node in &document.nodes {
        if node.action.is_none() {
            continue;
        }
        let Some(target) = &node.target else {
            if host_local {
                continue;
            }
            return Err(format!(
                "host recipe action node \"{}\" requires an exact target binding",
                node.id
            ));
        };
        let reference = target.host_ref.trim();
        if !reference.starts_with("${vars.inputs.")
            || !reference.ends_with('}')
            || reference.contains('/')
        {
            return Err(format!(
                "host recipe node \"{}\" target hostRef must be an exact vars.inputs reference",
                node.id
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::schema::{Action, Node, TargetRef, WaitSpec, WaitTrigger};

    fn minimal_plan() -> PlanDocument {
        PlanDocument {
            contract_version: plan::CONTRACT_VERSION.to_string(),
            plan_id: "p1".to_string(),
            idempotency_key: "k1".to_string(),
            generation: 1,
            nodes: vec![Node {
                id: "n1".to_string(),
                action: Some(Action {
                    tool: "noop".to_string(),
                    args: Map::new(),
                }),
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    fn local_execution() -> HostExecution {
        HostExecution {
            coordinator: HOST_COORDINATOR_HOST_AGENT.to_string(),
            mode: HOST_MODE_LOCAL.to_string(),
        }
    }

    fn base_document() -> HostDocument {
        HostDocument {
            contract_version: HOST_CONTRACT_VERSION.to_string(),
            recipe_id: "r1".to_string(),
            recipe_version: "1.0.0".to_string(),
            execution: local_execution(),
            plan: minimal_plan(),
            ..Default::default()
        }
    }

    #[test]
    fn envelope_requires_the_host_contract_version() {
        let mut doc = base_document();
        doc.contract_version = "host-recipe.v2".to_string();
        assert!(validate_host_envelope(&doc).is_err());
    }

    #[test]
    fn envelope_accepts_distributed_or_host_local() {
        assert!(validate_host_envelope(&base_document()).is_ok());
        let mut distributed = base_document();
        distributed.execution = HostExecution {
            coordinator: HOST_COORDINATOR_PLATFORM.to_string(),
            mode: HOST_MODE_DISTRIBUTED.to_string(),
        };
        assert!(validate_host_envelope(&distributed).is_ok());
        let mut other = base_document();
        other.execution = HostExecution {
            coordinator: "something".to_string(),
            mode: "else".to_string(),
        };
        assert!(validate_host_envelope(&other).is_err());
    }

    #[test]
    fn host_local_plan_rejects_a_wait_node() {
        let mut doc = base_document();
        doc.plan.nodes[0].action = None;
        doc.plan.nodes[0].wait = Some(WaitSpec {
            trigger: WaitTrigger { kind: "operator".to_string(), kind_type: "approval".to_string() },
            schema_revision: "r1".to_string(),
            input_schema: serde_json::json!({"type": "object", "additionalProperties": false, "properties": {}}).as_object().unwrap().clone(),
            ..Default::default()
        });
        assert!(validate_host_envelope(&doc).is_err());
    }

    #[test]
    fn host_local_plan_rejects_more_than_one_host() {
        let mut doc = base_document();
        doc.plan.nodes[0].target = Some(TargetRef {
            host_ref: "${vars.inputs.a}".to_string(),
            resource_ref: String::new(),
        });
        doc.plan.nodes.push(Node {
            id: "n2".to_string(),
            action: Some(Action {
                tool: "noop".to_string(),
                args: Map::new(),
            }),
            target: Some(TargetRef {
                host_ref: "${vars.inputs.b}".to_string(),
                resource_ref: String::new(),
            }),
            ..Default::default()
        });
        assert!(validate_host_envelope(&doc).is_err());
    }

    #[test]
    fn host_local_action_target_may_be_omitted() {
        let doc = base_document();
        assert!(validate_host_targets(&doc.plan, true).is_ok());
    }

    #[test]
    fn distributed_action_target_is_required() {
        let doc = base_document();
        assert!(validate_host_targets(&doc.plan, false).is_err());
    }

    #[test]
    fn distributed_action_target_must_be_an_exact_inputs_reference() {
        let mut doc = base_document();
        doc.plan.nodes[0].target = Some(TargetRef {
            host_ref: "literal-host".to_string(),
            resource_ref: String::new(),
        });
        assert!(validate_host_targets(&doc.plan, false).is_err());
        doc.plan.nodes[0].target = Some(TargetRef {
            host_ref: "${vars.inputs.hostId}".to_string(),
            resource_ref: String::new(),
        });
        assert!(validate_host_targets(&doc.plan, false).is_ok());
    }

    #[test]
    fn nested_plan_run_is_rejected() {
        let mut doc = minimal_plan();
        doc.nodes[0].action = Some(Action {
            tool: "run_host_plan".to_string(),
            args: Map::new(),
        });
        assert!(reject_nested_plan_runs(&doc).is_err());
    }

    #[test]
    fn resolve_inputs_feeds_tenant_and_inputs_into_plan_variables() {
        let mut doc = base_document();
        doc.plan.idempotency_key = "run-${vars.inputs.name}-${vars.tenantId}".to_string();
        doc.inputs.insert(
            "name".to_string(),
            InputSpec {
                required: true,
                ..Default::default()
            },
        );
        let mut values = Map::new();
        values.insert("name".to_string(), Value::String("demo".to_string()));
        let loaded = resolve_host_inputs(doc, &values).unwrap();
        assert_eq!(loaded.expanded_plan.idempotency_key, "run-demo-local");
    }

    #[test]
    fn redacted_inputs_masks_only_secret_names() {
        let mut doc = base_document();
        doc.inputs.insert(
            "token".to_string(),
            InputSpec {
                secret: true,
                ..Default::default()
            },
        );
        doc.inputs.insert(
            "name".to_string(),
            InputSpec {
                ..Default::default()
            },
        );
        let mut values = Map::new();
        values.insert(
            "token".to_string(),
            Value::String("super-secret".to_string()),
        );
        values.insert("name".to_string(), Value::String("demo".to_string()));
        let loaded = resolve_host_inputs(doc, &values).unwrap();
        let redacted = loaded.redacted_inputs();
        assert_eq!(
            redacted.get("token"),
            Some(&Value::String("[redacted]".to_string()))
        );
        assert_eq!(
            redacted.get("name"),
            Some(&Value::String("demo".to_string()))
        );
        assert_eq!(loaded.secret_input_names(), vec!["token".to_string()]);
    }
}
