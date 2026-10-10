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
const TASK_INACTIVE: &str =
    "host_resource_delegation_task_inactive: the provider callback task is not active";
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

/// Values borrowed from the owning Host Agent server, never caller tool args.
pub struct CallbackScope<'a> {
    pub agent: &'a str,
    pub tasks: &'a crate::tasks::Registry,
    pub host_tools: &'a [String],
    pub catalog: &'a crate::catalog::Snapshot,
}

/// A verified parent resource owner; the durable child keeps its own registry
/// identity while admission remains attached to this parent task's reservation.
pub struct TaskOwner {
    reservation: crate::resource::Reservation,
    operation: String,
    task: String,
}
impl TaskOwner {
    pub fn child_context(
        child: &crate::plan::runner::RunCtx,
        name: &str,
        task: &str,
        owner: Option<&Self>,
    ) -> crate::plan::runner::RunCtx {
        match owner {
            None => child.with_operation_identity(name, task),
            Some(owner) => child
                .with_reservation(owner.reservation.clone())
                .with_operation_identity(&owner.operation, &owner.task),
        }
    }
}
impl CallbackScope<'_> {
    pub fn task_owner(
        &self,
        ctx: &crate::plan::runner::RunCtx,
    ) -> Result<Option<TaskOwner>, &'static str> {
        let Some(reservation) = ctx.reservation() else {
            return Ok(None);
        };
        let (operation, task) = ctx.operation_identity();
        if matches!(reservation.id.as_str(), "control" | "unmanaged")
            || operation.trim().is_empty()
            || task.trim().is_empty()
            || reservation.request.agent_id != self.agent.trim()
            || reservation.request.operation_id != operation
            || reservation.request.task_id != task
        {
            return Err("host_resource_delegation_owner_invalid: provider callback reservation and task owner do not match");
        }
        if reservation
            .expires_at
            .is_none_or(|expiry| !go_time_before(std::time::SystemTime::now(), expiry))
        {
            return Err(
                "host_resource_delegation_expired: provider callback reservation has expired",
            );
        }
        if !self.tasks.is_working(&task) {
            return Err(TASK_INACTIVE);
        }
        Ok(Some(TaskOwner {
            reservation: (*reservation).clone(),
            operation,
            task,
        }))
    }
}
// Go time.Unix stores signed seconds since year 1. That epoch shift wraps
// for extreme signed claims; Time.Before compares the shifted signed value.
fn go_time_before(left: std::time::SystemTime, right: std::time::SystemTime) -> bool {
    let parts = |time: std::time::SystemTime| {
        let nanos = match time.duration_since(std::time::UNIX_EPOCH) {
            Ok(duration) => duration.as_nanos() as i128,
            Err(error) => -(error.duration().as_nanos() as i128),
        };
        (
            (nanos.div_euclid(1_000_000_000) as i64).wrapping_add(62_135_596_800),
            nanos.rem_euclid(1_000_000_000),
        )
    };
    parts(left) < parts(right)
}

