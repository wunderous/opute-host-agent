//! Command dispatch (`internal/cli`).
//!
//! The command surface, flag sets, precedence and error text follow the Go
//! baseline. `recipe`, `provider` and `public-mcp` validate their arguments
//! and configuration exactly as Go does; the operations they then invoke are
//! implemented by later milestones (M6/M7) and report that explicitly.

use std::io::Write;

use crate::app;
use crate::config::Env;
use crate::goerr::{self, quote, Result};
use crate::goflag::{FlagSet, Values};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub struct Io<'a> {
    pub stdout: &'a mut dyn Write,
    pub stderr: &'a mut dyn Write,
}

impl Io<'_> {
    fn err(&mut self, text: &str) {
        let _ = self.stderr.write_all(text.as_bytes());
        let _ = self.stderr.flush();
    }
}

const COMMANDS: [&str; 6] = [
    "standalone",
    "serve",
    "public-mcp",
    "recipe",
    "provider",
    "help",
];

/// `cli.Run`. The returned error is printed by `main` followed by exit 1.
pub fn run(args: &[String], env: Env, io: &mut Io<'_>) -> Result<()> {
    if args.len() == 1 && (args[0] == "--version" || args[0] == "-version") {
        let _ = writeln!(io.stdout, "{VERSION}");
        return Ok(());
    }
    let (command, rest) = split_command(args);
    match command.as_str() {
        "standalone" => {
            let mut with_mode = vec!["--mode=standalone".to_string()];
            with_mode.extend(rest);
            run_server(&with_mode, env, io)
        }
        "serve" => run_server(&rest, env, io),
        "public-mcp" => run_public_mcp(&rest, env, io),
        "recipe" => run_recipe(&rest, env, io),
        "provider" => run_provider(&rest, env, io),
        "help" => {
            let _ = io.stdout.write_all(USAGE.as_bytes());
            Ok(())
        }
        other => Err(go_err!(
            "unknown command {}; use standalone, serve, public-mcp, recipe, provider, or help",
            quote(other)
        )),
    }
}

fn split_command(args: &[String]) -> (String, Vec<String>) {
    let Some(first) = args.first() else {
        return ("serve".into(), vec![]);
    };
    let trimmed = first.trim();
    if COMMANDS.contains(&trimmed) {
        return (trimmed.into(), args[1..].to_vec());
    }
    // Flags keep the server's historical implicit command.
    if trimmed.starts_with('-') {
        return ("serve".into(), args.to_vec());
    }
    (trimmed.into(), args[1..].to_vec())
}

const USAGE: &str = "Usage: opute-host-agent [standalone|serve|public-mcp|recipe|provider|help] [flags]

  opute-host-agent                   server-only standalone MCP profile (HTTP)
  opute-host-agent serve             MCP server only (HTTP)
  opute-host-agent recipe validate --source ./recipe.yaml
  opute-host-agent recipe apply --source ./recipe.yaml --activate --input model=hf.co/LiquidAI/LFM2-2.6B-GGUF:Q4_K_M
  opute-host-agent recipe status --run-id RUN_ID
  opute-host-agent public-mcp --binding-id ID --endpoint https://host.example/mcp --local-target http://127.0.0.1:3004/mcp --token-file ~/.config/opute/tunnels/ID.env [--origin-host-id ID]
  opute-host-agent provider install --source ./plugin.yaml --activate

Standalone mode never requires Opute Platform.
";

fn parse(fs: &FlagSet, args: &[String], io: &mut Io<'_>) -> Result<Values> {
    let mut diag = String::new();
    let parsed = fs.parse(args, &mut diag);
    io.err(&diag);
    parsed.map_err(|e| goerr::Error(e.message()))
}

