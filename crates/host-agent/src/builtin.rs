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

fn op(run: impl Fn(&Invocation) -> Result<J, String> + Send + Sync + 'static) -> Handler {
    Arc::new(run)
}

fn flag(args: &Map<String, J>, key: &str) -> bool {
    args.get(key).and_then(J::as_bool).unwrap_or(false)
}

/// `intField`: a JSON number truncated toward zero; anything else is 0.
fn int_field(args: &Map<String, J>, key: &str) -> i64 {
    args.get(key)
        .and_then(J::as_f64)
        .map(|v| v as i64)
        .unwrap_or(0)
}

/// `rawStringField`: the string as sent, untrimmed.
fn raw_text<'a>(args: &'a Map<String, J>, key: &str) -> &'a str {
    args.get(key).and_then(J::as_str).unwrap_or("")
}

fn text<'a>(args: &'a Map<String, J>, key: &str) -> &'a str {
    args.get(key)
        .and_then(J::as_str)
        .map(str::trim)
        .unwrap_or("")
}

/// The host's built-in services, in dependency order, each built from the
/// seams it declares.
pub fn services(host: &crate::tools::Host, server: &ServerHandle) -> Vec<Service> {
    let k8s = kubernetes(host, server);
    let adopter = k8s.inventory();
    let _ = host
        .cluster_adopter
        .set(Arc::new(move |uri: &crate::resource::Uri| {
            adopter.adopt_cluster(uri)
        }));
    let oci = oci(host, &k8s);
    let postgres = postgres(host, &k8s);
    let agent = crate::clusterops::ClusterAgent {
        instances: instances(host),
        env: host.env.clone(),
        relays: host.relays.clone(),
    };
    vec![
        host_info(host),
        incus_inventory(host),
        host_observation(host),
        host_files(host),
        host_archives(host),
        host_tools(host),
        host_exec(host),
        host_services(host),
        bridge(host),
        agent_connection(host),
        host_firewall(),
        container_engine(),
        virtualization_stack(host),
        system_containers(host),
        instance_lifecycle(host),
        local_status(host),
        instance_exec(host),
        cluster_discovery(&k8s),
        cluster_resources(&k8s),
        cluster_listings(&k8s),
        cluster_packages(&k8s),
        service_exposure(&k8s),
        guest_storage(&k8s),
        cluster_agent(&agent),
        oci_registry(&oci),
        oci_builder(&oci),
        build_context(&oci),
        container_storage(&oci),
        image_build(&oci),
        sqlite_databases(&postgres),
        postgresql_service(&postgres),
        postgresql_relay(&postgres),
        sql_connector(&postgres, host),
        standalone_postgresql(&postgres),
        host_artifacts(host),
        public_mcp(host),
        host_vm_subsystem(host),
        catalog_service(),
        host_resources(host),
        tasks(server),
        plans(server),
        host_recipes(server),
        runtime_recipes(server),
        tunnel_recipes(server),
        providers(server),
    ]
}

/// M7: the public provider lifecycle, through the activation owner and the
/// single plan executor.
fn providers(server: &ServerHandle) -> Service {
    use crate::provider_mcp::{handle_install, handle_reload, handle_teardown};
    Service {
        id: "providers",
        dependencies: &[
            "host-observation",
            "plans",
            "runtime-recipes",
            "tunnel-recipes",
        ],
        operations: vec![
            (
                "opute.provider.install",
                lifecycle(server, |server, call| {
                    handle_install(server, call.args, call.lease())
                }),
            ),
            (
                "opute.provider.validate",
                lifecycle(server, |server, call| {
                    server.providers.validate(&RunCtx::background(), call.args)
                }),
            ),
            (
                "opute.provider.status",
                lifecycle(server, |server, call| server.providers.status(call.args)),
            ),
            (
                "opute.provider.reload",
                lifecycle(server, |server, call| {
                    handle_reload(server, call.args, call.lease())
                }),
            ),
            (
                "opute.provider.teardown",
                lifecycle(server, |server, call| {
                    handle_teardown(server, call.args, call.lease())
                }),
            ),
        ],
    }
}

/// A lifecycle operation: owned by the transport pipeline, as Go's
/// `isLifecycleTool` tools are, it drives the server's plan, task and
/// provider owners through the late-bound handle.
fn lifecycle(server: &ServerHandle, run: fn(&Arc<Server>, &Invocation) -> J) -> Handler {
    let server = server.clone();
    op(move |call| Ok(run(&server.get()?, call)))
}

/// M6: host plans through the single plan executor.
fn plans(server: &ServerHandle) -> Service {
    use crate::plan_mcp::{
        handle_get_host_plan_run, handle_run_host_plan, handle_validate_host_plan,
    };
    Service {
        id: "plans",
        dependencies: &[],
        operations: vec![
            (
                "validate_host_plan",
                op(|call| Ok(handle_validate_host_plan(call.args, call.snapshot))),
            ),
            (
                "run_host_plan",
                lifecycle(server, |server, call| {
                    handle_run_host_plan(server, call.args, call.reservation(), call.claimed())
                }),
            ),
            (
                "get_host_plan_run",
                lifecycle(server, |server, call| {
                    handle_get_host_plan_run(server, call.args)
                }),
            ),
        ],
    }
}

/// M6: host-local recipes, expanded into host plans.
fn host_recipes(server: &ServerHandle) -> Service {
    use crate::host_recipe_mcp::{handle_run_host_local_recipe, handle_validate_host_local_recipe};
    Service {
        id: "host-recipes",
        dependencies: &["plans"],
        operations: vec![
            (
                "validate_host_local_recipe",
                lifecycle(server, |server, call| {
                    handle_validate_host_local_recipe(server, call.args)
                }),
            ),
            (
                "run_host_local_recipe",
                lifecycle(server, |server, call| {
                    handle_run_host_local_recipe(
                        server,
                        call.args,
                        call.reservation(),
                        call.claimed(),
                    )
                }),
            ),
        ],
    }
}

/// M6: runtime recipes, expanded into host plans.
fn runtime_recipes(server: &ServerHandle) -> Service {
    use crate::provider_mcp::{handle_run_runtime_recipe, handle_validate_runtime_recipe};
    Service {
        id: "runtime-recipes",
        dependencies: &["plans"],
        operations: vec![
            (
                "validate_runtime_recipe",
                lifecycle(server, |server, call| {
                    handle_validate_runtime_recipe(server, call.args)
                }),
            ),
            (
                "run_runtime_recipe",
                lifecycle(server, |server, call| {
                    handle_run_runtime_recipe(server, call.args, call.lease())
                }),
            ),
            (
                "get_runtime_recipe_run",
                lifecycle(server, |server, call| {
                    crate::plan_mcp::handle_get_host_plan_run(server, call.args)
                }),
            ),
        ],
    }
}

/// M6: tunnel recipes, expanded into host plans.
fn tunnel_recipes(server: &ServerHandle) -> Service {
    use crate::provider_mcp::{handle_run_tunnel_recipe, handle_validate_tunnel_recipe};
    Service {
        id: "tunnel-recipes",
        dependencies: &["plans"],
        operations: vec![
            (
                "validate_tunnel_recipe",
                lifecycle(server, |server, call| {
                    handle_validate_tunnel_recipe(server, call.args)
                }),
            ),
            (
                "run_tunnel_recipe",
                lifecycle(server, |server, call| {
                    handle_run_tunnel_recipe(server, call.args, call.lease())
                }),
            ),
            (
                "get_tunnel_run",
                lifecycle(server, |server, call| {
                    crate::plan_mcp::handle_get_host_plan_run(server, call.args)
                }),
            ),
        ],
    }
}

/// A resource registration seam: tenant-scoped canonical URIs only.
fn registrar(
    host: &crate::tools::Host,
) -> impl Fn(&str, Map<String, J>) + Send + Sync + Clone + 'static {
    let (state, tenant) = (host.state.clone(), host.tenant_id.clone());
    move |uri: &str, coordinates: Map<String, J>| {
        let _ = crate::resource::register(&state, &tenant, uri, &coordinates);
    }
}

/// M3: the host's own description and platform identity. Describing the
/// host is the one operation whose seam is the whole host record.
fn host_info(host: &crate::tools::Host) -> Service {
    use crate::tools::{describe_host, structured_with_text, HOST_INFO};
    let described = Arc::new(host.clone());
    let env = host.env.clone();
    Service {
        id: "host-info",
        dependencies: &[],
        operations: vec![
            (
                "get_host_info",
                op(move |_| Ok(structured_with_text(&describe_host(&described), &HOST_INFO))),
            ),
            (
                "detect_host_platform",
                op(move |_| {
                    let platform = crate::hostobs::detect_platform(&env);
                    let text = format!(
                        "Host platform detected: {} on {}.",
                        platform["kind"].as_str().unwrap_or(""),
                        platform["cpu"]["architecture"].as_str().unwrap_or("")
                    );
                    Ok(
                        serde_json::json!({"content": [{"type": "text", "text": text}], "structuredContent": platform}),
                    )
                }),
            ),
        ],
    }
}

/// M3: Incus inventory reads. The provider-native name comes from the
/// canonical binding, never from a raw argument.
fn incus_inventory(host: &crate::tools::Host) -> Service {
    use crate::tools::{
        probe_incus_gpu, structured_result, structured_with_text, VM_INFO, VM_LIST,
    };
    let (incus, register) = (host.incus.clone(), registrar(host));
    let (listing, listed) = (host.incus.clone(), registrar(host));
    let env = host.env.clone();
    Service {
        id: "instance-inventory",
        dependencies: &[],
        operations: vec![
            (
                "get_vm_info",
                op(move |call| {
                    incus
                        .get_vm_info(
                            &call.binding.provider_instance_name(),
                            flag(call.args, "fast"),
                            &register,
                        )
                        .map(|v| structured_with_text(&v, &VM_INFO))
                }),
            ),
            (
                "probe_virtualization_gpu",
                op(move |call| {
                    Ok(structured_result(
                        probe_incus_gpu(&env, call.args),
                        Some("Incus GPU capability report generated"),
                    ))
                }),
            ),
            (
                "list_vms",
                op(move |call| {
                    listing
                        .list_vms(flag(call.args, "fast"), &listed)
                        .map(|v| structured_with_text(&v, &VM_LIST))
                }),
            ),
        ],
    }
}

