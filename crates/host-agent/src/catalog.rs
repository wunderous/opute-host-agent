//! Catalog publication.
//!
//! The pinned Go agent assembles its catalog from schema files and from
//! definitions and registration tables declared in Go code.
//! `catalog/source.json` holds those declarations, generated from the pinned
//! tree by `python3 -m parity catalog-source` (never edited by hand). This
//! module derives everything else the way Go does: canonical resource
//! bindings, capability descriptors, edges, default labels, and the catalog
//! revision, which is a SHA-256 over Go's exact `json.Marshal` bytes.

use crate::gojson::{encode, encode_float, encode_string};
use serde_json::{json, Map, Value as J};
use std::collections::BTreeMap;
use std::sync::OnceLock;

const SOURCE: &str = include_str!("../catalog/source.json");
const RESOURCE_TYPE_KEYWORD: &str = "x-opute-resource-type";

/// A tool definition as Go's `ToolDefinition` holds it.
#[derive(Clone, Debug)]
pub struct Definition {
    pub name: String,
    pub title: String,
    pub description: String,
    pub input_schema: J,
    pub output_schema: Option<J>,
    pub meta: Option<Map<String, J>>,
}

impl Definition {
    fn from_json(v: &J) -> Definition {
        let text = |k: &str| v.get(k).and_then(J::as_str).unwrap_or("").to_string();
        Definition {
            name: text("name"),
            title: text("title"),
            description: text("description"),
            input_schema: v.get("inputSchema").cloned().unwrap_or(J::Null),
            output_schema: v.get("outputSchema").filter(|s| !s.is_null()).cloned(),
            meta: v.get("_meta").and_then(J::as_object).cloned(),
        }
    }
}

struct Source {
    provider_id: String,
    host: Vec<Definition>,
    standalone: Vec<Definition>,
    standalone_from_all: Vec<Definition>,
    internal: Vec<Definition>,
    residual_effects: Map<String, J>,
    registrations: Map<String, J>,
    task_aware: Vec<String>,
}

fn source() -> &'static Source {
    static SRC: OnceLock<Source> = OnceLock::new();
    SRC.get_or_init(|| {
        let doc: J = serde_json::from_str(SOURCE).expect("catalog/source.json is valid JSON");
        let defs = |k: &str| -> Vec<Definition> {
            doc[k]
                .as_array()
                .map(|a| a.iter().map(Definition::from_json).collect())
                .unwrap_or_default()
        };
        let obj = |k: &str| doc[k].as_object().cloned().unwrap_or_default();
        Source {
            provider_id: doc["providerId"].as_str().unwrap_or("incus").to_string(),
            host: defs("hostDefinitions"),
            standalone: defs("standaloneDefinitions"),
            standalone_from_all: defs("standaloneFromAll"),
            internal: defs("internalDefinitions"),
            residual_effects: obj("residualEffects"),
            registrations: obj("registrations"),
            task_aware: doc["taskAwareTools"]
                .as_array()
                .map(|a| a.iter().filter_map(J::as_str).map(str::to_string).collect())
                .unwrap_or_default(),
        }
    })
}

fn registration(name: &str) -> Option<&'static Map<String, J>> {
    source().registrations.get(name).and_then(J::as_object)
}

/// `IsStandaloneMutation`: tools the standalone mutation gate closes.
pub fn is_standalone_mutation(name: &str) -> bool {
    registration(name)
        .and_then(|r| r.get("standaloneMutation"))
        .and_then(J::as_bool)
        .unwrap_or(false)
}

/// `tools.IsTaskAware(name) || tasks.TaskAwareTools[name]`: the call
/// crosses the MCP Tasks boundary.
pub fn is_task_aware(name: &str) -> bool {
    registration(name)
        .and_then(|r| r.get("taskAware"))
        .and_then(J::as_bool)
        .unwrap_or(false)
        || source().task_aware.iter().any(|n| n == name)
}

/// `RegisteredAdmissionClass`: the class a capability declared at its
/// registration site, if it has one.
pub fn admission_class(name: &str) -> Option<String> {
    registration(name)?
        .get("admissionClass")
        .and_then(J::as_str)
        .map(str::to_string)
}

/// `StandaloneToolMetadata`.
pub fn standalone_metadata(name: &str) -> Option<&'static J> {
    registration(name)?.get("standaloneMetadata")
}

// --- canonical bindings (CanonicalizeToolDefinitions) ------------------------------

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Binding {
    pub argument: String,
    pub resource_type: String,
    pub source_path: String,
    pub selector_id: String,
    pub required: bool,
}

