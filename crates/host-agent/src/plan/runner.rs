//! Port of `internal/plan/runner.go`: the single plan executor. Every path
//! that can dispatch a tool call, apply a mutation, or move a plan forward
//! goes through `Runner::run` / `Runner::resume` -- this is the one
//! executor the M6 structural test pins down.

use super::assert::{evaluate_assertions, resolve_json_pointer};
use super::graph::{reverse_topological_nodes, topological_levels};
use super::interpolate::{interpolate_args, interpolate_value, resolve_reference, EvalContext};
use super::schema::{
    self, attempts_for_pub as attempts_for, node_by_id_pub as node_by_id, AssertionFailure,
    Capability, ContextEntry, Document, Node, NodeRunState, Recovery, Retry, RunState, Validation,
    WaitState, MAX_FAN_OUT, RUN_STATUS_EXPIRED, RUN_STATUS_WAITING, STATUS_APPLIED,
    STATUS_COMPENSATED, STATUS_COMPENSATION_FAILED, STATUS_EXPIRED, STATUS_FAILED, STATUS_PENDING,
    STATUS_SATISFIED, STATUS_SKIPPED, STATUS_UNKNOWN, STATUS_WAITING,
};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime};

/// Cooperative cancellation + deadline, standing in for Go's `context.Context`.
/// Checking `err()` walks the parent chain, so a child created via
/// `with_cancel`/`with_timeout` observes its ancestors' cancellation and
/// deadlines without sharing their flags -- cancelling a child never
/// cancels its parent, matching `context.WithCancel` semantics.
#[derive(Clone)]
pub struct RunCtx {
    own_cancelled: Arc<AtomicBool>,
    parent: Option<Arc<RunCtx>>,
    deadline: Option<Instant>,
    /// `resource.WithReservation`: the launcher's reservation a node
    /// dispatched under this context should inherit instead of admitting
    /// independently. Carried like cancellation -- set once near the root,
    /// read by walking up through `with_cancel`/`with_timeout` children.
    reservation: Option<Arc<crate::resource::Reservation>>,
}

#[derive(Clone)]
pub struct CancelHandle(Arc<AtomicBool>);

impl CancelHandle {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

impl RunCtx {
    /// Wraps an externally-owned cancellation flag (e.g. a task registry's
    /// `Arc<AtomicBool>`) so cancelling it through that other owner --
    /// `tasks/cancel`, say -- is observed here too, without this `RunCtx`
    /// needing its own `CancelHandle`.
    pub fn from_flag(flag: Arc<AtomicBool>) -> Self {
        Self {
            own_cancelled: flag,
            parent: None,
            deadline: None,
            reservation: None,
        }
    }

    pub fn background() -> Self {
        Self {
            own_cancelled: Arc::new(AtomicBool::new(false)),
            parent: None,
            deadline: None,
            reservation: None,
        }
    }

    /// `resource.WithReservation`: a copy of this context that carries
    /// `reservation` for every node dispatched under it (and under any
    /// `with_cancel`/`with_timeout` child), in place of the one it had.
    pub fn with_reservation(&self, reservation: crate::resource::Reservation) -> Self {
        let mut ctx = self.clone();
        ctx.reservation = Some(Arc::new(reservation));
        ctx
    }

    /// `resource.ReservationFromContext`.
    pub fn reservation(&self) -> Option<Arc<crate::resource::Reservation>> {
        self.reservation
            .clone()
            .or_else(|| self.parent.as_ref().and_then(|p| p.reservation()))
    }

    pub fn err(&self) -> Option<String> {
        if self.own_cancelled.load(Ordering::SeqCst) {
            return Some("context canceled".to_string());
        }
        if let Some(deadline) = self.deadline {
            if Instant::now() >= deadline {
                return Some("context deadline exceeded".to_string());
            }
        }
        self.parent.as_ref().and_then(|p| p.err())
    }

    pub fn with_cancel(&self) -> (Self, CancelHandle) {
        let flag = Arc::new(AtomicBool::new(false));
        let child = Self {
            own_cancelled: flag.clone(),
            parent: Some(Arc::new(self.clone())),
            deadline: None,
            reservation: None,
        };
        (child, CancelHandle(flag))
    }

    pub fn with_timeout(&self, duration: Duration) -> (Self, CancelHandle) {
        let flag = Arc::new(AtomicBool::new(false));
        let child = Self {
            own_cancelled: flag.clone(),
            parent: Some(Arc::new(self.clone())),
            deadline: Some(Instant::now() + duration),
            reservation: None,
        };
        (child, CancelHandle(flag))
    }

    /// Sleeps `duration` in short slices, checking cancellation between each
    /// so `sleepBackoff`/polling loops stay responsive to cancel-mid-wait
    /// the same way Go's `select { <-ctx.Done(): ... }` does.
    fn sleep(&self, duration: Duration) -> Result<(), String> {
        let deadline = Instant::now() + duration;
        loop {
            if let Some(e) = self.err() {
                return Err(e);
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(());
            }
            std::thread::sleep(remaining.min(Duration::from_millis(25)));
        }
    }
}

/// Result of a single tool dispatch, standing in for `*mcp.CallToolResult`.
#[derive(Debug, Clone, Default)]
pub struct DispatchResult {
    pub is_error: bool,
    pub structured_content: Option<Value>,
    pub text: String,
}

type DispatchFn = dyn Fn(&RunCtx, &str, &Map<String, Value>) -> Result<DispatchResult, String>
    + Send
    + Sync
    + 'static;
type SinkFn<'a> = dyn Fn(&RunState) -> Result<(), String> + Send + Sync + 'a;

pub struct Runner<'a> {
    pub capabilities: BTreeMap<String, Capability>,
    pub catalog_revision: String,
    pub host_agent_id: String,
    /// `Arc`, not `Box`, and `'static`: a cancelled dispatch is raced on its
    /// own detached thread (see `dispatch`), which needs to hold its own
    /// owned handle to the closure after this `Runner` may have moved on.
    pub dispatch: Arc<DispatchFn>,
    pub sink: Option<Box<SinkFn<'a>>>,
}

/// Internal error currency: `Wait` is the sentinel that unwinds up to `run`
/// without being treated as a failure, mirroring Go's `ErrWaitReached`
/// compared via `errors.Is` rather than a typed return value.
enum PlanError {
    Wait,
    Other(String),
}

impl From<String> for PlanError {
    fn from(value: String) -> Self {
        PlanError::Other(value)
    }
}

// The wait/resume path (`resume`, `validate_resume`, and the constants and
// helpers they use) is exercised by this module's own tests but not yet
// called from `plan_mcp.rs` -- the durable wait -> `tasks/input_required`
// escalation is a documented follow-up to the bare `run_host_plan` wiring.
#[allow(dead_code)]
pub const ERR_RESUME_CONFLICT: &str = "plan wait resume conflict";
#[allow(dead_code)]
pub const ERR_WAIT_EXPIRED: &str = "plan wait expired";

fn now_rfc3339() -> String {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    let nanos = now.subsec_nanos();
    let days = secs / 86400;
    let (y, m, d) = civil_from_days(days as i64);
    let hh = (secs % 86400) / 3600;
    let mm = (secs % 3600) / 60;
    let ss = secs % 60;
    format!("{y:04}-{m:02}-{d:02}T{hh:02}:{mm:02}:{ss:02}.{nanos:09}Z")
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn context_values(entries: &BTreeMap<String, ContextEntry>) -> Map<String, Value> {
    entries
        .iter()
        .map(|(name, entry)| (name.clone(), entry.value.clone().unwrap_or(Value::Null)))
        .collect()
}

fn evaluation_context(
    doc: &Document,
    outputs: &Map<String, Value>,
    contexts: &BTreeMap<String, ContextEntry>,
    item: Option<&Map<String, Value>>,
) -> EvalContext {
    EvalContext {
        variables: doc.variables.clone(),
        node_output: outputs.clone(),
        item: item.cloned().unwrap_or_default(),
        input: Map::new(),
        context: context_values(contexts),
    }
}

fn failed_node(mut state: NodeRunState, message: &str) -> NodeRunState {
    state.status = STATUS_FAILED.to_string();
    state.error = message.to_string();
    state.completed_at = now_rfc3339();
    state
}

fn blocked_by_dependency(node: &Node, states: &BTreeMap<String, NodeRunState>) -> bool {
    node.depends_on.iter().any(|dependency| {
        let status = states
            .get(dependency)
            .map(|s| s.status.as_str())
            .unwrap_or("");
        matches!(
            status,
            s if s == STATUS_FAILED
                || s == STATUS_UNKNOWN
                || s == STATUS_SKIPPED
                || s == STATUS_COMPENSATION_FAILED
                || s == STATUS_WAITING
                || s == STATUS_EXPIRED
        )
    })
}

fn dependency_applied(states: &BTreeMap<String, NodeRunState>, node: &Node) -> bool {
    node.depends_on
        .iter()
        .any(|dependency| states.get(dependency).map(|s| s.status.as_str()) == Some(STATUS_APPLIED))
}

fn resolve_source(source: &str, doc: &Document, state: &RunState) -> Result<Value, String> {
    if let Some(inner) = source.strip_prefix("${").and_then(|s| s.strip_suffix('}')) {
        return resolve_reference(
            inner,
            &evaluation_context(doc, &state.outputs, &state.context, None),
        );
    }
    Err("forEach source must be an interpolation reference".to_string())
}

struct Semaphore {
    permits: AtomicUsize,
    max: usize,
    lock: Mutex<()>,
    cond: Condvar,
}

impl Semaphore {
    fn new(max: usize) -> Self {
        Self {
            permits: AtomicUsize::new(0),
            max,
            lock: Mutex::new(()),
            cond: Condvar::new(),
        }
    }

    fn acquire(&self) {
        let mut guard = self.lock.lock().unwrap();
        while self.permits.load(Ordering::SeqCst) >= self.max {
            guard = self.cond.wait(guard).unwrap();
        }
        self.permits.fetch_add(1, Ordering::SeqCst);
        drop(guard);
    }

    fn release(&self) {
        self.permits.fetch_sub(1, Ordering::SeqCst);
        let _guard = self.lock.lock().unwrap();
        self.cond.notify_one();
    }
}

type ValidateOutcome = (Option<Value>, Option<AssertionFailure>);
type LevelResult = (usize, Node, RunState, Result<(), PlanError>);

/// The outputs/context maps a `forEach` item evaluates against, bundled so
/// per-item helpers stay under clippy's argument-count limit.
struct ForEachScope<'s> {
    outputs: &'s Map<String, Value>,
    contexts: &'s BTreeMap<String, ContextEntry>,
}

