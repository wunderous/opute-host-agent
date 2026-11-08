//! The plain process primitives the host domain shares
//! (`internal/exec/process.go`, `internal/hostruntime/workload.go`):
//! `exec.CommandContext(...).CombinedOutput()` with Go's error texts, the
//! non-interactive privileged package runner, and the systemd workload unit.

use std::io::Read;
use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::process::{Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use crate::config::Env;
use crate::hostobs::{look_path, CommandResult};

/// `(*exec.ExitError).Error()` / `os.ProcessState.String()`.
pub fn exit_text(status: ExitStatus) -> String {
    if let Some(code) = status.code() {
        return format!("exit status {code}");
    }
    let signal = status.signal().unwrap_or(0);
    let name = nix::sys::signal::Signal::try_from(signal)
        .map(go_signal_name)
        .unwrap_or_else(|_| format!("signal {signal}"));
    if status.core_dumped() {
        format!("signal: {name} (core dumped)")
    } else {
        format!("signal: {name}")
    }
}

/// Go's `syscall.Signal.String` for the signals a child can die of.
fn go_signal_name(signal: nix::sys::signal::Signal) -> String {
    use nix::sys::signal::Signal::*;
    match signal {
        SIGHUP => "hangup",
        SIGINT => "interrupt",
        SIGQUIT => "quit",
        SIGILL => "illegal instruction",
        SIGTRAP => "trace/breakpoint trap",
        SIGABRT => "aborted",
        SIGBUS => "bus error",
        SIGFPE => "floating point exception",
        SIGKILL => "killed",
        SIGUSR1 => "user defined signal 1",
        SIGSEGV => "segmentation fault",
        SIGUSR2 => "user defined signal 2",
        SIGPIPE => "broken pipe",
        SIGALRM => "alarm clock",
        SIGTERM => "terminated",
        other => return format!("signal {}", other as i32),
    }
    .to_string()
}

/// `exec.CommandContext(ctx, name, args...).CombinedOutput()`: stdout and
/// stderr share one pipe, the context kills only the direct child, and a
/// failure is Go's error text.
pub fn combined_output(
    env: &Env,
    argv: &[String],
    deadline: Option<Instant>,
) -> (Vec<u8>, Result<(), String>) {
    let (output, _, result) = run_piped(env, argv, deadline, false);
    (output, result)
}

/// As `combined_output`, with stderr kept apart: (stdout, stderr, result).
pub fn split_output(
    env: &Env,
    argv: &[String],
    deadline: Option<Instant>,
) -> (Vec<u8>, Vec<u8>, Result<(), String>) {
    run_piped(env, argv, deadline, true)
}

fn run_piped(
    env: &Env,
    argv: &[String],
    deadline: Option<Instant>,
    split: bool,
) -> (Vec<u8>, Vec<u8>, Result<(), String>) {
    let name = &argv[0];
    let program = if name.contains('/') {
        Some(PathBuf::from(name))
    } else {
        look_path(env, name)
    };
    let Some(program) = program else {
        return (
            Vec::new(),
            Vec::new(),
            Err(format!(
                "exec: {}: executable file not found in $PATH",
                crate::goerr::quote(name)
            )),
        );
    };
    if deadline.is_some_and(|d| Instant::now() >= d) {
        return (
            Vec::new(),
            Vec::new(),
            Err("context deadline exceeded".into()),
        );
    }
    let (reader, writer) = match nix::unistd::pipe() {
        Ok(pair) => pair,
        Err(e) => return (Vec::new(), Vec::new(), Err(e.desc().to_lowercase())),
    };
    // Split: stderr gets a pipe of its own; else it shares stdout's.
    let (err_reader, writer2) = if split {
        match nix::unistd::pipe() {
            Ok((r, w)) => (Some(r), w),
            Err(e) => return (Vec::new(), Vec::new(), Err(e.desc().to_lowercase())),
        }
    } else {
        match writer.try_clone() {
            Ok(w) => (None, w),
            Err(e) => return (Vec::new(), Vec::new(), Err(crate::goerr::errno_text(&e))),
        }
    };
    let mut cmd = Command::new(&program);
    cmd.args(&argv[1..])
        .env_clear()
        .envs(env.pairs())
        .stdin(Stdio::null())
        .stdout(Stdio::from(writer))
        .stderr(Stdio::from(writer2));
    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            return (
                Vec::new(),
                Vec::new(),
                Err(format!(
                    "fork/exec {}: {}",
                    program.display(),
                    crate::goerr::errno_text(&e)
                )),
            )
        }
    };
    // Drop the parent's write ends so the reader sees EOF when the child exits.
    drop(cmd);
    let collect = |fd: std::os::fd::OwnedFd| {
        let mut pipe = std::fs::File::from(fd);
        std::thread::spawn(move || {
            let mut out = Vec::new();
            let _ = pipe.read_to_end(&mut out);
            out
        })
    };
    let collector = collect(reader);
    let err_collector = err_reader.map(collect);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if deadline.is_some_and(|d| Instant::now() >= d) => {
                let _ = child.kill();
                break child.wait().ok();
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(_) => break None,
        }
    };
    let output = collector.join().unwrap_or_default();
    let errors = err_collector
        .map(|c| c.join().unwrap_or_default())
        .unwrap_or_default();
    let result = match status {
        Some(status) if status.success() => Ok(()),
        Some(status) => Err(exit_text(status)),
        None => Err("wait: no child processes".into()),
    };
    (output, errors, result)
}