fn explicit_bindings(meta: Option<&Map<String, J>>, key: &str) -> Vec<Binding> {
    let Some(items) = meta.and_then(|m| m.get(key)).and_then(J::as_array) else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(J::as_object)
        .filter_map(|o| {
            let resource_type = o.get("resourceType").and_then(J::as_str)?;
            if resource_type.trim().is_empty() {
                return None;
            }
            let text = |k: &str| o.get(k).and_then(J::as_str).unwrap_or("").to_string();
            Some(Binding {
                argument: text("argument"),
                resource_type: resource_type.to_string(),
                source_path: text("sourcePath"),
                selector_id: text("selectorId"),
                required: o.get("required").and_then(J::as_bool).unwrap_or(false),
            })
        })
        .collect()
}

fn annotated_bindings(schema: &J, requires: bool) -> Vec<Binding> {
    fn walk(node: &Map<String, J>, path: &str, requires: bool, out: &mut Vec<Binding>) {
        if let Some(resource_type) = node
            .get(RESOURCE_TYPE_KEYWORD)
            .and_then(J::as_str)
            .map(str::trim)
            .filter(|t| !t.is_empty())
        {
            let mut b = Binding {
                resource_type: resource_type.to_string(),
                ..Binding::default()
            };
            if requires {
                b.argument = path.to_string();
                b.required = node
                    .get("x-opute-required")
                    .and_then(J::as_bool)
                    .unwrap_or(path == "uri");
            } else {
                b.source_path = path.to_string();
            }
            out.push(b);
        }
        // Go ranges over the map in random order; materialized bindings are
        // already explicit in the generated source, so only a new annotation
        // depends on this order, and sorted is deterministic.
        if let Some(properties) = node.get("properties").and_then(J::as_object) {
            for (name, property) in properties {
                if let Some(property) = property.as_object() {
                    let child = if path.is_empty() {
                        name.clone()
                    } else {
                        format!("{path}.{name}")
                    };
                    walk(property, &child, requires, out);
                }
            }
        }
        if let Some(items) = node.get("items").and_then(J::as_object) {
            walk(items, &format!("{path}[]"), requires, out);
        }
    }
    let mut out = Vec::new();
    if let Some(root) = schema.as_object().filter(|m| !m.is_empty()) {
        walk(root, "", requires, &mut out);
    }
    out
}

fn resource_bindings(def: &Definition, requires: bool) -> Vec<Binding> {
    let key = if requires { "requires" } else { "produces" };
    let mut bindings = explicit_bindings(def.meta.as_ref(), key);
    let schema = if requires {
        &def.input_schema
    } else {
        def.output_schema.as_ref().unwrap_or(&J::Null)
    };
    for candidate in annotated_bindings(schema, requires) {
        if !bindings.iter().any(|b| {
            b.argument == candidate.argument
                && b.source_path == candidate.source_path
                && b.resource_type == candidate.resource_type
        }) {
            bindings.push(candidate);
        }
    }
    bindings
}

fn canonicalize(defs: &[Definition]) -> Vec<Definition> {
    defs.iter()
        .map(|original| {
            let mut def = original.clone();
            if def.output_schema.is_none() {
                def.output_schema = Some(json!({"type": "object"}));
            }
            let mut meta: Map<String, J> = def
                .meta
                .iter()
                .flatten()
                .filter(|(k, _)| k.as_str() != "argumentProducers")
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            for requires in [true, false] {
                let bindings = resource_bindings(&def, requires);
                if bindings.is_empty() {
                    continue;
                }
                let items: Vec<J> = bindings
                    .iter()
                    .map(|b| {
                        let mut item = Map::new();
                        item.insert("resourceType".into(), J::from(b.resource_type.clone()));
                        if !b.argument.is_empty() {
                            item.insert("argument".into(), J::from(b.argument.clone()));
                        }
                        if !b.source_path.is_empty() {
                            item.insert("sourcePath".into(), J::from(b.source_path.clone()));
                        }
                        if !b.selector_id.is_empty() {
                            item.insert("selectorId".into(), J::from(b.selector_id.clone()));
                        }
                        if b.required {
                            item.insert("required".into(), J::Bool(true));
                        }
                        J::Object(item)
                    })
                    .collect();
                meta.insert(
                    if requires { "requires" } else { "produces" }.into(),
                    J::Array(items),
                );
            }
            def.meta = if meta.is_empty() { None } else { Some(meta) };
            def
        })
        .collect()
}

