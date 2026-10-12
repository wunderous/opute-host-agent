//! Typed tree indexing and JSON Pointers for the owning schema compiler.
use super::*;
use std::collections::HashMap;

fn escape(segment: &str) -> String {
    segment.replace('~', "~0").replace('/', "~1")
}
fn unescape(segment: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < segment.len() {
        if segment[i] == b'~' && i + 1 < segment.len() {
            match segment[i + 1] {
                b'0' => {
                    out.push(b'~');
                    i += 2;
                    continue;
                }
                b'1' => {
                    out.push(b'/');
                    i += 2;
                    continue;
                }
                _ => (),
            }
        }
        out.push(segment[i]);
        i += 1;
    }
    out
}

fn schema_fields(schema: &Schema) -> Vec<(&'static str, &Slot)> {
    let mut fields: Vec<_> = FIELDS
        .iter()
        .map(|&(json, name, _)| (json, &schema.fields[name]))
        .collect();
    fields.push(("items", &schema.fields["ItemsArray"]));
    fields.sort_by_key(|&(key, _)| key);
    fields
}

/// Go's all()/children() traversal sorts schema fields and map keys. This is
/// distinct from checkStructure's unspecified first-error map order.
fn preorder(root: &Schema) -> Vec<&Schema> {
    fn visit<'a>(schema: &'a Schema, nodes: &mut Vec<&'a Schema>) {
        nodes.push(schema);
        for (_, slot) in schema_fields(schema) {
            match slot {
                Slot::Child(Some(child)) => visit(child, nodes),
                Slot::Children(Some((children, len))) => {
                    for child in children[..*len].iter().flatten() {
                        visit(child, nodes);
                    }
                }
                Slot::ChildMap(Some(children)) => {
                    for child in children.values().flatten() {
                        visit(child, nodes);
                    }
                }
                _ => (),
            }
        }
    }
    let mut nodes = Vec::new();
    visit(root, &mut nodes);
    nodes
}
fn text<'a>(schema: &'a Schema, name: &str) -> &'a str {
    let Slot::Scalar(J::String(value)) = &schema.fields[name] else {
        unreachable!()
    };
    value
}
fn description(schema: &Schema) -> String {
    if !text(schema, "ID").is_empty() {
        return text(schema, "ID").into();
    }
    for name in ["Anchor", "DynamicAnchor"] {
        if !text(schema, name).is_empty() {
            return format!("anchor {}", text(schema, name));
        }
    }
    "<anonymous schema>".into()
}
fn basic(schema: &Schema) -> Result<(), String> {
    if !text(schema, "Type").is_empty() && matches!(schema.fields["Types"], Slot::Strings(Some(_)))
    {
        return Err("both Type and Types are set; at most one should be".into());
    }
    if matches!(schema.fields["Defs"], Slot::ChildMap(Some(_)))
        && matches!(schema.fields["Definitions"], Slot::ChildMap(Some(_)))
    {
        return Err("both Defs and Definitions are set; at most one should be".into());
    }
    if matches!(schema.fields["Items"], Slot::Child(Some(_)))
        && matches!(schema.fields["ItemsArray"], Slot::Children(Some(_)))
    {
        return Err("both Items and ItemsArray are set; at most one should be".into());
    }
    if let Slot::Strings(Some((order, len))) = &schema.fields["PropertyOrder"] {
        let mut seen = std::collections::BTreeSet::new();
        for value in &order[..*len] {
            if !seen.insert(value) {
                return Err(format!(
                    "property order slice cannot contain duplicate entries, found duplicate {}",
                    quote(value)
                ));
            }
        }
    }
    if let (Slot::ChildMap(Some(schemas)), Slot::Scalar(J::Object(strings))) = (
        &schema.fields["DependencySchemas"],
        &schema.fields["DependencyStrings"],
    ) {
        // The owning library ranges an unordered map to choose the first key.
        let unordered: HashMap<_, _> = schemas.iter().collect();
        for (key, _) in unordered {
            if strings.contains_key(key) {
                return Err(format!(
                    "dependency key {} cannot be defined as both a schema and a string array",
                    quote(key)
                ));
            }
        }
    }
    Ok(())
}