/// M3: read-only observation of host files, services and endpoints.
fn host_observation(host: &crate::tools::Host) -> Service {
    use crate::tools::{structured_result, structured_with_text, HTTP_OBSERVATION};
    let inspect_env = host.env.clone();
    let (list_env, tenant, register) = (host.env.clone(), host.tenant_id.clone(), registrar(host));
    Service {
        id: "host-observation",
        dependencies: &[],
        operations: vec![
            (
                "inspect_host_file",
                op(|call| {
                    // expectedContent is read raw: ensure_host_file writes it raw.
                    let raw = call
                        .args
                        .get("expectedContent")
                        .and_then(J::as_str)
                        .unwrap_or("");
                    crate::hostread::inspect_host_file(
                        text(call.args, "path"),
                        text(call.args, "scope"),
                        text(call.args, "expectedSha256"),
                        raw,
                    )
                    .map(|v| structured_result(v, Some("Managed host file inspected.")))
                }),
            ),
            (
                "inspect_host_service",
                op(move |call| {
                    // `serviceNameFromBinding` / `serviceScopeFromBinding`: an
                    // explicit argument wins over the bound coordinate.
                    let field = |key: &str| {
                        let explicit = text(call.args, key);
                        if explicit.is_empty() {
                            call.binding
                                .coordinate(key)
                                .and_then(J::as_str)
                                .map(|v| v.trim().to_string())
                                .unwrap_or_default()
                        } else {
                            explicit.to_string()
                        }
                    };
                    let listen_port = call
                        .args
                        .get("listenPort")
                        .and_then(J::as_f64)
                        .map(|v| v as i64)
                        .unwrap_or(0);
                    crate::hostread::inspect_host_service(
                        &inspect_env,
                        &field("serviceName"),
                        &field("scope"),
                        listen_port,
                    )
                    .map(|v| structured_result(v, Some("Host service status inspected.")))
                }),
            ),
            (
                "list_host_services",
                op(move |call| {
                    let scope = call.args.get("scope").and_then(J::as_str).unwrap_or("");
                    crate::hostread::list_host_services(&list_env, &tenant, scope, &register)
                        .map(|v| structured_result(v, None))
                }),
            ),
            (
                "probe_http_endpoint",
                op(|call| {
                    crate::hostread::probe_http_endpoint(
                        text(call.args, "endpoint"),
                        flag(call.args, "acceptAuthenticationChallenge"),
                    )
                    .map(|v| structured_with_text(&v, &HTTP_OBSERVATION))
                }),
            ),
        ],
    }
}

/// M8a: managed host files (`internal/domain/host/file.go`). Each mutation
/// first runs the shared-host owner check, so the service is built from the
/// instance identity alone.
fn host_files(host: &crate::tools::Host) -> Service {
    use crate::tools::structured_result;
    let ensure_owner = (
        host.instance_id.clone(),
        host.shared_host_owner_instance.clone(),
    );
    let remove_owner = ensure_owner.clone();
    Service {
        id: "host-files",
        dependencies: &[],
        operations: vec![
            (
                "ensure_host_file",
                op(move |call| {
                    let owner = crate::hostfiles::Owner {
                        instance_id: &ensure_owner.0,
                        owner_instance: &ensure_owner.1,
                    };
                    crate::hostfiles::ensure_host_file(
                        &owner,
                        text(call.args, "path"),
                        raw_text(call.args, "content"),
                        int_field(call.args, "mode"),
                        text(call.args, "scope"),
                    )
                    .map(|v| structured_result(v, Some("Managed host file reconciled.")))
                }),
            ),
            (
                "remove_host_file",
                op(move |call| {
                    let owner = crate::hostfiles::Owner {
                        instance_id: &remove_owner.0,
                        owner_instance: &remove_owner.1,
                    };
                    crate::hostfiles::remove_host_file(
                        &owner,
                        text(call.args, "path"),
                        text(call.args, "expectedSha256"),
                        flag(call.args, "confirm"),
                        text(call.args, "scope"),
                    )
                    .map(|v| structured_result(v, Some("Managed host file removed.")))
                }),
            ),
        ],
    }
}

/// M8a: verified archives extracted beneath the account home
/// (`internal/domain/host/archive.go`).
fn host_archives(host: &crate::tools::Host) -> Service {
    use crate::tools::structured_result;
    let owner = (
        host.instance_id.clone(),
        host.shared_host_owner_instance.clone(),
    );
    Service {
        id: "host-archives",
        dependencies: &[],
        operations: vec![(
            "extract_host_archive",
            op(move |call| {
                let owner = crate::hostfiles::Owner {
                    instance_id: &owner.0,
                    owner_instance: &owner.1,
                };
                crate::hostarchive::extract_host_archive(
                    &owner,
                    text(call.args, "archivePath"),
                    text(call.args, "destination"),
                    text(call.args, "format"),
                )
                .map(|v| structured_result(v, Some("Verified host archive extracted.")))
            }),
        )],
    }
}

/// M8a: verified HTTPS artifacts installed beneath the account home
/// (`internal/domain/host/artifact.go`).
fn host_artifacts(host: &crate::tools::Host) -> Service {
    use crate::tools::structured_result;
    let owner = (
        host.instance_id.clone(),
        host.shared_host_owner_instance.clone(),
    );
    let prepare_owner = owner.clone();
    let env = host.env.clone();
    Service {
        id: "host-artifacts",
        dependencies: &["host-tools"],
        operations: vec![
            (
                "ensure_host_artifact",
                op(move |call| {
                    let owner = crate::hostfiles::Owner {
                        instance_id: &owner.0,
                        owner_instance: &owner.1,
                    };
                    crate::hostartifact::ensure_host_artifact(
                        &owner,
                        text(call.args, "uri"),
                        text(call.args, "destination"),
                        text(call.args, "sha256"),
                        flag(call.args, "executable"),
                    )
                    .map(|v| structured_result(v, Some("Verified host artifact reconciled.")))
                }),
            ),
            (
                "prepare_host_agent_artifacts",
                op(move |call| {
                    let owner = crate::hostfiles::Owner {
                        instance_id: &prepare_owner.0,
                        owner_instance: &prepare_owner.1,
                    };
                    crate::hostartifact::prepare_host_agent_artifacts(
                        &env,
                        &owner,
                        text(call.args, "sourceDir"),
                        text(call.args, "destDir"),
                        &string_slice(call.args, "archs"),
                    )
                    .map(|v| structured_result(v, Some("Host-agent build artifacts are ready.")))
                }),
            ),
        ],
    }
}

/// M8a: caller-declared host commands in a workload unit, and the internal
/// guest exec on a bound instance.
fn host_exec(host: &crate::tools::Host) -> Service {
    use crate::tools::structured_result;
    let shell_env = host.env.clone();
    let command_env = host.env.clone();
    let incus = host.incus.clone();
    Service {
        id: "host-exec",
        dependencies: &[],
        operations: vec![
            (
                "agent_shell",
                op(move |call| {
                    crate::hostcommand::run_host_command(
                        &shell_env,
                        call.ctx,
                        call.args,
                        int_field(call.args, "timeoutMs"),
                    )
                }),
            ),
            (
                "run_host_command",
                op(move |call| {
                    crate::hostcommand::run_host_command(
                        &command_env,
                        call.ctx,
                        call.args,
                        int_field(call.args, "timeoutMs"),
                    )
                }),
            ),
            (
                "exec_command",
                op(move |call| {
                    let out = crate::hostcommand::exec_command(
                        &incus,
                        &call.binding.provider_instance_name(),
                        text(call.args, "command"),
                        &string_slice(call.args, "args"),
                        int_field(call.args, "timeout"),
                    )?;
                    let output = out["output"].as_str().unwrap_or("").to_string();
                    Ok(structured_result(out, Some(&output)))
                }),
            ),
        ],
    }
}

/// M8a: the typed guest command on a resolved `vm:` or `container:` URI.
fn instance_exec(host: &crate::tools::Host) -> Service {
    use crate::tools::structured_result;
    let resolve = crate::admission::resolver(host);
    let incus = host.incus.clone();
    Service {
        id: "instance-exec",
        dependencies: &[],
        operations: vec![(
            "run_instance_command",
            op(move |call| {
                let out = crate::hostcommand::run_instance_command(
                    &resolve,
                    &incus,
                    call.ctx,
                    text(call.args, "uri"),
                    text(call.args, "command"),
                    &string_slice(call.args, "args"),
                    raw_text(call.args, "stdin"),
                    int_field(call.args, "timeoutMs"),
                )?;
                let text = format!("Executed command on {}.", out["uri"].as_str().unwrap_or(""));
                Ok(structured_result(out, Some(&text)))
            }),
        )],
    }
}

