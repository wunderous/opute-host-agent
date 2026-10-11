//! Port of `internal/recipe/recipe.go` (`runtime-recipe.v1`) and `tunnel.go`
//! (`tunnel-recipe.v1`). Both are envelopes around one host plan; the plan
//! remains the only execution language and the single executor runs it.

use super::host_recipe::reject_nested_plan_runs;
use super::source::{
    canonical_hash, decode_value, load_source, reserved_plan_variables, resolve_inputs,
    resolve_plan_identity, sorted_strings, CompatibilitySpec, InputSpec, SourceMetadata,
    SourceRequest,
};
use crate::goerr::quote;
use crate::plan::interpolate::{interpolate_args, EvalContext};
use crate::plan::schema::{self as plan, Capability, Document as PlanDocument};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

pub const CONTRACT_VERSION: &str = "runtime-recipe.v1";
pub const TUNNEL_CONTRACT_VERSION: &str = "tunnel-recipe.v1";

const SUPPORTED_SERVING_CONTRACTS: &[&str] = &[
    "openai-chat.v1",
    "http-exposure.v1",
    "kubernetes.v1",
    // Deprecated fan-out alias (ADR-0016).
    "network-overlay.v1",
    "mesh-runtime.v1",
    "mesh-membership.v1",
    "private-mesh.v1",
    "public-ingress.v1",
    "tunneling.v1",
    "llm-serving.v1",
];

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct RuntimeSpec {
    #[serde(default)]
    pub id: String,
    #[serde(rename = "servingContract", default)]
    pub serving_contract: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
}

/// Names recipe inputs, never an implementation-specific command, so the
/// host owns one validation flow per serving contract.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ActivationSpec {
    #[serde(default)]
    pub capability: String,
    #[serde(rename = "servingContract", default)]
    pub serving_contract: String,
    #[serde(rename = "inputBindings", default)]
    pub input_bindings: Option<BTreeMap<String, String>>,
}

