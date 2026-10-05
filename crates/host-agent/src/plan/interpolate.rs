//! Port of `internal/plan/interpolate.go`: `${...}` reference resolution
//! inside node arguments.

use serde_json::{Map, Value};
use std::sync::OnceLock;

static REFERENCE_PATTERN: OnceLock<regex::Regex> = OnceLock::new();
fn reference_pattern() -> &'static regex::Regex {
    REFERENCE_PATTERN.get_or_init(|| regex::Regex::new(r"\$\{([^}]+)\}").unwrap())
}

#[derive(Debug, Clone, Default)]
pub struct EvalContext {
    pub variables: Map<String, Value>,
    pub node_output: Map<String, Value>,
    pub item: Map<String, Value>,
    pub input: Map<String, Value>,
    pub context: Map<String, Value>,
}

pub fn interpolate_args(
    args: &Map<String, Value>,
    context: &EvalContext,
) -> Result<Map<String, Value>, String> {
    let value = interpolate_value(&Value::Object(args.clone()), context)?;
    match value {
        Value::Object(map) => Ok(map),
        _ => Err("interpolated arguments are not an object".to_string()),
    }
}

pub fn interpolate_value(value: &Value, context: &EvalContext) -> Result<Value, String> {
    match value {
        Value::Object(map) => {
            let mut result = Map::with_capacity(map.len());
            for (key, child) in map {
                result.insert(key.clone(), interpolate_value(child, context)?);
            }
            Ok(Value::Object(result))
        }
        Value::Array(items) => {
            let mut result = Vec::with_capacity(items.len());
            for child in items {
                result.push(interpolate_value(child, context)?);
            }
            Ok(Value::Array(result))
        }
        Value::String(text) => {
            let matches: Vec<regex::Captures> = reference_pattern().captures_iter(text).collect();
            if matches.is_empty() {
                return Ok(Value::String(text.clone()));
            }
            if matches.len() == 1 {
                let m = matches[0].get(0).unwrap();
                if m.start() == 0 && m.end() == text.len() {
                    let reference = matches[0].get(1).unwrap().as_str();
                    return resolve_reference(reference, context);
                }
            }
            let mut builder = String::new();
            let mut last = 0usize;
            for m in &matches {
                let whole = m.get(0).unwrap();
                builder.push_str(&text[last..whole.start()]);
                let reference = m.get(1).unwrap().as_str();
                let resolved = resolve_reference(reference, context)?;
                builder.push_str(&go_sprint(&resolved));
                last = whole.end();
            }
            builder.push_str(&text[last..]);
            Ok(Value::String(builder))
        }
        other => Ok(other.clone()),
    }
}

pub(crate) fn resolve_reference(reference: &str, context: &EvalContext) -> Result<Value, String> {
    let parts: Vec<&str> = split_n(reference, '.', 3);
    if parts.len() < 2 {
        return Err(format!("invalid interpolation reference \"{reference}\""));
    }
    let root: &Map<String, Value> = match parts[0] {
        "vars" => &context.variables,
        "item" => &context.item,
        "input" => &context.input,
        "context" => &context.context,
        "nodes" => {
            if parts.len() < 3 || parts[2].is_empty() {
                return Err(format!(
                    "node interpolation must include output: \"{reference}\""
                ));
            }
            let node = context
                .node_output
                .get(parts[1])
                .ok_or_else(|| format!("unresolved node interpolation \"{reference}\""))?;
            let mut path = parts[2];
            if let Some(stripped) = path.strip_prefix("output") {
                path = stripped.strip_prefix('.').unwrap_or(stripped);
                if path.is_empty() {
                    return Ok(node.clone());
                }
            }
            return resolve_dotted_path(node, path, reference);
        }
        other => return Err(format!("unknown interpolation root \"{other}\"")),
    };
    let path = parts[1..].join(".");
    resolve_dotted_path(&Value::Object(root.clone()), &path, reference)
}

fn split_n(s: &str, sep: char, n: usize) -> Vec<&str> {
    if n == 0 {
        return vec![];
    }
    let mut parts = Vec::with_capacity(n);
    let mut rest = s;
    for _ in 1..n {
        match rest.find(sep) {
            Some(idx) => {
                parts.push(&rest[..idx]);
                rest = &rest[idx + 1..];
            }
            None => break,
        }
    }
    parts.push(rest);
    parts
}

fn resolve_dotted_path(root: &Value, path: &str, reference: &str) -> Result<Value, String> {
    if path.is_empty() {
        return Ok(root.clone());
    }
    let path = path.strip_prefix('/').unwrap_or(path);
    let mut current = root.clone();
    for part in path.split(['.', '/']).filter(|p| !p.is_empty()) {
        current = match current {
            Value::Object(map) => map
                .get(part)
                .cloned()
                .ok_or_else(|| format!("unresolved interpolation reference \"{reference}\""))?,
            Value::Array(items) => {
                let index: usize = part
                    .parse()
                    .map_err(|_| format!("unresolved interpolation reference \"{reference}\""))?;
                items
                    .get(index)
                    .cloned()
                    .ok_or_else(|| format!("unresolved interpolation reference \"{reference}\""))?
            }
            _ => {
                return Err(format!(
                    "unresolved interpolation reference \"{reference}\""
                ))
            }
        };
    }
    Ok(current)
}

/// Mirrors Go's `fmt.Sprint` for the JSON-decoded values interpolation can
/// produce: nil, bool, string, and float64 (JSON numbers), formatted with
/// the shortest round-tripping representation so e.g. `5.0` prints `5`.
fn go_sprint(value: &Value) -> String {
    match value {
        Value::Null => "<nil>".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::String(s) => s.clone(),
        Value::Number(n) => {
            if let Some(f) = n.as_f64() {
                if f == f.trunc() && f.abs() < 1e15 {
                    format!("{}", f as i64)
                } else {
                    let mut s = format!("{f}");
                    if !s.contains('.') && !s.contains('e') {
                        s.push_str(".0");
                    }
                    s
                }
            } else {
                n.to_string()
            }
        }
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_match_returns_raw_typed_value() {
        let mut vars = Map::new();
        vars.insert("count".to_string(), Value::Number(5.into()));
        let context = EvalContext {
            variables: vars,
            ..Default::default()
        };
        let result =
            interpolate_value(&Value::String("${vars.count}".to_string()), &context).unwrap();
        assert_eq!(result, Value::Number(5.into()));
    }

    #[test]
    fn partial_match_does_string_concatenation() {
        let mut vars = Map::new();
        vars.insert("name".to_string(), Value::String("world".to_string()));
        let context = EvalContext {
            variables: vars,
            ..Default::default()
        };
        let result =
            interpolate_value(&Value::String("hello ${vars.name}!".to_string()), &context).unwrap();
        assert_eq!(result, Value::String("hello world!".to_string()));
    }

    #[test]
    fn node_output_reference_strips_output_prefix() {
        let mut node_output = Map::new();
        node_output.insert("n1".to_string(), serde_json::json!({"id": "abc"}));
        let context = EvalContext {
            node_output,
            ..Default::default()
        };
        let result = interpolate_value(
            &Value::String("${nodes.n1.output.id}".to_string()),
            &context,
        )
        .unwrap();
        assert_eq!(result, Value::String("abc".to_string()));
    }

    #[test]
    fn unresolved_reference_is_an_error() {
        let context = EvalContext::default();
        assert!(
            interpolate_value(&Value::String("${vars.missing}".to_string()), &context).is_err()
        );
    }
}
