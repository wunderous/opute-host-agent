//! Port of `internal/plan/assert.go`: JSON Pointer (RFC 6901) assertion
//! evaluation against an arbitrary decoded JSON value.

use super::schema::{Assertion, AssertionFailure};
use serde_json::Value;

pub fn evaluate_assertions(
    value: &Value,
    assertions: &[Assertion],
) -> (Option<AssertionFailure>, bool) {
    for assertion in assertions {
        match resolve_json_pointer(value, &assertion.path) {
            Err(message) => {
                return (
                    Some(AssertionFailure {
                        assertion: assertion.clone(),
                        observed: None,
                        expected: None,
                        message,
                    }),
                    false,
                );
            }
            Ok((observed, exists)) => {
                let (ok, message) = evaluate_assertion(assertion, observed.as_ref(), exists);
                if !ok {
                    return (
                        Some(AssertionFailure {
                            assertion: assertion.clone(),
                            observed,
                            expected: assertion.value.clone(),
                            message,
                        }),
                        false,
                    );
                }
            }
        }
    }
    (None, true)
}

pub(crate) fn resolve_json_pointer(
    value: &Value,
    pointer: &str,
) -> Result<(Option<Value>, bool), String> {
    if pointer.is_empty() || pointer == "/" {
        return Ok((Some(value.clone()), true));
    }
    if !pointer.starts_with('/') {
        return Err(format!("JSON pointer must start with '/': \"{pointer}\""));
    }
    let mut current = value.clone();
    for raw in pointer[1..].split('/') {
        let part = raw.replace("~1", "/").replace("~0", "~");
        match current {
            Value::Object(ref map) => match map.get(&part) {
                Some(next) => current = next.clone(),
                None => return Ok((None, false)),
            },
            Value::Array(ref items) => match part.parse::<usize>() {
                Ok(index) if index < items.len() => current = items[index].clone(),
                _ => return Ok((None, false)),
            },
            _ => return Ok((None, false)),
        }
    }
    Ok((Some(current), true))
}

fn number(value: Option<&Value>) -> Option<f64> {
    value.and_then(Value::as_f64)
}

fn json_equal(left: Option<&Value>, right: Option<&Value>) -> bool {
    left == right
}

fn evaluate_assertion(
    assertion: &Assertion,
    observed: Option<&Value>,
    exists: bool,
) -> (bool, String) {
    match assertion.op.as_str() {
        "exists" => (exists, "expected value to exist".to_string()),
        "notExists" => (!exists, "expected value not to exist".to_string()),
        "empty" => (
            !exists || is_empty(observed),
            "expected value to be empty".to_string(),
        ),
        "notEmpty" => (
            exists && !is_empty(observed),
            "expected value to be non-empty".to_string(),
        ),
        "eq" => (
            exists && json_equal(observed, assertion.value.as_ref()),
            "values are not equal".to_string(),
        ),
        "ne" => (
            !exists || !json_equal(observed, assertion.value.as_ref()),
            "values are equal".to_string(),
        ),
        "gt" | "gte" | "lt" | "lte" => {
            let left = number(observed);
            let right = number(assertion.value.as_ref());
            match (left, right) {
                (Some(left), Some(right)) => match assertion.op.as_str() {
                    "gt" => (
                        left > right,
                        "value is not greater than expected".to_string(),
                    ),
                    "gte" => (left >= right, "value is less than expected".to_string()),
                    "lt" => (left < right, "value is not less than expected".to_string()),
                    _ => (left <= right, "value is greater than expected".to_string()),
                },
                _ => (false, "comparison requires numeric values".to_string()),
            }
        }
        "contains" => {
            if let Some(Value::String(text)) = observed {
                let needle = assertion
                    .value
                    .as_ref()
                    .and_then(Value::as_str)
                    .unwrap_or("");
                (
                    text.contains(needle),
                    "string does not contain expected value".to_string(),
                )
            } else if let Some(Value::Array(items)) = observed {
                let found = items
                    .iter()
                    .any(|item| json_equal(Some(item), assertion.value.as_ref()));
                (
                    found,
                    "collection does not contain expected value".to_string(),
                )
            } else {
                (
                    false,
                    "collection does not contain expected value".to_string(),
                )
            }
        }
        "matches" => {
            let text = observed.and_then(Value::as_str);
            let pattern = assertion.value.as_ref().and_then(Value::as_str);
            match (text, pattern) {
                (Some(text), Some(pattern)) => match regex::Regex::new(pattern) {
                    Ok(re) => (
                        re.is_match(text),
                        "value does not match expected pattern".to_string(),
                    ),
                    Err(_) => (false, "value does not match expected pattern".to_string()),
                },
                _ => (false, "matches requires string values".to_string()),
            }
        }
        "all" | "any" => {
            let Some(Value::Array(items)) = observed else {
                return (false, format!("{} requires an array", assertion.op));
            };
            let matched = items
                .iter()
                .filter(|item| evaluate_assertions(item, &assertion.assertions).1)
                .count();
            if assertion.op == "all" {
                (
                    matched == items.len(),
                    "not every array element matched".to_string(),
                )
            } else {
                (matched > 0, "no array element matched".to_string())
            }
        }
        other => (false, format!("unsupported assertion operator {other}")),
    }
}

fn is_empty(value: Option<&Value>) -> bool {
    match value {
        None => true,
        Some(Value::Null) => true,
        Some(Value::String(s)) => s.trim().is_empty(),
        Some(Value::Array(items)) => items.is_empty(),
        Some(Value::Object(map)) => map.is_empty(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::schema::Assertion;

    fn assertion(path: &str, op: &str, value: Option<Value>) -> Assertion {
        Assertion {
            path: path.to_string(),
            op: op.to_string(),
            value,
            assertions: vec![],
        }
    }

    #[test]
    fn eq_passes_when_values_match() {
        let value = serde_json::json!({"status": "ready"});
        let (_, ok) = evaluate_assertions(
            &value,
            &[assertion(
                "/status",
                "eq",
                Some(Value::String("ready".to_string())),
            )],
        );
        assert!(ok);
    }

    #[test]
    fn exists_fails_on_missing_pointer() {
        let value = serde_json::json!({});
        let (failure, ok) = evaluate_assertions(&value, &[assertion("/missing", "exists", None)]);
        assert!(!ok);
        assert!(failure.is_some());
    }

    #[test]
    fn gt_requires_numeric_values() {
        let value = serde_json::json!({"n": "not-a-number"});
        let (_, ok) = evaluate_assertions(
            &value,
            &[assertion("/n", "gt", Some(Value::Number(1.into())))],
        );
        assert!(!ok);
    }

    #[test]
    fn all_requires_every_element_to_match() {
        let value = serde_json::json!([{"ok": true}, {"ok": false}]);
        let nested = assertion("/ok", "eq", Some(Value::Bool(true)));
        let all = Assertion {
            path: "/".to_string(),
            op: "all".to_string(),
            value: None,
            assertions: vec![nested],
        };
        let (_, ok) = evaluate_assertions(&value, &[all]);
        assert!(!ok);
    }

    #[test]
    fn json_pointer_escapes_tilde_and_slash() {
        let value = serde_json::json!({"a/b": {"c~d": 1}});
        let (_, exists) = resolve_json_pointer(&value, "/a~1b/c~0d").unwrap();
        assert!(exists);
    }
}