/// `hostexec.RunPrivilegedPackage`: `sudo -n <command> <args>`; a failure is
/// the trimmed combined output, else the process error.
pub fn run_privileged_package(
    env: &Env,
    deadline: Instant,
    command: &str,
    args: &[&str],
) -> Result<(), String> {
    let mut argv = vec!["sudo".to_string(), "-n".to_string(), command.to_string()];
    argv.extend(args.iter().map(|a| a.to_string()));
    let (output, result) = combined_output(env, &argv, Some(deadline));
    result.map_err(|err| {
        let message = String::from_utf8_lossy(&output).trim().to_string();
        if message.is_empty() {
            err
        } else {
            message
        }
    })
}

/// `HostCommandRunnerContext` with a zero timeout under a caller deadline:
/// Go reports the expired context as a timeout "after 0s".
pub fn run_host_until(env: &Env, argv: &[String], deadline: Instant) -> CommandResult {
    crate::hostobs::run_command_until(env, argv, Some(deadline), Duration::ZERO)
}

const SYSTEMD_RUN: &str = "/usr/bin/systemd-run";

/// `workloadUnitName`: the scope and argv hashed into a unit name.
fn workload_unit_name(scope: &str, command: &[String]) -> String {
    use sha2::{Digest, Sha256};
    let mut seed = format!("{}\0{}", scope.trim(), command.join("\0"));
    if scope.trim().is_empty() {
        seed.push('\0');
        seed.push_str(&crate::hostobs::rfc3339_nano_now());
    }
    let digest = hex::encode(Sha256::digest(seed.as_bytes()));
    format!("opute-host-workload-{}", &digest[..24])
}

/// The workload scope of a call: its operation and its task, so two tasks
/// never share a unit (Q29, decided 2026-11-08). A call with neither gets a
/// unit of its own from the clock (`workload_unit_name`).
pub fn workload_scope(ctx: &crate::plan::runner::RunCtx) -> String {
    let (operation, task) = ctx.operation_identity();
    [operation, task]
        .into_iter()
        .filter(|part| !part.trim().is_empty())
        .collect::<Vec<_>>()
        .join("/")
}

/// Q29: a cancelled or timed-out workload stops its unit. Killing the
/// `systemd-run --wait` client alone leaves the unit and its command running.
fn stop_workload_unit_if_interrupted(env: &Env, unit: &str, interrupted: bool, exit_code: i32) {
    if !interrupted && exit_code != 124 {
        return;
    }
    let mut argv = vec!["systemctl".to_string()];
    if !nix::unistd::geteuid().is_root() {
        argv.push("--user".into());
    }
    argv.extend(["stop".into(), unit.to_string()]);
    let budget = Duration::from_secs(40);
    let _ = crate::hostobs::run_command_with(
        env,
        &argv,
        None,
        Some(Instant::now() + budget),
        budget,
        None,
    );
}

