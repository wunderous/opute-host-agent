//! The one service context (decision D14): generation-scoped service
//! ownership from provider_service_plugin.go, for provider generations and
//! the built-in provider alike. Public activation and callback admission are
//! owned by the composition layer.
use super::{
    adapter::Adapter,
    definition::{service_key, CapabilityRef, InstallManifest, ServiceDefinition},
    Lifecycle,
};
use crate::goerr::quote;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

/// What implements a generation's services: a provider's MCP adapter, or
/// the built-in provider in process. Registration, dependencies, families,
/// resolution and disposal do not depend on which.
#[derive(Clone)]
pub enum Implementation {
    Adapter(Arc<Adapter>),
    Builtin(Arc<crate::builtin::Provider>),
}

#[derive(Clone)]
pub struct Service {
    pub key: String,
    pub provider_id: String,
    pub generation_id: String,
    pub capability_id: String,
    pub definition: ServiceDefinition,
    pub implementation: Implementation,
    pub inject: Vec<String>,
}

impl Service {
    pub fn offering_key(&self) -> String {
        service_key(&self.provider_id, &self.definition.id)
    }
    pub fn adapter(&self) -> Option<&Arc<Adapter>> {
        match &self.implementation {
            Implementation::Adapter(adapter) => Some(adapter),
            Implementation::Builtin(_) => None,
        }
    }
}
struct Mount {
    service: Arc<Service>,
    owns_adapter: bool,
    disposed: bool,
}
pub struct Mounts {
    lifecycle: Lifecycle,
    services: Arc<Mutex<Vec<Mount>>>,
}
pub fn key(provider: &str, service: &str, generation: &str) -> String {
    format!("{}@{}", service_key(provider, service), generation.trim())
}
fn dependencies(
    required: &[CapabilityRef],
    provided: &BTreeMap<String, String>,
) -> Result<Vec<String>, String> {
    let mut keys = Vec::new();
    for dependency in required {
        let family = dependency.id.trim();
        let matches: Vec<_> = provided
            .iter()
            .filter(|(_, id)| *id == family)
            .map(|(key, _)| key.clone())
            .collect();
        match matches.len() {
            0 => return Err(format!("capability {} is not provided by any mounted service", quote(family))),
            1 => keys.push(matches[0].clone()),
            count => return Err(format!("capability {} is provided by {count} mounted services; typed provenance is required", quote(family))),
        }
    }
    Ok(keys)
}
impl Mounts {
    pub fn new(lifecycle: Lifecycle) -> Self {
        Self {
            lifecycle,
            services: Arc::new(Mutex::new(Vec::new())),
        }
    }
    fn families(&self, services: &[Mount], generation: &str) -> BTreeMap<String, String> {
        services
            .iter()
            .filter(|mount| {
                mount.service.generation_id == generation
                    || self
                        .lifecycle
                        .active(&mount.service.provider_id)
                        .is_some_and(|active| active.id == mount.service.generation_id)
            })
            .map(|mount| {
                (
                    mount.service.key.clone(),
                    mount.service.capability_id.clone(),
                )
            })
            .collect()
    }
    pub fn injectable_families(&self, generation: &str) -> BTreeMap<String, String> {
        self.families(&self.services.lock().unwrap(), generation)
    }
    fn mount_service(
        &self,
        manifest: &InstallManifest,
        generation: &str,
        implementation: Implementation,
        definition: &ServiceDefinition,
        owns_adapter: bool,
    ) -> Result<String, String> {
        let mut services = self.services.lock().unwrap();
        let inject = dependencies(
            &definition.dependencies,
            &self.families(&services, generation),
        )?;
        let key = key(&manifest.provider.id, &definition.id, generation);
        if services.iter().any(|mount| mount.service.key == key) {
            return Err(format!("plugin {} is already mounted", quote(&key)));
        }
        services.push(Mount {
            service: Arc::new(Service {
                key: key.clone(),
                provider_id: manifest.provider.id.clone(),
                generation_id: generation.into(),
                capability_id: definition.capability_id.trim().into(),
                definition: definition.clone(),
                implementation,
                inject,
            }),
            owns_adapter,
            disposed: false,
        });
        Ok(key)
    }
    /// Register every service of a generation, in manifest order. On failure
    /// the services already registered are returned for disposal.
    fn register(
        &self,
        manifest: &InstallManifest,
        generation: &str,
        implementation: Option<Implementation>,
    ) -> Result<(), (String, Vec<String>)> {
        let implementation = implementation.ok_or_else(|| {
            (
                format!(
                    "provider {} generation {} has no connected adapter",
                    quote(&manifest.provider.id),
                    quote(generation)
                ),
                Vec::new(),
            )
        })?;
        if generation.trim().is_empty() {
            return Err((
                format!(
                    "provider {} must be mounted against a generation",
                    quote(&manifest.provider.id)
                ),
                Vec::new(),
            ));
        }
        let mut mounted = Vec::new();
        for service in manifest.services.iter() {
            match self.mount_service(
                manifest,
                generation,
                implementation.clone(),
                service,
                mounted.is_empty(),
            ) {
                Ok(key) => mounted.push(key),
                Err(error) => {
                    return Err((
                        format!("mount provider service {}: {error}", quote(&service.id)),
                        mounted,
                    ))
                }
            }
        }
        Ok(())
    }
    pub async fn mount(
        &self,
        manifest: &InstallManifest,
        generation: &str,
        implementation: Option<Implementation>,
    ) -> Result<(), String> {
        match self.register(manifest, generation, implementation) {
            Ok(()) => Ok(()),
            Err((error, mounted)) => {
                self.unmount_ids(&mounted).await;
                Err(error)
            }
        }
    }
    /// Mount a generation whose implementation owns no connection to close,
    /// such as the built-in provider, without an async runtime.
    pub fn mount_now(
        &self,
        manifest: &InstallManifest,
        generation: &str,
        implementation: Implementation,
    ) -> Result<(), String> {
        match self.register(manifest, generation, Some(implementation)) {
            Ok(()) => Ok(()),
            Err((error, mounted)) => {
                self.unmount_now(&mounted);
                Err(error)
            }
        }
    }
    /// Dispose services that own no adapter connection, in reverse order and
    /// exactly once. A service that owns an adapter is left for
    /// `unmount_ids`, which closes it.
    pub fn unmount_now(&self, ids: &[String]) {
        let mut services = self.services.lock().unwrap();
        for id in ids.iter().rev() {
            let Some(index) = services.iter().position(|mount| mount.service.key == *id) else {
                continue;
            };
            if services[index].owns_adapter && services[index].service.adapter().is_some() {
                continue;
            }
            services[index].disposed = true;
            services.remove(index);
        }
    }
    /// The service of `provider`'s `generation` that offers `operation`.
    pub fn resolve_operation(
        &self,
        provider: &str,
        generation: &str,
        operation: &str,
    ) -> Option<Arc<Service>> {
        self.services
            .lock()
            .unwrap()
            .iter()
            .filter(|mount| {
                !mount.disposed
                    && mount.service.provider_id == provider
                    && mount.service.generation_id == generation
            })
            .find(|mount| {
                mount
                    .service
                    .definition
                    .operations
                    .iter()
                    .any(|candidate| candidate.id == operation)
            })
            .map(|mount| mount.service.clone())
    }
    /// The services of one provider generation, in mount order.
    pub fn generation_services(&self, provider: &str, generation: &str) -> Vec<Arc<Service>> {
        self.services
            .lock()
            .unwrap()
            .iter()
            .filter(|mount| {
                mount.service.provider_id == provider && mount.service.generation_id == generation
            })
            .map(|mount| mount.service.clone())
            .collect()
    }
    pub fn plugin_ids(&self) -> Vec<String> {
        self.services
            .lock()
            .unwrap()
            .iter()
            .map(|mount| mount.service.key.clone())
            .collect()
    }
    pub fn service_keys(&self) -> Vec<String> {
        let mut keys = self.plugin_ids();
        keys.sort();
        keys
    }
    pub fn resolve(&self, provider: &str, service: &str, generation: &str) -> Option<Arc<Service>> {
        let wanted = key(provider, service, generation);
        self.services
            .lock()
            .unwrap()
            .iter()
            .find(|mount| mount.service.key == wanted)
            .map(|mount| mount.service.clone())
    }
    pub fn generation_adapter(&self, provider: &str, generation: &str) -> Option<Arc<Adapter>> {
        let prefix = service_key(provider, "");
        let suffix = format!("@{}", generation.trim());
        self.services
            .lock()
            .unwrap()
            .iter()
            .filter(|mount| {
                mount.service.key.starts_with(&prefix) && mount.service.key.ends_with(&suffix)
            })
            .min_by_key(|mount| &mount.service.key)
            .and_then(|mount| mount.service.adapter().cloned())
    }
    pub async fn unmount_ids(&self, ids: &[String]) {
        // Go disposal uses Background and adapter Close ignores cancellation.
        // An owning cleanup task must finish even if its async caller is dropped.
        let services = self.services.clone();
        let ids = ids.to_vec();
        tokio::spawn(async move {
            for id in ids.iter().rev() {
                let owner = {
                    let mut services = services.lock().unwrap();
                    let Some(mount) = services.iter_mut().find(|mount| mount.service.key == *id)
                    else {
                        continue;
                    };
                    if mount.disposed {
                        continue;
                    }
                    mount.disposed = true;
                    mount
                        .owns_adapter
                        .then(|| mount.service.adapter().cloned())
                        .flatten()
                };
                if let Some(adapter) = owner {
                    adapter.close().await;
                }
                services
                    .lock()
                    .unwrap()
                    .retain(|mount| mount.service.key != *id);
            }
        })
        .await
        .expect("provider service cleanup task panicked");
    }
    pub async fn unmount_generation(&self, provider: &str, generation: &str) {
        let prefix = service_key(provider, "");
        let suffix = format!("@{}", generation.trim());
        let ids = self
            .plugin_ids()
            .into_iter()
            .filter(|id| id.starts_with(&prefix) && id.ends_with(&suffix))
            .collect::<Vec<_>>();
        self.unmount_ids(&ids).await;
    }
    pub async fn displace(&self, owner: &str, families: &[String]) {
        let owner = owner.trim();
        if owner.is_empty() {
            return;
        }
        let ids = self
            .services
            .lock()
            .unwrap()
            .iter()
            .filter(|mount| {
                mount.service.provider_id.trim() != owner
                    && families.iter().any(|family| {
                        !family.trim().is_empty()
                            && family.trim() == mount.service.capability_id.trim()
                    })
            })
            .map(|mount| mount.service.key.clone())
            .collect::<Vec<_>>();
        self.unmount_ids(&ids).await;
    }
    /// Dispose every provider generation's services. The built-in
    /// generation belongs to the composition root, which disposes it once
    /// accepted work has drained.
    pub async fn close(&self) {
        let ids = self
            .services
            .lock()
            .unwrap()
            .iter()
            .filter(|mount| mount.service.generation_id != crate::builtin::GENERATION)
            .map(|mount| mount.service.key.clone())
            .collect::<Vec<_>>();
        self.unmount_ids(&ids).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        plan::runner::RunCtx,
        provider::{DrainPolicy, Generation},
    };
    use serde_json::{json, Value};