// --- descriptors (capabilityDescriptor) -----------------------------------------------

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ResourceCost {
    pub cpu_cores: f64,
    pub memory_bytes: i64,
    pub disk_bytes: i64,
    pub tasks: i64,
    pub class: String,
    pub argument_bindings: Option<[String; 4]>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Selector {
    id: String,
    source_path: String,
    cardinality: String,
    label_path: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResultType {
    id: String,
    version: i64,
    selectors: Vec<Selector>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Edge {
    pub source_tool: String,
    pub source_path: String,
    pub target_tool: String,
    pub target_argument: String,
    pub resource_type: String,
    pub selector_id: String,
    pub required: bool,
}

/// Go's `CapabilityDescriptor`, with the fields the built-in catalog uses.
#[derive(Clone, Debug)]
pub struct Descriptor {
    pub name: String,
    pub title: String,
    pub description: String,
    pub input_schema: J,
    pub output_schema: Option<J>,
    pub output_type: String,
    result_types: Vec<ResultType>,
    pub effect: String,
    pub privilege: String,
    pub requires_approval: bool,
    pub provider: String,
    pub implementation: String,
    resource_kinds: Vec<String>,
    required_fields: Vec<String>,
    produced_observables: Vec<String>,
    argument_producers: BTreeMap<String, Vec<String>>,
    default_labels: Option<BTreeMap<String, String>>,
    pub gate_message: String,
    pub consequence: String,
    pub resource_cost: Option<ResourceCost>,
    pub idempotent: bool,
    pub supports_readiness: bool,
    pub requires: Vec<Binding>,
    pub produces: Vec<Binding>,
    input_edges: Vec<Edge>,
    output_edges: Vec<Edge>,
}

fn meta_string(meta: Option<&Map<String, J>>, key: &str, fallback: &str) -> String {
    meta.and_then(|m| m.get(key))
        .and_then(J::as_str)
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(fallback)
        .to_string()
}

fn meta_bool(meta: Option<&Map<String, J>>, key: &str) -> bool {
    meta.and_then(|m| m.get(key))
        .and_then(J::as_bool)
        .unwrap_or(false)
}

fn string_slice(v: Option<&J>) -> Vec<String> {
    v.and_then(J::as_array)
        .map(|a| {
            a.iter()
                .filter_map(J::as_str)
                .filter(|s| !s.trim().is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// The key `encoding/json` would match for a struct field: exact first, then
/// case-insensitively (last match wins, as Go keeps decoding).
fn go_field<'a>(object: &'a Map<String, J>, name: &str) -> Option<&'a J> {
    object.get(name).or_else(|| {
        object
            .iter()
            .rev()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v)
    })
}

/// `json.Unmarshal` of a number into int64: integer literals only.
fn go_int(v: &J) -> Result<i64, ()> {
    match v {
        J::Null => Ok(0),
        J::Number(n) => n.as_i64().ok_or(()),
        _ => Err(()),
    }
}

fn go_float(v: &J) -> Result<f64, ()> {
    match v {
        J::Null => Ok(0.0),
        J::Number(n) => n.as_f64().ok_or(()),
        _ => Err(()),
    }
}

fn go_str(v: &J) -> Result<String, ()> {
    match v {
        J::Null => Ok(String::new()),
        J::String(s) => Ok(s.clone()),
        _ => Err(()),
    }
}

/// `json.Unmarshal` into `ResourceCost`; any type error makes the whole
/// declaration unusable, as in Go.
fn parse_cost(v: &J) -> Option<ResourceCost> {
    let o = v.as_object()?;
    let mut cost = ResourceCost::default();
    if let Some(x) = go_field(o, "cpuCores") {
        cost.cpu_cores = go_float(x).ok()?;
    }
    if let Some(x) = go_field(o, "memoryBytes") {
        cost.memory_bytes = go_int(x).ok()?;
    }
    if let Some(x) = go_field(o, "diskBytes") {
        cost.disk_bytes = go_int(x).ok()?;
    }
    if let Some(x) = go_field(o, "tasks") {
        cost.tasks = go_int(x).ok()?;
    }
    if let Some(x) = go_field(o, "class") {
        cost.class = go_str(x).ok()?;
    }
    if let Some(x) = go_field(o, "argumentBindings") {
        if !x.is_null() {
            let b = x.as_object()?;
            let mut out: [String; 4] = Default::default();
            for (i, k) in ["cpuCores", "memoryBytes", "diskBytes", "tasks"]
                .iter()
                .enumerate()
            {
                if let Some(v) = go_field(b, k) {
                    out[i] = go_str(v).ok()?;
                }
            }
            cost.argument_bindings = Some(out);
        }
    }
    Some(cost)
}

fn resource_cost(def: &Definition) -> Option<ResourceCost> {
    if let Some(raw) = def.meta.as_ref().and_then(|m| m.get("resourceCost")) {
        if !raw.is_null() {
            return parse_cost(raw);
        }
    }
    registration(&def.name)
        .and_then(|r| r.get("resourceCost"))
        .and_then(parse_cost)
}

fn result_types(meta: Option<&Map<String, J>>) -> Vec<ResultType> {
    let Some(items) = meta.and_then(|m| m.get("resultTypes")) else {
        return Vec::new();
    };
    let parse = || -> Option<Vec<ResultType>> {
        let mut out = Vec::new();
        for item in items.as_array()? {
            let o = item.as_object()?;
            let mut rt = ResultType {
                id: go_field(o, "id")
                    .map(go_str)
                    .transpose()
                    .ok()?
                    .unwrap_or_default(),
                version: go_field(o, "version")
                    .map(go_int)
                    .transpose()
                    .ok()?
                    .unwrap_or(0),
                selectors: Vec::new(),
            };
            if let Some(selectors) = go_field(o, "selectors").filter(|s| !s.is_null()) {
                for s in selectors.as_array()? {
                    let s = s.as_object()?;
                    let text = |k: &str| -> Option<String> {
                        go_field(s, k)
                            .map(go_str)
                            .transpose()
                            .ok()
                            .map(Option::unwrap_or_default)
                    };
                    rt.selectors.push(Selector {
                        id: text("id")?,
                        source_path: text("sourcePath")?,
                        cardinality: text("cardinality")?,
                        label_path: text("labelPath")?,
                    });
                }
            }
            out.push(rt);
        }
        Some(out)
    };
    parse().unwrap_or_default()
}

fn effect(def: &Definition) -> String {
    declared_effect(def).unwrap_or_else(|| "read".to_string())
}

/// The effect a definition declares: its own metadata, its registration, the
/// residual effects, then its standalone classification. `effect` falls back
/// to `read` when nothing is declared; the D10 gate does not.
fn declared_effect(def: &Definition) -> Option<String> {
    let declared = meta_string(def.meta.as_ref(), "effect", "");
    if !declared.is_empty() {
        return Some(declared);
    }
    if let Some(e) = registration(&def.name)
        .and_then(|r| r.get("effect"))
        .and_then(J::as_str)
    {
        return Some(e.to_string());
    }
    if let Some(e) = source().residual_effects.get(&def.name).and_then(J::as_str) {
        return Some(e.to_string());
    }
    let classification = standalone_metadata(&def.name)
        .and_then(|m| m.get("opute"))
        .and_then(|o| o.get("classification"))
        .and_then(J::as_str)
        .map(str::trim)
        .unwrap_or("");
    match classification {
        "" => None,
        "read_only" => Some("read".to_string()),
        other => Some(other.to_string()),
    }
}

/// Decision D10 (`standalone-read-only-gate`): whether a standalone server
/// with mutations disabled may run `name`. A tool the catalog publishes runs
/// only when its published effect is `read`; a tool it does not publish runs
/// only when a `read` effect is declared, never inferred.
pub fn standalone_read_only(catalog: &Snapshot, name: &str) -> bool {
    if is_standalone_mutation(name) {
        return false;
    }
    if let Some(d) = catalog.tools.iter().find(|d| d.name == name) {
        return d.effect == "read";
    }
    source()
        .internal
        .iter()
        .find(|d| d.name == name)
        .and_then(declared_effect)
        .is_some_and(|e| e == "read")
}

fn descriptor(provider: &str, def: &Definition) -> Descriptor {
    let meta = def.meta.as_ref();
    let effect = effect(def);
    let mut gate_message = meta_string(meta, "gateMessage", "");
    let mut consequence = meta_string(meta, "consequence", "");
    if effect != "read" {
        if gate_message.is_empty() {
            gate_message = "Host approval is required before this capability can execute.".into();
        }
        if consequence.is_empty() {
            consequence = format!("This capability may change host state (effect: {effect}).");
        }
    }
    let default_labels: BTreeMap<String, String> = meta
        .and_then(|m| m.get("defaultLabels"))
        .and_then(J::as_object)
        .map(|o| {
            o.iter()
                .filter_map(|(k, v)| {
                    v.as_str()
                        .filter(|s| !s.trim().is_empty())
                        .map(|s| (k.clone(), s.to_string()))
                })
                .collect()
        })
        .unwrap_or_default();
    Descriptor {
        name: def.name.clone(),
        title: def.title.clone(),
        description: def.description.clone(),
        input_schema: def.input_schema.clone(),
        output_schema: def.output_schema.clone(),
        output_type: meta_string(meta, "outputType", ""),
        result_types: result_types(meta),
        privilege: meta_string(meta, "privilege", &effect),
        requires_approval: effect != "read",
        provider: provider.to_string(),
        implementation: format!("host-agent:{provider}"),
        resource_kinds: string_slice(meta.and_then(|m| m.get("resourceKinds"))),
        required_fields: def
            .input_schema
            .get("required")
            .and_then(J::as_array)
            .map(|a| a.iter().filter_map(J::as_str).map(str::to_string).collect())
            .unwrap_or_default(),
        produced_observables: string_slice(meta.and_then(|m| m.get("producedObservables"))),
        argument_producers: BTreeMap::new(),
        default_labels: (!default_labels.is_empty()).then_some(default_labels),
        gate_message,
        consequence,
        resource_cost: resource_cost(def),
        idempotent: effect == "read" || meta_bool(meta, "idempotent"),
        supports_readiness: meta_bool(meta, "supportsReadiness") || effect != "read",
        requires: resource_bindings(def, true),
        produces: resource_bindings(def, false),
        input_edges: Vec::new(),
        output_edges: Vec::new(),
        effect,
    }
}

fn default_labels(schema: &J) -> Option<BTreeMap<String, String>> {
    let properties = schema.get("properties")?.as_object()?;
    let labels: BTreeMap<String, String> = properties
        .iter()
        .filter(|(_, p)| p.as_object().is_some_and(|p| p.contains_key("default")))
        .map(|(k, _)| (k.clone(), "default".to_string()))
        .collect();
    (!labels.is_empty()).then_some(labels)
}

fn derive_edges(descriptors: &[Descriptor]) -> Vec<Edge> {
    let mut edges = Vec::new();
    for target in descriptors {
        for required in &target.requires {
            if required.argument.trim().is_empty() || required.resource_type.trim().is_empty() {
                continue;
            }
            for source in descriptors {
                if source.name == target.name {
                    continue;
                }
                for produced in &source.produces {
                    if produced.resource_type != required.resource_type
                        || produced.source_path.trim().is_empty()
                    {
                        continue;
                    }
                    edges.push(Edge {
                        source_tool: source.name.clone(),
                        source_path: produced.source_path.clone(),
                        target_tool: target.name.clone(),
                        target_argument: required.argument.clone(),
                        resource_type: required.resource_type.clone(),
                        selector_id: produced.selector_id.clone(),
                        required: required.required,
                    });
                }
            }
        }
    }
    edges.sort_by(|a, b| {
        (
            &a.source_tool,
            &a.target_tool,
            &a.target_argument,
            &a.source_path,
            &a.resource_type,
        )
            .cmp(&(
                &b.source_tool,
                &b.target_tool,
                &b.target_argument,
                &b.source_path,
                &b.resource_type,
            ))
    });
    edges
}

/// A published catalog: Go's `CapabilityCatalogSnapshot`.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub provider_id: String,
    pub revision: String,
    pub tools: Vec<Descriptor>,
    pub edges: Vec<Edge>,
}

impl Snapshot {
    /// `json.Marshal(CapabilityCatalogSnapshot)`.
    pub fn encode(&self, out: &mut String) {
        let mut o = Obj::new(out);
        o.str("providerId", &self.provider_id, false);
        o.str("catalogRevision", &self.revision, false);
        let out = o.key("tools");
        out.push('[');
        for (i, d) in self.tools.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            d.encode(out);
        }
        out.push(']');
        o.list("edges", &self.edges, Edge::encode);
        o.end();
    }
}

/// `BuildCapabilityCatalog`: first definition per name wins, sorted by
/// operation id, then defaults, edges and the revision.
pub fn build(provider: &str, defs: &[Definition]) -> Snapshot {
    let mut by_name: BTreeMap<&str, &Definition> = BTreeMap::new();
    for def in defs {
        if !def.name.trim().is_empty() {
            by_name.entry(def.name.as_str()).or_insert(def);
        }
    }
    let mut tools: Vec<Descriptor> = by_name.values().map(|d| descriptor(provider, d)).collect();
    for d in &mut tools {
        if d.default_labels.is_none() {
            d.default_labels = default_labels(&d.input_schema);
        }
    }
    let edges = derive_edges(&tools);
    {
        let index: BTreeMap<String, usize> = tools
            .iter()
            .enumerate()
            .map(|(i, d)| (d.name.clone(), i))
            .collect();
        for edge in &edges {
            if let Some(&i) = index.get(&edge.source_tool) {
                tools[i].output_edges.push(edge.clone());
            }
            if let Some(&i) = index.get(&edge.target_tool) {
                tools[i].input_edges.push(edge.clone());
                let producers = tools[i]
                    .argument_producers
                    .entry(edge.target_argument.clone())
                    .or_default();
                if !producers.contains(&edge.source_tool) {
                    producers.push(edge.source_tool.clone());
                }
            }
        }
    }
    let mut canonical = String::from("{\"providerId\":");
    encode_string(provider, &mut canonical);
    canonical.push_str(",\"tools\":[");
    for (i, d) in tools.iter().enumerate() {
        if i > 0 {
            canonical.push(',');
        }
        d.encode(&mut canonical);
    }
    canonical.push(']');
    if !edges.is_empty() {
        canonical.push_str(",\"edges\":[");
        for (i, e) in edges.iter().enumerate() {
            if i > 0 {
                canonical.push(',');
            }
            e.encode(&mut canonical);
        }
        canonical.push(']');
    }
    canonical.push('}');
    let digest = <sha2::Sha256 as sha2::Digest>::digest(canonical.as_bytes());
    Snapshot {
        provider_id: provider.to_string(),
        revision: format!("sha256:{}", hex::encode(digest)),
        tools,
        edges,
    }
}

// --- encoding/json projections ----------------------------------------------------------

/// Writes struct fields in declaration order, skipping `omitempty` zeros.
struct Obj<'a> {
    out: &'a mut String,
    first: bool,
}

impl<'a> Obj<'a> {
    fn new(out: &'a mut String) -> Self {
        out.push('{');
        Obj { out, first: true }
    }
    fn key(&mut self, k: &str) -> &mut String {
        if !self.first {
            self.out.push(',');
        }
        self.first = false;
        encode_string(k, self.out);
        self.out.push(':');
        self.out
    }
    fn str(&mut self, k: &str, v: &str, omit_empty: bool) {
        if omit_empty && v.is_empty() {
            return;
        }
        let out = self.key(k);
        encode_string(v, out);
    }
    fn bool(&mut self, k: &str, v: bool, omit_empty: bool) {
        if omit_empty && !v {
            return;
        }
        self.key(k).push_str(if v { "true" } else { "false" });
    }
    fn int(&mut self, k: &str, v: i64, omit_empty: bool) {
        if omit_empty && v == 0 {
            return;
        }
        let s = v.to_string();
        self.key(k).push_str(&s);
    }
    fn value(&mut self, k: &str, v: &J) {
        let out = self.key(k);
        encode(v, out);
    }
    fn strings(&mut self, k: &str, v: &[String]) {
        if v.is_empty() {
            return;
        }
        let out = self.key(k);
        out.push('[');
        for (i, s) in v.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            encode_string(s, out);
        }
        out.push(']');
    }
    fn list<T>(&mut self, k: &str, v: &[T], f: impl Fn(&T, &mut String)) {
        if v.is_empty() {
            return;
        }
        let out = self.key(k);
        out.push('[');
        for (i, item) in v.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            f(item, out);
        }
        out.push(']');
    }
    fn end(self) {
        self.out.push('}');
    }
}

