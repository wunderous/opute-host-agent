//! `plan.ValidateJSON`: the declarative schema subset every capability's
//! input and output is checked against, with Go's error texts.
//!
//! Values are JSON decoded into `any`, so numbers are float64 and types print
//! as Go names (`%T`). Go ranges over an object's properties in random order
//! and reports the first failure it meets; this port checks them in sorted
//! order, which is one of Go's possible orders.

use serde_json::{Map, Value as J};

/// `fmt`'s `%T` for a value decoded into `any`.
fn go_type(v: &J) -> &'static str {
    match v {
        J::Null => "<nil>",
        J::Bool(_) => "bool",
        J::Number(_) => "float64",
        J::String(_) => "string",
        J::Array(_) => "[]interface {}",
        J::Object(_) => "map[string]interface {}",
    }
}

/// `strconv.FormatFloat(f, 'g', -1, 64)`, which `%v` and `%g` print.
pub fn go_g(f: f64) -> String {
    if f == 0.0 {
        return if f.is_sign_negative() {
            "-0".into()
        } else {
            "0".into()
        };
    }
    if !f.is_finite() {
        return if f.is_nan() {
            "NaN".into()
        } else if f > 0.0 {
            "+Inf".into()
        } else {
            "-Inf".into()
        };
    }
    // Shortest round-trip digits and the decimal exponent.
    let sci = format!("{f:e}");
    let (mantissa, exp) = sci.split_once('e').expect("exponent form");
    let exp: i32 = exp.parse().expect("integer exponent");
    if !(-4..6).contains(&exp) {
        let sign = if exp < 0 { '-' } else { '+' };
        return format!("{mantissa}e{sign}{:02}", exp.abs());
    }
    format!("{f}")
}

/// `fmt`'s `%v` for a value decoded into `any`.
pub fn go_v(v: &J) -> String {
    match v {
        J::Null => "<nil>".into(),
        J::Bool(b) => b.to_string(),
        J::Number(n) => go_g(n.as_f64().unwrap_or(0.0)),
        J::String(s) => s.clone(),
        J::Array(items) => format!("[{}]", items.iter().map(go_v).collect::<Vec<_>>().join(" ")),
        J::Object(map) => format!(
            "map[{}]",
            map.iter()
                .map(|(k, v)| format!("{k}:{}", go_v(v)))
                .collect::<Vec<_>>()
                .join(" ")
        ),
    }
}

fn json_equal(a: &J, b: &J) -> bool {
    let (mut x, mut y) = (String::new(), String::new());
    crate::gojson::encode(a, &mut x);
    crate::gojson::encode(b, &mut y);
    x == y
}

/// `stringOrAnySlice`: a list whose items are all strings.
fn string_list(v: Option<&J>) -> Option<Vec<String>> {
    v?.as_array()?
        .iter()
        .map(|i| i.as_str().map(str::to_string))
        .collect()
}

/// `schemaTypes`.
fn types(v: Option<&J>) -> Vec<String> {
    match v {
        Some(J::String(s)) if !s.is_empty() => vec![s.clone()],
        Some(J::Array(items)) => items
            .iter()
            .filter_map(J::as_str)
            .map(str::to_string)
            .collect(),
        _ => Vec::new(),
    }
}

/// `isWholeReference`: the whole string is one `${...}` reference.
fn whole_reference(v: &J) -> bool {
    let Some(s) = v.as_str() else { return false };
    let Some(inner) = s.strip_prefix("${").and_then(|r| r.strip_suffix('}')) else {
        return false;
    };
    !inner.is_empty() && !inner.contains('}')
}

fn number(v: Option<&J>) -> Option<f64> {
    v?.as_f64()
}

