//! `cli-operation-commands`: `recipe`, `provider` and `public-mcp` call the
//! agent's published tools in process, through the same admission, mutation
//! authority and plan runner as an MCP client, and print the structured
//! result. Each test runs the real binary on its own state.

use std::path::Path;
use std::process::{Command, Output};

/// A runtime recipe whose one node writes a file under the user's home.
const RECIPE: &str = r#"contractVersion: runtime-recipe.v1
recipeId: com.opute.test.cli-apply
recipeVersion: 1.0.0
runtime: {id: cli-apply, servingContract: kubernetes.v1}
inputs:
  word: {required: true}
plan:
  contractVersion: host-plan.v1
  planId: com.opute.test.cli-apply
  generation: 1
  idempotencyKey: cli-apply-${vars.inputs.word}
  nodes:
    - id: write
      action:
        tool: ensure_host_file
        args: {path: cli-apply.txt, content: "${vars.inputs.word}", scope: user}
      validate:
        tool: inspect_host_file
        args: {path: cli-apply.txt, scope: user}
        assert:
          - {path: /exists, op: eq, value: true}
"#;

struct Host {
    dir: tempfile::TempDir,
}

impl Host {
    fn new() -> Host {
        let host = Host {
            dir: tempfile::tempdir().unwrap(),
        };
        std::fs::create_dir_all(host.home()).unwrap();
        std::fs::write(host.dir.path().join("recipe.yaml"), RECIPE).unwrap();
        host
    }

    fn home(&self) -> std::path::PathBuf {
        self.dir.path().join("home")
    }

    fn recipe(&self) -> String {
        self.dir.path().join("recipe.yaml").display().to_string()
    }

    fn run(&self, args: &[&str]) -> Output {
        command(&self.home(), &self.dir.path().join("state"), args)
            .output()
            .unwrap()
    }
}

fn command(home: &Path, state: &Path, args: &[&str]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_opute-host-agent"));
    cmd.args(args)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("OPUTE_REMOTE_AGENT_ID", "cli-operations-test-agent")
        .env("OPUTE_HOST_AGENT_INSTANCE_ROOT", home.join("instance"))
        .env("OPUTE_STANDALONE_STATE_DIR", state)
        // Resource enforcement needs a cgroup slice the test host may not
        // have, and its capacity a host this size; admission is not what
        // these tests check.
        .env("OPUTE_HOST_RESOURCE_ENFORCEMENT", "advisory")
        .env("OPUTE_HOST_RESOURCE_CPU_CAPACITY_CORES", "64")
        .env("OPUTE_HOST_RESOURCE_MEMORY_CAPACITY_BYTES", "68719476736");
    cmd
}

fn stdout_json(out: &Output) -> serde_json::Value {
    assert!(
        out.status.success(),
        "exit {:?}\nstdout: {}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn recipe_validate_prints_the_tool_result() {
    let host = Host::new();
    let out = host.run(&[
        "recipe",
        "validate",
        "--source",
        &host.recipe(),
        "--input",
        "word=hello",
    ]);
    let result = stdout_json(&out);
    assert_eq!(result["recipeId"], "com.opute.test.cli-apply", "{result}");
    // Indented as Go's json.MarshalIndent writes it.
    assert!(String::from_utf8_lossy(&out.stdout).starts_with("{\n  \""));
}

#[test]
fn recipe_apply_without_the_grant_is_refused() {
    let host = Host::new();
    let out = host.run(&[
        "recipe",
        "apply",
        "--source",
        &host.recipe(),
        "--input",
        "word=hello",
    ]);
    assert!(!out.status.success());
    assert!(out.stdout.is_empty());
    let err = stderr(&out);
    assert!(
        err.contains("recipe operation failed: Error: mutation refused: run_runtime_recipe"),
        "{err}"
    );
    assert!(!host.home().join("cli-apply.txt").exists());
}

#[test]
fn recipe_apply_waits_for_the_run_and_its_effect() {
    let host = Host::new();
    let out = host.run(&[
        "recipe",
        "apply",
        "--allow-mutations",
        "--source",
        &host.recipe(),
        "--input",
        "word=hello",
    ]);
    let result = stdout_json(&out);
    assert_eq!(result["status"], "completed", "{result}");
    assert_eq!(
        std::fs::read_to_string(host.home().join("cli-apply.txt")).unwrap(),
        "hello"
    );
    let run_id = result["runId"].as_str().unwrap().to_string();
    let status = stdout_json(&host.run(&["recipe", "status", "--run-id", &run_id]));
    assert_eq!(status["status"], "completed", "{status}");
    assert_eq!(status["runId"], run_id.as_str());
}

#[test]
fn provider_status_reports_through_the_tool() {
    let host = Host::new();
    let status = stdout_json(&host.run(&["provider", "status", "--provider", "nope"]));
    assert_eq!(
        status,
        serde_json::json!({"active": false, "connected": false, "providerId": "nope"})
    );
}

#[test]
fn provider_install_failure_names_the_tool_error() {
    let host = Host::new();
    let missing = host.dir.path().join("missing.yaml");
    let out = host.run(&[
        "provider",
        "install",
        "--allow-mutations",
        "--source",
        missing.to_str().unwrap(),
    ]);
    assert!(!out.status.success());
    assert!(out.stdout.is_empty());
    let err = stderr(&out);
    assert!(err.contains("provider operation failed: Error: "), "{err}");
    assert!(!err.contains("not implemented"), "{err}");
}

#[test]
fn public_mcp_without_the_grant_is_refused() {
    use std::os::unix::fs::PermissionsExt;
    let host = Host::new();
    let token = host.dir.path().join("tunnel.env");
    std::fs::write(&token, "OPUTE_CLOUDFLARED_TUNNEL_TOKEN=not-a-real-token\n").unwrap();
    std::fs::set_permissions(&token, std::fs::Permissions::from_mode(0o600)).unwrap();
    let out = host.run(&[
        "public-mcp",
        "--binding-id",
        "b",
        "--endpoint",
        "https://host.example/mcp",
        "--local-target",
        "http://127.0.0.1:3004/mcp",
        "--token-file",
        token.to_str().unwrap(),
    ]);
    assert!(!out.status.success());
    let err = stderr(&out);
    assert!(
        err.contains(
            "public MCP operation failed: Error: mutation refused: ensure_public_mcp_tunnel"
        ),
        "{err}"
    );
    assert!(!err.contains("not-a-real-token"), "{err}");
}

#[test]
fn a_running_agent_holds_the_instance() {
    let host = Host::new();
    let state = host.dir.path().join("state");
    let _lock = {
        // The agent's own instance lock, as a running agent holds it.
        let out = host.run(&["serve", "--check"]);
        assert!(out.status.success(), "{}", stderr(&out));
        let file = std::fs::File::open(state.join("host-agent.lock"))
            .or_else(|_| std::fs::File::open(host.home().join("instance/host-agent.lock")))
            .unwrap();
        nix::fcntl::Flock::lock(file, nix::fcntl::FlockArg::LockExclusiveNonblock).unwrap()
    };
    let out = host.run(&["provider", "status", "--provider", "nope"]);
    assert!(!out.status.success());
    assert!(
        stderr(&out).contains("host agent instance is already in use"),
        "{}",
        stderr(&out)
    );
}
