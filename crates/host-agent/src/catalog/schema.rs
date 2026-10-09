//! Typed decoding for the pinned google/jsonschema-go v0.4.3 compiler.
//!
//! The private compiler assembly is deliberately not wired to registry
//! admission pending provider registry and lifecycle validation. It retains
//! typed children and union fields rather than traversing generic JSON.
//! Integer keywords use the owner's custom int32 decoder,
//! rather than a meta-schema rule (negative bounds are accepted by the owner).
use crate::gojson::{Node, Value};
use crate::{goerr::quote, gojson};
use serde_json::{json, Map, Value as J};
use std::collections::BTreeMap;

mod regexp;
mod regexp_names;
mod regexp_tree;
mod resolve;
mod uri;

// Schema.UnmarshalJSON decodes through an anonymous embedded Go struct.
// Keep its error context here; catalog compilation does not depend on MCP.
struct TypeError {
    raw: Option<String>,
    value: String,
    ty: String,
    field: String,
}
impl TypeError {
    fn message(&self) -> String {
        if let Some(raw) = &self.raw {
            return raw.clone();
        }
        if self.field.is_empty() {
            format!(
                "json: cannot unmarshal {} into Go value of type {}",
                self.value, self.ty
            )
        } else {
            format!(
                "json: cannot unmarshal {} into Go struct field .{} of type {}",
                self.value, self.field, self.ty
            )
        }
    }
}
impl From<String> for TypeError {
    fn from(raw: String) -> Self {
        Self {
            raw: Some(raw),
            value: String::new(),
            ty: String::new(),
            field: String::new(),
        }
    }
}
impl From<&str> for TypeError {
    fn from(raw: &str) -> Self {
        raw.to_string().into()
    }
}

#[derive(Clone, Copy)]
enum Kind {
    Text,
    Bool,
    Float,
    Integer,
    AnyList,
    TextList,
    TextMap,
    BoolMap,
    Child,
    ChildList,
    ChildMap,
    Raw,
    Special,
}
use Kind::*;
// This is the pinned compiler's typed keyword inventory, not a second tool
// catalog or a meta-schema. The non-JSON union fields are handled separately.
const FIELDS: &[(&str, &str, Kind)] = &[
    ("$id", "ID", Text),
    ("$schema", "Schema", Text),
    ("$ref", "Ref", Text),
    ("$comment", "Comment", Text),
    ("$defs", "Defs", ChildMap),
    ("definitions", "Definitions", ChildMap),
    ("$anchor", "Anchor", Text),
    ("$dynamicAnchor", "DynamicAnchor", Text),
    ("$dynamicRef", "DynamicRef", Text),
    ("$vocabulary", "Vocabulary", BoolMap),
    ("title", "Title", Text),
    ("description", "Description", Text),
    ("default", "Default", Raw),
    ("deprecated", "Deprecated", Bool),
    ("readOnly", "ReadOnly", Bool),
    ("writeOnly", "WriteOnly", Bool),
    ("examples", "Examples", AnyList),
    ("enum", "Enum", AnyList),
    ("multipleOf", "MultipleOf", Float),
    ("minimum", "Minimum", Float),
    ("maximum", "Maximum", Float),
    ("exclusiveMinimum", "ExclusiveMinimum", Float),
    ("exclusiveMaximum", "ExclusiveMaximum", Float),
    ("minLength", "MinLength", Integer),
    ("maxLength", "MaxLength", Integer),
    ("pattern", "Pattern", Text),
    ("prefixItems", "PrefixItems", ChildList),
    ("minItems", "MinItems", Integer),
    ("maxItems", "MaxItems", Integer),
    ("additionalItems", "AdditionalItems", Child),
    ("uniqueItems", "UniqueItems", Bool),
    ("contains", "Contains", Child),
    ("minContains", "MinContains", Integer),
    ("maxContains", "MaxContains", Integer),
    ("unevaluatedItems", "UnevaluatedItems", Child),
    ("minProperties", "MinProperties", Integer),
    ("maxProperties", "MaxProperties", Integer),
    ("required", "Required", TextList),
    ("dependentRequired", "DependentRequired", TextMap),
    ("properties", "Properties", ChildMap),
    ("patternProperties", "PatternProperties", ChildMap),
    ("additionalProperties", "AdditionalProperties", Child),
    ("propertyNames", "PropertyNames", Child),
    ("unevaluatedProperties", "UnevaluatedProperties", Child),
    ("allOf", "AllOf", ChildList),
    ("anyOf", "AnyOf", ChildList),
    ("oneOf", "OneOf", ChildList),
    ("not", "Not", Child),
    ("if", "If", Child),
    ("then", "Then", Child),
    ("else", "Else", Child),
    ("dependentSchemas", "DependentSchemas", ChildMap),
    ("contentEncoding", "ContentEncoding", Text),
    ("contentMediaType", "ContentMediaType", Text),
    ("contentSchema", "ContentSchema", Child),
    ("format", "Format", Text),
    ("type", "Type", Special),
    ("items", "Items", Special),
    ("dependencies", "DependencySchemas", Special),
    ("const", "Const", Special),
];

