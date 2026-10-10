//! Process-local signed callback claims from hostmcp/resource_delegation.go.
//! This cryptographic owner does not authorize dispatch. Issuance must follow
//! reservation/task ownership checks; callback scope and admission remain
//! separate owning boundaries before public provider composition is opened.
use base64::{alphabet, engine::general_purpose, Engine};
use ring::hmac;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

const VERSION: &str = "host-resource-delegation.v1";
const MALFORMED: &str =
    "host_resource_delegation_invalid: provider callback delegation is malformed";
const SIGNATURE: &str =
    "host_resource_delegation_invalid: provider callback delegation signature is invalid";
const EXPIRED: &str =
    "host_resource_delegation_expired: provider callback delegation is expired or incomplete";
const INACTIVE: &str =
    "host_resource_delegation_inactive: provider callback delegation is no longer active";
const FIELDS: [&str; 9] = [
    "Version",
    "ID",
    "ReservationID",
    "AgentID",
    "TaskID",
    "TaskOperation",
    "ProviderID",
    "ProviderGeneration",
    "ProviderOperation",
];

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Claims {
    strings: [String; 9],
    pub expires_at: i64,
}
impl Claims {
    fn decode(payload: &[u8]) -> Result<Self, &'static str> {
        use crate::gojson::{equal_fold, Value};
        let root = crate::gojson::parse(payload).map_err(|_| MALFORMED)?;
        let mut claims = Self::default();
        if root.is_null() {
            return Ok(claims);
        }
        for (name, node) in root.as_object().ok_or(MALFORMED)? {
            // Null leaves the previous value intact, including duplicate keys.
            if node.is_null() {
                continue;
            }
            if let Some(index) = FIELDS.iter().position(|field| equal_fold(name, field)) {
                claims.strings[index] = node.as_str().ok_or(MALFORMED)?.into();
            } else if equal_fold(name, "ExpiresAt") {
                let Value::Number(number) = &node.value else {
                    return Err(MALFORMED);
                };
                claims.expires_at = number.parse().map_err(|_| MALFORMED)?;
            }
        }
        Ok(claims)
    }
    pub fn encode(&self) -> String {
        let mut out = String::from("{");
        for (index, name) in FIELDS.iter().enumerate() {
            if index != 0 {
                out.push(',');
            }
            crate::gojson::encode_string(name, &mut out);
            out.push(':');
            crate::gojson::encode_string(&self.strings[index], &mut out);
        }
        out.push_str(",\"ExpiresAt\":");
        out.push_str(&self.expires_at.to_string());
        out.push('}');
        out
    }
    pub fn id(&self) -> &str {
        &self.strings[1]
    }
    fn complete(&self, now: i64) -> bool {
        self.strings[0] == VERSION
            && self.strings.iter().all(|value| !value.is_empty())
            && now < self.expires_at
    }
}

#[derive(Clone)]
pub struct Delegations {
    key: Vec<u8>,
    active: Arc<Mutex<HashMap<String, String>>>,
}
impl Delegations {
    pub fn new() -> Result<Self, getrandom::Error> {
        let mut key = vec![0; 32];
        getrandom::getrandom(&mut key)?;
        Ok(Self {
            key,
            active: Arc::default(),
        })
    }
    // Private until the typed reservation/task issuer is composed. A token
    // remains live exactly while its operation's permit is retained.
    fn sign(&self, claims: &Claims) -> Permit {
        let payload = general_purpose::URL_SAFE_NO_PAD.encode(claims.encode());
        let signature = hmac::sign(
            &hmac::Key::new(hmac::HMAC_SHA256, &self.key),
            payload.as_bytes(),
        );
        let token = format!(
            "{payload}.{}",
            general_purpose::URL_SAFE_NO_PAD.encode(signature)
        );
        self.active
            .lock()
            .unwrap()
            .insert(claims.id().into(), token.clone());
        Permit {
            owner: self.clone(),
            id: claims.id().into(),
            token,
        }
    }
    pub fn verify(&self, token: &str, now: i64) -> Result<Claims, &'static str> {
        let mut parts = token.split('.');
        let payload = parts.next().unwrap_or_default();
        let signature = parts.next().ok_or(MALFORMED)?;
        if parts.next().is_some() || self.key.is_empty() {
            return Err(MALFORMED);
        }
        let signature = decode(signature).map_err(|_| SIGNATURE)?;
        hmac::verify(
            &hmac::Key::new(hmac::HMAC_SHA256, &self.key),
            payload.as_bytes(),
            &signature,
        )
        .map_err(|_| SIGNATURE)?;
        let payload = decode(payload).map_err(|_| MALFORMED)?;
        let claims = Claims::decode(&payload)?;
        if !claims.complete(now) {
            return Err(EXPIRED);
        }
        if self
            .active
            .lock()
            .unwrap()
            .get(claims.id())
            .is_none_or(|active| active != token)
        {
            return Err(INACTIVE);
        }
        Ok(claims)
    }
}
fn decode(value: &str) -> Result<Vec<u8>, base64::DecodeError> {
    // Go RawURLEncoding ignores CR/LF and accepts noncanonical trailing bits,
    // while rejecting padding and every other character outside the alphabet.
    let engine = general_purpose::GeneralPurpose::new(
        &alphabet::URL_SAFE,
        general_purpose::GeneralPurposeConfig::new()
            .with_decode_padding_mode(base64::engine::DecodePaddingMode::RequireNone)
            .with_decode_allow_trailing_bits(true),
    );
    engine.decode(
        value
            .bytes()
            .filter(|byte| !matches!(byte, b'\r' | b'\n'))
            .collect::<Vec<_>>(),
    )
}

