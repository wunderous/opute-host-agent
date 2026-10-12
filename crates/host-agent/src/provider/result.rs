//! Go SDK CallToolResult projection. Unknown wire fields are not promoted into
//! observations. Content and SEP-2322 input requests retain their distinct Go
//! codecs. Public activation and complete transport parity remain separate gates.
use crate::goerr::quote;
use base64::Engine;
use serde_json::{Map, Value};

use crate::mcpsdk::{StructDef, Ty};
mod requests;
static ANNOTATIONS: StructDef = StructDef {
    name: "mcp.Annotations",
    fields: &[
        ("audience", Ty::SliceNamedStr("mcp.Role")),
        ("lastModified", Ty::Str("string")),
        ("priority", Ty::Float),
    ],
};
static ICON: StructDef = StructDef {
    name: "mcp.Icon",
    fields: &[
        ("src", Ty::Str("string")),
        ("mimeType", Ty::Str("string")),
        ("sizes", Ty::SliceStr),
        ("theme", Ty::Str("string")),
    ],
};
static RESOURCE: StructDef = StructDef {
    name: "mcp.ResourceContents",
    fields: &[
        ("uri", Ty::Str("string")),
        ("mimeType", Ty::Str("string")),
        ("text", Ty::Str("string")),
        ("blob", Ty::Bytes),
        ("_meta", Ty::MapAny("map[string]interface {}")),
    ],
};
static CONTENT: StructDef = StructDef {
    name: "mcp.wireContent",
    fields: &[
        ("type", Ty::Str("string")),
        ("text", Ty::Str("string")),
        ("mimeType", Ty::Str("string")),
        ("data", Ty::Bytes),
        ("resource", Ty::Struct(&RESOURCE)),
        ("uri", Ty::Str("string")),
        ("name", Ty::Str("string")),
        ("title", Ty::Str("string")),
        ("description", Ty::Str("string")),
        ("size", Ty::Int64),
        ("_meta", Ty::MapAny("map[string]interface {}")),
        ("annotations", Ty::Struct(&ANNOTATIONS)),
        ("icons", Ty::SliceOf(&ICON)),
        ("id", Ty::Str("string")),
        ("input", Ty::MapAny("map[string]interface {}")),
        ("toolUseId", Ty::Str("string")),
        ("content", Ty::PointerSlice(&CONTENT)),
        ("structuredContent", Ty::Raw),
        ("isError", Ty::Bool),
    ],
};
static RESULT: StructDef = StructDef {
    name: r#"struct { mcp.res; Content []*mcp.wireContent "json:\"content\""; ResultType mcp.resultType "json:\"resultType\"" }"#,
    fields: &[
        ("_meta", Ty::MapAny("map[string]interface {}")),
        ("content", Ty::PointerSlice(&CONTENT)),
        ("structuredContent", Ty::Raw),
        ("isError", Ty::Bool),
        ("requestState", Ty::Str("string")),
        ("resultType", Ty::Str("string")),
        ("inputRequests", Ty::Custom(requests::validate)),
    ],
};

fn generic(value: &Value) -> Value {
    match value {
        Value::Number(number) => {
            crate::gojson::float_value(number.as_f64().expect("finite JSON number"))
        }
        Value::Array(items) => Value::Array(items.iter().map(generic).collect()),
        Value::Object(map) => Value::Object(
            map.iter()
                .map(|(key, value)| (key.clone(), generic(value)))
                .collect(),
        ),
        _ => value.clone(),
    }
}