/// M8a: host service lifecycle through the matching systemd manager.
fn host_services(host: &crate::tools::Host) -> Service {
    use crate::hostservice as svc;
    use crate::tools::structured_result;
    let owner = (
        host.instance_id.clone(),
        host.shared_host_owner_instance.clone(),
    );
    let env = host.env.clone();
    let (o1, o2, o3) = (owner.clone(), owner.clone(), owner);
    let (e1, e2, e3, e4) = (env.clone(), env.clone(), env.clone(), env);
    Service {
        id: "host-services",
        dependencies: &[],
        operations: vec![
            (
                "restart_host_service",
                op(move |call| {
                    let owner = crate::hostfiles::Owner {
                        instance_id: &o1.0,
                        owner_instance: &o1.1,
                    };
                    let out =
                        svc::restart_host_service(&e1, &owner, text(call.args, "serviceName"))?;
                    let text = format!(
                        "Restarted service '{}'.",
                        out["serviceName"].as_str().unwrap_or("")
                    );
                    Ok(structured_result(out, Some(&text)))
                }),
            ),
            (
                "set_host_service_state",
                op(move |call| {
                    let owner = crate::hostfiles::Owner {
                        instance_id: &o2.0,
                        owner_instance: &o2.1,
                    };
                    let service = binding_or(call, "serviceName");
                    let scope = binding_or(call, "scope");
                    let out = svc::set_host_service_state(
                        &e2,
                        &owner,
                        &service,
                        text(call.args, "state"),
                        &scope,
                    )?;
                    let text = format!(
                        "Applied service state '{}' to '{}'.",
                        out["state"].as_str().unwrap_or(""),
                        out["serviceName"].as_str().unwrap_or("")
                    );
                    Ok(structured_result(out, Some(&text)))
                }),
            ),
            (
                "ensure_host_service_supervisor",
                op(move |call| {
                    let owner = crate::hostfiles::Owner {
                        instance_id: &o3.0,
                        owner_instance: &o3.1,
                    };
                    svc::ensure_host_service_supervisor(&e3, &owner, text(call.args, "scope"))
                        .map(|v| structured_result(v, Some("Host service supervisor is ready.")))
                }),
            ),
            (
                "inspect_host_service_supervisor",
                op(move |call| {
                    svc::inspect_host_service_supervisor(&e4, text(call.args, "scope"))
                        .map(|v| structured_result(v, Some("Host service supervisor inspected.")))
                }),
            ),
        ],
    }
}

/// `serviceNameFromBinding` / `serviceScopeFromBinding`: the argument, else
/// the bound host service's coordinate.
fn binding_or(call: &Invocation, key: &str) -> String {
    let value = text(call.args, key);
    if !value.is_empty() {
        return value.to_string();
    }
    call.binding
        .coordinate(key)
        .and_then(J::as_str)
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// M8a: the platform bridge probe and its recovery by unit restart.
fn bridge(host: &crate::tools::Host) -> Service {
    use crate::hostservice as svc;
    use crate::tools::structured_result;
    let owner = (
        host.instance_id.clone(),
        host.shared_host_owner_instance.clone(),
    );
    let (diagnose_env, recover_env) = (host.env.clone(), host.env.clone());
    Service {
        id: "bridge",
        dependencies: &["host-services"],
        operations: vec![
            (
                "diagnose_bridge",
                op(move |_| {
                    let out = svc::diagnose_bridge(&diagnose_env);
                    let text = svc::bridge_text(&out);
                    Ok(structured_result(out, Some(&text)))
                }),
            ),
            (
                "recover_bridge",
                op(move |_| {
                    let owner = crate::hostfiles::Owner {
                        instance_id: &owner.0,
                        owner_instance: &owner.1,
                    };
                    let out = svc::recover_bridge(&recover_env, &owner)?;
                    let text = format!(
                        "Attempted bridge recovery by restarting '{}'.",
                        out["bridgeProcess"]["command"].as_str().unwrap_or("")
                    );
                    Ok(structured_result(out, Some(&text)))
                }),
            ),
        ],
    }
}

/// M8a: a caller-owned connection environment file, then an optional
/// restart through the host service lifecycle.
fn agent_connection(host: &crate::tools::Host) -> Service {
    use crate::tools::structured_result;
    let owner = (
        host.instance_id.clone(),
        host.shared_host_owner_instance.clone(),
    );
    let env = host.env.clone();
    Service {
        id: "agent-connection",
        dependencies: &["host-services"],
        operations: vec![(
            "configure_agent_connection",
            op(move |call| {
                let owner = crate::hostfiles::Owner {
                    instance_id: &owner.0,
                    owner_instance: &owner.1,
                };
                let environment = call
                    .args
                    .get("environment")
                    .and_then(J::as_object)
                    .map(|m| {
                        m.iter()
                            .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_string())))
                            .collect()
                    })
                    .unwrap_or_default();
                crate::hostconnect::configure_agent_connection(
                    &env,
                    &owner,
                    text(call.args, "envFile"),
                    &environment,
                    &string_slice(call.args, "remove"),
                    text(call.args, "serviceName"),
                    call.args.get("restart").and_then(J::as_bool),
                    text(call.args, "scope"),
                )
                .map(|v| structured_result(v, Some("Agent connection configuration written.")))
            }),
        )],
    }
}

/// M8a: public MCP exposure through a pinned connector, stable or quick.
fn public_mcp(host: &crate::tools::Host) -> Service {
    use crate::tools::structured_result;
    let owner = Arc::new((
        host.instance_id.clone(),
        host.shared_host_owner_instance.clone(),
    ));
    let env = host.env.clone();
    let (tunnel_owner, tunnel_env) = (owner.clone(), env.clone());
    let (quick_owner, quick_env) = (owner.clone(), env.clone());
    Service {
        id: "public-mcp",
        dependencies: &["host-artifacts", "host-files", "host-services"],
        operations: vec![
            (
                "ensure_public_mcp_tunnel",
                op(move |call| {
                    let owner = crate::hostfiles::Owner {
                        instance_id: &tunnel_owner.0,
                        owner_instance: &tunnel_owner.1,
                    };
                    let a = call.args;
                    let args = crate::hostpublic::TunnelArgs {
                        binding_id: text(a, "bindingId").into(),
                        endpoint: text(a, "endpoint").into(),
                        local_target: text(a, "localTarget").into(),
                        origin_host_id: text(a, "originHostId").into(),
                        tunnel_token: text(a, "tunnelToken").into(),
                        artifact_uri: text(a, "artifactUri").into(),
                        artifact_sha256: text(a, "artifactSha256").into(),
                        artifact_path: text(a, "artifactPath").into(),
                        token_file: text(a, "tokenFile").into(),
                        service_name: text(a, "serviceName").into(),
                        service_file: text(a, "serviceFile").into(),
                        scope: text(a, "scope").into(),
                    };
                    crate::hostpublic::ensure_public_mcp_tunnel(
                        &tunnel_env,
                        &owner,
                        call.ctx,
                        &args,
                    )
                    .map(|v| {
                        structured_result(v, Some("Authenticated public MCP tunnel reconciled."))
                    })
                }),
            ),
            (
                "ensure_public_mcp_quick_tunnel",
                op(move |call| {
                    let owner = crate::hostfiles::Owner {
                        instance_id: &quick_owner.0,
                        owner_instance: &quick_owner.1,
                    };
                    let a = call.args;
                    crate::hostpublic::ensure_public_mcp_quick_tunnel(
                        &quick_env,
                        &owner,
                        call.ctx,
                        text(a, "bindingId"),
                        text(a, "localTarget"),
                        text(a, "artifactUri"),
                        text(a, "artifactSha256"),
                        text(a, "scope"),
                    )
                    .map(|v| {
                        structured_result(
                            v,
                            Some("Authenticated public MCP quick tunnel reconciled."),
                        )
                    })
                }),
            ),
            (
                "remove_public_mcp_quick_tunnel",
                op(move |call| {
                    let owner = crate::hostfiles::Owner {
                        instance_id: &owner.0,
                        owner_instance: &owner.1,
                    };
                    let a = call.args;
                    crate::hostpublic::remove_public_mcp_quick_tunnel(
                        &env,
                        &owner,
                        call.ctx,
                        text(a, "bindingId"),
                        text(a, "artifactUri"),
                        text(a, "artifactSha256"),
                        text(a, "scope"),
                        flag(a, "confirm"),
                    )
                    .map(|v| {
                        structured_result(
                            v,
                            Some("Authenticated public MCP quick tunnel cleanup reconciled."),
                        )
                    })
                }),
            ),
        ],
    }
}

/// M8a: the WSL2 host VM subsystem: distribution termination, subsystem
/// shutdown and fail-closed disk compaction.
fn host_vm_subsystem(host: &crate::tools::Host) -> Service {
    use crate::tools::structured_result;
    let (terminate_env, shutdown_env, compact_env) =
        (host.env.clone(), host.env.clone(), host.env.clone());
    Service {
        id: "host-vm-subsystem",
        dependencies: &[],
        operations: vec![
            (
                "terminate_host_vm_distribution",
                op(move |call| {
                    crate::hostwsl::terminate_distribution(
                        &terminate_env,
                        call.ctx,
                        text(call.args, "distro"),
                    )
                    .map(|v| structured_result(v, Some("WSL distribution terminated.")))
                }),
            ),
            (
                "shutdown_host_vm_subsystem",
                op(move |call| {
                    crate::hostwsl::shutdown(&shutdown_env, call.ctx)
                        .map(|v| structured_result(v, Some("WSL environment shut down.")))
                }),
            ),
            (
                "compact_host_vm_disk",
                op(move |call| {
                    match crate::hostwsl::compact_disk(
                        &compact_env,
                        call.ctx,
                        text(call.args, "distro"),
                        flag(call.args, "dryRun"),
                    )? {
                        crate::hostwsl::Compact::Report(v) => {
                            Ok(structured_result(v, Some("WSL disk compact completed.")))
                        }
                        crate::hostwsl::Compact::Closed(result) => Ok(result),
                    }
                }),
            ),
        ],
    }
}

/// M8a: the host firewall rule a tunnel provider may request.
fn host_firewall() -> Service {
    Service {
        id: "host-firewall",
        dependencies: &[],
        operations: vec![(
            "ensure_host_firewall_rule",
            op(|call| {
                let out = crate::hostconnect::ensure_host_firewall_rule(
                    text(call.args, "bindingId"),
                    int_field(call.args, "port"),
                );
                let text = format!("Firewall rule applied={}", out["applied"]);
                Ok(crate::tools::structured_result(out, Some(&text)))
            }),
        )],
    }
}