#[derive(Clone)]
enum Slot {
    Scalar(J),
    Presence(Option<J>),
    Raw(Option<Node>),
    Child(Option<Box<Schema>>),
    Children(Option<(Vec<Option<Schema>>, usize)>),
    ChildMap(Option<BTreeMap<String, Option<Schema>>>),
    Strings(Option<(Vec<String>, usize)>),
}
#[derive(Clone)]
struct Schema {
    fields: BTreeMap<&'static str, Slot>,
}
impl Schema {
    fn empty() -> Self {
        let mut fields = BTreeMap::new();
        for &(_, name, kind) in FIELDS {
            fields.insert(
                name,
                match kind {
                    Text => Slot::Scalar(J::String(String::new())),
                    Bool => Slot::Scalar(J::Bool(false)),
                    Child => Slot::Child(None),
                    ChildList => Slot::Children(None),
                    ChildMap => Slot::ChildMap(None),
                    TextList => Slot::Strings(None),
                    Raw => Slot::Raw(None),
                    _ => Slot::Scalar(J::Null),
                },
            );
        }
        fields.insert("Type", Slot::Scalar(J::String(String::new())));
        fields.insert("Types", Slot::Strings(None));
        fields.insert("Items", Slot::Child(None));
        fields.insert("ItemsArray", Slot::Children(None));
        fields.insert("DependencySchemas", Slot::ChildMap(None));
        fields.insert("DependencyStrings", Slot::Scalar(J::Null));
        fields.insert("Const", Slot::Presence(None));
        fields.insert("Extra", Slot::Scalar(J::Null));
        fields.insert("PropertyOrder", Slot::Strings(None));
        Self { fields }
    }
    #[cfg(test)]
    fn projection(&self) -> J {
        J::Object(
            self.fields
                .iter()
                .map(|(&key, slot)| {
                    let value = match slot {
                    Slot::Scalar(v) => project_value(v),
                    Slot::Presence(v) => json!({"present":v.is_some(),"value":v.as_ref().map(project_value)}),
                    Slot::Raw(v) => json!({"present":v.is_some(),"value":v.as_ref().map(|n|project_value(&any(n))),
                        "raw":v.as_ref().map_or("",|n|std::str::from_utf8(&n.raw).unwrap())}),
                        Slot::Child(v) => v.as_ref().map_or(J::Null, |s| s.projection()),
                        Slot::Children(v) => v.as_ref().map_or(J::Null, |(slots, len)| {
                            J::Array(
                                slots[..*len]
                                    .iter()
                                    .map(|s| s.as_ref().map_or(J::Null, Schema::projection))
                                    .collect(),
                            )
                        }),
                        Slot::ChildMap(v) => v.as_ref().map_or(J::Null, |m| {
                            J::Object(
                                m.iter()
                                    .map(|(k, s)| {
                                        (k.clone(), s.as_ref().map_or(J::Null, Schema::projection))
                                    })
                                    .collect(),
                            )
                        }),
                        Slot::Strings(v) => v
                            .as_ref()
                            .map_or(J::Null, |(slots, len)| json!(&slots[..*len])),
                    };
                    (key.to_string(), value)
                })
                .collect(),
        )
    }
}

