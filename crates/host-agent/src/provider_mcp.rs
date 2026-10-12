//! MCP-layer glue for provider lifecycle tools and the provider-shaped recipe
//! families (`internal/hostmcp/provider_install.go`, `provider_teardown.go`,
//! `recipe_run.go`, `tunnel_run.go`). Every path that executes work expands
//! to one host plan for the single durable executor
//! (`plan_mcp::handle_run_host_plan_with_metadata`); activation, candidate
//! cleanup and teardown completion run as that executor's completion hooks.

use crate::plan_mcp::{
    bool_field, capabilities_from_snapshot, handle_run_host_plan_with_metadata, string_field,
    validate_host_plan_with_snapshot,
};
use crate::provider::definition::{
    decode_descriptor, encode_manifest, validate_descriptor, InstallManifest, PluginDescriptor,
    RecipeRef,
};
use crate::recipe::runtime::{self as recipes, RuntimeLoaded, TunnelLoaded};
use crate::recipe::source::{validate_host_agent_version, SourceRequest};
use crate::resource::Reservation;
use crate::transport::Server;
use crate::{goerr::quote, plan::runner::RunCtx};
use serde_json::{json, Map, Value as J};
use sha2::{Digest, Sha256};
use std::sync::{atomic::AtomicBool, Arc};

const TUNNELING_CAPABILITY: &str = "opute.capability.tunneling.v1";
const TEARDOWN_OPERATION: &str = "opute.provider.teardown";

/// The admitted launcher reservation and the lease flag a durable run claims.
pub struct Lease {
    pub reservation: Reservation,
    pub claimed: Arc<AtomicBool>,
}

fn source_request(args: &Map<String, J>, require_hash: bool) -> SourceRequest {
    SourceRequest {
        source: string_field(args, "source"),
        revision: string_field(args, "revision"),
        sha256: string_field(args, "sha256"),
        require_sha256: require_hash,
    }
}

/// `recipeInputValues`.
fn input_values(args: &Map<String, J>) -> Result<Map<String, J>, String> {
    match args.get("inputs") {
        None | Some(J::Null) => Ok(Map::new()),
        Some(J::Object(values)) => Ok(values.clone()),
        Some(_) => Err("inputs must be an object".into()),
    }
}

fn first_non_empty(values: &[&str]) -> String {
    values
        .iter()
        .map(|value| value.trim())
        .find(|value| !value.is_empty())
        .unwrap_or_default()
        .to_string()
}

fn error(message: &str) -> J {
    crate::tools::error_result(message)
}

/// Go's nil map encodes as `null`; a decoded empty mapping is not
/// distinguished from an absent one by the typed document.
fn output_mapping(mapping: &std::collections::BTreeMap<String, String>) -> J {
    if mapping.is_empty() {
        J::Null
    } else {
        serde_json::to_value(mapping).unwrap_or(J::Null)
    }
}

fn activation_metadata(
    metadata: &mut Map<String, J>,
    activation: Option<&recipes::ActivationSpec>,
    inputs: &Map<String, J>,
) {
    let Some(activation) = activation else {
        return;
    };
    let mut bindings = Map::new();
    let mut names = Map::new();
    for (binding, input) in activation.input_bindings.clone().unwrap_or_default() {
        bindings.insert(
            binding.clone(),
            inputs.get(&input).cloned().unwrap_or(J::Null),
        );
        names.insert(binding, J::String(input));
    }
    metadata.insert(
        "activation".into(),
        json!({
            "capability": activation.capability,
            "servingContract": activation.serving_contract,
            "inputBindings": bindings,
        }),
    );
    metadata.insert("activationInputNames".into(), J::Object(names));
}

fn provider_metadata(metadata: &mut Map<String, J>, args: &Map<String, J>) {
    let provider = string_field(args, "providerId");
    if provider.is_empty() {
        return;
    }
    metadata.insert("providerId".into(), J::String(provider));
    metadata.insert(
        "providerVersion".into(),
        J::String(string_field(args, "providerVersion")),
    );
    metadata.insert(
        "providerGenerationId".into(),
        J::String(string_field(args, "providerGenerationId")),
    );
    metadata.insert(
        "providerManifest".into(),
        args.get("providerManifest").cloned().unwrap_or(J::Null),
    );
}

fn plan_args(plan: J, resume: bool) -> Map<String, J> {
    [
        ("plan".to_string(), plan),
        ("resume".to_string(), J::Bool(resume)),
    ]
    .into_iter()
    .collect()
}