/// M8a: the virtualization stack's install and fail-closed uninstall.
fn virtualization_stack(host: &crate::tools::Host) -> Service {
    use crate::tools::structured_result;
    let owner = (
        host.instance_id.clone(),
        host.shared_host_owner_instance.clone(),
    );
    let uninstall_owner = owner.clone();
    let (env, uninstall_env) = (host.env.clone(), host.env.clone());
    let (incus, uninstall_incus) = (host.incus.clone(), host.incus.clone());
    let reset = instances(host);
    let reset_owner = (
        host.instance_id.clone(),
        host.shared_host_owner_instance.clone(),
    );
    let (reset_env, reset_incus) = (host.env.clone(), host.incus.clone());
    Service {
        id: "virtualization-stack",
        dependencies: &[],
        operations: vec![
            (
                "install_virtualization_stack",
                op(move |call| {
                    let owner = crate::hostfiles::Owner {
                        instance_id: &owner.0,
                        owner_instance: &owner.1,
                    };
                    let args = crate::hostvirt::InstallArgs {
                        package_name: text(call.args, "packageName").into(),
                        qemu_package: text(call.args, "qemuPackage").into(),
                        gpu_packages: string_slice(call.args, "gpuPackages"),
                        channel: text(call.args, "incusChannel").into(),
                        version: text(call.args, "incusVersion").into(),
                        install_qemu: flag(call.args, "installQemu"),
                    };
                    crate::hostvirt::install_incus_stack(&env, &owner, &incus, &args).map(|v| {
                        structured_result(v, Some("Incus virtualization prerequisites installed"))
                    })
                }),
            ),
            (
                "uninstall_virtualization_stack",
                op(move |call| {
                    let owner = crate::hostfiles::Owner {
                        instance_id: &uninstall_owner.0,
                        owner_instance: &uninstall_owner.1,
                    };
                    crate::hostvirt::uninstall_incus_stack(
                        &uninstall_env,
                        &owner,
                        &uninstall_incus,
                        flag(call.args, "confirm"),
                        flag(call.args, "removeState"),
                        flag(call.args, "keepRepository"),
                    )
                    .map(|v| {
                        structured_result(
                            v,
                            Some("Incus virtualization stack removed from this host"),
                        )
                    })
                }),
            ),
            (
                "reset_virtualization_stack",
                op(move |call| {
                    let a = call.args;
                    let args = crate::incusops::ResetArgs {
                        instance_names: string_slice(a, "instanceNames"),
                        instance_prefix: text(a, "instancePrefix").into(),
                        confirm: flag(a, "confirm"),
                        reinstall: flag(a, "reinstall"),
                        dry_run: flag(a, "dryRun"),
                        disposable_host_fingerprint: text(a, "disposableHostFingerprint").into(),
                        expected_host_fingerprint: text(a, "expectedHostFingerprint").into(),
                        disposable_host_authorization: text(a, "disposableHostAuthorization")
                            .into(),
                    };
                    // The reconcile is the host's stack install with default
                    // settings.
                    let reinstall = || {
                        let owner = crate::hostfiles::Owner {
                            instance_id: &reset_owner.0,
                            owner_instance: &reset_owner.1,
                        };
                        crate::hostvirt::install_incus_stack(
                            &reset_env,
                            &owner,
                            &reset_incus,
                            &crate::hostvirt::InstallArgs {
                                package_name: String::new(),
                                qemu_package: String::new(),
                                gpu_packages: Vec::new(),
                                channel: String::new(),
                                version: String::new(),
                                install_qemu: false,
                            },
                        )
                    };
                    settled(reset.reset_stack(&args, call.ctx, &reinstall).map(|out| {
                        structured_result(out, Some("Incus stack reset phase completed"))
                    }))
                }),
            ),
        ],
    }
}

/// The instance provider's operations over this host's identity, registry,
/// admission snapshot and reset checkpoint.
fn instances(host: &crate::tools::Host) -> crate::incusops::Instances {
    crate::incusops::Instances {
        incus: host.incus.clone(),
        env: host.env.clone(),
        state: host.state.clone(),
        tenant_id: host.tenant_id.clone(),
        coordinator: host.coordinator.clone(),
        shared_host_owner_instance: host.shared_host_owner_instance.clone(),
        reset_checkpoint_path: host.reset_checkpoint_path.clone(),
        relays: host.relays.clone(),
        postgres: host.postgres.clone(),
    }
}

/// An instance operation's outcome: a typed admission refusal is a
/// structured error result, any other failure a plain one.
fn settled(outcome: Result<J, crate::incusops::Fail>) -> Result<J, String> {
    match outcome {
        Ok(result) => Ok(result),
        Err(crate::incusops::Fail::Text(text)) => Err(text),
        Err(refusal) => Ok(refusal.render()),
    }
}

/// `provisionArgs`: `vmName`, else `name`.
fn provision_args(args: &Map<String, J>) -> crate::incusops::ProvisionArgs {
    let vm_name = match text(args, "vmName") {
        "" => text(args, "name"),
        name => name,
    };
    crate::incusops::ProvisionArgs {
        vm_name: vm_name.into(),
        image: text(args, "image").into(),
        cpus: int_field(args, "cpus"),
        memory: text(args, "memory").into(),
        disk: text(args, "disk").into(),
        instance_type: text(args, "instanceType").into(),
    }
}

/// M8b: the standalone agent's local prerequisites and status.
fn local_status(host: &crate::tools::Host) -> Service {
    use crate::tools::{structured_with_text, LOCAL_PREREQUISITES, LOCAL_STATUS};
    let (prerequisites, status) = (instances(host), instances(host));
    let register = registrar(host);
    Service {
        id: "local-status",
        dependencies: &["instance-inventory"],
        operations: vec![
            (
                "check_local_prerequisites",
                op(move |_| {
                    Ok(structured_with_text(
                        &prerequisites.local_prerequisites(),
                        &LOCAL_PREREQUISITES,
                    ))
                }),
            ),
            (
                "get_local_status",
                op(move |_| {
                    settled(
                        status
                            .local_status(&register)
                            .map(|v| structured_with_text(&v, &LOCAL_STATUS)),
                    )
                }),
            ),
        ],
    }
}

/// M8b: persistent system containers and the disposable GPU probe.
fn system_containers(host: &crate::tools::Host) -> Service {
    use crate::tools::structured_result;
    let (provision, probe) = (instances(host), instances(host));
    Service {
        id: "system-containers",
        dependencies: &["instance-inventory"],
        operations: vec![
            (
                "provision_container",
                op(move |call| {
                    let a = call.args;
                    let args = crate::incusops::ContainerArgs {
                        container_name: text(a, "containerName").into(),
                        image: text(a, "image").into(),
                        cpus: int_field(a, "cpus"),
                        memory: text(a, "memory").into(),
                        disk: text(a, "disk").into(),
                        gpu: flag(a, "gpu"),
                        host_gpu_libs: flag(a, "hostGpuLibs"),
                        nesting: a.contains_key("nesting").then(|| flag(a, "nesting")),
                        port: int_field(a, "port"),
                        model_volume: text(a, "modelVolume").into(),
                        ipv4_address: text(a, "ipv4Address").into(),
                    };
                    settled(provision.provision_container(&args).map(|out| {
                        let text = format!("Provisioned Incus container '{}'.", out.container_name);
                        structured_result(
                            serde_json::json!({
                                "uri": out.uri, "containerName": out.container_name,
                                "vmName": out.container_name, "image": out.image,
                                "status": out.status, "instanceType": out.instance_type,
                            }),
                            Some(&text),
                        )
                    }))
                }),
            ),
            (
                "probe_gpu_container",
                op(move |_| {
                    settled(probe.probe_gpu_container().map(|out| {
                        structured_result(
                            out,
                            Some("System container GPU capability report generated"),
                        )
                    }))
                }),
            ),
        ],
    }
}

/// M8b: VM and container lifecycle. The instance a scoped operation acts on
/// comes from the canonical binding, never from a raw argument.
fn instance_lifecycle(host: &crate::tools::Host) -> Service {
    use crate::tools::structured_result;
    let (create, provision, start, stop) = (
        instances(host),
        instances(host),
        instances(host),
        instances(host),
    );
    let (restart, update, delete) = (instances(host), instances(host), instances(host));
    let named = |out: &J, verb: &str| format!("{verb} '{}'.", out["vmName"].as_str().unwrap_or(""));
    Service {
        id: "instance-lifecycle",
        dependencies: &["system-containers"],
        operations: vec![
            (
                "create_vm",
                op(move |call| {
                    let mut args = provision_args(call.args);
                    if args.instance_type.trim().is_empty() {
                        args.instance_type = "virtual-machine".into();
                    }
                    settled(create.provision_vm(&args).map(|out| {
                        let text =
                            format!("Created VM '{}' from image '{}'.", out.vm_name, out.image);
                        structured_result(out.to_json(), Some(&text))
                    }))
                }),
            ),
            (
                "provision_vm",
                op(move |call| {
                    settled(
                        provision
                            .provision_vm(&provision_args(call.args))
                            .map(|out| {
                                let kind = match out.instance_type.trim() {
                                    "" => "container",
                                    kind => kind,
                                };
                                let text = format!(
                                    "Provisioned {kind} '{}' from image '{}'.",
                                    out.vm_name, out.image
                                );
                                structured_result(out.to_json(), Some(&text))
                            }),
                    )
                }),
            ),
            (
                "start_vm",
                op(move |call| {
                    settled(
                        start
                            .start_vm(&call.binding.provider_instance_name())
                            .map(|out| {
                                let text = named(&out, "Started VM");
                                structured_result(out, Some(&text))
                            }),
                    )
                }),
            ),
            (
                "stop_vm",
                op(move |call| {
                    settled(
                        stop.stop_vm(&call.binding.provider_instance_name())
                            .map(|out| {
                                let text = named(&out, "Stopped VM");
                                structured_result(out, Some(&text))
                            }),
                    )
                }),
            ),
            (
                "restart_vm",
                op(move |call| {
                    settled(
                        restart
                            .restart_vm(&call.binding.provider_instance_name())
                            .map(|out| {
                                let text = named(&out, "Restarted VM");
                                structured_result(out, Some(&text))
                            }),
                    )
                }),
            ),
            (
                "update_vm_resources",
                op(move |call| {
                    settled(
                        update
                            .update_vm_resources(
                                &call.binding.provider_instance_name(),
                                int_field(call.args, "cpus"),
                                text(call.args, "memory"),
                                text(call.args, "disk"),
                            )
                            .map(|out| {
                                let text = named(&out, "Updated resources for");
                                structured_result(out, Some(&text))
                            }),
                    )
                }),
            ),
            (
                "delete_vm",
                op(move |call| {
                    settled(
                        delete
                            .delete_vm(&call.binding.provider_instance_name())
                            .map(|out| {
                                let text = named(&out, "Deleted VM");
                                structured_result(out, Some(&text))
                            }),
                    )
                }),
            ),
        ],
    }
}