fn put(out: &mut Map<String, Value>, source: &Map<String, Value>, key: &str) {
    if let Some(value) = source.get(key).filter(|v| !v.is_null()) {
        out.insert(
            key.into(),
            if matches!(key, "_meta" | "structuredContent" | "priority") {
                generic(value)
            } else {
                value.clone()
            },
        );
    }
}
fn put_nonempty(out: &mut Map<String, Value>, source: &Map<String, Value>, key: &str) {
    if let Some(value) = source.get(key).filter(|v| match v {
        Value::Null => false,
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(m) => !m.is_empty(),
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.0),
    }) {
        out.insert(
            key.into(),
            if matches!(key, "_meta" | "structuredContent" | "priority") {
                generic(value)
            } else {
                value.clone()
            },
        );
    }
}
fn required_string(out: &mut Map<String, Value>, source: &Map<String, Value>, key: &str) {
    out.insert(
        key.into(),
        Value::String(
            source
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .into(),
        ),
    );
}
fn binary(value: Option<&Value>) -> Result<String, String> {
    let Some(value) = value.filter(|v| !v.is_null()) else {
        return Ok(String::new());
    };
    if let Some(items) = value.as_array() {
        let bytes: Vec<u8> = items
            .iter()
            .map(|v| v.as_u64().unwrap_or_default() as u8)
            .collect();
        return Ok(base64::engine::general_purpose::STANDARD.encode(bytes));
    }
    let raw = value.as_str().unwrap_or_default();
    let cleaned: Vec<u8> = raw
        .bytes()
        .filter(|b| !matches!(b, b'\r' | b'\n'))
        .collect();
    let engine = base64::engine::general_purpose::GeneralPurpose::new(
        &base64::alphabet::STANDARD,
        base64::engine::general_purpose::GeneralPurposeConfig::new()
            .with_decode_allow_trailing_bits(true),
    );
    crate::gojson::validate_base64(raw.as_bytes())?;
    let bytes = engine.decode(&cleaned).expect("Go-validated base64");
    Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
}

fn annotations(value: &Value) -> Value {
    let mut out = Map::new();
    if let Some(source) = value.as_object() {
        for key in ["audience", "lastModified", "priority"] {
            put_nonempty(&mut out, source, key);
        }
        normalize_strings(&mut out, "audience");
    }
    Value::Object(out)
}
fn normalize_strings(out: &mut Map<String, Value>, key: &str) {
    if let Some(Value::Array(items)) = out.get_mut(key) {
        for item in items {
            if item.is_null() {
                *item = Value::String(String::new());
            }
        }
    }
}
fn content(value: &Value, nested: bool) -> Result<Value, String> {
    if value.is_null() {
        return Err("nil content".into());
    }
    let source = value.as_object().cloned().unwrap_or_default();
    let kind = source
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if nested
        && !matches!(
            kind,
            "text" | "image" | "audio" | "resource_link" | "resource"
        )
    {
        return Err(format!("invalid content type {}", quote(kind)));
    }
    let mut out = Map::new();
    out.insert("type".into(), Value::String(kind.into()));
    match kind {
        "text" => required_string(&mut out, &source, "text"),
        "image" | "audio" => {
            required_string(&mut out, &source, "mimeType");
            out.insert("data".into(), Value::String(binary(source.get("data"))?));
        }
        "resource_link" => {
            for key in ["uri", "name", "title", "description", "mimeType"] {
                put_nonempty(&mut out, &source, key);
            }
            put(&mut out, &source, "size");
            if let Some(icons) = source
                .get("icons")
                .and_then(Value::as_array)
                .filter(|a| !a.is_empty())
            {
                out.insert(
                    "icons".into(),
                    Value::Array(
                        icons
                            .iter()
                            .map(|icon| {
                                let source = icon.as_object().cloned().unwrap_or_default();
                                let mut icon = Map::new();
                                required_string(&mut icon, &source, "src");
                                for key in ["mimeType", "sizes", "theme"] {
                                    put_nonempty(&mut icon, &source, key);
                                }
                                normalize_strings(&mut icon, "sizes");
                                Value::Object(icon)
                            })
                            .collect(),
                    ),
                );
            }
        }
        "resource" => {
            if let Some(resource) = source.get("resource").filter(|v| !v.is_null()) {
                let source = resource.as_object().cloned().unwrap_or_default();
                let mut resource = Map::new();
                required_string(&mut resource, &source, "uri");
                for key in ["mimeType", "text", "_meta"] {
                    put_nonempty(&mut resource, &source, key);
                }
                if source.get("blob").is_some_and(|v| !v.is_null()) {
                    resource.insert("blob".into(), Value::String(binary(source.get("blob"))?));
                }
                out.insert("resource".into(), Value::Object(resource));
            }
        }
        "tool_use" => {
            required_string(&mut out, &source, "id");
            required_string(&mut out, &source, "name");
            out.insert(
                "input".into(),
                source
                    .get("input")
                    .filter(|v| !v.is_null())
                    .cloned()
                    .unwrap_or(Value::Object(Map::new())),
            );
        }
        "tool_result" => {
            required_string(&mut out, &source, "toolUseId");
            let items = source
                .get("content")
                .and_then(Value::as_array)
                .map_or(&[][..], Vec::as_slice);
            // Nested content is re-encoded through wireContent, where empty
            // text/data are omitted even though top-level content requires them.
            let mut items = items
                .iter()
                .map(|v| content(v, true).map_err(|e| format!("tool_result nested content: {e}")))
                .collect::<Result<Vec<_>, _>>()?;
            for item in &mut items {
                if let Some(map) = item.as_object_mut() {
                    for key in ["text", "data", "mimeType"] {
                        if map.get(key) == Some(&Value::String(String::new())) {
                            map.remove(key);
                        }
                    }
                }
            }
            out.insert("content".into(), Value::Array(items));
            put(&mut out, &source, "structuredContent");
            put_nonempty(&mut out, &source, "isError");
        }
        _ => return Err(format!("unrecognized content type {}", quote(kind))),
    }
    if kind == "tool_use" {
        if let Some(input) = out.get_mut("input") {
            *input = generic(input);
        }
    }
    put_nonempty(&mut out, &source, "_meta");
    if !matches!(kind, "tool_use" | "tool_result") {
        if let Some(value) = source.get("annotations").filter(|v| !v.is_null()) {
            out.insert("annotations".into(), annotations(value));
        }
    }
    Ok(Value::Object(out))
}

