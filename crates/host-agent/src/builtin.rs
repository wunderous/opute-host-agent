//! The built-in provider (decision D14, `three-role-capability-seam`).
//!
//! Built-in and provider-supplied capabilities follow one pattern. Each has a
//! Service Definition (the provider manifest model, here built from the one
//! descriptor source), a Service Provider mounted in the one service context
//! (`provider::mounts`), and a Consumer. They are registered, resolved and
//! invoked the same way. They differ only in distribution and trust: the
//! built-in provider is compiled in and mounted by the composition root
//! instead of being installed, and runs in process instead of behind an MCP
//! adapter. Go-observable per-capability contracts (the lifecycle route, the
//! declarative result gate and audit row) are kept for parity, not as a
//! second architecture.
//!
//! The provider identity and its capability families use `host-agent:`, which
//! is not a valid manifest identifier. No provider can depend on, publish
//! into or displace a built-in family, so provider behaviour matches Go,
//! whose built-in tools sit outside its service context.
//!
//! Each operation captures only the seams its service is built with
//! (legacy item I-12); nothing receives the whole host through its call.

use crate::admission::Binding;
use crate::catalog::{self, Descriptor, Snapshot};
use crate::goerr::quote;
use crate::plan::runner::RunCtx;
use crate::provider::definition::{
    CapabilityRef, GoSlice, InstallManifest, Operation as Definition, ServiceDefinition,
};
use crate::provider::mounts::{Implementation, Mounts};
use crate::provider::ProviderRef;
use crate::transport::Server;
use serde_json::{Map, Value as J};
use std::collections::BTreeMap;
use std::sync::{atomic::AtomicBool, Arc, OnceLock, Weak};

/// The generation the built-in provider is mounted under. The composition
/// root owns it; provider generations are owned by the provider runtime.
pub const GENERATION: &str = "builtin";

/// The built-in provider's identity: the `implementation` Go publishes on
/// every built-in descriptor.
pub fn provider_id() -> String {
    catalog::builtin_implementation()
}

pub fn capability_id(service: &str) -> String {
    format!("{}.{service}", provider_id())
}

/// One admitted call, as the dispatch boundary hands it to an implementation.
pub struct Invocation<'a> {
    pub ctx: &'a RunCtx,
    pub args: &'a Map<String, J>,
    pub binding: &'a Binding,
    /// The catalog the call was admitted against.
    pub snapshot: &'a Snapshot,
    /// Set by a run that takes ownership of the admitted reservation.
    pub claimed: Option<&'a Arc<AtomicBool>>,
}

impl Invocation<'_> {
    pub fn reservation(&self) -> crate::resource::Reservation {
        self.ctx
            .reservation()
            .map(|reservation| (*reservation).clone())
            .expect("an admitted invocation carries its reservation")
    }
    pub fn claimed(&self) -> Arc<AtomicBool> {
        Arc::clone(
            self.claimed
                .expect("a lease-owning run is told how to claim"),
        )
    }
    pub fn lease(&self) -> crate::provider_mcp::Lease {
        crate::provider_mcp::Lease {
            reservation: self.reservation(),
            claimed: self.claimed(),
        }
    }
}

pub type Handler = Arc<dyn Fn(&Invocation) -> Result<J, String> + Send + Sync>;

/// One built-in service: its operations, and the built-in services it
/// depends on.
pub struct Service {
    pub id: &'static str,
    pub dependencies: &'static [&'static str],
    pub operations: Vec<(&'static str, Handler)>,
}

/// The in-process implementation of the built-in provider's generation, as
/// an MCP adapter is the implementation of a provider generation.
pub struct Provider {
    operations: BTreeMap<&'static str, Handler>,
}

impl Provider {
    pub fn call(&self, operation: &str, invocation: &Invocation) -> Option<Result<J, String>> {
        self.operations
            .get(operation)
            .map(|handler| handler(invocation))
    }
}

fn operation_definition(descriptor: &Descriptor) -> Definition {
    Definition {
        id: descriptor.name.clone(),
        version: descriptor.version,
        description: descriptor.description.clone(),
        input_schema: descriptor.input_schema.as_object().cloned(),
        output_schema: descriptor
            .output_schema
            .as_ref()
            .and_then(J::as_object)
            .cloned(),
        effect: descriptor.effect.clone(),
        idempotent: descriptor.idempotent,
        supports_readiness: descriptor.supports_readiness,
        ..Definition::default()
    }
}

