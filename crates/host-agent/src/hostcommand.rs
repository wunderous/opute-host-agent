//! Caller-declared commands (`internal/domain/host/exec_command.go`,
//! `host_service.go:RunWorkloadCommandContext`, and the shared
//! `run_host_command`/`agent_shell` handler in `dispatch_unassigned.go`).
//!
//! Host commands run in an operation-scoped systemd workload unit. Guest
//! commands run through the virtualization provider's ownership-checked exec,
//! on a resolved `vm:` or `container:` URI that is never a display name.

use std::time::Duration;

use serde_json::{json, Map, Value as J};

use crate::admission::Resolver;
use crate::config::Env;
use crate::incus::{first_non_empty, Incus};
use crate::plan::runner::RunCtx;

const DEFAULT_EXEC_TIMEOUT: Duration = Duration::from_secs(30);

fn exec_timeout(timeout_ms: i64) -> Duration {
    if timeout_ms > 0 {
        Duration::from_millis(timeout_ms as u64)
    } else {
        DEFAULT_EXEC_TIMEOUT
    }
}

/// `Service.RunInstanceCommandContext`.
#[allow(clippy::too_many_arguments)]
pub fn run_instance_command(
    resolve: &Resolver,
    incus: &Incus,
    ctx: &RunCtx,
    uri: &str,
    command: &str,
    args: &[String],
    stdin: &str,
    timeout_ms: i64,
) -> Result<J, String> {
    if let Some(err) = ctx.err() {
        return Err(err);
    }
    let (uri, coordinates) = resolve(uri.trim(), "")?;
    if uri.resource_type != "vm" && uri.resource_type != "container" {
        return Err(format!(
            "instance command requires vm or container URI, got {}",
            crate::goerr::quote(&uri.resource_type)
        ));
    }
    let provider_name = match coordinates.get("providerInstanceName") {
        Some(J::String(name)) if !name.trim().is_empty() => name.clone(),
        _ => {
            return Err(format!(
                "resource {uri} has no provider instance coordinate"
            ))
        }
    };
    let command = command.trim();
    if command.is_empty() {
        return Err("command is required".into());
    }
    let mut argv = vec![command.to_string()];
    argv.extend(args.iter().cloned());
    let input = (!stdin.is_empty()).then_some(stdin.as_bytes());
    let res = incus.run_vm_exec(&provider_name, &argv, input, exec_timeout(timeout_ms), ctx)?;
    let output = if res.stdout.is_empty() {
        res.stderr.clone()
    } else {
        res.stdout.clone()
    };
    Ok(json!({
        "uri": uri.to_string(),
        "instanceType": coordinates.get("instanceType").cloned().unwrap_or(J::Null),
        "exitCode": res.exit_code,
        "stdout": res.stdout,
        "stderr": res.stderr,
        "output": output,
    }))
}

/// `Service.ExecCommand`: the internal guest exec on the bound instance.
pub fn exec_command(
    incus: &Incus,
    vm_name: &str,
    command: &str,
    args: &[String],
    timeout_ms: i64,
) -> Result<J, String> {
    let vm_name = vm_name.trim();
    if vm_name.is_empty() {
        return Err("name is required".into());
    }
    let command = command.trim();
    if command.is_empty() {
        return Err("command is required".into());
    }
    let mut argv = vec![command.to_string()];
    argv.extend(args.iter().cloned());
    let res = incus.run_vm_exec(
        vm_name,
        &argv,
        None,
        exec_timeout(timeout_ms),
        &RunCtx::background(),
    )?;
    if res.exit_code != 0 {
        return Err(first_non_empty(&[
            &res.stderr,
            &res.stdout,
            &format!("command failed with exit {}", res.exit_code),
        ])
        .to_string());
    }
    let output = if res.stdout.is_empty() {
        res.stderr
    } else {
        res.stdout
    };
    Ok(json!({ "output": output }))
}

/// The `run_host_command` / `agent_shell` handler: one opaque shell command
/// in the operation's workload unit, answered with its exit status.
pub fn run_host_command(
    env: &Env,
    ctx: &RunCtx,
    args: &Map<String, J>,
    timeout_ms: i64,
) -> Result<J, String> {
    let command = args.get("command").and_then(J::as_str).unwrap_or("").trim();
    if command.is_empty() {
        return Err("command is required".into());
    }
    if timeout_ms < 0 {
        return Err("timeoutMs must be non-negative".into());
    }
    if timeout_ms > 2 * 60 * 60 * 1000 {
        return Err("timeoutMs exceeds the two-hour maximum".into());
    }
    let (operation, task) = ctx.operation_identity();
    let scope = if operation.is_empty() {
        task
    } else {
        operation
    };
    let res = crate::hostexec::run_host_workload(
        env,
        &scope,
        &["bash".into(), "-lc".into(), command.into()],
        Duration::from_millis(timeout_ms as u64),
        Some(ctx),
    )?;
    let text = if !res.stdout.is_empty() {
        res.stdout.clone()
    } else if !res.stderr.is_empty() {
        res.stderr.clone()
    } else {
        format!("exit {}", res.exit_code)
    };
    let mut result = json!({
        "content": [{"type": "text", "text": text}],
        "structuredContent": {"exitCode": res.exit_code, "stdout": res.stdout, "stderr": res.stderr},
    });
    if res.exit_code != 0 {
        result["isError"] = J::Bool(true);
    }
    Ok(result)
}