/// `map[string]any` with `omitempty`: absent or empty maps are omitted.
fn non_empty_map(v: &Option<J>) -> Option<&J> {
    v.as_ref()
        .filter(|v| !v.is_null() && v.as_object().is_none_or(|o| !o.is_empty()))
}

impl Binding {
    fn encode(&self, out: &mut String) {
        let mut o = Obj::new(out);
        o.str("argument", &self.argument, true);
        o.str("resourceType", &self.resource_type, false);
        o.str("sourcePath", &self.source_path, true);
        o.str("selectorId", &self.selector_id, true);
        o.bool("required", self.required, true);
        o.end();
    }
}

impl Edge {
    fn encode(&self, out: &mut String) {
        let mut o = Obj::new(out);
        o.str("sourceTool", &self.source_tool, false);
        o.str("sourcePath", &self.source_path, false);
        o.str("targetTool", &self.target_tool, false);
        o.str("targetArgument", &self.target_argument, false);
        o.str("resourceType", &self.resource_type, false);
        o.str("selectorId", &self.selector_id, true);
        o.bool("required", self.required, true);
        o.end();
    }
}

impl ResourceCost {
    fn encode(&self, out: &mut String) {
        let mut o = Obj::new(out);
        if self.cpu_cores != 0.0 {
            let out = o.key("cpuCores");
            encode_float(self.cpu_cores, out);
        }
        o.int("memoryBytes", self.memory_bytes, true);
        o.int("diskBytes", self.disk_bytes, true);
        o.int("tasks", self.tasks, true);
        o.str("class", &self.class, true);
        if let Some(b) = &self.argument_bindings {
            let out = o.key("argumentBindings");
            let mut inner = Obj::new(out);
            for (k, v) in ["cpuCores", "memoryBytes", "diskBytes", "tasks"]
                .iter()
                .zip(b)
            {
                inner.str(k, v, true);
            }
            inner.end();
        }
        o.end();
    }
}

