//! `tools/call`: the Go server's `handleToolCall` pipeline.
//!
//! The SDK layer has already decoded and validated the request envelope. This
//! module resolves the wire name to a registered capability, decodes the
//! arguments the way `json.Unmarshal` into `map[string]any` does, checks the
//! requested catalog revision, applies the standalone mutation gate, and
//! dispatches. Capabilities whose handlers have not been ported yet fail
//! closed with a typed `not_implemented` capability error; the catalog still
//! publishes them, so `tools/list` stays identical to Go.

use crate::admission::{self, Binding};
use crate::catalog;
use crate::evidence::{redact_for_delivery, redact_for_storage};
use crate::gojson::{Node, Value};
use crate::mcpsdk::ToolCallOutcome;
use crate::transport::Server;
use serde_json::{json, Map, Value as J};
use std::sync::{Arc, Mutex};

/// What host capabilities read: the agent's environment and identity, the
/// incus provider, the resource coordinator, and the resource registry.
#[derive(Clone)]
pub struct Host {
    pub env: crate::config::Env,
    pub incus: crate::incus::Incus,
    pub coordinator: crate::resource::Coordinator,
    pub state: Arc<Mutex<crate::store::StateStore>>,
    pub tenant_id: String,
    pub agent_id: String,
    pub instance_id: String,
    /// `OPUTE_SHARED_HOST_OWNER_INSTANCE`: when set, only that instance may
    /// change the shared host (`RequireSharedHostOwner`).
    pub shared_host_owner_instance: String,
    pub instance_root: String,
    pub mcp_port: i64,
    /// `resolveResetCheckpointPath`: where an interrupted stack reset
    /// resumes from.
    pub reset_checkpoint_path: String,
}

/// Resolves a wire name to the catalog name it was registered under, if any.
fn registered(server: &Server, wire: &str) -> Option<String> {
    let catalog_snapshot = server.catalog.wire_snapshot();
    let name = if server.tool_prefix.trim().is_empty() {
        wire.to_string()
    } else {
        wire.strip_prefix(&format!("{}_", server.tool_prefix.trim()))?
            .to_string()
    };
    let known = catalog_snapshot.find(&name).is_some()
        || catalog::internal().tools.iter().any(|d| d.name == name);
    (known && catalog::wire_name(&server.tool_prefix, &name) == wire).then_some(name)
}

/// `tools.ErrorResult` for a plain error.
pub fn error_result(message: &str) -> J {
    json!({"content": [{"type": "text", "text": format!("Error: {message}")}], "isError": true})
}

/// `tools.ErrorResult` for a typed `CapabilityError`.
pub fn capability_error(owner: &str, code: &str, message: &str) -> J {
    json!({
        "content": [{"type": "text", "text": format!("Error: {message}")}],
        "structuredContent": {"code": code, "owner": owner, "message": message},
        "isError": true,
    })
}

/// hostmcp's `structuredResult`: text content only when text is given.
pub fn structured_result(value: J, text: Option<&str>) -> J {
    let content: Vec<J> = text
        .map(|t| json!({"type": "text", "text": t}))
        .into_iter()
        .collect();
    json!({"content": content, "structuredContent": value})
}

/// `json.Unmarshal(arguments, &map[string]any{})`.
fn decode_arguments(params: Option<&Node>) -> Result<Map<String, J>, String> {
    let Some(raw) = params.and_then(|p| p.field("arguments")) else {
        return Ok(Map::new());
    };
    let kind = match &raw.value {
        Value::Null => return Ok(Map::new()),
        Value::Object(_) => {
            return Ok(match crate::transport::go_any(raw) {
                J::Object(m) => m,
                _ => Map::new(),
            })
        }
        Value::Array(_) => "array",
        Value::String(_) => "string",
        Value::Number(_) => "number",
        Value::Bool(_) => "bool",
    };
    Err(format!(
        "json: cannot unmarshal {kind} into Go value of type map[string]interface {{}}"
    ))
}

fn requested_revision(params: Option<&Node>) -> String {
    params
        .and_then(|p| p.field("_meta"))
        .and_then(|m| m.key("catalogRevision"))
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

pub fn call(server: &Arc<Server>, wire: &str, params: Option<&Node>) -> ToolCallOutcome {
    let catalog_snapshot = server.catalog.snapshot();
    let Some(name) = registered(server, wire) else {
        return ToolCallOutcome::Unknown;
    };
    let requested = requested_revision(params);
    if !requested.is_empty() && requested != catalog_snapshot.revision {
        return ToolCallOutcome::Result(capability_error(
            "lifecycle",
            "catalog_revision_stale",
            &format!(
                "catalog revision {} is stale; current revision is {}",
                crate::goerr::quote(&requested),
                crate::goerr::quote(&catalog_snapshot.revision)
            ),
        ));
    }
    let args = match decode_arguments(params) {
        Ok(a) => a,
        Err(e) => return ToolCallOutcome::Result(error_result(&format!("invalid arguments: {e}"))),
    };
    // `contextForProviderCallback`: a signed delegation re-attaches the
    // calling provider task's reservation and identity, or refuses.
    let meta = params
        .and_then(|p| p.field("_meta"))
        .map(crate::transport::go_any)
        .and_then(|value| match value {
            J::Object(map) => Some(map),
            _ => None,
        });
    // A provider callback may name a host tool by its D19 alias.
    let host_tools = catalog::callable_host_tool_names();
    let scope = crate::provider::delegation::CallbackScope {
        agent: &server.agent_id,
        tasks: &server.tasks,
        host_tools: &host_tools,
        catalog: &catalog_snapshot,
    };
    let callback = match server.providers.delegations.callback(
        &crate::plan::runner::RunCtx::background(),
        meta.as_ref(),
        &name,
        &scope,
    ) {
        Ok(ctx) => ctx.reservation().is_some().then_some(ctx),
        Err(e) => return ToolCallOutcome::Result(error_result(e)),
    };
    // Decision D10 (standalone-read-only-gate): with mutations disabled,
    // standalone runs read tools only.
    if server.standalone
        && !server.allow_mutations
        && !catalog::standalone_read_only(&catalog_snapshot, &name)
    {
        return ToolCallOutcome::Result(error_result(
            "standalone mutations are disabled; set OPUTE_STANDALONE_ALLOW_MUTATIONS=true",
        ));
    }
    // Lifecycle tools are routed before the task boundary. They carry no
    // resource binding but cross the same host admission boundary.
    if LIFECYCLE.contains(&name.as_str()) {
        let binding = Binding::default();
        let reservation = match admission::admit(server, &name, &args, &binding, None, None) {
            Ok(r) => r,
            Err(refusal) => return ToolCallOutcome::Result(refusal.render()),
        };
        // run_host_plan and run_host_local_recipe spawn their plan on a
        // detached thread that outlives this call; the reservation belongs
        // to that run, not to the synchronous request that launched it, so
        // whichever of the two finishes touching it last is the one that
        // must release it -- mirrors Go's reservationLease
        // claim/releaseIfUnclaimed. The handler claims the lease (setting
        // `claimed`) only once it actually spawns the thread; every other
        // outcome -- a validation error, an idempotent short-circuit to an
        // already-terminal run -- leaves it unclaimed for the plain release
        // below, same as every other lifecycle tool.
        let claimed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let ctx = crate::plan::runner::RunCtx::background().with_reservation(reservation.clone());
        let result = run_lifecycle(
            server,
            &name,
            &crate::builtin::Invocation {
                ctx: &ctx,
                args: &args,
                binding: &binding,
                snapshot: &catalog_snapshot,
                claimed: Some(&claimed),
            },
        );
        if !claimed.load(std::sync::atomic::Ordering::SeqCst) {
            admission::release(server, &reservation);
        }
        return ToolCallOutcome::Result(result);
    }
    if name == "request_task_input" {
        if !task_extension_declared(params) {
            return missing_tasks_capability();
        }
        return ToolCallOutcome::Result(run_task_input(server, &name, &args));
    }
    // Provider-declared bridged work crosses the Host Agent task boundary
    // before the public request reaches the provider.
    let bridged = catalog_snapshot
        .find(&name)
        .is_some_and(|d| d.task_support == "bridged");
    if bridged || catalog::is_task_aware(&name) {
        if !task_extension_declared(params) {
            return missing_tasks_capability();
        }
        return ToolCallOutcome::Result(create_async_task(server, &name, args, callback.as_ref()));
    }
    ToolCallOutcome::Result(dispatch(server, &name, &args, None, callback.as_ref()))
}

/// Callback-owned async tasks whose handle has not been acknowledged yet
/// (`pendingAsyncTaskStarts`). A delegated callback's parent reservation is
/// bounded by the provider call, so the child does not start until the
/// caller polls that exact task.
#[derive(Default)]
pub struct TaskStarts {
    pending: std::sync::Mutex<std::collections::HashMap<String, Box<dyn FnOnce() + Send>>>,
    closed: std::sync::atomic::AtomicBool,
}

/// `callbackTaskAcknowledgementTimeout`.
const CALLBACK_ACKNOWLEDGEMENT: std::time::Duration = std::time::Duration::from_secs(120);

fn cancel_pending_task(server: &Server, task_id: &str, message: &str) {
    if let Some(snapshot) = server.tasks.cancel_with_message(task_id, message) {
        persist_task(server, &snapshot);
    }
}

/// `startPendingAsyncTask`, on `tasks/get`.
pub fn start_pending_task(server: &Server, task_id: &str) {
    let start = server.task_starts.pending.lock().unwrap().remove(task_id);
    if let Some(start) = start {
        if !server
            .task_starts
            .closed
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            start();
        }
    }
}

