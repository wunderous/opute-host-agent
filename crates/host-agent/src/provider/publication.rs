//! Owning catalog publication from hostmcp/server.go. Registry replacement is
//! atomic, but family displacement precedes it in the pinned contract. Keep
//! registry and public projections distinct so failed publication is observable.
use super::{
    binding::{operations, Operation},
    definition::InstallManifest,
    Lifecycle,
};
use crate::catalog::{
    registry::{Capability, Registry},
    Snapshot,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};

struct State {
    catalog: Snapshot,
    wire_catalog: Snapshot,
    capabilities: BTreeMap<String, Arc<Capability>>,
    manifests: BTreeMap<String, InstallManifest>,
}
pub struct Publication {
    registry: Arc<Registry>,
    state: Mutex<State>,
}
impl Publication {
    pub fn new(registry: Arc<Registry>) -> Self {
        Self {
            state: Mutex::new(State {
                catalog: registry.snapshot(),
                wire_catalog: registry.snapshot(),
                capabilities: BTreeMap::new(),
                manifests: BTreeMap::new(),
            }),
            registry,
        }
    }
    pub fn snapshot(&self) -> Snapshot {
        self.state.lock().unwrap().catalog.clone()
    }
    /// Last completed MCP refresh, which can differ from the catalog snapshot
    /// after Go's no-op retirement following a failed family displacement.
    pub fn wire_snapshot(&self) -> Snapshot {
        self.state.lock().unwrap().wire_catalog.clone()
    }
    pub fn capability(&self, id: &str) -> Option<Arc<Capability>> {
        self.state.lock().unwrap().capabilities.get(id).cloned()
    }
    /// The MCP composition owner binds catalog entries not already backed by
    /// an executable to its trusted legacy dispatcher, as Go refresh does.
    pub fn ensure_legacy(
        &self,
        descriptor: &crate::catalog::Descriptor,
        factory: impl FnOnce(&crate::catalog::Descriptor) -> Arc<Capability>,
    ) {
        self.state
            .lock()
            .unwrap()
            .capabilities
            .entry(descriptor.operation_id.clone())
            .or_insert_with(|| factory(descriptor));
    }
    pub fn manifest(&self, id: &str) -> Option<InstallManifest> {
        self.state.lock().unwrap().manifests.get(id).cloned()
    }
    pub fn manifests(&self) -> BTreeMap<String, InstallManifest> {
        self.state.lock().unwrap().manifests.clone()
    }
    pub fn forget_manifest(&self, id: &str) {
        self.state.lock().unwrap().manifests.remove(id);
    }
    pub fn remember_manifest(&self, manifest: InstallManifest) {
        let id = manifest.provider.id.clone();
        self.remember_manifest_for(&id, manifest);
    }
    pub fn remember_manifest_for(&self, id: &str, manifest: InstallManifest) {
        self.state
            .lock()
            .unwrap()
            .manifests
            .insert(id.into(), manifest);
    }
    /// The composition factory binds an immutable operation to its executable.
    /// MCP refresh receives the entire committed projection, including edges.
    pub fn register(
        &self,
        manifest: &InstallManifest,
        generation: &str,
        factory: impl Fn(&Operation) -> Arc<Capability>,
        refresh: impl FnOnce(&Snapshot),
    ) -> Result<(), String> {
        self.registry.authorize_provider(&manifest.provider.id)?;
        let previous = self.registry.snapshot();
        let operations = operations(manifest, generation, false);
        let capabilities: Vec<_> = operations.iter().map(factory).collect();
        let mut seen = BTreeSet::new();
        let families: Vec<_> = manifest
            .services
            .iter()
            .filter_map(|service| {
                let family = service.capability_id.trim();
                (!family.is_empty() && seen.insert(family.to_owned())).then(|| family.to_owned())
            })
            .collect();
        let displaced = self
            .registry
            .displace_capability_families(&manifest.provider.id, &families);
        let registrations: Vec<_> = capabilities
            .iter()
            .map(|capability| Some(capability.clone()))
            .collect();
        if generation.is_empty() {
            self.registry
                .replace_provider(&manifest.provider.id, &registrations)?;
        } else {
            self.registry
                .replace_generation(generation, &registrations)?;
        }
        let current = self.registry.snapshot();
        {
            let mut state = self.state.lock().unwrap();
            for id in displaced.unwrap_or_default() {
                state.capabilities.remove(&id);
            }
            for descriptor in previous.tools {
                if descriptor.provider == manifest.provider.id {
                    state.capabilities.remove(&descriptor.operation_id);
                }
            }
            state.catalog = current.clone();
            for (operation, capability) in operations.iter().zip(capabilities) {
                state
                    .capabilities
                    .insert(operation.descriptor().operation_id.clone(), capability);
            }
        }
        refresh(&current);
        self.state.lock().unwrap().wire_catalog = current;
        self.remember_manifest(manifest.clone());
        Ok(())
    }
    pub fn retire(&self, provider: &str, generation: &str, refresh: impl FnOnce(&Snapshot)) {
        let (snapshot, changed) = {
            let mut state = self.state.lock().unwrap();
            let descriptors = state.catalog.tools.clone();
            let mut changed = false;
            for descriptor in descriptors {
                if descriptor.provider != provider
                    || (!generation.is_empty() && descriptor.generation_id != generation)
                {
                    continue;
                }
                if self.registry.unregister(&descriptor.operation_id).is_ok() {
                    state.capabilities.remove(&descriptor.operation_id);
                    changed = true;
                }
            }
            state.catalog = self.registry.snapshot();
            (state.catalog.clone(), changed)
        };
        if changed {
            refresh(&snapshot);
            self.state.lock().unwrap().wire_catalog = snapshot;
        }
        self.state.lock().unwrap().manifests.remove(provider);
    }
    pub fn restore(
        &self,
        provider: &str,
        manifest: Option<&InstallManifest>,
        lifecycle: &Lifecycle,
        factory: impl Fn(&Operation) -> Arc<Capability>,
        refresh: impl FnOnce(&Snapshot),
    ) -> Result<(), String> {
        if let Some(manifest) = manifest {
            let active = lifecycle.active(&manifest.provider.id).ok_or_else(|| {
                format!(
                    "provider {} has no active generation for catalog restore",
                    crate::goerr::quote(provider)
                )
            })?;
            self.register(manifest, &active.id, factory, refresh)
        } else {
            self.retire(provider, "", refresh);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    #[tokio::test]
    #[ignore = "requires actual owning Go publication and retirement driver"]
    async fn publication_fixture() {
        let rows: Value = serde_json::from_slice(
            &std::fs::read(std::env::var("PARITY_PROVIDER_PUBLICATION_INPUT").unwrap()).unwrap(),
        )
        .unwrap();
        let mut results = Vec::new();
        for row in rows.as_array().unwrap() {
            let input = &row["publication"];
            let standalone = input["standalone"].as_bool().unwrap_or(false);
            let registry = Arc::new(Registry::native(
                crate::catalog::for_mode(standalone).clone(),
                Default::default(),
            ));
            let owner = Arc::new(Publication::new(registry.clone()));
            let directory = tempfile::tempdir().unwrap();
            let http = if std::env::var("PARITY_PROVIDER_PUBLICATION_HTTP").as_deref() == Ok("1") {
                let environment = crate::config::Env::from_pairs([
                    ("OPUTE_REMOTE_AGENT_ID", "publication-http-fixture"),
                    (
                        "OPUTE_AGENT_MODE",
                        if standalone { "standalone" } else { "platform" },
                    ),
                ]);
                let mut config = crate::config::Config::load(&environment);
                config.prefix_tool_names = false;
                config.mcp_auth_token = "publication-fixture-bearer".into();
                let store = Arc::new(Mutex::new(
                    crate::store::StateStore::open(directory.path()).unwrap(),
                ));
                let auth =
                    crate::store::AuthzStore::open(directory.path(), "publication-fixture-secret")
                        .unwrap();
                let mut server = crate::app::http_server(&config, auth, store).unwrap();
                server.catalog = owner.clone();
                let server = Arc::new(server);
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                let endpoint = format!("http://{}/mcp", listener.local_addr().unwrap());
                let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
                let serving = tokio::spawn(async move {
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
                });
                Some((
                    super::super::adapter::Client::new(
                        endpoint,
                        "publication-fixture-bearer".into(),
                        "publication-fixture".into(),
                        "1".into(),
                    )
                    .unwrap(),
                    stop,
                    serving,
                ))
            } else {
                None
            };
            let lifecycle = Lifecycle::new(Default::default());
            let mut updates = 0;
            let mut wire = owner.snapshot();
            let mut observations = Vec::new();
            let mut handles: BTreeMap<String, Arc<Capability>> = BTreeMap::new();
            let mut snapshots: BTreeMap<String, Snapshot> = BTreeMap::new();
            for step in input["steps"].as_array().unwrap() {
                let string = |field: &str| step[field].as_str().unwrap_or_default();
                let manifest = if step["manifest"].is_null() {
                    None
                } else {
                    let raw = serde_json::to_vec(&step["manifest"]).unwrap();
                    Some(
                        super::super::definition::decode_manifest(
                            &crate::gojson::parse(&raw).unwrap(),
                        )
                        .unwrap(),
                    )
                };
                let factory = |operation: &Operation| {
                    Arc::new(Capability {
                        descriptor: operation.descriptor().clone(),
                        invoke: Arc::new(|_, _| Ok(Default::default())),
                    })
                };
                let mut error = String::new();
                let mut extra = json!({});
                match string("action") {
                    "register" => {
                        if let Err(err) = owner.register(
                            manifest.as_ref().unwrap(),
                            string("generation"),
                            factory,
                            |snapshot| {
                                updates += 1;
                                wire = snapshot.clone();
                                for descriptor in &snapshot.tools {
                                    if descriptor.implementation.trim().starts_with("provider:") {
                                        owner.ensure_legacy(descriptor, |descriptor| {
                                            Arc::new(Capability {
                                                descriptor: descriptor.clone(),
                                                invoke: Arc::new(|_, _| Ok(Default::default())),
                                            })
                                        });
                                    }
                                }
                            },
                        ) {
                            error = err;
                        }
                    }
                    "retire" => {
                        owner.retire(string("provider"), string("generation"), |snapshot| {
                            updates += 1;
                            wire = snapshot.clone();
                            for descriptor in &snapshot.tools {
                                if descriptor.implementation.trim().starts_with("provider:") {
                                    owner.ensure_legacy(descriptor, |descriptor| {
                                        Arc::new(Capability {
                                            descriptor: descriptor.clone(),
                                            invoke: Arc::new(|_, _| Ok(Default::default())),
                                        })
                                    });
                                }
                            }
                        })
                    }
                    "restore" => {
                        if let Err(err) = owner.restore(
                            string("provider"),
                            manifest.as_ref(),
                            &lifecycle,
                            factory,
                            |snapshot| {
                                updates += 1;
                                wire = snapshot.clone();
                                for descriptor in &snapshot.tools {
                                    if descriptor.implementation.trim().starts_with("provider:") {
                                        owner.ensure_legacy(descriptor, |descriptor| {
                                            Arc::new(Capability {
                                                descriptor: descriptor.clone(),
                                                invoke: Arc::new(|_, _| Ok(Default::default())),
                                            })
                                        });
                                    }
                                }
                            },
                        ) {
                            error = err;
                        }
                    }
                    "lifecycle" => {
                        if let Err(err) = lifecycle.restore(
                            serde_json::from_value(step["generationState"].clone()).unwrap(),
                        ) {
                            error = err;
                        }
                    }
                    "handle" => {
                        if let Some(capability) = owner.capability(string("id")) {
                            handles.insert(string("handle").into(), capability);
                        }
                    }
                    "snapshot" => {
                        snapshots.insert(string("handle").into(), owner.snapshot());
                    }
                    "readHandle" => {
                        extra["handle"] = handles
                            .get(string("handle"))
                            .map(|capability| descriptor_json(&capability.descriptor))
                            .unwrap_or(Value::Null);
                    }
                    "readSnapshot" => {
                        extra["snapshot"] = snapshots
                            .get(string("handle"))
                            .map(snapshot_json)
                            .unwrap_or(Value::Null);
                    }
                    _ => panic!("unknown publication fixture action"),
                }
                let wire_tools = if let Some((client, _, _)) = &http {
                    let result = client
                        .call(
                            &crate::plan::runner::RunCtx::background(),
                            "tools/list",
                            "",
                            serde_json::Map::new(),
                            "",
                        )
                        .await
                        .unwrap();
                    result.get("tools").unwrap().clone()
                } else {
                    json!(crate::catalog::published_tools(&wire, ""))
                };
                let state = owner.state.lock().unwrap();
                extra["error"] = json!(error);
                extra["updates"] = json!(updates);
                extra["wire"] = wire_tools;
                extra["published"] = snapshot_json(&state.catalog);
                extra["registry"] = snapshot_json(&registry.snapshot());
                extra["capabilities"] = Value::Object(
                    state
                        .capabilities
                        .iter()
                        .map(|(name, capability)| {
                            (name.clone(), descriptor_json(&capability.descriptor))
                        })
                        .collect(),
                );
                extra["manifests"] = Value::Object(
                    state
                        .manifests
                        .iter()
                        .map(|(name, manifest)| {
                            let encoded =
                                super::super::definition::encode_manifest(manifest).unwrap();
                            (name.clone(), serde_json::from_str(&encoded).unwrap())
                        })
                        .collect(),
                );
                observations.push(extra);
            }
            if let Some((_, stop, serving)) = http {
                stop.send(()).unwrap();
                serving.await.unwrap();
            }
            results.push(json!({"observations":observations}));
        }
        std::fs::write(
            std::env::var("PARITY_PROVIDER_PUBLICATION_OUTPUT").unwrap(),
            serde_json::to_vec_pretty(&results).unwrap(),
        )
        .unwrap();
    }
    fn snapshot_json(snapshot: &Snapshot) -> Value {
        let mut out = String::new();
        snapshot.encode(&mut out);
        serde_json::from_str(&out).unwrap()
    }
    fn descriptor_json(descriptor: &crate::catalog::Descriptor) -> Value {
        let mut out = String::new();
        descriptor.encode(&mut out);
        serde_json::from_str(&out).unwrap()
    }
}