/// M8a: the container engine an Incus host refuses.
fn container_engine() -> Service {
    Service {
        id: "container-engine",
        dependencies: &[],
        operations: vec![(
            "ensure_container_engine",
            op(|_| crate::hostservice::ensure_docker()),
        )],
    }
}

/// M8a: allowlisted generic host tools (`internal/domain/host/tool.go`).
fn host_tools(host: &crate::tools::Host) -> Service {
    use crate::tools::structured_result;
    let owner = (
        host.instance_id.clone(),
        host.shared_host_owner_instance.clone(),
    );
    let env = host.env.clone();
    Service {
        id: "host-tools",
        dependencies: &[],
        operations: vec![(
            "ensure_host_tool",
            op(move |call| {
                let owner = crate::hostfiles::Owner {
                    instance_id: &owner.0,
                    owner_instance: &owner.1,
                };
                crate::hosttool::ensure_host_tool(&env, &owner, text(call.args, "tool"))
                    .map(|v| structured_result(v, Some("Generic host tool is available.")))
            }),
        )],
    }
}

/// `stringSliceField`: the array's string elements, untrimmed.
fn string_slice(args: &Map<String, J>, key: &str) -> Vec<String> {
    args.get(key)
        .and_then(J::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// M4: host resource capacity and enforcement, from the coordinator alone.
fn host_resources(host: &crate::tools::Host) -> Service {
    let coordinator = host.coordinator.clone();
    let policy = host.coordinator.clone();
    let env = host.env.clone();
    let tenant = host.tenant_id.clone();
    Service {
        id: "host-resources",
        dependencies: &[],
        operations: vec![
            (
                "get_host_capacity",
                op(move |_| {
                    Ok(serde_json::json!({
                        "content": [{"type": "text", "text": "Host capacity and enforcement state observed."}],
                        "structuredContent": coordinator.snapshot(),
                    }))
                }),
            ),
            // M8a: the policy backend repairs the systemd projection
            // admission depends on (`resource_policy.go`).
            (
                "reconcile_host_resource_policy",
                op(move |call| {
                    crate::hostpolicy::reconcile_host_resource_policy(
                        &env,
                        &policy,
                        tenant.trim(),
                        call.ctx,
                        text(call.args, "policyRevision"),
                        text(call.args, "uri"),
                    )
                }),
            ),
        ],
    }
}

/// M4: operator input requests, which own a task in the server's registry.
fn tasks(server: &ServerHandle) -> Service {
    let server = server.clone();
    Service {
        id: "tasks",
        dependencies: &[],
        operations: vec![(
            "request_task_input",
            op(move |call| Ok(crate::tools::create_input_task(&server.get()?, call.args))),
        )],
    }
}

/// M3: the published capability catalog, as admitted for this call.
fn catalog_service() -> Service {
    Service {
        id: "catalog",
        dependencies: &[],
        operations: vec![(
            "get_capability_catalog",
            op(|call| {
                Ok(crate::tools::structured_result(
                    crate::tools::snapshot_json(call.snapshot),
                    None,
                ))
            }),
        )],
    }
}

/// The mounted built-in service that implements `name`, if any.
pub fn resolve(server: &Server, name: &str) -> Option<Arc<crate::provider::mounts::Service>> {
    // A D19 alias reaches the published operation's Consumer.
    let operation = catalog::neutral_name(name).unwrap_or(name);
    server
        .providers
        .activation
        .mounts
        .resolve_operation(&provider_id(), GENERATION, operation)
}

/// Invoke `name` through the service context. `None` when no mounted
/// service implements it.
pub fn invoke(server: &Server, name: &str, invocation: &Invocation) -> Option<Result<J, String>> {
    let service = resolve(server, name)?;
    let Implementation::Builtin(provider) = &service.implementation else {
        return None;
    };
    let Some(operation) = catalog::neutral_name(name) else {
        // D23: a mapped tool's Consumer reads Go's values and its result
        // publishes the mapped values.
        if let Some(args) = catalog::go_value_arguments(name, invocation.args) {
            let call = Invocation {
                ctx: invocation.ctx,
                args: &args,
                binding: invocation.binding,
                snapshot: invocation.snapshot,
                claimed: invocation.claimed,
            };
            return provider.call(name, &call).map(|outcome| {
                outcome.map(|mut result| {
                    if let Some(structured) = result.get_mut("structuredContent") {
                        catalog::publish_result_values(name, structured);
                    }
                    result
                })
            });
        }
        let Some(args) = catalog::renamed_field_arguments(name, invocation.args) else {
            return provider.call(name, invocation);
        };
        return provider.call(
            name,
            &Invocation {
                ctx: invocation.ctx,
                args: &args,
                binding: invocation.binding,
                snapshot: invocation.snapshot,
                claimed: invocation.claimed,
            },
        );
    };
    // An alias call validated against Go's schema; its renamed fields move
    // to the published names before the Consumer reads them.
    let args = catalog::alias_arguments(name, invocation.args);
    provider.call(
        operation,
        &Invocation {
            ctx: invocation.ctx,
            args: &args,
            binding: invocation.binding,
            snapshot: invocation.snapshot,
            claimed: invocation.claimed,
        },
    )
}

// --- M8c: the kubernetes and cluster domains ----------------------------------------

/// The kubernetes domain over its seams: the active Kubernetes provider
/// (through the late-bound server), resource resolution, the registry, and
/// the host tool and command runner `helm template` uses.
fn kubernetes(host: &crate::tools::Host, server: &ServerHandle) -> crate::clusterops::Kubernetes {
    let server = server.clone();
    let register = {
        let (state, tenant) = (host.state.clone(), host.tenant_id.clone());
        Arc::new(move |uri: &str, coordinates: Map<String, J>| {
            crate::resource::register(&state, &tenant, uri, &coordinates)
        })
    };
    let (env, tool_env) = (host.env.clone(), host.env.clone());
    let (instance, owner) = (
        host.instance_id.clone(),
        host.shared_host_owner_instance.clone(),
    );
    crate::clusterops::Kubernetes {
        execute: Arc::new(move |operation: &str, arguments: Map<String, J>| {
            server
                .get()?
                .providers
                .kubernetes_execute(operation, arguments)
        }),
        resolve: crate::admission::resolver(host),
        tenant_id: host.tenant_id.clone(),
        register,
        ensure_tool: Arc::new(move |tool: &str| {
            let owner = crate::hostfiles::Owner {
                instance_id: &instance,
                owner_instance: &owner,
            };
            crate::hosttool::ensure_host_tool(&tool_env, &owner, tool)
        }),
        run_host: Arc::new(move |argv: &[String], timeout| {
            crate::hostobs::run_command(&env, argv, timeout)
        }),
        env: host.env.clone(),
    }
}

/// `resourceURIFromBinding`: the bound cluster's URI.
fn cluster_uri(binding: &Binding) -> String {
    binding
        .resources
        .iter()
        .find(|r| {
            crate::resource::Uri::parse(&r.uri).is_ok_and(|u| u.resource_type == "cluster")
                && !r.uri.trim().is_empty()
        })
        .map(|r| r.uri.clone())
        .unwrap_or_default()
}

/// `withBindingURI(out, binding, "cluster")`.
fn with_cluster_uri(mut out: Map<String, J>, binding: &Binding) -> Map<String, J> {
    if !out.contains_key("uri") {
        let uri = cluster_uri(binding);
        if !uri.is_empty() {
            out.insert("uri".into(), J::from(uri));
        }
    }
    out
}

/// `structuredResult(out, "")` for a `map[string]any`.
fn map_result(out: Map<String, J>) -> J {
    crate::tools::structured_with_text(&J::Object(out), &crate::gojson::Shape::Any)
}

fn resource_args(call: &Invocation) -> crate::clusterops::ResourceArgs {
    crate::clusterops::ResourceArgs {
        uri: cluster_uri(call.binding),
        kind: text(call.args, "kind").into(),
        resource_kind: text(call.args, "resourceKind").into(),
        name: text(call.args, "resourceName").into(),
        namespace: text(call.args, "namespace").into(),
    }
}

/// M8c: provider-backed cluster inventory (`list_kubernetes_clusters`).
fn cluster_discovery(k8s: &crate::clusterops::Kubernetes) -> Service {
    let k8s = k8s.clone();
    Service {
        id: "cluster-discovery",
        dependencies: &[],
        operations: vec![(
            "list_compute_clusters",
            op(move |call| {
                k8s.list_clusters(text(call.args, "source")).map(|out| {
                    crate::tools::structured_with_text(&out, &crate::clusterops::CLUSTER_LIST)
                })
            }),
        )],
    }
}

/// M8c: manifests, resources, events, secrets and kubectl, each one call to
/// the active Kubernetes provider against the bound cluster.
fn cluster_resources(k8s: &crate::clusterops::Kubernetes) -> Service {
    use crate::tools::structured_result;
    let (apply, get, delete, status) = (k8s.clone(), k8s.clone(), k8s.clone(), k8s.clone());
    let (events, secret, exec) = (k8s.clone(), k8s.clone(), k8s.clone());
    Service {
        id: "cluster-resources",
        dependencies: &["cluster-discovery"],
        operations: vec![
            (
                "apply_manifest",
                op(move |call| {
                    apply
                        .apply_manifest(&cluster_uri(call.binding), text(call.args, "manifest"))
                        .map(|out| {
                            structured_result(J::Object(out), Some("Kubernetes manifest applied."))
                        })
                }),
            ),
            (
                "get_cluster_resource",
                op(move |call| {
                    get.get_resource(&resource_args(call))
                        .map(|out| map_result(with_cluster_uri(out, call.binding)))
                }),
            ),
            (
                "delete_cluster_resource",
                op(move |call| {
                    delete.delete_resource(&resource_args(call)).map(|out| {
                        structured_result(
                            J::Object(with_cluster_uri(out, call.binding)),
                            Some("Kubernetes resource deleted."),
                        )
                    })
                }),
            ),
            (
                "get_cluster_resource_status",
                op(move |call| {
                    status.get_resource_status(&resource_args(call)).map(|out| {
                        let mut out = with_cluster_uri(out, call.binding);
                        out.entry("message")
                            .or_insert_with(|| J::from("Kubernetes resource status returned."));
                        structured_result(
                            J::Object(out),
                            Some("Kubernetes resource status returned."),
                        )
                    })
                }),
            ),
            (
                "list_cluster_events",
                op(move |call| {
                    // `args["limit"].(float64)`, truncated.
                    let limit = call.args.get("limit").and_then(J::as_f64).unwrap_or(0.0) as i64;
                    events
                        .list_events(
                            &cluster_uri(call.binding),
                            text(call.args, "namespace"),
                            limit,
                        )
                        .map(|out| map_result(with_cluster_uri(out, call.binding)))
                }),
            ),
            (
                "put_cluster_secret",
                op(move |call| {
                    // Only string values are kept.
                    let data: Map<String, J> = call
                        .args
                        .get("data")
                        .and_then(J::as_object)
                        .map(|raw| {
                            raw.iter()
                                .filter(|(_, v)| v.is_string())
                                .map(|(k, v)| (k.clone(), v.clone()))
                                .collect()
                        })
                        .unwrap_or_default();
                    secret
                        .put_secret(
                            &cluster_uri(call.binding),
                            text(call.args, "namespace"),
                            text(call.args, "name"),
                            &data,
                        )
                        .map(|out| {
                            structured_result(J::Object(out), Some("Kubernetes Secret configured."))
                        })
                }),
            ),
            (
                "exec_cluster_command",
                op(move |call| {
                    exec.exec_command(
                        &cluster_uri(call.binding),
                        text(call.args, "command"),
                        &string_slice(call.args, "args"),
                        text(call.args, "stdin"),
                    )
                    .map(|out| match out {
                        J::Object(out) => map_result(out),
                        other => structured_result(other, None),
                    })
                }),
            ),
        ],
    }
}

/// M8c: cluster listings, each a provider kubectl `get` on the bound
/// cluster, answered with the binding's canonical URI.
fn cluster_listings(k8s: &crate::clusterops::Kubernetes) -> Service {
    let names = |k8s: &crate::clusterops::Kubernetes,
                 resource: &'static str,
                 keys: &'static [&'static str]| {
        let k8s = k8s.clone();
        op(move |call| {
            let names = k8s.list_names(&call.binding.provider_instance_name(), resource)?;
            let names = J::Array(names.into_iter().map(J::from).collect());
            let mut out = Map::new();
            for key in keys {
                out.insert((*key).into(), names.clone());
            }
            Ok(map_result(with_cluster_uri(out, call.binding)))
        })
    };
    type List = fn(&crate::clusterops::Kubernetes, &str, &str) -> Result<Vec<J>, String>;
    let rows = |k8s: &crate::clusterops::Kubernetes, key: &'static str, list: List| {
        let k8s = k8s.clone();
        op(move |call| {
            let rows = list(
                &k8s,
                &call.binding.provider_instance_name(),
                text(call.args, "namespace"),
            )?;
            let mut out = Map::new();
            out.insert(key.into(), J::Array(rows));
            Ok(map_result(with_cluster_uri(out, call.binding)))
        })
    };
    use crate::clusterops::Kubernetes as K;
    Service {
        id: "cluster-listings",
        dependencies: &["cluster-discovery"],
        operations: vec![
            ("list_namespaces", names(k8s, "namespaces", &["namespaces"])),
            (
                "list_storage_classes",
                names(k8s, "storageclasses", &["storageClasses"]),
            ),
            (
                "list_ingress_classes",
                names(k8s, "ingressclasses", &["classes", "ingressClasses"]),
            ),
            (
                "list_certificate_issuers",
                rows(k8s, "issuers", K::list_issuers),
            ),
            ("list_pods", rows(k8s, "pods", K::list_pods)),
            ("list_services", rows(k8s, "services", K::list_services)),
            (
                "list_deployments",
                rows(k8s, "deployments", K::list_deployments),
            ),
        ],
    }
}