/// `loadRuntimeRecipe`.
fn load_runtime(
    server: &Server,
    args: &Map<String, J>,
    require_hash: bool,
) -> Result<(RuntimeLoaded, crate::catalog::Snapshot), String> {
    let mut loaded = recipes::load_runtime(&source_request(args, require_hash))?;
    loaded.resolve_inputs(&input_values(args)?)?;
    validate_host_agent_version(
        &loaded.document.compatibility.min_host_agent_version,
        env!("CARGO_PKG_VERSION"),
    )?;
    let snapshot = server
        .providers
        .catalog_for_recipe(&string_field(args, "providerGenerationId"));
    loaded.validate(&capabilities_from_snapshot(&snapshot), &snapshot.revision)?;
    Ok((loaded, snapshot))
}

/// `loadTunnelRecipe`.
fn load_tunnel(
    server: &Server,
    args: &Map<String, J>,
    require_hash: bool,
) -> Result<(TunnelLoaded, crate::catalog::Snapshot), String> {
    let mut loaded = recipes::load_tunnel(&source_request(args, require_hash))?;
    loaded.resolve_inputs(&input_values(args)?)?;
    validate_host_agent_version(
        &loaded.document.compatibility.min_host_agent_version,
        env!("CARGO_PKG_VERSION"),
    )?;
    let snapshot = server
        .providers
        .catalog_for_recipe(&string_field(args, "providerGenerationId"));
    loaded.validate(&capabilities_from_snapshot(&snapshot), &snapshot.revision)?;
    Ok((loaded, snapshot))
}

/// `runtimeRecipeMetadata`.
fn runtime_metadata(loaded: &RuntimeLoaded, activate: bool) -> Map<String, J> {
    let document = &loaded.document;
    let mut metadata = Map::new();
    metadata.insert("contractVersion".into(), J::from(recipes::CONTRACT_VERSION));
    metadata.insert("recipeId".into(), J::from(document.recipe_id.clone()));
    metadata.insert(
        "recipeVersion".into(),
        J::from(document.recipe_version.clone()),
    );
    metadata.insert(
        "runtime".into(),
        serde_json::to_value(&document.runtime).unwrap_or(J::Null),
    );
    metadata.insert("runtimeId".into(), J::from(document.runtime.id.clone()));
    metadata.insert(
        "source".into(),
        serde_json::to_value(&loaded.source).unwrap_or(J::Null),
    );
    metadata.insert("inputs".into(), J::Object(loaded.redacted_inputs()));
    metadata.insert(
        "secretInputs".into(),
        serde_json::to_value(loaded.secret_input_names()).unwrap_or(J::Null),
    );
    metadata.insert(
        "recipeHash".into(),
        J::from(loaded.source.recipe_hash.clone()),
    );
    metadata.insert(
        "recipeDocument".into(),
        serde_json::to_value(document).unwrap_or(J::Null),
    );
    metadata.insert(
        "outputMapping".into(),
        output_mapping(&document.output_mapping),
    );
    metadata.insert(
        "expandedPlan".into(),
        serde_json::to_value(&loaded.expanded_plan).unwrap_or(J::Null),
    );
    metadata.insert("activate".into(), J::Bool(activate));
    activation_metadata(&mut metadata, document.activation.as_ref(), &loaded.inputs);
    metadata
}

/// `tunnelRecipeMetadata`.
fn tunnel_metadata(loaded: &TunnelLoaded, activate: bool) -> Map<String, J> {
    let document = &loaded.document;
    let mut metadata = Map::new();
    metadata.insert("recipeKind".into(), J::from("tunnel"));
    metadata.insert(
        "contractVersion".into(),
        J::from(recipes::TUNNEL_CONTRACT_VERSION),
    );
    metadata.insert("recipeId".into(), J::from(document.recipe_id.clone()));
    metadata.insert(
        "recipeVersion".into(),
        J::from(document.recipe_version.clone()),
    );
    metadata.insert(
        "provider".into(),
        serde_json::to_value(&document.provider).unwrap_or(J::Null),
    );
    metadata.insert("providerId".into(), J::from(document.provider.id.clone()));
    metadata.insert(
        "bindings".into(),
        serde_json::to_value(&document.bindings).unwrap_or(J::Null),
    );
    metadata.insert(
        "source".into(),
        serde_json::to_value(&loaded.source).unwrap_or(J::Null),
    );
    metadata.insert("inputs".into(), J::Object(loaded.redacted_inputs()));
    metadata.insert(
        "secretInputs".into(),
        serde_json::to_value(loaded.secret_input_names()).unwrap_or(J::Null),
    );
    metadata.insert(
        "recipeHash".into(),
        J::from(loaded.source.recipe_hash.clone()),
    );
    metadata.insert(
        "recipeDocument".into(),
        serde_json::to_value(document).unwrap_or(J::Null),
    );
    metadata.insert(
        "outputMapping".into(),
        output_mapping(&document.output_mapping),
    );
    metadata.insert(
        "expandedPlan".into(),
        serde_json::to_value(&loaded.expanded_plan).unwrap_or(J::Null),
    );
    metadata.insert("activate".into(), J::Bool(activate));
    activation_metadata(&mut metadata, document.activation.as_ref(), &loaded.inputs);
    metadata
}