/// `closePendingAsyncTaskStarts`: shutdown cancels unacknowledged callback
/// tasks and every task still working.
pub fn close_task_starts(server: &Server) {
    server
        .task_starts
        .closed
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let pending: Vec<String> = server
        .task_starts
        .pending
        .lock()
        .unwrap()
        .drain()
        .map(|(id, _)| id)
        .collect();
    for id in pending {
        cancel_pending_task(
            server,
            &id,
            "The Host Agent shut down before the callback task handle was acknowledged.",
        );
    }
    for id in server.tasks.working_ids() {
        cancel_pending_task(
            server,
            &id,
            "The Host Agent shut down before the task completed.",
        );
    }
}

/// The task-input operation, resolved through the service context.
pub(crate) fn run_task_input(server: &Arc<Server>, name: &str, args: &Map<String, J>) -> J {
    let ctx = crate::plan::runner::RunCtx::background();
    let invocation = crate::builtin::Invocation {
        ctx: &ctx,
        args,
        binding: &Binding::default(),
        snapshot: &server.catalog.snapshot(),
        claimed: None,
    };
    if let Some(outcome) = crate::builtin::invoke(server, name, &invocation) {
        return outcome.unwrap_or_else(|e| error_result(&e));
    }
    not_implemented(name)
}

/// `createInputRequestTask`: a task that completes with the operator's
/// response once `tasks/update` supplies it.
pub(crate) fn create_input_task(server: &Arc<Server>, args: &Map<String, J>) -> J {
    let prompt = args.get("prompt").and_then(J::as_str).unwrap_or("");
    if prompt.trim().is_empty() {
        return error_result("request_task_input requires prompt");
    }
    let response_type = match args.get("responseType").and_then(J::as_str) {
        Some(requested) if !requested.is_empty() => requested,
        _ => "string",
    };
    if !matches!(response_type, "string" | "boolean") {
        return error_result("request_task_input responseType must be string or boolean");
    }
    let mut inputs = Map::new();
    inputs.insert(
        "response".into(),
        json!({"type": response_type, "prompt": prompt}),
    );
    let desc = "Waiting for operator input...";
    let tool_args = redact_task_args(server, "request_task_input", args);
    // The continuation lives inside the registry the server owns; a weak
    // reference keeps it from holding the server alive.
    let owner = Arc::downgrade(server);
    let rec = server.tasks.create_with_input(
        "request_task_input",
        desc,
        tool_args,
        inputs,
        move |task_id, responses| {
            if let Some(server) = owner.upgrade() {
                let response = responses.get("response").cloned().unwrap_or(J::Null);
                let tool_result = crate::tasks::ToolResult {
                    structured_content: Some(json!({"response": response})),
                    ..Default::default()
                };
                server.tasks.complete(task_id, tool_result.clone());
                let _ = server.host.state.lock().map(|state| {
                    state.complete_operation(task_id, &tool_result_json(&tool_result))
                });
                if let Some(completed) = server.tasks.get(task_id) {
                    persist_task(&server, &completed);
                }
            }
        },
    );
    let _ = server
        .host
        .state
        .lock()
        .map(|state| state.create_operation(&rec.task_id, "request_task_input", desc));
    persist_task(server, &rec);
    json!({
        "content": [{"type": "text", "text": desc}],
        "structuredContent": rec.create_result(),
    })
}

/// `json.Marshal` of a Go `tasks.ToolResult`: `content`, `structuredContent`
/// and `isError` are all `omitempty`, and this is the exact value Go's
/// `_ = s.state.Complete(taskID, tr)` persists as `result_json`.
fn tool_result_json(result: &crate::tasks::ToolResult) -> J {
    let mut out = Map::new();
    if let Some(content) = result.content.as_ref().filter(|c| !c.is_empty()) {
        out.insert("content".into(), J::Array(content.clone()));
    }
    if let Some(structured) = result
        .structured_content
        .as_ref()
        .filter(|v| **v != J::Null)
    {
        out.insert("structuredContent".into(), structured.clone());
    }
    if result.is_error {
        out.insert("isError".into(), J::Bool(true));
    }
    J::Object(out)
}

/// `persistTask`: best-effort, exactly as Go's own
/// `_ = s.state.SaveTaskSnapshot(...)`. A durability failure here must never
/// fail the in-memory task that `server.tasks` already tracks; only a crash
/// before this write loses the snapshot, a risk Go's design accepts too.
pub(crate) fn persist_task(server: &Server, snapshot: &crate::tasks::Snapshot) {
    let mut doc = snapshot.get_result();
    if let J::Object(map) = &mut doc {
        map.insert("toolName".into(), J::from(snapshot.tool_name.clone()));
        map.insert("toolArgs".into(), snapshot.tool_args.clone());
        if !snapshot.description.is_empty() {
            map.insert("description".into(), J::from(snapshot.description.clone()));
        }
    }
    if let Ok(state) = server.host.state.lock() {
        // D12: refused task-aware calls have only an in-memory task handle.
        // Cancellation and terminal snapshot writes must not materialize an
        // operation that admission never accepted.
        if !matches!(state.get_operation(&snapshot.task_id), Ok(Some(_))) {
            return;
        }
        let _ = state.save_registry_task_snapshot(
            &snapshot.task_id,
            &snapshot.tool_name,
            &snapshot.description,
            &doc,
        );
    }
}

