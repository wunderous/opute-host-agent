//! `json.Unmarshal` into the K3s provider's typed Go values: the decoded
//! value, or Go's error text (a syntax error first, else the first
//! `UnmarshalTypeError` in document order, with its struct field path).

use crate::gojson::{Node, Value};
use serde_json::{Map, Value as J};

/// The Go type a document decodes into.
pub(super) enum Shape {
    Str,
    Bool,
    /// `any`.
    Any,
    /// `map[string]string`.
    MapStr,
    /// `map[string]any`.
    MapAny,
    Slice(Box<Shape>),
    /// `name` is the type's `Name()`: empty for an anonymous struct.
    Struct { name: &'static str, fields: Vec<Field> },
}

pub(super) struct Field {
    pub go: &'static str,
    pub json: &'static str,
    pub shape: Shape,
}

pub(super) fn field(go: &'static str, json: &'static str, shape: Shape) -> Field {
    Field { go, json, shape }
}

pub(super) fn slice(shape: Shape) -> Shape {
    Shape::Slice(Box::new(shape))
}

pub(super) fn anon(fields: Vec<Field>) -> Shape {
    Shape::Struct { name: "", fields }
}

impl Shape {
    /// `reflect.Type.String()`.
    fn type_string(&self) -> String {
        match self {
            Shape::Str => "string".into(),
            Shape::Bool => "bool".into(),
            Shape::Any => "interface {}".into(),
            Shape::MapStr => "map[string]string".into(),
            Shape::MapAny => "map[string]interface {}".into(),
            Shape::Slice(inner) => format!("[]{}", inner.type_string()),
            Shape::Struct { name, .. } if !name.is_empty() => format!("main.{name}"),
            Shape::Struct { fields, .. } => {
                let parts: Vec<String> = fields
                    .iter()
                    .map(|f| {
                        let tag = format!("json:\"{}\"", f.json);
                        format!("{} {} {}", f.go, f.shape.type_string(), crate::goerr::quote(&tag))
                    })
                    .collect();
                format!("struct {{ {} }}", parts.join("; "))
            }
        }
    }

    /// The zero value, as the decoded representation holds it.
    fn zero(&self) -> J {
        match self {
            Shape::Str => J::from(""),
            Shape::Bool => J::Bool(false),
            Shape::Any | Shape::MapStr | Shape::MapAny | Shape::Slice(_) => J::Null,
            Shape::Struct { fields, .. } => {
                J::Object(fields.iter().map(|f| (f.json.to_string(), f.shape.zero())).collect())
            }
        }
    }
}

struct Decoder {
    /// The innermost struct's `Name()`, once a struct field was entered.
    in_struct: Option<&'static str>,
    stack: Vec<&'static str>,
    first: Option<String>,
}

impl Decoder {
    fn mismatch(&mut self, node: &Node, shape: &Shape) {
        if self.first.is_some() {
            return;
        }
        let kind = crate::gojson::kind_name(&node.value);
        let ty = shape.type_string();
        self.first = Some(match self.in_struct {
            Some(name) => format!(
                "json: cannot unmarshal {kind} into Go struct field {name}.{} of type {ty}",
                self.stack.join(".")
            ),
            None => format!("json: cannot unmarshal {kind} into Go value of type {ty}"),
        });
    }

    /// Decode `node` into `target`, as Go decodes into an existing value.
    fn decode(&mut self, target: &mut J, node: &Node, shape: &Shape) {
        if matches!(node.value, Value::Null) {
            // null leaves strings, bools and structs as they are and nils
            // maps, slices and interfaces.
            if matches!(shape, Shape::Any | Shape::MapStr | Shape::MapAny | Shape::Slice(_)) {
                *target = J::Null;
            }
            return;
        }
        match (shape, &node.value) {
            (Shape::Any, _) => *target = super::super::go_any(node),
            (Shape::Str, Value::String(s)) => *target = J::from(s.as_str()),
            (Shape::Bool, Value::Bool(b)) => *target = J::Bool(*b),
            (Shape::MapStr | Shape::MapAny, Value::Object(entries)) => {
                if !target.is_object() {
                    *target = J::Object(Map::new());
                }
                for (key, value) in entries {
                    let (mut element, element_shape) = match shape {
                        Shape::MapStr => (J::from(""), &Shape::Str),
                        _ => (J::Null, &Shape::Any),
                    };
                    self.decode(&mut element, value, element_shape);
                    if let J::Object(map) = target {
                        map.insert(key.clone(), element);
                    }
                }
            }
            (Shape::Slice(inner), Value::Array(items)) => {
                let decoded = items
                    .iter()
                    .map(|item| {
                        let mut element = inner.zero();
                        self.decode(&mut element, item, inner);
                        element
                    })
                    .collect();
                *target = J::Array(decoded);
            }
            (Shape::Struct { name, fields }, Value::Object(entries)) => {
                if !target.is_object() {
                    *target = shape.zero();
                }
                for (key, value) in entries {
                    let found = fields
                        .iter()
                        .find(|f| f.json == key)
                        .or_else(|| fields.iter().find(|f| crate::gojson::equal_fold(f.json, key)));
                    let Some(f) = found else {
                        continue;
                    };
                    let (saved_struct, depth) = (self.in_struct, self.stack.len());
                    self.in_struct = Some(name);
                    self.stack.push(f.json);
                    let mut current = target.get(f.json).cloned().unwrap_or_else(|| f.shape.zero());
                    self.decode(&mut current, value, &f.shape);
                    if let J::Object(map) = target {
                        map.insert(f.json.to_string(), current);
                    }
                    self.stack.truncate(depth);
                    self.in_struct = saved_struct;
                }
            }
            _ => self.mismatch(node, shape),
        }
    }
}

/// `json.Unmarshal(raw, &value)` for a value of this shape.
pub(super) fn unmarshal(raw: &[u8], shape: &Shape) -> Result<J, String> {
    if let Some(e) = crate::gojson::syntax_error(raw) {
        return Err(e);
    }
    let node = crate::gojson::parse(raw).map_err(|_| "unexpected end of JSON input".to_string())?;
    let mut decoder = Decoder { in_struct: None, stack: Vec::new(), first: None };
    let mut value = shape.zero();
    decoder.decode(&mut value, &node, shape);
    match decoder.first {
        Some(e) => Err(e),
        None => Ok(value),
    }
}

/// A decoded string field (empty when absent).
pub(super) fn text<'a>(value: &'a J, key: &str) -> &'a str {
    value.get(key).and_then(J::as_str).unwrap_or("")
}

/// A decoded slice field's elements (none when nil).
pub(super) fn items<'a>(value: &'a J, key: &str) -> &'a [J] {
    value.get(key).and_then(J::as_array).map_or(&[], Vec::as_slice)
}
