//! Executable generation binding from hostmcp.newProviderOperationCapability.
//! The composition owner must issue resource delegation before accepting work.
//! This private boundary does not admit, publish, or authorize an invocation.
use super::{
    adapter::Adapter, candidates::Candidates, definition::InstallManifest, mounts::Mounts,
    Lifecycle, Session,
};
use crate::{
    catalog::{provider_descriptors, Descriptor},
    goerr::quote,
    plan::runner::RunCtx,
};
use serde_json::{Map, Value};
use std::sync::Arc;

#[derive(Clone)]
pub struct Operation {
    descriptor: Descriptor,
    service_id: String,
    candidate: bool,
}

pub fn operations(manifest: &InstallManifest, generation: &str, candidate: bool) -> Vec<Operation> {
    provider_descriptors(manifest, generation)
        .into_iter()
        .zip(
            manifest
                .services
                .iter()
                .flat_map(|service| service.operations.iter().map(move |_| service.id.clone())),
        )
        .map(|(descriptor, service_id)| Operation {
            descriptor,
            service_id,
            candidate,
        })
        .collect()
}

/// Retains the accepted generation and adapter across replacement and awaits.
/// Cancellation drops the session; it never resolves a replacement adapter.
pub struct Accepted {
    _session: Session,
    adapter: Arc<Adapter>,
    operation_id: String,
}
impl Operation {
    pub fn validate_result(
        &self,
        result: Option<&Map<String, Value>>,
    ) -> Result<super::observation::Observation, String> {
        super::observation::validate(&self.descriptor, result)
    }
    pub fn descriptor(&self) -> &Descriptor {
        &self.descriptor
    }
    pub fn accept(
        &self,
        lifecycle: &Lifecycle,
        candidates: &Candidates,
        mounts: &Mounts,
    ) -> Result<Accepted, String> {
        let generation = &self.descriptor.generation_id;
        let provider = &self.descriptor.provider;
        let session = if self.candidate {
            lifecycle.open_session_for_generation(provider, generation)?
        } else {
            match lifecycle.open_session(provider) {
                Ok(session) if session.generation_id() == generation => session,
                _ => {
                    return Err(format!(
                        "provider generation {} is no longer active",
                        quote(generation)
                    ))
                }
            }
        };
        let adapter = if self.candidate {
            candidates.adapter(generation)
        } else {
            mounts
                .resolve(provider, &self.service_id, session.generation_id())
                .map(|service| service.adapter.clone())
        }
        .ok_or_else(|| format!("provider generation {} is not connected", quote(generation)))?;
        Ok(Accepted {
            _session: session,
            adapter,
            operation_id: self.descriptor.operation_id.clone(),
        })
    }
}
impl Accepted {
    pub async fn call(
        self,
        ctx: &RunCtx,
        arguments: Option<Map<String, Value>>,
        delegation: &str,
    ) -> Result<Map<String, Value>, String> {
        self.adapter
            .call_synchronous_only(ctx, &self.operation_id, arguments, delegation)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{
        definition::{GoSlice, Operation as Definition, ServiceDefinition},
        DrainPolicy, Generation, GenerationState, ProviderRef,
    };
    use serde_json::json;
    use std::{
        collections::BTreeSet,
        io::{Read, Write},
        time::Duration,
    };

    fn manifest() -> InstallManifest {
        InstallManifest {
            provider: ProviderRef {
                id: "test.provider".into(),
                version: "v1".into(),
            },
            services: GoSlice(Some(vec![ServiceDefinition {
                id: "base".into(),
                capability_id: "test.base".into(),
                operations: GoSlice(Some(vec![Definition {
                    id: "probe".into(),
                    version: 1,
                    ..Definition::default()
                }])),
                ..ServiceDefinition::default()
            }])),
            ..InstallManifest::default()
        }
    }
    fn restore(lifecycle: &Lifecycle, id: &str, state: GenerationState) {
        lifecycle
            .restore(Generation {
                id: id.into(),
                provider: manifest().provider,
                manifest_hash: "hash".into(),
                endpoint: "http://127.0.0.1:1".into(),
                state,
                ..Generation::default()
            })
            .unwrap();
    }
    fn server(
        generation: &'static str,
    ) -> (
        Arc<Adapter>,
        tokio::sync::oneshot::Receiver<()>,
        std::sync::mpsc::Sender<()>,
        std::thread::JoinHandle<()>,
    ) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/mcp", listener.local_addr().unwrap());
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut bytes = Vec::new();
            let (header_end, length) = loop {
                let mut buffer = [0; 4096];
                let count = socket.read(&mut buffer).unwrap();
                assert_ne!(count, 0);
                bytes.extend_from_slice(&buffer[..count]);
                if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&bytes[..end]);
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            line.to_lowercase()
                                .strip_prefix("content-length:")
                                .map(|value| value.trim().parse::<usize>().unwrap())
                        })
                        .unwrap();
                    break (end + 4, length);
                }
            };
            while bytes.len() < header_end + length {
                let mut buffer = [0; 4096];
                let count = socket.read(&mut buffer).unwrap();
                assert_ne!(count, 0);
                bytes.extend_from_slice(&buffer[..count]);
            }
            let request: Value =
                serde_json::from_slice(&bytes[header_end..header_end + length]).unwrap();
            assert_eq!(request["params"]["name"], "probe");
            assert_eq!(request["params"]["arguments"], json!({"untouched":"雪"}));
            started_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            let body = serde_json::to_string(&json!({"jsonrpc":"2.0","id":request["id"],"result":{"content":[],"structuredContent":{"generation":generation}}})).unwrap();
            // Cancellation can close the peer before the response is written.
            let _ = write!(
                socket,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
        });
        (
            Arc::new(Adapter::fixture_client(
                endpoint,
                BTreeSet::from(["probe".into()]),
            )),
            started_rx,
            release_tx,
            thread,
        )
    }
    fn args() -> Option<Map<String, Value>> {
        Some(json!({"untouched":"雪"}).as_object().unwrap().clone())
    }

    #[tokio::test]
    async fn accepted_call_finishes_on_predecessor_while_new_calls_use_replacement() {
        let lifecycle = Lifecycle::new(DrainPolicy::default());
        let candidates = Candidates::default();
        let mounts = Mounts::new(lifecycle.clone());
        let (first, started, release, first_server) = server("g1");
        restore(&lifecycle, "g1", GenerationState::Active);
        mounts.mount(&manifest(), "g1", Some(first)).await.unwrap();
        let old = operations(&manifest(), "g1", false).remove(0);
        let accepted = old.accept(&lifecycle, &candidates, &mounts).unwrap();
        let call =
            tokio::spawn(async move { accepted.call(&RunCtx::background(), args(), "").await });
        tokio::time::timeout(Duration::from_secs(5), started)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(lifecycle.get("g1").unwrap().sessions, 1);
        let (second, started, release_second, second_server) = server("g2");
        restore(&lifecycle, "g2", GenerationState::Candidate);
        lifecycle.mark_ready("g2").unwrap();
        mounts.mount(&manifest(), "g2", Some(second)).await.unwrap();
        lifecycle.activate("g2").unwrap();
        assert_eq!(
            lifecycle.get("g1").unwrap().state,
            GenerationState::Draining
        );
        assert_eq!(
            old.accept(&lifecycle, &candidates, &mounts).err().unwrap(),
            "provider generation \"g1\" is no longer active"
        );
        assert_eq!(lifecycle.get("g2").unwrap().sessions, 0);
        let new = operations(&manifest(), "g2", false)
            .remove(0)
            .accept(&lifecycle, &candidates, &mounts)
            .unwrap();
        let second_call =
            tokio::spawn(async move { new.call(&RunCtx::background(), args(), "").await });
        tokio::time::timeout(Duration::from_secs(5), started)
            .await
            .unwrap()
            .unwrap();
        release_second.send(()).unwrap();
        assert_eq!(
            second_call.await.unwrap().unwrap()["structuredContent"]["generation"],
            "g2"
        );
        assert!(tokio::time::timeout(
            Duration::from_millis(20),
            mounts.unmount_generation("test.provider", "g1")
        )
        .await
        .is_err());
        release.send(()).unwrap();
        assert_eq!(
            call.await.unwrap().unwrap()["structuredContent"]["generation"],
            "g1"
        );
        assert_eq!(lifecycle.get("g1").unwrap().sessions, 0);
        tokio::time::timeout(Duration::from_secs(5), async {
            while mounts.resolve("test.provider", "base", "g1").is_some() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(mounts.resolve("test.provider", "base", "g2").is_some());
        mounts.close().await;
        assert!(mounts.plugin_ids().is_empty());
        first_server.join().unwrap();
        second_server.join().unwrap();
    }

    #[tokio::test]
    async fn cancelling_accepted_http_call_releases_session_and_allows_disposal() {
        let lifecycle = Lifecycle::new(DrainPolicy::default());
        let candidates = Candidates::default();
        let mounts = Mounts::new(lifecycle.clone());
        let (adapter, started, release, provider) = server("g1");
        restore(&lifecycle, "g1", GenerationState::Active);
        mounts
            .mount(&manifest(), "g1", Some(adapter.clone()))
            .await
            .unwrap();
        let accepted = operations(&manifest(), "g1", false)
            .remove(0)
            .accept(&lifecycle, &candidates, &mounts)
            .unwrap();
        let call =
            tokio::spawn(async move { accepted.call(&RunCtx::background(), args(), "").await });
        tokio::time::timeout(Duration::from_secs(5), started)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(lifecycle.get("g1").unwrap().sessions, 1);
        call.abort();
        assert!(call.await.unwrap_err().is_cancelled());
        assert_eq!(lifecycle.get("g1").unwrap().sessions, 0);
        tokio::time::timeout(Duration::from_secs(5), mounts.close())
            .await
            .unwrap();
        assert!(mounts.plugin_ids().is_empty());
        assert_eq!(
            adapter
                .call(&RunCtx::background(), "probe", args(), "")
                .await
                .unwrap_err(),
            "provider adapter is closed"
        );
        release.send(()).unwrap();
        provider.join().unwrap();
    }
}