/// `handleValidateRuntimeRecipe`.
pub fn handle_validate_runtime_recipe(server: &Server, args: &Map<String, J>) -> J {
    let (loaded, snapshot) = match load_runtime(server, args, false) {
        Ok(loaded) => loaded,
        Err(e) => return error(&e),
    };
    let plan = match validate_host_plan_with_snapshot(&loaded.expanded_plan, &snapshot) {
        Ok(plan) => plan,
        Err(e) => return error(&e),
    };
    crate::tools::structured_result(
        json!({
            "valid": true,
            "contractVersion": recipes::CONTRACT_VERSION,
            "recipeId": loaded.document.recipe_id,
            "recipeVersion": loaded.document.recipe_version,
            "runtime": loaded.document.runtime,
            "source": loaded.source,
            "inputs": loaded.redacted_inputs(),
            "recipeHash": loaded.source.recipe_hash,
            "rawSha256": loaded.source.raw_sha256,
            "plan": plan,
        }),
        Some("runtime recipe is valid"),
    )
}

/// `handleValidateTunnelRecipe`.
pub fn handle_validate_tunnel_recipe(server: &Server, args: &Map<String, J>) -> J {
    let (loaded, snapshot) = match load_tunnel(server, args, false) {
        Ok(loaded) => loaded,
        Err(e) => return error(&e),
    };
    let plan = match validate_host_plan_with_snapshot(&loaded.expanded_plan, &snapshot) {
        Ok(plan) => plan,
        Err(e) => return error(&e),
    };
    crate::tools::structured_result(
        json!({
            "valid": true,
            "contractVersion": recipes::TUNNEL_CONTRACT_VERSION,
            "recipeId": loaded.document.recipe_id,
            "recipeVersion": loaded.document.recipe_version,
            "provider": loaded.document.provider,
            "bindings": loaded.document.bindings,
            "source": loaded.source,
            "inputs": loaded.redacted_inputs(),
            "recipeHash": loaded.source.recipe_hash,
            "rawSha256": loaded.source.raw_sha256,
            "plan": plan,
        }),
        Some("tunnel recipe is valid"),
    )
}

/// `persistedPlanHasRedactedSecret`.
fn persisted_plan_has_redacted_secret(plan: &J, metadata: &Map<String, J>) -> bool {
    let inputs = plan
        .get("variables")
        .and_then(|variables| variables.get("inputs"))
        .and_then(J::as_object);
    metadata
        .get("secretInputs")
        .and_then(J::as_array)
        .into_iter()
        .flatten()
        .filter_map(J::as_str)
        .any(|name| inputs.and_then(|inputs| inputs.get(name)) == Some(&J::from("[redacted]")))
}

/// `resumeRecipeRun` and the runtime recipe's inline equivalent.
#[allow(clippy::too_many_arguments)]
fn resume_recipe_run(
    server: &Arc<Server>,
    args: &Map<String, J>,
    run_id: &str,
    kind: &str,
    not_found: &str,
    task_name: &str,
    description: &str,
    lease: Lease,
) -> J {
    let record = {
        let state = match server.host.state.lock() {
            Ok(state) => state,
            Err(_) => return error("durable plan state is unavailable"),
        };
        match state.get_plan(run_id) {
            Err(e) => return error(&format!("get {kind} run: {e}")),
            Ok(record) => record,
        }
    };
    let record = match record {
        Some(record) if !record.recipe_json.is_empty() => record,
        _ => return error(&format!("{not_found}: {run_id}")),
    };
    let plan: J = match serde_json::from_str(&record.plan_json) {
        Ok(plan) => plan,
        Err(e) => return error(&format!("decode persisted {kind} recipe plan: {e}")),
    };
    let mut metadata: Map<String, J> = match serde_json::from_str(&record.recipe_json) {
        Ok(metadata) => metadata,
        Err(e) => return error(&format!("decode persisted {kind} recipe metadata: {e}")),
    };
    if bool_field(args, "activate") {
        metadata.insert("activate".into(), J::Bool(true));
    }
    if persisted_plan_has_redacted_secret(&plan, &metadata) {
        return error(&format!(
            "{kind} recipe resume requires secret inputs to be supplied through references; refusing to execute redacted values"
        ));
    }
    handle_run_host_plan_with_metadata(
        server,
        &plan_args(plan, true),
        Some(&metadata),
        task_name,
        description,
        lease.reservation,
        lease.claimed,
    )
}

