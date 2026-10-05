//! The execution admission boundary (`resolveExecutionBinding` +
//! `admitInvocationWithDescriptor`).
//!
//! Every capability call crosses two gates before its handler runs:
//!
//! 1. **Resource binding.** Each argument the descriptor declares in
//!    `requires` must carry a canonical URI that resolves, inside the active
//!    tenant, to an active registry record. A record the registry has never
//!    seen is adopted only when the owning domain confirms it exists. The
//!    handler receives provider-native coordinates from the binding and never
//!    reads a provider name out of the raw arguments.
//! 2. **Host resource admission.** The typed cost (registered class, then the
//!    descriptor's `resourceCost`, then argument-bound overrides) is admitted
//!    by the host-wide coordinator, which may refuse on pressure, unverified
//!    enforcement or saturation. The returned reservation is released when
//!    the invocation ends.
//!
//! Refusals render exactly as Go's `tools.ErrorResult` does.

use crate::catalog;
use crate::goerr::quote;
use crate::resource::{self, AdmissionRequest, AdmitError, Reservation, Uri};
use crate::store::ResourceRecord;
use crate::tools::Host;
use crate::transport::Server;
use serde_json::{json, Map, Value as J};
use std::collections::BTreeMap;

/// `tools.BoundResource`.
#[derive(Clone, Debug)]
pub struct Bound {
    pub argument: String,
    pub uri: String,
    pub coordinates: Map<String, J>,
}

/// `tools.ExecutionBinding`, reduced to what handlers and admission read.
#[derive(Clone, Debug, Default)]
pub struct Binding {
    pub resources: Vec<Bound>,
}

impl Binding {
    /// `ExecutionBinding.Coordinate`: the first non-nil, non-blank value.
    pub fn coordinate(&self, name: &str) -> Option<&J> {
        if name.is_empty() {
            return None;
        }
        self.resources
            .iter()
            .find_map(|r| match r.coordinates.get(name) {
                None | Some(J::Null) => None,
                Some(J::String(s)) if s.trim().is_empty() => None,
                Some(v) => Some(v),
            })
    }

    /// `ExecutionBinding.ProviderInstanceName`.
    pub fn provider_instance_name(&self) -> String {
        self.coordinate("providerInstanceName")
            .and_then(J::as_str)
            .map(|s| s.trim().to_string())
            .unwrap_or_default()
    }

    /// `firstBoundURI`.
    fn first_uri(&self) -> String {
        self.resources
            .iter()
            .find(|r| !r.uri.trim().is_empty())
            .map(|r| r.uri.clone())
            .unwrap_or_default()
    }
}

/// Why an invocation was not admitted, typed the way `tools.ErrorResult`
/// distinguishes errors.
#[derive(Clone, Debug)]
pub enum Refusal {
    /// `tools.CapabilityError`.
    Capability {
        owner: &'static str,
        code: &'static str,
        message: String,
    },
    /// `resource.RequestError` / `resource.AdmissionError` / untyped.
    Admit(AdmitError),
    /// A plain `error`.
    Plain(String),
}

impl Refusal {
    fn binding(message: String) -> Refusal {
        Refusal::Capability {
            owner: "admission",
            code: "resource_binding",
            message,
        }
    }

    pub fn message(&self) -> String {
        match self {
            Refusal::Capability { message, .. } | Refusal::Plain(message) => message.clone(),
            Refusal::Admit(e) => e.to_string(),
        }
    }

    /// `tools.ErrorResult(err)`.
    pub fn render(&self) -> J {
        let text = json!([{"type": "text", "text": format!("Error: {}", self.message())}]);
        let structured = match self {
            Refusal::Capability {
                owner,
                code,
                message,
            } => json!({"code": code, "owner": owner, "message": message}),
            Refusal::Admit(AdmitError::Request {
                code,
                field,
                reason,
            }) => json!({"code": code, "field": field, "reason": reason, "owner": "admission"}),
            Refusal::Admit(AdmitError::Admission {
                code,
                class,
                pressure,
                reason,
                retry_after_ms,
            }) => json!({
                "code": code, "class": class, "pressure": pressure, "reason": reason,
                "retryAfterMs": retry_after_ms, "owner": "admission",
            }),
            Refusal::Admit(AdmitError::Other(_)) | Refusal::Plain(_) => {
                return json!({"content": text, "isError": true});
            }
        };
        json!({"content": text, "structuredContent": structured, "isError": true})
    }
}

