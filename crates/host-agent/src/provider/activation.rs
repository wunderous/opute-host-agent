//! Owned activation composition from hostmcp/recipe_run.go. This executes one
//! explicit lifecycle transition; recipes retain the existing single runner.
use super::{
    binding::Operation,
    candidates::Candidates,
    definition::{
        encode_descriptor, encode_manifest, GoSlice, InstallManifest, PluginDescriptor,
        ServerDescriptor,
    },
    mounts::Mounts,
    publication::Publication,
    Generation, GenerationState, Lifecycle,
};
use crate::{
    catalog::{registry::Capability, Snapshot},
    goerr::quote,
    hostobs::rfc3339_nano_now,
    store::{ProviderGenerationRecord, StateStore},
};
use serde::Serialize;
use serde_json::{Map, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Event {
    pub name: String,
    pub provider_id: String,
    pub generation_id: String,
    pub catalog_revision: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub reason: String,
    pub at: String,
}
type Factory = Arc<dyn Fn(&Operation) -> Arc<Capability> + Send + Sync>;
type Refresh = Arc<dyn Fn(&Snapshot) + Send + Sync>;
type Events = Arc<dyn Fn(&Event) -> Result<(), String> + Send + Sync>;

/// Trusted composition callbacks. The event owner implements the pinned serial
/// admission listener contract; announcement errors cannot undo committed work.
pub struct Hooks {
    pub capability: Factory,
    pub refresh: Refresh,
    pub event: Events,
}
pub struct Activation {
    pub lifecycle: Lifecycle,
    pub candidates: Arc<Candidates>,
    pub mounts: Arc<Mounts>,
    pub publication: Arc<Publication>,
    pub state: Option<Arc<Mutex<StateStore>>>,
    hooks: Hooks,
    validation: Mutex<BTreeMap<String, String>>,
}
impl Activation {
    pub fn new(
        lifecycle: Lifecycle,
        candidates: Arc<Candidates>,
        mounts: Arc<Mounts>,
        publication: Arc<Publication>,
        state: Option<Arc<Mutex<StateStore>>>,
        hooks: Hooks,
    ) -> Arc<Self> {
        Arc::new(Self {
            lifecycle,
            candidates,
            mounts,
            publication,
            state,
            hooks,
            validation: Mutex::new(BTreeMap::new()),
        })
    }
    pub fn validation(&self) -> BTreeMap<String, String> {
        self.validation.lock().unwrap().clone()
    }
    fn event(
        &self,
        name: &str,
        provider: &str,
        generation: &str,
        reason: &str,
    ) -> Result<(), String> {
        (self.hooks.event)(&Event {
            name: name.into(),
            provider_id: provider.into(),
            generation_id: generation.into(),
            catalog_revision: self.publication.snapshot().revision,
            reason: reason.into(),
            at: rfc3339_nano_now(),
        })
    }
    fn register(&self, manifest: &InstallManifest, generation: &str) -> Result<(), String> {
        self.publication.register(
            manifest,
            generation,
            &*self.hooks.capability,
            &*self.hooks.refresh,
        )
    }
    fn restore(&self, provider: &str, previous: Option<&InstallManifest>) -> Result<(), String> {
        self.publication.restore(
            provider,
            previous,
            &self.lifecycle,
            &*self.hooks.capability,
            &*self.hooks.refresh,
        )
    }
    pub fn persist(&self, generation: &Generation) -> Result<(), String> {
        let Some(state) = &self.state else {
            return Ok(());
        };
        let manifest = self
            .publication
            .manifest(&generation.provider.id)
            .or_else(|| {
                self.candidates.manifests().into_iter().find(|manifest| {
                    manifest.provider.id == generation.provider.id
                        && manifest.provider.version == generation.provider.version
                })
            });
        let mut descriptor_json = String::new();
        let mut manifest_json = String::new();
        if let Some(manifest) = manifest {
            let capabilities =
                GoSlice((!manifest.provides.is_empty()).then(|| manifest.provides.to_vec()));
            descriptor_json = encode_descriptor(&PluginDescriptor {
                schema: "opute-provider-plugin.v1".into(),
                plugin_id: generation.provider.id.clone(),
                version: generation.provider.version.clone(),
                capabilities,
                server: ServerDescriptor {
                    transport: "streamable_http".into(),
                    endpoint: generation.endpoint.clone(),
                    ..Default::default()
                },
                ..Default::default()
            });
            // Go ignores the marshaling error while constructing its durable row.
            manifest_json = encode_manifest(&manifest).unwrap_or_default();
        }
        state
            .lock()
            .unwrap()
            .save_provider_generation(&ProviderGenerationRecord {
                generation_id: generation.id.clone(),
                provider_id: generation.provider.id.clone(),
                provider_version: generation.provider.version.clone(),
                manifest_hash: generation.manifest_hash.clone(),
                endpoint: generation.endpoint.clone(),
                descriptor_json,
                manifest_json,
                catalog_revision: generation.catalog_revision.clone(),
                status: generation.state.name().into(),
                created_at: generation.created_at.clone(),
                active_at: if generation.active_at == "0001-01-01T00:00:00Z" {
                    String::new()
                } else {
                    generation.active_at.clone()
                },
            })
    }
    /// Background ownership matches Go's activation work. Dropping the caller
    /// never strands publication between its durable and mount stages.
    pub async fn activate(self: &Arc<Self>, metadata: Map<String, Value>) -> Result<(), String> {
        let owner = self.clone();
        tokio::spawn(async move { owner.activate_owned(&metadata).await })
            .await
            .expect("provider activation owner panicked")
    }
    async fn activate_owned(&self, metadata: &Map<String, Value>) -> Result<(), String> {
        if metadata.get("activate").and_then(Value::as_bool) != Some(true) {
            return Ok(());
        }
        let id = metadata
            .get("providerGenerationId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim();
        if id.is_empty() {
            return Ok(());
        }
        if let Some(existing) = self.lifecycle.get(id) {
            if self
                .lifecycle
                .active(&existing.provider.id)
                .is_some_and(|active| active.id == id)
            {
                if let Some(manifest) = self.candidates.manifest(id) {
                    let publish = publish_services(&manifest, metadata)?;
                    self.register(&publish, id).map_err(|e| {
                        format!("reaffirm provider services for active generation: {e}")
                    })?;
                    self.remember(&existing.provider.id, &manifest);
                    if let Some(adapter) = self.candidates.adapter(id) {
                        self.mounts
                            .mount(&publish, id, Some(adapter))
                            .await
                            .map_err(|e| {
                                format!("reaffirm provider mount for active generation: {e}")
                            })?;
                        self.candidates.remove(id);
                    }
                }
                return Ok(());
            }
        }
        let candidate = self.candidates.manifest(id);
        let manifest = candidate.clone().unwrap_or_default();
        let previous_manifest = self.publication.manifest(&manifest.provider.id);
        if !self
            .lifecycle
            .get(id)
            .is_some_and(|g| g.state == GenerationState::Ready)
        {
            self.lifecycle
                .mark_ready(id)
                .map_err(|e| format!("provider generation readiness: {e}"))?;
        }
        if let Some(ready) = self.lifecycle.get(id) {
            self.persist(&ready)
                .map_err(|e| format!("persist provider generation readiness: {e}"))?;
        }
        let _ = self.event("provider.ready", &manifest.provider.id, id, "");
        if let Err(error) = self.event("provider.admission", &manifest.provider.id, id, "") {
            let _ = self.lifecycle.fail(id);
            let _ = self.restore(&manifest.provider.id, previous_manifest.as_ref());
            return Err(format!(
                "provider {} generation {} was denied admission: {error}",
                quote(&manifest.provider.id),
                quote(id)
            ));
        }
        let publish = if candidate.is_some() {
            let publish = publish_services(&manifest, metadata)?;
            if let Err(error) = self.register(&publish, id) {
                let _ = self.lifecycle.fail(id);
                let _ = self.restore(&manifest.provider.id, previous_manifest.as_ref());
                return Err(format!("register provider services: {error}"));
            }
            publish
        } else {
            InstallManifest::default()
        };
        let (previous, activated) = match self.lifecycle.activate(id) {
            Ok(value) => value,
            Err(error) => {
                if candidate.is_some() {
                    let _ = self.restore(&manifest.provider.id, previous_manifest.as_ref());
                }
                return Err(format!("provider generation activation: {error}"));
            }
        };
        if let Some(previous) = &previous {
            if let Err(error) = self.persist(previous) {
                return Err(self
                    .rollback(
                        candidate.as_ref(),
                        previous_manifest.as_ref(),
                        &activated,
                        Some(previous),
                        format!("persist draining provider generation: {error}"),
                    )
                    .await);
            }
        }
        if let Err(error) = self.persist(&activated) {
            return Err(self
                .rollback(
                    candidate.as_ref(),
                    previous_manifest.as_ref(),
                    &activated,
                    previous.as_ref(),
                    format!("persist active provider generation: {error}"),
                )
                .await);
        }
        if let Some(adapter) = self.candidates.adapter(id) {
            let mut seen = BTreeSet::new();
            let families = publish
                .services
                .iter()
                .filter_map(|service| {
                    let family = service.capability_id.trim();
                    (!family.is_empty() && seen.insert(family.to_owned()))
                        .then(|| family.to_owned())
                })
                .collect::<Vec<_>>();
            self.mounts.displace(&manifest.provider.id, &families).await;
            if let Err(error) = self.mounts.mount(&publish, id, Some(adapter)).await {
                return Err(self
                    .rollback(
                        candidate.as_ref(),
                        previous_manifest.as_ref(),
                        &activated,
                        previous.as_ref(),
                        format!("mount provider generation: {error}"),
                    )
                    .await);
            }
        }
        if candidate.is_some() {
            self.remember(&activated.provider.id, &manifest);
        }
        self.candidates.remove(id);
        if let Some(previous) = previous {
            let _ = self.event(
                "provider.draining",
                &previous.provider.id,
                &previous.id,
                "superseded",
            );
            self.mounts
                .unmount_generation(&previous.provider.id, &previous.id)
                .await;
            let _ = self.event(
                "provider.stopped",
                &previous.provider.id,
                &previous.id,
                "superseded",
            );
        }
        let _ = self.event(
            "provider.activated",
            &activated.provider.id,
            &activated.id,
            "",
        );
        Ok(())
    }
    fn remember(&self, provider: &str, manifest: &InstallManifest) {
        self.validation
            .lock()
            .unwrap()
            .insert(provider.into(), manifest.validation.operation.clone());
        self.publication
            .remember_manifest_for(provider, manifest.clone());
    }
    pub async fn rollback(
        &self,
        candidate: Option<&InstallManifest>,
        previous_manifest: Option<&InstallManifest>,
        activated: &Generation,
        previous: Option<&Generation>,
        cause: String,
    ) -> String {
        self.mounts
            .unmount_generation(&activated.provider.id, &activated.id)
            .await;
        let mut lifecycle_error = self
            .lifecycle
            .rollback_activation(
                &activated.id,
                previous.map(|g| g.id.as_str()).unwrap_or_default(),
            )
            .err();
        if lifecycle_error.is_none() {
            if let Some(previous) = previous {
                if let Some(restored) = self.lifecycle.get(&previous.id) {
                    lifecycle_error = self
                        .persist(&restored)
                        .err()
                        .map(|e| format!("persist restored provider generation: {e}"));
                }
            }
        }
        let catalog_error = candidate
            .and_then(|manifest| self.restore(&manifest.provider.id, previous_manifest).err());
        if lifecycle_error.is_some() || catalog_error.is_some() {
            format!(
                "{cause}; rollback failed: lifecycle={} catalog={} unmount=<nil>",
                lifecycle_error.as_deref().unwrap_or("<nil>"),
                catalog_error.as_deref().unwrap_or("<nil>")
            )
        } else {
            cause
        }
    }
    pub async fn cleanup(self: &Arc<Self>, id: &str) {
        let owner = self.clone();
        let id = id.to_owned();
        tokio::spawn(async move {
            let _ = owner.lifecycle.fail(&id);
            let adapter = owner.candidates.adapter(&id);
            owner.candidates.remove(&id);
            if let Some(adapter) = adapter {
                adapter.close().await;
            }
            if let Some(generation) = owner.lifecycle.get(&id) {
                let _ = owner.persist(&generation);
            }
        })
        .await
        .expect("provider candidate cleanup owner panicked");
    }
}

pub fn publish_services(
    manifest: &InstallManifest,
    metadata: &Map<String, Value>,
) -> Result<InstallManifest, String> {
    let runtime = metadata
        .get("runtime")
        .and_then(Value::as_object)
        .or_else(|| {
            metadata
                .get("recipeDocument")
                .and_then(Value::as_object)?
                .get("runtime")?
                .as_object()
        });
    let allow = runtime
        .and_then(|runtime| runtime.get("capabilities")?.as_array())
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter_map(|name| {
            let name = name.trim();
            if name.is_empty() {
                return None;
            }
            Some(if name.starts_with("opute.capability.") {
                if name.contains(".v") {
                    name.to_owned()
                } else {
                    format!("{name}.v1")
                }
            } else {
                format!("opute.capability.{name}.v1")
            })
        })
        .collect::<BTreeSet<_>>();
    let mut publish = manifest.clone();
    if !allow.is_empty() {
        let services = manifest
            .services
            .iter()
            .filter(|service| allow.contains(service.capability_id.trim()))
            .cloned()
            .collect::<Vec<_>>();
        if services.is_empty() {
            return Err("recipe runtime.capabilities matched no provider Services; refusing empty MCP publish surface".into());
        }
        publish.services = GoSlice(Some(services));
    }
    Ok(publish)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::registry::Registry;
    use serde_json::json;

    #[test]
    fn caller_abort_does_not_strand_accepted_activation() {
        let lifecycle = Lifecycle::new(Default::default());
        lifecycle
            .restore(Generation {
                id: "g1".into(),
                provider: super::super::ProviderRef {
                    id: "opaque.provider".into(),
                    version: "v1".into(),
                },
                manifest_hash: "hash".into(),
                endpoint: "http://127.0.0.1:1".into(),
                ..Default::default()
            })
            .unwrap();
        let candidates = Arc::new(Candidates::default());
        let manifest = super::super::definition::decode_manifest(&crate::gojson::parse(br#"{"provider":{"id":"opaque.provider","version":"v1"},"services":[{"id":"service","capabilityId":"test.family","operations":[{"id":"test.probe","version":1,"effect":"read","inputSchema":{"type":"object"},"outputSchema":{"type":"object"}}]}]}"#).unwrap()).unwrap();
        candidates.set_manifest("g1", manifest);
        let publication = Arc::new(Publication::new(Arc::new(Registry::native(
            crate::catalog::for_mode(false).clone(),
            Default::default(),
        ))));
        let directory = tempfile::tempdir().unwrap();
        let state = Arc::new(Mutex::new(StateStore::open(directory.path()).unwrap()));
        let gate = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
        struct Release(Arc<(Mutex<bool>, std::sync::Condvar)>);
        impl Drop for Release {
            fn drop(&mut self) {
                *self.0 .0.lock().unwrap() = true;
                self.0 .1.notify_all();
            }
        }
        let release = Release(gate.clone());
        let (sender, receiver) = std::sync::mpsc::channel();
        let owner = Activation::new(
            lifecycle.clone(),
            candidates.clone(),
            Arc::new(Mounts::new(lifecycle.clone())),
            publication.clone(),
            Some(state.clone()),
            Hooks {
                capability: Arc::new(|operation| {
                    Arc::new(Capability {
                        descriptor: operation.descriptor().clone(),
                        invoke: Arc::new(|_, _| Ok(Default::default())),
                    })
                }),
                refresh: Arc::new(|_| {}),
                event: Arc::new(move |event| {
                    sender.send(event.name.clone()).unwrap();
                    if event.name == "provider.ready" {
                        let mut unlocked = gate.0.lock().unwrap();
                        while !*unlocked {
                            unlocked = gate.1.wait(unlocked).unwrap();
                        }
                    }
                    Ok(())
                }),
            },
        );
        let (handle_sender, handle_receiver) = std::sync::mpsc::channel();
        let worker = std::thread::spawn({
            let owner = owner.clone();
            move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                runtime.block_on(async move {
                    let caller = tokio::spawn(async move {
                        owner
                            .activate(
                                json!({"activate":true,"providerGenerationId":"g1"})
                                    .as_object()
                                    .unwrap()
                                    .clone(),
                            )
                            .await
                    });
                    handle_sender.send(caller.abort_handle()).unwrap();
                    assert!(caller.await.unwrap_err().is_cancelled());
                });
            }
        });
        let timeout = std::time::Duration::from_secs(5);
        let caller = handle_receiver.recv_timeout(timeout).unwrap();
        assert_eq!(receiver.recv_timeout(timeout).unwrap(), "provider.ready");
        caller.abort();
        drop(release);
        assert_eq!(
            receiver.recv_timeout(timeout).unwrap(),
            "provider.admission"
        );
        assert_eq!(
            receiver.recv_timeout(timeout).unwrap(),
            "provider.activated"
        );
        worker.join().unwrap();
        assert_eq!(lifecycle.active("opaque.provider").unwrap().id, "g1");
        assert!(candidates.manifest("g1").is_none());
        assert_eq!(
            publication
                .capability("test.probe")
                .unwrap()
                .descriptor
                .generation_id,
            "g1"
        );
        assert_eq!(
            state.lock().unwrap().list_provider_generations().unwrap()[0].status,
            "active"
        );
    }

    #[tokio::test]
    #[ignore = "requires owning Go activation differential driver"]
    async fn activation_fixture() {
        let rows: Value = serde_json::from_slice(
            &std::fs::read(std::env::var("PARITY_PROVIDER_ACTIVATION_INPUT").unwrap()).unwrap(),
        )
        .unwrap();
        let mut outputs = Vec::new();
        for row in rows.as_array().unwrap() {
            let input = &row["activation"];
            let registry = Arc::new(Registry::native(
                crate::catalog::for_mode(input["standalone"].as_bool().unwrap_or(false)).clone(),
                Default::default(),
            ));
            let publication = Arc::new(Publication::new(registry.clone()));
            let lifecycle = Lifecycle::new(Default::default());
            let candidates = Arc::new(Candidates::default());
            let directory = tempfile::tempdir().unwrap();
            let state = Arc::new(Mutex::new(StateStore::open(directory.path()).unwrap()));
            let events = Arc::new(Mutex::new(Vec::<Event>::new()));
            let denial = Arc::new(Mutex::new(String::new()));
            let hooks = Hooks {
                capability: Arc::new(|operation| {
                    Arc::new(Capability {
                        descriptor: operation.descriptor().clone(),
                        invoke: Arc::new(|_, _| Ok(Default::default())),
                    })
                }),
                refresh: Arc::new(|_| {}),
                event: Arc::new({
                    let events = events.clone();
                    let denial = denial.clone();
                    move |event| {
                        events.lock().unwrap().push(event.clone());
                        if event.name == "provider.admission" && !denial.lock().unwrap().is_empty()
                        {
                            Err(denial.lock().unwrap().clone())
                        } else {
                            Ok(())
                        }
                    }
                }),
            };
            let owner = Activation::new(
                lifecycle.clone(),
                candidates.clone(),
                Arc::new(Mounts::new(lifecycle.clone())),
                publication.clone(),
                Some(state.clone()),
                hooks,
            );
            let mut adapters: BTreeMap<String, Arc<super::super::adapter::Adapter>> =
                BTreeMap::new();
            let mut ids = BTreeSet::new();
            let mut providers = BTreeSet::new();
            let mut observations = Vec::new();
            for step in input["steps"].as_array().unwrap() {
                let text = |key: &str| step[key].as_str().unwrap_or_default();
                let manifest = if step["manifest"].is_null() {
                    None
                } else {
                    Some(
                        super::super::definition::decode_manifest(
                            &crate::gojson::parse(&serde_json::to_vec(&step["manifest"]).unwrap())
                                .unwrap(),
                        )
                        .unwrap(),
                    )
                };
                let mut error = String::new();
                let mut probe = Value::Null;
                match text("action") {
                    "restore" => {
                        let generation: Generation =
                            serde_json::from_value(step["generation"].clone()).unwrap();
                        ids.insert(generation.id.clone());
                        providers.insert(generation.provider.id.clone());
                        error = lifecycle.restore(generation).err().unwrap_or_default();
                    }
                    "connect" => {
                        match super::super::adapter::Adapter::connect(
                            &crate::plan::runner::RunCtx::background(),
                            serde_json::from_value(step["descriptor"].clone()).unwrap(),
                            String::new(),
                            "",
                            "",
                        )
                        .await
                        {
                            Ok(adapter) => {
                                let adapter = Arc::new(adapter);
                                candidates.set_adapter(text("id"), Some(adapter.clone()));
                                adapters.insert(text("id").into(), adapter);
                            }
                            Err(failure) => error = failure,
                        }
                    }
                    "mount" => {
                        error = owner
                            .mounts
                            .mount(
                                manifest.as_ref().unwrap(),
                                text("id"),
                                adapters.get(text("id")).cloned(),
                            )
                            .await
                            .err()
                            .unwrap_or_default()
                    }
                    "unmount" => {
                        owner
                            .mounts
                            .unmount_generation(text("provider"), text("id"))
                            .await
                    }
                    "cleanup" => owner.cleanup(text("id")).await,
                    "probe" => {
                        if let Some(adapter) = adapters.get(text("id")) {
                            match adapter
                                .call_synchronous_only(
                                    &crate::plan::runner::RunCtx::background(),
                                    text("operation"),
                                    Some(Map::new()),
                                    "",
                                )
                                .await
                            {
                                Ok(result) => {
                                    probe = result
                                        .get("structuredContent")
                                        .cloned()
                                        .unwrap_or(Value::Null)
                                }
                                Err(failure) => error = failure,
                            }
                        } else {
                            error = "fixture adapter is absent".into();
                        }
                    }
                    "candidate" => {
                        if let Some(manifest) = manifest {
                            candidates.set_manifest(text("id"), manifest);
                        } else {
                            candidates.remove_manifest(text("id"));
                        }
                    }
                    "remember" => {
                        if let Some(manifest) = manifest {
                            publication.remember_manifest(manifest);
                        } else {
                            publication.forget_manifest(text("provider"));
                        }
                    }
                    "register" => {
                        error = owner
                            .register(manifest.as_ref().unwrap(), text("id"))
                            .err()
                            .unwrap_or_default();
                    }
                    "admission" => *denial.lock().unwrap() = text("denial").into(),
                    "activate" => {
                        error = owner
                            .activate(step["metadata"].as_object().cloned().unwrap_or_default())
                            .await
                            .err()
                            .unwrap_or_default();
                    }
                    "complete" => candidates.remove(text("id")),
                    "closeState" => state.lock().unwrap().close(),
                    "rollback" => {
                        let activated = lifecycle.get(text("id")).unwrap_or_default();
                        let previous = lifecycle.get(text("previousID"));
                        let old = manifest
                            .as_ref()
                            .and_then(|m| publication.manifest(&m.provider.id));
                        error = owner
                            .rollback(
                                manifest.as_ref(),
                                old.as_ref(),
                                &activated,
                                previous.as_ref(),
                                text("cause").into(),
                            )
                            .await;
                    }
                    _ => panic!("unknown activation fixture action"),
                }
                let generations = ids
                    .iter()
                    .filter_map(|id| lifecycle.get(id).map(|g| (id.clone(), g)))
                    .collect::<BTreeMap<_, _>>();
                let active = providers
                    .iter()
                    .filter_map(|provider| {
                        lifecycle.active(provider).map(|g| (provider.clone(), g))
                    })
                    .collect::<BTreeMap<_, _>>();
                let manifest_map = |values: BTreeMap<String, InstallManifest>| {
                    values
                        .into_iter()
                        .map(|(id, m)| {
                            (
                                id,
                                serde_json::from_str::<Value>(&encode_manifest(&m).unwrap())
                                    .unwrap(),
                            )
                        })
                        .collect::<BTreeMap<_, _>>()
                };
                let candidate_map = candidates.manifest_entries();
                let remembered = publication.manifests();
                let (durable, durable_error) = match state
                    .lock()
                    .unwrap()
                    .list_provider_generations()
                {
                    Ok(rows) => (
                        if rows.is_empty() {
                            Value::Null
                        } else {
                            Value::Array(rows.into_iter().map(|r| json!({
                        "GenerationID":r.generation_id,"ProviderID":r.provider_id,"ProviderVersion":r.provider_version,
                        "ManifestHash":r.manifest_hash,"Endpoint":r.endpoint,"DescriptorJSON":r.descriptor_json,"ManifestJSON":r.manifest_json,
                        "CatalogRevision":r.catalog_revision,"Status":r.status,"CreatedAt":r.created_at,"ActiveAt":r.active_at
                    })).collect())
                        },
                        String::new(),
                    ),
                    Err(error) => (Value::Null, error),
                };
                let snapshot = |value: Snapshot| {
                    let mut out = String::new();
                    value.encode(&mut out);
                    serde_json::from_str::<Value>(&out).unwrap()
                };
                let mut observation = json!({"error":error,"generations":generations,"active":active,
                    "candidates":manifest_map(candidate_map),"manifests":manifest_map(remembered),"validation":owner.validation(),
                    "events":events.lock().unwrap().clone(),"catalog":snapshot(publication.snapshot()),"registry":snapshot(registry.snapshot()),
                    "durable":durable,"durableError":durable_error});
                if input["observeMounts"].as_bool().unwrap_or(false) {
                    let mounted = owner.mounts.plugin_ids().into_iter().map(|id| {
                        // The fixture knows each service's typed declaration; no key parsing.
                        let service = ids.iter().find_map(|generation| {
                            let provider = lifecycle.get(generation)?;
                            let manifest = candidates.manifest(generation).or_else(|| publication.manifest(&provider.provider.id))?;
                            manifest.services.iter().find_map(|definition| {
                                owner.mounts.resolve(&provider.provider.id, &definition.id, generation).filter(|service| service.key == id)
                            })
                        }).expect("fixture mounted service declaration");
                        json!({"key":service.key,"provider":service.provider_id,"generation":service.generation_id,"capability":service.capability_id,"inject":service.inject})
                    }).collect::<Vec<_>>();
                    let candidate_adapters = ids
                        .iter()
                        .filter(|id| candidates.adapter(id).is_some())
                        .collect::<Vec<_>>();
                    observation["mounts"] = json!(mounted);
                    observation["candidateAdapters"] = json!(candidate_adapters);
                    if text("action") == "probe" {
                        observation["probe"] = probe;
                    }
                }
                observations.push(observation);
            }
            owner.mounts.close().await;
            for adapter in adapters.values() {
                adapter.close().await;
            }
            outputs.push(json!({"observations":observations}));
        }
        std::fs::write(
            std::env::var("PARITY_PROVIDER_ACTIVATION_OUTPUT").unwrap(),
            serde_json::to_vec(&outputs).unwrap(),
        )
        .unwrap();
    }
}
