//! D55 (`per-client-mutation-authority`) at the process boundary: the real
//! binary, its HTTP surface and its operator CLI. Mutation authority belongs
//! to each caller: an OAuth token's `read`/`mutate` scope, the bootstrap
//! bearer only with `--allow-mutations`. No environment setting grants it,
//! a refusal names the grant that would admit the call, and `tools/list`
//! annotations follow each tool's effect.

use base64::Engine;
use sha2::Digest;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const BEARER: &str = "mutation-authority-test-bearer-0123456789abcdef";
/// A mutation (`ensure_host_file`, effect `mutation`) called without
/// arguments: once admitted it fails on its own input, before any effect.
const MUTATION: &str = "ensure_host_file";
const REFUSED: &str = "mutation refused: ";

struct Agent {
    child: Child,
    port: u16,
    state: PathBuf,
    home: PathBuf,
    _dir: tempfile::TempDir,
}

impl Drop for Agent {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A free port no other test in this process has drawn: tests run in
/// parallel, and a port handed out twice would let one test's readiness
/// probe reach another test's agent.
fn free_port() -> u16 {
    static TAKEN: std::sync::Mutex<Vec<u16>> = std::sync::Mutex::new(Vec::new());
    let mut taken = TAKEN.lock().unwrap();
    loop {
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        if !taken.contains(&port) {
            taken.push(port);
            return port;
        }
    }
}

fn command(home: &Path, state: &Path, port: u16, env: &[(&str, &str)], args: &[&str]) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_opute-host-agent"));
    cmd.args(args)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("OPUTE_REMOTE_AGENT_ID", "mutation-authority-test-agent")
        .env("OPUTE_HOST_AGENT_INSTANCE_ROOT", home.join("instance"))
        .env("OPUTE_STANDALONE_STATE_DIR", state)
        .env("MCP_AUTH_TOKEN", BEARER)
        .env("HOST_MCP_PORT", port.to_string());
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd
}

/// Whether this agent, and not another listener, answers on `port`.
fn serving(port: u16) -> bool {
    let Ok(mut stream) = TcpStream::connect(("127.0.0.1", port)) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let req = format!(
        "GET /.well-known/oauth-protected-resource/mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n"
    );
    let mut raw = String::new();
    stream.write_all(req.as_bytes()).is_ok()
        && stream.read_to_string(&mut raw).is_ok()
        && raw.starts_with("HTTP/1.1 200")
        && raw.contains(&format!("127.0.0.1:{port}/mcp"))
}

/// Start the agent on a fresh port. Each agent also opens provider listeners
/// on ephemeral ports, so a drawn port can be taken before the agent binds
/// it: that start is retried on another port.
fn start(env: &[(&str, &str)], args: &[&str]) -> Agent {
    for _ in 0..5 {
        let dir = tempfile::tempdir().unwrap();
        let (home, state, port) = (
            dir.path().join("home"),
            dir.path().join("state"),
            free_port(),
        );
        let log = dir.path().join("agent.log");
        let mut child = command(&home, &state, port, env, args)
            .stdout(Stdio::null())
            .stderr(std::fs::File::create(&log).unwrap())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if child.try_wait().unwrap().is_some() {
                let stderr = std::fs::read_to_string(&log).unwrap_or_default();
                if stderr.contains("address already in use")
                    || stderr.contains("Address already in use")
                {
                    break;
                }
                panic!("agent exited before serving; stderr:\n{stderr}");
            }
            if serving(port) {
                return Agent {
                    child,
                    port,
                    state,
                    home,
                    _dir: dir,
                };
            }
            if Instant::now() > deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!(
                    "agent did not start serving; stderr:\n{}",
                    std::fs::read_to_string(&log).unwrap_or_default()
                );
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    panic!("no free port held for the agent in five attempts");
}

struct Reply {
    status: u16,
    headers: String,
    body: String,
}

fn http(port: u16, method: &str, path: &str, headers: &[(&str, &str)], body: &str) -> Reply {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(60)))
        .unwrap();
    let mut req = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\nContent-Length: {}\r\n",
        body.len()
    );
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    req.push_str(body);
    stream.write_all(req.as_bytes()).unwrap();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).unwrap();
    let text = String::from_utf8_lossy(&raw).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status = head
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let body = if head
        .to_ascii_lowercase()
        .contains("transfer-encoding: chunked")
    {
        dechunk(body)
    } else {
        body.to_string()
    };
    Reply {
        status,
        headers: head.to_string(),
        body,
    }
}