#[cfg(test)]
fn project_value(value: &J) -> J {
    match value {
        J::Number(n) if n.is_f64() => {
            json!({"$float64":format!("{:016x}",n.as_f64().unwrap().to_bits()),"value":value})
        }
        J::Array(a) => J::Array(a.iter().map(project_value).collect()),
        J::Object(m) => J::Object(
            m.iter()
                .map(|(k, v)| (k.clone(), project_value(v)))
                .collect(),
        ),
        _ => value.clone(),
    }
}

fn any(node: &Node) -> J {
    match &node.value {
        Value::Null => J::Null,
        Value::Bool(b) => J::Bool(*b),
        Value::String(s) => J::String(s.clone()),
        Value::Number(n) => J::Number(
            serde_json::Number::from_f64(n.parse().expect("finite generic map number")).unwrap(),
        ),
        Value::Array(a) => J::Array(a.iter().map(any).collect()),
        Value::Object(m) => J::Object(m.iter().map(|(k, n)| (k.clone(), any(n))).collect()),
    }
}
fn mismatch(node: &Node, ty: &str) -> TypeError {
    let value = match node.value {
        Value::Object(_) => "object",
        Value::Array(_) => "array",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Null => "null",
    };
    TypeError {
        raw: None,
        value: value.into(),
        ty: ty.into(),
        field: String::new(),
    }
}
fn context(mut error: TypeError, field: &str) -> TypeError {
    if error.raw.is_none() {
        error.field = if error.field.is_empty() {
            field.into()
        } else {
            format!("{field}.{}", error.field)
        };
    }
    error
}
fn anonymous_type() -> String {
    let mut members = vec![
        ("Type", "json.RawMessage", "type"),
        ("Dependencies", "map[string]json.RawMessage", "dependencies"),
        ("Items", "json.RawMessage", "items"),
        ("Const", "json.RawMessage", "const"),
    ];
    for key in [
        "minLength",
        "maxLength",
        "minItems",
        "maxItems",
        "minProperties",
        "maxProperties",
        "minContains",
        "maxContains",
    ] {
        let &(_, name, _) = FIELDS.iter().find(|(k, _, _)| *k == key).unwrap();
        members.push((name, "*jsonschema.integer", key));
    }
    let mut out = String::from("struct { ");
    for (name, ty, key) in members {
        out.push_str(&format!(
            "{name} {ty} {}; ",
            quote(&format!("json:\"{key},omitempty\""))
        ));
    }
    out.push_str("*jsonschema.schemaWithoutMethods }");
    out
}

// Ordinary encoding/json type errors are remembered while decoding continues.
// A custom integer/child-schema decoder error stops immediately, even if its
// underlying error happens to be an UnmarshalTypeError.
struct Failure {
    error: Box<TypeError>,
    fatal: bool,
}
fn ordinary(error: TypeError) -> Failure {
    Failure {
        error: Box::new(error),
        fatal: false,
    }
}
fn fatal(error: TypeError) -> Failure {
    Failure {
        error: Box::new(error),
        fatal: true,
    }
}

