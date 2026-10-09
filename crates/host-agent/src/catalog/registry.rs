//! Executable catalog ownership from internal/catalog/registry.go.
//! Schema compilation is an explicit dependency, never an implicit success.
use super::{build_from_descriptors, Binding, Descriptor, Snapshot};
use crate::{goerr::quote, plan::runner::RunCtx};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

type Invocation =
    dyn Fn(&RunCtx, &Map<String, Value>) -> Result<Map<String, Value>, String> + Send + Sync;
type SchemaCheck = dyn Fn(&str, &Map<String, Value>) -> Result<(), String> + Send + Sync;

/// A resolved reference retains the exact executable and immutable descriptor.
#[derive(Clone)]
pub struct Capability {
    pub descriptor: Descriptor,
    pub invoke: Arc<Invocation>,
}
#[derive(Clone)]
pub struct Registration {
    pub descriptor: Descriptor,
    pub provider_id: String,
    pub implementation: String,
    pub capability: Option<Arc<Capability>>,
}
#[derive(Default)]
pub struct Options {
    pub authorized_providers: BTreeMap<String, bool>,
    pub known_resource_kinds: BTreeMap<String, bool>,
    pub allowed_implementations: BTreeMap<String, bool>,
}
struct State {
    base: Snapshot,
    overlays: BTreeMap<String, Registration>,
    authorized: BTreeSet<String>,
}
pub struct Registry {
    state: Mutex<State>,
    known: BTreeMap<String, bool>,
    allowed: BTreeMap<String, bool>,
    check_schema: Arc<SchemaCheck>,
}
fn provider(id: &str) -> String {
    id.trim().to_lowercase()
}
fn normalize(mut r: Registration) -> Registration {
    if r.descriptor.version == 0 {
        r.descriptor.version = 1;
    }
    r
}
fn registration(
    value: Option<Arc<Capability>>,
    id: &str,
    implementation: &str,
) -> Result<Registration, String> {
    let value = value.ok_or("capability implementation is required")?;
    let mut descriptor = value.descriptor.clone();
    if descriptor.version < 1 {
        return Err(format!(
            "capability {} must declare a positive version",
            quote(&descriptor.operation_id)
        ));
    }
    if descriptor.implementation.is_empty() {
        descriptor.implementation = implementation.into();
    }
    if descriptor.provider.is_empty() {
        descriptor.provider = id.into();
    }
    Ok(Registration {
        descriptor,
        provider_id: id.into(),
        implementation: implementation.into(),
        capability: Some(value),
    })
}
impl Registry {
    pub fn new(base: Snapshot, options: Options, check_schema: Arc<SchemaCheck>) -> Self {
        let mut authorized = BTreeSet::from(["incus".into()]);
        authorized.extend(
            options
                .authorized_providers
                .into_iter()
                .filter(|(_, enabled)| *enabled)
                .map(|(id, _)| provider(&id)),
        );
        Self {
            state: Mutex::new(State {
                base,
                overlays: BTreeMap::new(),
                authorized,
            }),
            known: options.known_resource_kinds,
            allowed: options.allowed_implementations,
            check_schema,
        }
    }
    pub fn snapshot(&self) -> Snapshot {
        let state = self.state.lock().unwrap();
        let mut descriptors = state.base.tools.clone();
        descriptors.extend(state.overlays.values().map(|r| r.descriptor.clone()));
        descriptors.sort_by(|a, b| a.operation_id.cmp(&b.operation_id));
        build_from_descriptors("incus", &descriptors)
    }
    pub fn validate_base(&self) -> Result<(), String> {
        let state = self.state.lock().unwrap();
        for descriptor in &state.base.tools {
            self.validate_descriptor(descriptor)?;
        }
        Ok(())
    }
    pub fn authorize_provider(&self, id: &str) -> Result<(), String> {
        let id = provider(id);
        if id.is_empty() {
            return Err("provider id is required".into());
        }
        self.state.lock().unwrap().authorized.insert(id);
        Ok(())
    }
    pub fn register(&self, value: Option<Arc<Capability>>) -> Result<(), String> {
        let value = value.ok_or("capability implementation is required")?;
        self.register_registration(Registration {
            descriptor: value.descriptor.clone(),
            provider_id: value.descriptor.provider.clone(),
            implementation: value.descriptor.implementation.clone(),
            capability: Some(value),
        })
    }
    pub fn register_registration(&self, r: Registration) -> Result<(), String> {
        let r = normalize(r);
        self.validate(&r)?;
        let mut state = self.state.lock().unwrap();
        let id = &r.descriptor.operation_id;
        if Self::base_conflict(&state, id) {
            return Err(format!(
                "capability {} conflicts with the base catalog",
                quote(id)
            ));
        }
        if state.overlays.contains_key(id) {
            return Err(format!("capability {} is already registered", quote(id)));
        }
        state.overlays.insert(id.clone(), r);
        Ok(())
    }
    pub fn upsert(&self, r: Registration) -> Result<(), String> {
        let r = normalize(r);
        self.validate(&r)?;
        let mut state = self.state.lock().unwrap();
        let id = &r.descriptor.operation_id;
        if Self::base_conflict(&state, id) {
            return Err(format!(
                "capability {} conflicts with the base catalog",
                quote(id)
            ));
        }
        if let Some(existing) = state.overlays.get(id) {
            if existing.provider_id != r.provider_id || existing.implementation != r.implementation
            {
                return Err(format!(
                    "capability {} is already registered by another provider",
                    quote(id)
                ));
            }
            if r.descriptor.version < existing.descriptor.version {
                return Err(format!(
                    "capability {} cannot be downgraded from version {} to {}",
                    quote(id),
                    existing.descriptor.version,
                    r.descriptor.version
                ));
            }
        }
        state.overlays.insert(id.clone(), r);
        Ok(())
    }
    pub fn register_capability(
        &self,
        value: Option<Arc<Capability>>,
        id: &str,
        implementation: &str,
    ) -> Result<(), String> {
        self.register_registration(registration(value, id, implementation)?)
    }
    pub fn upsert_capability(
        &self,
        value: Option<Arc<Capability>>,
        id: &str,
        implementation: &str,
    ) -> Result<(), String> {
        self.upsert(registration(value, id, implementation)?)
    }
    pub fn replace_generation(
        &self,
        generation: &str,
        values: &[Option<Arc<Capability>>],
    ) -> Result<(), String> {
        if generation.trim().is_empty() {
            return Err("generation id is required".into());
        }
        let mut registrations = Vec::new();
        let mut seen = BTreeSet::new();
        for value in values {
            let value = value
                .as_ref()
                .ok_or("generation contains a nil capability")?;
            let d = &value.descriptor;
            if d.generation_id != generation {
                return Err(format!(
                    "capability {} does not belong to generation {}",
                    quote(&d.operation_id),
                    quote(generation)
                ));
            }
            if !seen.insert(d.operation_id.clone()) {
                return Err(format!(
                    "generation contains duplicate capability {}",
                    quote(&d.operation_id)
                ));
            }
            registrations.push(normalize(Registration {
                descriptor: d.clone(),
                provider_id: d.provider.clone(),
                implementation: d.implementation.clone(),
                capability: Some(value.clone()),
            }));
        }
        for r in &registrations {
            self.validate(r)?;
        }
        let mut state = self.state.lock().unwrap();
        let id = registrations
            .first()
            .map(|r| r.provider_id.as_str())
            .unwrap_or("");
        for r in registrations.iter().skip(1) {
            if r.provider_id != id {
                return Err(format!(
                    "generation {} contains capabilities from multiple providers: {} and {}",
                    quote(generation),
                    quote(id),
                    quote(&r.provider_id)
                ));
            }
        }
        let remove: BTreeSet<String> = state
            .overlays
            .iter()
            .filter(|(_, r)| {
                r.descriptor.generation_id == generation || (!id.is_empty() && r.provider_id == id)
            })
            .map(|(name, _)| name.clone())
            .collect();
        for r in &registrations {
            let name = &r.descriptor.operation_id;
            if Self::base_conflict(&state, name) {
                return Err(format!(
                    "capability {} conflicts with the base catalog",
                    quote(name)
                ));
            }
            if state.overlays.contains_key(name) && !remove.contains(name) {
                return Err(format!(
                    "capability {} is already registered by another overlay",
                    quote(name)
                ));
            }
        }
        for name in remove {
            state.overlays.remove(&name);
        }
        for r in registrations {
            state.overlays.insert(r.descriptor.operation_id.clone(), r);
        }
        Ok(())
    }
    pub fn replace_provider(
        &self,
        id: &str,
        values: &[Option<Arc<Capability>>],
    ) -> Result<(), String> {
        let id = provider(id);
        if id.is_empty() {
            return Err("provider id is required".into());
        }
        let mut registrations = Vec::new();
        let mut seen = BTreeSet::new();
        for value in values {
            let value = value
                .as_ref()
                .ok_or_else(|| format!("provider {} contains a nil capability", quote(&id)))?;
            let r = registration(Some(value.clone()), &id, &value.descriptor.implementation)?;
            if provider(&r.descriptor.provider) != id {
                return Err(format!(
                    "capability {} belongs to provider {}, not {}",
                    quote(&r.descriptor.operation_id),
                    quote(&r.descriptor.provider),
                    quote(&id)
                ));
            }
            if !seen.insert(r.descriptor.operation_id.clone()) {
                return Err(format!(
                    "provider {} contains duplicate capability {}",
                    quote(&id),
                    quote(&r.descriptor.operation_id)
                ));
            }
            registrations.push(normalize(r));
        }
        for r in &registrations {
            self.validate(r)?;
        }
        let mut state = self.state.lock().unwrap();
        let remove: BTreeSet<String> = state
            .overlays
            .iter()
            .filter(|(_, r)| provider(&r.provider_id) == id)
            .map(|(name, _)| name.clone())
            .collect();
        for r in &registrations {
            let name = &r.descriptor.operation_id;
            if Self::base_conflict(&state, name) {
                return Err(format!(
                    "capability {} conflicts with the base catalog",
                    quote(name)
                ));
            }
            if state.overlays.contains_key(name) && !remove.contains(name) {
                return Err(format!(
                    "capability {} is already registered by another provider",
                    quote(name)
                ));
            }
        }
        for name in remove {
            state.overlays.remove(&name);
        }
        for r in registrations {
            state.overlays.insert(r.descriptor.operation_id.clone(), r);
        }
        Ok(())
    }
    pub fn resolve_capability(&self, id: &str) -> Option<Arc<Capability>> {
        self.state
            .lock()
            .unwrap()
            .overlays
            .get(id)
            .and_then(|r| r.capability.clone())
    }
    pub fn resolve(&self, id: &str, revision: &str) -> Result<Arc<Capability>, String> {
        if revision.trim().is_empty() {
            return Err("catalog revision is required".into());
        }
        let current = self.snapshot().revision;
        if current != revision {
            return Err(format!(
                "catalog revision {} is stale; current revision is {}",
                quote(revision),
                quote(&current)
            ));
        }
        self.resolve_capability(id)
            .ok_or_else(|| format!("capability {} is not registered", quote(id)))
    }
    pub fn unregister(&self, id: &str) -> Result<(), String> {
        self.state
            .lock()
            .unwrap()
            .overlays
            .remove(id)
            .map(|_| ())
            .ok_or_else(|| format!("capability {} is not an overlay", quote(id)))
    }
    pub fn displace_capability_families(
        &self,
        owner: &str,
        families: &[String],
    ) -> Option<Vec<String>> {
        let owner = provider(owner);
        let families: BTreeSet<&str> = families
            .iter()
            .map(|f| f.trim())
            .filter(|f| !f.is_empty())
            .collect();
        if owner.is_empty() || families.is_empty() {
            return None;
        }
        let mut state = self.state.lock().unwrap();
        let remove: Vec<String> = state
            .overlays
            .iter()
            .filter(|(_, r)| {
                families.contains(r.descriptor.capability_id.trim())
                    && provider(&r.provider_id) != owner
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in &remove {
            state.overlays.remove(id);
        }
        Some(remove)
    }
    fn base_conflict(state: &State, id: &str) -> bool {
        state.base.tools.iter().any(|d| d.operation_id == id)
    }
    fn validate(&self, r: &Registration) -> Result<(), String> {
        let d = &r.descriptor;
        let value = r.capability.as_ref().ok_or_else(|| {
            format!(
                "capability {} must provide an executable validation boundary",
                quote(&d.operation_id)
            )
        })?;
        if value.descriptor.operation_id != d.operation_id || value.descriptor.name != d.name {
            return Err(format!(
                "capability {} descriptor does not match its registration",
                quote(&d.operation_id)
            ));
        }
        let name = &d.operation_id;
        if name != &d.name
            || name.is_empty()
            || name.len() > 128
            || !name.as_bytes()[0].is_ascii_lowercase()
            || !name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_.-".contains(&b))
        {
            return Err("operationId and name must be the same valid operation identifier".into());
        }
        let id = provider(&r.provider_id);
        if id.is_empty() || !self.state.lock().unwrap().authorized.contains(&id) {
            return Err("registration provider does not match the authorized provider".into());
        }
        if r.implementation.trim().is_empty() {
            return Err("registration must name an installed implementation".into());
        }
        if d.implementation.trim().is_empty() || d.implementation != r.implementation {
            return Err(format!(
                "capability {} implementation must match the installed implementation",
                quote(&d.operation_id)
            ));
        }
        if !self.allowed.is_empty()
            && !self
                .allowed
                .get(&r.implementation)
                .copied()
                .unwrap_or(false)
        {
            return Err(format!(
                "registration implementation {} is not authorized",
                quote(&r.implementation)
            ));
        }
        if !d.provider.is_empty() && provider(&d.provider) != id {
            return Err("descriptor provider does not match the authorized provider".into());
        }
        if !d.input_schema.is_object() || d.output_schema.as_ref().is_none_or(|s| s.is_null()) {
            return Err(format!(
                "capability {} must provide input and output schemas",
                quote(&d.operation_id)
            ));
        }
        self.validate_descriptor(d)
    }
    fn validate_descriptor(&self, d: &Descriptor) -> Result<(), String> {
        let (Some(input), Some(output)) = (
            d.input_schema.as_object(),
            d.output_schema.as_ref().and_then(Value::as_object),
        ) else {
            if !d.requires.is_empty() || !d.produces.is_empty() || !d.resource_kinds.is_empty() {
                return Err(format!(
                    "capability {} has typed metadata but is missing input or output schemas",
                    quote(&d.operation_id)
                ));
            }
            return Ok(());
        };
        if input.get("type").and_then(Value::as_str) != Some("object") {
            return Err(format!(
                "capability {} input schema must declare type object",
                quote(&d.operation_id)
            ));
        }
        (self.check_schema)(&format!("{} input", d.operation_id), input)?;
        (self.check_schema)(&format!("{} output", d.operation_id), output)?;
        self.validate_cost(d, input)?;
        self.validate_selectors(d, output).map_err(|e| {
            format!(
                "capability {} result selectors: {e}",
                quote(&d.operation_id)
            )
        })?;
        if d.version < 1 {
            return Err(format!(
                "capability {} must declare a positive version",
                quote(&d.operation_id)
            ));
        }
        if !["read", "mutation", "destructive", "credential_bearing"].contains(&d.effect.as_str()) {
            return Err(format!(
                "capability {} has unsupported effect {}",
                quote(&d.operation_id),
                quote(&d.effect)
            ));
        }
        if !["", "sync_only", "bridged"].contains(&d.task_support.as_str()) {
            return Err(format!(
                "capability {} has unsupported taskSupport {}",
                quote(&d.operation_id),
                quote(&d.task_support)
            ));
        }
        for kind in &d.resource_kinds {
            if !self.known_kind(kind) {
                return Err(format!(
                    "capability {} references unknown resource kind {}",
                    quote(&d.operation_id),
                    quote(kind)
                ));
            }
        }
        for binding in &d.requires {
            self.validate_binding(d, binding, input, false)?;
        }
        for binding in &d.produces {
            self.validate_binding(d, binding, output, true)?;
        }
        Ok(())
    }
    fn known_kind(&self, kind: &str) -> bool {
        crate::resource::KNOWN_TYPES.contains(&kind.trim())
            && (self.known.is_empty() || self.known.get(kind).copied().unwrap_or(false))
    }
    fn validate_cost(&self, d: &Descriptor, input: &Map<String, Value>) -> Result<(), String> {
        let Some(cost) = &d.resource_cost else {
            return Ok(());
        };
        let id = quote(&d.operation_id);
        if !cost.cpu_cores.is_finite() || cost.cpu_cores < 0.0 {
            return Err(format!(
                "capability {id} has an invalid resourceCost.cpuCores"
            ));
        }
        // Go's map iteration can choose any negative dimension first; preserve
        // single-error fixtures, and leave ambiguous first-error ordering open.
        for (field, value) in [
            ("memoryBytes", cost.memory_bytes),
            ("diskBytes", cost.disk_bytes),
            ("tasks", cost.tasks),
        ] {
            if value < 0 {
                return Err(format!(
                    "capability {id} has a negative resourceCost.{field}"
                ));
            }
        }
        if !["", "control", "normal", "heavy"].contains(&cost.class.as_str()) {
            return Err(format!(
                "capability {id} has unsupported resourceCost.class {}",
                quote(&cost.class)
            ));
        }
        if let Some(bindings) = &cost.argument_bindings {
            for (i, (field, fallback, types)) in [
                ("cpuCores", cost.cpu_cores > 0.0, &["number", "integer"][..]),
                (
                    "memoryBytes",
                    cost.memory_bytes > 0,
                    &["string", "number", "integer"][..],
                ),
                ("diskBytes", true, &["string", "number", "integer"][..]),
                ("tasks", cost.tasks > 0, &["number", "integer"][..]),
            ]
            .into_iter()
            .enumerate()
            {
                let path = bindings[i].trim();
                if path.is_empty() {
                    continue;
                }
                if !fallback {
                    return Err(format!(
                        "capability {id} resourceCost.{field} requires a positive static fallback"
                    ));
                }
                if path.starts_with('.') || path.ends_with('.') || path.contains(['[', ']', '/']) {
                    return Err(format!("capability {id} resourceCost.{field} binding path {} is not a supported object path",quote(path)));
                }
                let property=schema_path(input,path).ok_or_else(||format!("capability {id} resourceCost.{field} binding path {} is absent from inputSchema",quote(path)))?;
                let ty = property
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if !types.contains(&ty) {
                    return Err(format!("capability {id} resourceCost.{field} binding path {} has unsupported type {}",quote(path),quote(ty)));
                }
            }
        }
        Ok(())
    }
    fn validate_selectors(
        &self,
        d: &Descriptor,
        output: &Map<String, Value>,
    ) -> Result<(), String> {
        if d.result_types.is_empty() {
            return if d.output_type.trim().is_empty() {
                Ok(())
            } else {
                Err(format!(
                    "output type {} has no result type descriptor",
                    quote(&d.output_type)
                ))
            };
        }
        if d.output_type.trim().is_empty() {
            return Err("result selectors require an output type".into());
        }
        let mut types = BTreeSet::new();
        for ty in &d.result_types {
            if ty.id.trim().is_empty() || ty.version < 1 {
                return Err("result types require a non-empty id and positive version".into());
            }
            if !types.insert(ty.id.as_str()) {
                return Err(format!("duplicate result type {}", quote(&ty.id)));
            }
            let mut selectors = BTreeSet::new();
            for s in &ty.selectors {
                if s.id.trim().is_empty() || s.source_path.trim().is_empty() {
                    return Err(format!(
                        "result type {} has an incomplete selector",
                        quote(&ty.id)
                    ));
                }
                if !selectors.insert(s.id.as_str()) {
                    return Err(format!(
                        "result type {} has duplicate selector {}",
                        quote(&ty.id),
                        quote(&s.id)
                    ));
                }
                let cardinality = if s.cardinality.trim().is_empty() {
                    "one"
                } else {
                    &s.cardinality
                };
                if !["one", "many"].contains(&cardinality) {
                    return Err(format!(
                        "selector {} has unsupported cardinality {}",
                        quote(&s.id),
                        quote(&s.cardinality)
                    ));
                }
                if schema_path(output, &s.source_path).is_none() {
                    return Err(format!(
                        "selector {} sourcePath {} is absent from the output schema",
                        quote(&s.id),
                        quote(&s.source_path)
                    ));
                }
                if !s.label_path.is_empty() && schema_path(output, &s.label_path).is_none() {
                    return Err(format!(
                        "selector {} labelPath {} is absent from the output schema",
                        quote(&s.id),
                        quote(&s.label_path)
                    ));
                }
            }
        }
        if !types.contains(d.output_type.as_str()) {
            return Err(format!(
                "output type {} is not declared",
                quote(&d.output_type)
            ));
        }
        Ok(())
    }
    fn validate_binding(
        &self,
        d: &Descriptor,
        b: &Binding,
        schema: &Map<String, Value>,
        produces: bool,
    ) -> Result<(), String> {
        let id = quote(&d.operation_id);
        if b.resource_type.trim().is_empty() {
            return Err(format!(
                "capability {id} has a resource binding without a resource type"
            ));
        }
        if !self.known_kind(&b.resource_type) {
            return Err(format!(
                "capability {id} references unknown binding resource kind {}",
                quote(&b.resource_type)
            ));
        }
        let path = if produces {
            &b.source_path
        } else {
            &b.argument
        };
        let property = schema_path(schema, path);
        if path.trim().is_empty() || property.is_none() {
            return Err(if produces {
                format!(
                    "capability {id} produces binding sourcePath {} in its output schema",
                    quote(path)
                )
            } else {
                format!(
                    "capability {id} requires binding argument {} in its input schema",
                    quote(path)
                )
            });
        }
        if property.unwrap().get("type").and_then(Value::as_str) != Some("string") {
            return Err(format!(
                "capability {id} resource binding path {} must be a string",
                quote(path)
            ));
        }
        if produces && !b.selector_id.is_empty() {
            let selector = d
                .result_types
                .iter()
                .filter(|t| t.id == d.output_type)
                .flat_map(|t| t.selectors.iter())
                .find(|s| s.id == b.selector_id)
                .ok_or_else(|| {
                    format!(
                        "capability {id} produced binding references unknown selector {}",
                        quote(&b.selector_id)
                    )
                })?;
            if selector.source_path != b.source_path {
                return Err(format!("capability {id} selector {} sourcePath {} disagrees with produced binding path {}",quote(&b.selector_id),quote(&selector.source_path),quote(&b.source_path)));
            }
        }
        Ok(())
    }
}
fn schema_path<'a>(schema: &'a Map<String, Value>, path: &str) -> Option<&'a Map<String, Value>> {
    let mut current = schema;
    for segment in path.split('.') {
        let segment = segment.strip_suffix("[]").unwrap_or(segment);
        current = current
            .get("properties")?
            .as_object()?
            .get(segment)?
            .as_object()?;
        if let Some(items) = current.get("items").and_then(Value::as_object) {
            current = items;
        }
    }
    Some(current)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn descriptor(value: &Value) -> Descriptor {
        let raw = serde_json::to_vec(&value["manifest"]).unwrap();
        let manifest =
            crate::provider::definition::decode_manifest(&crate::gojson::parse(&raw).unwrap())
                .unwrap();
        let mut d = super::super::provider_descriptors(&manifest, "fixture-generation").remove(0);
        d.generation_id = value["generation"].as_str().unwrap_or_default().into();
        patch(&mut d, &value["patch"]);
        d
    }
    fn patch(d: &mut Descriptor, p: &Value) {
        if let Some(p) = p.as_object() {
            for (key, v) in p {
                let text = || v.as_str().unwrap_or_default().to_string();
                match key.as_str() {
                    "operationId" => d.operation_id = text(),
                    "name" => d.name = text(),
                    "provider" => d.provider = text(),
                    "implementation" => d.implementation = text(),
                    "generationId" => d.generation_id = text(),
                    "capabilityId" => d.capability_id = text(),
                    "version" => d.version = v.as_i64().unwrap(),
                    "description" => d.description = text(),
                    "effect" => d.effect = text(),
                    "taskSupport" => d.task_support = text(),
                    other => panic!("unsupported fixture descriptor patch {other}"),
                }
            }
        }
    }
    fn executable(v: &Value) -> Option<Arc<Capability>> {
        if v.is_null() {
            return None;
        }
        let d = descriptor(v);
        let generation = d.generation_id.clone();
        let marker = v["marker"].clone();
        let input = d.input_schema.as_object().cloned().unwrap_or_default();
        Some(Arc::new(Capability {
            descriptor: d,
            invoke: Arc::new(move |_, args| {
                crate::plan::schema::validate_json(&input, &Value::Object(args.clone()))
                    .map_err(|error| format!("invalid capability arguments: {error}"))?;
                Ok(json!({"marker":marker,"generation":generation})
                    .as_object()
                    .unwrap()
                    .clone())
            }),
        }))
    }
    #[test]
    #[ignore = "requires the owning Go registry differential driver"]
    fn registry_fixture() {
        let input: Value = serde_json::from_slice(
            &std::fs::read(std::env::var("PARITY_PROVIDER_REGISTRY_INPUT").unwrap()).unwrap(),
        )
        .unwrap();
        let outputs: Vec<Value> = input
            .as_array()
            .unwrap()
            .iter()
            .map(|row| {
                let row = &row["registry"];
                let base: Vec<Descriptor> = row["base"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(descriptor)
                    .collect();
                let map = |key: &str| {
                    row["options"][key]
                        .as_object()
                        .map(|m| {
                            m.iter()
                                .map(|(k, v)| (k.clone(), v.as_bool().unwrap()))
                                .collect()
                        })
                        .unwrap_or_default()
                };
                let registry = Registry::new(
                    build_from_descriptors("incus", &base),
                    Options {
                        authorized_providers: map("AuthorizedProviders"),
                        known_resource_kinds: map("KnownResourceKinds"),
                        allowed_implementations: map("AllowedImplementations"),
                    },
                    Arc::new(super::super::schema::check),
                );
                let mut handles: BTreeMap<String, Arc<Capability>> = BTreeMap::new();
                let mut observations = Vec::new();
                for step in row["steps"].as_array().unwrap() {
                    let text = |key: &str| step[key].as_str().unwrap_or_default();
                    let values: Vec<Option<Arc<Capability>>> = step["values"]
                        .as_array()
                        .map(|a| a.iter().map(executable).collect())
                        .unwrap_or_default();
                    let mut observation = Map::new();
                    let result = match text("action") {
                        "register" => registry.register(executable(&step["value"])),
                        "registerCapability" => registry.register_capability(
                            executable(&step["value"]),
                            text("provider"),
                            text("implementation"),
                        ),
                        "upsertCapability" => registry.upsert_capability(
                            executable(&step["value"]),
                            text("provider"),
                            text("implementation"),
                        ),
                        "registration" | "upsert" => {
                            let mut d = descriptor(&step["value"]);
                            patch(&mut d, &step["descriptorPatch"]);
                            let r = Registration {
                                descriptor: d,
                                provider_id: text("provider").into(),
                                implementation: text("implementation").into(),
                                capability: if step["nilExecutable"] == true {
                                    None
                                } else {
                                    executable(&step["value"])
                                },
                            };
                            if text("action") == "upsert" {
                                registry.upsert(r)
                            } else {
                                registry.register_registration(r)
                            }
                        }
                        "replaceGeneration" => {
                            registry.replace_generation(text("generation"), &values)
                        }
                        "replaceProvider" => registry.replace_provider(text("provider"), &values),
                        "authorize" => registry.authorize_provider(text("provider")),
                        "unregister" => registry.unregister(text("id")),
                        "displace" => {
                            let families: Vec<String> = step["families"]
                                .as_array()
                                .map(|a| a.iter().map(|v| v.as_str().unwrap().into()).collect())
                                .unwrap_or_default();
                            observation.insert(
                                "removed".into(),
                                json!(registry
                                    .displace_capability_families(text("provider"), &families)),
                            );
                            Ok(())
                        }
                        "validateBase" => registry.validate_base(),
                        "resolve" => {
                            let revision = if text("revision") == "$current" {
                                registry.snapshot().revision
                            } else {
                                text("revision").into()
                            };
                            registry.resolve(text("id"), &revision).map(|value| {
                                observation.insert("resolved".into(), value.descriptor.to_json());
                                handles.insert(text("handle").into(), value);
                            })
                        }
                        "lookup" => {
                            let value = registry.resolve_capability(text("id"));
                            observation.insert("found".into(), json!(value.is_some()));
                            if let Some(value) = value {
                                observation.insert("resolved".into(), value.descriptor.to_json());
                            }
                            Ok(())
                        }
                        "invoke" => {
                            (handles[text("handle")].invoke)(&RunCtx::background(), &Map::new())
                                .map(|v| {
                                    observation.insert("result".into(), Value::Object(v));
                                })
                        }
                        other => panic!("unsupported registry fixture action {other}"),
                    };
                    observation.insert("error".into(), json!(result.err().unwrap_or_default()));
                    let mut snapshot = String::new();
                    registry.snapshot().encode(&mut snapshot);
                    observation.insert("snapshot".into(), json!(snapshot));
                    observations.push(Value::Object(observation));
                }
                json!({"observations":observations})
            })
            .collect();
        std::fs::write(
            std::env::var("PARITY_PROVIDER_REGISTRY_OUTPUT").unwrap(),
            serde_json::to_vec_pretty(&outputs).unwrap(),
        )
        .unwrap();
    }
}