/// `handleRunRuntimeRecipe`.
pub fn handle_run_runtime_recipe(server: &Arc<Server>, args: &Map<String, J>, lease: Lease) -> J {
    let resume_run = string_field(args, "runId");
    if string_field(args, "source").is_empty()
        || (!resume_run.is_empty() && bool_field(args, "resume"))
    {
        if resume_run.is_empty() {
            return error("source is required unless resuming with runId");
        }
        return resume_recipe_run(
            server,
            args,
            &resume_run,
            "runtime recipe",
            "runtime recipe run not found",
            "run_runtime_recipe",
            "Executing runtime recipe...",
            lease,
        );
    }
    let (loaded, _) = match load_runtime(server, args, true) {
        Ok(loaded) => loaded,
        Err(e) => return error(&e),
    };
    let activate = bool_field(args, "activate");
    if activate && loaded.document.activation.is_none() {
        return error(
            "recipe does not declare an activation contract; activation cannot be requested",
        );
    }
    let mut metadata = runtime_metadata(&loaded, activate);
    provider_metadata(&mut metadata, args);
    handle_run_host_plan_with_metadata(
        server,
        &plan_args(
            serde_json::to_value(&loaded.expanded_plan).unwrap_or(J::Null),
            bool_field(args, "resume"),
        ),
        Some(&metadata),
        "run_runtime_recipe",
        "Executing runtime recipe...",
        lease.reservation,
        lease.claimed,
    )
}

/// `handleRunTunnelRecipe`.
pub fn handle_run_tunnel_recipe(server: &Arc<Server>, args: &Map<String, J>, lease: Lease) -> J {
    if string_field(args, "source").is_empty() {
        let resume_run = string_field(args, "runId");
        if resume_run.is_empty() || !bool_field(args, "resume") {
            return error("source is required unless resuming with runId");
        }
        return resume_recipe_run(
            server,
            args,
            &resume_run,
            "tunnel",
            "tunnel recipe run not found",
            "run_tunnel_recipe",
            "Executing tunnel recipe...",
            lease,
        );
    }
    let (loaded, _) = match load_tunnel(server, args, true) {
        Ok(loaded) => loaded,
        Err(e) => return error(&e),
    };
    let activate = bool_field(args, "activate");
    if activate && loaded.document.activation.is_none() {
        return error(
            "tunnel recipe does not declare an activation contract; activation cannot be requested",
        );
    }
    let mut metadata = tunnel_metadata(&loaded, activate);
    provider_metadata(&mut metadata, args);
    handle_run_host_plan_with_metadata(
        server,
        &plan_args(
            serde_json::to_value(&loaded.expanded_plan).unwrap_or(J::Null),
            bool_field(args, "resume"),
        ),
        Some(&metadata),
        "run_tunnel_recipe",
        "Executing tunnel recipe...",
        lease.reservation,
        lease.claimed,
    )
}

/// `loadProviderDescriptor`: inline descriptors or trusted local files only.
fn load_descriptor(args: &Map<String, J>) -> Result<(PluginDescriptor, String), String> {
    let (raw, base) = match args.get("descriptor") {
        Some(value) if !value.is_null() => (
            crate::gojson::encode_to_string(value).into_bytes(),
            String::new(),
        ),
        _ => {
            let source = first_non_empty(&[
                &string_field(args, "source"),
                &string_field(args, "descriptorSource"),
            ]);
            if source.is_empty() {
                return Err("provider descriptor source is required".into());
            }
            if source.starts_with("https://") || source.starts_with("github:") {
                return Err("provider descriptors must be trusted local composition data".into());
            }
            let path = crate::hostread::clean(&source);
            let data = std::fs::read(&path).map_err(|e| {
                format!(
                    "read provider descriptor: {}",
                    crate::goerr::path_error("open", std::path::Path::new(&path), &e)
                )
            })?;
            // `filepath.Dir`.
            let base = crate::hostread::clean(path.rsplit_once('/').map_or("", |(dir, _)| {
                if dir.is_empty() {
                    "/"
                } else {
                    dir
                }
            }));
            (data, base)
        }
    };
    let descriptor = decode_descriptor_document(&raw)?;
    validate_descriptor(&descriptor)?;
    Ok((descriptor, base))
}