/// M8c: charts as k3s HelmChart manifests through the provider, and
/// `helm template` on this host.
fn cluster_packages(k8s: &crate::clusterops::Kubernetes) -> Service {
    use crate::tools::structured_result;
    let (install, render) = (k8s.clone(), k8s.clone());
    Service {
        id: "cluster-packages",
        dependencies: &["cluster-resources", "host-tools"],
        operations: vec![
            (
                "install_cluster_package",
                op(move |call| {
                    let first = |keys: &[&str]| {
                        keys.iter()
                            .map(|k| text(call.args, k))
                            .find(|v| !v.is_empty())
                            .unwrap_or("")
                            .to_string()
                    };
                    let namespace = match text(call.args, "namespace") {
                        "" => "kube-system",
                        namespace => namespace,
                    };
                    let args = crate::clusterops::ChartArgs {
                        vm_name: call.binding.provider_instance_name(),
                        release_name: first(&["releaseName", "chartName", "name"]),
                        chart_source: first(&["chartSource", "chart"]),
                        namespace: namespace.into(),
                        repo: text(call.args, "repo").into(),
                        values: crate::clusterops::helm_values(call.args.get("values")),
                    };
                    install.install_chart(&args).map(|out| {
                        let message =
                            format!("HelmChart '{}' deployment initiated.", args.release_name);
                        structured_result(out, Some(&message))
                    })
                }),
            ),
            (
                "render_cluster_package",
                op(move |call| {
                    render
                        .render_template(&crate::clusterops::TemplateArgs {
                            chart_path: text(call.args, "chartPath").into(),
                            release_name: text(call.args, "releaseName").into(),
                            values_files: string_slice(call.args, "valuesFiles"),
                            set: string_slice(call.args, "set"),
                            namespace: text(call.args, "namespace").into(),
                        })
                        .map(|out| structured_result(out, Some("Helm chart rendered.")))
                }),
            ),
        ],
    }
}

/// M8c: service domains as Ingress objects through the provider.
fn service_exposure(k8s: &crate::clusterops::Kubernetes) -> Service {
    use crate::tools::structured_result;
    let (configure, remove) = (k8s.clone(), k8s.clone());
    let domain = |call: &Invocation| crate::clusterops::DomainArgs {
        vm_name: call.binding.provider_instance_name(),
        namespace: text(call.args, "namespace").into(),
        ingress_name: text(call.args, "ingressName").into(),
        hostname: text(call.args, "hostname").into(),
        service_name: text(call.args, "serviceName").into(),
        service_port: int_field(call.args, "servicePort"),
        ingress_class: text(call.args, "ingressClass").into(),
    };
    Service {
        id: "service-exposure",
        dependencies: &["cluster-resources"],
        operations: vec![
            (
                "configure_service_domain",
                op(move |call| {
                    configure.configure_domain(&domain(call)).map(|out| {
                        structured_result(out, Some("Service domain mapping configured."))
                    })
                }),
            ),
            (
                "remove_service_domain",
                op(move |call| {
                    remove
                        .remove_domain(&domain(call))
                        .map(|out| structured_result(out, Some("Service domain mapping removed.")))
                }),
            ),
        ],
    }
}

/// M8c: guest storage inspection and reclamation, through the provider.
fn guest_storage(k8s: &crate::clusterops::Kubernetes) -> Service {
    use crate::clusterops::{
        GuestStorageArgs, GARBAGE_COLLECT_REGISTRY, INSPECT_GUEST_STORAGE, PRUNE_UNUSED_IMAGES,
        TRIM_GUEST_STORAGE,
    };
    let storage = |k8s: &crate::clusterops::Kubernetes,
                   operation: &'static str,
                   dry_run: bool,
                   message: Option<&'static str>,
                   parse: fn(&Invocation) -> GuestStorageArgs| {
        let k8s = k8s.clone();
        op(move |call| {
            let out = k8s.guest_storage(operation, &parse(call), dry_run)?;
            let out = with_cluster_uri(out, call.binding);
            Ok(match message {
                None => map_result(out),
                Some(text) => crate::tools::structured_result(J::Object(out), Some(text)),
            })
        })
    };
    Service {
        id: "guest-storage",
        dependencies: &["cluster-resources"],
        operations: vec![
            (
                "inspect_guest_storage",
                storage(k8s, INSPECT_GUEST_STORAGE, false, None, |call| {
                    GuestStorageArgs {
                        uri: cluster_uri(call.binding),
                        include_registry: flag(call.args, "includeRegistry"),
                        extra_keep_tags: string_slice(call.args, "extraKeepTags"),
                        registry_namespace: text(call.args, "registryNamespace").into(),
                        registry_name: text(call.args, "registryName").into(),
                        ..GuestStorageArgs::default()
                    }
                }),
            ),
            (
                "prune_unused_cluster_images",
                storage(
                    k8s,
                    PRUNE_UNUSED_IMAGES,
                    true,
                    Some("Unused cluster images pruned."),
                    |call| GuestStorageArgs {
                        uri: cluster_uri(call.binding),
                        dry_run: flag(call.args, "dryRun"),
                        min_age_seconds: call
                            .args
                            .contains_key("minAgeSeconds")
                            .then(|| int_field(call.args, "minAgeSeconds")),
                        extra_keep_tags: string_slice(call.args, "extraKeepTags"),
                        ..GuestStorageArgs::default()
                    },
                ),
            ),
            (
                "garbage_collect_cluster_registry",
                storage(
                    k8s,
                    GARBAGE_COLLECT_REGISTRY,
                    true,
                    Some("Cluster registry garbage collection completed."),
                    |call| GuestStorageArgs {
                        uri: cluster_uri(call.binding),
                        dry_run: flag(call.args, "dryRun"),
                        include_registry: flag(call.args, "includeRegistry"),
                        extra_keep_tags: string_slice(call.args, "extraKeepTags"),
                        registry_namespace: text(call.args, "registryNamespace").into(),
                        registry_name: text(call.args, "registryName").into(),
                        ..GuestStorageArgs::default()
                    },
                ),
            ),
            (
                "trim_guest_storage",
                storage(
                    k8s,
                    TRIM_GUEST_STORAGE,
                    false,
                    Some("Guest filesystem trimmed."),
                    |call| GuestStorageArgs {
                        uri: cluster_uri(call.binding),
                        ..GuestStorageArgs::default()
                    },
                ),
            ),
        ],
    }
}