/// `Runtime.RunHostWorkloadContext`: one bounded, killable systemd unit.
pub fn run_host_workload(
    env: &Env,
    scope: &str,
    command: &[String],
    timeout: Duration,
    ctx: Option<&crate::plan::runner::RunCtx>,
) -> Result<CommandResult, String> {
    if command.first().is_none_or(|c| c.trim().is_empty()) {
        return Err("workload command is required".into());
    }
    let timeout = if timeout.is_zero() {
        Duration::from_secs(2 * 60 * 60)
    } else {
        timeout
    };
    let seconds = timeout.as_secs() + u64::from(timeout.subsec_nanos() != 0);
    let unit = workload_unit_name(scope, command);
    let mut argv: Vec<String> = vec![
        SYSTEMD_RUN.into(),
        format!("--unit={unit}"),
        "--wait".into(),
        "--collect".into(),
        "--pipe".into(),
        "--property=Slice=opute-workload.slice".into(),
        "--property=KillMode=control-group".into(),
        "--property=MemoryHigh=10G".into(),
        "--property=MemoryMax=11G".into(),
        "--property=MemorySwapMax=1G".into(),
        "--property=CPUQuota=600%".into(),
        "--property=CPUWeight=100".into(),
        "--property=TasksMax=4096".into(),
        format!("--property=RuntimeMaxSec={}", seconds.max(1)),
        "--property=TimeoutStopSec=30s".into(),
    ];
    if !nix::unistd::geteuid().is_root() {
        argv.insert(1, "--user".into());
    }
    argv.extend(command.iter().cloned());
    let budget = timeout + Duration::from_secs(30);
    let result = crate::hostobs::run_command_with(
        env,
        &argv,
        None,
        Some(Instant::now() + budget),
        budget,
        ctx,
    );
    let interrupted = ctx.is_some_and(|c| c.err().is_some());
    stop_workload_unit_if_interrupted(env, &unit, interrupted, result.exit_code);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_texts_follow_go() {
        assert_eq!(exit_text(ExitStatus::from_raw(3 << 8)), "exit status 3");
        assert_eq!(exit_text(ExitStatus::from_raw(9)), "signal: killed");
        assert_eq!(exit_text(ExitStatus::from_raw(15)), "signal: terminated");
    }

    #[test]
    fn durations_print_like_go() {
        use crate::hostobs::go_duration;
        let ms = Duration::from_millis;
        assert_eq!(go_duration(Duration::ZERO), "0s");
        assert_eq!(go_duration(ms(1500)), "1.5s");
        assert_eq!(go_duration(ms(250)), "250ms");
        assert_eq!(go_duration(Duration::from_micros(1500)), "1.5ms");
        assert_eq!(go_duration(Duration::from_secs(90)), "1m30s");
        assert_eq!(go_duration(Duration::from_secs(7230)), "2h0m30s");
        assert_eq!(go_duration(ms(60_500)), "1m0.5s");
    }

    /// Q29: two tasks running the same command get units of their own.
    #[test]
    fn two_tasks_with_the_same_command_get_their_own_units() {
        use crate::plan::runner::RunCtx;
        let argv = vec![
            "bash".to_string(),
            "-lc".to_string(),
            "sleep 120".to_string(),
        ];
        let first = RunCtx::background().with_operation_identity("run_host_command", "task-1");
        let second = RunCtx::background().with_operation_identity("run_host_command", "task-2");
        assert_ne!(
            workload_unit_name(&workload_scope(&first), &argv),
            workload_unit_name(&workload_scope(&second), &argv)
        );
    }

    /// Q29: a cancelled workload stops its unit, not only the systemd-run client.
    #[test]
    fn a_cancelled_workload_stops_its_unit() {
        use crate::plan::runner::RunCtx;
        use std::os::unix::fs::PermissionsExt;
        if !std::path::Path::new(SYSTEMD_RUN).exists() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("systemctl.log");
        let fake = dir.path().join("systemctl");
        std::fs::write(
            &fake,
            format!("#!/bin/sh\necho \"$@\" >> {}\n", log.display()),
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let env = Env::from_pairs([("PATH", dir.path().to_str().unwrap())]);
        let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let ctx = RunCtx::from_flag(flag).with_operation_identity("run_host_command", "task-1");
        let argv = vec![
            "bash".to_string(),
            "-lc".to_string(),
            "sleep 120".to_string(),
        ];
        let unit = workload_unit_name(&workload_scope(&ctx), &argv);
        run_host_workload(
            &env,
            &workload_scope(&ctx),
            &argv,
            Duration::from_secs(60),
            Some(&ctx),
        )
        .unwrap();
        let calls = std::fs::read_to_string(&log).unwrap_or_default();
        assert!(
            calls.lines().any(|l| l.ends_with(&format!("stop {unit}"))),
            "{calls:?}"
        );
    }

    /// A workload that completes leaves its unit to systemd (--collect).
    #[test]
    fn a_completed_workload_is_not_stopped() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("systemctl.log");
        let fake = dir.path().join("systemctl");
        std::fs::write(
            &fake,
            format!("#!/bin/sh\necho \"$@\" >> {}\n", log.display()),
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let env = Env::from_pairs([("PATH", dir.path().to_str().unwrap())]);
        stop_workload_unit_if_interrupted(&env, "opute-host-workload-x", false, 0);
        assert!(!log.exists());
    }

    #[test]
    fn workload_units_hash_scope_and_argv() {
        let argv = vec!["bash".to_string(), "-lc".to_string(), "true".to_string()];
        let a = workload_unit_name("s", &argv);
        assert_eq!(a, workload_unit_name("s", &argv));
        assert!(a.starts_with("opute-host-workload-") && a.len() == 44);
    }
}