fn run_server(args: &[String], mut env: Env, io: &mut Io<'_>) -> Result<()> {
    let fs = FlagSet::new("serve")
        .string("mode", "", "agent profile: standalone or platform")
        .string("transport", "", "MCP transport: http")
        .string("env-file", "", "load KEY=VALUE settings from a file")
        .bool(
            "check",
            false,
            "validate configuration and state access, then exit",
        )
        .multi("env", "set a KEY=VALUE environment override; repeatable");
    let flags = parse(&fs, args, io)?;

    let mut env_file = flags.string("env-file").trim().to_string();
    if env_file.is_empty() {
        env_file = env.get("OPUTE_HOST_AGENT_ENV_FILE").trim().to_string();
    }
    if !env_file.is_empty() {
        env.load_env_file(&env_file)
            .map_err(|e| go_err!("load env file: {e}"))?;
    }
    for assignment in flags.multi("env") {
        match assignment.split_once('=') {
            Some((key, value)) if !key.trim().is_empty() => env.set(key.trim(), value),
            _ => return Err(go_err!("--env requires KEY=VALUE")),
        }
    }
    let mut mode = flags.string("mode").trim().to_string();
    if mode.is_empty() {
        mode = env.get("OPUTE_AGENT_MODE").trim().to_string();
    }
    if mode.is_empty() {
        mode = "standalone".into();
    }
    let mut transport = flags.string("transport").trim().to_string();
    if transport.is_empty() {
        transport = env.get("OPUTE_TRANSPORT").trim().to_string();
    }
    if transport.is_empty() {
        transport = "http".into();
    }
    if !transport.eq_ignore_ascii_case("http") {
        return Err(go_err!(
            "invalid --transport {}: only Streamable HTTP (http) is supported",
            quote(&transport)
        ));
    }
    env.set("OPUTE_AGENT_MODE", &mode);
    env.set("OPUTE_TRANSPORT", &transport);
    if flags.bool("check") {
        app::check(&env)?;
        let _ = writeln!(io.stdout, "configuration ok");
        return Ok(());
    }
    app::run(&env, io.stderr)
}

/// Placeholder for the host operations later milestones implement. It runs
/// only after the same argument and configuration validation Go performs.
fn not_yet_implemented(env: &Env, what: &str, milestone: &str) -> Result<()> {
    let mut runtime = app::new_runtime(env)?;
    runtime.state.close();
    Err(go_err!(
        "{what} is not implemented in this build yet (planned in {milestone})"
    ))
}

fn parse_inputs(values: &[String], env: &Env) -> Result<()> {
    for value in values {
        let Some((key, raw)) = value.split_once('=') else {
            return Err(go_err!("--input requires key=value"));
        };
        if key.trim().is_empty() {
            return Err(go_err!("--input requires key=value"));
        }
        if let Some(name) = raw.strip_prefix("@env:") {
            let name = name.trim();
            if name.is_empty() || env.get(name).is_empty() {
                return Err(go_err!("environment input {} is missing", quote(name)));
            }
        }
    }
    Ok(())
}

fn run_recipe(args: &[String], env: Env, io: &mut Io<'_>) -> Result<()> {
    let Some(subcommand) = args.first() else {
        return Err(go_err!("recipe requires validate, apply, or status"));
    };
    let fs = FlagSet::new(format!("recipe {subcommand}"))
        .string("source", "", "recipe path or pinned remote source")
        .string("revision", "", "immutable source revision")
        .string(
            "sha256",
            "",
            "expected raw recipe sha256, as hex or sha256:<hex>",
        )
        .string("kind", "runtime", "recipe family: runtime or tunnel")
        .string("run-id", "", "durable recipe run ID")
        .bool("resume", false, "resume a persisted recipe run")
        .bool(
            "wait",
            true,
            "wait for recipe apply to reach a terminal state",
        )
        .bool(
            "activate",
            false,
            "after successful validation, make this runtime active for its declared capability",
        )
        .multi("input", "recipe input as key=value; repeatable");
    let flags = parse(&fs, &args[1..], io)?;
    let sub = subcommand.as_str();
    if !matches!(sub, "validate" | "apply" | "status") {
        return Err(go_err!(
            "unknown recipe command {}; use validate, apply, or status",
            quote(sub)
        ));
    }
    if sub == "apply" && !flags.bool("wait") {
        return Err(go_err!(
            "recipe apply always waits for a terminal result; use the MCP operation directly for asynchronous execution"
        ));
    }
    let kind = flags.string("kind");
    if kind != "runtime" && kind != "tunnel" {
        return Err(go_err!("recipe kind must be runtime or tunnel"));
    }
    if sub == "status" {
        if flags.string("run-id").trim().is_empty() {
            return Err(go_err!("recipe status requires --run-id"));
        }
    } else {
        if flags.string("source").trim().is_empty() {
            return Err(go_err!("recipe {sub} requires --source"));
        }
        parse_inputs(&flags.multi("input"), &env)?;
    }
    not_yet_implemented(&env, "recipe operations", "M6")
}

