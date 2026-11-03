//! The verified K3s install (Go `bootstrap.go`): the release binary and the
//! installer are downloaded on the provider's host, the binary checked
//! against the release checksums, staged into the guest, and the official
//! installer run there with downloads disabled.

use super::run_command;
use std::io::Write as _;
use std::time::Duration;

const BOOTSTRAP_SCRIPT_URL: &str = "https://get.k3s.io";
const RELEASE_URL: &str = "https://github.com/k3s-io/k3s/releases/download/";
const MAX_BOOTSTRAP_SCRIPT: u64 = 1 << 20;
const MAX_BINARY_BYTES: u64 = 256 << 20;
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const DIAGNOSTIC_TIMEOUT: Duration = Duration::from_secs(10);

pub(super) fn install_k3s_server(
    instance: &str,
    version: &str,
    install_exec: &str,
    address: &str,
    token: &str,
) -> Result<(), String> {
    let (installer, binary, checksum) = download_artifacts(version)?;
    let (mut file, temporary) = crate::goerr::create_temp(
        &std::env::temp_dir().display().to_string(),
        "opute-k3s-",
        "",
    )
    .map_err(|e| format!("create staged K3s binary: {e}"))?;
    let result = (|| {
        file.write_all(&binary).map_err(|e| {
            format!(
                "stage K3s binary: write {temporary}: {}",
                crate::goerr::errno_text(&e)
            )
        })?;
        drop(file);
        stage_and_install(
            instance,
            version,
            install_exec,
            address,
            token,
            &temporary,
            &checksum,
            installer,
        )
    })();
    let _ = std::fs::remove_file(&temporary);
    result
}

#[allow(clippy::too_many_arguments)]
fn stage_and_install(
    instance: &str,
    version: &str,
    install_exec: &str,
    address: &str,
    token: &str,
    temporary: &str,
    checksum: &str,
    installer: Vec<u8>,
) -> Result<(), String> {
    let remote = format!("/tmp/opute-k3s-{}", &checksum[..16]);
    run_command(
        &["file", "push", temporary, &format!("{instance}{remote}")],
        None,
    )
    .map_err(|e| format!("stage K3s binary in guest: {e}"))?;
    let result = (|| {
        run_command(
            &[
                "exec",
                instance,
                "--",
                "install",
                "-m",
                "0755",
                &remote,
                "/usr/local/bin/k3s",
            ],
            None,
        )
        .map_err(|e| format!("install staged K3s binary: {e}"))?;
        let skip = "INSTALL_K3S_SKIP_DOWNLOAD=true".to_string();
        let version_env = format!("INSTALL_K3S_VERSION={version}");
        let exec_env = format!("INSTALL_K3S_EXEC={install_exec}");
        let url_env = format!("K3S_URL={address}");
        let mut args = vec![
            "exec",
            instance,
            "--env",
            &skip,
            "--env",
            &version_env,
            "--env",
            &exec_env,
        ];
        let input = if token.is_empty() {
            args.extend(["--", "bash", "-s"]);
            installer
        } else {
            args.extend([
                "--env",
                &url_env,
                "--",
                "bash",
                "-lc",
                "IFS= read -r K3S_TOKEN || exit 1; export K3S_TOKEN; exec bash -s",
            ]);
            let mut input = format!("{token}\n").into_bytes();
            input.extend(installer);
            input
        };
        if let Err(e) = run_command(&args, Some(&input)) {
            let diagnostics = install_diagnostics(instance, token);
            if !diagnostics.is_empty() {
                return Err(format!(
                    "run verified K3s installer: {e}; guest diagnostics: {diagnostics}"
                ));
            }
            return Err(format!("run verified K3s installer: {e}"));
        }
        Ok(())
    })();
    let _ = run_command(&["exec", instance, "--", "rm", "-f", &remote], None);
    result
}