/// The built-in provider's manifest. Every operation's definition comes from
/// the descriptor source; one it does not define is refused.
pub fn manifest(services: &[Service]) -> Result<InstallManifest, String> {
    let mut definitions = Vec::new();
    for service in services {
        let mut operations = Vec::new();
        for (name, _) in &service.operations {
            let descriptor = catalog::source_descriptor(name).ok_or_else(|| {
                format!(
                    "built-in operation {} has no Service Definition in the descriptor source",
                    quote(name)
                )
            })?;
            operations.push(operation_definition(&descriptor));
        }
        definitions.push(ServiceDefinition {
            id: service.id.into(),
            capability_id: capability_id(service.id),
            version: 1,
            operations: GoSlice(Some(operations)),
            dependencies: GoSlice(Some(
                service
                    .dependencies
                    .iter()
                    .map(|id| CapabilityRef {
                        id: capability_id(id),
                        version: 1,
                    })
                    .collect(),
            )),
        });
    }
    Ok(InstallManifest {
        provider: ProviderRef {
            id: provider_id(),
            version: env!("CARGO_PKG_VERSION").into(),
        },
        services: GoSlice(Some(definitions)),
        ..InstallManifest::default()
    })
}

/// Mount the built-in provider's generation through the same registration
/// as a provider generation. Services are mounted in the given order, which
/// is their dependency order; a failure disposes what was mounted.
pub fn mount(mounts: &Mounts, services: Vec<Service>) -> Result<(), String> {
    let manifest = manifest(&services)?;
    let provider = Provider {
        operations: services
            .into_iter()
            .flat_map(|service| service.operations)
            .collect(),
    };
    mounts.mount_now(
        &manifest,
        GENERATION,
        Implementation::Builtin(Arc::new(provider)),
    )
}

/// Dispose the built-in generation's services, in reverse mount order.
pub fn dispose(mounts: &Mounts) {
    let ids: Vec<String> = mounts
        .generation_services(&provider_id(), GENERATION)
        .iter()
        .map(|service| service.key.clone())
        .collect();
    mounts.unmount_now(&ids);
}

/// The late-bound server handle lifecycle services capture: the pipeline
/// owners they drive exist only once the server is assembled.
#[derive(Clone, Default)]
pub struct ServerHandle(Arc<OnceLock<Weak<Server>>>);

impl ServerHandle {
    pub fn bind(&self, server: &Arc<Server>) {
        let _ = self.0.set(Arc::downgrade(server));
    }
    pub fn get(&self) -> Result<Arc<Server>, String> {
        self.0
            .get()
            .and_then(Weak::upgrade)
            .ok_or_else(|| "host agent is unavailable".to_string())
    }
}

/// The host's built-in services, in dependency order, each built from the
/// seams it declares.
pub fn services(_host: &crate::tools::Host, _server: &ServerHandle) -> Vec<Service> {
    Vec::new()
}

/// The mounted built-in service that implements `name`, if any.
pub fn resolve(server: &Server, name: &str) -> Option<Arc<crate::provider::mounts::Service>> {
    server
        .providers
        .activation
        .mounts
        .resolve_operation(&provider_id(), GENERATION, name)
}