pub struct Permit {
    owner: Delegations,
    id: String,
    token: String,
}
impl Permit {
    pub fn token(&self) -> &str {
        &self.token
    }
}
impl Drop for Permit {
    fn drop(&mut self) {
        self.owner.active.lock().unwrap().remove(&self.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn permit_release_revokes_without_changing_signed_claims() {
        let owner = Delegations::new().unwrap();
        let claims = Claims {
            strings: [
                VERSION,
                "id",
                "reservation",
                "opaque agent",
                "task",
                "operation",
                "provider",
                "generation",
                "provider.operation",
            ]
            .map(String::from),
            expires_at: 200,
        };
        let permit = owner.sign(&claims);
        assert_eq!(owner.verify(&permit.token, 100), Ok(claims.clone()));
        assert_eq!(owner.verify(&permit.token, 200), Err(EXPIRED));
        assert_eq!(
            Delegations::new().unwrap().verify(&permit.token, 100),
            Err(SIGNATURE)
        );
        let token = permit.token.clone();
        drop(permit);
        assert_eq!(owner.verify(&token, 100), Err(INACTIVE));
    }

    #[tokio::test]
    async fn aborting_an_accepted_operation_revokes_its_permit() {
        let owner = Delegations::new().unwrap();
        let claims = Claims {
            strings: [
                VERSION,
                "id",
                "reservation",
                "agent",
                "task",
                "operation",
                "provider",
                "generation",
                "provider.operation",
            ]
            .map(String::from),
            expires_at: 200,
        };
        let permit = owner.sign(&claims);
        let token = permit.token().to_owned();
        let (accepted, ready) = tokio::sync::oneshot::channel();
        let operation = tokio::spawn(async move {
            let _permit = permit;
            accepted.send(()).unwrap();
            std::future::pending::<()>().await;
        });
        ready.await.unwrap();
        assert_eq!(owner.verify(&token, 100), Ok(claims));
        operation.abort();
        assert!(operation.await.unwrap_err().is_cancelled());
        assert_eq!(owner.verify(&token, 100), Err(INACTIVE));
    }

    #[test]
    #[ignore = "requires actual owning Go signed delegation differential driver"]
    fn delegation_fixture() {
        use serde_json::{json, Value};
        let rows: Value = serde_json::from_slice(
            &std::fs::read(std::env::var("PARITY_PROVIDER_DELEGATION_INPUT").unwrap()).unwrap(),
        )
        .unwrap();
        let mut outputs = Vec::new();
        for row in rows.as_array().unwrap() {
            let input = &row["delegation"];
            let owner = Delegations {
                key: hex::decode(input["key"].as_str().unwrap()).unwrap(),
                active: Arc::default(),
            };
            for (id, token) in input["active"].as_object().unwrap() {
                owner
                    .active
                    .lock()
                    .unwrap()
                    .insert(id.clone(), token.as_str().unwrap().into());
            }
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64;
            let (claims, error) = match owner.verify(input["token"].as_str().unwrap(), now) {
                Ok(claims) => (claims, ""),
                Err(error) => (Claims::default(), error),
            };
            outputs.push(json!({"claims":claims.encode(),"error":error}));
        }
        std::fs::write(
            std::env::var("PARITY_PROVIDER_DELEGATION_OUTPUT").unwrap(),
            serde_json::to_vec_pretty(&outputs).unwrap(),
        )
        .unwrap();
    }
}
