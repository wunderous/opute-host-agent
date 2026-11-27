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

/// A minimal OpenAI-compatible endpoint: `/v1/models` lists `fixture`, and a
/// chat completion streams one content chunk, a finish and `[DONE]`.
fn openai_fixture() -> String {
    use std::io::{BufRead, BufReader, Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request = String::new();
            let mut length = 0usize;
            let mut line = String::new();
            while reader.read_line(&mut line).is_ok_and(|n| n > 2) {
                if request.is_empty() {
                    request = line.clone();
                }
                if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = v.trim().parse().unwrap_or(0);
                }
                line.clear();
            }
            let mut body = vec![0u8; length];
            let _ = reader.read_exact(&mut body);
            let mut out = stream;
            let reply = if request.starts_with("GET /v1/models") {
                let json = r#"{"object":"list","data":[{"id":"fixture","object":"model"}]}"#;
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{json}",
                    json.len()
                )
            } else if request.starts_with("POST /v1/chat/completions") {
                let chunk =
                    r#"{"choices":[{"index":0,"delta":{"content":"hi"},"finish_reason":null}]}"#;
                let done = r#"{"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#;
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\ndata: {chunk}\n\ndata: {done}\n\ndata: [DONE]\n\n"
                )
            } else {
                "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into()
            };
            let _ = out.write_all(reply.as_bytes());
        }
    });
    endpoint
}

#[test]
fn openai_chat_activation_probes_under_the_run() {
    // activation-probe-inherits-run-reservation: the activation probe is
    // admitted under the run it validates, as the run's nodes are.
    let host = Host::new();
    let endpoint = openai_fixture();
    let recipe = format!(
        r#"contractVersion: runtime-recipe.v1
recipeId: com.opute.test.cli-activate
recipeVersion: 1.0.0
runtime: {{id: cli-activate, servingContract: openai-chat.v1, capabilities: [llm-serving]}}
activation:
  capability: llm-serving
  servingContract: openai-chat.v1
  inputBindings: {{endpoint: endpoint, modelRef: modelRef}}
inputs:
  endpoint: {{default: "{endpoint}"}}
  modelRef: {{default: fixture}}
plan:
  contractVersion: host-plan.v1
  planId: com.opute.test.cli-activate
  generation: 1
  idempotencyKey: cli-activate-1
  nodes:
    - id: write
      action:
        tool: ensure_host_file
        args: {{path: cli-activate.txt, content: x, scope: user}}
      validate:
        tool: probe_openai_compatible_server
        args: {{endpoint: "${{vars.inputs.endpoint}}", modelRef: "${{vars.inputs.modelRef}}", includeChat: true}}
        assert:
          - {{path: /streamingChatReady, op: eq, value: true}}
"#
    );
    let path = host.dir.path().join("activate.yaml");
    std::fs::write(&path, recipe).unwrap();
    let result = stdout_json(&host.run(&[
        "recipe",
        "apply",
        "--allow-mutations",
        "--activate",
        "--source",
        path.to_str().unwrap(),
    ]));
    assert_eq!(result["status"], "completed", "{result}");
    assert_eq!(
        result["activeRuntime"]["capability"], "llm-serving",
        "{result}"
    );
    assert_eq!(
        result["activeRuntime"]["observation"]["streamingChatReady"], true,
        "{result}"
    );
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