fn run_provider(args: &[String], env: Env, io: &mut Io<'_>) -> Result<()> {
    let Some(subcommand) = args.first() else {
        return Err(go_err!(
            "provider requires install, validate, status, or reload"
        ));
    };
    let fs = FlagSet::new(format!("provider {subcommand}"))
        .string("source", "", "trusted local provider descriptor path")
        .string("endpoint", "", "provider MCP endpoint override")
        .string("token", "", "provider bearer token")
        .string(
            "mode",
            "",
            "provider recipe mode, such as managed or external",
        )
        .string("provider", "", "connected provider ID")
        .string("operation", "", "provider validation operation")
        .bool(
            "activate",
            false,
            "after successful validation, make this provider active",
        )
        .multi("input", "provider input as key=value; repeatable");
    let flags = parse(&fs, &args[1..], io)?;
    let sub = subcommand.as_str();
    if !matches!(sub, "install" | "validate" | "status" | "reload") {
        return Err(go_err!(
            "unknown provider command {}; use install, validate, status, or reload",
            quote(sub)
        ));
    }
    parse_inputs(&flags.multi("input"), &env)?;
    if (sub == "install" || sub == "reload") && flags.string("source").is_empty() {
        return Err(go_err!("provider {sub} requires --source"));
    }
    if (sub == "validate" || sub == "status") && flags.string("provider").is_empty() {
        return Err(go_err!("provider {sub} requires --provider"));
    }
    not_yet_implemented(&env, "provider operations", "M7")
}

fn run_public_mcp(args: &[String], mut env: Env, io: &mut Io<'_>) -> Result<()> {
    let fs = FlagSet::new("public-mcp")
        .string("env-file", "", "load Host Agent configuration from a file")
        .string(
            "binding-id",
            "",
            "provider-issued public exposure binding ID",
        )
        .string("endpoint", "", "stable HTTPS MCP endpoint, ending in /mcp")
        .string(
            "local-target",
            "",
            "Host Agent MCP origin, ending in /mcp; non-loopback targets require --origin-host-id",
        )
        .string(
            "origin-host-id",
            "",
            "exact enrolled origin Host Agent identity for a non-loopback MCP origin",
        )
        .string(
            "token-file",
            "",
            "0600 file containing OPUTE_CLOUDFLARED_TUNNEL_TOKEN",
        )
        .string(
            "artifact-uri",
            "",
            "pinned cloudflared artifact URI override",
        )
        .string(
            "artifact-sha256",
            "",
            "pinned cloudflared artifact SHA-256 override",
        )
        .string(
            "artifact-path",
            "",
            "Opute-owned cloudflared artifact path override",
        )
        .string(
            "service-name",
            "",
            "Opute-owned connector service name override",
        )
        .string(
            "service-file",
            "",
            "Opute-owned connector service file override",
        )
        .string("scope", "user", "systemd service scope: user or system");
    let flags = parse(&fs, args, io)?;
    let env_file = flags.string("env-file").trim().to_string();
    if !env_file.is_empty() {
        env.load_env_file(&env_file)
            .map_err(|e| go_err!("load env file: {e}"))?;
    }
    if ["binding-id", "endpoint", "local-target", "token-file"]
        .iter()
        .any(|k| flags.string(k).trim().is_empty())
    {
        return Err(go_err!(
            "public-mcp requires --binding-id, --endpoint, --local-target, and --token-file"
        ));
    }
    read_tunnel_token(&flags.string("token-file"))?;
    not_yet_implemented(&env, "public MCP exposure", "M8a")
}

