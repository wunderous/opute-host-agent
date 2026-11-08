//! The Platform boundary (`platform-neutral-host-agent`): the agent knows no
//! orchestrator. Non-test source, the published descriptors and schemas, the
//! default configuration and the CLI help carry no Platform identifier.
//!
//! The product name, the binary and the `OPUTE_` prefix are allowed. Test
//! code is not scanned: a file named `tests.rs` or `*_tests.rs`, a module
//! `lib.rs` declares under `#[cfg(test)]`, and the rest of a file from its
//! first `#[cfg(test)]` module on.

use std::path::{Path, PathBuf};

/// Identifiers that tell Platform-managed hosts apart from others, or name
/// Platform's own endpoints and clients.
const FORBIDDEN: &[&str] = &[
    "OPUTE_AGENT_MODE",
    "OPUTE_STANDALONE_ALLOW_MUTATIONS",
    "agent_mode",
    // `platform` as a behaviour switch; a host's OS platform is not one.
    "== \"platform\"",
    "\"platform\" =>",
    "\"platform\".into()",
    "\"platform\".to_string()",
    "(\"platform\")",
    "= \"platform\";",
    "opute.io",
    "opute-mcp-host",
    "opute-platform",
    "Opute Platform",
    "Platform",
];

/// Recorded exceptions: (file, identifier, why it is not a Platform switch).
const ALLOWED: &[(&str, &str, &str)] = &[(
    "src/providers/hostos.rs",
    "== \"platform\"",
    "the host's OS platform field of detect_host_platform",
)];

/// The forbidden identifiers in `text`, ignoring a capitalised `Platform`
/// that is part of a longer word (`HostPlatform`).
fn findings(text: &str) -> Vec<&'static str> {
    FORBIDDEN
        .iter()
        .copied()
        .filter(|needle| {
            if *needle != "Platform" {
                return text.contains(needle);
            }
            text.match_indices(needle).any(|(i, _)| {
                let before = text[..i].chars().next_back();
                let after = text[i + needle.len()..].chars().next();
                !before.is_some_and(char::is_alphanumeric)
                    && !after.is_some_and(char::is_alphanumeric)
            })
        })
        .collect()
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Modules `lib.rs` compiles only for tests.
fn test_modules(root: &Path) -> Vec<String> {
    let lib = std::fs::read_to_string(root.join("src/lib.rs")).unwrap();
    lib.split("#[cfg(test)]\nmod ")
        .skip(1)
        .filter_map(|rest| rest.split(';').next())
        .map(|name| format!("{name}.rs"))
        .collect()
}

/// The non-test part of a source file, or None for a test-only file.
fn non_test(path: &Path, test_only: &[String]) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    if name == "tests.rs" || name.ends_with("_tests.rs") || test_only.iter().any(|t| t == name) {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    let cut = text
        .match_indices("#[cfg(test)]\nmod ")
        .map(|(i, _)| i)
        .next()
        .unwrap_or(text.len());
    Some(text[..cut].to_string())
}

#[test]
#[ignore = "red until the single profile, mode removal and OAuth clients land (tasks 3.1-3.3, owner questions N1-N7)"]
fn no_platform_identifier_crosses_the_boundary() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut report = Vec::new();

    let test_only = test_modules(root);
    let mut files = Vec::new();
    rust_files(&root.join("src"), &mut files);
    files.sort();
    for path in files {
        let Some(text) = non_test(&path, &test_only) else {
            continue;
        };
        for (n, line) in text.lines().enumerate() {
            let rel = path.strip_prefix(root).unwrap().display().to_string();
            for found in findings(line) {
                if ALLOWED
                    .iter()
                    .any(|(file, id, _)| *file == rel && *id == found)
                {
                    continue;
                }
                report.push(format!("{rel}:{}: {found}", n + 1));
            }
        }
    }

    // The published descriptors and schemas come from these files.
    for name in ["source.json", "neutral-names.json", "write-only.json"] {
        let text = std::fs::read_to_string(root.join("catalog").join(name)).unwrap();
        for found in findings(&text) {
            report.push(format!("catalog/{name}: {found}"));
        }
    }

    let defaults = format!(
        "{:?}",
        crate::config::Config::load(&crate::config::Env::default())
    );
    for found in findings(&defaults) {
        report.push(format!("default configuration: {found}"));
    }

    let mut help = Vec::new();
    let _ = crate::cli::run(
        &["help".to_string()],
        crate::config::Env::default(),
        &mut crate::cli::Io {
            stdout: &mut help,
            stderr: &mut Vec::new(),
        },
    );
    for found in findings(&String::from_utf8_lossy(&help)) {
        report.push(format!("CLI help: {found}"));
    }

    assert!(
        report.is_empty(),
        "Platform identifiers cross the boundary:\n{}",
        report.join("\n")
    );
}

#[test]
fn the_scanner_finds_what_it_forbids() {
    assert_eq!(findings("a Platform default"), vec!["Platform"]);
    assert!(findings("detect_host_platform HostPlatform").is_empty());
    assert!(findings(r#"text(call.args, "platform")"#).is_empty());
    assert_eq!(
        findings(r#"if mode == "platform" {"#),
        vec![r#"== "platform""#]
    );
    assert_eq!(findings("mcp.opute.io"), vec!["opute.io"]);
    assert!(findings("opute-host-agent OPUTE_HOST_MCP_PORT").is_empty());
}