/// `createAsyncTask`: the call returns a task handle at once and the
/// capability runs on its own thread under the task's operation identity.
/// A tool-level error completes the task with `isError`; `failed` is
/// reserved for execution errors.
fn create_async_task(
    server: &Arc<Server>,
    name: &str,
    args: Map<String, J>,
    callback: Option<&crate::plan::runner::RunCtx>,
) -> J {
    let owner = match callback {
        None => None,
        Some(ctx) => {
            // A provider callback may name a host tool by its D19 alias.
            let host_tools = catalog::callable_host_tool_names();
            let catalog_snapshot = server.catalog.snapshot();
            let scope = crate::provider::delegation::CallbackScope {
                agent: &server.agent_id,
                tasks: &server.tasks,
                host_tools: &host_tools,
                catalog: &catalog_snapshot,
            };
            match scope.task_owner(ctx) {
                Ok(owner) => owner,
                Err(e) => return error_result(e),
            }
        }
    };
    if server
        .task_starts
        .closed
        .load(std::sync::atomic::Ordering::SeqCst)
    {
        return error_result("the Host Agent is shutting down and cannot start a task");
    }
    let desc = match args.get("vmName").and_then(J::as_str) {
        Some(vm) if !vm.is_empty() => format!("Running {name} on '{vm}'..."),
        _ => format!("Executing {name}..."),
    };
    // Cancellation is cooperative, as in Go: the work runs to completion
    // even when the task is cancelled first, and the registry discards the
    // late result (a cancelled task stays cancelled).
    let tool_args = redact_task_args(server, name, &args);
    let (rec, cancelled) = server.tasks.create(name, &desc, tool_args);
    let worker = Arc::clone(server);
    let (task_id, name) = (rec.task_id.clone(), name.to_string());
    // Go runs the task under `CreateWithCancel`'s context, so tasks/cancel
    // and shutdown abort an in-flight provider request. Host tools keep the
    // cooperative path above.
    let provider = server
        .catalog
        .snapshot()
        .tools
        .iter()
        .any(|d| d.name == name && d.implementation.trim().starts_with("provider:"));
    let base = if provider {
        crate::plan::runner::RunCtx::from_flag(cancelled)
    } else {
        crate::plan::runner::RunCtx::background()
    };
    // A delegated child keeps its own task identity for its record while
    // admission stays attached to the parent task's reservation.
    let delegated = owner.is_some();
    let child = match owner.as_ref() {
        Some(owner) => Some(crate::provider::delegation::TaskOwner::child_context(
            &base,
            &name,
            &task_id,
            Some(owner),
        )),
        None => provider.then_some(base),
    };
    let start = move || {
        let result = dispatch(
            &worker,
            &name,
            &args,
            Some((&name, &task_id)),
            child.as_ref(),
        );
        let content: Vec<J> = result
            .get("content")
            .and_then(J::as_array)
            .into_iter()
            .flatten()
            .filter(|c| c.get("type").and_then(J::as_str) == Some("text"))
            .map(|c| json!({"type": "text", "text": c.get("text").cloned().unwrap_or(J::Null)}))
            .collect();
        let structured = redact_task_result(&worker, &name, result.get("structuredContent"));
        let tool_result = crate::tasks::ToolResult {
            content: (!content.is_empty()).then_some(content),
            structured_content: structured,
            is_error: result.get("isError") == Some(&J::Bool(true)),
        };
        worker.tasks.complete(&task_id, tool_result.clone());
        let _ = worker
            .host
            .state
            .lock()
            .map(|state| state.complete_operation(&task_id, &tool_result_json(&tool_result)));
        if let Some(completed) = worker.tasks.get(&task_id) {
            persist_task(&worker, &completed);
        }
    };
    let spawner = Arc::downgrade(server);
    let spawn = move || {
        let Some(server) = spawner.upgrade() else {
            return;
        };
        let handle = std::thread::spawn(start);
        let workers = server.task_workers.lock();
        if let Ok(mut workers) = workers {
            workers.retain(|worker| !worker.is_finished());
            workers.push(handle);
        }
    };
    if delegated {
        server
            .task_starts
            .pending
            .lock()
            .unwrap()
            .insert(rec.task_id.clone(), Box::new(spawn));
        let expiry = Arc::downgrade(server);
        let task_id = rec.task_id.clone();
        std::thread::spawn(move || {
            std::thread::sleep(CALLBACK_ACKNOWLEDGEMENT);
            if let Some(server) = expiry.upgrade() {
                let expired = server.task_starts.pending.lock().unwrap().remove(&task_id);
                if expired.is_some() {
                    cancel_pending_task(
                        &server,
                        &task_id,
                        "The callback task handle was not acknowledged before its timeout.",
                    );
                }
            }
        });
    } else {
        spawn();
    }
    json!({
        "content": [{"type": "text", "text": desc}],
        "structuredContent": rec.create_result(),
    })
}

/// `redactTaskArgs`: a persisted task snapshot's `toolArgs` is an echo of
/// the caller's own input, so D13's stricter storage projection costs
/// nothing here -- the caller already holds whatever they sent -- while
/// still ensuring a secret argument never carries into `state.db`.
fn redact_task_args(server: &Server, name: &str, args: &Map<String, J>) -> J {
    let catalog_snapshot = server.catalog.snapshot();
    let descriptor = catalog_snapshot
        .find(name)
        .or_else(|| catalog::internal().tools.iter().find(|d| d.name == name));
    let Some(descriptor) = descriptor else {
        return json!({"redacted": true});
    };
    let schema = descriptor.input_schema.as_object();
    redact_for_storage(&J::Object(args.clone()), schema)
}

/// `redactTaskResult`: this becomes the task's own completed result --
/// delivered through `tasks/get`, live and after a restart -- so it uses
/// the delivery projection, not the stricter storage one: a `writeOnly`
/// value still never reaches task state, but open/undeclared content
/// reaches the caller exactly as Go returns it, since the caller already
/// owns it and this is its one delivery channel.
fn redact_task_result(server: &Server, name: &str, value: Option<&J>) -> Option<J> {
    let catalog_snapshot = server.catalog.snapshot();
    let descriptor = catalog_snapshot
        .find(name)
        .or_else(|| catalog::internal().tools.iter().find(|d| d.name == name));
    let Some(descriptor) = descriptor else {
        return Some(json!({"redacted": true}));
    };
    let schema = descriptor.output_schema.as_ref().and_then(J::as_object);
    value.map(|v| redact_for_delivery(v, schema))
}

/// `isLifecycleTool`: transport-owned operations routed before tasks.
pub(crate) const LIFECYCLE: &[&str] = &[
    "validate_host_plan",
    "run_host_plan",
    "get_host_plan_run",
    "validate_host_local_recipe",
    "run_host_local_recipe",
    "validate_runtime_recipe",
    "run_runtime_recipe",
    "get_runtime_recipe_run",
    "validate_tunnel_recipe",
    "run_tunnel_recipe",
    "get_tunnel_run",
    "opute.provider.install",
    "opute.provider.validate",
    "opute.provider.status",
    "opute.provider.reload",
    "opute.provider.teardown",
    "get_capability_catalog",
    "open_assistant_session",
];

/// A lifecycle operation, resolved through the service context.
pub(crate) fn run_lifecycle(
    server: &Arc<Server>,
    name: &str,
    call: &crate::builtin::Invocation,
) -> J {
    match crate::builtin::invoke(server, name, call) {
        Some(outcome) => outcome.unwrap_or_else(|e| error_result(&e)),
        None => not_implemented(name),
    }
}

/// A capability whose handler this build does not carry yet. It fails
/// closed with a typed error rather than doing anything.
fn not_implemented(name: &str) -> J {
    capability_error(
        "capability",
        "not_implemented",
        &format!("{name} is not implemented by this Host Agent build"),
    )
}