/// `recipe.Decode(raw, &descriptor, "provider descriptor")`: a leading `{`
/// is JSON, otherwise YAML.
fn decode_descriptor_document(raw: &[u8]) -> Result<PluginDescriptor, String> {
    let text = String::from_utf8_lossy(raw);
    let json = if text.trim_start().starts_with('{') {
        raw.to_vec()
    } else {
        let value: J = serde_yaml::from_slice(raw)
            .map_err(|e| format!("decode provider descriptor YAML: {e}"))?;
        crate::gojson::encode_to_string(&value).into_bytes()
    };
    crate::gojson::validate::syntax(&json)
        .map_err(|e| format!("decode provider descriptor JSON: {e}"))?;
    let node = crate::gojson::parse(&json)
        .map_err(|_| "decode provider descriptor JSON: invalid JSON".to_string())?;
    decode_descriptor(&node).map_err(|e| format!("decode provider descriptor JSON: {e}"))
}

/// `hashProviderValue`.
pub(crate) fn hash_manifest(manifest: &InstallManifest) -> Result<String, String> {
    let encoded = encode_manifest(manifest)?;
    Ok(format!(
        "sha256:{}",
        hex::encode(Sha256::digest(encoded.as_bytes()))
    ))
}

/// `selectProviderRecipe`.
fn select_recipe(manifest: &InstallManifest, mode: &str) -> Result<RecipeRef, String> {
    manifest
        .recipes
        .iter()
        .find(|recipe| mode.is_empty() || recipe.mode == mode)
        .cloned()
        .ok_or_else(|| format!("provider manifest has no recipe for mode {}", quote(mode)))
}

/// `handleProviderInstallContext`.
pub fn handle_install(server: &Arc<Server>, args: &Map<String, J>, lease: Lease) -> J {
    let providers = &server.providers;
    let (mut descriptor, base) = match load_descriptor(args) {
        Ok(loaded) => loaded,
        Err(e) => return error(&e),
    };
    let mut endpoint = descriptor.server.endpoint.clone();
    let requested = string_field(args, "endpoint");
    if !requested.is_empty() {
        descriptor.server.endpoint = requested.clone();
        endpoint = requested;
    }
    let ctx = RunCtx::background();
    let token = string_field(args, "token");
    let adapter = match providers.connect(&ctx, descriptor, token) {
        Ok(adapter) => adapter,
        Err(e) => return error(&e),
    };
    let manifest = match providers.install_manifest(&ctx, &adapter) {
        Ok(manifest) => manifest,
        Err(e) => {
            providers.close_adapter(&adapter);
            return error(&e);
        }
    };
    let hash = match hash_manifest(&manifest) {
        Ok(hash) => hash,
        Err(e) => {
            providers.close_adapter(&adapter);
            return error(&e);
        }
    };
    // An install that changes nothing does not mint a generation: the same
    // provider, manifest and endpoint on an active, connected generation.
    if !bool_field(args, "force") && !bool_field(args, "resume") {
        if let Some(active) = providers.reusable_generation(&manifest.provider.id, &hash, &endpoint)
        {
            providers.close_adapter(&adapter);
            return crate::tools::structured_result(
                json!({
                    "providerId": manifest.provider.id,
                    "providerVersion": manifest.provider.version,
                    "providerGenerationId": active.id,
                    "generation": active,
                    "activated": true,
                    "reused": true,
                    "status": "already-installed",
                }),
                None,
            );
        }
    }
    let candidate = match providers.create_candidate(&manifest, &hash, &endpoint) {
        Ok(candidate) => candidate,
        Err(e) => {
            providers.close_adapter(&adapter);
            return error(&e);
        }
    };
    if let Err(e) = providers.persist(&candidate) {
        let _ = providers.activation.lifecycle.fail(&candidate.id);
        providers.close_adapter(&adapter);
        return error(&format!("persist provider generation: {e}"));
    }
    providers.announce("provider.candidate", &manifest.provider.id, &candidate.id);
    providers.register_candidate(&candidate.id, manifest.clone(), adapter.clone());
    let fail = |message: String| {
        let _ = providers.activation.lifecycle.fail(&candidate.id);
        providers.close_adapter(&adapter);
        providers.cleanup_candidate(&candidate.id);
        error(&message)
    };
    let recipe = match select_recipe(&manifest, &string_field(args, "mode")) {
        Ok(recipe) => recipe,
        Err(e) => return fail(e),
    };
    let mut source = string_field(args, "recipeSource");
    if source.is_empty() {
        source = recipe.source.uri.clone();
        if !base.is_empty()
            && !source.starts_with('/')
            && !source.starts_with("github:")
            && !source.starts_with("https://")
        {
            source = crate::hostread::clean(&format!("{base}/{source}"));
        }
    }
    let inputs = match input_values(args) {
        Ok(inputs) => inputs,
        Err(e) => return fail(e),
    };
    let manifest_value: J = encode_manifest(&manifest)
        .ok()
        .and_then(|encoded| serde_json::from_str(&encoded).ok())
        .unwrap_or(J::Null);
    let run_args: Map<String, J> = json!({
        "source": source,
        "revision": first_non_empty(&[&string_field(args, "revision"), &recipe.source.revision]),
        "sha256": first_non_empty(&[&string_field(args, "sha256"), &recipe.source.sha256]),
        "inputs": inputs,
        "activate": bool_field(args, "activate"),
        "resume": bool_field(args, "resume"),
        "providerId": manifest.provider.id,
        "providerVersion": manifest.provider.version,
        "providerGenerationId": candidate.id,
        "providerManifest": manifest_value,
    })
    .as_object()
    .unwrap()
    .clone();
    if manifest.validation.capability == TUNNELING_CAPABILITY {
        return handle_run_tunnel_recipe(server, &run_args, lease);
    }
    handle_run_runtime_recipe(server, &run_args, lease)
}

