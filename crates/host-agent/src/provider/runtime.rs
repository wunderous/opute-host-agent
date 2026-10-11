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
    catalog::registry::Capability, plan::runner::RunCtx, store::StateStore, transport::Server,
};
use serde_json::{json, Map, Value};
use std::sync::{Arc, Mutex, OnceLock, Weak};

pub struct Runtime {
    pub activation: Arc<Activation>,
    pub delegations: Delegations,
    server: OnceLock<Weak<Server>>,
    io: OnceLock<tokio::runtime::Handle>,
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
                server: OnceLock::new(),
                io: OnceLock::new(),
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
        let server = Arc::new(crate::app::http_server(&config, auth, state).unwrap());
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
            let server = Arc::new(crate::app::http_server(&config, auth, state).unwrap());
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