/// `DispatchTool`: the single execution boundary for direct calls and task
/// workers. Binding, then admission, then the capability; the reservation is
/// released when the capability returns. `identity` is the async task's
/// operation identity. `cancel_ctx`, when given, is the plan executor's
/// `RunCtx` for this node: a watcher releases the reservation as soon as
/// that context is cancelled, instead of holding it until `invoke` itself
/// returns -- the executor already gives up waiting on the call in that
/// case (see `Runner::dispatch`), and the call runs to completion on its
/// own thread regardless, so the reservation must free up independently of
/// it, not stay held for a call nothing is still waiting on.
pub(crate) fn dispatch(
    server: &Server,
    name: &str,
    args: &Map<String, J>,
    identity: Option<(&str, &str)>,
    cancel_ctx: Option<&crate::plan::runner::RunCtx>,
) -> J {
    let binding = match admission::resolve_binding(server, name, args) {
        Ok(b) => b,
        Err(refusal) => return refusal.render(),
    };
    let parent = cancel_ctx.and_then(|ctx| ctx.reservation());
    // Go reads the admission identity from the context; a delegated
    // callback's context carries its parent task, which the parent
    // reservation is bound to.
    let context_identity = cancel_ctx
        .map(|ctx| ctx.operation_identity())
        .filter(|(operation, task)| !operation.is_empty() || !task.is_empty());
    let admission_identity = context_identity
        .as_ref()
        .map(|(operation, task)| (operation.as_str(), task.as_str()))
        .or(identity);
    let reservation = match admission::admit(
        server,
        name,
        args,
        &binding,
        admission_identity,
        parent.as_deref(),
    ) {
        Ok(r) => r,
        Err(refusal) => return refusal.render(),
    };
    if let Some((_, task_id)) = identity {
        if let Some(snapshot) = server.tasks.get(task_id) {
            let _ = server
                .host
                .state
                .lock()
                .map(|state| state.create_operation(task_id, name, &snapshot.description));
            persist_task(server, &snapshot);
        }
    }
    let catalog_snapshot = server.catalog.snapshot();
    let provider = catalog_snapshot
        .tools
        .iter()
        .find(|d| d.name == name && d.implementation.trim().starts_with("provider:"));
    if let (Some(descriptor), Some(capability)) = (provider, server.catalog.capability(name)) {
        let base = cancel_ctx
            .cloned()
            .unwrap_or_else(crate::plan::runner::RunCtx::background);
        let mut ctx = base.with_reservation(reservation.clone());
        if let Some((operation, task)) = identity {
            ctx = ctx.with_operation_identity(operation, task);
        }
        let result = dispatch_provider(
            server,
            &catalog_snapshot,
            descriptor,
            &capability,
            &ctx,
            args,
            &binding,
            &reservation,
        );
        admission::release(server, &reservation);
        return result;
    }
    // The same invocation context a provider capability receives: the
    // caller's cancellation, the admitted reservation and the task identity.
    let mut invocation_ctx = cancel_ctx
        .cloned()
        .unwrap_or_else(crate::plan::runner::RunCtx::background)
        .with_reservation(reservation.clone());
    if let Some((operation, task)) = identity {
        invocation_ctx = invocation_ctx.with_operation_identity(operation, task);
    }
    let result = match cancel_ctx {
        Some(ctx) => {
            let finished = std::sync::atomic::AtomicBool::new(false);
            std::thread::scope(|scope| {
                scope.spawn(|| {
                    while !finished.load(std::sync::atomic::Ordering::SeqCst) {
                        if ctx.err().is_some() {
                            admission::release(server, &reservation);
                            finished.store(true, std::sync::atomic::Ordering::SeqCst);
                            return;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(20));
                    }
                });
                let r = invoke(server, &invocation_ctx, name, args, &binding);
                finished.store(true, std::sync::atomic::Ordering::SeqCst);
                r
            })
        }
        None => invoke(server, &invocation_ctx, name, args, &binding),
    };
    // Schema-invalid arguments are an owner-approved reject-without-audit
    // divergence (D11). Admission refusals returned above never get here.
    let code = result
        .pointer("/structuredContent/code")
        .and_then(J::as_str);
    if code != Some("invalid_arguments") {
        record_invocation(server, name, args, &binding, &reservation, &result);
    }
    // Idempotent: a no-op if the watcher above already released this on
    // abandonment.
    admission::release(server, &reservation);
    result
}

/// `dispatchCapabilityValue` for a provider-contributed capability, shared by
/// ordinary dispatch and a candidate's private setup recipe. The capability
/// owns generation affinity and delegation; this owns result admission and
/// durable invocation evidence.
#[allow(clippy::too_many_arguments)]
pub(crate) fn dispatch_provider(
    server: &Server,
    snapshot: &catalog::Snapshot,
    descriptor: &catalog::Descriptor,
    capability: &catalog::registry::Capability,
    ctx: &crate::plan::runner::RunCtx,
    args: &Map<String, J>,
    binding: &Binding,
    reservation: &crate::resource::Reservation,
) -> J {
    let binding = complete_produced_binding(server, descriptor, args, binding);
    let record = |result: Option<&J>, observation: J, error: Option<(&str, &str)>| {
        record_provider_invocation(
            server,
            snapshot,
            descriptor,
            args,
            &binding,
            reservation,
            result,
            observation,
            error,
        )
    };
    let mut result = match (capability.invoke)(ctx, args) {
        Ok(result) => result,
        Err(error) => {
            let failed = error_result(&error);
            record(
                Some(&failed),
                provider_observation(snapshot, descriptor, "invoke_error", None, ""),
                Some(("", "")),
            );
            return failed;
        }
    };
    if result.get("isError") != Some(&J::Bool(true)) {
        let structured = result.remove("structuredContent").unwrap_or(J::Null);
        result.insert(
            "structuredContent".into(),
            materialize_bound_outputs(descriptor, structured, &binding),
        );
    }
    let observation = match crate::provider::observation::validate(descriptor, Some(&result)) {
        Ok(observation) => observation,
        Err(error) => {
            let message = format!(
                "capability {} returned invalid result: {error}",
                crate::goerr::quote(name_of(descriptor))
            );
            let failed = capability_error("capability", "invalid_result", &message);
            record(
                Some(&failed),
                provider_observation(snapshot, descriptor, "invalid_result", None, ""),
                Some(("capability", "invalid_result")),
            );
            return failed;
        }
    };
    if result.get("isError") != Some(&J::Bool(true)) {
        let structured = result.get("structuredContent").cloned().unwrap_or(J::Null);
        if let Err(error) = validate_produced(descriptor, &structured, &server.host.tenant_id) {
            let failed = capability_error("capability", "invalid_resource_output", &error);
            record(
                Some(&failed),
                provider_observation(snapshot, descriptor, "invalid_result", None, ""),
                Some(("capability", "invalid_resource_output")),
            );
            return failed;
        }
    }
    let result = J::Object(result);
    record(
        Some(&result),
        provider_observation(
            snapshot,
            descriptor,
            &observation.status,
            observation.structured.as_ref(),
            &observation.retryability,
        ),
        None,
    );
    result
}

fn name_of(descriptor: &catalog::Descriptor) -> &str {
    &descriptor.name
}

/// `normalizeCapabilityObservation` over the pass-through provider observation.
fn provider_observation(
    snapshot: &catalog::Snapshot,
    descriptor: &catalog::Descriptor,
    status: &str,
    structured: Option<&J>,
    retryability: &str,
) -> J {
    let mut observation = json!({
        "schemaVersion": "opute-capability-observation.v1",
        "operationId": descriptor.operation_id,
        "capabilityVersion": descriptor.version,
        "status": if status.is_empty() { "unknown" } else { status },
        "catalogRevision": snapshot.revision,
    });
    if let Some(structured) = structured {
        observation["structured"] = redact_for_storage(
            structured,
            descriptor.output_schema.as_ref().and_then(J::as_object),
        );
    }
    if !retryability.is_empty() {
        observation["retryability"] = J::from(retryability);
    }
    if !descriptor.generation_id.is_empty() {
        observation["generationId"] = J::from(descriptor.generation_id.clone());
    }
    observation
}

fn execution_binding_json(
    server: &Server,
    revision: &str,
    generation: &str,
    binding: &Binding,
    reservation: &crate::resource::Reservation,
) -> J {
    let mut execution_binding = json!({
        "schemaVersion": "opute-capability-execution-binding.v1",
        "tenantId": server.host.tenant_id, "admission": "tenant-resource-registry",
        "authorization": "admitted", "catalogRevision": revision,
        "reservationId": reservation.id,
        "resourcePolicyRevision": server.host.coordinator.policy_revision(),
    });
    if !generation.is_empty() {
        execution_binding["generationId"] = J::from(generation);
    }
    if !binding.resources.is_empty() {
        let resources: Vec<J> = binding
            .resources
            .iter()
            .filter_map(|bound| {
                let uri = crate::resource::Uri::parse(&bound.uri).ok()?;
                Some(json!({"argument": bound.argument, "uri": bound.uri,
                        "resourceType": uri.resource_type, "tenantId": uri.tenant_id,
                        "resourceId": uri.resource_id, "coordinates": bound.coordinates}))
            })
            .collect();
        execution_binding["resources"] = J::from(resources);
    }
    execution_binding
}