fn strings(node: &Node, old: &mut Option<(Vec<String>, usize)>) -> Result<(), TypeError> {
    if node.is_null() {
        *old = None;
        return Ok(());
    }
    let Value::Array(items) = &node.value else {
        return Err(mismatch(node, "[]string"));
    };
    if items.is_empty() {
        *old = Some((Vec::new(), 0));
        return Ok(());
    }
    let (slots, len) = old.get_or_insert_with(|| (Vec::new(), 0));
    let mut first = None;
    for (i, n) in items.iter().enumerate() {
        if i == slots.len() {
            slots.push(String::new());
        }
        match &n.value {
            Value::String(s) => slots[i] = s.clone(),
            Value::Null => (),
            _ => {
                if first.is_none() {
                    first = Some(mismatch(n, "string"));
                }
            }
        }
    }
    *len = items.len();
    first.map_or(Ok(()), Err)
}
fn children(node: &Node, old: &mut Option<(Vec<Option<Schema>>, usize)>) -> Result<(), Failure> {
    if node.is_null() {
        *old = None;
        return Ok(());
    }
    let Value::Array(items) = &node.value else {
        return Err(ordinary(mismatch(node, "[]*jsonschema.Schema")));
    };
    if items.is_empty() {
        *old = Some((Vec::new(), 0));
        return Ok(());
    }
    let (slots, len) = old.get_or_insert_with(|| (Vec::new(), 0));
    for (i, n) in items.iter().enumerate() {
        if i == slots.len() {
            slots.push(None);
        }
        if n.is_null() {
            slots[i] = None;
        } else {
            let s = slots[i].get_or_insert_with(Schema::empty);
            decode_into(n, s).map_err(fatal)?;
        }
    }
    *len = items.len();
    Ok(())
}
fn field(node: &Node, kind: Kind, slot: &mut Slot) -> Result<(), Failure> {
    let wrong = |ty: &str| ordinary(mismatch(node, ty));
    match (kind, slot) {
        (Text, Slot::Scalar(v)) => {
            if node.is_null() {
                return Ok(());
            }
            *v = J::String(node.as_str().ok_or_else(|| wrong("string"))?.into());
        }
        (Bool, Slot::Scalar(v)) => {
            if node.is_null() {
                return Ok(());
            }
            let Value::Bool(b) = node.value else {
                return Err(wrong("bool"));
            };
            *v = J::Bool(b);
        }
        (Float, Slot::Scalar(v)) => {
            if node.is_null() {
                *v = J::Null;
                return Ok(());
            }
            if !matches!(node.value, Value::Number(_)) {
                return Err(wrong("float64"));
            }
            *v = any(node);
        }
        (Raw, Slot::Raw(v)) => {
            *v = Some(node.clone());
        }
        (AnyList, Slot::Scalar(v)) => {
            if !node.is_null() && !matches!(node.value, Value::Array(_)) {
                return Err(wrong("[]interface {}"));
            }
            *v = any(node);
        }
        (TextList, Slot::Strings(v)) => strings(node, v).map_err(ordinary)?,
        (Child, Slot::Child(v)) => {
            if node.is_null() {
                *v = None;
            } else {
                decode_into(node, v.get_or_insert_with(|| Box::new(Schema::empty())))
                    .map_err(fatal)?;
            }
        }
        (ChildList, Slot::Children(v)) => children(node, v)?,
        (ChildMap, Slot::ChildMap(v)) => {
            if node.is_null() {
                *v = None;
                return Ok(());
            }
            let m = node
                .as_object()
                .ok_or_else(|| wrong("map[string]*jsonschema.Schema"))?;
            let target = v.get_or_insert_with(BTreeMap::new);
            for (key, n) in m {
                let child = if n.is_null() {
                    None
                } else {
                    let mut s = Schema::empty();
                    decode_into(n, &mut s).map_err(fatal)?;
                    Some(s)
                };
                target.insert(key.clone(), child);
            }
        }
        (TextMap | BoolMap, Slot::Scalar(v)) => {
            if node.is_null() {
                *v = J::Null;
                return Ok(());
            }
            let m = node.as_object().ok_or_else(|| {
                wrong(if matches!(kind, TextMap) {
                    "map[string][]string"
                } else {
                    "map[string]bool"
                })
            })?;
            let mut target = v.as_object().cloned().unwrap_or_default();
            let mut first = None;
            for (key, n) in m {
                let result = if matches!(kind, TextMap) {
                    let mut s = None;
                    strings(n, &mut s)
                        .map(|()| s.map_or(J::Null, |(slots, len)| json!(&slots[..len])))
                } else {
                    match n.value {
                        Value::Bool(b) => Ok(J::Bool(b)),
                        Value::Null => Ok(J::Bool(false)),
                        _ => Err(mismatch(n, "bool")),
                    }
                };
                match result {
                    Ok(value) => {
                        target.insert(key.clone(), value);
                    }
                    Err(e) => {
                        if first.is_none() {
                            first = Some(e);
                        }
                    }
                }
            }
            *v = J::Object(target);
            if let Some(e) = first {
                return Err(ordinary(e));
            }
        }
        _ => unreachable!("typed schema field inventory"),
    }
    Ok(())
}