impl ResultType {
    fn encode(&self, out: &mut String) {
        let mut o = Obj::new(out);
        o.str("id", &self.id, false);
        o.int("version", self.version, false);
        o.list("selectors", &self.selectors, |s, out| {
            let mut o = Obj::new(out);
            o.str("id", &s.id, false);
            o.str("sourcePath", &s.source_path, false);
            o.str("cardinality", &s.cardinality, false);
            o.str("labelPath", &s.label_path, true);
            o.end();
        });
        o.end();
    }
}

impl Descriptor {
    /// `json.Marshal(CapabilityDescriptor)`.
    pub fn encode(&self, out: &mut String) {
        let mut o = Obj::new(out);
        o.str("operationId", &self.name, false);
        o.int("version", 1, true);
        o.str("name", &self.name, false);
        o.str("title", &self.title, true);
        o.str("description", &self.description, true);
        o.value("inputSchema", &self.input_schema);
        if let Some(schema) = non_empty_map(&self.output_schema) {
            o.value("outputSchema", schema);
        }
        o.str("outputType", &self.output_type, true);
        o.list("resultTypes", &self.result_types, ResultType::encode);
        o.str("effect", &self.effect, false);
        o.str("privilege", &self.privilege, true);
        o.bool("requiresApproval", self.requires_approval, false);
        o.str("provider", &self.provider, false);
        o.str("implementation", &self.implementation, false);
        o.strings("resourceKinds", &self.resource_kinds);
        o.strings("requiredFields", &self.required_fields);
        o.strings("producedObservables", &self.produced_observables);
        if !self.argument_producers.is_empty() {
            let out = o.key("argumentProducers");
            let mut m = Obj::new(out);
            for (k, v) in &self.argument_producers {
                let out = m.key(k);
                out.push('[');
                for (i, s) in v.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    encode_string(s, out);
                }
                out.push(']');
            }
            m.end();
        }
        if let Some(labels) = &self.default_labels {
            let out = o.key("defaultLabels");
            let mut m = Obj::new(out);
            for (k, v) in labels {
                m.str(k, v, false);
            }
            m.end();
        }
        o.str("gateMessage", &self.gate_message, true);
        o.str("consequence", &self.consequence, true);
        if let Some(cost) = &self.resource_cost {
            let out = o.key("resourceCost");
            cost.encode(out);
        }
        o.bool("idempotent", self.idempotent, false);
        o.bool("supportsReadiness", self.supports_readiness, false);
        o.list("requires", &self.requires, Binding::encode);
        o.list("produces", &self.produces, Binding::encode);
        o.list("inputEdges", &self.input_edges, Edge::encode);
        o.list("outputEdges", &self.output_edges, Edge::encode);
        o.end();
    }

    pub fn to_json(&self) -> J {
        let mut s = String::new();
        self.encode(&mut s);
        serde_json::from_str(&s).expect("descriptor encodes as JSON")
    }
}