// --- M8d: the oci domain --------------------------------------------------------------

/// The oci domain over its seams: the kubernetes domain for the registry,
/// the host's container tools, and the storage policy file.
fn oci(host: &crate::tools::Host, k8s: &crate::clusterops::Kubernetes) -> crate::ociops::Oci {
    let env = host.env.clone();
    crate::ociops::Oci {
        k8s: k8s.clone(),
        env: host.env.clone(),
        command: Arc::new(move |argv: &[String], deadline| {
            crate::hostexec::combined_output(&env, argv, deadline)
        }),
        storage_policy_path: host.oci_storage_policy_path.clone(),
        storage_mu: Default::default(),
        instance_id: host.instance_id.clone(),
        shared_host_owner_instance: host.shared_host_owner_instance.clone(),
        home: host.env.get("HOME").to_string(),
    }
}

/// `optionalInt64Field`: present (even as null) means set, as `int64Field`
/// reads it.
fn optional_int64(args: &Map<String, J>, key: &str) -> Option<i64> {
    args.contains_key(key).then(|| {
        args.get(key)
            .and_then(J::as_f64)
            .map(|v| v as i64)
            .unwrap_or(0)
    })
}

/// M8d: the generic OCI registry, deployed through the kubernetes domain. As
/// in Go, the VM is the binding's provider instance.
fn oci_registry(oci: &crate::ociops::Oci) -> Service {
    use crate::ociops::RegistryArgs;
    use crate::tools::structured_result;
    let (install, status, delete) = (oci.clone(), oci.clone(), oci.clone());
    let registry = |call: &Invocation| RegistryArgs {
        vm_name: call.binding.provider_instance_name(),
        namespace: text(call.args, "namespace").into(),
        name: text(call.args, "name").into(),
        image: text(call.args, "image").into(),
        storage_size: text(call.args, "storageSize").into(),
        storage_class: text(call.args, "storageClass").into(),
        node_port: int_field(call.args, "nodePort"),
    };
    Service {
        id: "oci-registry",
        dependencies: &["cluster-resources"],
        operations: vec![
            (
                "install_oci_registry",
                op(move |call| {
                    install.install_registry(&registry(call)).map(|out| {
                        structured_result(
                            J::Object(out),
                            Some("OCI registry deployment initiated."),
                        )
                    })
                }),
            ),
            (
                "get_oci_registry_status",
                op(move |call| {
                    let args = RegistryArgs {
                        vm_name: call.binding.provider_instance_name(),
                        namespace: text(call.args, "namespace").into(),
                        name: text(call.args, "name").into(),
                        ..RegistryArgs::default()
                    };
                    status.registry_status(&args).map(map_result)
                }),
            ),
            (
                "delete_oci_registry",
                op(move |call| {
                    let args = RegistryArgs {
                        vm_name: call.binding.provider_instance_name(),
                        namespace: text(call.args, "namespace").into(),
                        ..RegistryArgs::default()
                    };
                    delete
                        .delete_registry(&args)
                        .map(|out| structured_result(J::Object(out), Some("OCI registry deleted.")))
                }),
            ),
        ],
    }
}

// --- M8e: the postgres domain ---------------------------------------------------------

/// The postgres domain over its seams: the kubernetes domain, the incus
/// provider for the relay's loopback forward, and its two directories.
fn postgres(
    host: &crate::tools::Host,
    k8s: &crate::clusterops::Kubernetes,
) -> crate::postgresops::Postgres {
    crate::postgresops::Postgres {
        k8s: k8s.clone(),
        incus: host.incus.clone(),
        relay_config_dir: host.postgres_relay_dir.clone(),
        sqlite_root: host.sqlite_root.clone(),
        live: host.postgres.clone(),
    }
}

fn sqlite_args(args: &Map<String, J>) -> crate::postgresops::SqliteArgs {
    crate::postgresops::SqliteArgs {
        consumer_id: text(args, "consumerId").into(),
        database_name: text(args, "databaseName").into(),
    }
}

/// M8e: caller-owned SQLite databases under the managed root.
fn sqlite_databases(postgres: &crate::postgresops::Postgres) -> Service {
    use crate::tools::structured_result;
    let (ensure, status, remove) = (postgres.clone(), postgres.clone(), postgres.clone());
    Service {
        id: "sqlite-database",
        dependencies: &[],
        operations: vec![
            (
                "ensure_sqlite_database",
                op(move |call| {
                    ensure
                        .ensure_sqlite(&sqlite_args(call.args))
                        .map(|out| structured_result(out, Some("SQLite database provisioned.")))
                }),
            ),
            (
                "get_sqlite_database_status",
                op(move |call| {
                    status
                        .sqlite_status(&sqlite_args(call.args))
                        .map(|out| structured_result(out, Some("SQLite database status returned.")))
                }),
            ),
            (
                "remove_sqlite_database",
                op(move |call| {
                    remove
                        .remove_sqlite(&sqlite_args(call.args), flag(call.args, "confirm"))
                        .map(|out| structured_result(out, Some("SQLite database removed.")))
                }),
            ),
        ],
    }
}

/// `postgresqlServiceRelayArgs`: present whenever `localRelay` is an object.
fn postgresql_relay_args(args: &Map<String, J>) -> Option<crate::postgresops::RelayArgs> {
    let raw = args.get("localRelay")?.as_object()?;
    Some(crate::postgresops::RelayArgs {
        session_id: text(raw, "sessionId").into(),
        listen_host: text(raw, "listenHost").into(),
        listen_port: int_field(raw, "listenPort"),
        target_host: text(raw, "targetHost").into(),
        target_port: int_field(raw, "targetPort"),
        ttl_seconds: int_field(raw, "ttlSeconds"),
        relay_token: text(raw, "relayToken").into(),
        persistent: flag(raw, "persistent"),
        replace_existing: flag(raw, "replaceExisting"),
    })
}

/// `postgresqlServiceArgs`: the VM is the binding's provider instance.
fn postgresql_service_args(call: &Invocation) -> crate::postgresops::ServiceArgs {
    let args = call.args;
    let mut databases = Vec::new();
    for database in string_slice(args, "databases") {
        if !databases.contains(&database) {
            databases.push(database);
        }
    }
    crate::postgresops::ServiceArgs {
        vm_name: call.binding.provider_instance_name(),
        cluster_name: text(args, "clusterName").into(),
        namespace: text(args, "namespace").into(),
        instances: int_field(args, "instances"),
        storage_class: text(args, "storageClass").into(),
        storage_size: text(args, "storageSize").into(),
        retention_policy: text(args, "retentionPolicy").into(),
        restart_consumers: args
            .contains_key("restartConsumers")
            .then(|| flag(args, "restartConsumers")),
        relay: postgresql_relay_args(args),
        databases,
        consumer_secret_name: text(args, "consumerSecretName").into(),
        consumer_secret_label: text(args, "consumerSecretLabel").into(),
        service_owner: text(args, "serviceOwner").into(),
        service_part_of: text(args, "servicePartOf").into(),
        consumer_database_keys: args
            .get("consumerDatabaseKeys")
            .and_then(J::as_object)
            .map(|keys| {
                keys.iter()
                    .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_string())))
                    .collect()
            })
            .unwrap_or_default(),
        relay_device_name: text(args, "relayDeviceName").into(),
    }
}

/// M8e: the CloudNativePG-backed PostgreSQL service on a cluster.
fn postgresql_service(postgres: &crate::postgresops::Postgres) -> Service {
    use crate::tools::structured_result;
    let (reconcile, status, remove) = (postgres.clone(), postgres.clone(), postgres.clone());
    Service {
        id: "postgresql-service",
        dependencies: &["cluster-resources"],
        operations: vec![
            (
                "reconcile_postgresql_service",
                op(move |call| {
                    reconcile
                        .reconcile_service(call.ctx, &postgresql_service_args(call))
                        .map(|out| {
                            structured_result(J::Object(out), Some("PostgreSQL service is ready"))
                        })
                }),
            ),
            (
                "get_postgresql_service_status",
                op(move |call| {
                    status
                        .service_status(&postgresql_service_args(call))
                        .map(|out| {
                            structured_result(
                                J::Object(out),
                                Some("PostgreSQL service status returned"),
                            )
                        })
                }),
            ),
            (
                "remove_postgresql_service",
                op(move |call| {
                    remove
                        .remove_service(&postgresql_service_args(call), flag(call.args, "confirm"))
                        .map(|out| {
                            structured_result(
                                J::Object(out),
                                Some("PostgreSQL service was removed"),
                            )
                        })
                }),
            ),
        ],
    }
}

/// M8e: releasing an authenticated PostgreSQL service relay.
fn postgresql_relay(postgres: &crate::postgresops::Postgres) -> Service {
    use crate::tools::structured_result;
    let postgres = postgres.clone();
    Service {
        id: "postgresql-relay",
        dependencies: &[],
        operations: vec![(
            "release_postgresql_service_relay",
            op(move |call| {
                postgres
                    .release_relay(text(call.args, "sessionId"), text(call.args, "relayToken"))
                    .map(|out| {
                        structured_result(
                            J::Object(out),
                            Some("PostgreSQL service relay was released"),
                        )
                    })
            }),
        )],
    }
}

/// `resourceid.SQLConnectorURI`.
fn sql_connector_uri(tenant: &str, database: &str) -> Result<String, String> {
    crate::resource::Uri::new("sql-connector", tenant, database).map(|uri| uri.to_string())
}

