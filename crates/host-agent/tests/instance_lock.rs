//! The D25 writer lock at the process boundary, mirroring Go's
//! internal/cli/instance_lock_test.go and the SIGKILL case of
//! internal/instancelock/lock_test.go: a second agent on one instance exits 1
//! with one stderr line, nothing on stdout, no store and no port, while the
//! first keeps serving; a killed agent leaves no stale lock.

use nix::fcntl::{Flock, FlockArg};
use nix::sys::signal::{kill, Signal};
use nix::unistd::Pid;
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const LOCK_FILE: &str = "host-agent.lock";

struct AgentEnv {
    instance_root: PathBuf,
    state_dir: Option<PathBuf>,
    port: u16,
}

impl AgentEnv {
    fn command(&self, home: &Path, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_opute-host-agent"));
        cmd.args(args)
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("HOME", home)
            .env("XDG_CONFIG_HOME", home.join(".config"))
            .env("OPUTE_REMOTE_AGENT_ID", "instance-lock-test-agent")
            .env("OPUTE_INFRA_PROVIDER_ID", "incus")
            .env(
                "MCP_AUTH_TOKEN",
                "instance-lock-test-token-0123456789abcdef",
            )
            .env("OPUTE_HOST_AGENT_INSTANCE_ROOT", &self.instance_root)
            .env("HOST_MCP_PORT", self.port.to_string());
        if let Some(dir) = &self.state_dir {
            cmd.env("OPUTE_STANDALONE_STATE_DIR", dir);
        }
        cmd
    }
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn port_bound(port: u16) -> bool {
    TcpStream::connect_timeout(
        &format!("127.0.0.1:{port}").parse().unwrap(),
        Duration::from_millis(200),
    )
    .is_ok()
}

/// The test's own hold on a root, as another agent would hold it.
fn hold(root: &Path) -> Flock<std::fs::File> {
    std::fs::create_dir_all(root).unwrap();
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join(LOCK_FILE))
        .unwrap();
    Flock::lock(file, FlockArg::LockExclusiveNonblock)
        .map_err(|(_, e)| e)
        .unwrap()
}

fn refusal(root: &Path) -> String {
    format!(
        "host agent instance is already in use: {}/{LOCK_FILE} is locked by another process",
        root.display()
    )
}

/// The agent exits 1 with exactly the refusal line on stderr and nothing on
/// stdout.
fn run_refused(mut cmd: Command, locked_root: &Path) {
    let out = cmd.output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(1), "stderr:\n{stderr}");
    assert_eq!(stderr.trim_end_matches('\n'), refusal(locked_root));
    assert!(out.stdout.is_empty(), "stdout = {:?}", out.stdout);
}

fn entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn start_serving(cmd: &mut Command, port: u16) -> Child {
    let mut child = cmd
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while !port_bound(port) {
        if Instant::now() > deadline || child.try_wait().unwrap().is_some() {
            let _ = child.kill();
            let out = child.wait_with_output().unwrap();
            panic!(
                "agent did not start serving; stderr:\n{}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    child
}

fn stop(mut child: Child, signal: Signal) {
    kill(Pid::from_raw(child.id() as i32), signal).unwrap();
    let _ = child.wait();
}

#[test]
fn second_agent_on_instance_is_refused_before_any_store_or_port() {
    let dir = tempfile::tempdir().unwrap();
    let env = AgentEnv {
        instance_root: dir.path().join("instance"),
        state_dir: None,
        port: free_port(),
    };
    let _held = hold(&env.instance_root);
    run_refused(
        env.command(&dir.path().join("home"), &["serve"]),
        &env.instance_root,
    );
    assert_eq!(entries(&env.instance_root), [LOCK_FILE]);
    assert!(
        !port_bound(env.port),
        "refused agent bound port {}",
        env.port
    );
}

#[test]
fn second_agent_is_refused_while_first_keeps_serving() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("instance");
    let first = AgentEnv {
        instance_root: root.clone(),
        state_dir: None,
        port: free_port(),
    };
    let child = start_serving(
        &mut first.command(&dir.path().join("home1"), &["serve"]),
        first.port,
    );
    let second = AgentEnv {
        instance_root: root.clone(),
        state_dir: None,
        port: free_port(),
    };
    run_refused(second.command(&dir.path().join("home2"), &["serve"]), &root);
    assert!(
        !port_bound(second.port),
        "refused agent bound port {}",
        second.port
    );
    assert!(
        port_bound(first.port),
        "first agent stopped serving after the refusal"
    );
    stop(child, Signal::SIGINT);
}

#[test]
fn state_directory_outside_instance_root_is_locked() {
    let dir = tempfile::tempdir().unwrap();
    let env = AgentEnv {
        instance_root: dir.path().join("instance"),
        state_dir: Some(dir.path().join("state")),
        port: free_port(),
    };
    let state_dir = env.state_dir.clone().unwrap();
    let _held = hold(&state_dir);
    run_refused(
        env.command(&dir.path().join("home"), &["serve"]),
        &state_dir,
    );
    assert_eq!(entries(&state_dir), [LOCK_FILE]);
    // The instance root is locked first and released on refusal; it keeps
    // only its lock file.
    assert_eq!(entries(&env.instance_root), [LOCK_FILE]);
}

#[test]
fn check_refuses_while_instance_is_locked() {
    let dir = tempfile::tempdir().unwrap();
    let env = AgentEnv {
        instance_root: dir.path().join("instance"),
        state_dir: None,
        port: free_port(),
    };
    let home = dir.path().join("home");
    let held = hold(&env.instance_root);
    run_refused(
        env.command(&home, &["serve", "--check"]),
        &env.instance_root,
    );
    drop(held);
    let out = env.command(&home, &["serve", "--check"]).output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        "configuration ok"
    );
}

#[test]
fn killed_agent_leaves_no_stale_lock() {
    let dir = tempfile::tempdir().unwrap();
    let env = AgentEnv {
        instance_root: dir.path().join("instance"),
        state_dir: None,
        port: free_port(),
    };
    let home = dir.path().join("home");
    let child = start_serving(&mut env.command(&home, &["serve"]), env.port);
    run_refused(
        env.command(&home, &["serve", "--check"]),
        &env.instance_root,
    );
    stop(child, Signal::SIGKILL);
    let child = start_serving(&mut env.command(&home, &["serve"]), env.port);
    stop(child, Signal::SIGINT);
}