/// The guest's unit status and journal, within its own short window, the
/// join token redacted.
fn install_diagnostics(instance: &str, token: &str) -> String {
    let script = "systemctl status --no-pager --full k3s || true; journalctl -u k3s -n 120 --no-pager || true";
    let Ok(output) = super::run_program(
        "incus",
        &["exec", instance, "--", "bash", "-lc", script],
        None,
        Some(DIAGNOSTIC_TIMEOUT),
    ) else {
        return String::new();
    };
    let diagnostics = String::from_utf8_lossy(&output).trim().to_string();
    if token.is_empty() {
        diagnostics
    } else {
        diagnostics.replace(token, "[REDACTED]")
    }
}

/// `url.PathEscape`, then `+` as `%2B`.
pub(super) fn escape_version(version: &str) -> String {
    let mut out = String::new();
    for b in version.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~$&:=@".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The installer, the binary and its checksum.
fn download_artifacts(version: &str) -> Result<(Vec<u8>, Vec<u8>, String), String> {
    if version.is_empty() || super::has_unsafe(version, super::UNSAFE_VERSION) {
        return Err("invalid K3s version".into());
    }
    let base = format!("{RELEASE_URL}{}", escape_version(version));
    let checksums = download(&format!("{base}/sha256sum-amd64.txt"), MAX_BOOTSTRAP_SCRIPT)
        .map_err(|e| format!("download K3s release checksums: {e}"))?;
    let checksum = checksum_for_binary(&String::from_utf8_lossy(&checksums))?;
    let binary = download(&format!("{base}/k3s"), MAX_BINARY_BYTES)
        .map_err(|e| format!("download K3s binary: {e}"))?;
    use sha2::Digest as _;
    let observed = hex::encode(sha2::Sha256::digest(&binary));
    if !observed.eq_ignore_ascii_case(&checksum) {
        return Err(format!(
            "K3s binary checksum mismatch: expected {checksum}, observed {observed}"
        ));
    }
    let installer = download(BOOTSTRAP_SCRIPT_URL, MAX_BOOTSTRAP_SCRIPT)
        .map_err(|e| format!("download K3s installer: {e}"))?;
    Ok((installer, binary, checksum))
}

fn download(url: &str, limit: u64) -> Result<Vec<u8>, String> {
    let response = super::super::support::http_do(super::super::support::HttpRequest {
        method: "GET",
        url,
        headers: Vec::new(),
        body: None,
        timeout: Some(DOWNLOAD_TIMEOUT),
        https_redirects: None,
        limit: limit + 1,
    })?;
    if !(200..300).contains(&response.status) {
        return Err(format!("unexpected HTTP status {}", response.status_line));
    }
    if response.body.len() as u64 > limit {
        return Err(format!("response exceeds {limit}-byte limit"));
    }
    Ok(response.body)
}

/// `filepath.Base`.
fn base_name(path: &str) -> &str {
    if path.is_empty() {
        return ".";
    }
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return "/";
    }
    trimmed.rsplit('/').next().unwrap_or(trimmed)
}

/// fmt's `%#U` for a byte decoded as a rune.
fn hex_invalid_byte(b: u8) -> String {
    let c = char::from(b);
    let printable = (0x20..0x7f).contains(&b) || (b >= 0xa1 && b != 0xad);
    if printable {
        format!("encoding/hex: invalid byte: U+{:04X} '{c}'", b)
    } else {
        format!("encoding/hex: invalid byte: U+{:04X}", b)
    }
}

/// The `k3s` line of the release checksums.
pub(super) fn checksum_for_binary(checksums: &str) -> Result<String, String> {
    for line in checksums.split('\n') {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 2 || fields[0].len() != 64 {
            continue;
        }
        let name = base_name(fields[1]);
        if name.strip_prefix('*').unwrap_or(name) == "k3s" {
            if let Some(&bad) = fields[0].as_bytes().iter().find(|b| !b.is_ascii_hexdigit()) {
                return Err(format!(
                    "K3s release checksum is not hexadecimal: {}",
                    hex_invalid_byte(bad)
                ));
            }
            return Ok(fields[0].to_lowercase());
        }
    }
    Err("K3s release checksums do not contain the amd64 k3s binary".into())
}
