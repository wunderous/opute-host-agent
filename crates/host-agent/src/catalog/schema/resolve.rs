//! Typed tree indexing and JSON Pointers for the owning schema compiler.
use super::*;
use std::collections::HashMap;

fn escape(segment: &str) -> String {
    segment.replace('~', "~0").replace('/', "~1")
}
fn unescape(segment: &str) -> String {
    let mut chars = segment.chars();
    let mut out = String::new();
    while let Some(ch) = chars.next() {
        if ch == '~' {
            match chars.next() {
                Some('0') => out.push('~'),
                Some('1') => out.push('/'),
                Some(c) => {
                    out.push('~');
                    out.push(c);
                }
                None => out.push('~'),
            }
        } else {
            out.push(ch);
        }
    }
    out
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
        let mut fields: Vec<_> = FIELDS
            .iter()
            .map(|&(json, name, _)| (json, &s.fields[name]))
            .collect();
        fields.push(("items", &s.fields["ItemsArray"]));
        fields.sort_by_key(|&(key, _)| key);
        for (key, slot) in fields {
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
fn array_index(segment: &str, len: usize) -> Result<usize, String> {
    if segment == "-" {
        return Err("the JSON Pointer array segment '-' is not supported".into());
    }
    if segment.len() > 1 && segment.starts_with('0') {
        return Err(format!("segment {} has leading zeroes", quote(segment)));
    }
    let n = segment
        .parse::<i64>()
        .map_err(|_| format!("invalid int: {}", quote(segment)))?;
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
    let traverse = || -> Result<Option<&'a Schema>, String> {
        if !pointer.is_empty() && !pointer.starts_with('/') {
            return Err(format!(
                "JSON Pointer {} does not begin with '/'",
                quote(pointer)
            ));
        }
        let mut cursor = Cursor::Schema(Some(root));
        if !pointer.is_empty() {
            for raw in pointer[1..].split('/') {
                let segment = unescape(raw);
                cursor = match cursor {
                    Cursor::Schema(Some(s)) => field_cursor(s, &segment)?,
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
                        map.and_then(|m| m.get(&segment))
                            .ok_or_else(|| format!("no key {} in map", quote(&segment)))?
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
                        value
                            .get(&segment)
                            .ok_or_else(|| format!("no key {} in map", quote(&segment)))?,
                    ),
                    Cursor::BoolMap(value) => Cursor::Bool(
                        value
                            .get(&segment)
                            .ok_or_else(|| format!("no key {} in map", quote(&segment)))?
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
    traverse().map_err(|cause| format!("JSON Pointer {}: {cause}", quote(pointer)))
}