    #[tokio::test(flavor = "current_thread")]
    async fn cancelled_teardown_keeps_its_owner_until_accepted_work_finishes() {
        use super::super::{definition::GoSlice, ProviderRef};
        use std::{
            collections::BTreeSet,
            io::{Read, Write},
            time::Duration,
        };
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let provider = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut buffer = [0; 4096];
            assert!(socket.read(&mut buffer).unwrap() > 0);
            started_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(2)).unwrap();
            let body = r#"{"jsonrpc":"2.0","id":1,"result":{"content":[]}}"#;
            write!(
                socket,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .unwrap();
        });
        let adapter = Arc::new(Adapter::fixture_client(
            format!("http://{address}/mcp"),
            BTreeSet::from(["test.operation".into()]),
        ));
        let mounts = Mounts::new(Lifecycle::new(DrainPolicy::default()));
        let manifest = InstallManifest {
            provider: ProviderRef {
                id: "test.provider".into(),
                version: "v1".into(),
            },
            services: GoSlice(Some(vec![
                ServiceDefinition {
                    id: "base".into(),
                    capability_id: "test.base".into(),
                    ..ServiceDefinition::default()
                },
                ServiceDefinition {
                    id: "consumer".into(),
                    capability_id: "test.consumer".into(),
                    dependencies: GoSlice(Some(vec![CapabilityRef {
                        id: "test.base".into(),
                        version: 1,
                    }])),
                    ..ServiceDefinition::default()
                },
            ])),
            ..InstallManifest::default()
        };
        mounts
            .mount(
                &manifest,
                "g1",
                Some(Implementation::Adapter(adapter.clone())),
            )
            .await
            .unwrap();
        let accepted = mounts.resolve("test.provider", "consumer", "g1").unwrap();
        let calling = accepted.clone();
        let call = tokio::spawn(async move {
            calling
                .adapter()
                .unwrap()
                .call(&RunCtx::background(), "test.operation", None, "")
                .await
        });
        tokio::time::timeout(Duration::from_secs(2), started_rx)
            .await
            .unwrap()
            .unwrap();
        assert!(tokio::time::timeout(
            Duration::from_millis(20),
            mounts.unmount_generation("test.provider", "g1")
        )
        .await
        .is_err());
        assert!(mounts.generation_adapter("test.provider", "g1").is_some());
        release_tx.send(()).unwrap();
        assert_eq!(
            call.await.unwrap().unwrap(),
            json!({"content":[]}).as_object().unwrap().clone()
        );
        tokio::time::timeout(Duration::from_secs(2), async {
            while !mounts.plugin_ids().is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(mounts.service_keys().is_empty());
        assert!(mounts.generation_adapter("test.provider", "g1").is_none());
        assert_eq!(
            accepted
                .adapter()
                .unwrap()
                .call(&RunCtx::background(), "test.operation", None, "")
                .await
                .unwrap_err(),
            "provider adapter is closed"
        );
        mounts.close().await;
        provider.join().unwrap();
    }

    #[tokio::test]
    #[ignore = "requires the owning Go service-mount differential driver"]
    async fn mounts_fixture() {
        let input: Value = serde_json::from_slice(
            &std::fs::read(std::env::var("PARITY_PROVIDER_MOUNTS_INPUT").unwrap()).unwrap(),
        )
        .unwrap();
        let mut outputs = Vec::new();
        for row in input.as_array().unwrap() {
            let lifecycle = Lifecycle::new(DrainPolicy::default());
            let mounts = Mounts::new(lifecycle.clone());
            let candidates = super::super::candidates::Candidates::default();
            let mut adapters: BTreeMap<String, Arc<Adapter>> = BTreeMap::new();
            let mut observations = Vec::new();
            for step in row["mounts"]["steps"].as_array().unwrap() {
                let text = |key: &str| step[key].as_str().unwrap_or_default();
                let strings = |key: &str| {
                    step[key]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|value| value.as_str().unwrap().to_string())
                        .collect::<Vec<_>>()
                };
                let mut result = serde_json::Map::new();
                let outcome = match text("action") {
                    "adapter" => {
                        adapters.insert(
                            text("adapter").into(),
                            Arc::new(Adapter::unconnected_fixture()),
                        );
                        Ok(())
                    }
                    "restore" => lifecycle.restore(
                        serde_json::from_value::<Generation>(step["restore"].clone()).unwrap(),
                    ),
                    "candidateAdapter" => {
                        candidates.set_adapter(
                            text("generation"),
                            adapters.get(text("adapter")).cloned(),
                        );
                        Ok(())
                    }
                    "invoke" => {
                        let raw = serde_json::to_vec(&serde_json::json!({"provider":{"id":text("provider")},"services":[{"id":text("service"),"operations":[step["operation"].clone()]}]})).unwrap();
                        let manifest = super::super::definition::decode_manifest(
                            &crate::gojson::parse(&raw).unwrap(),
                        )
                        .unwrap();
                        let operation = super::super::binding::operations(
                            &manifest,
                            text("generation"),
                            step["candidate"].as_bool().unwrap_or(false),
                        )
                        .remove(0);
                        let (tool_error, error) =
                            match operation.accept(&lifecycle, &candidates, &mounts) {
                                Err(error) => (true, error),
                                Ok(accepted) => (
                                    false,
                                    accepted
                                        .call(&RunCtx::background(), None, "")
                                        .await
                                        .err()
                                        .unwrap_or_default(),
                                ),
                            };
                        result.insert("toolError".into(), json!(tool_error));
                        result.insert("invokeError".into(), json!(error));
                        let mut sessions = BTreeMap::new();
                        for id in [text("generation"), "g1", "g2"] {
                            if let Some(observed) = lifecycle.get(id) {
                                sessions.insert(id, observed.sessions);
                            }
                        }
                        result.insert("sessions".into(), json!(sessions));
                        Ok(())
                    }
                    "ready" => lifecycle.mark_ready(text("generation")),
                    "activate" => lifecycle.activate(text("generation")).map(|_| ()),
                    "mount" => {
                        let raw = serde_json::to_vec(&step["manifest"]).unwrap();
                        let manifest = super::super::definition::decode_manifest(
                            &crate::gojson::parse(&raw).unwrap(),
                        )
                        .unwrap();
                        mounts
                            .mount(
                                &manifest,
                                text("generation"),
                                adapters
                                    .get(text("adapter"))
                                    .cloned()
                                    .map(Implementation::Adapter),
                            )
                            .await
                    }
                    "unmount" => {
                        mounts
                            .unmount_generation(text("provider"), text("generation"))
                            .await;
                        Ok(())
                    }
                    "ids" => {
                        mounts.unmount_ids(&strings("ids")).await;
                        Ok(())
                    }
                    "displace" => {
                        mounts
                            .displace(text("provider"), &strings("families"))
                            .await;
                        Ok(())
                    }
                    "close" => {
                        mounts.close().await;
                        Ok(())
                    }
                    "lookup" => {
                        let service =
                            mounts.resolve(text("provider"), text("service"), text("generation"));
                        result.insert("found".into(), json!(service.is_some()));
                        if let Some(service) = service {
                            let inject = if service.inject.is_empty() {
                                Value::Null
                            } else {
                                json!(service.inject)
                            };
                            result.insert("service".into(), json!({"key":service.key,"provider":service.provider_id,"generation":service.generation_id,"capability":service.capability_id,"offering":service.offering_key(),"inject":inject}));
                        }
                        result.insert(
                            "generationAdapter".into(),
                            json!(mounts
                                .generation_adapter(text("provider"), text("generation"))
                                .is_some()),
                        );
                        Ok(())
                    }
                    "families" => Ok(()),
                    other => panic!("unsupported mount fixture action {other}"),
                };
                let mut probes = BTreeMap::new();
                for (id, adapter) in &adapters {
                    probes.insert(
                        id.clone(),
                        adapter
                            .call(&RunCtx::background(), "probe", None, "")
                            .await
                            .unwrap_err(),
                    );
                }
                result.insert("error".into(), json!(outcome.err().unwrap_or_default()));
                result.insert("plugins".into(), json!(mounts.plugin_ids()));
                result.insert("services".into(), json!(mounts.service_keys()));
                result.insert(
                    "families".into(),
                    json!(mounts.injectable_families(text("generation"))),
                );
                result.insert("adapters".into(), json!(probes));
                observations.push(Value::Object(result));
            }
            mounts.close().await;
            outputs.push(json!({"observations":observations}));
        }
        std::fs::write(
            std::env::var("PARITY_PROVIDER_MOUNTS_OUTPUT").unwrap(),
            serde_json::to_vec_pretty(&outputs).unwrap(),
        )
        .unwrap();
    }
}