/// `argumentValue`: a dotted path through nested objects.
fn argument_value<'a>(args: &'a Map<String, J>, path: &str) -> Option<&'a J> {
    if path.trim().is_empty() {
        return None;
    }
    let mut segments = path.split('.');
    let mut current = args.get(segments.next()?)?;
    for segment in segments {
        current = current.as_object()?.get(segment)?;
    }
    Some(current)
}

fn registry_lookup(host: &Host, uri: &Uri) -> Result<Option<ResourceRecord>, String> {
    let store = host
        .state
        .lock()
        .map_err(|_| "state store unavailable".to_string())?;
    store.get_resource(uri)
}

/// `Shared.ResolveResource` with the Host service's adopter.
pub fn resolve_resource(
    host: &Host,
    uri: &str,
    want_type: &str,
) -> Result<(Uri, Map<String, J>), String> {
    let parsed = Uri::parse(uri)?;
    let tenant = host.tenant_id.trim();
    if !tenant.is_empty() && parsed.tenant_id != tenant {
        return Err(format!(
            "resource URI belongs to a different tenant: active tenant {}",
            quote(tenant)
        ));
    }
    if !want_type.is_empty() && parsed.resource_type != want_type {
        return Err(format!(
            "invalid resource URI: expected {}, got {}",
            quote(want_type),
            quote(&parsed.resource_type)
        ));
    }
    let mut record =
        registry_lookup(host, &parsed).map_err(|e| format!("resolve resource {parsed}: {e}"))?;
    if !matches!(&record, Some((status, _)) if status == "active") {
        if let Some(coordinates) = adopt(host, &parsed)? {
            resource::register(
                &host.state,
                &host.tenant_id,
                &parsed.to_string(),
                &coordinates,
            )?;
            record = registry_lookup(host, &parsed)?;
        }
    }
    match record {
        Some((status, coordinates)) if status == "active" => Ok((parsed, coordinates)),
        _ => Err(format!("resource not found: {parsed}")),
    }
}

/// `Service.adoptResource`: ask the owning domain whether a resource the
/// registry has never seen really exists. Only Incus instances are adoptable
/// in this build; cluster and host-service adoption belong to the Kubernetes
/// and host-service domains (M6), so those URIs fail closed instead.
fn adopt(host: &Host, parsed: &Uri) -> Result<Option<Map<String, J>>, String> {
    match parsed.resource_type.as_str() {
        "vm" | "container" => {
            let register = |uri: &str, coordinates: Map<String, J>| {
                let _ = resource::register(&host.state, &host.tenant_id, uri, &coordinates);
            };
            let Ok(info) = host.incus.get_vm_info(&parsed.resource_id, true, &register) else {
                return Ok(None);
            };
            let name = info.get("name").and_then(J::as_str).unwrap_or("");
            if name.trim().is_empty() {
                return Ok(None);
            }
            let kind = info.get("type").and_then(J::as_str).unwrap_or("");
            let actual = if kind.eq_ignore_ascii_case("vm") {
                "vm"
            } else {
                "container"
            };
            if parsed.resource_type != actual {
                return Err(format!(
                    "resource type mismatch: {parsed} resolves to {actual}"
                ));
            }
            let mut coordinates = Map::new();
            coordinates.insert("providerInstanceName".into(), J::from(name));
            coordinates.insert("displayName".into(), J::from(name));
            coordinates.insert("instanceType".into(), J::from(kind));
            Ok(Some(coordinates))
        }
        "cluster" => Err(format!(
            "adopt cluster {parsed}: cluster adoption is not implemented by this Host Agent build"
        )),
        "host-service" => {
            let valid = parsed
                .resource_id
                .split_once('/')
                .is_some_and(|(scope, service)| {
                    matches!(scope, "user" | "system") && !service.trim().is_empty()
                });
            if !valid {
                return Err(format!(
                    "host-service URI must use <scope>/<service-name>: {parsed}"
                ));
            }
            Err(format!(
                "adopt host service {parsed}: host service adoption is not implemented by this Host Agent build"
            ))
        }
        _ => Ok(None),
    }
}

