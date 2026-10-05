//! The MCP Tasks registry (`internal/tasks.Registry`).
//!
//! Task handles live in memory for the life of the process, exactly as Go's
//! registry does. Go's `internal/hostmcp` mirrors each transition into the
//! durable operation store from the call site, not from inside
//! `internal/tasks` itself; the M5 persistence calls in `tools.rs` and
//! `transport.rs` follow that same separation.
//!
//! State machine:
//!
//! ```text
//! input_required --update(all inputs)--> working --complete--> completed
//!       |                                   |
//!       +-----------cancel------------------+--cancel--> cancelled
//!                                           +--fail----> failed
//! ```
//!
//! Terminal states are absorbing: a late `complete` after a cancel is a
//! no-op, and cancelling a terminal task is refused.

use serde_json::{json, Map, Value as J};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// `DefaultTTL` (one hour) in milliseconds.
pub const DEFAULT_TTL_MS: i64 = 3_600_000;
/// `PollIntervalMs`.
pub const POLL_INTERVAL_MS: i64 = 3000;

/// `tasks.ToolResult`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ToolResult {
    pub content: Option<Vec<J>>,
    pub structured_content: Option<J>,
    pub is_error: bool,
}

type Resume = Box<dyn FnOnce(Map<String, J>) + Send>;

struct Record {
    task_id: String,
    tool_name: String,
    description: String,
    /// Already redacted at creation time (Go's `redactTaskArgs`, before the
    /// record exists): the raw call arguments never reach this struct.
    tool_args: J,
    status: &'static str,
    status_message: String,
    created_at: String,
    last_updated_at: String,
    ttl_ms: i64,
    poll_interval_ms: i64,
    input_requests: Option<Map<String, J>>,
    input_request: Option<Map<String, J>>,
    tool_result: Option<ToolResult>,
    cancelled: Arc<AtomicBool>,
    resume: Option<Resume>,
}

/// A stable snapshot of a record, safe to hold after the lock is released.
/// `tool_name`/`description` are carried here (not just on `Record`) so the
/// M5 persistence layer can build a durable snapshot without re-locking the
/// registry: Go's `persistTask` reads them off the same `*tasks.Record` it
/// already has in hand.
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub task_id: String,
    pub tool_name: String,
    pub description: String,
    pub tool_args: J,
    pub status: &'static str,
    status_message: String,
    created_at: String,
    last_updated_at: String,
    ttl_ms: i64,
    poll_interval_ms: i64,
    input_requests: Option<Map<String, J>>,
    input_request: Option<Map<String, J>>,
    tool_result: Option<ToolResult>,
}

impl Record {
    fn snapshot(&self) -> Snapshot {
        Snapshot {
            task_id: self.task_id.clone(),
            tool_name: self.tool_name.clone(),
            description: self.description.clone(),
            tool_args: self.tool_args.clone(),
            status: self.status,
            status_message: self.status_message.clone(),
            created_at: self.created_at.clone(),
            last_updated_at: self.last_updated_at.clone(),
            ttl_ms: self.ttl_ms,
            poll_interval_ms: self.poll_interval_ms,
            input_requests: self.input_requests.clone(),
            input_request: self.input_request.clone(),
            tool_result: self.tool_result.clone(),
        }
    }

    fn touch(&mut self) {
        self.last_updated_at = crate::hostobs::rfc3339_now();
    }

    fn live(&self) -> bool {
        matches!(self.status, "working" | "input_required")
    }
}