fn decode_into(node: &Node, out: &mut Schema) -> Result<(), TypeError> {
    if matches!(node.value, Value::Bool(_) | Value::Null) {
        *out = Schema::empty();
        if !matches!(node.value, Value::Bool(true)) {
            out.fields
                .insert("Not", Slot::Child(Some(Box::new(Schema::empty()))));
        }
        return Ok(());
    }
    let members = node
        .as_object()
        .ok_or_else(|| mismatch(node, &anonymous_type()))?;
    let mut special: BTreeMap<&str, Node> = BTreeMap::new();
    let mut dependencies: Option<BTreeMap<String, Node>> = None;
    let mut integers = BTreeMap::new();
    let mut first = None;
    for (key, n) in members {
        let Some(&(canonical, name, kind)) = FIELDS
            .iter()
            .find(|(k, _, _)| *k == key)
            .or_else(|| FIELDS.iter().find(|(k, _, _)| gojson::equal_fold(k, key)))
        else {
            continue;
        };
        if matches!(kind, Integer) {
            integers.insert(name, integer(n).map_err(TypeError::from)?);
            continue;
        }
        if matches!(kind, Special) {
            if canonical == "dependencies" {
                if n.is_null() {
                    dependencies = None;
                } else if let Some(map) = n.as_object() {
                    dependencies
                        .get_or_insert_with(BTreeMap::new)
                        .extend(map.iter().cloned());
                } else if first.is_none() {
                    first = Some(context(
                        mismatch(n, "map[string]json.RawMessage"),
                        "dependencies",
                    ));
                }
            } else {
                special.insert(canonical, n.clone());
            }
            continue;
        }
        match field(n, kind, out.fields.get_mut(name).expect("schema inventory")) {
            Ok(()) => (),
            Err(f) => {
                let e = context(*f.error, &format!("schemaWithoutMethods.{canonical}"));
                if f.fatal {
                    return Err(e);
                }
                if first.is_none() {
                    first = Some(e);
                }
            }
        }
    }
    if let Some(error) = first {
        return Err(error);
    }
    let extra: Map<String, J> = members
        .iter()
        .filter(|(k, _)| !FIELDS.iter().any(|(field, _, _)| field == k))
        .map(|(k, n)| (k.clone(), any(n)))
        .collect();
    if !extra.is_empty() {
        out.fields.insert("Extra", Slot::Scalar(J::Object(extra)));
    }
    if let Some(n) = special.get("type") {
        match &n.value {
            Value::String(s) => {
                out.fields
                    .insert("Type", Slot::Scalar(J::String(s.clone())));
            }
            Value::Array(_) => {
                let Slot::Strings(slot) = out.fields.get_mut("Types").unwrap() else {
                    unreachable!()
                };
                strings(n, slot)?;
            }
            _ => {
                return Err(format!(
                    "invalid value for \"type\": {}",
                    quote(std::str::from_utf8(&n.raw).unwrap())
                )
                .into())
            }
        }
    }
    if let Some(n) = special.get("items") {
        if matches!(n.value, Value::Array(_)) {
            // ItemsArray is decoded into a fresh local slice, not its old backing array.
            let mut slot = None;
            children(n, &mut slot).map_err(|f| *f.error)?;
            out.fields.insert("ItemsArray", Slot::Children(slot));
        } else {
            let mut child = Schema::empty();
            decode_into(n, &mut child)?;
            out.fields
                .insert("Items", Slot::Child(Some(Box::new(child))));
        }
    }
    if let Some(deps) = dependencies {
        for (key, n) in deps {
            if matches!(n.value, Value::Array(_)) {
                let mut slot = None;
                strings(&n, &mut slot)?;
                let Slot::Scalar(old) = out.fields.get_mut("DependencyStrings").unwrap() else {
                    unreachable!()
                };
                let mut map = old.as_object().cloned().unwrap_or_default();
                map.insert(
                    key,
                    slot.map_or(J::Null, |(slots, len)| json!(&slots[..len])),
                );
                *old = J::Object(map);
            } else {
                let mut child = Schema::empty();
                decode_into(&n, &mut child)?;
                let Slot::ChildMap(old) = out.fields.get_mut("DependencySchemas").unwrap() else {
                    unreachable!()
                };
                old.get_or_insert_with(BTreeMap::new)
                    .insert(key, Some(child));
            }
        }
    }
    if let Some(n) = special.get("const") {
        out.fields.insert("Const", Slot::Presence(Some(any(n))));
    }
    for (name, value) in integers {
        if let Some(value) = value {
            out.fields.insert(name, Slot::Scalar(J::from(value)));
        }
    }
    Ok(())
}