// --- the published catalogs ----------------------------------------------------------

/// The catalog the Go server builds in `NewServer`: the host definitions,
/// plus the standalone definitions in standalone mode.
pub fn for_mode(standalone: bool) -> &'static Snapshot {
    static STANDALONE: OnceLock<Snapshot> = OnceLock::new();
    static PLATFORM: OnceLock<Snapshot> = OnceLock::new();
    let cell = if standalone { &STANDALONE } else { &PLATFORM };
    cell.get_or_init(|| {
        let src = source();
        let mut defs = src.host.clone();
        if standalone {
            defs.extend(src.standalone.iter().cloned());
            defs.extend(src.standalone_from_all.iter().cloned());
        }
        build(&src.provider_id, &canonicalize(&defs))
    })
}

/// Host-internal tools: dispatchable over `tools/call`, never listed.
pub fn internal() -> &'static Snapshot {
    static INTERNAL: OnceLock<Snapshot> = OnceLock::new();
    INTERNAL.get_or_init(|| build(&source().provider_id, &canonicalize(&source().internal)))
}

/// `HostToolNamesForProvider`: the host catalog's tool names.
pub fn host_tool_names() -> Vec<String> {
    source().host.iter().map(|d| d.name.clone()).collect()
}

/// `WireToolName`.
pub fn wire_name(prefix: &str, name: &str) -> String {
    let (prefix, name) = (prefix.trim(), name.trim());
    if prefix.is_empty() || name.is_empty() {
        name.to_string()
    } else {
        format!("{prefix}_{name}")
    }
}

