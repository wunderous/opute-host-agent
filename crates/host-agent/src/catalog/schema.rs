//! Native components of the pinned google/jsonschema-go v0.4.3 compiler.
//!
//! This module is not yet a complete checker and is deliberately not wired to
//! registry admission. Integer keywords use the owner's custom int32 decoder,
//! rather than a meta-schema rule (negative bounds are accepted by the owner).
use crate::gojson::{Node, Value};

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