fn decode(value: &Map<String, J>) -> Result<Schema, TypeError> {
    // Generic map numbers are float64 before the owning Go marshal step.
    let original = gojson::parse(&serde_json::to_vec(value).unwrap()).unwrap();
    let mut raw = String::new();
    gojson::encode(&any(&original), &mut raw);
    let node = gojson::parse(raw.as_bytes()).unwrap();
    let mut schema = Schema::empty();
    decode_into(&node, &mut schema)?;
    Ok(schema)
}

/// Assemble the owning compiler phases without enabling provider admission.
/// `Map` holds JSON-only values; decoding preserves Go's float64 marshal step.
fn check(label: &str, value: &Map<String, J>) -> Result<(), String> {
    let schema = decode(value)
        .map_err(|error| format!("{label} schema is malformed: {}", error.message()))?;
    resolve::graph(&schema, "", true, &regexp::parse)
        .map(|_| ())
        .map_err(|error| format!("{label} schema is invalid: {error}"))
}

/// Decode a nullable `*jsonschema.integer` from already validated JSON. The
/// owning boundary first marshals generic float64 map values; callers must
/// pass those canonical bytes, not the original manifest's number spelling.
fn integer(node: &Node) -> Result<Option<i32>, &'static str> {
    if node.is_null() {
        return Ok(None);
    }
    let Value::Number(text) = &node.value else {
        return Err(if node.raw.contains(&b'.') {
            "not a number"
        } else {
            "cannot be unmarshaled into an int"
        });
    };
    let value = if node.raw.contains(&b'.') {
        let f = text.parse::<f64>().map_err(|_| "not a number")?;
        // Go's float64-to-int64 conversion on the supported Linux targets
        // yields MinInt64 for out-of-range values. Such values cannot round
        // trip, except MinInt64 itself, which fails the int32 range below.
        if !f.is_finite()
            || !(-9_223_372_036_854_775_808.0..9_223_372_036_854_775_808.0).contains(&f)
        {
            return Err("not an integer value");
        }
        let i = f as i64;
        if i as f64 != f {
            return Err("not an integer value");
        }
        i
    } else {
        text.parse::<i64>()
            .map_err(|_| "cannot be unmarshaled into an int")?
    };
    i32::try_from(value)
        .map(Some)
        .map_err(|_| "integer is out of range")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gojson;
    use serde_json::{json, Value as J};

    #[test]
    #[ignore = "requires actual pinned Go catalog compiler"]
    fn compiler_fixture() {
        let input = std::env::var("PARITY_SCHEMA_COMPILER_INPUT").unwrap();
        let output = std::env::var("PARITY_SCHEMA_COMPILER_OUTPUT").unwrap();
        let node = gojson::parse(&std::fs::read(input).unwrap()).unwrap();
        let rows = any(&node);
        let values: Vec<J> = rows.as_array().unwrap().iter().map(|row| {
            json!({"error": check("fixture", row["schema"].as_object().unwrap()).err().unwrap_or_default()})
        }).collect();
        std::fs::write(output, serde_json::to_vec(&values).unwrap()).unwrap();
    }

    #[test]
    #[ignore = "requires actual pinned Go regexp compiler"]
    fn regexp_fixture() {
        let input = std::env::var("PARITY_SCHEMA_REGEXP_INPUT").unwrap();
        let output = std::env::var("PARITY_SCHEMA_REGEXP_OUTPUT").unwrap();
        let rows: Vec<J> = serde_json::from_slice(&std::fs::read(input).unwrap()).unwrap();
        let values:Vec<J>=rows.iter().map(|row|json!({"error":regexp::parse(row["regexp"].as_str().unwrap()).err().unwrap_or_default()})).collect();
        std::fs::write(output, serde_json::to_vec(&values).unwrap()).unwrap();
    }

    #[test]
    #[ignore = "requires pinned Go net/url oracle"]
    fn uri_fixture() {
        let input = std::env::var("PARITY_SCHEMA_URI_INPUT").unwrap();
        let output = std::env::var("PARITY_SCHEMA_URI_OUTPUT").unwrap();
        let rows: Vec<J> = serde_json::from_slice(&std::fs::read(input).unwrap()).unwrap();
        let hex = |bytes: &[u8]| bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
        let results: Vec<J> = rows.iter().map(|row| {
            let r = &row["uri"];
            let u = uri::Uri::parse(r["base"].as_str().unwrap()).and_then(|base| {
                match r["ref"].as_str() { Some(reference) => uri::Uri::parse(reference).map(|r| base.resolve(&r)), None => Ok(base) }
            });
            match u {
                Err(error) => json!({"error":error}),
                Ok(u) => json!({"error":"","scheme":u.scheme,"opaque":u.opaque,
                    "user":u.user.as_ref().map(|(n,p)|json!({"name":hex(n),"password":p.as_ref().map(|p|hex(p))})),
                    "host":hex(&u.host),"path":hex(&u.path),"rawPath":u.raw_path,"omitHost":u.omit_host,
                    "forceQuery":u.force_query,"query":u.query,"fragment":hex(&u.fragment),"rawFragment":u.raw_fragment,
                    "escapedPath":u.escaped_path(),"escapedFragment":u.escaped_fragment(),"string":u.string()}),
            }
        }).collect();
        std::fs::write(output, serde_json::to_vec(&results).unwrap()).unwrap();
    }

    #[test]
    #[ignore = "requires the actual owning schema pointer/structure oracle"]
    fn resolver_fixture() {
        let input = std::env::var("PARITY_SCHEMA_RESOLVER_INPUT").unwrap();
        let output = std::env::var("PARITY_SCHEMA_RESOLVER_OUTPUT").unwrap();
        let node = gojson::parse(&std::fs::read(input).unwrap()).unwrap();
        let rows = any(&node);
        let results:Vec<J>=rows.as_array().unwrap().iter().map(|row| {
            let schema=match decode(row["schema"].as_object().unwrap()) {
                Ok(schema)=>schema,Err(e)=>return json!({"error":format!("fixture schema is malformed: {}",e.message())}),
            };
            match row["resolverAction"].as_str().unwrap() {
                "graph"|"uris"=>{
                    match resolve::graph(&schema,row["baseURI"].as_str().unwrap_or(""),row["resolverAction"]=="graph",&|pattern| {
                        assert!(pattern.is_empty(),"graph fixture excludes regexp compilation");Ok(())
                    }) { Ok(graph)=>graph.projection(),Err(error)=>json!({"error":error}) }
                },
                "local"=>{
                    let error=resolve::local(&schema,&|pattern| {
                        assert!(pattern.is_empty(),"local fixture excludes regexp compilation unless basic checks bypass it");
                        Ok(())
                    }).err().unwrap_or_default();json!({"error":error})
                },
                "structure"=>match resolve::structure(&schema) {
                    Ok(index)=>json!({"error":"","paths":index.keys().collect::<Vec<_>>()}),
                    Err(error)=>json!({"error":error,"paths":null}),
                },
                "pointer"=>match resolve::pointer(&schema,row["pointer"].as_str().unwrap()) {
                    Ok(target)=>json!({"error":"","nil":target.is_none(),"schema":target.map(Schema::projection)}),
                    Err(error)=>json!({"error":error,"nil":true,"schema":null}),
                },
                _=>panic!("unknown schema resolver action"),
            }
        }).collect();
        std::fs::write(output, serde_json::to_vec(&results).unwrap()).unwrap();
    }

    #[test]
    #[ignore = "requires the actual pinned typed schema decoder oracle"]
    fn decoder_fixture() {
        let input = std::env::var("PARITY_SCHEMA_DECODER_INPUT").unwrap();
        let output = std::env::var("PARITY_SCHEMA_DECODER_OUTPUT").unwrap();
        let rows: Vec<J> = serde_json::from_slice(&std::fs::read(input).unwrap()).unwrap();
        let results:Vec<J>=rows.iter().map(|row|match decode(row["schema"].as_object().unwrap()) {
            Ok(schema)=>json!({"error":"","schema":schema.projection()}),
            Err(error)=>json!({"error":format!("fixture schema is malformed: {}",error.message()),"schema":null}),
        }).collect();
        std::fs::write(output, serde_json::to_vec(&results).unwrap()).unwrap();
    }

    /// Explicitly invoked by the differential driver. Only integer keyword
    /// decoding is exercised; it must never masquerade as full compilation.
    #[test]
    #[ignore]
    fn integer_fixture() {
        let input = std::env::var("PARITY_SCHEMA_INTEGER_INPUT").unwrap();
        let output = std::env::var("PARITY_SCHEMA_INTEGER_OUTPUT").unwrap();
        let rows: Vec<J> = serde_json::from_slice(&std::fs::read(input).unwrap()).unwrap();
        let results: Vec<J> = rows
            .iter()
            .map(|row| {
                let fields = row["schema"].as_object().unwrap();
                assert_eq!(fields.len(), 1, "integer fixture accepts exactly one field");
                let (key, value) = fields.iter().next().unwrap();
                assert!([
                    "minLength",
                    "maxLength",
                    "minItems",
                    "maxItems",
                    "minProperties",
                    "maxProperties",
                    "minContains",
                    "maxContains",
                ]
                .contains(&key.as_str()));
                let mut encoded = String::new();
                if let Some(number) = value.as_f64() {
                    gojson::encode_float(number, &mut encoded);
                } else {
                    gojson::encode(value, &mut encoded);
                }
                let node = gojson::parse(encoded.as_bytes()).unwrap();
                let decoded = integer(&node);
                let error = decoded.as_ref().err().map_or(String::new(), |cause| {
                    format!("fixture schema is malformed: {cause}")
                });
                json!({"error": error, "value": decoded.ok().flatten()})
            })
            .collect();
        std::fs::write(output, serde_json::to_vec(&results).unwrap()).unwrap();
    }
}