pub fn project(source: &Map<String, Value>) -> Result<Map<String, Value>, String> {
    // The pinned client first decodes the RPC result into map[string]any,
    // then marshals that map for the typed SDK decoder. All numbers therefore
    // cross the float64 boundary before content fields receive their types.
    let generic_source = generic(&Value::Object(source.clone()));
    let source = generic_source.as_object().expect("object projection");
    let mut encoded = String::new();
    crate::gojson::encode(&Value::Object(source.clone()), &mut encoded);
    let node = crate::gojson::parse(encoded.as_bytes()).expect("encoded provider result");
    crate::mcpsdk::validate_exact(&node, &RESULT, encoded.as_bytes())?;

    let mut out = Map::new();
    put_nonempty(&mut out, source, "_meta");
    let items = source
        .get("content")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    out.insert(
        "content".into(),
        Value::Array(
            items
                .iter()
                .map(|v| content(v, false))
                .collect::<Result<_, _>>()?,
        ),
    );
    put(&mut out, source, "structuredContent");
    for key in ["isError", "requestState", "resultType"] {
        put_nonempty(&mut out, source, key);
    }
    if let Some(input) = node.key("inputRequests").filter(|n| !n.is_null()) {
        out.insert("inputRequests".into(), requests::project(input)?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn proposed_null_input_request_refusal_preserves_decoder() {
        let input = serde_json::json!({"inputRequests":{"fixture":null}});
        assert_eq!(
            project(input.as_object().unwrap()).unwrap_err(),
            "nil InputRequest entry"
        );
        let valid =
            serde_json::json!({"inputRequests":{"fixture":{"method":"roots/list","params":{}}}});
        let result = project(valid.as_object().unwrap()).unwrap();
        assert_eq!(result.get("inputRequests"), valid.get("inputRequests"));
    }

    #[test]
    #[ignore = "requires the pinned Go SDK result differential driver"]
    fn result_fixture() {
        let rows: Vec<Value> = serde_json::from_slice(
            &std::fs::read(std::env::var("PARITY_PROVIDER_RESULT_INPUT").unwrap()).unwrap(),
        )
        .unwrap();
        let results: Vec<Value> = rows
            .iter()
            .map(|row| match project(row.as_object().unwrap()) {
                Ok(v) => serde_json::json!({"result":v}),
                Err(e) => serde_json::json!({"error":e}),
            })
            .collect();
        let mut encoded = String::new();
        crate::gojson::encode(&Value::Array(results), &mut encoded);
        std::fs::write(
            std::env::var("PARITY_PROVIDER_RESULT_OUTPUT").unwrap(),
            encoded,
        )
        .unwrap();
    }
}