/// `handleProviderReloadContext`: the one caller that must never take the
/// install short-circuit.
pub fn handle_reload(server: &Arc<Server>, args: &Map<String, J>, lease: Lease) -> J {
    if string_field(args, "source").is_empty() && string_field(args, "descriptor").is_empty() {
        return error("provider reload requires descriptor source");
    }
    let mut reload = args.clone();
    reload.insert("force".into(), J::Bool(true));
    handle_install(server, &reload, lease)
}

/// `resolveProviderHostService`: the host service a provider generation runs
/// as, from what the host observes. A unit whose executable or unit file sits
/// under a path segment equal to the provider id belongs to that provider.
/// Observation goes through the service context, as a call by the providers
/// service on its declared `host-observation` dependency; listing registers
/// each observed unit, as Go's `ListHostServices` does.
fn resolve_host_service(server: &Server, provider: &str) -> Map<String, J> {
    let snapshot = server.catalog.snapshot();
    resolve_host_service_with(provider, |name: &str, args: J| {
        let args = args.as_object().cloned().unwrap_or_default();
        let call = crate::builtin::Invocation {
            ctx: &RunCtx::background(),
            args: &args,
            binding: &Default::default(),
            snapshot: &snapshot,
            claimed: None,
        };
        match crate::builtin::invoke(server, name, &call) {
            Some(Ok(result)) => result.get("structuredContent").cloned(),
            _ => None,
        }
    })
}

/// The resolution rule over `observe`, which answers a host observation
/// operation's structured result, or `None` when it fails.
fn resolve_host_service_with(
    provider: &str,
    observe: impl Fn(&str, J) -> Option<J>,
) -> Map<String, J> {
    let provider = provider.trim();
    if provider.is_empty() {
        return Map::new();
    }
    for scope in ["user", "system"] {
        let Some(listed) = observe("list_host_services", json!({"scope": scope})) else {
            continue;
        };
        let services = listed
            .get("services")
            .and_then(J::as_array)
            .cloned()
            .unwrap_or_default();
        for service in services {
            let name = service.get("serviceName").and_then(J::as_str).unwrap_or("");
            if !service_name_is_plausible(provider, name) {
                continue;
            }
            let Some(observed) = observe(
                "inspect_host_service",
                json!({"serviceName": name, "scope": scope}),
            ) else {
                continue;
            };
            let exec_start = observed.get("execStart").and_then(J::as_str).unwrap_or("");
            let fragment_path = observed
                .get("fragmentPath")
                .and_then(J::as_str)
                .unwrap_or("");
            if !path_has_segment(exec_start, provider) && !path_has_segment(fragment_path, provider)
            {
                continue;
            }
            let mut resolved = Map::new();
            resolved.insert("serviceName".into(), J::from(name));
            resolved.insert("scope".into(), J::from(scope));
            if !fragment_path.is_empty() {
                resolved.insert("serviceFile".into(), J::from(fragment_path));
            }
            return resolved;
        }
    }
    Map::new()
}