/// M8e: the reference-counted host TCP connector to a database.
fn sql_connector(postgres: &crate::postgresops::Postgres, host: &crate::tools::Host) -> Service {
    use crate::tools::structured_result;
    let (ensure, status, release) = (postgres.clone(), postgres.clone(), postgres.clone());
    let (tenant, status_tenant) = (host.tenant_id.clone(), host.tenant_id.clone());
    Service {
        id: "sql-connector",
        dependencies: &[],
        operations: vec![
            (
                "ensure_sql_connector",
                op(move |call| {
                    let args = crate::postgresops::ConnectorArgs {
                        database_id: text(call.args, "databaseId").into(),
                        target_host: text(call.args, "targetHost").into(),
                        target_port: int_field(call.args, "targetPort"),
                        listen_port: int_field(call.args, "listenPort"),
                        listen_host: text(call.args, "listenHost").into(),
                    };
                    let mut out = ensure.ensure_connector(&args)?;
                    // The connector is the resource this capability
                    // produces; identity is tenant-scoped, so the boundary
                    // names it.
                    let uri = sql_connector_uri(&tenant, &args.database_id)?;
                    out.insert("uri".into(), J::from(uri));
                    let text = format!(
                        "SQL connector {} listening on {}:{}",
                        out["databaseId"].as_str().unwrap_or(""),
                        out["listenHost"].as_str().unwrap_or(""),
                        out["listenPort"]
                    );
                    Ok(structured_result(J::Object(out), Some(&text)))
                }),
            ),
            (
                "get_sql_connector_status",
                op(move |call| {
                    let database = text(call.args, "databaseId");
                    let mut out = status.connector_status(database);
                    if let Ok(uri) = sql_connector_uri(&status_tenant, database) {
                        out.insert("uri".into(), J::from(uri));
                    }
                    let text = if out.get("active") == Some(&J::Bool(true)) {
                        format!(
                            "SQL connector active on {}:{}",
                            out["listenHost"].as_str().unwrap_or(""),
                            out["listenPort"]
                        )
                    } else {
                        "SQL connector inactive".into()
                    };
                    Ok(structured_result(J::Object(out), Some(&text)))
                }),
            ),
            (
                "release_sql_connector",
                op(move |call| {
                    let database = text(call.args, "databaseId");
                    let force = flag(call.args, "force");
                    let released = release.release_connector(database, force)?;
                    let text = if released {
                        format!("Released SQL connector {database}")
                    } else {
                        format!("No SQL connector for {database}")
                    };
                    Ok(structured_result(
                        serde_json::json!({"released": released, "databaseId": database}),
                        Some(&text),
                    ))
                }),
            ),
        ],
    }
}

/// M8e: a single-instance PostgreSQL in a cluster namespace, and SQL run in
/// its primary.
fn standalone_postgresql(postgres: &crate::postgresops::Postgres) -> Service {
    use crate::tools::structured_result;
    let (install, status, delete, sql) = (
        postgres.clone(),
        postgres.clone(),
        postgres.clone(),
        postgres.clone(),
    );
    Service {
        id: "standalone-postgresql",
        dependencies: &["cluster-resources"],
        operations: vec![
            (
                "install_postgresql",
                op(move |call| {
                    let args = crate::postgresops::InstallArgs {
                        vm_name: call.binding.provider_instance_name(),
                        namespace: text(call.args, "namespace").into(),
                        database: text(call.args, "database").into(),
                    };
                    install
                        .install(call.ctx, &args)
                        .map(|out| structured_result(J::Object(out), Some("PostgreSQL is ready.")))
                }),
            ),
            (
                "get_postgresql_status",
                op(move |call| {
                    status
                        .standalone_status(
                            &call.binding.provider_instance_name(),
                            text(call.args, "namespace"),
                        )
                        .map(map_result)
                }),
            ),
            (
                "delete_postgresql",
                op(move |call| {
                    delete
                        .delete_standalone(
                            &call.binding.provider_instance_name(),
                            text(call.args, "namespace"),
                        )
                        .map(|out| structured_result(J::Object(out), Some("PostgreSQL deleted.")))
                }),
            ),
            (
                "run_sql",
                op(move |call| {
                    sql.run_sql(
                        &call.binding.provider_instance_name(),
                        text(call.args, "namespace"),
                        text(call.args, "database"),
                        text(call.args, "sql"),
                    )
                    .map(|out| structured_result(J::Object(out), Some("SQL completed.")))
                }),
            ),
        ],
    }
}

/// M8d: the host-side image builder.
fn oci_builder(oci: &crate::ociops::Oci) -> Service {
    use crate::tools::structured_result;
    let oci = oci.clone();
    Service {
        id: "oci-builder",
        dependencies: &[],
        operations: vec![(
            "ensure_oci_builder",
            op(move |call| {
                oci.ensure_builder(text(call.args, "builder")).map(|out| {
                    structured_result(J::Object(out), Some("Host OCI image builder is available."))
                })
            }),
        )],
    }
}

/// M8d: a caller-provided build context written under an allowlisted root.
fn build_context(oci: &crate::ociops::Oci) -> Service {
    use crate::tools::structured_result;
    let oci = oci.clone();
    Service {
        id: "build-context",
        dependencies: &[],
        operations: vec![(
            "stage_build_context",
            op(move |call| {
                let files = call
                    .args
                    .get("files")
                    .and_then(J::as_object)
                    .map(|raw| {
                        raw.iter()
                            .filter_map(|(k, v)| v.as_str().map(|t| (k.clone(), t.to_string())))
                            .collect()
                    })
                    .unwrap_or_default();
                let args = crate::ociops::StageArgs {
                    dest_dir: text(call.args, "destDir").into(),
                    files,
                    file_encoding: text(call.args, "fileEncoding").into(),
                };
                oci.stage_build_context(&args).map(|out| {
                    let message = format!("Staged {} files into build context.", out["fileCount"]);
                    structured_result(J::Object(out), Some(&message))
                })
            }),
        )],
    }
}

/// M8d: the local container storage budget, its inspection and cleanup.
fn container_storage(oci: &crate::ociops::Oci) -> Service {
    use crate::tools::structured_result;
    let (configure, inspect, cleanup) = (oci.clone(), oci.clone(), oci.clone());
    Service {
        id: "container-storage",
        dependencies: &[],
        operations: vec![
            (
                "configure_oci_storage",
                op(move |call| {
                    configure
                        .configure_storage(
                            text(call.args, "runtime"),
                            optional_int64(call.args, "maxBytes"),
                            optional_int64(call.args, "minAgeSeconds"),
                            flag(call.args, "pruneNow"),
                        )
                        .map(|out| {
                            structured_result(
                                J::Object(out),
                                Some("OCI image storage retention policy applied."),
                            )
                        })
                }),
            ),
            (
                "inspect_container_storage",
                op(move |call| {
                    inspect
                        .inspect_storage(text(call.args, "runtime"))
                        .map(|out| {
                            structured_result(
                                J::Object(out),
                                Some("Container runtime storage usage inspected."),
                            )
                        })
                }),
            ),
            (
                "cleanup_container_storage",
                op(move |call| {
                    cleanup
                        .cleanup_storage(
                            text(call.args, "runtime"),
                            optional_int64(call.args, "maxBytes"),
                            optional_int64(call.args, "minAgeSeconds"),
                            flag(call.args, "dryRun"),
                        )
                        .map(|out| {
                            structured_result(
                                J::Object(out),
                                Some("Container runtime storage cleanup completed."),
                            )
                        })
                }),
            ),
        ],
    }
}

/// M8d: build an image from a staged context and push it.
fn image_build(oci: &crate::ociops::Oci) -> Service {
    use crate::tools::structured_result;
    let oci = oci.clone();
    Service {
        id: "image-build",
        dependencies: &["oci-builder", "container-storage"],
        operations: vec![(
            "build_and_push_oci_image",
            op(move |call| {
                let build_args = call
                    .args
                    .get("buildArgs")
                    .and_then(J::as_object)
                    .map(|raw| {
                        raw.iter()
                            .filter(|(_, v)| v.is_string())
                            .map(|(k, v)| (k.clone(), v.clone()))
                            .collect()
                    })
                    .unwrap_or_default();
                let args = crate::ociops::BuildArgs {
                    context_dir: text(call.args, "contextDir").into(),
                    dockerfile: text(call.args, "dockerfile").into(),
                    image: text(call.args, "image").into(),
                    builder: text(call.args, "builder").into(),
                    insecure_registry: flag(call.args, "insecureRegistry"),
                    platform: text(call.args, "platform").into(),
                    build_args,
                };
                oci.build_and_push(&args).map(|out| {
                    let message = format!(
                        "Built and pushed {}.",
                        out.get("image").and_then(J::as_str).unwrap_or("")
                    );
                    structured_result(J::Object(out), Some(&message))
                })
            }),
        )],
    }
}

/// M8c: the cluster agent, installed in a guest or on this host.
fn cluster_agent(agent: &crate::clusterops::ClusterAgent) -> Service {
    let agent = agent.clone();
    Service {
        id: "cluster-agent",
        dependencies: &["instance-lifecycle"],
        operations: vec![(
            "install_cluster_agent",
            op(move |call| {
                let args = crate::clusterops::AgentArgs {
                    vm_name: text(call.args, "vmName").into(),
                    cluster_id: text(call.args, "clusterId").into(),
                    cluster_name: text(call.args, "clusterName").into(),
                    agent_id: text(call.args, "agentId").into(),
                    bridge_token: text(call.args, "bridgeToken").into(),
                    bridge_url: text(call.args, "bridgeUrl").into(),
                    bridge_port: int_field(call.args, "bridgePort"),
                    api_endpoint: text(call.args, "apiEndpoint").into(),
                    provider_id: text(call.args, "providerId").into(),
                    resource_id: text(call.args, "resourceId").into(),
                    source: text(call.args, "source").into(),
                };
                agent.install(&args).map(|out| {
                    crate::tools::structured_result(
                        out,
                        Some("Kubernetes cluster agent installed."),
                    )
                })
            }),
        )],
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