/// `readPublicMcpTunnelToken`.
fn read_tunnel_token(path: &str) -> Result<String> {
    use std::os::unix::fs::PermissionsExt;
    let path = path.trim();
    if path.is_empty() {
        return Err(go_err!("token file is required"));
    }
    let p = std::path::Path::new(path);
    let meta = std::fs::metadata(p).map_err(|e| {
        go_err!(
            "inspect public MCP token file: {}",
            goerr::path_error("stat", p, &e)
        )
    })?;
    if !meta.is_file() || meta.permissions().mode() & 0o077 != 0 {
        return Err(go_err!(
            "public MCP token file must be a regular 0600-or-more-restrictive file"
        ));
    }
    let data = std::fs::read(p).map_err(|e| {
        go_err!(
            "read public MCP token file: {}",
            goerr::path_error("open", p, &e)
        )
    })?;
    for line in String::from_utf8_lossy(&data).split('\n') {
        let line = line.trim();
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() != "OPUTE_CLOUDFLARED_TUNNEL_TOKEN" {
            continue;
        }
        let value = value.trim();
        if value.is_empty() {
            return Err(go_err!(
                "public MCP token file contains an empty tunnel token"
            ));
        }
        return Ok(value.to_string());
    }
    Err(go_err!(
        "public MCP token file did not contain OPUTE_CLOUDFLARED_TUNNEL_TOKEN"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_capture(
        args: &[&str],
        env: &[(&str, &str)],
    ) -> (std::result::Result<(), String>, String, String) {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let result = {
            let mut io = Io {
                stdout: &mut out,
                stderr: &mut err,
            };
            run(&args, Env::from_pairs(env.iter().copied()), &mut io).map_err(|e| e.0)
        };
        (
            result,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    #[test]
    fn version_only_as_sole_argument() {
        assert_eq!(run_capture(&["--version"], &[]).1, format!("{VERSION}\n"));
        let (r, _, _) = run_capture(&["--version", "x"], &[]);
        assert_eq!(r, Err("flag provided but not defined: -version".into()));
    }

    #[test]
    fn version_tracks_the_source_lock() {
        let lock: serde_json::Value =
            serde_json::from_str(include_str!("../../../baseline/source-lock.json")).unwrap();
        let package = lock["publishedPackage"].as_str().unwrap();
        assert_eq!(package.rsplit('@').next().unwrap(), VERSION);
    }

    #[test]
    fn dispatch_errors_match_go() {
        assert_eq!(
            run_capture(&["bogus"], &[]).0,
            Err("unknown command \"bogus\"; use standalone, serve, public-mcp, recipe, provider, or help".into())
        );
        assert_eq!(
            run_capture(&["recipe"], &[]).0,
            Err("recipe requires validate, apply, or status".into())
        );
        assert_eq!(
            run_capture(&["recipe", "bogus"], &[]).0,
            Err("unknown recipe command \"bogus\"; use validate, apply, or status".into())
        );
        assert_eq!(
            run_capture(&["recipe", "apply", "--source", "x", "--wait=false"], &[]).0,
            Err("recipe apply always waits for a terminal result; use the MCP operation directly for asynchronous execution".into())
        );
        assert_eq!(
            run_capture(&["recipe", "status"], &[]).0,
            Err("recipe status requires --run-id".into())
        );
        assert_eq!(
            run_capture(
                &["recipe", "validate", "--source", "x", "--input", "k"],
                &[]
            )
            .0,
            Err("--input requires key=value".into())
        );
        assert_eq!(
            run_capture(
                &[
                    "recipe",
                    "validate",
                    "--source",
                    "x",
                    "--input",
                    "k=@env:NOPE"
                ],
                &[]
            )
            .0,
            Err("environment input \"NOPE\" is missing".into())
        );
        assert_eq!(
            run_capture(&["provider", "install"], &[]).0,
            Err("provider install requires --source".into())
        );
        assert_eq!(
            run_capture(&["provider", "status"], &[]).0,
            Err("provider status requires --provider".into())
        );
        assert_eq!(
            run_capture(&["public-mcp"], &[]).0,
            Err(
                "public-mcp requires --binding-id, --endpoint, --local-target, and --token-file"
                    .into()
            )
        );
        assert_eq!(
            run_capture(&["serve", "--env", "NOEQ"], &[]).0,
            Err("--env requires KEY=VALUE".into())
        );
        assert_eq!(
            run_capture(&["--transport=ws"], &[]).0,
            Err("invalid --transport \"ws\": only Streamable HTTP (http) is supported".into())
        );
        assert_eq!(
            run_capture(&["serve"], &[("OPUTE_TRANSPORT", "stdio")]).0,
            Err("invalid --transport \"stdio\": only Streamable HTTP (http) is supported".into())
        );
    }

    #[test]
    fn help_goes_to_stdout_and_flag_help_to_stderr() {
        let (r, out, _) = run_capture(&["help"], &[]);
        assert_eq!((r, out.as_str()), (Ok(()), USAGE));
        let (r, out, err) = run_capture(&["serve", "-h"], &[]);
        assert_eq!(r, Err("flag: help requested".into()));
        assert!(out.is_empty());
        assert!(err.starts_with("Usage of serve:\n"));
    }

    #[test]
    fn token_file_rules() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.env");
        std::fs::write(
            &path,
            "# c\nOTHER=1\nOPUTE_CLOUDFLARED_TUNNEL_TOKEN= abc \n",
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let p = path.to_str().unwrap();
        assert_eq!(
            read_tunnel_token(p).unwrap_err().0,
            "public MCP token file must be a regular 0600-or-more-restrictive file"
        );
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(read_tunnel_token(p).unwrap(), "abc");
        assert_eq!(
            read_tunnel_token("/nonexistent").unwrap_err().0,
            "inspect public MCP token file: stat /nonexistent: no such file or directory"
        );
    }
}