/// The `tools/list` entries `addRegisteredCapability` publishes, sorted by
/// wire name as the SDK lists them.
pub fn published_tools(snapshot: &Snapshot, prefix: &str) -> Vec<J> {
    let mut tools: Vec<J> = snapshot
        .tools
        .iter()
        .map(|d| {
            let mut tool = Map::new();
            tool.insert(
                "_meta".into(),
                json!({"catalogRevision": snapshot.revision, "capability": d.to_json()}),
            );
            if !d.description.is_empty() {
                tool.insert("description".into(), J::from(d.description.clone()));
            }
            tool.insert("inputSchema".into(), d.input_schema.clone());
            tool.insert("name".into(), J::from(wire_name(prefix, &d.name)));
            if let Some(schema) = d.output_schema.as_ref().filter(|s| !s.is_null()) {
                tool.insert("outputSchema".into(), schema.clone());
            }
            J::Object(tool)
        })
        .collect();
    tools.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    tools
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_sizes_match_go() {
        assert_eq!(for_mode(true).tools.len(), 142);
        assert_eq!(for_mode(false).tools.len(), 130);
        assert!(for_mode(true).revision.starts_with("sha256:"));
    }

    /// X5: a catalog change needs a reviewed manifest update.
    #[test]
    fn revisions_match_manifest() {
        let manifest: J =
            serde_json::from_str(include_str!("../../../parity-manifest.json")).unwrap();
        let recorded = &manifest["catalogRevisions"];
        assert_eq!(
            recorded["standalone"].as_str(),
            Some(for_mode(true).revision.as_str())
        );
        assert_eq!(
            recorded["platform"].as_str(),
            Some(for_mode(false).revision.as_str())
        );
    }

    #[test]
    fn wire_names_round_trip() {
        assert_eq!(
            wire_name("ab12cd34", "get_host_info"),
            "ab12cd34_get_host_info"
        );
        assert_eq!(wire_name("", " x "), "x");
    }

    #[test]
    fn malformed_cost_is_dropped_like_go() {
        assert!(parse_cost(&json!({"memoryBytes": 1.5})).is_none());
        assert!(parse_cost(&json!({"class": 3})).is_none());
        let c = parse_cost(&json!({"CPUCORES": 0.5, "tasks": 2})).unwrap();
        assert_eq!((c.cpu_cores, c.tasks), (0.5, 2));
    }

    /// D10: with mutations disabled, standalone runs exactly the tools whose
    /// effect is read, and internal tools only with a declared read effect.
    #[test]
    fn standalone_read_only_gate() {
        let cat = for_mode(true);
        for d in &cat.tools {
            assert_eq!(
                standalone_read_only(cat, &d.name),
                d.effect == "read",
                "{}",
                d.name
            );
            if is_standalone_mutation(&d.name) {
                assert!(!standalone_read_only(cat, &d.name), "{}", d.name);
            }
        }
        for name in [
            "get_host_info",
            "list_vms",
            "get_host_capacity",
            "detect_host_platform",
            "get_capability_catalog",
        ] {
            assert!(standalone_read_only(cat, name), "{name}");
        }
        let internal: Vec<(&str, bool)> = internal()
            .tools
            .iter()
            .filter(|d| !cat.tools.iter().any(|t| t.name == d.name))
            .map(|d| (d.name.as_str(), standalone_read_only(cat, &d.name)))
            .collect();
        for (name, allowed) in &internal {
            // Inferred `read` (nothing declared) is refused.
            let declared = source()
                .internal
                .iter()
                .find(|d| d.name == *name)
                .and_then(declared_effect);
            assert_eq!(*allowed, declared.as_deref() == Some("read"), "{name}");
        }
        assert!(internal
            .iter()
            .any(|(n, a)| *n == "configure_host_network" && !a));
        assert!(internal.iter().any(|(n, a)| *n == "exec_command" && !a));
        assert!(!standalone_read_only(cat, "no_such_tool"));
        let allowed = cat
            .tools
            .iter()
            .filter(|d| standalone_read_only(cat, &d.name))
            .count();
        assert_eq!(
            allowed,
            cat.tools.iter().filter(|d| d.effect == "read").count()
        );
    }
}