/// `recordCapabilityInvocation` for a provider capability.
#[allow(clippy::too_many_arguments)]
fn record_provider_invocation(
    server: &Server,
    snapshot: &catalog::Snapshot,
    descriptor: &catalog::Descriptor,
    args: &Map<String, J>,
    binding: &Binding,
    reservation: &crate::resource::Reservation,
    result: Option<&J>,
    observation: J,
    error: Option<(&str, &str)>,
) {
    let mut envelope = json!({"isError": false});
    if let Some(result) = result {
        envelope["isError"] = J::Bool(result.get("isError") == Some(&J::Bool(true)));
        envelope["structured"] = redact_for_storage(
            result.get("structuredContent").unwrap_or(&J::Null),
            descriptor.output_schema.as_ref().and_then(J::as_object),
        );
        envelope["content"] = json!([]);
    }
    if let Some((owner, code)) = error {
        envelope["error"] = if owner.is_empty() {
            json!({"message": "capability invocation failed"})
        } else {
            json!({"owner": owner, "code": code, "message": "capability invocation failed"})
        };
    }
    let status = observation
        .get("status")
        .and_then(J::as_str)
        .unwrap_or("unknown")
        .to_string();
    let record = crate::store::CapabilityInvocationRecord {
        invocation_id: crate::tasks::new_task_id(),
        operation_id: descriptor.operation_id.clone(),
        capability_version: descriptor.version,
        catalog_revision: snapshot.revision.clone(),
        generation_id: descriptor.generation_id.clone(),
        authorization: "admitted".into(),
        arguments_json: redact_for_storage(
            &J::Object(args.clone()),
            descriptor.input_schema.as_object(),
        )
        .to_string(),
        binding_json: execution_binding_json(
            server,
            &snapshot.revision,
            &descriptor.generation_id,
            binding,
            reservation,
        )
        .to_string(),
        result_json: envelope.to_string(),
        observation_json: observation.to_string(),
        terminal_status: if error.is_some() {
            "error".into()
        } else {
            status
        },
        ..Default::default()
    };
    if let Ok(state) = server.host.state.lock() {
        let _ = state.record_capability_invocation(&record);
    }
}

/// `completeProducedResourceBinding`.
fn complete_produced_binding(
    server: &Server,
    descriptor: &catalog::Descriptor,
    args: &Map<String, J>,
    binding: &Binding,
) -> Binding {
    let mut binding = binding.clone();
    for produced in &descriptor.produces {
        if produced.source_path.trim() != "uri" {
            continue;
        }
        let bound = binding.resources.iter().any(|resource| {
            !resource.uri.trim().is_empty()
                && crate::resource::Uri::parse(&resource.uri)
                    .is_ok_and(|uri| uri.resource_type == produced.resource_type)
        });
        if bound {
            return binding;
        }
        let Some(candidate) = args.get("uri").and_then(J::as_str) else {
            continue;
        };
        if candidate.trim().is_empty() {
            continue;
        }
        if let Ok((parsed, coordinates)) =
            admission::resolve_resource(&server.host, candidate, &produced.resource_type)
        {
            binding.resources.push(admission::Bound {
                argument: "uri".into(),
                uri: parsed.to_string(),
                coordinates,
            });
        }
    }
    binding
}

/// `materializeBoundResourceOutputs`.
fn materialize_bound_outputs(
    descriptor: &catalog::Descriptor,
    structured: J,
    binding: &Binding,
) -> J {
    let mut object = match structured {
        J::Object(object) => object,
        J::Null => Map::new(),
        other => return other,
    };
    for produced in &descriptor.produces {
        let existing = object
            .get("uri")
            .and_then(J::as_str)
            .is_some_and(|uri| !uri.trim().is_empty());
        if produced.source_path.trim() != "uri" || existing {
            continue;
        }
        if let Some(resource) = binding.resources.iter().find(|resource| {
            !resource.uri.trim().is_empty()
                && crate::resource::Uri::parse(&resource.uri)
                    .is_ok_and(|uri| uri.resource_type == produced.resource_type)
        }) {
            object.insert("uri".into(), J::from(resource.uri.clone()));
        }
    }
    J::Object(object)
}

/// `validateProducedResources`. Provider selectors require the host selector
/// registry, which no pinned provider declares; such a binding fails closed.
fn validate_produced(
    descriptor: &catalog::Descriptor,
    structured: &J,
    tenant: &str,
) -> Result<(), String> {
    if descriptor.produces.is_empty() {
        return Ok(());
    }
    let operation = crate::goerr::quote(&descriptor.operation_id);
    let mut allowed: std::collections::BTreeMap<String, std::collections::BTreeSet<String>> =
        Default::default();
    for produced in &descriptor.produces {
        let path = produced.source_path.trim();
        let kind = produced.resource_type.trim();
        if path.is_empty() || kind.is_empty() {
            return Err(format!(
                "capability {operation} has an incomplete produced resource binding"
            ));
        }
        if !produced.selector_id.is_empty() {
            return Err(format!(
                "capability {operation} selector {} failed: provider selectors are not supported",
                crate::goerr::quote(&produced.selector_id)
            ));
        }
        allowed.entry(path.into()).or_default().insert(kind.into());
    }
    for (path, kinds) in &allowed {
        let segments: Vec<&str> = path.split('.').collect();
        let Some(values) = values_at_path(structured, &segments) else {
            return Err(format!(
                "capability {operation} did not return declared resource output {}",
                crate::goerr::quote(path)
            ));
        };
        if values.is_empty() {
            if segments.iter().any(|segment| segment.ends_with("[]")) {
                continue;
            }
            return Err(format!(
                "capability {operation} did not return declared resource output {}",
                crate::goerr::quote(path)
            ));
        }
        for value in values {
            let J::String(uri) = value else {
                return Err(format!(
                    "capability {operation} produced non-string resource output {}",
                    crate::goerr::quote(path)
                ));
            };
            let parsed = crate::resource::Uri::parse(&uri).map_err(|e| {
                format!(
                    "capability {operation} produced invalid resource output {}: {e}",
                    crate::goerr::quote(path)
                )
            })?;
            if parsed.tenant_id != tenant {
                return Err(format!(
                    "capability {operation} produced foreign-tenant resource output {}",
                    crate::goerr::quote(path)
                ));
            }
            if !kinds.contains(&parsed.resource_type) {
                let declared: Vec<&str> = kinds.iter().map(String::as_str).collect();
                return Err(format!(
                    "capability {operation} produced resource kind {} at {}; declared kinds are [{}]",
                    crate::goerr::quote(&parsed.resource_type),
                    crate::goerr::quote(path),
                    declared.join(" ")
                ));
            }
        }
    }
    Ok(())
}

fn values_at_path(value: &J, segments: &[&str]) -> Option<Vec<J>> {
    let Some((segment, rest)) = segments.split_first() else {
        return Some(vec![value.clone()]);
    };
    let object = value.as_object()?;
    if let Some(key) = segment.strip_suffix("[]") {
        let items = object.get(key)?.as_array()?;
        let mut values = Vec::new();
        for item in items {
            values.extend(values_at_path(item, rest)?);
        }
        return Some(values);
    }
    values_at_path(object.get(*segment)?, rest)
}

