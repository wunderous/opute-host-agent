//! Provider-owned output validation from capability.NewProviderAdapter.
//! Observations retain declared structured results; no resources or completion
//! semantics are inferred from content, resource bindings, or generation IDs.
use crate::catalog::Descriptor;
use serde_json::{Map, Value};

#[derive(Default)]
pub struct Observation {
    pub schema_version: String,
    pub operation_id: String,
    pub capability_version: i64,
    pub status: String,
    pub structured: Option<Value>,
    pub retryability: String,
}
impl Observation {
    pub fn encode(&self) -> String {
        let mut fields = vec![
            ("schemaVersion", Value::String(self.schema_version.clone())),
            ("operationId", Value::String(self.operation_id.clone())),
            ("capabilityVersion", Value::from(self.capability_version)),
            ("status", Value::String(self.status.clone())),
        ];
        if let Some(structured) = &self.structured {
            fields.push(("structured", structured.clone()));
        }
        if !self.retryability.is_empty() {
            fields.push(("retryability", Value::String(self.retryability.clone())));
        }
        let mut out = String::from("{");
        for (index, (key, value)) in fields.iter().enumerate() {
            if index != 0 {
                out.push(',');
            }
            crate::gojson::encode_string(key, &mut out);
            out.push(':');
            crate::gojson::encode(value, &mut out);
        }
        out.push('}');
        out
    }
}
pub fn validate(
    descriptor: &Descriptor,
    result: Option<&Map<String, Value>>,
) -> Result<Observation, String> {
    // The outer capability adapter refuses nil before invoking its provider hook.
    let result = result.ok_or("capability returned a nil result")?;
    let is_error = result
        .get("isError")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let structured = result
        .get("structuredContent")
        .filter(|value| !value.is_null());
    let empty_schema = Map::new();
    if !is_error {
        crate::schema::validate(
            descriptor
                .output_schema
                .as_ref()
                .and_then(Value::as_object)
                .unwrap_or(&empty_schema),
            structured.unwrap_or(&Value::Null),
        )
        .map_err(|error| {
            format!("provider structured result does not match output schema: {error}")
        })?;
    }
    Ok(Observation {
        schema_version: "opute-capability-observation.v1".into(),
        operation_id: descriptor.operation_id.clone(),
        capability_version: descriptor.version,
        status: if is_error { "error" } else { "success" }.into(),
        structured: structured.cloned(),
        retryability: if is_error { "capability" } else { "" }.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "requires the owning Go provider observation differential driver"]
    fn observation_fixture() {
        let rows: Value = serde_json::from_slice(
            &std::fs::read(std::env::var("PARITY_PROVIDER_OBSERVATION_INPUT").unwrap()).unwrap(),
        )
        .unwrap();
        let mut outputs = Vec::new();
        for row in rows.as_array().unwrap() {
            let raw = serde_json::to_vec(&row["observation"]["manifest"]).unwrap();
            let manifest =
                super::super::definition::decode_manifest(&crate::gojson::parse(&raw).unwrap())
                    .unwrap();
            let operation = super::super::binding::operations(&manifest, "g1", false).remove(0);
            let source = &row["observation"]["result"];
            let result = if source.is_null() {
                None
            } else {
                Some(super::super::result::project(source.as_object().unwrap()).unwrap())
            };
            let validated = operation.validate_result(result.as_ref());
            let (observation, error) = match validated {
                Ok(observation) => (observation, String::new()),
                Err(error) => (Observation::default(), error),
            };
            outputs.push(serde_json::json!({"observation":observation.encode(),"error":error}));
        }
        std::fs::write(
            std::env::var("PARITY_PROVIDER_OBSERVATION_OUTPUT").unwrap(),
            serde_json::to_vec_pretty(&outputs).unwrap(),
        )
        .unwrap();
    }
}