/// `providerServiceNameIsPlausible`: narrows which units are worth inspecting.
fn service_name_is_plausible(provider: &str, service: &str) -> bool {
    !service.is_empty()
        && provider
            .split('.')
            .any(|segment| segment.len() > 2 && service.contains(segment))
}

/// `pathHasSegment`: `segment` as a whole path element.
fn path_has_segment(path: &str, segment: &str) -> bool {
    !path.is_empty() && !segment.is_empty() && path.split('/').any(|element| element == segment)
}

/// `providerTeardownInputs`: adds the host-resolved, tenant-scoped service
/// URI to the neutral provider callback.
fn teardown_inputs(server: &Server, provider: &str, args: &Map<String, J>) -> Map<String, J> {
    let mut inputs = args
        .get("inputs")
        .and_then(J::as_object)
        .cloned()
        .unwrap_or_default();
    if string_field(&inputs, "serviceName").is_empty()
        && string_field(&inputs, "hostService") != "none"
    {
        for (key, value) in resolve_host_service(server, provider) {
            inputs.entry(key).or_insert(value);
        }
    }
    let service = string_field(&inputs, "serviceName");
    if service.is_empty() {
        return inputs;
    }
    let mut scope = string_field(&inputs, "scope");
    if scope.is_empty() {
        scope = "user".into();
    }
    inputs.insert(
        "serviceUri".into(),
        J::from(format!(
            "host-service:{}:{scope}/{service}",
            server.host.tenant_id
        )),
    );
    inputs
}

/// `requireProviderHostServiceTarget`.
fn require_host_service_target(provider: &str, inputs: &Map<String, J>) -> Result<(), String> {
    if !string_field(inputs, "serviceName").is_empty()
        || string_field(inputs, "hostService") == "none"
    {
        return Ok(());
    }
    Err(format!("provider_host_service_unknown: cannot determine which host service runs provider {}, so tearing it down would retire the generation and leave its process running; pass inputs.serviceName (list_host_services names the units) or inputs.hostService=\"none\" if this provider has no unit to reclaim", quote(provider)))
}

/// `handleProviderTeardownContext`.
pub fn handle_teardown(server: &Arc<Server>, args: &Map<String, J>, lease: Lease) -> J {
    let providers = &server.providers;
    let provider = string_field(args, "provider");
    if provider.is_empty() {
        return error("provider is required");
    }
    if !bool_field(args, "confirm") {
        return error("provider teardown requires confirm=true");
    }
    let Some(active) = providers.activation.lifecycle.active(&provider) else {
        return error(&format!(
            "provider {} has no active generation",
            quote(&provider)
        ));
    };
    let generation = string_field(args, "generation");
    if !generation.is_empty() && generation != active.id {
        return error(&format!(
            "provider generation {} is not active; active generation is {}",
            quote(&generation),
            quote(&active.id)
        ));
    }
    let forced = bool_field(args, "force");
    let session = match providers.activation.lifecycle.open_session(&provider) {
        Ok(session) => session,
        Err(e) => return error(&e),
    };
    let inputs = teardown_inputs(server, &provider, args);
    if let Err(e) = require_host_service_target(&provider, &inputs) {
        return error(&e);
    }
    let prepared = providers.prepare_teardown(&provider, session.generation_id(), args, &inputs);
    // The plan runner may finalize the provider before this handler returns;
    // release the preparation session before handing control to it.
    drop(session);
    let resume = bool_field(args, "resume");
    let mut metadata: Map<String, J> = json!({
        "providerTeardown": true,
        "providerId": active.provider.id,
        "providerVersion": active.provider.version,
        "providerGenerationId": active.id,
        "teardownContractVersion": "provider-teardown.v1",
        "providerTeardownInputs": inputs,
    })
    .as_object()
    .unwrap()
    .clone();
    let (plan, description) = match prepared {
        Ok(plan) => (plan, "Tearing down provider..."),
        Err(blocked) => {
            if !forced {
                return error(&blocked);
            }
            metadata.insert("providerTeardownForced".into(), J::Bool(true));
            metadata.insert("providerTeardownForcedReason".into(), J::from(blocked));
            (
                forced_reclaim_plan(&active.id, &inputs),
                "Reclaiming orphaned provider generation...",
            )
        }
    };
    let plan = match crate::plan::schema::decode_value(&plan) {
        Ok(plan) => serde_json::to_value(plan).unwrap_or(J::Null),
        Err(e) => return error(&format!("build forced provider reclaim plan: {e}")),
    };
    handle_run_host_plan_with_metadata(
        server,
        &plan_args(plan, resume),
        Some(&metadata),
        TEARDOWN_OPERATION,
        description,
        lease.reservation,
        lease.claimed,
    )
}