/// Invoke `name` through the service context. `None` when no mounted
/// service implements it.
pub fn invoke(server: &Server, name: &str, invocation: &Invocation) -> Option<Result<J, String>> {
    let service = resolve(server, name)?;
    match &service.implementation {
        Implementation::Builtin(provider) => provider.call(name, invocation),
        Implementation::Adapter(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{DrainPolicy, Lifecycle};

    fn handler() -> Handler {
        Arc::new(|_| Ok(J::Null))
    }

    fn service(id: &'static str, deps: &'static [&'static str], ops: &[&'static str]) -> Service {
        Service {
            id,
            dependencies: deps,
            operations: ops.iter().map(|op| (*op, handler())).collect(),
        }
    }

    fn mounts() -> Mounts {
        Mounts::new(Lifecycle::new(DrainPolicy::default()))
    }

    #[test]
    fn an_operation_without_a_definition_is_refused_and_nothing_stays_mounted() {
        let mounts = mounts();
        let error = mount(
            &mounts,
            vec![
                service("host-info", &[], &["get_host_info"]),
                service("unknown", &[], &["not_in_the_descriptor_source"]),
            ],
        )
        .unwrap_err();
        assert!(error.contains("has no Service Definition"), "{error}");
        assert!(mounts
            .generation_services(&provider_id(), GENERATION)
            .is_empty());
    }

    #[test]
    fn a_failed_mount_disposes_what_was_mounted() {
        let mounts = mounts();
        let error = mount(
            &mounts,
            vec![
                service("host-info", &[], &["get_host_info"]),
                service("plans", &["missing"], &["run_host_plan"]),
            ],
        )
        .unwrap_err();
        assert!(
            error.contains("is not provided by any mounted service"),
            "{error}"
        );
        assert!(mounts
            .generation_services(&provider_id(), GENERATION)
            .is_empty());
    }

    #[test]
    fn dependencies_resolve_in_mount_order_and_disposal_reverses_it() {
        let mounts = mounts();
        mount(
            &mounts,
            vec![
                service("plans", &[], &["run_host_plan"]),
                service("providers", &["plans"], &["opute.provider.install"]),
            ],
        )
        .unwrap();
        let mounted = mounts.generation_services(&provider_id(), GENERATION);
        assert_eq!(mounted[1].inject, vec![mounted[0].key.clone()]);
        assert_eq!(mounted[0].capability_id, "host-agent:incus.plans");
        dispose(&mounts);
        assert!(mounts
            .generation_services(&provider_id(), GENERATION)
            .is_empty());
    }

    /// A provider manifest cannot name a built-in family: `host-agent:` is
    /// not a valid manifest identifier, so validation refuses it before any
    /// family, injection or displacement rule runs.
    #[test]
    fn provider_manifests_cannot_name_built_in_families() {
        use crate::provider::definition::validate_install_manifest;
        let provider = ProviderRef {
            id: "com.example.provider".into(),
            version: "1".into(),
        };
        let manifest = |capability: String, dependency: Option<String>| {
            let mut doc = InstallManifest {
                schema: "opute-provider-install-manifest.v1".into(),
                provider: provider.clone(),
                provides: GoSlice(Some(vec![CapabilityRef {
                    id: "com.example.cap".into(),
                    version: 1,
                }])),
                services: GoSlice(Some(vec![ServiceDefinition {
                    id: "svc".into(),
                    capability_id: capability,
                    version: 1,
                    dependencies: GoSlice(Some(
                        dependency
                            .map(|id| vec![CapabilityRef { id, version: 1 }])
                            .unwrap_or_default(),
                    )),
                    ..ServiceDefinition::default()
                }])),
                ..InstallManifest::default()
            };
            doc.validation.capability = "com.example.cap".into();
            doc.validation.operation = "com.example.cap.validate".into();
            doc
        };
        let publishes =
            validate_install_manifest(&manifest(capability_id("host-info"), None), &provider);
        assert!(publishes
            .unwrap_err()
            .contains("must declare a neutral capabilityId"));
        let depends = validate_install_manifest(
            &manifest("com.example.cap".into(), Some(capability_id("host-info"))),
            &provider,
        );
        assert!(depends
            .unwrap_err()
            .contains("declares an invalid dependency"));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn providers_cannot_reach_built_in_families_and_close_leaves_them() {
        let mounts = mounts();
        mount(&mounts, vec![service("host-info", &[], &["get_host_info"])]).unwrap();
        // A provider generation's injection sees only generations in the
        // provider lifecycle; the built-in generation is not one.
        assert!(mounts.injectable_families("g1").is_empty());
        // The provider runtime's close leaves the composition root's
        // generation; the composition root disposes it.
        mounts.close().await;
        assert!(mounts
            .resolve_operation(&provider_id(), GENERATION, "get_host_info")
            .is_some());
        dispose(&mounts);
        assert!(mounts
            .resolve_operation(&provider_id(), GENERATION, "get_host_info")
            .is_none());
    }
}