fn lifecycle_tool(name: &str) -> bool {
    matches!(
        name,
        "validate_host_plan"
            | "run_host_plan"
            | "get_host_plan_run"
            | "validate_host_local_recipe"
            | "run_host_local_recipe"
            | "validate_runtime_recipe"
            | "run_runtime_recipe"
            | "get_runtime_recipe_run"
            | "validate_tunnel_recipe"
            | "run_tunnel_recipe"
            | "get_tunnel_run"
            | "opute.provider.install"
            | "opute.provider.validate"
            | "opute.provider.status"
            | "opute.provider.reload"
            | "opute.provider.teardown"
            | "get_capability_catalog"
            | "open_assistant_session"
    )
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
    #[cfg(test)]
    pub fn active_count(&self) -> usize {
        self.active.lock().unwrap().len()
    }
    pub fn new() -> Result<Self, getrandom::Error> {
        let mut key = vec![0; 32];
        getrandom::getrandom(&mut key)?;
        Ok(Self {
            key,
            active: Arc::default(),
        })
    }
    /// Issuance follows the independent durable task identity in context;
    /// a reservation alone cannot assert that it owns the current work.
    pub fn issue(
        &self,
        ctx: &crate::plan::runner::RunCtx,
        tasks: &crate::tasks::Registry,
        agent: &str,
        provider: &str,
        generation: &str,
        operation: &str,
    ) -> Result<(crate::plan::runner::RunCtx, Option<Permit>), &'static str> {
        let Some(reservation) = ctx.reservation() else {
            return Ok((ctx.clone(), None));
        };
        if matches!(reservation.id.as_str(), "" | "control" | "unmanaged") {
            return Ok((ctx.clone(), None));
        }
        let (task_operation, task_id) = ctx.operation_identity();
        if task_id.is_empty() || reservation.request.task_id != task_id {
            return Err("host_resource_delegation_owner_invalid: provider callback reservation is not bound to the active durable task");
        }
        if task_operation.is_empty() || reservation.request.operation_id != task_operation {
            return Err("host_resource_delegation_owner_invalid: provider callback operation does not match its reservation owner");
        }
        let agent = agent.trim();
        if agent.is_empty() || reservation.request.agent_id != agent {
            return Err("host_resource_delegation_owner_invalid: provider callback reservation does not belong to this Host Agent");
        }
        if !tasks.is_working(&task_id) {
            return Err(
                "host_resource_delegation_task_inactive: the provider callback task is not active",
            );
        }
        if provider.trim().is_empty()
            || generation.trim().is_empty()
            || operation.trim().is_empty()
            || task_operation.trim().is_empty()
        {
            return Err("host_resource_delegation_context_invalid: provider and task identities are required");
        }
        let id = crate::tasks::new_task_id();
        let claims = Claims {
            strings: [
                VERSION,
                &id,
                &reservation.id,
                agent,
                &task_id,
                &task_operation,
                provider,
                generation,
                operation,
            ]
            .map(String::from),
            expires_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64
                + 3600,
        };
        let permit = self.sign(&claims);
        Ok((ctx.with_resource_delegation(permit.token()), Some(permit)))
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
    /// Reattaches a verified resource owner before ordinary authenticated
    /// dispatch. This check supplies no authorization or admission decision.
    pub fn callback(
        &self,
        ctx: &crate::plan::runner::RunCtx,
        meta: Option<&serde_json::Map<String, serde_json::Value>>,
        tool: &str,
        scope: &CallbackScope<'_>,
    ) -> Result<crate::plan::runner::RunCtx, &'static str> {
        let token = meta
            .and_then(|meta| meta.get("io.opute/host-resource-delegation"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .trim();
        if token.is_empty() {
            return Ok(ctx.clone());
        }
        let now = std::time::SystemTime::now();
        let claims = self.verify(
            token,
            now.duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64,
        )?;
        let provider = scope
            .catalog
            .tools
            .iter()
            .find(|descriptor| descriptor.name == tool)
            .is_some_and(|descriptor| descriptor.implementation.trim().starts_with("provider:"));
        if claims.strings[3] != scope.agent.trim()
            || lifecycle_tool(tool)
            || provider
            || !scope.host_tools.iter().any(|name| name == tool)
        {
            return Err("host_resource_delegation_scope_invalid: callback is outside the signed Host Agent execution scope");
        }
        if !scope.tasks.is_working(&claims.strings[4]) {
            return Err(TASK_INACTIVE);
        }
        let reservation = crate::resource::Reservation {
            id: claims.strings[2].clone(),
            request: crate::resource::AdmissionRequest {
                agent_id: claims.strings[3].clone(),
                task_id: claims.strings[4].clone(),
                operation_id: claims.strings[5].clone(),
                ..Default::default()
            },
            created_at: Some(now),
            expires_at: std::time::UNIX_EPOCH
                .checked_add(std::time::Duration::from_secs(claims.expires_at as u64)),
            inherited: false,
        };
        Ok(ctx
            .with_reservation(reservation)
            .with_operation_identity(&claims.strings[5], &claims.strings[4]))
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
    fn context_observation(ctx: &crate::plan::runner::RunCtx) -> serde_json::Value {
        use serde_json::json;
        let reservation = ctx.reservation().map(|r| {
            let expiry = r.expires_at.map(|expiry| match expiry.duration_since(std::time::UNIX_EPOCH) { Ok(d)=>d.as_secs() as i64, Err(e)=>(-(e.duration().as_secs() as i128)) as i64 });
            json!({"id":r.id,"agent":r.request.agent_id,"task":r.request.task_id,"operation":r.request.operation_id,"expiresAt":expiry,"created":r.created_at.is_some()})
        });
        json!({"reservation":reservation,"identity":ctx.operation_identity(),"error":ctx.err().unwrap_or_default(),"delegation":ctx.resource_delegation()})
    }
    fn callback_fixture(input: &serde_json::Value) -> serde_json::Value {
        use crate::{
            plan::runner::RunCtx,
            resource::{AdmissionRequest, Reservation},
            tasks::Registry,
        };
        use serde_json::{json, Value};
        let string = |value: &Value| value.as_str().unwrap_or_default().to_owned();
        let owner = Delegations {
            key: hex::decode(input["key"].as_str().unwrap()).unwrap(),
            active: Arc::default(),
        };
        for (id, token) in input["active"].as_object().unwrap() {
            owner
                .active
                .lock()
                .unwrap()
                .insert(id.clone(), string(token));
        }
        let tasks = Registry::default();
        let task = string(&input["registryTask"]);
        if input["status"] == "working" {
            tasks.create_with_id(&task, "fixture", "", json!({}));
        } else if input["status"] != "missing" {
            tasks.restore_snapshot(
                &json!({"taskId":task,"toolName":"fixture","status":input["status"]}),
            );
        }
        let mut catalog = crate::catalog::internal().clone();
        let template = catalog.tools[0].clone();
        catalog.tools.clear();
        for (name, implementation) in input["implementations"].as_object().unwrap() {
            if implementation.is_null() {
                continue;
            }
            let mut descriptor = template.clone();
            descriptor.name = name.clone();
            descriptor.implementation = string(implementation);
            catalog.tools.push(descriptor);
        }
        let host_tools: Vec<String> = input["hostTools"]
            .as_array()
            .unwrap()
            .iter()
            .map(string)
            .collect();
        let agent = string(&input["agent"]);
        let scope = CallbackScope {
            agent: &agent,
            tasks: &tasks,
            host_tools: &host_tools,
            catalog: &catalog,
        };
        let (mut ctx, cancel) = RunCtx::background().with_cancel();
        ctx = ctx
            .with_operation_identity(&string(&input["operation"]), &string(&input["task"]))
            .with_resource_delegation("parent-only-token");
        let reservation = &input["reservation"];
        if !reservation.is_null() {
            ctx = ctx.with_reservation(Reservation {
                id: string(&reservation["id"]),
                request: AdmissionRequest {
                    agent_id: string(&reservation["agent"]),
                    task_id: string(&reservation["task"]),
                    operation_id: string(&reservation["operation"]),
                    ..Default::default()
                },
                inherited: false,
                created_at: None,
                expires_at: reservation["expiresAt"].as_i64().and_then(|sec| {
                    if sec >= 0 {
                        std::time::UNIX_EPOCH
                            .checked_add(std::time::Duration::from_secs(sec as u64))
                    } else {
                        std::time::UNIX_EPOCH
                            .checked_sub(std::time::Duration::from_secs(sec.unsigned_abs()))
                    }
                }),
            });
        }
        if input["cancelled"].as_bool().unwrap_or(false) {
            cancel.cancel();
        }
        let mut output =
            json!({"error":"","ownerError":"","callback":null,"child":null,"owner":false});
        let ctx = match owner.callback(
            &ctx,
            input["meta"].as_object(),
            &string(&input["tool"]),
            &scope,
        ) {
            Ok(ctx) => ctx,
            Err(error) => {
                output["error"] = json!(error);
                return output;
            }
        };
        output["callback"] = context_observation(&ctx);
        let task_owner = match scope.task_owner(&ctx) {
            Ok(owner) => owner,
            Err(error) => {
                output["ownerError"] = json!(error);
                return output;
            }
        };
        output["owner"] = json!(task_owner.is_some());
        let (child, cancel) = RunCtx::background().with_cancel();
        let child = child.with_resource_delegation("child-only-token");
        if input["childCancelled"].as_bool().unwrap_or(false) {
            cancel.cancel();
        }
        let child =
            TaskOwner::child_context(&child, "child.operation", "child-task", task_owner.as_ref());
        output["child"] = context_observation(&child);
        output
    }

    #[test]
    #[ignore = "requires actual owning Go callback scope and task-owner driver"]
    fn callback_scope_fixture() {
        let rows: serde_json::Value = serde_json::from_slice(
            &std::fs::read(std::env::var("PARITY_PROVIDER_CALLBACK_INPUT").unwrap()).unwrap(),
        )
        .unwrap();
        let outputs: Vec<_> = rows
            .as_array()
            .unwrap()
            .iter()
            .map(|row| callback_fixture(&row["callback"]))
            .collect();
        std::fs::write(
            std::env::var("PARITY_PROVIDER_CALLBACK_OUTPUT").unwrap(),
            serde_json::to_vec_pretty(&outputs).unwrap(),
        )
        .unwrap();
    }
    fn issue_fixture(input: &serde_json::Value) -> serde_json::Value {
        use crate::{
            plan::runner::RunCtx,
            resource::{AdmissionRequest, Reservation},
            tasks::Registry,
        };
        use serde_json::json;
        let string = |value: &serde_json::Value| value.as_str().unwrap_or_default().to_owned();
        let owner = Delegations {
            key: hex::decode(input["key"].as_str().unwrap()).unwrap(),
            active: Arc::default(),
        };
        let tasks = Registry::default();
        let task_id = string(&input["registryTask"]);
        let status = string(&input["status"]);
        if status == "working" {
            tasks.create_with_id(&task_id, "fixture", "", json!({}));
        } else if status != "missing" {
            tasks.restore_snapshot(&json!({"taskId":task_id,"toolName":"fixture","status":status}));
        }
        let (mut ctx, cancel) = RunCtx::background().with_cancel();
        ctx = ctx.with_resource_delegation(&string(&input["priorToken"]));
        let reservation = &input["reservation"];
        if !reservation.is_null() {
            ctx = ctx.with_reservation(Reservation {
                id: string(&reservation["id"]),
                request: AdmissionRequest {
                    agent_id: string(&reservation["agent"]),
                    task_id: string(&reservation["task"]),
                    operation_id: string(&reservation["operation"]),
                    ..Default::default()
                },
                inherited: false,
                created_at: None,
                expires_at: None,
            });
        }
        ctx = ctx.with_operation_identity(&string(&input["operation"]), &string(&input["task"]));
        // Context values must walk through independent child cancellation nodes.
        let (ctx, _) = ctx.with_cancel();
        if input["cancelled"].as_bool().unwrap_or(false) {
            cancel.cancel();
        }
        let issued = owner.issue(
            &ctx,
            &tasks,
            &string(&input["agent"]),
            &string(&input["provider"]),
            &string(&input["generation"]),
            &string(&input["providerOperation"]),
        );
        let mut output = json!({"error":"","token":"","claims":Claims::default().encode(),"verificationError":"","releaseError":"","contextError":"","issued":false});
        let (delegated, permit) = match issued {
            Ok(value) => value,
            Err(error) => {
                output["error"] = json!(error);
                return output;
            }
        };
        output["token"] = json!(delegated.resource_delegation());
        output["contextError"] = json!(delegated.err().unwrap_or_default());
        output["issued"] = json!(permit.is_some());
        if let Some(permit) = permit {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64;
            match owner.verify(permit.token(), now) {
                Ok(claims) => output["claims"] = json!(claims.encode()),
                Err(error) => output["verificationError"] = json!(error),
            }
            let token = permit.token().to_owned();
            drop(permit);
            output["releaseError"] = json!(owner.verify(&token, now).unwrap_err());
        }
        output
    }

    #[test]
    #[ignore = "requires actual owning Go typed delegation issuance driver"]
    fn issuance_fixture() {
        let rows: serde_json::Value = serde_json::from_slice(
            &std::fs::read(std::env::var("PARITY_PROVIDER_ISSUANCE_INPUT").unwrap()).unwrap(),
        )
        .unwrap();
        let outputs: Vec<_> = rows
            .as_array()
            .unwrap()
            .iter()
            .map(|row| issue_fixture(&row["issuance"]))
            .collect();
        std::fs::write(
            std::env::var("PARITY_PROVIDER_ISSUANCE_OUTPUT").unwrap(),
            serde_json::to_vec_pretty(&outputs).unwrap(),
        )
        .unwrap();
    }
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