impl<'a> Runner<'a> {
    pub fn run(
        &self,
        ctx: &RunCtx,
        doc: &Document,
        mut state: RunState,
    ) -> (RunState, Result<(), String>) {
        if let Err(e) = schema::validate(doc, &self.capabilities, &self.catalog_revision) {
            return (state, Err(e));
        }
        if state.status == RUN_STATUS_WAITING
            && state
                .wait
                .as_ref()
                .map(|w| w.status == RUN_STATUS_WAITING)
                .unwrap_or(false)
        {
            return (state, Ok(()));
        }
        let levels = match topological_levels(doc) {
            Ok(levels) => levels,
            Err(e) => return (state, Err(e)),
        };
        state.plan_id = doc.plan_id.clone();
        state.generation = doc.generation;
        state.status = "running".to_string();
        if let Err(e) = self.emit(&state) {
            return (state, Err(e));
        }

        let mut max_passes = doc.converge.max_passes;
        if max_passes <= 0 {
            max_passes = doc.defaults.max_passes;
        }
        if max_passes <= 0 {
            max_passes = 1;
        }

        for pass in 1..=max_passes {
            for level in &levels {
                match self.run_level(ctx, doc, level, &mut state) {
                    Err(PlanError::Wait) => return (state, Ok(())),
                    Err(PlanError::Other(e)) => return self.abort_run(ctx, doc, state, e),
                    Ok(()) => {}
                }
            }
            match self.check_converged(ctx, doc, &levels, &mut state, pass < max_passes) {
                Ok(true) => {
                    state.status = "completed".to_string();
                    state.error.clear();
                    if let Err(e) = self.emit(&state) {
                        return (state, Err(e));
                    }
                    return (state, Ok(()));
                }
                Ok(false) => {}
                Err(e) => return self.abort_run(ctx, doc, state, e),
            }
        }

        let message = format!("plan did not converge after {max_passes} passes");
        if doc.converge.abort_on_exhaustion {
            return self.abort_run(ctx, doc, state, message);
        }
        state.status = "completed".to_string();
        state.error = message;
        if let Err(e) = self.emit(&state) {
            return (state, Err(e));
        }
        (state, Ok(()))
    }

    /// Consumes the currently persisted wait exactly once, applies the
    /// node's `contextDelta`s, then re-enters `run`. The caller is expected
    /// to have durably persisted the pre-call state; this does not retry on
    /// its own, matching Go's single-attempt `Resume`.
    #[allow(dead_code)]
    pub fn resume(
        &self,
        ctx: &RunCtx,
        doc: &Document,
        mut state: RunState,
        request: schema::ResumeRequest,
    ) -> (RunState, Result<(), String>) {
        if let Err(e) = schema::validate(doc, &self.capabilities, &self.catalog_revision) {
            return (state, Err(e));
        }
        if let Err(e) = validate_resume(doc, &state, &request) {
            if e == ERR_WAIT_EXPIRED {
                state.status = RUN_STATUS_EXPIRED.to_string();
                if let Some(wait) = state.wait.as_mut() {
                    wait.status = RUN_STATUS_EXPIRED.to_string();
                }
                if let Some(node) = state.nodes.get_mut(&request.wait_node_id) {
                    node.status = STATUS_EXPIRED.to_string();
                    node.error = e.clone();
                    node.completed_at = now_rfc3339();
                }
                state.error = e.clone();
                let _ = self.emit(&state);
            }
            return (state, Err(e));
        }

        let wait_node_id = state.wait.as_ref().unwrap().node_id.clone();
        let Some(node) = node_by_id(doc, &wait_node_id).cloned() else {
            return (
                state,
                Err(format!(
                    "{ERR_RESUME_CONFLICT}: wait node \"{wait_node_id}\" is not declared"
                )),
            );
        };
        let Some(wait_spec) = node.wait.clone() else {
            return (
                state,
                Err(format!(
                    "{ERR_RESUME_CONFLICT}: wait node \"{wait_node_id}\" is not declared"
                )),
            );
        };
        for delta in &wait_spec.context_delta {
            let context = EvalContext {
                variables: doc.variables.clone(),
                node_output: state.outputs.clone(),
                item: Map::new(),
                input: request.input.clone(),
                context: context_values(&state.context),
            };
            let value = match interpolate_value(&Value::String(delta.value.clone()), &context) {
                Ok(v) => v,
                Err(e) => return (state, Err(format!("context delta \"{}\": {e}", delta.name))),
            };
            if let Err(e) = schema::validate_json(&delta.schema, &value) {
                return (state, Err(format!("context delta \"{}\": {e}", delta.name)));
            }
            let entry = ContextEntry {
                name: delta.name.clone(),
                value: Some(value),
                schema: delta.schema.clone(),
                schema_revision: wait_spec.schema_revision.clone(),
                producer_node: node.id.clone(),
                source: request.source.clone(),
                secret: delta.secret,
                recorded_at: now_rfc3339(),
            };
            state.context.insert(delta.name.clone(), entry.clone());
            state.context_history.push(entry);
        }
        state.nodes.insert(
            node.id.clone(),
            NodeRunState {
                id: node.id.clone(),
                status: STATUS_SATISFIED.to_string(),
                completed_at: now_rfc3339(),
                ..Default::default()
            },
        );
        state.wait = None;
        state.status = "running".to_string();
        state.error.clear();
        if let Err(e) = self.emit(&state) {
            return (state, Err(e));
        }
        self.run(ctx, doc, state)
    }

    fn abort_run(
        &self,
        ctx: &RunCtx,
        doc: &Document,
        mut state: RunState,
        run_err: String,
    ) -> (RunState, Result<(), String>) {
        state.status = "failed".to_string();
        if ctx.err().is_some() {
            state.status = "unknown".to_string();
        }
        state.error = run_err.clone();
        let _ = self.emit(&state);
        let _ = self.compensate_applied(doc, &mut state);
        (state, Err(run_err))
    }