pub fn validate(schema: &Map<String, J>, value: &J) -> Result<(), String> {
    if schema.is_empty() || whole_reference(value) {
        return Ok(());
    }
    if let Some(choices) = string_list(schema.get("enum")) {
        if !choices
            .iter()
            .any(|c| json_equal(&J::from(c.clone()), value))
        {
            return Err(format!("value {} is not in enum", go_v(value)));
        }
    }
    if let Some(constant) = schema.get("const") {
        if !json_equal(constant, value) {
            return Err(format!(
                "value {} does not equal const {}",
                go_v(value),
                go_v(constant)
            ));
        }
    }
    let kinds = types(schema.get("type"));
    if kinds.is_empty() {
        return Ok(());
    }
    if kinds.len() > 1 {
        let mut last = Ok(());
        for kind in &kinds {
            let mut candidate = schema.clone();
            candidate.insert("type".into(), J::from(kind.clone()));
            match validate(&candidate, value) {
                Ok(()) => return Ok(()),
                Err(e) => last = Err(e),
            }
        }
        return last;
    }
    match kinds[0].as_str() {
        "object" => {
            let J::Object(object) = value else {
                return Err(format!("expected object, got {}", go_type(value)));
            };
            if let Some(required) = string_list(schema.get("required")) {
                for name in required {
                    if !object.contains_key(&name) {
                        return Err(format!(
                            "missing required property {}",
                            crate::goerr::quote(&name)
                        ));
                    }
                }
            }
            let properties = schema.get("properties").and_then(J::as_object);
            for (key, child) in object {
                match properties.and_then(|p| p.get(key)).and_then(J::as_object) {
                    Some(property) => validate(property, child)
                        .map_err(|e| format!("property {}: {e}", crate::goerr::quote(key)))?,
                    None => {
                        if schema.get("additionalProperties") == Some(&J::Bool(false)) {
                            return Err(format!("unknown property {}", crate::goerr::quote(key)));
                        }
                    }
                }
            }
        }
        "array" => {
            let J::Array(array) = value else {
                return Err(format!("expected array, got {}", go_type(value)));
            };
            if let Some(minimum) = number(schema.get("minItems")) {
                if (array.len() as f64) < minimum {
                    return Err(format!(
                        "array has {} items, minimum is {}",
                        array.len(),
                        go_g(minimum)
                    ));
                }
            }
            if let Some(items) = schema.get("items").and_then(J::as_object) {
                for (index, item) in array.iter().enumerate() {
                    validate(items, item).map_err(|e| format!("item {index}: {e}"))?;
                }
            }
        }
        "string" => {
            let J::String(text) = value else {
                return Err(format!("expected string, got {}", go_type(value)));
            };
            if let Some(minimum) = number(schema.get("minLength")) {
                if (text.len() as f64) < minimum {
                    return Err(format!("string is shorter than {}", go_g(minimum)));
                }
            }
            if let Some(pattern) = schema.get("pattern").and_then(J::as_str) {
                let re = regex::Regex::new(pattern)
                    .map_err(|e| format!("invalid schema pattern: {e}"))?;
                if !re.is_match(text) {
                    return Err("string does not match pattern".into());
                }
            }
        }
        "integer" => match value.as_f64() {
            Some(n) if n.trunc() == n => {}
            _ => return Err(format!("expected integer, got {}", go_type(value))),
        },
        "number" => {
            if value.as_f64().is_none() {
                return Err(format!("expected number, got {}", go_type(value)));
            }
        }
        "boolean" => {
            if !value.is_boolean() {
                return Err(format!("expected boolean, got {}", go_type(value)));
            }
        }
        _ => {}
    }
    if let Some(minimum) = number(schema.get("minimum")) {
        if value.as_f64().is_none_or(|actual| actual < minimum) {
            return Err(format!("number is below minimum {}", go_g(minimum)));
        }
    }
    Ok(())
}

/// `argumentsForSchemaValidation`: a schema that requires `name` (and not
/// `vmName`) accepts `vmName` as its alias during validation.
pub fn arguments_for_validation(schema: &Map<String, J>, args: &Map<String, J>) -> Map<String, J> {
    let mut cloned = args.clone();
    let requires = |field: &str| {
        schema
            .get("required")
            .and_then(J::as_array)
            .is_some_and(|r| r.iter().any(|i| i.as_str() == Some(field)))
    };
    if !requires("name") || requires("vmName") {
        return cloned;
    }
    if cloned
        .get("name")
        .and_then(J::as_str)
        .is_some_and(|n| !n.trim().is_empty())
    {
        return cloned;
    }
    if let Some(vm) = cloned
        .get("vmName")
        .and_then(J::as_str)
        .filter(|v| !v.trim().is_empty())
    {
        let vm = vm.to_string();
        cloned.insert("name".into(), J::from(vm));
    }
    cloned
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn check(schema: J, value: J) -> Result<(), String> {
        validate(schema.as_object().unwrap(), &value)
    }

    #[test]
    fn go_texts() {
        let s = json!({"type": "object", "properties": {"fast": {"type": "boolean"}}});
        assert_eq!(
            check(s.clone(), json!({"fast": "true"})).unwrap_err(),
            r#"property "fast": expected boolean, got string"#
        );
        assert_eq!(
            check(s, json!([])).unwrap_err(),
            "expected object, got []interface {}"
        );
        assert_eq!(
            check(json!({"type": "integer"}), json!(1.5)).unwrap_err(),
            "expected integer, got float64"
        );
        assert_eq!(
            check(json!({"type": "integer", "minimum": 3600}), json!(10)).unwrap_err(),
            "number is below minimum 3600"
        );
        assert_eq!(
            check(json!({"enum": ["a", "b"]}), json!(1000000)).unwrap_err(),
            "value 1e+06 is not in enum"
        );
        assert_eq!(
            check(json!({"type": "object", "required": ["uri"]}), json!({})).unwrap_err(),
            r#"missing required property "uri""#
        );
        assert!(check(json!({"type": "string"}), json!("${node.out}")).is_ok());
        assert!(check(json!({"type": ["string", "null"]}), json!(null)).is_ok());
    }

    #[test]
    fn go_number_formats() {
        assert_eq!(go_g(100000.0), "100000");
        assert_eq!(go_g(1000000.0), "1e+06");
        assert_eq!(go_g(0.0001), "0.0001");
        assert_eq!(go_g(0.00001), "1e-05");
        assert_eq!(go_g(1.5), "1.5");
        // Checked against go1.25 fmt.
        assert_eq!(go_g(123456.0), "123456");
        assert_eq!(go_g(1234567.0), "1.234567e+06");
        assert_eq!(go_g(2.5e-7), "2.5e-07");
        assert_eq!(go_g(1e21), "1e+21");
        assert_eq!(
            go_v(&json!({"b": [1, true], "a": null})),
            "map[a:<nil> b:[1 true]]"
        );
    }
}