/// Native local checks require an explicit regexp compiler. There is no
/// production success callback: the full checker stays unwired until the
/// Go-compatible regexp boundary is implemented and validated.
pub(super) fn local(
    root: &Schema,
    compile_pattern: &dyn Fn(&str) -> Result<(), String>,
) -> Result<(), String> {
    structure(root)?;
    let mut errors = Vec::new();
    for schema in preorder(root) {
        if let Err(error) = basic(schema) {
            errors.push(error);
            continue;
        }
        let prefix = format!("jsonschema.Schema: {}: ", description(schema));
        if let Slot::Scalar(vocabulary) = &schema.fields["Vocabulary"] {
            if !vocabulary.is_null()
                && text(schema, "Schema") != "https://json-schema.org/draft/2020-12/schema"
            {
                errors.push(format!("{prefix}cannot validate a schema with $vocabulary"));
            }
        }
        let pattern = text(schema, "Pattern");
        if !pattern.is_empty() {
            if let Err(error) = compile_pattern(pattern) {
                errors.push(format!("{prefix}pattern: {error}"));
            }
        }
        if let Slot::ChildMap(Some(patterns)) = &schema.fields["PatternProperties"] {
            let unordered: HashMap<_, _> = patterns.iter().collect();
            for (pattern, _) in unordered {
                if let Err(error) = compile_pattern(pattern) {
                    errors.push(format!(
                        "{prefix}patternProperties[{}]: {error}",
                        quote(pattern)
                    ));
                }
            }
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("\n"))
    }
}

/// JSON decoding owns a tree, so shared/cyclic child pointers cannot arise.
/// Go map traversal is unspecified. Preserve arbitrary first-error selection
/// among map children; valid indices are sorted only for the private projection.
pub(super) fn structure(root: &Schema) -> Result<BTreeMap<String, &Schema>, String> {
    fn visit<'a>(
        s: &'a Schema,
        path: String,
        index: &mut BTreeMap<String, &'a Schema>,
    ) -> Result<(), String> {
        index.insert(
            if path.is_empty() {
                "root".into()
            } else {
                path.clone()
            },
            s,
        );
        for (key, slot) in schema_fields(s) {
            let base = format!("{path}/{key}");
            let descend = |child: Option<&'a Schema>,
                           at: String,
                           index: &mut BTreeMap<String, &'a Schema>| {
                visit(
                    child.ok_or_else(|| format!("jsonschema: schema at {at} is nil"))?,
                    at,
                    index,
                )
            };
            match slot {
                Slot::Child(Some(child)) => visit(child, base, index)?,
                Slot::Children(Some((slots, len))) => {
                    for (i, child) in slots[..*len].iter().enumerate() {
                        descend(child.as_ref(), format!("{base}/{i}"), index)?;
                    }
                }
                Slot::ChildMap(Some(children)) => {
                    let unordered: HashMap<_, _> = children.iter().collect();
                    for (key, child) in unordered {
                        descend(child.as_ref(), format!("{base}/{}", escape(key)), index)?;
                    }
                }
                _ => (),
            }
        }
        Ok(())
    }
    let mut index = BTreeMap::new();
    visit(root, String::new(), &mut index)?;
    Ok(index)
}