fn dechunk(mut body: &str) -> String {
    let mut out = String::new();
    while let Some((size, rest)) = body.split_once("\r\n") {
        let n = usize::from_str_radix(size.trim(), 16).unwrap_or(0);
        if n == 0 {
            break;
        }
        out.push_str(&rest[..n]);
        body = rest[n..].trim_start_matches("\r\n");
    }
    out
}

fn rpc(agent: &Agent, token: &str, method: &str, params: serde_json::Value) -> serde_json::Value {
    let mut params = params;
    params["_meta"] = serde_json::json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": {},
        "io.modelcontextprotocol/clientInfo": {"name": "mutation-authority-test", "version": "1"}
    });
    let name = params["name"].as_str().unwrap_or_default().to_string();
    let body = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params})
        .to_string();
    let auth = format!("Bearer {token}");
    let mut headers = vec![
        ("Authorization", auth.as_str()),
        ("Content-Type", "application/json"),
        ("Accept", "application/json, text/event-stream"),
        ("MCP-Protocol-Version", "2026-07-28"),
        ("Mcp-Method", method),
    ];
    if !name.is_empty() {
        headers.push(("Mcp-Name", name.as_str()));
    }
    let reply = http(agent.port, "POST", "/mcp", &headers, &body);
    assert_eq!(
        reply.status, 200,
        "{method}: {} {}",
        reply.headers, reply.body
    );
    let json = reply
        .body
        .lines()
        .find_map(|l| l.strip_prefix("data: "))
        .unwrap_or(&reply.body)
        .to_string();
    serde_json::from_str(&json).unwrap()
}

/// The text of a `tools/call` result, after the `Error: ` every tool error
/// starts with. A refusal is also typed `mutation_refused`.
fn call(agent: &Agent, token: &str, tool: &str) -> String {
    let reply = rpc(
        agent,
        token,
        "tools/call",
        serde_json::json!({"name": tool, "arguments": {}}),
    );
    let result = &reply["result"];
    let text = result["content"][0]["text"].as_str().unwrap_or_default();
    let text = text.strip_prefix("Error: ").unwrap_or(text).to_string();
    if text.starts_with(REFUSED) {
        assert_eq!(
            result["structuredContent"]["code"], "mutation_refused",
            "{result}"
        );
        assert_eq!(result["isError"], true, "{result}");
    }
    text
}

fn admitted(text: &str) -> bool {
    !text.contains(REFUSED) && !text.contains("mutations are disabled")
}

fn form(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| {
            let enc: String = v
                .bytes()
                .map(|b| match b {
                    b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                        (b as char).to_string()
                    }
                    _ => format!("%{b:02X}"),
                })
                .collect();
            format!("{k}={enc}")
        })
        .collect::<Vec<_>>()
        .join("&")
}

fn resource(agent: &Agent) -> String {
    format!("http://127.0.0.1:{}/mcp", agent.port)
}

fn secret(agent: &Agent, client: &str) -> String {
    let raw = std::fs::read_to_string(
        agent
            .state
            .join("credentials")
            .join(format!("{client}.json")),
    )
    .unwrap();
    serde_json::from_str::<serde_json::Value>(&raw).unwrap()["client_secret"]
        .as_str()
        .unwrap()
        .to_string()
}

/// `client_credentials` for a listed client; `scope` empty asks for nothing.
fn client_token(agent: &Agent, client: &str, scope: &str) -> Reply {
    let basic = base64::engine::general_purpose::STANDARD
        .encode(format!("{client}:{}", secret(agent, client)));
    let auth = format!("Basic {basic}");
    let mut pairs = vec![("grant_type", "client_credentials"), ("resource", "")];
    let res = resource(agent);
    pairs[1].1 = &res;
    if !scope.is_empty() {
        pairs.push(("scope", scope));
    }
    http(
        agent.port,
        "POST",
        "/oauth/token",
        &[
            ("Authorization", &auth),
            ("Content-Type", "application/x-www-form-urlencoded"),
        ],
        &form(&pairs),
    )
}

