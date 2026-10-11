//! Provider generation ownership, pinned to `internal/cordis/generation.go`.
//! Runtime connections belong to provider mounts, not these durable projections.
//! A session owns the generation it accepted; replacement never redirects it.

pub mod activation;
pub mod adapter;
pub mod binding;
pub mod candidates;
pub mod definition;
pub mod delegation;
pub mod mounts;
pub mod observation;
pub mod publication;
pub mod result;

use crate::{goerr::quote, hostobs::rfc3339_nano_now, plan::runner::RunCtx};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderRef {
    pub id: String,
    pub version: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GenerationState {
    #[default]
    Candidate,
    Ready,
    Active,
    Draining,
    Stopped,
    Failed,
    // Restore preserves unknown observations; they remain undispatchable.
    #[serde(untagged)]
    Other(String),
}
impl GenerationState {
    fn name(&self) -> &str {
        match self {
            Self::Candidate => "candidate",
            Self::Ready => "ready",
            Self::Active => "active",
            Self::Draining => "draining",
            Self::Stopped => "stopped",
            Self::Failed => "failed",
            Self::Other(s) => s,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct Generation {
    #[serde(rename = "ID")]
    pub id: String,
    pub provider: ProviderRef,
    pub manifest_hash: String,
    pub endpoint: String,
    pub catalog_revision: String,
    pub state: GenerationState,
    pub created_at: String,
    pub active_at: String,
    pub sessions: i64,
}
impl Default for Generation {
    fn default() -> Self {
        Self {
            id: String::new(),
            provider: ProviderRef::default(),
            manifest_hash: String::new(),
            endpoint: String::new(),
            catalog_revision: String::new(),
            state: GenerationState::default(),
            created_at: "0001-01-01T00:00:00Z".into(),
            active_at: "0001-01-01T00:00:00Z".into(),
            sessions: 0,
        }
    }
}
#[derive(Clone, Copy)]
pub struct DrainPolicy {
    pub timeout: Duration,
    pub cancel_on_timeout: bool,
}
impl Default for DrainPolicy {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            cancel_on_timeout: false,
        }
    }
}
#[derive(Default)]
struct Generations {
    generations: BTreeMap<String, Generation>,
    active: BTreeMap<String, String>,
    sequence: u64,
}
#[derive(Clone)]
pub struct Lifecycle {
    inner: Arc<Mutex<Generations>>,
    policy: DrainPolicy,
}
impl Lifecycle {
    pub fn new(mut policy: DrainPolicy) -> Self {
        if policy.timeout.is_zero() {
            policy.timeout = Duration::from_secs(30);
        }
        Self {
            inner: Arc::new(Mutex::new(Generations::default())),
            policy,
        }
    }
    pub fn restore(&self, mut generation: Generation) -> Result<(), String> {
        if generation.id.is_empty()
            || generation.provider.id.is_empty()
            || generation.provider.version.is_empty()
            || generation.manifest_hash.is_empty()
            || generation.endpoint.is_empty()
        {
            return Err(
                "provider generation identity, manifest hash, and endpoint are required".into(),
            );
        }
        if generation.state.name().is_empty() {
            generation.state = GenerationState::Candidate;
        }
        let mut inner = self.inner.lock().unwrap();
        if inner.generations.contains_key(&generation.id) {
            return Ok(());
        }
        let sequence = generation
            .id
            .rsplit_once('-')
            .and_then(|(_, s)| {
                // strconv.ParseUint rejects sign prefixes and whitespace.
                (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
                    .then(|| s.parse::<u64>().ok())
                    .flatten()
            })
            .unwrap_or(0);
        inner.sequence = inner.sequence.max(sequence);
        // Go inserts the observation before rejecting a conflicting active
        // projection. Preserve that state even when Restore returns an error.
        inner
            .generations
            .insert(generation.id.clone(), generation.clone());
        if generation.state == GenerationState::Active {
            if inner
                .active
                .get(&generation.provider.id)
                .is_some_and(|id| !id.is_empty() && id != &generation.id)
            {
                return Err(format!(
                    "provider {} has multiple active generations",
                    quote(&generation.provider.id)
                ));
            }
            inner.active.insert(generation.provider.id, generation.id);
        }
        Ok(())
    }
    pub fn get(&self, id: &str) -> Option<Generation> {
        self.inner.lock().unwrap().generations.get(id).cloned()
    }
    pub fn create_candidate(
        &self,
        provider: ProviderRef,
        manifest_hash: &str,
        endpoint: &str,
        revision: &str,
    ) -> Result<Generation, String> {
        if provider.id.is_empty()
            || provider.version.is_empty()
            || manifest_hash.is_empty()
            || endpoint.is_empty()
            || revision.trim().is_empty()
        {
            return Err(
                "provider, manifest hash, endpoint, and catalog revision are required".into(),
            );
        }
        let mut inner = self.inner.lock().unwrap();
        inner.sequence = inner.sequence.wrapping_add(1);
        let id = format!("{}-{}", provider.id, inner.sequence);
        let generation = Generation {
            id: id.clone(),
            provider,
            manifest_hash: manifest_hash.into(),
            endpoint: endpoint.into(),
            catalog_revision: revision.into(),
            created_at: rfc3339_nano_now(),
            ..Generation::default()
        };
        inner.generations.insert(id, generation.clone());
        Ok(generation)
    }
    pub fn mark_ready(&self, id: &str) -> Result<(), String> {
        let mut inner = self.inner.lock().unwrap();
        let generation = inner.generations.get_mut(id).ok_or_else(|| not_found(id))?;
        if generation.state != GenerationState::Candidate {
            return Err(expected(id, &generation.state, "candidate"));
        }
        generation.state = GenerationState::Ready;
        Ok(())
    }
    pub fn activate(&self, id: &str) -> Result<(Option<Generation>, Generation), String> {
        let mut inner = self.inner.lock().unwrap();
        let candidate = inner.generations.get(id).ok_or_else(|| not_found(id))?;
        if candidate.state != GenerationState::Ready {
            return Err(expected(id, &candidate.state, "ready"));
        }
        let provider_id = candidate.provider.id.clone();
        let previous_id = inner.active.get(&provider_id).cloned();
        let mut previous = None;
        if let Some(old_id) = previous_id.filter(|old| !old.is_empty() && old != id) {
            if let Some(old) = inner.generations.get_mut(&old_id) {
                if old.state == GenerationState::Active {
                    old.state = GenerationState::Draining;
                    previous = Some(old.clone());
                }
            }
        }
        let candidate = inner.generations.get_mut(id).unwrap();
        candidate.state = GenerationState::Active;
        candidate.active_at = rfc3339_nano_now();
        let active = candidate.clone();
        inner.active.insert(provider_id, id.into());
        Ok((previous, active))
    }
    pub fn rollback_activation(&self, id: &str, previous_id: &str) -> Result<(), String> {
        let mut inner = self.inner.lock().unwrap();
        let current = inner
            .generations
            .get(id)
            .filter(|g| {
                g.state == GenerationState::Active
                    && inner
                        .active
                        .get(&g.provider.id)
                        .is_some_and(|active| active == id)
            })
            .ok_or_else(|| {
                format!(
                    "provider generation {} is not the active generation",
                    quote(id)
                )
            })?;
        if current.sessions > 0 {
            return Err(format!(
                "provider generation {} has {} active session(s)",
                quote(id),
                current.sessions
            ));
        }
        let provider_id = current.provider.id.clone();
        if previous_id.is_empty() {
            inner.active.remove(&provider_id);
        } else {
            let previous = inner
                .generations
                .get_mut(previous_id)
                .filter(|g| g.provider.id == provider_id && g.state == GenerationState::Draining)
                .ok_or_else(|| {
                    format!(
                        "provider generation {} cannot restore previous generation {}",
                        quote(id),
                        quote(previous_id)
                    )
                })?;
            previous.state = GenerationState::Active;
            inner.active.insert(provider_id, previous_id.into());
        }
        inner.generations.get_mut(id).unwrap().state = GenerationState::Failed;
        Ok(())
    }
    pub fn active(&self, provider: &str) -> Option<Generation> {
        let inner = self.inner.lock().unwrap();
        inner
            .active
            .get(provider)
            .and_then(|id| inner.generations.get(id))
            .filter(|g| g.state == GenerationState::Active)
            .cloned()
    }
    pub fn refresh(
        &self,
        id: &str,
        manifest_hash: &str,
        revision: &str,
    ) -> Result<Generation, String> {
        if id.trim().is_empty() || manifest_hash.trim().is_empty() || revision.trim().is_empty() {
            return Err(
                "provider generation, manifest hash, and catalog revision are required".into(),
            );
        }
        let mut inner = self.inner.lock().unwrap();
        let generation = inner.generations.get_mut(id).ok_or_else(|| not_found(id))?;
        if generation.state != GenerationState::Active {
            return Err(expected(id, &generation.state, "active"));
        }
        generation.manifest_hash = manifest_hash.into();
        generation.catalog_revision = revision.into();
        Ok(generation.clone())
    }
    pub fn open_session(&self, provider: &str) -> Result<Session, String> {
        let mut inner = self.inner.lock().unwrap();
        let id = inner.active.get(provider).cloned().unwrap_or_default();
        let generation = inner
            .generations
            .get_mut(&id)
            .filter(|g| g.state == GenerationState::Active)
            .ok_or_else(|| format!("provider {} has no active generation", quote(provider)))?;
        generation.sessions = generation.sessions.wrapping_add(1);
        Ok(Session {
            lifecycle: self.clone(),
            id,
            closed: false,
        })
    }
    pub fn open_session_for_generation(&self, provider: &str, id: &str) -> Result<Session, String> {
        if id.trim().is_empty() {
            return Err("provider generation is required".into());
        }
        let mut inner = self.inner.lock().unwrap();
        let generation = inner
            .generations
            .get_mut(id)
            .filter(|g| g.provider.id == provider)
            .ok_or_else(|| {
                format!(
                    "provider generation {} is not registered for provider {}",
                    quote(id),
                    quote(provider)
                )
            })?;
        match generation.state {
            GenerationState::Candidate
            | GenerationState::Ready
            | GenerationState::Active
            | GenerationState::Draining => {}
            _ => {
                return Err(format!(
                    "provider generation {} is {}",
                    quote(id),
                    generation.state.name()
                ))
            }
        }
        generation.sessions = generation.sessions.wrapping_add(1);
        Ok(Session {
            lifecycle: self.clone(),
            id: id.into(),
            closed: false,
        })
    }
    pub fn fail(&self, id: &str) -> Result<(), String> {
        let mut inner = self.inner.lock().unwrap();
        let generation = inner.generations.get_mut(id).ok_or_else(|| not_found(id))?;
        generation.state = GenerationState::Failed;
        let provider = generation.provider.id.clone();
        if inner
            .active
            .get(&provider)
            .is_some_and(|active| active == id)
        {
            inner.active.remove(&provider);
        }
        Ok(())
    }
    pub fn dispose(&self, id: &str) -> Result<(), String> {
        let mut inner = self.inner.lock().unwrap();
        let generation = inner.generations.get_mut(id).ok_or_else(|| not_found(id))?;
        if generation.sessions > 0 {
            return Err(format!(
                "provider generation {} still has {} session(s)",
                quote(id),
                generation.sessions
            ));
        }
        generation.state = GenerationState::Stopped;
        let provider = generation.provider.id.clone();
        if inner
            .active
            .get(&provider)
            .is_some_and(|active| active == id)
        {
            inner.active.remove(&provider);
        }
        Ok(())
    }
    pub fn drain(&self, context: &RunCtx, id: &str) -> Result<(), String> {
        let deadline = Instant::now() + self.policy.timeout;
        loop {
            let generation = self.get(id).ok_or_else(|| not_found(id))?;
            if generation.sessions == 0 {
                return self.dispose(id);
            }
            if let Some(error) = context.err() {
                return Err(error);
            }
            if Instant::now() >= deadline {
                if self.policy.cancel_on_timeout {
                    return self.dispose(id);
                }
                return Err(format!(
                    "provider generation {} drain timed out with {} session(s)",
                    quote(id),
                    generation.sessions
                ));
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
fn not_found(id: &str) -> String {
    format!("provider generation {} not found", quote(id))
}
fn expected(id: &str, state: &GenerationState, expected: &str) -> String {
    format!(
        "provider generation {} is {}, expected {}",
        quote(id),
        state.name(),
        expected
    )
}

pub struct Session {
    lifecycle: Lifecycle,
    id: String,
    closed: bool,
}
impl Session {
    pub fn generation_id(&self) -> &str {
        &self.id
    }
    pub fn close(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        let mut inner = self.lifecycle.inner.lock().unwrap();
        if let Some(generation) = inner.generations.get_mut(&self.id) {
            if generation.sessions > 0 {
                generation.sessions -= 1;
            }
        }
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    // Dedicated differential entry point, absent from the product CLI/MCP.
    #[test]
    #[ignore = "driven by the pinned Go/Rust provider lifecycle oracle"]
    fn lifecycle_fixture() {
        let input: Value = serde_json::from_slice(
            &std::fs::read(std::env::var("PARITY_PROVIDER_INPUT").unwrap()).unwrap(),
        )
        .unwrap();
        let policy = DrainPolicy {
            timeout: Duration::from_millis(input["timeoutMs"].as_u64().unwrap_or(20)),
            cancel_on_timeout: input["cancelOnTimeout"].as_bool().unwrap_or(false),
        };
        let lifecycle = Lifecycle::new(policy);
        let mut sessions = BTreeMap::new();
        let mut results = Vec::new();
        for step in input["steps"].as_array().unwrap() {
            let text = |field: &str| step[field].as_str().unwrap_or("");
            let result: Result<Value, String> = match text("action") {
                "create" => lifecycle
                    .create_candidate(
                        serde_json::from_value(step["provider"].clone()).unwrap(),
                        text("hash"),
                        text("endpoint"),
                        text("revision"),
                    )
                    .map(|g| json!(g)),
                "restore" => lifecycle
                    .restore(serde_json::from_value(step["generation"].clone()).unwrap())
                    .map(|_| Value::Null),
                "ready" => lifecycle.mark_ready(text("id")).map(|_| Value::Null),
                "activate" => lifecycle
                    .activate(text("id"))
                    .map(|(previous, active)| json!({"previous":previous,"active":active})),
                "rollback" => lifecycle
                    .rollback_activation(text("id"), text("previous"))
                    .map(|_| Value::Null),
                "refresh" => lifecycle
                    .refresh(text("id"), text("hash"), text("revision"))
                    .map(|g| json!(g)),
                "get" => Ok(json!(lifecycle.get(text("id")))),
                "active" => Ok(json!(lifecycle.active(text("providerId")))),
                "open" | "open-explicit" => {
                    let session = if text("action") == "open" {
                        lifecycle.open_session(text("providerId"))
                    } else {
                        lifecycle.open_session_for_generation(text("providerId"), text("id"))
                    };
                    session.map(|session| {
                        let id = session.generation_id().to_owned();
                        sessions.insert(text("session").to_owned(), session);
                        json!(id)
                    })
                }
                "close" => {
                    if let Some(session) = sessions.get_mut(text("session")) {
                        session.close();
                    }
                    Ok(Value::Null)
                }
                "fail" => lifecycle.fail(text("id")).map(|_| Value::Null),
                "dispose" => lifecycle.dispose(text("id")).map(|_| Value::Null),
                "drain" => {
                    let ctx = RunCtx::background();
                    let ctx = if step["cancelled"].as_bool().unwrap_or(false) {
                        let (child, handle) = ctx.with_cancel();
                        handle.cancel();
                        child
                    } else {
                        ctx
                    };
                    lifecycle.drain(&ctx, text("id")).map(|_| Value::Null)
                }
                other => panic!("unknown fixture action {other}"),
            };
            results.push(match result {
                Ok(value) => json!({"value":value}),
                Err(error) => json!({"error":error}),
            });
        }
        std::fs::write(
            std::env::var("PARITY_PROVIDER_OUTPUT").unwrap(),
            serde_json::to_vec_pretty(&results).unwrap(),
        )
        .unwrap();
    }
    #[test]
    fn session_drop_does_not_release_the_replacement_generation() {
        let lifecycle = Lifecycle::new(DrainPolicy::default());
        let provider = ProviderRef {
            id: "com.opute.example".into(),
            version: "1.0.0".into(),
        };
        let first = lifecycle
            .create_candidate(provider.clone(), "one", "http://localhost/one", "r1")
            .unwrap();
        lifecycle.mark_ready(&first.id).unwrap();
        lifecycle.activate(&first.id).unwrap();
        let held = lifecycle.open_session(&provider.id).unwrap();
        let next = lifecycle
            .create_candidate(provider.clone(), "two", "http://localhost/two", "r2")
            .unwrap();
        lifecycle.mark_ready(&next.id).unwrap();
        lifecycle.activate(&next.id).unwrap();
        drop(held);
        assert_eq!(lifecycle.get(&first.id).unwrap().sessions, 0);
        assert_eq!(lifecycle.active(&provider.id).unwrap().id, next.id);
        lifecycle.dispose(&first.id).unwrap();
    }

    #[test]
    fn drain_waits_for_the_session_owner_and_observes_its_release() {
        let lifecycle = Lifecycle::new(DrainPolicy {
            timeout: Duration::from_secs(1),
            cancel_on_timeout: true,
        });
        let provider = ProviderRef {
            id: "com.opute.example".into(),
            version: "1.0.0".into(),
        };
        let generation = lifecycle
            .create_candidate(provider.clone(), "one", "http://localhost/one", "r1")
            .unwrap();
        lifecycle.mark_ready(&generation.id).unwrap();
        lifecycle.activate(&generation.id).unwrap();
        let session = lifecycle.open_session(&provider.id).unwrap();
        let manager = lifecycle.clone();
        let id = generation.id.clone();
        let drain = std::thread::spawn(move || manager.drain(&RunCtx::background(), &id));
        // The live session owns the old generation until it is released.
        assert!(lifecycle.dispose(&generation.id).is_err());
        drop(session);
        drain.join().unwrap().unwrap();
        assert_eq!(lifecycle.get(&generation.id).unwrap().sessions, 0);
        assert_eq!(
            lifecycle.get(&generation.id).unwrap().state,
            GenerationState::Stopped
        );
    }
}