enum Cursor<'a> {
    Schema(Option<&'a Schema>),
    Children(Option<&'a (Vec<Option<Schema>>, usize)>),
    SchemaMap(Option<&'a BTreeMap<String, Option<Schema>>>),
    Strings(Option<&'a (Vec<String>, usize)>),
    Text(&'a str),
    Bool(bool),
    Pointer(Option<&'a J>, &'static str),
    AnyList(&'a J),
    StringMap(&'a J),
    BoolMap(&'a J),
    StringList(&'a J),
    Generic(&'a J),
    Raw(Option<&'a Node>),
    Byte(u8),
}
impl Cursor<'_> {
    fn ty(&self) -> String {
        match self {
            Self::Schema(_) => "*jsonschema.Schema",
            Self::Children(_) => "[]*jsonschema.Schema",
            Self::SchemaMap(_) => "map[string]*jsonschema.Schema",
            Self::Strings(_) | Self::StringList(_) => "[]string",
            Self::Text(_) => "string",
            Self::Bool(_) => "bool",
            Self::AnyList(_) => "[]interface {}",
            Self::StringMap(_) => "map[string][]string",
            Self::BoolMap(_) => "map[string]bool",
            Self::Generic(_) => "interface {}",
            Self::Raw(_) => "json.RawMessage",
            Self::Byte(_) => "uint8",
            Self::Pointer(_, name) => return format!("*{name}"),
        }
        .into()
    }
}
fn field_cursor<'a>(schema: &'a Schema, key: &str) -> Result<Cursor<'a>, String> {
    let (name, kind) = if key == "type" {
        match &schema.fields["Type"] {
            Slot::Scalar(J::String(s)) if !s.is_empty() => ("Type", Text),
            _ => ("Types", TextList),
        }
    } else if key == "items" {
        if matches!(schema.fields["Items"], Slot::Child(Some(_))) {
            ("Items", Child)
        } else {
            ("ItemsArray", ChildList)
        }
    } else if key == "dependencies" {
        ("DependencySchemas", ChildMap)
    } else {
        FIELDS
            .iter()
            .find(|&&(json, _, _)| json == key)
            .map(|&(_, name, kind)| (name, kind))
            .ok_or_else(|| format!("no schema field {}", quote(key)))?
    };
    Ok(match (&schema.fields[name], kind) {
        (Slot::Child(s), _) => Cursor::Schema(s.as_deref()),
        (Slot::Children(s), _) => Cursor::Children(s.as_ref()),
        (Slot::ChildMap(s), _) => Cursor::SchemaMap(s.as_ref()),
        (Slot::Strings(s), _) => Cursor::Strings(s.as_ref()),
        (Slot::Raw(s), _) => Cursor::Raw(s.as_ref()),
        (Slot::Presence(s), _) => Cursor::Pointer(s.as_ref(), "interface {}"),
        (Slot::Scalar(J::String(s)), Text) => Cursor::Text(s),
        (Slot::Scalar(J::Bool(b)), Bool) => Cursor::Bool(*b),
        (Slot::Scalar(s), Float | Integer) => Cursor::Pointer(
            if s.is_null() { None } else { Some(s) },
            if matches!(kind, Float) {
                "float64"
            } else {
                "int"
            },
        ),
        (Slot::Scalar(s), AnyList) => Cursor::AnyList(s),
        (Slot::Scalar(s), TextMap) => Cursor::StringMap(s),
        (Slot::Scalar(s), BoolMap) => Cursor::BoolMap(s),
        _ => unreachable!("schema pointer field inventory"),
    })
}
fn array_index(segment: &[u8], len: usize) -> Result<usize, String> {
    if segment == b"-" {
        return Err("the JSON Pointer array segment '-' is not supported".into());
    }
    if segment.len() > 1 && segment.starts_with(b"0") {
        return Err(format!(
            "segment {} has leading zeroes",
            uri::quote_bytes(segment)
        ));
    }
    let n = std::str::from_utf8(segment)
        .ok()
        .and_then(|s| s.parse::<i64>().ok())
        .ok_or(())
        .map_err(|_| format!("invalid int: {}", uri::quote_bytes(segment)))?;
    if n < 0 || n as u64 >= len as u64 {
        return Err(format!(
            "index {n} is out of bounds for array of length {len}"
        ));
    }
    Ok(n as usize)
}
fn general_float(f: f64) -> String {
    let text = format!("{f:e}");
    let (mantissa, exp) = text.split_once('e').unwrap();
    let exp: i32 = exp.parse().unwrap();
    if f == 0.0 || (-4..6).contains(&exp) {
        format!("{f}")
    } else {
        format!(
            "{mantissa}e{}{abs:02}",
            if exp < 0 { '-' } else { '+' },
            abs = exp.abs()
        )
    }
}
// fmt's %s applied to reflect.Value recursively, for diagnostic values only.
fn fmt_s(value: &J) -> String {
    match value {
        J::Null => "<nil>".into(),
        J::String(s) => s.clone(),
        J::Bool(b) => format!("%!s(bool={b})"),
        J::Number(n) => format!("%!s(float64={})", general_float(n.as_f64().unwrap())),
        J::Array(a) => format!("[{}]", a.iter().map(fmt_s).collect::<Vec<_>>().join(" ")),
        J::Object(m) => format!(
            "map[{}]",
            m.iter()
                .map(|(k, v)| format!("{k}:{}", fmt_s(v)))
                .collect::<Vec<_>>()
                .join(" ")
        ),
    }
}
pub(super) fn pointer<'a>(root: &'a Schema, pointer: &str) -> Result<Option<&'a Schema>, String> {
    pointer_bytes(root, pointer.as_bytes())
}
fn pointer_bytes<'a>(root: &'a Schema, pointer: &[u8]) -> Result<Option<&'a Schema>, String> {
    let traverse = || -> Result<Option<&'a Schema>, String> {
        if !pointer.is_empty() && !pointer.starts_with(b"/") {
            return Err(format!(
                "JSON Pointer {} does not begin with '/'",
                uri::quote_bytes(pointer)
            ));
        }
        let mut cursor = Cursor::Schema(Some(root));
        if !pointer.is_empty() {
            for raw in pointer[1..].split(|&b| b == b'/') {
                let segment = unescape(raw);
                cursor = match cursor {
                    Cursor::Schema(Some(s)) => match std::str::from_utf8(&segment) {
                        Ok(segment) => field_cursor(s, segment)?,
                        Err(_) => {
                            return Err(format!("no schema field {}", uri::quote_bytes(&segment)))
                        }
                    },
                    Cursor::Schema(None) | Cursor::Pointer(None, _) => {
                        return Err("navigated to nil reference".into())
                    }
                    Cursor::Pointer(Some(_), kind) => {
                        return Err(format!("navigated to non-Schema {kind}"))
                    }
                    Cursor::Children(list) => {
                        let slots = list.map_or(&[][..], |(slots, len)| &slots[..*len]);
                        Cursor::Schema(slots[array_index(&segment, slots.len())?].as_ref())
                    }
                    Cursor::SchemaMap(map) => Cursor::Schema(
                        map.and_then(|m| {
                            std::str::from_utf8(&segment)
                                .ok()
                                .and_then(|key| m.get(key))
                        })
                        .ok_or_else(|| format!("no key {} in map", uri::quote_bytes(&segment)))?
                        .as_ref(),
                    ),
                    Cursor::Strings(list) => {
                        let slots = list.map_or(&[][..], |(slots, len)| &slots[..*len]);
                        Cursor::Text(&slots[array_index(&segment, slots.len())?])
                    }
                    Cursor::StringList(value) => {
                        let a = value.as_array().map_or(&[][..], |a| a.as_slice());
                        Cursor::Text(a[array_index(&segment, a.len())?].as_str().unwrap())
                    }
                    Cursor::AnyList(value) => {
                        let a = value.as_array().map_or(&[][..], |a| a.as_slice());
                        Cursor::Generic(&a[array_index(&segment, a.len())?])
                    }
                    Cursor::StringMap(value) => Cursor::StringList(
                        std::str::from_utf8(&segment)
                            .ok()
                            .and_then(|key| value.get(key))
                            .ok_or_else(|| {
                                format!("no key {} in map", uri::quote_bytes(&segment))
                            })?,
                    ),
                    Cursor::BoolMap(value) => Cursor::Bool(
                        std::str::from_utf8(&segment)
                            .ok()
                            .and_then(|key| value.get(key))
                            .ok_or_else(|| format!("no key {} in map", uri::quote_bytes(&segment)))?
                            .as_bool()
                            .unwrap(),
                    ),
                    Cursor::Raw(value) => {
                        let bytes = value.map_or(&[][..], |n| n.raw.as_slice());
                        Cursor::Byte(bytes[array_index(&segment, bytes.len())?])
                    }
                    other => {
                        let text = match &other {
                            Cursor::Text(s) => s.to_string(),
                            Cursor::Bool(b) => format!("%!s(bool={b})"),
                            Cursor::Byte(b) => format!("%!s(uint8={b})"),
                            Cursor::Generic(v) => fmt_s(v),
                            _ => unreachable!(),
                        };
                        return Err(format!(
                            "value {text} ({}) is not a schema, slice or map",
                            other.ty()
                        ));
                    }
                };
            }
        }
        match cursor {
            Cursor::Schema(target) => Ok(target),
            other => Err(format!(
                "does not refer to a schema, but to a {}",
                other.ty()
            )),
        }
    };
    traverse().map_err(|cause| format!("JSON Pointer {}: {cause}", uri::quote_bytes(pointer)))
}

/// The registry compiler uses Resolve(nil): no remote schema loader or default
/// validation. Keep lexical schema identity separate from URI identity, which
/// may be overwritten by later IDs in the owning traversal.
struct Info {
    base: String,
    uri: Option<uri::Uri>,
    reference: Option<String>,
    dynamic_reference: Option<String>,
    dynamic_anchor: String,
    anchors: Option<BTreeMap<String, (String, bool)>>,
    required: Option<BTreeMap<String, bool>>,
}
impl Info {
    fn new(schema: &Schema) -> Self {
        let required = match &schema.fields["Required"] {
            Slot::Strings(Some((values, len))) if *len > 0 => {
                Some(values[..*len].iter().map(|v| (v.clone(), true)).collect())
            }
            _ => None,
        };
        Self {
            base: String::new(),
            uri: None,
            reference: None,
            dynamic_reference: None,
            dynamic_anchor: String::new(),
            anchors: None,
            required,
        }
    }
}
pub(super) struct Graph<'a> {
    index: BTreeMap<String, &'a Schema>,
    paths: HashMap<*const Schema, String>,
    infos: BTreeMap<String, Info>,
    uris: BTreeMap<String, String>,
    draft7: bool,
}
impl Graph<'_> {
    fn anchor(&mut self, base: &str, node: &str, name: &str, dynamic: bool) {
        if name.is_empty() {
            return;
        }
        // resolveURIs ignores setAnchor's duplicate error. Preserve the first
        // lexical registration rather than introducing a stricter policy.
        self.infos
            .get_mut(base)
            .unwrap()
            .anchors
            .get_or_insert_with(BTreeMap::new)
            .entry(name.into())
            .or_insert_with(|| (node.into(), dynamic));
    }
    fn visit(&mut self, schema: &Schema, mut base: String) -> Result<(), String> {
        let path = self.paths[&(schema as *const Schema)].clone();
        let id = text(schema, "ID");
        if !id.is_empty() && (!self.draft7 || text(schema, "Ref").is_empty()) {
            let id_uri = uri::Uri::parse(id)?;
            if !self.draft7 && !id_uri.fragment.is_empty() {
                return Err(format!("$id {id} must not have a fragment"));
            }
            if self.draft7 && !id_uri.fragment.is_empty() {
                self.anchor(&base, &path, id.strip_prefix('#').unwrap_or(id), false);
            } else {
                let resolved = self.infos[&base].uri.as_ref().unwrap().resolve(&id_uri);
                let absolute = !resolved.scheme.is_empty();
                let name = resolved.string();
                self.infos.get_mut(&path).unwrap().uri = Some(resolved);
                if !absolute {
                    // Root and base can alias; the Go error observes the newly
                    // assigned URI in that case, not its previous value.
                    return Err(format!(
                        "$id {id} does not resolve to an absolute URI (base is {})",
                        quote(&self.infos[&base].uri.as_ref().unwrap().string())
                    ));
                }
                self.uris.insert(name, path.clone());
                base = path.clone();
            }
        }
        self.infos.get_mut(&path).unwrap().base = base.clone();
        if !self.draft7 {
            self.anchor(&base, &path, text(schema, "Anchor"), false);
            self.anchor(&base, &path, text(schema, "DynamicAnchor"), true);
        }
        for (_, slot) in schema_fields(schema) {
            match slot {
                Slot::Child(Some(child)) => self.visit(child, base.clone())?,
                Slot::Children(Some((children, len))) => {
                    for child in children[..*len].iter().flatten() {
                        self.visit(child, base.clone())?;
                    }
                }
                Slot::ChildMap(Some(children)) => {
                    for child in children.values().flatten() {
                        self.visit(child, base.clone())?;
                    }
                }
                _ => (),
            }
        }
        Ok(())
    }
    fn reference(&self, schema: &Schema, value: &str) -> Result<(Option<String>, String), String> {
        let parsed = uri::Uri::parse(value)?;
        let info = &self.infos[&self.paths[&(schema as *const Schema)]];
        let resolved = self.infos[&info.base]
            .uri
            .as_ref()
            .unwrap()
            .resolve(&parsed);
        let mut no_fragment = resolved.clone();
        no_fragment.fragment.clear();
        let key = no_fragment.string();
        let target = self.uris.get(&key).ok_or_else(|| {
            format!(
                "loading {key}: cannot resolve remote schemas: no loader passed to Schema.Resolve"
            )
        })?;
        if !resolved.fragment.is_empty() && !resolved.fragment.starts_with(b"/") {
            let anchor = self.infos[target]
                .anchors
                .as_ref()
                .and_then(|anchors| {
                    // Anchor names are valid UTF-8 schema strings; percent-decoded
                    // invalid bytes cannot equal any map key.
                    std::str::from_utf8(&resolved.fragment)
                        .ok()
                        .and_then(|name| anchors.get(name))
                })
                .ok_or_else(|| {
                    format!(
                        "no anchor {} in {}",
                        uri::quote_bytes(&resolved.fragment),
                        description(schema)
                    )
                })?;
            return Ok((
                Some(anchor.0.clone()),
                if anchor.1 {
                    String::from_utf8(resolved.fragment).unwrap()
                } else {
                    String::new()
                },
            ));
        }
        let node = pointer_bytes(self.index[target], &resolved.fragment)?;
        Ok((
            node.map(|node| self.paths[&(node as *const Schema)].clone()),
            String::new(),
        ))
    }
    #[cfg(test)]
    pub(super) fn projection(&self) -> J {
        let infos:BTreeMap<_,_>=self.infos.iter().map(|(path,info)| {
            let anchors=info.anchors.as_ref().map(|a|a.iter().map(|(key,(path,dynamic))|(key.clone(),json!({"path":path,"dynamic":dynamic}))).collect::<BTreeMap<_,_>>());
            (path,json!({"base":info.base,"uri":info.uri.as_ref().map(uri::Uri::string),"ref":info.reference,
                "dynamicRef":info.dynamic_reference,"dynamicAnchor":info.dynamic_anchor,"anchors":anchors,"required":info.required}))
        }).collect();
        json!({"error":"","draft":if self.draft7 {0}else{1},"uris":self.uris,"infos":infos})
    }
}
pub(super) fn graph<'a>(
    root: &'a Schema,
    base: &str,
    refs: bool,
    compile_pattern: &dyn Fn(&str) -> Result<(), String>,
) -> Result<Graph<'a>, String> {
    let base_uri = if base.is_empty() {
        uri::Uri::default()
    } else {
        uri::Uri::parse(base).map_err(|error| format!("parsing base URI: {error}"))?
    };
    if !base_uri.fragment.is_empty() {
        return Err(format!(
            "base URI {} must not have a fragment",
            base_uri.string()
        ));
    }
    local(root, compile_pattern)?;
    let index = structure(root)?;
    let paths = index
        .iter()
        .map(|(path, schema)| (*schema as *const Schema, path.clone()))
        .collect();
    let infos = index
        .iter()
        .map(|(path, schema)| (path.clone(), Info::new(schema)))
        .collect();
    let draft7 = matches!(
        text(root, "Schema"),
        "http://json-schema.org/draft-07/schema#" | "https://json-schema.org/draft-07/schema#"
    );
    let mut graph = Graph {
        index,
        paths,
        infos,
        uris: BTreeMap::new(),
        draft7,
    };
    graph.uris.insert(base_uri.string(), "root".into());
    graph.infos.get_mut("root").unwrap().uri = Some(base_uri);
    graph.visit(root, "root".into())?;
    if refs {
        // The actual loaded-document cache also binds the original base and
        // canonical root URI. Those entries are already in the local URI map.
        for schema in preorder(root) {
            let path = graph.paths[&(schema as *const Schema)].clone();
            if !text(schema, "Ref").is_empty() {
                let (target, _) = graph.reference(schema, text(schema, "Ref"))?;
                graph.infos.get_mut(&path).unwrap().reference = target;
            }
            if !text(schema, "DynamicRef").is_empty() {
                let (target, dynamic) = graph.reference(schema, text(schema, "DynamicRef"))?;
                let info = graph.infos.get_mut(&path).unwrap();
                if !dynamic.is_empty() {
                    info.dynamic_anchor = dynamic;
                } else {
                    info.dynamic_reference = target;
                }
            }
        }
    }
    Ok(graph)
}