/// Go's recordCapabilityInvocation for the built-in compatibility adapters.
/// The raw MCP text is deliberately absent from durable evidence.
fn record_invocation(
    server: &Server,
    name: &str,
    args: &Map<String, J>,
    binding: &Binding,
    reservation: &crate::resource::Reservation,
    result: &J,
) {
    let catalog_snapshot = server.catalog.snapshot();
    // Go records every registered capability, host-internal ones included,
    // under the published catalog revision.
    let Some(descriptor) = catalog_snapshot
        .find(name)
        .or_else(|| catalog::internal().tools.iter().find(|d| d.name == name))
    else {
        return;
    };
    let invalid_result = result
        .pointer("/structuredContent/code")
        .and_then(J::as_str)
        == Some("invalid_result");
    let is_error = result.get("isError") == Some(&J::Bool(true));
    let status = if invalid_result {
        "invalid_result"
    } else if is_error {
        "error"
    } else {
        "success"
    };
    let structured = redact_for_storage(
        result.get("structuredContent").unwrap_or(&J::Null),
        descriptor.output_schema.as_ref().and_then(J::as_object),
    );
    let mut envelope = json!({"isError": is_error, "structured": structured, "content": []});
    let mut observation = json!({
        "schemaVersion": "opute-capability-observation.v1",
        "operationId": name, "capabilityVersion": 1,
        "catalogRevision": catalog_snapshot.revision, "status": status,
    });
    if invalid_result {
        envelope["error"] = json!({"owner": "capability", "code": "invalid_result",
                                   "message": "capability invocation failed"});
    } else {
        if is_error {
            observation["retryability"] = J::from("capability");
        }
        if result
            .get("structuredContent")
            .is_some_and(|v| !v.is_null())
        {
            observation["structured"] = structured;
        }
    }
    let mut execution_binding = json!({
        "schemaVersion": "opute-capability-execution-binding.v1",
        "tenantId": server.host.tenant_id, "admission": "tenant-resource-registry",
        "authorization": "admitted", "catalogRevision": catalog_snapshot.revision,
        "reservationId": reservation.id,
        "resourcePolicyRevision": server.host.coordinator.policy_revision(),
    });
    if !binding.resources.is_empty() {
        let resources: Vec<J> = binding
            .resources
            .iter()
            .filter_map(|bound| {
                let uri = crate::resource::Uri::parse(&bound.uri).ok()?;
                Some(json!({"argument": bound.argument, "uri": bound.uri,
                        "resourceType": uri.resource_type, "tenantId": uri.tenant_id,
                        "resourceId": uri.resource_id, "coordinates": bound.coordinates}))
            })
            .collect();
        execution_binding["resources"] = J::from(resources);
    }
    let record = crate::store::CapabilityInvocationRecord {
        invocation_id: crate::tasks::new_task_id(),
        operation_id: name.into(),
        capability_version: 1,
        catalog_revision: catalog_snapshot.revision.clone(),
        authorization: "admitted".into(),
        arguments_json: redact_for_storage(
            &J::Object(args.clone()),
            descriptor.input_schema.as_object(),
        )
        .to_string(),
        binding_json: execution_binding.to_string(),
        result_json: envelope.to_string(),
        observation_json: observation.to_string(),
        terminal_status: if invalid_result { "error" } else { status }.into(),
        ..Default::default()
    };
    // Go treats a failed audit write as best-effort; execution has completed.
    if let Ok(state) = server.host.state.lock() {
        let _ = state.record_capability_invocation(&record);
    }
}

pub(crate) fn invoke(
    server: &Server,
    ctx: &crate::plan::runner::RunCtx,
    name: &str,
    args: &Map<String, J>,
    binding: &Binding,
) -> J {
    let catalog_snapshot = server.catalog.snapshot();
    // A catalog-excluded tool is dispatchable with its internal descriptor,
    // which never joins the agent-facing snapshot
    // (`registerInternalDispatchTools`).
    let Some(descriptor) = catalog_snapshot
        .find(name)
        .or_else(|| catalog::internal().tools.iter().find(|d| d.name == name))
    else {
        return not_implemented(name);
    };
    // As in Go's registry lookup, an operation no service offers fails
    // closed before its arguments are checked.
    if crate::builtin::resolve(server, name).is_none() {
        return not_implemented(name);
    }
    // The legacy adapter's declarative gate: arguments against the input
    // schema, then a successful structured result against the output schema.
    let input = descriptor
        .input_schema
        .as_object()
        .cloned()
        .unwrap_or_default();
    let checked = crate::schema::arguments_for_validation(&input, args);
    if let Err(e) = crate::schema::validate(&input, &J::Object(checked)) {
        let message = format!("invalid capability arguments: {e}");
        return capability_error("capability", "invalid_arguments", &message);
    }
    let invocation = crate::builtin::Invocation {
        ctx,
        args,
        binding,
        snapshot: &catalog_snapshot,
        claimed: None,
    };
    let Some(outcome) = crate::builtin::invoke(server, name, &invocation) else {
        return not_implemented(name);
    };
    let result = match outcome {
        Ok(result) => result,
        Err(e) => return error_result(&e),
    };
    let output = descriptor
        .output_schema
        .as_ref()
        .and_then(J::as_object)
        .cloned()
        .unwrap_or_default();
    if result.get("isError") != Some(&J::Bool(true)) {
        let structured = result.get("structuredContent").cloned().unwrap_or(J::Null);
        if let Err(e) = crate::schema::validate(&output, &structured) {
            let message = format!(
                "capability {} returned invalid result: structured result does not match output schema: {e}",
                crate::goerr::quote(name)
            );
            return capability_error("capability", "invalid_result", &message);
        }
    }
    result
}

use crate::gojson::Shape;

#[cfg(test)]
#[path = "evidence_fixture.rs"]
mod evidence_fixture;

/// `host.HTTPObservation`.
pub(crate) const HTTP_OBSERVATION: Shape = Shape::Struct(&[
    ("endpoint", Shape::Any),
    ("statusCode", Shape::Any),
    ("ready", Shape::Any),
    ("error", Shape::Any),
]);

pub(crate) const VM_INFO: Shape = Shape::Struct(&[
    ("uri", Shape::Any),
    ("kind", Shape::Any),
    ("name", Shape::Any),
    ("type", Shape::Any),
    ("status", Shape::Any),
    ("state", Shape::Any),
    ("ipv4", Shape::Any),
    ("release", Shape::Any),
    ("providerId", Shape::Any),
    ("cpus", Shape::Any),
    ("memory", Shape::Any),
    ("disk", Shape::Any),
    ("agentReady", Shape::Any),
    ("hostId", Shape::Any),
]);
pub(crate) const VM_LIST: Shape = Shape::Struct(&[("vms", Shape::List(&VM_INFO))]);
const STALL: Shape = Shape::Struct(&[
    ("someAvg10", Shape::Any),
    ("someAvg60", Shape::Any),
    ("someAvg300", Shape::Any),
    ("someTotalUsec", Shape::Any),
    ("fullAvg10", Shape::Any),
    ("fullAvg60", Shape::Any),
    ("fullAvg300", Shape::Any),
    ("fullTotalUsec", Shape::Any),
]);
const LIMITS: &[(&str, Shape)] = &[
    ("cpuCores", Shape::Any),
    ("memoryBytes", Shape::Any),
    ("diskBytes", Shape::Any),
    ("tasks", Shape::Any),
];
const ADMISSION: Shape = Shape::Keys(&[
    ("effectiveLimits", Shape::Struct(LIMITS)),
    (
        "currentUsage",
        Shape::Struct(&[
            ("cpuCores", Shape::Any),
            ("memoryBytes", Shape::Any),
            ("memoryAvailableBytes", Shape::Any),
            ("diskBytes", Shape::Any),
            ("diskAvailableBytes", Shape::Any),
            ("tasks", Shape::Any),
        ]),
    ),
    (
        "reservations",
        Shape::Struct(&[
            ("count", Shape::Any),
            ("cpuCores", Shape::Any),
            ("memoryBytes", Shape::Any),
            ("diskBytes", Shape::Any),
            ("tasks", Shape::Any),
        ]),
    ),
    (
        "queue",
        Shape::Struct(&[
            ("queued", Shape::Any),
            ("heavyQueued", Shape::Any),
            ("normalActive", Shape::Any),
            ("heavyActive", Shape::Any),
        ]),
    ),
    ("psi", Shape::Map(&STALL)),
]);
const CAPACITY: Shape = Shape::Struct(&[
    ("runningVmCount", Shape::Any),
    ("totalVmCount", Shape::Any),
    ("runningVmCpuLimitCores", Shape::Any),
    ("totalVmCpuLimitCores", Shape::Any),
    ("runningVmMemoryLimitBytes", Shape::Any),
    ("totalVmMemoryLimitBytes", Shape::Any),
    ("runningVmDiskLimitBytes", Shape::Any),
    ("totalVmDiskLimitBytes", Shape::Any),
    ("runningQemuCount", Shape::Any),
    ("totalQemuCount", Shape::Any),
    ("runningContainerCount", Shape::Any),
    ("totalContainerCount", Shape::Any),
]);
const ROOT_DISK_QUOTA: Shape = Shape::Struct(&[
    ("pool", Shape::Any),
    ("driver", Shape::Any),
    ("enforced", Shape::Any),
    ("reason", Shape::Any),
]);
const SYSTEM: Shape = Shape::Keys(&[
    ("psi", Shape::Map(&STALL)),
    ("resourceAdmission", ADMISSION),
]);
/// `incus.LocalPrerequisitesResult`.
pub(crate) const LOCAL_PREREQUISITES: Shape = Shape::Struct(&[
    ("provider", Shape::Any),
    ("providerBinary", Shape::Any),
    ("providerReady", Shape::Any),
    ("commands", Shape::Any),
    ("checks", Shape::Any),
    ("rootDiskQuota", ROOT_DISK_QUOTA),
]);
/// `GetLocalStatus`: a map holding the inventory, its capacity and the
/// system metadata.
pub(crate) const LOCAL_STATUS: Shape = Shape::Keys(&[
    ("vms", Shape::List(&VM_INFO)),
    ("capacity", CAPACITY),
    ("system", SYSTEM),
]);
pub(crate) const HOST_INFO: Shape = Shape::Struct(&[
    ("uri", Shape::Any),
    ("hostName", Shape::Any),
    ("providerId", Shape::Any),
    ("lxcBinaryPath", Shape::Any),
    ("systemctlPath", Shape::Any),
    ("supportedTools", Shape::Any),
    ("capacity", CAPACITY),
    ("rootDiskQuota", ROOT_DISK_QUOTA),
    ("system", SYSTEM),
    (
        "agent",
        Shape::Struct(&[
            ("agentId", Shape::Any),
            ("instanceId", Shape::Any),
            ("instanceRoot", Shape::Any),
            ("environmentFile", Shape::Any),
            ("homeDir", Shape::Any),
            ("serviceScope", Shape::Any),
            ("serviceUnitDir", Shape::Any),
            ("serviceWantedBy", Shape::Any),
            ("mcpEndpoint", Shape::Any),
            ("providerRoot", Shape::Any),
        ]),
    ),
]);

