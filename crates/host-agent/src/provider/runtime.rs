//! Server composition for generation owners and outbound MCP I/O. The HTTP
//! runtime drives connections; ordinary admission and the single plan runner
//! retain execution ownership. No provider workflow executor is introduced.
use super::{
    activation::{Activation, Hooks},
    binding::{InvocationError, Operation},
    candidates::Candidates,
    delegation::Delegations,
    mounts::Mounts,
    publication::Publication,
    Lifecycle,
};
use crate::{
    catalog::{registry::Capability, Descriptor, Snapshot},
    plan::runner::RunCtx,
    store::StateStore,
    transport::Server,
};
use serde_json::{json, Map, Value};
use std::sync::{Arc, Mutex, OnceLock, Weak};

pub struct Runtime {
    pub activation: Arc<Activation>,
    pub delegations: Delegations,
    this: Weak<Self>,
    server: OnceLock<Weak<Server>>,
    io: OnceLock<tokio::runtime::Handle>,
    restored: Mutex<Vec<crate::store::ProviderGenerationRecord>>,
}
impl Runtime {
    pub fn new(
        publication: Arc<Publication>,
        state: Arc<Mutex<StateStore>>,
    ) -> Result<Arc<Self>, getrandom::Error> {
        let delegations = Delegations::new()?;
        Ok(Arc::new_cyclic(|owner: &Weak<Self>| {
            let lifecycle = Lifecycle::new(Default::default());
            let candidates = Arc::new(Candidates::default());
            let mounts = Arc::new(Mounts::new(lifecycle.clone()));
            let hooks = Hooks {
                capability: Arc::new({
                    let owner = owner.clone();
                    move |operation| Self::capability(owner.clone(), operation.clone())
                }),
                refresh: Arc::new(|_| {}),
                // The default server has no admission listeners. Trusted event
                // listeners are a separate composition boundary, not tool args.
                event: Arc::new(|_| Ok(())),
            };
            Self {
                activation: Activation::new(
                    lifecycle,
                    candidates,
                    mounts,
                    publication,
                    Some(state),
                    hooks,
                ),
                delegations,
                this: owner.clone(),
                server: OnceLock::new(),
                io: OnceLock::new(),
                restored: Mutex::new(Vec::new()),
            }
        }))
    }
    /// Bind once to the existing serving runtime before any outbound work.
    pub fn bind(&self, server: &Arc<Server>) {
        self.server.get_or_init(|| Arc::downgrade(server));
        self.io.get_or_init(tokio::runtime::Handle::current);
    }
    fn capability(owner: Weak<Self>, operation: Operation) -> Arc<Capability> {
        Arc::new(Capability {
            descriptor: operation.descriptor().clone(),
            invoke: Arc::new(move |ctx, arguments| {
                let owner = owner.upgrade().ok_or("provider runtime is unavailable")?;
                let server = owner
                    .server
                    .get()
                    .and_then(Weak::upgrade)
                    .ok_or("provider runtime is unavailable")?;
                let io = owner.io.get().ok_or("provider runtime is unavailable")?;
                let runtime = super::binding::Runtime {
                    lifecycle: &owner.activation.lifecycle,
                    candidates: &owner.activation.candidates,
                    mounts: &owner.activation.mounts,
                    delegations: &owner.delegations,
                    tasks: &server.tasks,
                    agent: &server.agent_id,
                };
                match io.block_on(operation.invoke(ctx, Some(arguments.clone()), &runtime)) {
                    Ok(result) => Ok(result),
                    Err(InvocationError::Refused(error)) => Ok(crate::tools::error_result(&error)
                        .as_object()
                        .unwrap()
                        .clone()),
                    Err(InvocationError::Adapter(error)) => Err(error),
                }
            }),
        })
    }
    /// `restoreProviderGenerations`, first half: durable generations re-enter
    /// the lifecycle while the server is constructed.
    pub fn restore(&self) -> Result<(), String> {
        let records = self.activation.restore_durable()?;
        *self.restored.lock().unwrap() = records;
        Ok(())
    }
    /// `restoreProviderGenerations`, second half: reconnect each durable
    /// active generation before the listener accepts, in durable order. A
    /// generation that cannot reconnect stays active and is logged.
    pub async fn reconnect_restored(&self) {
        let records = std::mem::take(&mut *self.restored.lock().unwrap());
        let ctx = RunCtx::background();
        for record in records {
            if record.status != "active" || record.descriptor_json.is_empty() {
                continue;
            }
            if let Err((stage, error)) = self.activation.reconnect(&ctx, &record).await {
                crate::app::log_warn(
                    &mut std::io::stderr(),
                    "provider generation restore skipped",
                    &[
                        ("provider_id", &record.provider_id),
                        ("generation_id", &record.generation_id),
                        ("stage", stage),
                        ("error", &error),
                    ],
                );
            }
        }
    }
    /// Dispose while the serving runtime still drives accepted provider I/O.
    /// Cleanup owns its continuation if its caller is cancelled.
    pub async fn close(self: &Arc<Self>) {
        let owner = self.clone();
        tokio::spawn(async move {
            owner.activation.mounts.close().await;
            let candidates = owner.activation.candidates.take_adapters();
            owner.activation.clear_metadata();
            for adapter in candidates {
                adapter.close().await;
            }
        })
        .await
        .expect("provider runtime cleanup owner panicked");
    }
    fn block<T>(&self, work: impl std::future::Future<Output = T>) -> Result<T, String> {
        let io = self.io.get().ok_or("provider runtime is unavailable")?;
        Ok(io.block_on(work))
    }
    /// `catalogSnapshotForRecipe`: a candidate's setup recipe validates and
    /// executes against its private snapshot; everything else sees the
    /// committed public catalog.
    pub fn catalog_for_recipe(&self, generation: &str) -> Snapshot {
        let active = self.activation.publication.snapshot();
        self.activation
            .candidates
            .snapshot(&active, generation)
            .unwrap_or(active)
    }
    /// `providerCandidateCapability`.
    pub fn candidate_capability(
        &self,
        generation: &str,
        name: &str,
    ) -> Option<(Descriptor, Arc<Capability>)> {
        let manifest = self.activation.candidates.manifest(generation)?;
        let operation = super::binding::operations(&manifest, generation, true)
            .into_iter()
            .find(|operation| operation.descriptor().name == name)?;
        let descriptor = operation.descriptor().clone();
        Some((descriptor, Self::capability(self.this.clone(), operation)))
    }
    /// `dispatchCandidateTool`: shares binding, admission, result validation
    /// and invocation evidence with ordinary dispatch, while the candidate's
    /// capabilities stay out of the public registry until activation.
    pub fn dispatch_candidate(
        &self,
        server: &Server,
        ctx: &RunCtx,
        name: &str,
        arguments: &Map<String, Value>,
        generation: &str,
        snapshot: &Snapshot,
    ) -> Option<Value> {
        let (descriptor, capability) = self.candidate_capability(generation, name)?;
        let binding = match crate::admission::resolve_binding_in(server, snapshot, name, arguments)
        {
            Ok(binding) => binding,
            Err(refusal) => return Some(refusal.render()),
        };
        let parent = ctx.reservation();
        let reservation = match crate::admission::admit_in(
            server,
            snapshot,
            name,
            arguments,
            &binding,
            None,
            parent.as_deref(),
        ) {
            Ok(reservation) => reservation,
            Err(refusal) => return Some(refusal.render()),
        };
        let invocation = ctx.with_reservation(reservation.clone());
        let result = crate::tools::dispatch_provider(
            server,
            snapshot,
            &descriptor,
            &capability,
            &invocation,
            arguments,
            &binding,
            &reservation,
        );
        crate::admission::release(server, &reservation);
        Some(result)
    }
    /// `activateProviderGeneration` from a plan-runner thread.
    pub fn activate_generation(&self, metadata: &Map<String, Value>) -> Result<(), String> {
        self.block(self.activation.activate(metadata.clone()))?
    }
    /// `activateCompletedProviderCandidate`: reconnects after a restart rebind
    /// the process-local adapter of a durable run that already completed.
    pub fn activate_completed_candidate(
        &self,
        metadata: &Map<String, Value>,
    ) -> Result<(), String> {
        let generation = string(metadata, "providerGenerationId");
        if generation.is_empty() {
            return Ok(());
        }
        self.activate_generation(metadata)
            .map_err(|error| format!("provider generation activation after reconnect: {error}"))?;
        self.complete_candidate(generation);
        Ok(())
    }
    /// `cleanupProviderCandidate`.
    pub fn cleanup_candidate(&self, generation: &str) {
        let _ = self.block(self.activation.cleanup(generation));
    }
    /// `completeProviderCandidate`.
    pub fn complete_candidate(&self, generation: &str) {
        self.activation.candidates.remove(generation);
    }
    /// `provideradapter.Connect` with the default client identity.
    pub fn connect(
        &self,
        ctx: &RunCtx,
        descriptor: super::definition::PluginDescriptor,
        token: String,
    ) -> Result<Arc<super::adapter::Adapter>, String> {
        let adapter = self.block(super::adapter::Adapter::connect(
            ctx, descriptor, token, "", "",
        ))??;
        Ok(Arc::new(adapter))
    }
    pub fn install_manifest(
        &self,
        ctx: &RunCtx,
        adapter: &super::adapter::Adapter,
    ) -> Result<super::definition::InstallManifest, String> {
        self.block(adapter.install_manifest(ctx))?
    }
    pub fn close_adapter(&self, adapter: &super::adapter::Adapter) {
        let _ = self.block(adapter.close());
    }
    /// The install short-circuit: an active generation of the same provider,
    /// manifest and endpoint whose adapter is still connected.
    pub fn reusable_generation(
        &self,
        provider: &str,
        manifest_hash: &str,
        endpoint: &str,
    ) -> Option<super::Generation> {
        let active = self.activation.lifecycle.active(provider)?;
        (active.manifest_hash == manifest_hash
            && active.endpoint == endpoint
            && self
                .activation
                .mounts
                .generation_adapter(provider, &active.id)
                .is_some())
        .then_some(active)
    }
    pub fn create_candidate(
        &self,
        manifest: &super::definition::InstallManifest,
        manifest_hash: &str,
        endpoint: &str,
    ) -> Result<super::Generation, String> {
        let revision = self.activation.publication.snapshot().revision;
        self.activation.lifecycle.create_candidate(
            manifest.provider.clone(),
            manifest_hash,
            endpoint,
            &revision,
        )
    }
    pub fn persist(&self, generation: &super::Generation) -> Result<(), String> {
        self.activation.persist(generation)
    }
    pub fn announce(&self, name: &str, provider: &str, generation: &str) {
        let _ = self.activation.event(name, provider, generation, "");
    }
    pub fn register_candidate(
        &self,
        generation: &str,
        manifest: super::definition::InstallManifest,
        adapter: Arc<super::adapter::Adapter>,
    ) {
        self.activation
            .candidates
            .set_adapter(generation, Some(adapter));
        self.activation
            .candidates
            .set_manifest(generation, manifest);
    }
    /// `resolveProviderHostService` requires host service observation, which
    /// this build does not carry; it never claims a unit it cannot observe.
    pub fn resolve_host_service(&self, _provider: &str) -> Map<String, Value> {
        Map::new()
    }
    /// `prepareProviderTeardownPlan`: the provider's own prepare phase. An
    /// error is the reason the provider could not declare a plan.
    pub fn prepare_teardown(
        &self,
        provider: &str,
        generation: &str,
        arguments: &Map<String, Value>,
        inputs: &Map<String, Value>,
    ) -> Result<Value, String> {
        let adapter = self
            .activation
            .mounts
            .generation_adapter(provider, generation)
            .ok_or_else(|| {
                format!(
                    "provider {} is not connected",
                    crate::goerr::quote(provider)
                )
            })?;
        let mut prepare = arguments.clone();
        prepare.insert("phase".into(), Value::from("prepare"));
        prepare.insert("inputs".into(), Value::Object(inputs.clone()));
        let result = self.block(adapter.call_synchronous_only(
            &RunCtx::background(),
            "opute.provider.teardown",
            Some(prepare),
            "",
        ))??;
        if result.get("isError") == Some(&Value::Bool(true)) {
            return Err(format!(
                "provider {} teardown operation failed",
                crate::goerr::quote(provider)
            ));
        }
        let object = result
            .get("structuredContent")
            .and_then(Value::as_object)
            .ok_or_else(|| {
                format!(
                    "provider {} teardown returned no structured plan",
                    crate::goerr::quote(provider)
                )
            })?;
        let version = object
            .get("contractVersion")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if version != "host-plan.v1" {
            return Err(format!(
                "provider teardown returned unsupported contract {}",
                crate::goerr::quote(version)
            ));
        }
        let plan = object.get("plan").cloned().unwrap_or(Value::Null);
        crate::plan::schema::decode_value(&plan)
            .map(|document| serde_json::to_value(document).unwrap_or(Value::Null))
            .map_err(|e| format!("decode provider teardown plan: {e}"))
    }
    /// `completeProviderTeardownContext`: finalize with the provider, reclaim
    /// the host-owned service, then drain, dispose and retire the generation.
    pub fn complete_teardown(
        &self,
        server: &Server,
        metadata: &Map<String, Value>,
    ) -> Result<(), String> {
        let provider = string(metadata, "providerId");
        let generation = string(metadata, "providerGenerationId");
        if provider.is_empty() || generation.is_empty() {
            return Err("provider teardown metadata is incomplete".into());
        }
        let lifecycle = &self.activation.lifecycle;
        let session = lifecycle
            .open_session(provider)
            .map_err(|e| format!("open provider teardown session: {e}"))?;
        if session.generation_id() != generation {
            return Err(format!(
                "provider teardown generation {} is no longer active",
                crate::goerr::quote(generation)
            ));
        }
        let inputs = metadata
            .get("providerTeardownInputs")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        if inputs.contains_key("redacted") {
            return Err("provider teardown inputs are redacted; resume requires the provider inputs to be supplied again".into());
        }
        if metadata
            .get("providerTeardownForced")
            .and_then(Value::as_bool)
            != Some(true)
        {
            let adapter = self
                .activation
                .mounts
                .generation_adapter(provider, session.generation_id())
                .ok_or_else(|| {
                    format!(
                        "provider {} is not connected for teardown finalization",
                        crate::goerr::quote(provider)
                    )
                })?;
            let mut finalize = inputs.clone();
            finalize.insert("phase".into(), Value::from("finalize"));
            let arguments = serde_json::json!({"phase": "finalize", "inputs": finalize});
            let result = self
                .block(adapter.call_synchronous_only(
                    &RunCtx::background(),
                    "opute.provider.teardown",
                    arguments.as_object().cloned(),
                    "",
                ))?
                .map_err(|e| format!("finalize provider teardown: {e}"))?;
            if result.get("isError") == Some(&Value::Bool(true)) {
                return Err(format!(
                    "provider {} teardown finalization failed",
                    crate::goerr::quote(provider)
                ));
            }
        }
        cleanup_host_service(&inputs)?;
        drop(session);
        self.announce_reason("provider.draining", provider, generation, "teardown");
        lifecycle
            .drain(&RunCtx::background(), generation)
            .map_err(|e| format!("drain provider generation: {e}"))?;
        if let Some(drained) = lifecycle.get(generation) {
            self.activation
                .persist(&drained)
                .map_err(|e| format!("persist stopped provider generation: {e}"))?;
        }
        // Disposing the generation's mounts closes the adapter they own.
        self.block(
            self.activation
                .mounts
                .unmount_generation(provider, generation),
        )?;
        self.activation.forget_validation(provider);
        self.activation
            .publication
            .retire(provider, generation, |_| {});
        self.announce_reason("provider.stopped", provider, generation, "teardown");
        server
            .host
            .state
            .lock()
            .map_err(|_| {
                "clear active provider capabilities: state store is unavailable".to_string()
            })?
            .remove_active_capabilities_for_provider(provider)
            .map_err(|e| format!("clear active provider capabilities: {e}"))
    }
    fn announce_reason(&self, name: &str, provider: &str, generation: &str, reason: &str) {
        let _ = self.activation.event(name, provider, generation, reason);
    }
    /// The adapter for a generation: mounted when active, otherwise still a
    /// candidate, as activation validation runs before promotion.
    fn generation_or_candidate_adapter(
        &self,
        provider: &str,
        generation: &str,
    ) -> Option<Arc<super::adapter::Adapter>> {
        self.activation
            .mounts
            .generation_adapter(provider, generation)
            .or_else(|| self.activation.candidates.adapter(generation))
    }
    fn open_validation_session(
        &self,
        provider: &str,
        generation: &str,
    ) -> Result<super::Session, String> {
        if generation.is_empty() {
            self.activation.lifecycle.open_session(provider)
        } else {
            self.activation
                .lifecycle
                .open_session_for_generation(provider, generation)
        }
    }
    /// `validateRecipeActivation`: the host-owned serving-contract flow that
    /// must pass before a completed run may replace the active capability.
    pub fn validate_activation(
        &self,
        server: &Server,
        run_id: &str,
        metadata: &Map<String, Value>,
    ) -> (
        Option<crate::store::ActiveCapabilityRecord>,
        Option<Value>,
        Result<(), String>,
    ) {
        if metadata.get("activate").and_then(Value::as_bool) != Some(true) {
            return (None, None, Ok(()));
        }
        let Some(activation) = metadata.get("activation").and_then(Value::as_object) else {
            return (
                None,
                None,
                Err("recipe activation metadata is incomplete".into()),
            );
        };
        let capability = string(activation, "capability");
        let contract = string(activation, "servingContract");
        let bindings = activation.get("inputBindings").and_then(Value::as_object);
        let (Some(bindings), false, false) = (bindings, capability.is_empty(), contract.is_empty())
        else {
            return (
                None,
                None,
                Err(
                    "recipe activation requires capability, servingContract, and inputBindings"
                        .into(),
                ),
            );
        };
        let generation = string(metadata, "providerGenerationId");
        let observation = match self.activation_flow(server, contract, bindings, generation) {
            Some(Ok(observation)) => observation,
            Some(Err(error)) => {
                return (
                    None,
                    None,
                    Err(format!("activation validation failed: {error}")),
                )
            }
            None => {
                return (
                    None,
                    None,
                    Err(format!(
                        "no host-agent activation validation flow exists for serving contract {}",
                        crate::goerr::quote(contract)
                    )),
                )
            }
        };
        let observed = Value::Object(observation.clone());
        if observation.get("ready").and_then(Value::as_bool) != Some(true) {
            return (
                None,
                Some(observed),
                Err(format!(
                    "activation validation failed: capability is not ready for {contract}"
                )),
            );
        }
        if contract == "openai-chat.v1"
            && observation
                .get("streamingChatReady")
                .and_then(Value::as_bool)
                != Some(true)
        {
            return (
                None,
                Some(observed),
                Err(format!(
                    "activation validation failed: runtime is not ready for {contract}"
                )),
            );
        }
        let bindings_json = redacted_activation_bindings(metadata, bindings);
        let runtime = first_non_empty(
            string(metadata, "runtimeId"),
            string(metadata, "providerId"),
        );
        let provider = string(metadata, "providerId");
        let active = crate::store::ActiveCapabilityRecord {
            capability: capability.into(),
            serving_contract: contract.into(),
            provider: first_non_empty(provider, runtime).into(),
            recipe_id: string(metadata, "recipeId").into(),
            recipe_version: string(metadata, "recipeVersion").into(),
            recipe_hash: string(metadata, "recipeHash").into(),
            run_id: run_id.into(),
            input_bindings_json: crate::gojson::encode_to_string(&Value::Object(bindings_json)),
            observation_json: crate::gojson::encode_to_string(&observed),
            activated_at: String::new(),
        };
        (Some(active), Some(observed), Ok(()))
    }
    /// `activationValidationFlows`: recipes select a serving contract, never
    /// an implementation-specific validator or executable.
    fn activation_flow(
        &self,
        server: &Server,
        contract: &str,
        bindings: &Map<String, Value>,
        generation: &str,
    ) -> Option<Result<Map<String, Value>, String>> {
        Some(match contract {
            "kubernetes.v1" => self.kubernetes_activation(bindings, generation),
            "network-overlay.v1" | "mesh-runtime.v1" | "mesh-membership.v1" | "private-mesh.v1"
            | "public-ingress.v1" | "tunneling.v1" | "llm-serving.v1" => {
                self.networking_activation(contract, bindings, generation)
            }
            "openai-chat.v1" => {
                let endpoint = string(bindings, "endpoint");
                if endpoint.is_empty() {
                    return Some(Err(
                        "openai-chat.v1 activation requires input binding endpoint".into(),
                    ));
                }
                let arguments = serde_json::json!({
                    "endpoint": endpoint,
                    "modelRef": string(bindings, "modelRef"),
                    "includeChat": true,
                    "bearerToken": string(bindings, "bearerToken"),
                });
                let probe = crate::tools::dispatch(
                    server,
                    "probe_openai_compatible_server",
                    arguments.as_object().unwrap(),
                    None,
                    None,
                );
                if probe.get("isError") == Some(&Value::Bool(true)) {
                    Err("runtime probe returned an error".into())
                } else {
                    structured_object(&probe)
                        .ok_or_else(|| "runtime probe returned no observation".into())
                }
            }
            "http-exposure.v1" => {
                let mut endpoints = Vec::new();
                let endpoint = string(bindings, "endpoint");
                if !endpoint.is_empty() {
                    endpoints.push(endpoint.to_owned());
                }
                if let Some(values) = bindings.get("endpoints").and_then(Value::as_array) {
                    endpoints.extend(
                        values
                            .iter()
                            .filter_map(Value::as_str)
                            .map(str::trim)
                            .filter(|endpoint| !endpoint.is_empty())
                            .map(str::to_owned),
                    );
                }
                if endpoints.is_empty() {
                    return Some(Err(
                        "http-exposure.v1 activation requires endpoint or endpoints input binding"
                            .into(),
                    ));
                }
                let mut checks = Vec::new();
                for endpoint in endpoints {
                    let arguments = serde_json::json!({"endpoint": endpoint});
                    let probe = crate::tools::dispatch(
                        server,
                        "probe_http_endpoint",
                        arguments.as_object().unwrap(),
                        None,
                        None,
                    );
                    if probe.get("isError") == Some(&Value::Bool(true)) {
                        return Some(Err(format!(
                            "HTTP endpoint probe returned an error for {endpoint}"
                        )));
                    }
                    let Some(observation) = structured_object(&probe) else {
                        return Some(Err(format!(
                            "HTTP endpoint probe returned no observation for {endpoint}"
                        )));
                    };
                    let ready = observation.get("ready").and_then(Value::as_bool) == Some(true);
                    checks.push(Value::Object(observation));
                    if !ready {
                        return Some(Ok(serde_json::json!({"servingContract": "http-exposure.v1", "ready": false, "checks": checks}).as_object().unwrap().clone()));
                    }
                }
                Ok(serde_json::json!({"servingContract": "http-exposure.v1", "ready": true, "checks": checks}).as_object().unwrap().clone())
            }
            // The authenticated MCP exposure probe belongs to tunnel recipes,
            // whose host-service operations are not part of this build. It
            // fails closed rather than reporting an unproven exposure.
            "mcp-exposure.v1" => {
                let endpoint = string(bindings, "endpoint");
                if endpoint.is_empty() {
                    return Some(Err(
                        "mcp-exposure.v1 activation requires input binding endpoint".into(),
                    ));
                }
                Err(
                    "mcp-exposure.v1 activation probe is not implemented by this Host Agent build"
                        .into(),
                )
            }
            _ => return None,
        })
    }
    fn kubernetes_activation(
        &self,
        bindings: &Map<String, Value>,
        generation: &str,
    ) -> Result<Map<String, Value>, String> {
        let provider = string(bindings, "providerId");
        if provider.is_empty() {
            return Err("kubernetes.v1 activation requires input binding providerId".into());
        }
        let session = self.open_validation_session(provider, generation)?;
        let adapter = self
            .generation_or_candidate_adapter(provider, session.generation_id())
            .ok_or_else(|| {
                format!(
                    "Kubernetes provider {} is not connected",
                    crate::goerr::quote(provider)
                )
            })?;
        let result = self.block(adapter.call_synchronous_only(
            &RunCtx::background(),
            "opute.capability.kubernetes.validate",
            Some(Map::new()),
            "",
        ))??;
        drop(session);
        if result.get("isError") == Some(&Value::Bool(true)) {
            return Err(format!(
                "Kubernetes provider {} validation failed",
                crate::goerr::quote(provider)
            ));
        }
        let observation = result
            .get("structuredContent")
            .and_then(Value::as_object)
            .cloned()
            .ok_or_else(|| {
                format!(
                    "Kubernetes provider {} returned invalid validation evidence",
                    crate::goerr::quote(provider)
                )
            })?;
        if observation.get("ready").and_then(Value::as_bool) != Some(true) {
            return Err(format!(
                "Kubernetes provider {} is not ready",
                crate::goerr::quote(provider)
            ));
        }
        Ok(observation)
    }
    fn networking_activation(
        &self,
        contract: &str,
        bindings: &Map<String, Value>,
        generation: &str,
    ) -> Result<Map<String, Value>, String> {
        let provider = string(bindings, "providerId");
        if provider.is_empty() {
            return Err(format!(
                "{contract} activation requires input binding providerId"
            ));
        }
        let session = self.open_validation_session(provider, generation)?;
        let adapter = self
            .generation_or_candidate_adapter(provider, session.generation_id())
            .ok_or_else(|| {
                format!(
                    "{contract} provider {} is not connected",
                    crate::goerr::quote(provider)
                )
            })?;
        let result = self.block(adapter.call_synchronous_only(
            &RunCtx::background(),
            "opute.provider.get_install_manifest",
            Some(Map::new()),
            "",
        ))??;
        if result.get("isError") == Some(&Value::Bool(true)) {
            return Err(format!(
                "{contract} provider {} install-manifest probe failed",
                crate::goerr::quote(provider)
            ));
        }
        let generation = session.generation_id().to_owned();
        drop(session);
        Ok(serde_json::json!({
            "servingContract": contract,
            "ready": true,
            "providerId": provider,
            "generationId": generation,
        })
        .as_object()
        .unwrap()
        .clone())
    }
    pub fn status(&self, arguments: &Map<String, Value>) -> Value {
        let provider = string(arguments, "provider");
        let expected = string(arguments, "expectedEndpoint");
        let active = self.activation.lifecycle.active(provider);
        let connected = active.as_ref().is_some_and(|generation| {
            self.activation
                .mounts
                .generation_adapter(provider, &generation.id)
                .is_some()
        });
        let mut result =
            json!({"providerId":provider,"connected":connected,"active":active.is_some()});
        if !expected.is_empty() {
            result["endpointMatches"] = json!(active
                .as_ref()
                .is_some_and(|generation| generation.endpoint == expected));
        }
        if let Some(generation) = active {
            result["generation"] = json!(generation);
        }
        crate::tools::structured_result(result, None)
    }
    pub fn validate(&self, ctx: &RunCtx, arguments: &Map<String, Value>) -> Value {
        match self.validate_owned(ctx, arguments) {
            Ok(result) => Value::Object(result),
            Err(error) => crate::tools::error_result(&error),
        }
    }
    fn validate_owned(
        &self,
        ctx: &RunCtx,
        arguments: &Map<String, Value>,
    ) -> Result<Map<String, Value>, String> {
        let provider = string(arguments, "provider");
        let session = self.activation.lifecycle.open_session(provider)?;
        let adapter = self
            .activation
            .mounts
            .generation_adapter(provider, session.generation_id())
            .ok_or_else(|| {
                format!(
                    "provider {} is not connected",
                    crate::goerr::quote(provider)
                )
            })?;
        let remembered = self.activation.validation();
        let explicit = string(arguments, "operation");
        let operation = if explicit.is_empty() {
            remembered
                .get(provider)
                .map(String::as_str)
                .unwrap_or_default()
        } else {
            explicit
        };
        if operation.is_empty() {
            return Err(format!(
                "provider {} does not declare a validation operation",
                crate::goerr::quote(provider)
            ));
        }
        let io = self.io.get().ok_or("provider runtime is unavailable")?;
        io.block_on(adapter.call_synchronous_only(ctx, operation, Some(arguments.clone()), ""))
    }
}
/// `cleanupProviderHostService`. The host-owned unit operations (inspect,
/// disable, remove, reload, stop) are host-service domain operations this
/// build does not carry, so a named unit fails closed instead of being left
/// running behind a completed teardown.
fn cleanup_host_service(inputs: &Map<String, Value>) -> Result<(), String> {
    let service = string(inputs, "serviceName");
    if service.is_empty() {
        if string(inputs, "hostService") == "none" {
            return Ok(());
        }
        return Err("provider_host_service_unknown: provider teardown reached host cleanup without a service to reclaim".into());
    }
    Err(format!(
        "inspect provider service {}: host service operations are not implemented by this Host Agent build",
        crate::goerr::quote(service)
    ))
}
fn first_non_empty<'a>(first: &'a str, second: &'a str) -> &'a str {
    if first.trim().is_empty() {
        second.trim()
    } else {
        first.trim()
    }
}
/// `structuredObject`: the structured content of a tool result as an object.
fn structured_object(result: &Value) -> Option<Map<String, Value>> {
    result.get("structuredContent")?.as_object().cloned()
}
/// `redactedActivationBindings`: secret recipe inputs never reach the durable
/// active-capability selection.
fn redacted_activation_bindings(
    metadata: &Map<String, Value>,
    bindings: &Map<String, Value>,
) -> Map<String, Value> {
    let secrets: std::collections::BTreeSet<&str> = metadata
        .get("secretInputs")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let names = metadata
        .get("activationInputNames")
        .and_then(Value::as_object);
    bindings
        .iter()
        .map(|(binding, value)| {
            let input = names
                .and_then(|names| names.get(binding))
                .map(go_sprint)
                .unwrap_or_else(|| "<nil>".into());
            let value = if secrets.contains(input.as_str()) {
                Value::String("[redacted]".into())
            } else {
                value.clone()
            };
            (binding.clone(), value)
        })
        .collect()
}
/// `fmt.Sprint` for the JSON values a decoded metadata map can hold.
fn go_sprint(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => "<nil>".into(),
        other => other.to_string(),
    }
}
fn string<'a>(arguments: &'a Map<String, Value>, key: &str) -> &'a str {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    #[tokio::test(flavor = "current_thread")]
    async fn shutdown_keeps_io_alive_until_accepted_factory_call_finishes() {
        use std::{
            io::{Read, Write},
            time::Duration,
        };
        let directory = tempfile::tempdir().unwrap();
        let environment = crate::config::Env::from_pairs([
            ("OPUTE_REMOTE_AGENT_ID", "parity.runtime.owner"),
            ("OPUTE_AGENT_MODE", "standalone"),
        ]);
        let config = crate::config::Config::load(&environment);
        let state = Arc::new(Mutex::new(StateStore::open(directory.path()).unwrap()));
        let auth = crate::store::AuthzStore::open(directory.path(), "fixture-secret").unwrap();
        let server = crate::app::http_server(&config, auth, state).unwrap();
        server.providers.bind(&server);
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/mcp", listener.local_addr().unwrap());
        let (started, accepted) = tokio::sync::oneshot::channel();
        let (release, released) = std::sync::mpsc::channel();
        let provider = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut bytes = [0; 4096];
            assert!(socket.read(&mut bytes).unwrap() > 0);
            started.send(()).unwrap();
            released.recv_timeout(Duration::from_secs(3)).unwrap();
            let body = r#"{"jsonrpc":"2.0","id":1,"result":{"content":[],"structuredContent":{"ready":true}}}"#;
            write!(
                socket,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        });
        let manifest = super::super::definition::decode_manifest(&crate::gojson::parse(br#"{"provider":{"id":"opaque.provider","version":"v1"},"services":[{"id":"service","capabilityId":"opute.capability.test.v1","operations":[{"id":"test.probe","version":1,"effect":"read","inputSchema":{"type":"object"},"outputSchema":{"type":"object"}}]}]}"#).unwrap()).unwrap();
        let owner = &server.providers.activation;
        owner
            .lifecycle
            .restore(super::super::Generation {
                id: "g1".into(),
                provider: manifest.provider.clone(),
                endpoint: endpoint.clone(),
                manifest_hash: "hash-g1".into(),
                state: super::super::GenerationState::Active,
                ..Default::default()
            })
            .unwrap();
        owner.candidates.set_manifest("g1", manifest);
        owner.candidates.set_adapter(
            "g1",
            Some(Arc::new(super::super::adapter::Adapter::fixture_client(
                endpoint,
                std::collections::BTreeSet::from(["test.probe".into()]),
            ))),
        );
        owner
            .activate(
                json!({"activate":true,"providerGenerationId":"g1"})
                    .as_object()
                    .unwrap()
                    .clone(),
            )
            .await
            .unwrap();
        let handle = server.catalog.capability("test.probe").unwrap();
        let calling = handle.clone();
        let call = tokio::task::spawn_blocking(move || {
            (calling.invoke)(&RunCtx::background(), &Map::new())
        });
        tokio::time::timeout(Duration::from_secs(3), accepted)
            .await
            .unwrap()
            .unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(20), server.providers.close())
                .await
                .is_err()
        );
        assert!(!owner.mounts.service_keys().is_empty());
        release.send(()).unwrap();
        let result = tokio::time::timeout(Duration::from_secs(3), call)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(result["structuredContent"]["ready"], true);
        server.providers.close().await;
        assert!(owner.mounts.service_keys().is_empty());
        assert_eq!(owner.lifecycle.get("g1").unwrap().sessions, 0);
        let late = tokio::task::spawn_blocking(move || {
            (handle.invoke)(&RunCtx::background(), &Map::new())
        })
        .await
        .unwrap()
        .unwrap();
        assert_eq!(late["isError"], true);
        provider.join().unwrap();
    }
    #[tokio::test]
    #[ignore = "requires the owning provider runtime differential driver"]
    async fn runtime_fixture() {
        let rows: Value = serde_json::from_slice(
            &std::fs::read(std::env::var("PARITY_PROVIDER_RUNTIME_INPUT").unwrap()).unwrap(),
        )
        .unwrap();
        let mut outputs = Vec::new();
        for row in rows.as_array().unwrap() {
            let input = &row["runtime"];
            let directory = tempfile::tempdir().unwrap();
            let environment = crate::config::Env::from_pairs([
                ("OPUTE_REMOTE_AGENT_ID", "parity.runtime.owner"),
                (
                    "OPUTE_AGENT_MODE",
                    if input["standalone"].as_bool().unwrap() {
                        "standalone"
                    } else {
                        "platform"
                    },
                ),
            ]);
            let mut config = crate::config::Config::load(&environment);
            config.prefix_tool_names = false;
            config.standalone_allow_mutations = true;
            config.mcp_auth_token = "provider-runtime-bearer".into();
            config.host_resource_lock_dir = directory.path().join("locks");
            config.host_resource_disk_paths = vec![directory.path().to_string_lossy().into_owned()];
            config.host_resource_memory_capacity = 64 * 1024 * 1024 * 1024;
            let state = Arc::new(Mutex::new(StateStore::open(directory.path()).unwrap()));
            let auth = crate::store::AuthzStore::open(directory.path(), "provider-runtime-secret")
                .unwrap();
            let server = crate::app::http_server(&config, auth, state).unwrap();
            server.providers.bind(&server);
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let endpoint = format!("http://{}/mcp", listener.local_addr().unwrap());
            let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
            let serving = tokio::spawn({
                let server = server.clone();
                async move {
                    let mut stopped = stopped;
                    let mut connections = tokio::task::JoinSet::new();
                    loop {
                        tokio::select! {
                            _ = &mut stopped => break,
                            connection = listener.accept() => {
                                let (socket, _) = connection.unwrap();
                                let server = server.clone();
                                connections.spawn(crate::http1::serve_conn(socket, move |request, body| crate::transport::route(server.clone(), request, body)));
                            }
                        }
                    }
                    connections.abort_all();
                    while connections.join_next().await.is_some() {}
                }
            });
            let client = super::super::adapter::Client::new(
                endpoint,
                "provider-runtime-bearer".into(),
                "runtime-fixture-client".into(),
                "1".into(),
            )
            .unwrap();
            let owner = &server.providers.activation;
            let mut adapters = BTreeMap::new();
            let mut handles = BTreeMap::new();
            let mut observations = Vec::new();
            for step in input["steps"].as_array().unwrap() {
                let text = |key: &str| step[key].as_str().unwrap_or_default();
                let mut error = String::new();
                let mut result = Value::Null;
                match text("action") {
                    "restore" => {
                        error = owner
                            .lifecycle
                            .restore(serde_json::from_value(step["generation"].clone()).unwrap())
                            .err()
                            .unwrap_or_default()
                    }
                    "candidate" => {
                        let manifest = super::super::definition::decode_manifest(
                            &crate::gojson::parse(&serde_json::to_vec(&step["manifest"]).unwrap())
                                .unwrap(),
                        )
                        .unwrap();
                        owner.candidates.set_manifest(text("id"), manifest);
                    }
                    "connect" => match super::super::adapter::Adapter::connect(
                        &RunCtx::background(),
                        serde_json::from_value(step["descriptor"].clone()).unwrap(),
                        String::new(),
                        "",
                        "",
                    )
                    .await
                    {
                        Ok(adapter) => {
                            let adapter = Arc::new(adapter);
                            owner
                                .candidates
                                .set_adapter(text("id"), Some(adapter.clone()));
                            adapters.insert(text("id").to_string(), adapter);
                        }
                        Err(failure) => error = failure,
                    },
                    "activate" => {
                        error = owner
                            .activate(step["metadata"].as_object().unwrap().clone())
                            .await
                            .err()
                            .unwrap_or_default()
                    }
                    "fail" => error = owner.lifecycle.fail(text("id")).err().unwrap_or_default(),
                    "unmount" => {
                        owner
                            .mounts
                            .unmount_generation(text("provider"), text("id"))
                            .await
                    }
                    "handle" => {
                        handles.insert(
                            text("id").to_string(),
                            server.catalog.capability(text("operation")).unwrap(),
                        );
                    }
                    "invoke" => {
                        let handle = handles.get(text("id")).unwrap().clone();
                        let arguments = step["arguments"].as_object().cloned().unwrap_or_default();
                        match tokio::task::spawn_blocking(move || {
                            (handle.invoke)(&RunCtx::background(), &arguments)
                        })
                        .await
                        .unwrap()
                        {
                            Ok(value) => result = Value::Object(value),
                            Err(failure) => error = failure,
                        }
                    }
                    "status" | "validate" => {
                        let parameters = json!({"name":format!("opute.provider.{}",text("action")),"arguments":step["arguments"].as_object().cloned().unwrap_or_default()});
                        match client
                            .call(
                                &RunCtx::background(),
                                "tools/call",
                                parameters["name"].as_str().unwrap(),
                                parameters.as_object().unwrap().clone(),
                                "",
                            )
                            .await
                        {
                            Ok(value) => result = Value::Object(value),
                            Err(failure) => error = failure,
                        }
                    }
                    _ => panic!("unknown runtime fixture action"),
                }
                observations.push(json!({"error":error,"result":result}));
            }
            server.providers.close().await;
            for adapter in adapters.values() {
                adapter.close().await;
            }
            stop.send(()).unwrap();
            serving.await.unwrap();
            outputs.push(json!({"observations":observations}));
        }
        std::fs::write(
            std::env::var("PARITY_PROVIDER_RUNTIME_OUTPUT").unwrap(),
            serde_json::to_vec(&outputs).unwrap(),
        )
        .unwrap();
    }
}