/// `uuid.NewString()`: a random (version 4) UUID.
pub(crate) fn new_task_id() -> String {
    let mut b = [0u8; 16];
    getrandom::getrandom(&mut b).expect("random task id");
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h = hex::encode(b);
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

#[derive(Default)]
pub struct Registry {
    tasks: Mutex<HashMap<String, Record>>,
}

impl Registry {
    fn new_record(tool_name: &str, description: &str, tool_args: J) -> Record {
        let now = crate::hostobs::rfc3339_now();
        Record {
            task_id: new_task_id(),
            tool_name: tool_name.to_string(),
            description: description.to_string(),
            tool_args,
            status: "working",
            status_message: String::new(),
            created_at: now.clone(),
            last_updated_at: now,
            ttl_ms: DEFAULT_TTL_MS,
            poll_interval_ms: POLL_INTERVAL_MS,
            input_requests: None,
            input_request: None,
            tool_result: None,
            cancelled: Arc::new(AtomicBool::new(false)),
            resume: None,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Record>> {
        self.tasks.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// `CreateWithCancel`: a working task. The returned flag is the task's
    /// cancellation signal (Go's context cancel).
    pub fn create(
        &self,
        tool_name: &str,
        description: &str,
        tool_args: J,
    ) -> (Snapshot, Arc<AtomicBool>) {
        let rec = Self::new_record(tool_name, description, tool_args);
        let (snapshot, cancelled) = (rec.snapshot(), Arc::clone(&rec.cancelled));
        self.lock().insert(rec.task_id.clone(), rec);
        (snapshot, cancelled)
    }

    /// `CreateWithID`: restores a durable operation identity after a
    /// process restart (or dedups a concurrent request against the same
    /// idempotency key). A working in-memory record for `task_id` is
    /// reused as-is; otherwise a fresh working record is created under
    /// that exact id rather than a generated one, so a later `get(task_id)`
    /// -- using the plan run's own `run_id` as the task id -- finds it.
    pub fn create_with_id(
        &self,
        task_id: &str,
        tool_name: &str,
        description: &str,
        tool_args: J,
    ) -> (Snapshot, Arc<AtomicBool>) {
        let mut guard = self.lock();
        if let Some(existing) = guard.get(task_id) {
            if existing.status == "working" {
                return (existing.snapshot(), Arc::clone(&existing.cancelled));
            }
        }
        let mut rec = Self::new_record(tool_name, description, tool_args);
        rec.task_id = task_id.to_string();
        let (snapshot, cancelled) = (rec.snapshot(), Arc::clone(&rec.cancelled));
        guard.insert(task_id.to_string(), rec);
        (snapshot, cancelled)
    }

    /// `CreateWithInput`: a task parked until `update` supplies every
    /// requested response. `resume` receives the task id and the accepted
    /// responses.
    pub fn create_with_input(
        &self,
        tool_name: &str,
        description: &str,
        tool_args: J,
        input_requests: Map<String, J>,
        resume: impl FnOnce(&str, Map<String, J>) + Send + 'static,
    ) -> Snapshot {
        let mut rec = Self::new_record(tool_name, description, tool_args);
        rec.status = "input_required";
        rec.status_message = "The task requires input before it can continue.".into();
        rec.input_requests = Some(input_requests);
        let id = rec.task_id.clone();
        rec.resume = Some(Box::new(move |accepted| resume(&id, accepted)));
        let snapshot = rec.snapshot();
        self.lock().insert(rec.task_id.clone(), rec);
        snapshot
    }

    pub fn get(&self, task_id: &str) -> Option<Snapshot> {
        self.lock().get(task_id).map(Record::snapshot)
    }

    /// `RestoreSnapshot`: rebuilds an in-memory record from a durable
    /// `task_snapshots` row written by `persist_task`. A snapshot captured
    /// mid-flight (status `working`) crashed before reaching a terminal
    /// state, so it comes back `failed`, never silently resumed as still
    /// working and never invented as `completed` -- this is the invariant
    /// the M5 crash-injection corpus checks. Host-plan tasks additionally
    /// need their resume continuation reattached by the caller; that is
    /// M6's plan executor, not this registry.
    pub fn restore_snapshot(&self, snapshot: &J) -> bool {
        let Some(obj) = snapshot.as_object() else {
            return false;
        };
        let task_id = obj.get("taskId").and_then(J::as_str).unwrap_or_default();
        let tool_name = obj.get("toolName").and_then(J::as_str).unwrap_or_default();
        if task_id.is_empty() || tool_name.is_empty() {
            return false;
        }
        let description = obj
            .get("description")
            .and_then(J::as_str)
            .unwrap_or_default();
        let mut tasks = self.lock();
        if tasks.get(task_id).is_some_and(|r| r.status == "working") {
            return true;
        }
        let tool_args = obj.get("toolArgs").cloned().unwrap_or(J::Null);
        let mut rec = Self::new_record(tool_name, description, tool_args);
        rec.task_id = task_id.to_string();
        if let Some(created) = non_empty_str(obj, "createdAt") {
            rec.created_at = created.to_string();
        }
        if let Some(updated) = non_empty_str(obj, "lastUpdatedAt") {
            rec.last_updated_at = updated.to_string();
        }
        if let Some(ttl) = obj.get("ttlMs").and_then(J::as_i64) {
            rec.ttl_ms = ttl;
        }
        if let Some(poll) = obj.get("pollIntervalMs").and_then(J::as_i64) {
            rec.poll_interval_ms = poll;
        }
        rec.status_message = obj
            .get("statusMessage")
            .and_then(J::as_str)
            .unwrap_or_default()
            .to_string();
        match obj.get("status").and_then(J::as_str).unwrap_or_default() {
            "completed" => {
                rec.status = "completed";
                if let Some(result) = obj.get("result") {
                    rec.tool_result = Some(tool_result_from_json(result));
                }
            }
            "cancelled" => rec.status = "cancelled",
            "failed" => rec.status = "failed",
            "input_required" => {
                rec.status = "input_required";
                if let Some(J::Object(requests)) = obj.get("inputRequests") {
                    rec.input_requests = Some(requests.clone());
                }
                if let Some(J::Object(request)) = obj.get("inputRequest") {
                    rec.input_request = Some(request.clone());
                }
            }
            // Includes `working`: a task snapshotted mid-flight never
            // resumes after a restart, in Rust or in Go.
            _ => {
                rec.status = "failed";
                rec.status_message = "The Host Agent restarted before the task completed.".into();
            }
        }
        tasks.insert(rec.task_id.clone(), rec);
        true
    }

    /// `Complete`: only a working task completes.
    pub fn complete(&self, task_id: &str, result: ToolResult) {
        let mut tasks = self.lock();
        let Some(rec) = tasks.get_mut(task_id).filter(|r| r.status == "working") else {
            return;
        };
        rec.status = "completed";
        rec.status_message = "The operation completed successfully.".into();
        rec.touch();
        rec.tool_result = Some(result);
    }

    /// `Fail`: Go's path for an execution error. Every Rust capability
    /// reports failure as a tool result, so only tests reach it today.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn fail(&self, task_id: &str, message: &str) {
        let mut tasks = self.lock();
        let Some(rec) = tasks.get_mut(task_id).filter(|r| r.live()) else {
            return;
        };
        rec.status = "failed";
        rec.status_message = message.to_string();
        rec.touch();
        rec.tool_result = Some(error_tool_result(message));
    }

    /// `Cancel`: signals the task's work and records the terminal state.
    pub fn cancel(&self, task_id: &str) -> Option<Snapshot> {
        let mut tasks = self.lock();
        let rec = tasks.get_mut(task_id).filter(|r| r.live())?;
        rec.cancelled.store(true, Ordering::SeqCst);
        rec.status = "cancelled";
        rec.status_message = "The task was cancelled by request.".into();
        rec.touch();
        rec.tool_result = Some(error_tool_result(&rec.status_message));
        rec.resume = None;
        Some(rec.snapshot())
    }

    /// `Update`: `None` for an unknown task, `Some((snapshot, false))` when
    /// the task is not waiting for input. Responses for keys that are not
    /// outstanding are ignored; once none remain the task resumes, exactly
    /// once, outside the lock.
    pub fn update(&self, task_id: &str, responses: &Map<String, J>) -> Option<(Snapshot, bool)> {
        let mut tasks = self.lock();
        let rec = tasks.get_mut(task_id)?;
        if rec.status != "input_required" {
            return Some((rec.snapshot(), false));
        }
        let outstanding = rec.input_requests.get_or_insert_with(Map::new);
        let mut accepted = Map::new();
        for (key, value) in responses {
            if outstanding.remove(key).is_some() {
                accepted.insert(key.clone(), value.clone());
            }
        }
        if !outstanding.is_empty() {
            rec.touch();
            return Some((rec.snapshot(), true));
        }
        rec.status = "working";
        rec.status_message = "Input received; resuming the task.".into();
        rec.input_request = None;
        rec.touch();
        let resume = rec.resume.take();
        let snapshot = rec.snapshot();
        drop(tasks);
        if let Some(resume) = resume {
            resume(accepted);
        }
        Some((snapshot, true))
    }
}

fn non_empty_str<'a>(obj: &'a Map<String, J>, key: &str) -> Option<&'a str> {
    obj.get(key).and_then(J::as_str).filter(|s| !s.is_empty())
}

/// The inverse of `tools::tool_result_json`: Go's
/// `json.Unmarshal(encoded, &toolResult)` on a persisted `result` field.
fn tool_result_from_json(value: &J) -> ToolResult {
    ToolResult {
        content: value
            .get("content")
            .and_then(J::as_array)
            .filter(|a| !a.is_empty())
            .cloned(),
        structured_content: value
            .get("structuredContent")
            .filter(|v| **v != J::Null)
            .cloned(),
        is_error: value.get("isError").and_then(J::as_bool).unwrap_or(false),
    }
}

fn error_tool_result(message: &str) -> ToolResult {
    ToolResult {
        content: Some(vec![
            json!({"type": "text", "text": format!("Error: {message}")}),
        ]),
        structured_content: None,
        is_error: true,
    }
}

impl Snapshot {
    /// `ToGetTaskResult`.
    pub fn get_result(&self) -> J {
        let mut out = self.handle("complete");
        if !self.status_message.is_empty() {
            out.insert("statusMessage".into(), J::from(self.status_message.clone()));
        }
        if let Some(requests) = self.input_requests.as_ref().filter(|r| !r.is_empty()) {
            out.insert("inputRequests".into(), J::Object(requests.clone()));
        }
        if self.status == "input_required" {
            if let Some(request) = self.input_request.as_ref().filter(|r| !r.is_empty()) {
                out.insert("inputRequest".into(), J::Object(request.clone()));
            }
        }
        if self.status == "failed" {
            out.insert(
                "error".into(),
                json!({"code": -32603, "message": self.status_message}),
            );
        } else if let (true, Some(result)) = (self.status == "completed", &self.tool_result) {
            out.insert(
                "result".into(),
                json!({
                    "structuredContent": result.structured_content.clone().unwrap_or(J::Null),
                    "content": result.content.clone().map(J::Array).unwrap_or(J::Null),
                    "isError": result.is_error,
                }),
            );
        }
        J::Object(out)
    }

    /// `ToCreateTaskResult`: the flat task handle.
    pub fn create_result(&self) -> J {
        J::Object(self.handle("task"))
    }

    fn handle(&self, result_type: &str) -> Map<String, J> {
        let mut out = Map::new();
        out.insert("resultType".into(), J::from(result_type));
        out.insert("taskId".into(), J::from(self.task_id.clone()));
        out.insert("status".into(), J::from(self.status));
        out.insert("createdAt".into(), J::from(self.created_at.clone()));
        out.insert(
            "lastUpdatedAt".into(),
            J::from(self.last_updated_at.clone()),
        );
        out.insert("ttlMs".into(), J::from(self.ttl_ms));
        out.insert("pollIntervalMs".into(), J::from(self.poll_interval_ms));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn inputs() -> Map<String, J> {
        serde_json::from_value(json!({"response": {"type": "string", "prompt": "Name?"}})).unwrap()
    }

    #[test]
    fn ids_are_v4_uuids() {
        let id = new_task_id();
        assert_eq!(id.len(), 36);
        assert_eq!(&id[14..15], "4");
        assert!(matches!(&id[19..20], "8" | "9" | "a" | "b"));
        assert_ne!(id, new_task_id());
    }

    #[test]
    fn create_with_id_reuses_a_working_record_and_replaces_a_terminal_one() {
        let registry = Registry::default();
        let (first, _) =
            registry.create_with_id("run-1", "run_host_plan", "Executing...", json!({}));
        let (again, _) =
            registry.create_with_id("run-1", "run_host_plan", "Executing...", json!({}));
        assert_eq!(first.task_id, again.task_id);
        assert_eq!(
            first.created_at, again.created_at,
            "a working record is reused, not recreated"
        );

        registry.complete(&first.task_id, ToolResult::default());
        let (fresh, _) =
            registry.create_with_id("run-1", "run_host_plan", "Resuming...", json!({}));
        assert_eq!(fresh.task_id, "run-1");
        assert_eq!(registry.get("run-1").unwrap().status, "working");
    }

    #[test]
    fn input_round_trip_resumes_once() {
        let registry = Arc::new(Registry::default());
        let (tx, rx) = mpsc::channel();
        let r = Arc::clone(&registry);
        let rec = registry.create_with_input(
            "request_task_input",
            "Waiting...",
            json!({}),
            inputs(),
            move |id, accepted| {
                tx.send(accepted.clone()).unwrap();
                r.complete(
                    id,
                    ToolResult {
                        structured_content: Some(json!({"response": accepted["response"]})),
                        ..Default::default()
                    },
                );
            },
        );
        let got = rec.get_result();
        assert_eq!(got["status"], "input_required");
        assert_eq!(got["inputRequests"]["response"]["prompt"], "Name?");
        // Unknown keys are ignored and do not resume.
        let unknown: Map<String, J> = serde_json::from_value(json!({"other": 1})).unwrap();
        let (_, accepted) = registry.update(&rec.task_id, &unknown).unwrap();
        assert!(accepted);
        assert!(rx.try_recv().is_err());
        let answer: Map<String, J> = serde_json::from_value(json!({"response": "Ada"})).unwrap();
        registry.update(&rec.task_id, &answer).unwrap();
        assert_eq!(rx.try_recv().unwrap()["response"], "Ada");
        let done = registry.get(&rec.task_id).unwrap().get_result();
        assert_eq!(done["status"], "completed");
        assert_eq!(
            done["result"],
            json!({"structuredContent": {"response": "Ada"}, "content": null, "isError": false})
        );
        assert!(done.get("inputRequests").is_none());
        // A completed task accepts no input and cannot be cancelled.
        assert!(!registry.update(&rec.task_id, &answer).unwrap().1);
        assert!(registry.cancel(&rec.task_id).is_none());
        assert!(registry.update("missing", &answer).is_none());
    }

    #[test]
    fn cancel_is_terminal_and_signals_work() {
        let registry = Registry::default();
        let (rec, cancelled) = registry.create("test_tool", "Running test_tool...", json!({}));
        let snapshot = registry.cancel(&rec.task_id).unwrap();
        assert!(cancelled.load(Ordering::SeqCst));
        assert_eq!(snapshot.status, "cancelled");
        registry.complete(&rec.task_id, ToolResult::default());
        registry.fail(&rec.task_id, "late");
        let got = registry.get(&rec.task_id).unwrap().get_result();
        assert_eq!(got["status"], "cancelled");
        assert_eq!(got["statusMessage"], "The task was cancelled by request.");
        assert!(got.get("result").is_none());
        assert!(registry.cancel(&rec.task_id).is_none());
    }

    #[test]
    fn failure_projects_a_jsonrpc_error() {
        let registry = Registry::default();
        let (rec, _) = registry.create("test_tool", "Running test_tool...", json!({}));
        registry.fail(&rec.task_id, "boom");
        let got = registry.get(&rec.task_id).unwrap().get_result();
        assert_eq!(got["error"], json!({"code": -32603, "message": "boom"}));
        let handle = rec.create_result();
        assert_eq!(handle["resultType"], "task");
        assert_eq!(handle["ttlMs"], DEFAULT_TTL_MS);
        assert_eq!(handle["pollIntervalMs"], POLL_INTERVAL_MS);
    }

    /// Builds the same document `tools::persist_task` writes to
    /// `task_snapshots`, for testing `restore_snapshot` without a live
    /// `StateStore`.
    fn snapshot_doc(snapshot: &Snapshot) -> J {
        let mut doc = snapshot.get_result();
        if let J::Object(map) = &mut doc {
            map.insert("toolName".into(), J::from(snapshot.tool_name.clone()));
            map.insert("description".into(), J::from(snapshot.description.clone()));
            map.insert("toolArgs".into(), snapshot.tool_args.clone());
        }
        doc
    }

    #[test]
    fn restore_completed_task_preserves_result() {
        let source = Registry::default();
        let (rec, _) = source.create("probe_http_endpoint", "Running probe...", json!({}));
        source.complete(
            &rec.task_id,
            ToolResult {
                structured_content: Some(json!({"status": 200})),
                ..Default::default()
            },
        );
        let doc = snapshot_doc(&source.get(&rec.task_id).unwrap());

        let restored = Registry::default();
        assert!(restored.restore_snapshot(&doc));
        let got = restored.get(&rec.task_id).unwrap().get_result();
        assert_eq!(got["status"], "completed");
        assert_eq!(got["result"]["structuredContent"], json!({"status": 200}));
    }

    /// Mirrors Go's `TestRestoreSnapshot` crash-safety case: a task that was
    /// still `working` when its last snapshot was written crashed before
    /// reaching a terminal state, so it comes back `failed`, never resumed
    /// as `working` and never invented as `completed`.
    #[test]
    fn restore_working_task_comes_back_failed() {
        let source = Registry::default();
        let (rec, _) = source.create("run_host_plan", "Running a plan...", json!({}));
        let doc = snapshot_doc(&source.get(&rec.task_id).unwrap());
        assert_eq!(doc["status"], "working");

        let restored = Registry::default();
        assert!(restored.restore_snapshot(&doc));
        let got = restored.get(&rec.task_id).unwrap().get_result();
        assert_eq!(got["status"], "failed");
        assert_eq!(
            got["error"]["message"],
            "The Host Agent restarted before the task completed."
        );
    }

    #[test]
    fn restore_input_required_task_preserves_requests() {
        let source = Registry::default();
        let rec = source.create_with_input(
            "request_task_input",
            "Waiting for operator input...",
            json!({}),
            inputs(),
            |_, _| {},
        );
        let doc = snapshot_doc(&rec);

        let restored = Registry::default();
        assert!(restored.restore_snapshot(&doc));
        let got = restored.get(&rec.task_id).unwrap().get_result();
        assert_eq!(got["status"], "input_required");
        assert_eq!(got["inputRequests"]["response"]["prompt"], "Name?");
    }

    #[test]
    fn restore_rejects_a_snapshot_missing_identity() {
        let restored = Registry::default();
        assert!(!restored.restore_snapshot(&json!({"status": "completed"})));
    }
}