/// tools' `structuredResult(value, "")`: the value, and its JSON (as the Go
/// type `shape` marshals) as text.
pub(crate) fn structured_with_text(value: &J, shape: &Shape) -> J {
    let mut text = String::new();
    crate::gojson::encode_shaped(value, shape, &mut text);
    json!({"content": [{"type": "text", "text": text}], "structuredContent": value})
}

/// `host.Service.DescribeHost`.
pub(crate) fn describe_host(host: &Host) -> J {
    let host_name = nix::unistd::gethostname()
        .map(|h: std::ffi::OsString| h.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut result = Map::new();
    let identity = crate::incus::first_non_empty(&[&host.agent_id, &host_name]).to_string();
    if let Ok(uri) = crate::resource::Uri::new("host", &host.tenant_id, &identity) {
        let uri = uri.to_string();
        let mut coordinates = Map::new();
        coordinates.insert("agentId".into(), J::from(host.agent_id.clone()));
        coordinates.insert("hostName".into(), J::from(host_name.clone()));
        let _ = crate::resource::register(&host.state, &host.tenant_id, &uri, &coordinates);
        result.insert("uri".into(), J::from(uri));
    } else {
        result.insert("uri".into(), J::from(""));
    }
    result.insert("hostName".into(), J::from(host_name));
    result.insert("providerId".into(), J::from("incus"));
    result.insert("lxcBinaryPath".into(), J::from(host.incus.binary.clone()));
    result.insert("systemctlPath".into(), J::from("/usr/bin/systemctl"));
    result.insert("supportedTools".into(), json!(catalog::host_tool_names()));
    if let Ok(capacity) = host.incus.inventory_capacity() {
        result.insert("capacity".into(), capacity);
    }
    if let Ok(quota) = host.incus.root_disk_quota() {
        result.insert("rootDiskQuota".into(), quota);
    }
    result.insert(
        "agent".into(),
        crate::hostobs::agent_installation(
            &host.env,
            &host.agent_id,
            &host.instance_id,
            &host.instance_root,
            host.mcp_port,
        ),
    );
    let paths = crate::hostobs::default_disk_paths(&host.env);
    let mut system = crate::hostobs::HostSystemStats::read(&paths)
        .metadata()
        .unwrap_or_default();
    system.insert("resourceAdmission".into(), host.coordinator.metadata());
    result.insert("system".into(), J::Object(system));
    J::Object(result)
}

/// `host.Service.ProbeIncusGPU`: virtualization versions (each recorded
/// only when its command succeeds), WSL GPU device and library presence, and
/// the version gate. Go iterates its command table in map order, so the
/// three commands run in no fixed order there; here they run in key order.
pub(crate) fn probe_incus_gpu(env: &crate::config::Env, args: &Map<String, J>) -> J {
    // exec.Command(...).CombinedOutput(): no deadline.
    const NO_DEADLINE: std::time::Duration = std::time::Duration::from_secs(365 * 24 * 3600);
    let mut out = Map::new();
    for (key, argv) in [
        ("incus", &["incus", "version"][..]),
        ("qemu", &["qemu-system-x86_64", "--version"][..]),
        (
            "virglrenderer",
            &["dpkg-query", "-W", "-f=${Version}", "virglrenderer2"][..],
        ),
    ] {
        let argv: Vec<String> = argv.iter().map(|s| s.to_string()).collect();
        let res = crate::hostobs::run_command(env, &argv, NO_DEADLINE);
        if res.exit_code == 0 {
            let combined = format!("{}{}", res.stdout, res.stderr);
            out.insert(key.into(), J::from(combined.trim()));
        }
    }
    let exists = |p: &str| std::path::Path::new(p).exists();
    let dxg = exists("/dev/dxg");
    let libraries =
        exists("/usr/lib/wsl/lib/libcuda.so") || exists("/usr/lib/wsl/lib/libcuda.so.1");
    let nvidia_smi = crate::hostobs::look_path(env, "nvidia-smi").is_some()
        || exists("/usr/lib/wsl/lib/nvidia-smi");
    out.insert("dxg".into(), J::Bool(dxg));
    out.insert("wslGpuLibraries".into(), J::Bool(libraries));
    out.insert("nvidiaSmi".into(), J::Bool(nvidia_smi));
    out.insert(
        "incusGpuDevice".into(),
        J::Bool(crate::hostobs::look_path(env, "incus").is_some()),
    );
    let incus_ok = version_at_least(out.get("incus"), 7, 2);
    let qemu_required = args
        .get("qemuRequired")
        .and_then(J::as_bool)
        .unwrap_or(false);
    let qemu_version_ok = version_at_least(out.get("qemu"), 11, 0);
    out.insert(
        "versionGate".into(),
        json!({"incusAtLeast7_2": incus_ok, "qemuRequired": qemu_required, "qemuAtLeast11": qemu_version_ok}),
    );
    let status = if !incus_ok || (qemu_required && !qemu_version_ok) {
        "blocked_version_gate"
    } else if dxg && libraries && nvidia_smi {
        "ready_for_host_probe"
    } else {
        "blocked"
    };
    out.insert("status".into(), J::from(status));
    J::Object(out)
}

/// `versionAtLeast(fmt.Sprint(value), major, minor)`: the first
/// `([0-9]+)\.([0-9]+)` match. A missing value prints as `<nil>`.
fn version_at_least(value: Option<&J>, want_major: i64, want_minor: i64) -> bool {
    let text = value.and_then(J::as_str).unwrap_or("<nil>").as_bytes();
    let digits_end = |from: usize| {
        (from..text.len())
            .find(|&k| !text[k].is_ascii_digit())
            .unwrap_or(text.len())
    };
    let mut i = 0;
    while i < text.len() {
        if !text[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let j = digits_end(i);
        if j + 1 < text.len() && text[j] == b'.' && text[j + 1].is_ascii_digit() {
            let k = digits_end(j + 1);
            let number = |a: usize, b: usize| {
                crate::goerr::atoi(std::str::from_utf8(&text[a..b]).unwrap_or("")).0
            };
            let (major, minor) = (number(i, j), number(j + 1, k));
            return major > want_major || (major == want_major && minor >= want_minor);
        }
        // Every start inside this digit run ends at the same place.
        i = j;
    }
    false
}

/// `taskExtensionDeclared`.
fn task_extension_declared(params: Option<&Node>) -> bool {
    let Some(meta) = params.and_then(|p| p.field("_meta")) else {
        return false;
    };
    let capabilities = meta
        .key("io.modelcontextprotocol/clientCapabilities")
        .filter(|c| c.as_object().is_some())
        .or_else(|| meta.key("clientCapabilities"));
    capabilities
        .and_then(|c| c.key("extensions"))
        .filter(|e| e.as_object().is_some())
        .and_then(|e| e.key("io.modelcontextprotocol/tasks"))
        .is_some()
}

/// `missingTasksCapabilityError`: the Tasks extension's -32003.
fn missing_tasks_capability() -> ToolCallOutcome {
    ToolCallOutcome::Protocol(
        -32003,
        "Missing required client capability".into(),
        // ClientCapabilities.Roots is a struct, which omitempty never drops.
        Some(json!({"requiredCapabilities": {
            "extensions": {"io.modelcontextprotocol/tasks": {}},
            "roots": {},
        }})),
    )
}

/// `CapabilityCatalogSnapshot` as published.
pub(crate) fn snapshot_json(snapshot: &catalog::Snapshot) -> J {
    let mut s = String::new();
    snapshot.encode(&mut s);
    serde_json::from_str(&s).expect("snapshot encodes as JSON")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_gate_reads_the_first_dotted_pair() {
        let v = |s: &str| J::from(s);
        assert!(version_at_least(
            Some(&v("Client version: 7.2\nServer version: 7.2")),
            7,
            2
        ));
        assert!(version_at_least(Some(&v("6.21")), 6, 3));
        assert!(!version_at_least(Some(&v("7.1")), 7, 2));
        assert!(version_at_least(
            Some(&v("QEMU emulator version 11.0.1")),
            11,
            0
        ));
        assert!(!version_at_least(Some(&v("1:9.2.1+ds-1")), 11, 0));
        // A run of digits not followed by .<digit> is skipped as a whole.
        assert!(version_at_least(Some(&v("v12x 8.3")), 8, 0));
        assert!(!version_at_least(Some(&v("12.")), 1, 0));
        assert!(!version_at_least(None, 0, 0));
    }

    #[test]
    fn redaction_masks_write_only_fields() {
        let schema: Map<String, J> = serde_json::from_value(json!({
            "properties": {"token": {"writeOnly": true}, "items": {"items": {"properties": {"secret": {"writeOnly": true}}}}},
            "additionalProperties": {"writeOnly": false},
        }))
        .unwrap();
        let value = json!({"token": "t", "items": [{"secret": "s", "keep": 1}], "other": 2});
        assert_eq!(
            redact_for_storage(&value, Some(&schema)),
            // D13: "keep" has no `properties` entry and no typed
            // `additionalProperties` schema on the inner item, so it is
            // redacted too -- only "other" is covered, via the top-level
            // object `additionalProperties` schema.
            json!({"token": "[redacted]", "items": [{"secret": "[redacted]", "keep": "[redacted]"}], "other": 2})
        );
    }

    /// D13 (`redact-unmarked-projections`): the storage projection. A value
    /// reached with no `properties` entry, no typed `additionalProperties`
    /// schema -- even a bare `additionalProperties: true` -- or with no
    /// enclosing schema at all, must be replaced rather than copied, so no
    /// unmarked content ever reaches a durable sink verbatim.
    #[test]
    fn redaction_for_storage_fails_closed_on_unmarked_content() {
        // No schema at all for this call.
        assert_eq!(
            redact_for_storage(&json!({"a": 1}), None),
            json!("[redacted]")
        );

        // additionalProperties: true admits the key but gives it no type.
        let open_schema: Map<String, J> =
            serde_json::from_value(json!({"additionalProperties": true})).unwrap();
        assert_eq!(
            redact_for_storage(&json!({"a": 1}), Some(&open_schema)),
            json!({"a": "[redacted]"})
        );

        // A typed additionalProperties schema still projects normally.
        let typed_schema: Map<String, J> =
            serde_json::from_value(json!({"additionalProperties": {"writeOnly": true}})).unwrap();
        assert_eq!(
            redact_for_storage(&json!({"a": 1}), Some(&typed_schema)),
            json!({"a": "[redacted]"})
        );

        // An array with no `items` schema redacts every element.
        let no_items_schema: Map<String, J> = serde_json::from_value(json!({})).unwrap();
        assert_eq!(
            redact_for_storage(&json!([1, {"a": 1}]), Some(&no_items_schema)),
            json!(["[redacted]", "[redacted]"])
        );

        // properties-covered, non-write-only content is unaffected.
        let covered_schema: Map<String, J> =
            serde_json::from_value(json!({"properties": {"a": {}}})).unwrap();
        assert_eq!(
            redact_for_storage(&json!({"a": 1}), Some(&covered_schema)),
            json!({"a": 1})
        );
    }

    /// The delivery projection is Go's original, unchanged behavior: the
    /// caller already owns this data, so only `writeOnly` is ever hidden.
    /// An open-schema diagnostic field (no `properties` entry, a bare
    /// `additionalProperties: true`, or no enclosing schema at all) passes
    /// through exactly as Go returns it -- the opposite of the storage
    /// projection above, proven here on the identical inputs.
    #[test]
    fn redaction_for_delivery_passes_through_unmarked_content() {
        // No schema at all for this call: nothing to hide.
        assert_eq!(redact_for_delivery(&json!({"a": 1}), None), json!({"a": 1}));

        // additionalProperties: true: the open diagnostic field survives.
        let open_schema: Map<String, J> =
            serde_json::from_value(json!({"additionalProperties": true})).unwrap();
        assert_eq!(
            redact_for_delivery(&json!({"a": 1}), Some(&open_schema)),
            json!({"a": 1})
        );

        // writeOnly is still hidden even though everything else passes.
        let schema: Map<String, J> = serde_json::from_value(json!({
            "additionalProperties": true,
            "properties": {"token": {"writeOnly": true}},
        }))
        .unwrap();
        assert_eq!(
            redact_for_delivery(&json!({"token": "t", "diagnostic": "open"}), Some(&schema)),
            json!({"token": "[redacted]", "diagnostic": "open"})
        );
    }

    /// X3 for the one new sink M5 adds: a persisted task snapshot's
    /// `toolArgs`. Walks the real pinned catalog (not a synthetic schema),
    /// finds every task-aware tool with a top-level `writeOnly` input
    /// property, and proves a unique canary placed in that property never
    /// survives `redact_for_storage` -- the exact function `redact_task_args`
    /// calls before a snapshot reaches `state.db`. This does not cover the
    /// other X3 sinks (WAL/SHM bytes, logs, stdout/stderr, HTTP responses):
    /// only the in-process redaction step for task persistence.
    #[test]
    fn every_task_aware_tools_write_only_fields_are_redacted() {
        let mut checked = 0;
        for standalone in [false, true] {
            let snapshot = catalog::for_mode(standalone);
            for descriptor in &snapshot.tools {
                if !catalog::is_task_aware(&descriptor.name) {
                    continue;
                }
                let Some(props) = descriptor
                    .input_schema
                    .get("properties")
                    .and_then(J::as_object)
                else {
                    continue;
                };
                let write_only_fields: Vec<&String> = props
                    .iter()
                    .filter(|(_, v)| v.get("writeOnly") == Some(&J::Bool(true)))
                    .map(|(k, _)| k)
                    .collect();
                if write_only_fields.is_empty() {
                    continue;
                }
                let canaries: Vec<(String, String)> = write_only_fields
                    .iter()
                    .map(|f| ((*f).clone(), format!("CANARY-{}-{f}", descriptor.name)))
                    .collect();
                let mut args = Map::new();
                for (field, canary) in &canaries {
                    args.insert(field.clone(), J::from(canary.clone()));
                }
                let schema = descriptor.input_schema.as_object();
                let redacted = redact_for_storage(&J::Object(args), schema);
                let serialized = redacted.to_string();
                for (field, canary) in &canaries {
                    assert!(
                        !serialized.contains(canary.as_str()),
                        "{}'s write-only field {field:?} leaked into redacted task \
                         args: {serialized}",
                        descriptor.name,
                    );
                }
                checked += 1;
            }
        }
        // If the catalog ever stops pairing a task-aware tool with a
        // write-only field, this test would pass vacuously; fail loudly
        // instead so the gap in X3 coverage is visible rather than silent.
        assert!(
            checked > 0,
            "no task-aware tool with a write-only input field was found in \
             either catalog mode -- update or remove this test"
        );
    }
}