fn access_token(reply: &Reply) -> String {
    assert_eq!(reply.status, 200, "{} {}", reply.headers, reply.body);
    serde_json::from_str::<serde_json::Value>(&reply.body).unwrap()["access_token"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn bearer_is_read_only_without_the_flag() {
    let agent = start(&[], &["serve"]);
    assert!(admitted(&call(&agent, BEARER, "get_host_info")));
    let text = call(&agent, BEARER, MUTATION);
    assert!(text.starts_with(REFUSED), "{text}");
    for part in [MUTATION, "mutation", "read-only", "--allow-mutations"] {
        assert!(text.contains(part), "missing {part:?} in {text}");
    }
}

#[test]
fn bearer_mutates_with_the_flag() {
    let agent = start(&[], &["serve", "--allow-mutations"]);
    let text = call(&agent, BEARER, MUTATION);
    assert!(admitted(&text), "{text}");
}

#[test]
fn a_leftover_setting_grants_nothing() {
    let agent = start(
        &[
            ("OPUTE_HOST_ALLOW_MUTATIONS", "true"),
            ("OPUTE_STANDALONE_ALLOW_MUTATIONS", "true"),
            ("OPUTE_AGENT_MODE", "standalone"),
        ],
        &["serve"],
    );
    let text = call(&agent, BEARER, MUTATION);
    assert!(text.starts_with(REFUSED), "{text}");
}

#[test]
fn read_and_mutate_clients_on_one_agent() {
    let agent = start(
        &[("OPUTE_OAUTH_CLIENTS", "ro:read,rw:mutate,plain")],
        &["serve"],
    );
    let ro = access_token(&client_token(&agent, "ro", ""));
    let rw = access_token(&client_token(&agent, "rw", ""));
    let plain = access_token(&client_token(&agent, "plain", ""));
    assert!(admitted(&call(&agent, &ro, "get_host_info")));
    let refused = call(&agent, &ro, MUTATION);
    assert!(refused.starts_with(REFUSED), "{refused}");
    for part in [
        MUTATION,
        "mutation",
        "read",
        "OPUTE_OAUTH_CLIENTS=ro:mutate",
        "scope mutate",
    ] {
        assert!(refused.contains(part), "missing {part:?} in {refused}");
    }
    let plain_refused = call(&agent, &plain, MUTATION);
    assert!(
        plain_refused.contains("OPUTE_OAUTH_CLIENTS=plain:mutate"),
        "{plain_refused}"
    );
    let text = call(&agent, &rw, MUTATION);
    assert!(admitted(&text), "{text}");
}

#[test]
fn the_flag_does_not_raise_a_read_client() {
    let agent = start(
        &[("OPUTE_OAUTH_CLIENTS", "ro:read")],
        &["serve", "--allow-mutations"],
    );
    let ro = access_token(&client_token(&agent, "ro", ""));
    assert!(call(&agent, &ro, MUTATION).starts_with(REFUSED));
}

#[test]
fn a_token_above_the_grant_is_refused() {
    let agent = start(&[("OPUTE_OAUTH_CLIENTS", "ro:read,rw:mutate")], &["serve"]);
    let above = client_token(&agent, "ro", "mutate");
    assert_eq!(above.status, 400, "{}", above.body);
    assert!(above.body.contains("invalid_scope"), "{}", above.body);
    let rw_read = access_token(&client_token(&agent, "rw", "read"));
    assert!(call(&agent, &rw_read, MUTATION).starts_with(REFUSED));
}

#[test]
fn the_former_scope_fails_the_start() {
    let dir = tempfile::tempdir().unwrap();
    let mut child = command(
        &dir.path().join("home"),
        &dir.path().join("state"),
        free_port(),
        &[("OPUTE_OAUTH_CLIENTS", "ci:mcp")],
        &["serve"],
    )
    .stdout(Stdio::null())
    .stderr(Stdio::piped())
    .spawn()
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("the agent started serving with the former scope mcp");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let out = child.wait_with_output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_ne!(out.status.code(), Some(0), "{stderr}");
    assert!(
        stderr.contains("read") && stderr.contains("mutate"),
        "{stderr}"
    );
}

#[test]
fn protected_resource_metadata_lists_read_and_mutate() {
    let agent = start(&[], &["serve"]);
    let reply = http(
        agent.port,
        "GET",
        "/.well-known/oauth-protected-resource/mcp",
        &[],
        "",
    );
    let doc: serde_json::Value = serde_json::from_str(&reply.body).unwrap();
    assert_eq!(
        doc["scopes_supported"],
        serde_json::json!(["read", "mutate"])
    );
}

#[test]
fn annotations_follow_the_effect() {
    let agent = start(&[], &["serve"]);
    let tools = rpc(&agent, BEARER, "tools/list", serde_json::json!({}))["result"]["tools"]
        .as_array()
        .unwrap()
        .clone();
    assert!(!tools.is_empty());
    for tool in tools {
        let effect = tool["_meta"]["capability"]["effect"]
            .as_str()
            .unwrap_or_default();
        let hints = &tool["annotations"];
        assert_eq!(
            hints["readOnlyHint"],
            serde_json::json!(effect == "read"),
            "{}",
            tool["name"]
        );
        if effect != "read" {
            assert_eq!(
                hints["destructiveHint"],
                serde_json::json!(effect == "destructive"),
                "{}",
                tool["name"]
            );
        }
    }
}

fn oauth_cli(agent: &Agent, args: &[&str]) -> String {
    let state = agent.state.display().to_string();
    let mut all = vec!["oauth"];
    all.extend_from_slice(args);
    all.extend_from_slice(&["--state-dir", &state]);
    let out = command(&agent.home, &agent.state, agent.port, &[], &all)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// The bootstrap client's interactive flow: authorize, the operator approves
/// through the CLI, the client redeems its code.
fn interactive_token(agent: &Agent, scope: &str) -> (String, String) {
    let verifier = "mutation-authority-test-verifier-0123456789abcdefghijklmnop";
    let challenge =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(verifier));
    let res = resource(agent);
    let mut pairs = vec![
        ("response_type", "code"),
        ("client_id", "host-agent-bootstrap"),
        ("redirect_uri", "http://127.0.0.1/callback"),
        ("resource", res.as_str()),
        ("code_challenge", challenge.as_str()),
        ("code_challenge_method", "S256"),
        ("state", "s1"),
    ];
    if !scope.is_empty() {
        pairs.push(("scope", scope));
    }
    let page = http(
        agent.port,
        "POST",
        "/oauth/authorize",
        &[("Content-Type", "application/x-www-form-urlencoded")],
        &form(&pairs),
    );
    assert_eq!(page.status, 200, "{}", page.body);
    let request = page
        .body
        .split("request=")
        .nth(1)
        .and_then(|s| s.split(['"', '&', '\'']).next())
        .unwrap()
        .to_string();
    let pending = oauth_cli(agent, &["pending"]);
    let code = pending.split_whitespace().next().unwrap().to_string();
    oauth_cli(agent, &["approve", "--once", &code]);
    let status = http(
        agent.port,
        "GET",
        &format!("/oauth/authorize/status?request={request}"),
        &[],
        "",
    );
    let location = status
        .headers
        .lines()
        .find_map(|l| {
            l.strip_prefix("Location: ")
                .or_else(|| l.strip_prefix("location: "))
        })
        .unwrap()
        .to_string();
    let auth_code = location
        .split("code=")
        .nth(1)
        .and_then(|s| s.split('&').next())
        .unwrap()
        .to_string();
    let token = http(
        agent.port,
        "POST",
        "/oauth/token",
        &[("Content-Type", "application/x-www-form-urlencoded")],
        &form(&[
            ("grant_type", "authorization_code"),
            ("code", &auth_code),
            ("client_id", "host-agent-bootstrap"),
            ("redirect_uri", "http://127.0.0.1/callback"),
            ("code_verifier", verifier),
            ("resource", &res),
        ]),
    );
    (access_token(&token), pending)
}

#[test]
fn an_interactive_client_gets_the_approved_scope() {
    let agent = start(&[], &["serve", "--allow-mutations"]);
    let (read, pending) = interactive_token(&agent, "");
    assert!(pending.contains("read"), "{pending}");
    let refused = call(&agent, &read, MUTATION);
    assert!(refused.starts_with(REFUSED), "{refused}");
    assert!(
        refused.contains("scope mutate") && refused.contains("oauth approve"),
        "{refused}"
    );
    let (mutate, pending) = interactive_token(&agent, "mutate");
    assert!(pending.contains("mutate"), "{pending}");
    assert!(admitted(&call(&agent, &mutate, MUTATION)));
}