impl ActivationSpec {
    fn bindings(&self) -> BTreeMap<String, String> {
        self.input_bindings.clone().unwrap_or_default()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct RuntimeDocument {
    #[serde(rename = "contractVersion", default)]
    pub contract_version: String,
    #[serde(rename = "recipeId", default)]
    pub recipe_id: String,
    #[serde(rename = "recipeVersion", default)]
    pub recipe_version: String,
    #[serde(default)]
    pub runtime: RuntimeSpec,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub inputs: BTreeMap<String, InputSpec>,
    #[serde(default)]
    pub compatibility: CompatibilitySpec,
    #[serde(default)]
    pub plan: PlanDocument,
    #[serde(
        rename = "outputMapping",
        default,
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub output_mapping: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activation: Option<ActivationSpec>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct TunnelProvider {
    #[serde(default)]
    pub id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub capabilities: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct TunnelBinding {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub hostname: String,
    #[serde(rename = "localTarget", default)]
    pub local_target: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub path: String,
}

/// Go embeds the shared `Envelope`, so its fields precede the tunnel fields
/// in the canonical JSON encoding.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct TunnelDocument {
    #[serde(rename = "contractVersion", default)]
    pub contract_version: String,
    #[serde(rename = "recipeId", default)]
    pub recipe_id: String,
    #[serde(rename = "recipeVersion", default)]
    pub recipe_version: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub inputs: BTreeMap<String, InputSpec>,
    #[serde(default)]
    pub compatibility: CompatibilitySpec,
    #[serde(default)]
    pub plan: PlanDocument,
    #[serde(
        rename = "outputMapping",
        default,
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub output_mapping: BTreeMap<String, String>,
    #[serde(default)]
    pub provider: TunnelProvider,
    #[serde(default)]
    pub bindings: Option<Vec<TunnelBinding>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activation: Option<ActivationSpec>,
}

/// A resolved recipe of either family: everything the MCP handlers need.
#[derive(Debug, Clone, Default)]
pub struct Loaded<D> {
    pub document: D,
    pub expanded_plan: PlanDocument,
    pub inputs: Map<String, Value>,
    pub source: SourceMetadata,
}

pub type RuntimeLoaded = Loaded<RuntimeDocument>;
pub type TunnelLoaded = Loaded<TunnelDocument>;

fn redacted(
    inputs: &Map<String, Value>,
    specs: &BTreeMap<String, InputSpec>,
) -> Map<String, Value> {
    inputs
        .iter()
        .map(|(name, value)| {
            let secret = specs.get(name).is_some_and(|spec| spec.secret);
            let value = if secret {
                Value::String("[redacted]".into())
            } else {
                value.clone()
            };
            (name.clone(), value)
        })
        .collect()
}

fn secret_names(specs: &BTreeMap<String, InputSpec>) -> Vec<String> {
    sorted_strings(
        specs
            .iter()
            .filter(|(_, spec)| spec.secret)
            .map(|(name, _)| name.clone())
            .collect(),
    )
}

impl RuntimeLoaded {
    pub fn redacted_inputs(&self) -> Map<String, Value> {
        redacted(&self.inputs, &self.document.inputs)
    }
    pub fn secret_input_names(&self) -> Vec<String> {
        secret_names(&self.document.inputs)
    }
    pub fn resolve_inputs(&mut self, values: &Map<String, Value>) -> Result<(), String> {
        let resolved = resolve_inputs(&self.document.inputs, values)?;
        let mut variables = reserved_plan_variables(&self.document.plan.variables);
        variables.insert("inputs".into(), Value::Object(resolved.clone()));
        let mut expanded = self.document.plan.clone();
        expanded.variables = variables.clone();
        resolve_plan_identity(&mut expanded, &variables)?;
        self.expanded_plan = expanded;
        self.inputs = resolved;
        Ok(())
    }
    pub fn validate(
        &self,
        capabilities: &BTreeMap<String, Capability>,
        catalog_revision: &str,
    ) -> Result<(), String> {
        validate_runtime_envelope(&self.document)?;
        validate_resolved(
            &self.expanded_plan,
            &self.document.compatibility,
            capabilities,
            catalog_revision,
        )
    }
}

impl TunnelLoaded {
    pub fn redacted_inputs(&self) -> Map<String, Value> {
        redacted(&self.inputs, &self.document.inputs)
    }
    pub fn secret_input_names(&self) -> Vec<String> {
        secret_names(&self.document.inputs)
    }
    pub fn bindings(&self) -> Vec<TunnelBinding> {
        self.document.bindings.clone().unwrap_or_default()
    }
    pub fn resolve_inputs(&mut self, values: &Map<String, Value>) -> Result<(), String> {
        let resolved = resolve_inputs(&self.document.inputs, values)?;
        let mut variables = reserved_plan_variables(&self.document.plan.variables);
        variables.insert("inputs".into(), Value::Object(resolved.clone()));
        let mut expanded = self.document.plan.clone();
        expanded.variables = variables.clone();
        resolve_plan_identity(&mut expanded, &variables)?;
        let raw: Vec<Value> = self
            .bindings()
            .iter()
            .map(|binding| {
                serde_json::json!({
                    "id": binding.id,
                    "hostname": binding.hostname,
                    "localTarget": binding.local_target,
                    "path": binding.path,
                })
            })
            .collect();
        let mut args = Map::new();
        args.insert("bindings".into(), Value::Array(raw));
        let context = EvalContext {
            variables,
            ..Default::default()
        };
        let expanded_bindings = interpolate_args(&args, &context)
            .map_err(|e| format!("resolve tunnel bindings: {e}"))?;
        let bindings: Vec<TunnelBinding> = serde_json::from_value(
            expanded_bindings
                .get("bindings")
                .cloned()
                .unwrap_or(Value::Null),
        )
        .map_err(|e| format!("decode resolved tunnel bindings: {e}"))?;
        for binding in &bindings {
            if binding.hostname.contains("${") || binding.local_target.contains("${") {
                return Err(format!(
                    "tunnel binding {} contains unresolved variables",
                    quote(&binding.id)
                ));
            }
        }
        self.document.bindings = Some(bindings);
        self.expanded_plan = expanded;
        self.inputs = resolved;
        Ok(())
    }
    pub fn validate(
        &self,
        capabilities: &BTreeMap<String, Capability>,
        catalog_revision: &str,
    ) -> Result<(), String> {
        validate_tunnel_envelope(&self.document)?;
        validate_resolved(
            &self.expanded_plan,
            &self.document.compatibility,
            capabilities,
            catalog_revision,
        )
    }
}

fn validate_resolved(
    expanded: &PlanDocument,
    compatibility: &CompatibilitySpec,
    capabilities: &BTreeMap<String, Capability>,
    catalog_revision: &str,
) -> Result<(), String> {
    if expanded.contract_version.is_empty() {
        return Err("recipe plan is not resolved; resolve inputs first".into());
    }
    for name in &compatibility.required_tools {
        if !capabilities.contains_key(name) {
            return Err(format!(
                "recipe requires unsupported host-agent capability {}",
                quote(name)
            ));
        }
    }
    reject_nested_plan_runs(expanded)?;
    plan::validate(expanded, capabilities, catalog_revision)
        .map_err(|e| format!("validate recipe host plan: {e}"))
}

pub fn load_runtime(request: &SourceRequest) -> Result<RuntimeLoaded, String> {
    let (raw, mut source) = load_source(request)?;
    let document: RuntimeDocument = decode_value(&raw, "runtime recipe")?;
    validate_runtime_envelope(&document)?;
    source.recipe_hash =
        canonical_hash(&document).map_err(|e| format!("hash runtime recipe: {e}"))?;
    Ok(Loaded {
        document,
        source,
        ..Default::default()
    })
}

pub fn load_tunnel(request: &SourceRequest) -> Result<TunnelLoaded, String> {
    let (raw, mut source) = load_source(request)?;
    let document: TunnelDocument = decode_value(&raw, "tunnel recipe")?;
    validate_tunnel_envelope(&document)?;
    source.recipe_hash =
        canonical_hash(&document).map_err(|e| format!("hash tunnel recipe: {e}"))?;
    Ok(Loaded {
        document,
        source,
        ..Default::default()
    })
}

fn validate_activation_bindings(
    activation: &ActivationSpec,
    inputs: &BTreeMap<String, InputSpec>,
) -> Result<(), String> {
    let bindings = activation.bindings();
    if bindings.is_empty() {
        return Err("activation.inputBindings is required".into());
    }
    for (binding, input) in &bindings {
        if binding.trim().is_empty() || input.trim().is_empty() {
            return Err("activation.inputBindings must contain non-empty names".into());
        }
        if !inputs.contains_key(input) {
            return Err(format!(
                "activation.inputBindings[{}] references unknown input {}",
                quote(binding),
                quote(input)
            ));
        }
    }
    Ok(())
}

pub fn validate_runtime_envelope(document: &RuntimeDocument) -> Result<(), String> {
    if document.contract_version != CONTRACT_VERSION {
        return Err(format!(
            "unsupported runtime recipe contractVersion {}",
            quote(&document.contract_version)
        ));
    }
    if document.recipe_id.trim().is_empty() || document.recipe_version.trim().is_empty() {
        return Err("recipeId and recipeVersion are required".into());
    }
    if document.runtime.id.trim().is_empty() {
        return Err("runtime.id is required".into());
    }
    if document.runtime.serving_contract.trim().is_empty() {
        return Err("runtime.servingContract is required".into());
    }
    if !SUPPORTED_SERVING_CONTRACTS.contains(&document.runtime.serving_contract.as_str()) {
        return Err(format!(
            "unsupported runtime serving contract {}",
            quote(&document.runtime.serving_contract)
        ));
    }
    if let Some(activation) = &document.activation {
        if activation.capability.trim().is_empty() {
            return Err("activation.capability is required".into());
        }
        if activation.serving_contract != document.runtime.serving_contract {
            return Err("activation.servingContract must match runtime.servingContract".into());
        }
        validate_activation_bindings(activation, &document.inputs)?;
    }
    if document.plan.contract_version != plan::CONTRACT_VERSION {
        return Err(format!("recipe plan must use {}", plan::CONTRACT_VERSION));
    }
    if document.plan.nodes.is_empty() {
        return Err("recipe plan must contain at least one node".into());
    }
    Ok(())
}

pub fn validate_tunnel_envelope(document: &TunnelDocument) -> Result<(), String> {
    if document.contract_version != TUNNEL_CONTRACT_VERSION {
        return Err(format!(
            "unsupported recipe contractVersion {}",
            quote(&document.contract_version)
        ));
    }
    if document.recipe_id.is_empty() || document.recipe_version.is_empty() {
        return Err("recipeId and recipeVersion are required".into());
    }
    if document.plan.contract_version != plan::CONTRACT_VERSION {
        return Err(format!("recipe plan must use {}", plan::CONTRACT_VERSION));
    }
    if document.plan.nodes.is_empty() {
        return Err("recipe plan must contain at least one node".into());
    }
    if document.provider.id.trim().is_empty() {
        return Err("provider.id is required".into());
    }
    let bindings = document.bindings.clone().unwrap_or_default();
    if bindings.is_empty() {
        return Err("tunnel recipe must contain at least one binding".into());
    }
    let mut seen = std::collections::BTreeSet::new();
    for binding in &bindings {
        if binding.id.trim().is_empty()
            || binding.hostname.trim().is_empty()
            || binding.local_target.trim().is_empty()
        {
            return Err("tunnel bindings require id, hostname, and localTarget".into());
        }
        if !seen.insert(binding.id.clone()) {
            return Err(format!("duplicate tunnel binding {}", quote(&binding.id)));
        }
    }
    if let Some(activation) = &document.activation {
        if activation.capability.trim().is_empty() || activation.serving_contract.trim().is_empty()
        {
            return Err("activation capability and servingContract are required".into());
        }
        validate_activation_bindings(activation, &document.inputs)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn runtime_document() -> Value {
        json!({
            "contractVersion": CONTRACT_VERSION,
            "recipeId": "r", "recipeVersion": "1",
            "runtime": {"id": "rt", "servingContract": "kubernetes.v1"},
            "inputs": {"providerId": {"required": true}, "token": {"secret": true}},
            "plan": {"contractVersion": "host-plan.v1", "planId": "p", "generation": 1,
                     "idempotencyKey": "k-${vars.inputs.providerId}",
                     "nodes": [{"id": "n", "action": {"tool": "get_host_info", "args": {}}}]},
            "activation": {"capability": "kubernetes", "servingContract": "kubernetes.v1",
                           "inputBindings": {"providerId": "providerId"}}
        })
    }

    #[test]
    fn runtime_envelope_requires_activation_contract_to_match_runtime() {
        let mut value = runtime_document();
        value["activation"]["servingContract"] = json!("openai-chat.v1");
        let document: RuntimeDocument = serde_json::from_value(value).unwrap();
        assert_eq!(
            validate_runtime_envelope(&document).unwrap_err(),
            "activation.servingContract must match runtime.servingContract"
        );
    }

    #[test]
    fn runtime_inputs_resolve_plan_identity_and_redact_secrets() {
        let document: RuntimeDocument = serde_json::from_value(runtime_document()).unwrap();
        let mut loaded = RuntimeLoaded {
            document,
            ..Default::default()
        };
        let values = json!({"providerId": "k3s", "token": "s3cret"});
        loaded.resolve_inputs(values.as_object().unwrap()).unwrap();
        assert_eq!(loaded.expanded_plan.idempotency_key, "k-k3s");
        assert_eq!(loaded.redacted_inputs()["token"], "[redacted]");
        assert_eq!(loaded.secret_input_names(), vec!["token".to_string()]);
    }

    #[test]
    fn tunnel_bindings_interpolate_and_reject_unresolved_variables() {
        let document: TunnelDocument = serde_json::from_value(json!({
            "contractVersion": TUNNEL_CONTRACT_VERSION,
            "recipeId": "t", "recipeVersion": "1",
            "inputs": {"host": {"required": true}},
            "plan": {"contractVersion": "host-plan.v1", "planId": "p", "generation": 1,
                     "idempotencyKey": "k", "nodes": [{"id": "n", "action": {"tool": "get_host_info", "args": {}}}]},
            "provider": {"id": "cloudflare"},
            "bindings": [{"id": "b", "hostname": "${vars.inputs.host}", "localTarget": "http://127.0.0.1:1"}]
        }))
        .unwrap();
        validate_tunnel_envelope(&document).unwrap();
        let mut loaded = TunnelLoaded {
            document,
            ..Default::default()
        };
        loaded
            .resolve_inputs(json!({"host": "a.example"}).as_object().unwrap())
            .unwrap();
        assert_eq!(loaded.bindings()[0].hostname, "a.example");
    }
}