/// `resolveExecutionBindingWithSnapshot`.
pub fn resolve_binding(
    server: &Server,
    name: &str,
    args: &Map<String, J>,
) -> Result<Binding, Refusal> {
    let mut binding = Binding::default();
    let Some(descriptor) = server.catalog.tools.iter().find(|d| d.name == name) else {
        return Ok(binding);
    };
    // Declared bindings are authoritative for their argument; there is no
    // generic `uri` fallback. Arguments resolve in sorted order.
    let mut by_argument: BTreeMap<&str, Vec<&catalog::Binding>> = BTreeMap::new();
    for require in &descriptor.requires {
        let argument = require.argument.trim();
        if !argument.is_empty() {
            by_argument.entry(argument).or_default().push(require);
        }
    }
    for (argument, requires) in by_argument {
        let uri = match argument_value(args, argument) {
            Some(J::String(s)) if !s.trim().is_empty() => s.clone(),
            _ => {
                if requires.iter().any(|r| r.required) {
                    return Err(Refusal::binding(if argument == "uri" {
                        format!("{name} requires canonical resource uri")
                    } else {
                        format!(
                            "{name} requires canonical resource argument {}",
                            quote(argument)
                        )
                    }));
                }
                continue;
            }
        };
        let mut last = String::new();
        let mut resolved = None;
        for require in &requires {
            match resolve_resource(&server.host, &uri, &require.resource_type) {
                Ok(found) => {
                    resolved = Some(found);
                    break;
                }
                Err(e) => last = e,
            }
        }
        let Some((parsed, coordinates)) = resolved else {
            return Err(Refusal::binding(format!(
                "resolve {name} argument {}: {last}",
                quote(argument)
            )));
        };
        binding.resources.push(Bound {
            argument: argument.to_string(),
            uri: parsed.to_string(),
            coordinates,
        });
    }
    Ok(binding)
}

