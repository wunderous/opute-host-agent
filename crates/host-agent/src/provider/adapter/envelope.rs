//! Typed response envelope used by the pinned internal/mcphttp client.
use crate::gojson::{self, Node, Value as G};
use serde_json::{Map, Value};
const ROOT: &str = r#"struct { Result map[string]interface {} "json:\"result\""; Error *struct { Code int "json:\"code\""; Message string "json:\"message\"" } "json:\"error\"" }"#;
const ERROR: &str = r#"struct { Code int "json:\"code\""; Message string "json:\"message\"" }"#;
fn wrong(node: &Node, ty: &str, path: &str) -> String {
    let kind = match &node.value {
        G::Null => "null".into(),
        G::Bool(_) => "bool".into(),
        G::String(_) => "string".into(),
        G::Number(n) if matches!(ty, "int" | "float64") => format!("number {n}"),
        G::Number(_) => "number".into(),
        G::Object(_) => "object".into(),
        G::Array(_) => "array".into(),
    };
    if path.is_empty() {
        format!("json: cannot unmarshal {kind} into Go value of type {ty}")
    } else {
        format!("json: cannot unmarshal {kind} into Go struct field .{path} of type {ty}")
    }
}
fn dynamic(node: &Node) -> Result<Value, String> {
    Ok(match &node.value {
        G::Null => Value::Null,
        G::Bool(b) => Value::Bool(*b),
        G::String(s) => Value::String(s.clone()),
        G::Number(n) => {
            let f = n
                .parse::<f64>()
                .ok()
                .filter(|f| f.is_finite())
                .ok_or_else(|| wrong(node, "float64", "result"))?;
            gojson::float_value(f)
        }
        G::Array(a) => Value::Array(a.iter().map(dynamic).collect::<Result<_, _>>()?),
        G::Object(m) => {
            let mut result = Map::new();
            for (k, v) in m {
                result.insert(k.clone(), dynamic(v)?);
            }
            Value::Object(result)
        }
    })
}
pub(super) fn decode(bytes: &[u8]) -> Result<Map<String, Value>, String> {
    gojson::validate_syntax(bytes).map_err(|e| format!("decode MCP response: {e}"))?;
    let node =
        gojson::parse(bytes).map_err(|_| "decode MCP response: invalid JSON value".to_string())?;
    let (result, error) = decode_node(&node).map_err(|e| format!("decode MCP response: {e}"))?;
    if let Some((code, message)) = error {
        return Err(format!("MCP error {code}: {message}"));
    }
    Ok(result)
}
type Decoded = (Map<String, Value>, Option<(i64, String)>);
fn decode_node(node: &Node) -> Result<Decoded, String> {
    if node.is_null() {
        return Ok((Map::new(), None));
    }
    let fields = node.as_object().ok_or_else(|| wrong(node, ROOT, ""))?;
    let mut result = Map::new();
    let mut error: Option<(i64, String)> = None;
    let mut first = None;
    for (key, node) in fields {
        let decoded = if gojson::equal_fold(key, "result") {
            if node.is_null() {
                result.clear();
                Ok(())
            } else if let Some(members) = node.as_object() {
                let mut local = None;
                for (k, n) in members {
                    match dynamic(n) {
                        Ok(v) => {
                            result.insert(k.clone(), v);
                        }
                        Err(e) => {
                            if local.is_none() {
                                local = Some(e);
                            }
                        }
                    }
                }
                if let Some(e) = local {
                    Err(e)
                } else {
                    Ok(())
                }
            } else {
                Err(wrong(node, "map[string]interface {}", "result"))
            }
        } else if gojson::equal_fold(key, "error") {
            if node.is_null() {
                error = None;
                Ok(())
            } else if let Some(members) = node.as_object() {
                let (code, message) = error.get_or_insert((0, String::new()));
                let mut local = None;
                for (k, n) in members {
                    let decoded = if gojson::equal_fold(k, "code") {
                        if n.is_null() {
                            Ok(())
                        } else if let G::Number(v) = &n.value {
                            match v.parse::<i64>() {
                                Ok(v) => {
                                    *code = v;
                                    Ok(())
                                }
                                Err(_) => Err(wrong(n, "int", "error.code")),
                            }
                        } else {
                            Err(wrong(n, "int", "error.code"))
                        }
                    } else if gojson::equal_fold(k, "message") {
                        if n.is_null() {
                            Ok(())
                        } else if let Some(v) = n.as_str() {
                            *message = v.into();
                            Ok(())
                        } else {
                            Err(wrong(n, "string", "error.message"))
                        }
                    } else {
                        Ok(())
                    };
                    if let Err(e) = decoded {
                        if local.is_none() {
                            local = Some(e);
                        }
                    }
                }
                if let Some(e) = local {
                    Err(e)
                } else {
                    Ok(())
                }
            } else {
                Err(wrong(node, ERROR, "error"))
            }
        } else {
            Ok(())
        };
        if let Err(e) = decoded {
            if first.is_none() {
                first = Some(e);
            }
        }
    }
    if let Some(e) = first {
        return Err(e);
    }
    Ok((result, error))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires the pinned typed response-envelope driver"]
    fn envelope_fixture() {
        let rows: Vec<Vec<u8>> = serde_json::from_slice(
            &std::fs::read(std::env::var("PARITY_PROVIDER_ENVELOPE_INPUT").unwrap()).unwrap(),
        )
        .unwrap();
        let results: Vec<Value> = rows
            .iter()
            .map(|row| match decode(row) {
                Ok(result) => serde_json::json!({"result":result}),
                Err(error) => serde_json::json!({"error":error}),
            })
            .collect();
        let mut encoded = String::new();
        gojson::encode(&Value::Array(results), &mut encoded);
        std::fs::write(
            std::env::var("PARITY_PROVIDER_ENVELOPE_OUTPUT").unwrap(),
            encoded,
        )
        .unwrap();
    }
}