    fn check_converged(
        &self,
        ctx: &RunCtx,
        doc: &Document,
        levels: &[Vec<Node>],
        state: &mut RunState,
        recheck: bool,
    ) -> Result<bool, String> {
        if recheck {
            for level in levels {
                for node in level {
                    let current_status = state
                        .nodes
                        .get(&node.id)
                        .map(|n| n.status.clone())
                        .unwrap_or_default();
                    if current_status != STATUS_APPLIED
                        && current_status != STATUS_SATISFIED
                        && current_status != STATUS_SKIPPED
                    {
                        return Ok(false);
                    }
                    if node.validate.is_none() || current_status == STATUS_SKIPPED {
                        continue;
                    }
                    let (result, failure) = self.validate_node_ready_plain(
                        ctx,
                        doc,
                        node,
                        None,
                        &state.outputs,
                        &state.context,
                    )?;
                    if let Some(failure) = failure {
                        state.nodes.insert(
                            node.id.clone(),
                            NodeRunState {
                                id: node.id.clone(),
                                status: STATUS_PENDING.to_string(),
                                error: failure.message,
                                ..Default::default()
                            },
                        );
                        state.outputs.remove(&node.id);
                        let _ = self.emit(state);
                        return Ok(false);
                    }
                    let mut current = state.nodes.get(&node.id).cloned().unwrap_or_default();
                    current.status = STATUS_SATISFIED.to_string();
                    current.output = result.clone();
                    current.observed = result.clone();
                    state.nodes.insert(node.id.clone(), current);
                    if let Some(result) = result {
                        state.outputs.insert(node.id.clone(), result);
                    }
                }
            }
        }
        for node in &doc.nodes {
            let status = state
                .nodes
                .get(&node.id)
                .map(|n| n.status.as_str())
                .unwrap_or("");
            if status != STATUS_APPLIED && status != STATUS_SATISFIED && status != STATUS_SKIPPED {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn run_level(
        &self,
        ctx: &RunCtx,
        doc: &Document,
        level: &[Node],
        state: &mut RunState,
    ) -> Result<(), PlanError> {
        if level.iter().any(|n| n.wait.is_some()) {
            // A wait is a graph barrier: run this level's nodes serially so no
            // sibling races the durable pause or gets partially dispatched.
            for node in level {
                self.run_node_with_preconditions(ctx, doc, node, state)?;
            }
            return Ok(());
        }

        let max_concurrency = if doc.converge.max_concurrency <= 0 {
            level.len()
        } else {
            doc.converge.max_concurrency as usize
        };

        if max_concurrency <= 1 || level.len() <= 1 {
            for node in level {
                if let Err(e) = self.run_node_with_preconditions(ctx, doc, node, state) {
                    if node.continue_on_failure {
                        continue;
                    }
                    return Err(e);
                }
            }
            return Ok(());
        }

        let semaphore = Semaphore::new(max_concurrency);
        let (level_ctx, cancel) = ctx.with_cancel();
        let results: Mutex<Vec<LevelResult>> = Mutex::new(Vec::new());

        std::thread::scope(|scope| {
            for (index, node) in level.iter().enumerate() {
                let semaphore = &semaphore;
                let level_ctx = level_ctx.clone();
                let cancel = cancel.clone();
                let results = &results;
                let runner = self;
                let mut local = state.clone();
                let node = node.clone();
                scope.spawn(move || {
                    semaphore.acquire();
                    let outcome =
                        runner.run_node_with_preconditions(&level_ctx, doc, &node, &mut local);
                    if outcome.is_err() && !node.continue_on_failure {
                        cancel.cancel();
                    }
                    results.lock().unwrap().push((index, node, local, outcome));
                    semaphore.release();
                });
            }
        });

        let mut ordered = results.into_inner().unwrap();
        ordered.sort_by_key(|r| r.0);
        for (_, node, local, _) in &ordered {
            if let Some(node_state) = local.nodes.get(&node.id) {
                state.nodes.insert(node.id.clone(), node_state.clone());
            }
            if let Some(output) = local.outputs.get(&node.id) {
                state.outputs.insert(node.id.clone(), output.clone());
            }
            self.emit(state).map_err(PlanError::Other)?;
        }
        for (_, node, _, outcome) in ordered {
            if let Err(e) = outcome {
                if !node.continue_on_failure {
                    return Err(e);
                }
            }
        }
        Ok(())
    }

    fn run_node_with_preconditions(
        &self,
        ctx: &RunCtx,
        doc: &Document,
        node: &Node,
        state: &mut RunState,
    ) -> Result<(), PlanError> {
        if let Some(e) = ctx.err() {
            state.nodes.insert(
                node.id.clone(),
                NodeRunState {
                    id: node.id.clone(),
                    status: STATUS_UNKNOWN.to_string(),
                    error: e.clone(),
                    ..Default::default()
                },
            );
            return Err(PlanError::Other(e));
        }
        if blocked_by_dependency(node, &state.nodes) {
            state.nodes.insert(
                node.id.clone(),
                NodeRunState {
                    id: node.id.clone(),
                    status: STATUS_SKIPPED.to_string(),
                    error: "dependency did not complete".to_string(),
                    ..Default::default()
                },
            );
            if !node.continue_on_failure {
                return Err(PlanError::Other(format!(
                    "node {} dependency failed",
                    node.id
                )));
            }
            return Ok(());
        }
        if !node.when.is_empty() {
            let when_value = Value::Object(Map::from_iter([
                ("vars".to_string(), Value::Object(doc.variables.clone())),
                ("nodes".to_string(), Value::Object(state.outputs.clone())),
                (
                    "context".to_string(),
                    Value::Object(context_values(&state.context)),
                ),
            ]));
            let (failure, ok) = evaluate_assertions(&when_value, &node.when);
            if !ok {
                state.nodes.insert(
                    node.id.clone(),
                    NodeRunState {
                        id: node.id.clone(),
                        status: STATUS_SKIPPED.to_string(),
                        error: failure.map(|f| f.message).unwrap_or_default(),
                        ..Default::default()
                    },
                );
                return Ok(());
            }
        }
        let force_action = dependency_applied(&state.nodes, node);
        if let Some(previous) = state.nodes.get(&node.id).cloned() {
            if (previous.status == STATUS_APPLIED || previous.status == STATUS_SATISFIED)
                && !force_action
            {
                if node.validate.is_some() {
                    let (result, failure) = self.validate_node_ready(
                        ctx,
                        doc,
                        node,
                        None,
                        &state.outputs,
                        &state.context,
                    )?;
                    if failure.is_none() {
                        let mut updated = previous;
                        updated.status = STATUS_SATISFIED.to_string();
                        updated.output = result.clone();
                        updated.observed = result.clone();
                        state.nodes.insert(node.id.clone(), updated);
                        if let Some(result) = result {
                            state.outputs.insert(node.id.clone(), result);
                        }
                        return Ok(());
                    }
                    state.nodes.insert(
                        node.id.clone(),
                        NodeRunState {
                            id: node.id.clone(),
                            status: STATUS_PENDING.to_string(),
                            ..Default::default()
                        },
                    );
                    state.outputs.remove(&node.id);
                } else {
                    let mut updated = previous;
                    updated.status = STATUS_SATISFIED.to_string();
                    state.nodes.insert(node.id.clone(), updated);
                    return Ok(());
                }
            }
        }
        self.run_node(ctx, doc, node, state, force_action)
    }

    fn run_node(
        &self,
        ctx: &RunCtx,
        doc: &Document,
        node: &Node,
        state: &mut RunState,
        force_action: bool,
    ) -> Result<(), PlanError> {
        let mut timeout_ms = node.timeout_ms;
        if timeout_ms <= 0 {
            timeout_ms = doc.defaults.timeout_ms;
        }
        let (node_ctx, _cancel) = if timeout_ms > 0 {
            ctx.with_timeout(Duration::from_millis(timeout_ms as u64))
        } else {
            ctx.with_cancel()
        };

        let started = now_rfc3339();
        let mut node_state = NodeRunState {
            id: node.id.clone(),
            status: STATUS_PENDING.to_string(),
            started_at: started,
            ..Default::default()
        };
        state.nodes.insert(node.id.clone(), node_state.clone());
        self.emit(state).map_err(PlanError::Other)?;

        if let Err(e) = self.validate_target(doc, node, state) {
            state
                .nodes
                .insert(node.id.clone(), failed_node(node_state, &e));
            let _ = self.emit(state);
            return Err(PlanError::Other(e));
        }

        if let Some(wait_spec) = &node.wait {
            let mut wait_revision = 1i64;
            if let Some(existing) = &state.wait {
                if existing.node_id == node.id {
                    wait_revision = existing.wait_revision + 1;
                }
            }
            let mut expires_at = wait_spec.expires_at.clone();
            if expires_at.is_empty() && wait_spec.expires_in_ms > 0 {
                expires_at = rfc3339_add_millis(wait_spec.expires_in_ms);
            }
            let correlation_value = interpolate_value(
                &Value::Object(wait_spec.correlation.clone()),
                &evaluation_context(doc, &state.outputs, &state.context, None),
            )
            .map_err(|e| {
                state
                    .nodes
                    .insert(node.id.clone(), failed_node(node_state.clone(), &e));
                let _ = self.emit(state);
                PlanError::Other(e)
            })?;
            let correlation_map = match correlation_value {
                Value::Object(map) => map,
                Value::Null => Map::new(),
                _ => {
                    let e = "wait correlation must be an object".to_string();
                    state
                        .nodes
                        .insert(node.id.clone(), failed_node(node_state.clone(), &e));
                    let _ = self.emit(state);
                    return Err(PlanError::Other(e));
                }
            };
            state.wait = Some(WaitState {
                node_id: node.id.clone(),
                wait_id: format!("{}:{}:{wait_revision}", state.run_id, node.id),
                wait_revision,
                schema_revision: wait_spec.schema_revision.clone(),
                trigger: wait_spec.trigger.clone(),
                correlation: correlation_map,
                input_schema: wait_spec.input_schema.clone(),
                expires_at,
                status: RUN_STATUS_WAITING.to_string(),
            });
            node_state.status = STATUS_WAITING.to_string();
            state.nodes.insert(node.id.clone(), node_state);
            state.status = RUN_STATUS_WAITING.to_string();
            state.error.clear();
            self.emit(state).map_err(PlanError::Other)?;
            return Err(PlanError::Wait);
        }

        if node.for_each.is_some() {
            let result = self.run_for_each(&node_ctx, doc, node, state, force_action);
            if let Err(e) = &result {
                let mut current = state.nodes.get(&node.id).cloned().unwrap_or_default();
                current.id = node.id.clone();
                current.error = e.clone();
                if node_ctx.err().is_some() {
                    current.status = STATUS_UNKNOWN.to_string();
                } else {
                    current.status = STATUS_FAILED.to_string();
                    current.completed_at = now_rfc3339();
                }
                state.nodes.insert(node.id.clone(), current);
                let _ = self.emit(state);
            }
            return result.map_err(PlanError::Other);
        }

        if node.validate.is_some() && !force_action {
            // A failed preflight must yield to the node action immediately;
            // polling here would wait for a resource this node itself
            // creates. A validation-only node has no action and may poll.
            let preflight = if node.action.is_none() {
                self.validate_node_ready_plain(
                    &node_ctx,
                    doc,
                    node,
                    None,
                    &state.outputs,
                    &state.context,
                )
            } else {
                self.validate_node(&node_ctx, doc, node, None, &state.outputs, &state.context)
            };
            let (result, failure) = match preflight {
                Ok(outcome) => outcome,
                Err(e) => {
                    state
                        .nodes
                        .insert(node.id.clone(), failed_node(node_state, &e));
                    let _ = self.emit(state);
                    return Err(PlanError::Other(e));
                }
            };
            if failure.is_none() {
                node_state.status = STATUS_SATISFIED.to_string();
                node_state.output = result.clone();
                node_state.observed = result.clone();
                node_state.completed_at = now_rfc3339();
                state.nodes.insert(node.id.clone(), node_state);
                if let Some(result) = result {
                    state.outputs.insert(node.id.clone(), result);
                }
                return self.emit(state).map_err(PlanError::Other);
            }
        }

        let Some(action) = &node.action else {
            let e = format!("node \"{}\" has no action after validation failed", node.id);
            state
                .nodes
                .insert(node.id.clone(), failed_node(node_state, &e));
            let _ = self.emit(state);
            return Err(PlanError::Other(e));
        };

        let mut attempts = node.retry.max_attempts;
        if attempts <= 0 {
            attempts = doc.defaults.retry.max_attempts;
        }
        if attempts <= 0 {
            attempts = 1;
        }

        for attempt in 1..=attempts {
            node_state.attempts = attempt;
            state.nodes.insert(node.id.clone(), node_state.clone());
            self.emit(state).map_err(PlanError::Other)?;

            let args = interpolate_args(
                &action.args,
                &evaluation_context(doc, &state.outputs, &state.context, None),
            )
            .map_err(PlanError::Other)?;
            self.validate_args(&action.tool, &args)
                .map_err(PlanError::Other)?;
            match self.dispatch(&node_ctx, &action.tool, &args) {
                Err(dispatch_err) => {
                    if let Some(e) = node_ctx.err() {
                        node_state.status = STATUS_UNKNOWN.to_string();
                        node_state.error = e;
                        state.nodes.insert(node.id.clone(), node_state);
                        let _ = self.emit(state);
                        return Err(PlanError::Other(dispatch_err));
                    }
                    node_state.error = dispatch_err.clone();
                    if attempt < attempts {
                        if let Err(e) =
                            sleep_backoff(&node_ctx, &node.retry, &doc.defaults.retry, attempt)
                        {
                            if node_ctx.err().is_some() {
                                node_state.status = STATUS_UNKNOWN.to_string();
                                node_state.error = e.clone();
                                state.nodes.insert(node.id.clone(), node_state);
                                let _ = self.emit(state);
                            }
                            return Err(PlanError::Other(e));
                        }
                        continue;
                    }
                    state
                        .nodes
                        .insert(node.id.clone(), failed_node(node_state, &dispatch_err));
                    let _ = self.emit(state);
                    return Err(PlanError::Other(dispatch_err));
                }
                Ok(result) => {
                    let output = structured_content(&result);
                    node_state.output = Some(output.clone());
                    if let Err(e) = self.validate_output(&action.tool, &output) {
                        node_state.status = STATUS_FAILED.to_string();
                        node_state.error = e.clone();
                        state.nodes.insert(node.id.clone(), node_state);
                        let _ = self.emit(state);
                        return Err(PlanError::Other(e));
                    }
                    state.outputs.insert(node.id.clone(), output);
                    if node.validate.is_none() {
                        node_state.status = STATUS_APPLIED.to_string();
                        node_state.completed_at = now_rfc3339();
                        state.nodes.insert(node.id.clone(), node_state);
                        return self.emit(state).map_err(PlanError::Other);
                    }
                    let (validation_output, mut failure) = self.validate_node_ready(
                        &node_ctx,
                        doc,
                        node,
                        None,
                        &state.outputs,
                        &state.context,
                    )?;
                    if failure.is_none() {
                        node_state.observed = validation_output;
                        node_state.status = STATUS_APPLIED.to_string();
                        node_state.completed_at = now_rfc3339();
                        state.nodes.insert(node.id.clone(), node_state);
                        return self.emit(state).map_err(PlanError::Other);
                    }
                    if let Some(recover) = &node.recover {
                        if let Some(recovered) =
                            self.try_recover(&node_ctx, doc, node, recover, state, &mut node_state)?
                        {
                            return Ok(recovered);
                        }
                        // try_recover already recorded its own last error/failure into node_state.
                    } else if let Some(failure) = failure.take() {
                        node_state.observed = failure.observed;
                        node_state.expected = failure.expected;
                        node_state.error = failure.message;
                    }
                }
            }
            if attempt < attempts {
                if let Err(e) = sleep_backoff(&node_ctx, &node.retry, &doc.defaults.retry, attempt)
                {
                    if node_ctx.err().is_some() {
                        node_state.status = STATUS_UNKNOWN.to_string();
                        node_state.error = e.clone();
                        state.nodes.insert(node.id.clone(), node_state);
                        let _ = self.emit(state);
                    }
                    return Err(PlanError::Other(e));
                }
                continue;
            }
        }
        let e = format!("node \"{}\" did not reach readiness", node.id);
        state
            .nodes
            .insert(node.id.clone(), failed_node(node_state, &e));
        let _ = self.emit(state);
        Err(PlanError::Other(e))
    }

    /// Runs the recovery action's bounded attempts for a node whose action
    /// dispatched successfully but did not reach readiness. Returns
    /// `Ok(Some(()))` when recovery restored readiness (the caller should
    /// return immediately), `Ok(None)` to keep retrying the outer action
    /// loop, or `Err` on a fatal (non-retryable) error.
    fn try_recover(
        &self,
        node_ctx: &RunCtx,
        doc: &Document,
        node: &Node,
        recover: &Recovery,
        state: &mut RunState,
        node_state: &mut NodeRunState,
    ) -> Result<Option<()>, PlanError> {
        let mut recovery_attempts = recover.max_attempts;
        if recovery_attempts <= 0 {
            recovery_attempts = 1;
        }
        let mut last_error = String::new();
        let mut last_failure: Option<AssertionFailure> = None;
        for recovery_attempt in 1..=recovery_attempts {
            let args = interpolate_args(
                &recover.action.args,
                &evaluation_context(doc, &state.outputs, &state.context, None),
            )
            .map_err(PlanError::Other)?;
            self.validate_args(&recover.action.tool, &args)
                .map_err(PlanError::Other)?;
            match self.dispatch(node_ctx, &recover.action.tool, &args) {
                Err(e) => {
                    last_error = e.clone();
                    if let Some(ctx_err) = node_ctx.err() {
                        node_state.status = STATUS_UNKNOWN.to_string();
                        node_state.error = ctx_err;
                        state.nodes.insert(node.id.clone(), node_state.clone());
                        let _ = self.emit(state);
                        return Err(PlanError::Other(e));
                    }
                    if recovery_attempt < recovery_attempts {
                        sleep_backoff(
                            node_ctx,
                            &Retry::default(),
                            &doc.defaults.retry,
                            recovery_attempt,
                        )
                        .map_err(PlanError::Other)?;
                    }
                    continue;
                }
                Ok(_) => {
                    let (observed, failure) = self.validate_node_ready(
                        node_ctx,
                        doc,
                        node,
                        None,
                        &state.outputs,
                        &state.context,
                    )?;
                    if failure.is_none() {
                        node_state.observed = observed;
                        node_state.status = STATUS_APPLIED.to_string();
                        node_state.completed_at = now_rfc3339();
                        state.nodes.insert(node.id.clone(), node_state.clone());
                        self.emit(state).map_err(PlanError::Other)?;
                        return Ok(Some(()));
                    }
                    last_failure = failure;
                }
            }
        }
        if let Some(failure) = last_failure {
            node_state.observed = failure.observed;
            node_state.expected = failure.expected;
            node_state.error = failure.message;
        } else if !last_error.is_empty() {
            node_state.error = last_error;
        }
        Ok(None)
    }

    fn run_for_each(
        &self,
        ctx: &RunCtx,
        doc: &Document,
        node: &Node,
        state: &mut RunState,
        force_action: bool,
    ) -> Result<(), String> {
        let for_each = node.for_each.as_ref().unwrap();
        let mut value = resolve_source(&for_each.source, doc, state)?;
        if !for_each.path.is_empty() {
            let (resolved, exists) = resolve_json_pointer(&value, &for_each.path)?;
            if !exists {
                return Err(format!(
                    "forEach source path \"{}\" was not found",
                    for_each.path
                ));
            }
            value = resolved.unwrap_or(Value::Null);
        }
        let Value::Array(items) = value else {
            return Err(format!(
                "forEach source must be an array with at most {MAX_FAN_OUT} items"
            ));
        };
        if items.len() > MAX_FAN_OUT {
            return Err(format!(
                "forEach source must be an array with at most {MAX_FAN_OUT} items"
            ));
        }
        let mut outputs = Vec::with_capacity(items.len());
        for item in items {
            let item_map = match &item {
                Value::Object(map) => {
                    let mut cloned = map.clone();
                    if !for_each.r#as.is_empty() {
                        cloned.insert(for_each.r#as.clone(), item.clone());
                    }
                    cloned
                }
                _ => {
                    let mut m = Map::new();
                    m.insert(for_each.r#as.clone(), item.clone());
                    m
                }
            };
            if !for_each.filter.is_empty() {
                let (_, matches) = evaluate_assertions(&item, &for_each.filter);
                if !matches {
                    continue;
                }
            }
            if node.action.is_none() {
                return Err(format!("forEach node \"{}\" requires an action", node.id));
            }
            let scope = ForEachScope {
                outputs: &state.outputs,
                contexts: &state.context,
            };
            let item_output =
                self.run_for_each_item(ctx, doc, node, &item_map, &scope, force_action)?;
            outputs.push(item_output);
        }
        state
            .outputs
            .insert(node.id.clone(), Value::Array(outputs.clone()));
        state.nodes.insert(
            node.id.clone(),
            NodeRunState {
                id: node.id.clone(),
                status: STATUS_APPLIED.to_string(),
                output: Some(Value::Array(outputs)),
                completed_at: now_rfc3339(),
                ..Default::default()
            },
        );
        self.emit(state)
    }

    // `last_err` is reassigned on every path (preflight, dispatch, post-
    // validate) exactly like Go's `lastErr` chain; some assignments are
    // superseded within the same attempt before being read, which is
    // intentional -- the loop keeps only the most recent failure reason.
    #[allow(unused_assignments)]
    fn run_for_each_item(
        &self,
        ctx: &RunCtx,
        doc: &Document,
        node: &Node,
        item: &Map<String, Value>,
        scope: &ForEachScope<'_>,
        force_action: bool,
    ) -> Result<Value, String> {
        let attempts = attempts_for(node, doc);
        let mut last_err = String::new();
        for attempt in 1..=attempts {
            if let Some(e) = ctx.err() {
                return Err(e);
            }
            if node.validate.is_some() && !force_action {
                match self.validate_node_ready_plain(
                    ctx,
                    doc,
                    node,
                    Some(item),
                    scope.outputs,
                    scope.contexts,
                ) {
                    Ok((validated, None)) => return Ok(validated.unwrap_or(Value::Null)),
                    Ok((_, Some(failure))) => {
                        last_err = format!(
                            "forEach node \"{}\" did not reach readiness: {}",
                            node.id, failure.message
                        )
                    }
                    Err(e) => last_err = e,
                }
            }
            let action = node.action.as_ref().unwrap();
            let args = interpolate_args(
                &action.args,
                &evaluation_context(doc, scope.outputs, scope.contexts, Some(item)),
            )?;
            self.validate_args(&action.tool, &args)?;
            match self.dispatch(ctx, &action.tool, &args) {
                Err(e) => last_err = e,
                Ok(result) => {
                    let item_output = structured_content(&result);
                    self.validate_output(&action.tool, &item_output)?;
                    if node.validate.is_none() {
                        return Ok(item_output);
                    }
                    match self.validate_node_ready_plain(
                        ctx,
                        doc,
                        node,
                        Some(item),
                        scope.outputs,
                        scope.contexts,
                    ) {
                        Ok((_, None)) => return Ok(item_output),
                        Ok((_, Some(failure))) => {
                            last_err = format!(
                                "forEach node \"{}\" did not reach readiness: {}",
                                node.id, failure.message
                            );
                        }
                        Err(e) => last_err = e,
                    }
                    if node.recover.is_some() {
                        match self.recover_for_each_item(ctx, doc, node, item, scope) {
                            Ok(true) => return Ok(item_output),
                            Ok(false) => {}
                            Err(e) => last_err = e,
                        }
                    }
                }
            }
            if attempt < attempts {
                sleep_backoff(ctx, &node.retry, &doc.defaults.retry, attempt)?;
            }
        }
        if last_err.is_empty() {
            last_err = format!("forEach node \"{}\" did not complete", node.id);
        }
        Err(last_err)
    }

    fn recover_for_each_item(
        &self,
        ctx: &RunCtx,
        doc: &Document,
        node: &Node,
        item: &Map<String, Value>,
        scope: &ForEachScope<'_>,
    ) -> Result<bool, String> {
        let recover = node.recover.as_ref().unwrap();
        let mut recovery_attempts = recover.max_attempts;
        if recovery_attempts <= 0 {
            recovery_attempts = 1;
        }
        let mut last_err = String::new();
        for attempt in 1..=recovery_attempts {
            let args = interpolate_args(
                &recover.action.args,
                &evaluation_context(doc, scope.outputs, scope.contexts, Some(item)),
            )?;
            self.validate_args(&recover.action.tool, &args)?;
            match self.dispatch(ctx, &recover.action.tool, &args) {
                Ok(result) => {
                    self.validate_output(&recover.action.tool, &structured_content(&result))?;
                    match self.validate_node_ready_plain(
                        ctx,
                        doc,
                        node,
                        Some(item),
                        scope.outputs,
                        scope.contexts,
                    ) {
                        Ok((_, None)) => return Ok(true),
                        Ok((_, Some(failure))) => {
                            last_err = format!(
                                "forEach node \"{}\" did not reach readiness: {}",
                                node.id, failure.message
                            )
                        }
                        Err(e) => last_err = e,
                    }
                }
                Err(e) => last_err = e,
            }
            if attempt < recovery_attempts {
                sleep_backoff(ctx, &node.retry, &doc.defaults.retry, attempt)?;
            }
        }
        if last_err.is_empty() {
            Ok(false)
        } else {
            Err(last_err)
        }
    }

    fn validate_node(
        &self,
        ctx: &RunCtx,
        doc: &Document,
        node: &Node,
        item: Option<&Map<String, Value>>,
        outputs: &Map<String, Value>,
        contexts: &BTreeMap<String, ContextEntry>,
    ) -> Result<ValidateOutcome, String> {
        let Some(validation) = &node.validate else {
            return Ok((None, None));
        };
        self.dispatch_validation(
            ctx,
            validation,
            &evaluation_context(doc, outputs, contexts, item),
        )
    }

    fn dispatch_validation(
        &self,
        ctx: &RunCtx,
        validation: &Validation,
        context: &EvalContext,
    ) -> Result<ValidateOutcome, String> {
        let args = interpolate_args(&validation.args, context)?;
        self.validate_args(&validation.tool, &args)?;
        let result = self.dispatch(ctx, &validation.tool, &args)?;
        let content = structured_content(&result);
        self.validate_output(&validation.tool, &content)?;
        let (failure, ok) = evaluate_assertions(&content, &validation.assert);
        if !ok {
            return Ok((Some(content), failure));
        }
        Ok((Some(content), None))
    }

    /// One bounded readiness check, polling only when the document
    /// explicitly supplies a validation timeout. A failed preflight with no
    /// timeout returns immediately so a mutation can be applied; a node with
    /// a timeout gets honest, bounded readiness polling.
    fn validate_node_ready(
        &self,
        ctx: &RunCtx,
        doc: &Document,
        node: &Node,
        item: Option<&Map<String, Value>>,
        outputs: &Map<String, Value>,
        contexts: &BTreeMap<String, ContextEntry>,
    ) -> Result<ValidateOutcome, PlanError> {
        self.validate_node_ready_plain(ctx, doc, node, item, outputs, contexts)
            .map_err(PlanError::Other)
    }

    fn validate_node_ready_plain(
        &self,
        ctx: &RunCtx,
        doc: &Document,
        node: &Node,
        item: Option<&Map<String, Value>>,
        outputs: &Map<String, Value>,
        contexts: &BTreeMap<String, ContextEntry>,
    ) -> Result<ValidateOutcome, String> {
        let Some(validation) = &node.validate else {
            return Ok((None, None));
        };
        let (mut result, mut failure) =
            self.validate_node(ctx, doc, node, item, outputs, contexts)?;
        if failure.is_none() || validation.timeout_ms <= 0 {
            return Ok((result, failure));
        }
        let (poll_ctx, _cancel) =
            ctx.with_timeout(Duration::from_millis(validation.timeout_ms as u64));
        let interval = if validation.poll_interval_ms > 0 {
            validation.poll_interval_ms
        } else {
            250
        };
        loop {
            if poll_ctx.err().is_some() {
                return Ok((result, failure));
            }
            if poll_ctx
                .sleep(Duration::from_millis(interval as u64))
                .is_err()
            {
                return Ok((result, failure));
            }
            let (next_result, next_failure) =
                self.validate_node(&poll_ctx, doc, node, item, outputs, contexts)?;
            result = next_result;
            failure = next_failure;
            if failure.is_none() {
                return Ok((result, failure));
            }
        }
    }

    /// Mirrors Go's `context.Context` propagation into a blocking call (for
    /// example `http.NewRequestWithContext`): the underlying tool here has
    /// no cancellation hook of its own, so the call runs on a detached
    /// thread and this races it against `ctx`. A cancellation during the
    /// call returns immediately with `ctx.err()`, matching Go's prompt
    /// "context canceled"; the abandoned thread's eventual result is
    /// dropped. A call that finishes first returns its own result and never
    /// touches `ctx` again, so the non-cancelled path is unchanged other
    /// than polling instead of calling straight through.
    fn dispatch(
        &self,
        ctx: &RunCtx,
        name: &str,
        args: &Map<String, Value>,
    ) -> Result<DispatchResult, String> {
        let dispatch_fn = Arc::clone(&self.dispatch);
        let call_ctx = ctx.clone();
        let call_name = name.to_string();
        let call_args = args.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(dispatch_fn(&call_ctx, &call_name, &call_args));
        });
        let result = loop {
            match rx.recv_timeout(Duration::from_millis(20)) {
                Ok(result) => break result,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    if let Some(e) = ctx.err() {
                        return Err(e);
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(format!(
                        "tool {name} dispatch thread exited without a result"
                    ));
                }
            }
        }?;
        if result.is_error {
            return Err(format!("tool {name} returned an error: {}", result.text));
        }
        Ok(result)
    }

    fn validate_args(&self, name: &str, args: &Map<String, Value>) -> Result<(), String> {
        let capability = self
            .capabilities
            .get(name)
            .ok_or_else(|| format!("unknown plan capability \"{name}\""))?;
        schema::validate_json(&capability.input_schema, &Value::Object(args.clone()))
            .map_err(|e| format!("capability {name} arguments: {e}"))
    }

    fn validate_output(&self, name: &str, value: &Value) -> Result<(), String> {
        let Some(capability) = self.capabilities.get(name) else {
            return Ok(());
        };
        if capability.output_schema.is_empty() {
            return Ok(());
        }
        schema::validate_json(&capability.output_schema, value)
            .map_err(|e| format!("capability {name} result: {e}"))
    }

    fn validate_target(&self, doc: &Document, node: &Node, state: &RunState) -> Result<(), String> {
        let Some(target) = &node.target else {
            return Ok(());
        };
        if self.host_agent_id.trim().is_empty() {
            return Ok(());
        }
        let value = interpolate_value(
            &Value::String(target.host_ref.clone()),
            &evaluation_context(doc, &state.outputs, &state.context, None),
        )
        .map_err(|e| format!("target hostRef: {e}"))?;
        let Value::String(host_ref) = value else {
            return Err("target hostRef must resolve to a non-empty string".to_string());
        };
        if host_ref.trim().is_empty() {
            return Err("target hostRef must resolve to a non-empty string".to_string());
        }
        if host_ref != self.host_agent_id {
            return Err(format!(
                "target hostRef \"{host_ref}\" does not match Host Agent \"{}\"",
                self.host_agent_id
            ));
        }
        Ok(())
    }

    fn compensate(&self, node: &Node, doc: &Document, state: &mut RunState) -> Result<(), String> {
        let Some(compensate) = &node.compensate else {
            return Ok(());
        };
        let args = interpolate_args(
            &compensate.args,
            &evaluation_context(doc, &state.outputs, &state.context, None),
        )?;
        let ctx = RunCtx::background();
        let (ctx, _cancel) = ctx.with_timeout(Duration::from_secs(120));
        if let Err(e) = self.validate_args(&compensate.tool, &args) {
            state.nodes.insert(
                node.id.clone(),
                NodeRunState {
                    id: node.id.clone(),
                    status: STATUS_COMPENSATION_FAILED.to_string(),
                    error: e.clone(),
                    ..Default::default()
                },
            );
            let _ = self.emit(state);
            return Err(e);
        }
        if let Err(e) = self.dispatch(&ctx, &compensate.tool, &args) {
            let mut current = state.nodes.get(&node.id).cloned().unwrap_or_default();
            current.id = node.id.clone();
            current.status = STATUS_COMPENSATION_FAILED.to_string();
            current.error = e.clone();
            state.nodes.insert(node.id.clone(), current);
            let _ = self.emit(state);
            return Err(e);
        }
        let mut current = state.nodes.get(&node.id).cloned().unwrap_or_default();
        current.id = node.id.clone();
        current.status = STATUS_COMPENSATED.to_string();
        current.completed_at = now_rfc3339();
        state.nodes.insert(node.id.clone(), current);
        self.emit(state)
    }

    fn compensate_applied(&self, doc: &Document, state: &mut RunState) -> Result<(), String> {
        let ordered = reverse_topological_nodes(doc)?;
        let mut first_err = Ok(());
        for node in &ordered {
            let status = state
                .nodes
                .get(&node.id)
                .map(|n| n.status.clone())
                .unwrap_or_default();
            if status != STATUS_APPLIED || node.compensate.is_none() {
                continue;
            }
            if let Err(e) = self.compensate(node, doc, state) {
                if first_err.is_ok() {
                    first_err = Err(e);
                }
            }
        }
        first_err
    }

    fn emit(&self, state: &RunState) -> Result<(), String> {
        let Some(sink) = &self.sink else {
            return Ok(());
        };
        sink(&redacted_run_state(state))
    }
}

/// The pure fence check shared by the runner and the MCP task adapter
/// before accepting an operator/event input against a persisted wait.
// Called only from `resume()` above and this module's tests; the same
// deferred-wiring note applies.
#[allow(dead_code)]
pub fn validate_resume(
    doc: &Document,
    state: &RunState,
    request: &schema::ResumeRequest,
) -> Result<(), String> {
    let Some(wait) = &state.wait else {
        return Err(format!("{ERR_RESUME_CONFLICT}: run is not waiting"));
    };
    if state.status != RUN_STATUS_WAITING || wait.status != RUN_STATUS_WAITING {
        return Err(format!("{ERR_RESUME_CONFLICT}: run is not waiting"));
    }
    if request.wait_node_id != wait.node_id
        || request.wait_revision != wait.wait_revision
        || request.schema_revision != wait.schema_revision
    {
        return Err(format!(
            "{ERR_RESUME_CONFLICT}: wait revision, node, or schema does not match"
        ));
    }
    if Value::Object(request.correlation.clone()) != Value::Object(wait.correlation.clone()) {
        return Err(format!(
            "{ERR_RESUME_CONFLICT}: wait correlation does not match"
        ));
    }
    if request.source.trim().is_empty() {
        return Err(format!("{ERR_RESUME_CONFLICT}: resume source is required"));
    }
    if request.source != "operator" && request.source != "authenticated-event" {
        return Err(format!(
            "{ERR_RESUME_CONFLICT}: unsupported resume source \"{}\"",
            request.source
        ));
    }
    let Some(node) = node_by_id(doc, &wait.node_id) else {
        return Err(format!(
            "{ERR_RESUME_CONFLICT}: wait node \"{}\" is not declared",
            wait.node_id
        ));
    };
    let Some(wait_spec) = &node.wait else {
        return Err(format!(
            "{ERR_RESUME_CONFLICT}: wait node \"{}\" is not declared",
            wait.node_id
        ));
    };
    if wait.trigger.kind == "operator" && request.source != "operator" {
        return Err(format!(
            "{ERR_RESUME_CONFLICT}: this wait accepts operator input only"
        ));
    }
    for delta in &wait_spec.context_delta {
        if delta.provenance == "operator" && request.source != "operator" {
            return Err(format!(
                "{ERR_RESUME_CONFLICT}: context \"{}\" accepts operator input only",
                delta.name
            ));
        }
        if delta.provenance == "authenticated-event" && request.source != "authenticated-event" {
            return Err(format!(
                "{ERR_RESUME_CONFLICT}: context \"{}\" requires an authenticated event",
                delta.name
            ));
        }
    }
    if !wait.expires_at.is_empty() {
        match parse_rfc3339_millis(&wait.expires_at) {
            Some(expires_at_ms) if now_millis() < expires_at_ms => {}
            _ => return Err(ERR_WAIT_EXPIRED.to_string()),
        }
    }
    schema::validate_json(&wait.input_schema, &Value::Object(request.input.clone()))
        .map_err(|e| format!("{ERR_RESUME_CONFLICT}: input: {e}"))?;
    Ok(())
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// A conservative RFC3339 millisecond parser covering the `expiresAt`
/// values this runner itself produces (always UTC `Z`-suffixed, with
/// fractional seconds), enough to compare against "now" without pulling in
/// a datetime crate for this one comparison.
#[allow(dead_code)]
fn parse_rfc3339_millis(value: &str) -> Option<i64> {
    let bytes = value.as_bytes();
    if bytes.len() < 20 || bytes[4] != b'-' || bytes[7] != b'-' || bytes[10] != b'T' {
        return None;
    }
    let year: i64 = value.get(0..4)?.parse().ok()?;
    let month: i64 = value.get(5..7)?.parse().ok()?;
    let day: i64 = value.get(8..10)?.parse().ok()?;
    let hour: i64 = value.get(11..13)?.parse().ok()?;
    let minute: i64 = value.get(14..16)?.parse().ok()?;
    let second: i64 = value.get(17..19)?.parse().ok()?;
    let days = days_from_civil(year, month, day);
    let mut millis = (days * 86400 + hour * 3600 + minute * 60 + second) * 1000;
    if let Some(dot) = value.find('.') {
        let end = value[dot + 1..]
            .find(|c: char| !c.is_ascii_digit())
            .map(|i| dot + 1 + i)
            .unwrap_or(value.len());
        let frac = &value[dot + 1..end];
        let frac_ms: i64 = format!("{frac:0<3}")[..3].parse().ok()?;
        millis += frac_ms;
    }
    Some(millis)
}

#[allow(dead_code)]
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64;
    let mp = if m > 2 { m - 3 } else { m + 9 } as u64;
    let doy = (153 * mp + 2) / 5 + d as u64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe as i64 - 719468
}

fn rfc3339_add_millis(ms: i64) -> String {
    let now = now_millis() + ms;
    let secs = now.div_euclid(1000);
    let frac_ms = now.rem_euclid(1000);
    let days = secs.div_euclid(86400);
    let (y, mo, d) = civil_from_days(days);
    let rem = secs.rem_euclid(86400);
    format!(
        "{y:04}-{mo:02}-{d:02}T{:02}:{:02}:{:02}.{frac_ms:03}000000Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

fn sleep_backoff(ctx: &RunCtx, node: &Retry, defaults: &Retry, attempt: i64) -> Result<(), String> {
    let mut backoff = node.backoff_ms;
    if backoff <= 0 {
        backoff = defaults.backoff_ms;
    }
    if backoff <= 0 {
        return Ok(());
    }
    let mut factor = node.backoff_factor;
    if factor <= 0 {
        factor = defaults.backoff_factor;
    }
    if factor <= 0 {
        factor = 2;
    }
    let mut delay = Duration::from_millis(backoff as u64);
    let max_delay = Duration::from_secs(5 * 60);
    for _ in 1..attempt {
        delay = delay.saturating_mul(factor as u32);
        if delay > max_delay {
            delay = max_delay;
            break;
        }
    }
    ctx.sleep(delay)
}

fn structured_content(result: &DispatchResult) -> Value {
    result
        .structured_content
        .clone()
        .unwrap_or_else(|| Value::Object(Map::new()))
}

fn redacted_run_state(state: &RunState) -> RunState {
    let mut redacted = state.clone();
    for entry in redacted.context.values_mut() {
        if entry.secret {
            entry.value = None;
        }
    }
    for entry in redacted.context_history.iter_mut() {
        if entry.secret {
            entry.value = None;
        }
    }
    redacted
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::schema::{Action, Assertion, Validation};
    use std::sync::atomic::AtomicI64;

    fn check_capability() -> Capability {
        Capability {
            name: "check".to_string(),
            input_schema: Map::new(),
            output_schema: Map::new(),
            effect: "read".to_string(),
            idempotent: true,
        }
    }

    fn mutating_capability() -> Capability {
        Capability {
            name: "mutate".to_string(),
            input_schema: Map::new(),
            output_schema: Map::new(),
            effect: "create".to_string(),
            idempotent: true,
        }
    }

    fn single_action_doc() -> Document {
        Document {
            contract_version: schema::CONTRACT_VERSION.to_string(),
            plan_id: "p1".to_string(),
            idempotency_key: "k1".to_string(),
            generation: 1,
            nodes: vec![Node {
                id: "n1".to_string(),
                action: Some(Action {
                    tool: "mutate".to_string(),
                    args: Map::new(),
                }),
                validate: Some(Validation {
                    tool: "check".to_string(),
                    ..Default::default()
                }),
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    fn runner_with_dispatch<'a, F>(f: F) -> Runner<'a>
    where
        F: Fn(&RunCtx, &str, &Map<String, Value>) -> Result<DispatchResult, String>
            + Send
            + Sync
            + 'static,
    {
        let mut capabilities = BTreeMap::new();
        capabilities.insert("check".to_string(), check_capability());
        capabilities.insert("mutate".to_string(), mutating_capability());
        Runner {
            capabilities,
            catalog_revision: String::new(),
            host_agent_id: String::new(),
            dispatch: Arc::new(f),
            sink: None,
        }
    }

    /// `single_action_doc` with a readiness assertion driven by a shared
    /// "applied" flag, so `check` only reports ready once `mutate` has
    /// actually dispatched -- a vacuous (assertion-free) validate block
    /// would trivially pass on the very first preflight check and the
    /// action would never dispatch at all, which is a legitimate plan
    /// shape but useless for exercising the action-then-validate path.
    fn gated_action_doc() -> (Document, Arc<AtomicBool>) {
        let mut doc = single_action_doc();
        doc.nodes[0].validate = Some(Validation {
            tool: "check".to_string(),
            assert: vec![Assertion {
                path: "/ready".to_string(),
                op: "eq".to_string(),
                value: Some(Value::Bool(true)),
                assertions: vec![],
            }],
            ..Default::default()
        });
        (doc, Arc::new(AtomicBool::new(false)))
    }

    fn gated_dispatch<'a>(applied: Arc<AtomicBool>, calls: Arc<AtomicUsize>) -> Runner<'a> {
        runner_with_dispatch(move |_ctx, name, _args| {
            calls.fetch_add(1, Ordering::SeqCst);
            match name {
                "mutate" => {
                    applied.store(true, Ordering::SeqCst);
                    Ok(DispatchResult {
                        is_error: false,
                        structured_content: Some(Value::Object(Map::new())),
                        text: String::new(),
                    })
                }
                "check" => {
                    let mut content = Map::new();
                    content.insert(
                        "ready".to_string(),
                        Value::Bool(applied.load(Ordering::SeqCst)),
                    );
                    Ok(DispatchResult {
                        is_error: false,
                        structured_content: Some(Value::Object(content)),
                        text: String::new(),
                    })
                }
                other => panic!("unexpected tool {other}"),
            }
        })
    }

    #[test]
    fn idempotent_rerun_dispatches_nothing_when_already_satisfied() {
        let (doc, applied) = gated_action_doc();
        let calls = Arc::new(AtomicUsize::new(0));
        let runner = gated_dispatch(applied, calls.clone());
        let (state, result) = runner.run(&RunCtx::background(), &doc, RunState::default());
        assert!(result.is_ok(), "first run should succeed: {:?}", result);
        assert_eq!(state.nodes.get("n1").unwrap().status, STATUS_APPLIED);
        // Preflight check (not ready) + action + post-action readiness check.
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        let (_, result2) = runner.run(&RunCtx::background(), &doc, state);
        assert!(result2.is_ok());
        assert_eq!(
            calls.load(Ordering::SeqCst),
            4,
            "the idempotency check dispatches once more, but the action never re-dispatches"
        );
    }

    #[test]
    fn retry_then_success_eventually_applies() {
        let (mut doc, applied) = gated_action_doc();
        doc.nodes[0].retry = Retry {
            max_attempts: 3,
            backoff_ms: 1,
            backoff_factor: 1,
        };
        let action_attempts = Arc::new(AtomicI64::new(0));
        let action_attempts_clone = action_attempts.clone();
        let runner = runner_with_dispatch(move |_ctx, name, _args| match name {
            "mutate" => {
                let n = action_attempts_clone.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    return Err("transient failure".to_string());
                }
                applied.store(true, Ordering::SeqCst);
                Ok(DispatchResult {
                    is_error: false,
                    structured_content: Some(Value::Object(Map::new())),
                    text: String::new(),
                })
            }
            "check" => {
                let mut content = Map::new();
                content.insert(
                    "ready".to_string(),
                    Value::Bool(applied.load(Ordering::SeqCst)),
                );
                Ok(DispatchResult {
                    is_error: false,
                    structured_content: Some(Value::Object(content)),
                    text: String::new(),
                })
            }
            other => panic!("unexpected tool {other}"),
        });
        let (state, result) = runner.run(&RunCtx::background(), &doc, RunState::default());
        assert!(result.is_ok(), "{:?}", result);
        assert_eq!(state.nodes.get("n1").unwrap().status, STATUS_APPLIED);
        assert!(action_attempts.load(Ordering::SeqCst) >= 2);
    }

    #[test]
    fn permanent_failure_triggers_compensation() {
        // n1 applies successfully and declares compensation; n2 depends on
        // n1 and fails permanently. Go's abort path only compensates nodes
        // that reached StatusApplied in an earlier node/level, so this needs
        // two nodes, not one -- a single node that never applies has
        // nothing to roll back.
        let doc = Document {
            contract_version: schema::CONTRACT_VERSION.to_string(),
            plan_id: "p1".to_string(),
            idempotency_key: "k1".to_string(),
            generation: 1,
            nodes: vec![
                Node {
                    id: "n1".to_string(),
                    action: Some(Action {
                        tool: "apply_n1".to_string(),
                        args: Map::new(),
                    }),
                    validate: Some(Validation {
                        tool: "check_n1".to_string(),
                        assert: vec![Assertion {
                            path: "/ready".to_string(),
                            op: "eq".to_string(),
                            value: Some(Value::Bool(true)),
                            assertions: vec![],
                        }],
                        ..Default::default()
                    }),
                    compensate: Some(Action {
                        tool: "undo_n1".to_string(),
                        args: Map::new(),
                    }),
                    ..Default::default()
                },
                Node {
                    id: "n2".to_string(),
                    depends_on: vec!["n1".to_string()],
                    action: Some(Action {
                        tool: "apply_n2".to_string(),
                        args: Map::new(),
                    }),
                    // check_n2 always errors (a readiness probe that can
                    // never reach the resource), which is a fatal dispatch
                    // error during the preflight check and never falls
                    // through to apply_n2 at all -- exercising the same
                    // "node failed permanently -> compensate upstream"
                    // path Go's runner takes for a hard validate failure.
                    validate: Some(Validation {
                        tool: "check_n2".to_string(),
                        ..Default::default()
                    }),
                    retry: Retry {
                        max_attempts: 1,
                        backoff_ms: 0,
                        backoff_factor: 1,
                    },
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let mut capabilities = BTreeMap::new();
        for tool in ["apply_n1", "undo_n1", "apply_n2"] {
            capabilities.insert(
                tool.to_string(),
                Capability {
                    name: tool.to_string(),
                    input_schema: Map::new(),
                    output_schema: Map::new(),
                    effect: "create".to_string(),
                    idempotent: true,
                },
            );
        }
        for tool in ["check_n1", "check_n2"] {
            capabilities.insert(
                tool.to_string(),
                Capability {
                    name: tool.to_string(),
                    input_schema: Map::new(),
                    output_schema: Map::new(),
                    effect: "read".to_string(),
                    idempotent: true,
                },
            );
        }
        let n1_applied = Arc::new(AtomicBool::new(false));
        let n1_applied_clone = n1_applied.clone();
        let compensated = Arc::new(AtomicBool::new(false));
        let compensated_clone = compensated.clone();
        let runner = Runner {
            capabilities,
            catalog_revision: String::new(),
            host_agent_id: String::new(),
            sink: None,
            dispatch: Arc::new(move |_ctx, name, _args| match name {
                "apply_n1" => {
                    n1_applied_clone.store(true, Ordering::SeqCst);
                    Ok(DispatchResult {
                        is_error: false,
                        structured_content: Some(Value::Object(Map::new())),
                        text: String::new(),
                    })
                }
                "check_n1" => {
                    let mut content = Map::new();
                    content.insert(
                        "ready".to_string(),
                        Value::Bool(n1_applied_clone.load(Ordering::SeqCst)),
                    );
                    Ok(DispatchResult {
                        is_error: false,
                        structured_content: Some(Value::Object(content)),
                        text: String::new(),
                    })
                }
                "undo_n1" => {
                    compensated_clone.store(true, Ordering::SeqCst);
                    Ok(DispatchResult {
                        is_error: false,
                        structured_content: Some(Value::Object(Map::new())),
                        text: String::new(),
                    })
                }
                _ => Err("permanent failure".to_string()),
            }),
        };
        let (state, result) = runner.run(&RunCtx::background(), &doc, RunState::default());
        assert!(result.is_err());
        assert_eq!(
            state.nodes.get("n1").unwrap().status,
            STATUS_COMPENSATED,
            "n1 applied, then n2's permanent failure must roll it back"
        );
        assert!(
            compensated.load(Ordering::SeqCst),
            "compensation should have dispatched"
        );
    }

    #[test]
    fn cycle_in_graph_is_rejected_before_any_dispatch() {
        let mut doc = single_action_doc();
        doc.nodes.push(Node {
            id: "n2".to_string(),
            depends_on: vec!["n1".to_string()],
            action: Some(Action {
                tool: "mutate".to_string(),
                args: Map::new(),
            }),
            ..Default::default()
        });
        doc.nodes[0].depends_on.push("n2".to_string());
        let runner =
            runner_with_dispatch(|_, _, _| panic!("must not dispatch on an invalid graph"));
        let (_, result) = runner.run(&RunCtx::background(), &doc, RunState::default());
        assert!(result.is_err());
    }

    #[test]
    fn cancel_before_dispatch_marks_node_unknown() {
        let doc = single_action_doc();
        let runner = runner_with_dispatch(|_, _, _| panic!("must not dispatch after cancellation"));
        let (ctx, cancel) = RunCtx::background().with_cancel();
        cancel.cancel();
        let (state, result) = runner.run(&ctx, &doc, RunState::default());
        assert!(result.is_err());
        assert_eq!(state.nodes.get("n1").unwrap().status, STATUS_UNKNOWN);
    }

    fn wait_doc() -> Document {
        let mut properties = Map::new();
        properties.insert(
            "approved".to_string(),
            serde_json::json!({"type": "boolean"}),
        );
        let input_schema = serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": properties,
            "required": ["approved"],
        })
        .as_object()
        .unwrap()
        .clone();
        Document {
            contract_version: schema::CONTRACT_VERSION.to_string(),
            plan_id: "p1".to_string(),
            idempotency_key: "k1".to_string(),
            generation: 1,
            nodes: vec![Node {
                id: "n1".to_string(),
                wait: Some(schema::WaitSpec {
                    trigger: schema::WaitTrigger {
                        kind: "operator".to_string(),
                        kind_type: "approval".to_string(),
                    },
                    input_schema,
                    schema_revision: "r1".to_string(),
                    ..Default::default()
                }),
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn wait_then_resume_completes_the_plan() {
        let runner =
            runner_with_dispatch(|_, _, _| panic!("a plain wait node never dispatches a tool"));
        let (waiting, result) = runner.run(&RunCtx::background(), &wait_doc(), RunState::default());
        assert!(result.is_ok(), "{:?}", result);
        assert_eq!(waiting.status, RUN_STATUS_WAITING);
        assert_eq!(waiting.nodes.get("n1").unwrap().status, STATUS_WAITING);
        let wait = waiting.wait.clone().unwrap();
        assert_eq!(wait.wait_revision, 1);

        let mut input = Map::new();
        input.insert("approved".to_string(), Value::Bool(true));
        let request = schema::ResumeRequest {
            wait_node_id: wait.node_id.clone(),
            wait_revision: wait.wait_revision,
            schema_revision: wait.schema_revision.clone(),
            correlation: wait.correlation.clone(),
            input,
            source: "operator".to_string(),
        };
        let (resumed, result) = runner.resume(&RunCtx::background(), &wait_doc(), waiting, request);
        assert!(result.is_ok(), "{:?}", result);
        assert_eq!(resumed.status, "completed");
        assert!(resumed.wait.is_none());
        assert_eq!(resumed.nodes.get("n1").unwrap().status, STATUS_SATISFIED);
    }

    #[test]
    fn resume_rejects_a_mismatched_wait_revision() {
        let runner = runner_with_dispatch(|_, _, _| panic!("a rejected resume must not dispatch"));
        let (waiting, _) = runner.run(&RunCtx::background(), &wait_doc(), RunState::default());
        let wait = waiting.wait.clone().unwrap();
        let mut input = Map::new();
        input.insert("approved".to_string(), Value::Bool(true));
        let request = schema::ResumeRequest {
            wait_node_id: wait.node_id.clone(),
            wait_revision: wait.wait_revision + 1,
            schema_revision: wait.schema_revision.clone(),
            correlation: wait.correlation.clone(),
            input,
            source: "operator".to_string(),
        };
        let err = validate_resume(&wait_doc(), &waiting, &request).unwrap_err();
        assert!(err.contains(ERR_RESUME_CONFLICT));
    }
}