fn string_argument(args: &Map<String, J>, key: &str) -> String {
    args.get(key)
        .and_then(J::as_str)
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// `admitInvocationWithDescriptor`. `identity` is the operation identity the
/// invocation context carries (`WithOperationIdentity`); an async task runs
/// with (tool name, task id). `parent`, when given, is the reservation
/// already held in the current call chain (`resource.ReservationFromContext`)
/// -- a plan node dispatched under its launcher's reservation inherits it
/// instead of admitting independently.
pub fn admit(
    server: &Server,
    name: &str,
    args: &Map<String, J>,
    binding: &Binding,
    identity: Option<(&str, &str)>,
    parent: Option<&Reservation>,
) -> Result<Reservation, Refusal> {
    let registered = catalog::admission_class(name);
    let declared = registered.is_some();
    let class = registered.unwrap_or_else(|| "control".into());
    let mut cost = resource::default_cost_for_class(&class);
    let descriptor = server.catalog.tools.iter().find(|d| d.name == name);
    let found = descriptor.is_some();
    let effect_read = descriptor.is_some_and(|d| d.effect == "read");
    let provider = descriptor.is_some_and(|d| d.implementation.trim().starts_with("provider:"));
    if let Some(declared_cost) = descriptor.and_then(|d| d.resource_cost.as_ref()) {
        let mut declared_class = class.clone();
        if !declared_cost.class.is_empty() {
            if !matches!(declared_cost.class.trim(), "control" | "normal" | "heavy") {
                return Err(Refusal::Plain(format!(
                    "resource_declaration_invalid: capability {} declares unknown resource class {}",
                    quote(name),
                    quote(&declared_cost.class)
                )));
            }
            // Go validates the trimmed class but carries the raw value; the
            // coordinator then rejects a padded one.
            declared_class = declared_cost.class.clone();
        }
        cost = AdmissionRequest {
            class: declared_class,
            cpu_cores: declared_cost.cpu_cores,
            memory_bytes: declared_cost.memory_bytes,
            disk_bytes: declared_cost.disk_bytes,
            tasks: declared_cost.tasks,
            ..Default::default()
        };
        if let Some(bindings) = &declared_cost.argument_bindings {
            cost = resource::resolve_argument_cost(cost, args, bindings).map_err(Refusal::Admit)?;
        }
    } else if found && provider && !effect_read {
        return Err(Refusal::Plain(format!(
            "resource_declaration_required: provider workload capability {} must declare typed resourceCost metadata",
            quote(name)
        )));
    } else if found && provider {
        cost = resource::default_cost_for_class("control");
    } else if found && !effect_read && !declared {
        return Err(Refusal::Plain(format!(
            "resource_declaration_required: capability {} must declare typed resourceCost metadata",
            quote(name)
        )));
    } else if !found && !declared {
        return Err(Refusal::Plain(format!(
            "resource_declaration_required: operation {} is not registered with a typed resource cost",
            quote(name)
        )));
    }
    cost.operation = name.to_string();
    cost.agent_id = server.agent_id.clone();
    cost.operation_id = string_argument(args, "operationId");
    cost.task_id = string_argument(args, "taskId");
    if cost.operation_id.is_empty() && cost.task_id.is_empty() {
        if let Some((operation, task)) = identity {
            cost.operation_id = operation.to_string();
            cost.task_id = task.to_string();
        }
    }
    cost.resource_uri = binding.first_uri();
    if let Some(parent) = parent {
        cost.parent_reservation_id = parent.id.clone();
    }
    server
        .host
        .coordinator
        .admit(cost, parent)
        .map_err(Refusal::Admit)
}

/// The deferred `admission.Release`; Go ignores its error.
pub fn release(server: &Server, reservation: &Reservation) {
    let _ = server.host.coordinator.release(reservation);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coordinates_skip_blank_values() {
        let mut first = Map::new();
        first.insert("providerInstanceName".into(), J::from("  "));
        let mut second = Map::new();
        second.insert("providerInstanceName".into(), J::from(" web-1 "));
        let binding = Binding {
            resources: vec![
                Bound {
                    argument: "uri".into(),
                    uri: String::new(),
                    coordinates: first,
                },
                Bound {
                    argument: "uri".into(),
                    uri: "vm:t:web-1".into(),
                    coordinates: second,
                },
            ],
        };
        assert_eq!(binding.provider_instance_name(), "web-1");
        assert_eq!(binding.first_uri(), "vm:t:web-1");
    }

    #[test]
    fn refusals_render_like_error_result() {
        let r = Refusal::binding("start_vm requires canonical resource uri".into());
        assert_eq!(
            r.render(),
            json!({
                "content": [{"type": "text", "text": "Error: start_vm requires canonical resource uri"}],
                "structuredContent": {"code": "resource_binding", "owner": "admission",
                    "message": "start_vm requires canonical resource uri"},
                "isError": true,
            })
        );
        let r = Refusal::Admit(AdmitError::Admission {
            code: "host_capacity_saturated".into(),
            class: "normal".into(),
            pressure: "normal".into(),
            reason: "declared resource cost exceeds effective host capacity".into(),
            retry_after_ms: 1000,
        });
        assert_eq!(r.render()["structuredContent"]["retryAfterMs"], json!(1000));
        assert_eq!(
            r.render()["content"][0]["text"],
            json!("Error: host_capacity_saturated: class=normal pressure=normal reason=declared resource cost exceeds effective host capacity retryAfterMs=1000")
        );
        let r = Refusal::Plain("boom".into());
        assert_eq!(
            r.render(),
            json!({"content": [{"type": "text", "text": "Error: boom"}], "isError": true})
        );
    }

    #[test]
    fn argument_paths_walk_objects() {
        let args: Map<String, J> =
            serde_json::from_value(json!({"a": {"b": "x"}, "uri": "vm:t:x"})).unwrap();
        assert_eq!(argument_value(&args, "a.b"), Some(&J::from("x")));
        assert_eq!(argument_value(&args, "uri.b"), None);
        assert_eq!(argument_value(&args, " "), None);
    }
}