/// `forcedProviderReclaimPlan`: one read node recording the service identity
/// being reclaimed; host cleanup runs from the completion hook.
fn forced_reclaim_plan(generation: &str, inputs: &Map<String, J>) -> J {
    let mut scope = string_field(inputs, "scope");
    if scope.is_empty() {
        scope = "user".into();
    }
    let service_uri = string_field(inputs, "serviceUri");
    let node = if service_uri.is_empty() {
        json!({"id": "list-host-services",
               "action": {"tool": "list_host_services", "args": {"scope": scope}},
               "continueOnFailure": true})
    } else {
        json!({"id": "inspect-orphaned-provider-service",
               "action": {"tool": "inspect_host_service", "args": {"uri": service_uri, "scope": scope}},
               "continueOnFailure": true})
    };
    json!({
        "contractVersion": "host-plan.v1",
        "planId": "com.opute.host.provider-reclaim",
        "generation": 1,
        "idempotencyKey": format!("com.opute.host.provider-reclaim-{generation}"),
        "nodes": [node],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Go's `TestResolveProviderHostServiceClaimsOnlyTheUnitInstalledUnderTheProviderID`:
    /// both units look like k3s; only one lives under the provider id.
    #[test]
    fn resolve_host_service_claims_only_the_unit_installed_under_the_provider_id() {
        let exec_start = |unit: &str| {
            match unit {
            "opute-provider-k3s" => "/home/u/.local/share/opute/instances/host-a/opute-provider-k3s",
            "opute-provider-k3s-p15-mtzntn1h" => {
                "/home/u/.local/share/opute/providers/com.opute.k3s/bin/opute-provider-k3s"
            }
            "opute-provider-cloudflare" => {
                "/home/u/.local/share/opute/providers/com.opute.cloudflare/bin/opute-provider-cloudflare"
            }
            _ => "",
        }
        };
        let inspected = std::cell::RefCell::new(Vec::new());
        let observe = |name: &str, args: J| match name {
            "list_host_services" => Some(json!({"services": [
                {"serviceName": "opute-provider-k3s"},
                {"serviceName": "opute-provider-k3s-p15-mtzntn1h"},
                {"serviceName": "opute-provider-cloudflare"},
                {"serviceName": "cron"},
            ]})),
            "inspect_host_service" => {
                let unit = args["serviceName"].as_str().unwrap().to_string();
                inspected.borrow_mut().push(unit.clone());
                let fragment = if unit == "opute-provider-k3s-p15-mtzntn1h" {
                    "/home/u/.config/systemd/user/opute-provider-k3s-p15-mtzntn1h.service"
                } else {
                    ""
                };
                Some(json!({"execStart": exec_start(&unit), "fragmentPath": fragment}))
            }
            _ => None,
        };
        let resolved = resolve_host_service_with("com.opute.k3s", observe);
        assert_eq!(resolved["serviceName"], "opute-provider-k3s-p15-mtzntn1h");
        assert_eq!(resolved["scope"], "user");
        assert!(resolved["serviceFile"]
            .as_str()
            .unwrap()
            .ends_with("opute-provider-k3s-p15-mtzntn1h.service"));
        assert!(!inspected.borrow().contains(&"cron".to_string()));
        // A provider not installed here has no unit to claim.
        assert!(resolve_host_service_with("com.opute.absent", observe).is_empty());
        // A failed listing in every scope resolves nothing.
        assert!(resolve_host_service_with("com.opute.k3s", |_, _| None).is_empty());
    }

    #[test]
    fn path_segments_match_whole_elements_only() {
        assert!(path_has_segment("/a/com.opute.k3s/bin", "com.opute.k3s"));
        assert!(!path_has_segment(
            "/a/com.opute.k3s-experimental/bin",
            "com.opute.k3s"
        ));
        assert!(!path_has_segment("", "com.opute.k3s"));
        assert!(service_name_is_plausible(
            "com.opute.k3s",
            "opute-provider-k3s"
        ));
        assert!(!service_name_is_plausible("com.opute.k3s", ""));
        assert!(!service_name_is_plausible("a.b", "ab-service"));
    }
}
