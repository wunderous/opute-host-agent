//! The WSL2 host VM subsystem (`internal/domain/host/wsl_lifecycle.go`,
//! `wsl_compact.go`): terminate one distribution, shut the subsystem down,
//! and compact a stopped distribution's VHDX through Windows interop. Each
//! operation re-checks the tested capability first and runs exact argv.
//! Compaction fails closed with a structured report while the distribution
//! is live, running, or its disk is still held.

use std::time::{Duration, Instant};

use serde_json::{json, Map, Value as J};

use crate::config::Env;
use crate::goerr::quote;
use crate::hostobs::{detect_capabilities, windows_interop_command, RawResult};
use crate::plan::runner::RunCtx;

const VHDX_LOCKED: &str =
    "sibling WSL VM still attached — compact needs shutdown_wsl (kills this agent)";
const LIVE_DISTRO: &str = "cannot compact the live distro that hosts this agent; run compact_wsl_disk from a sibling distro after shutdown_wsl";

fn capability(env: &Env, name: &str) -> bool {
    detect_capabilities(env)
        .get(name)
        .and_then(J::as_bool)
        .unwrap_or(false)
}

/// `HostCommandRunnerContext`: a done context is the error; otherwise the
/// command's result, raw.
fn run(env: &Env, ctx: &RunCtx, argv: &[String], timeout: Duration) -> Result<RawResult, String> {
    if let Some(err) = ctx.err() {
        return Err(err);
    }
    Ok(crate::hostobs::run_command_raw(
        env,
        argv,
        None,
        Some(Instant::now() + timeout),
        timeout,
        Some(ctx),
    ))
}

/// Go's `string(bytes)` seen through JSON: each invalid byte is U+FFFD.
fn go_lossy(mut bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len());
    loop {
        match std::str::from_utf8(bytes) {
            Ok(valid) => {
                out.push_str(valid);
                return out;
            }
            Err(e) => {
                let (valid, rest) = bytes.split_at(e.valid_up_to());
                out.push_str(std::str::from_utf8(valid).expect("valid prefix"));
                out.push('\u{FFFD}');
                bytes = &rest[1..];
            }
        }
    }
}

/// `decodeWindowsCLI`: UTF-16LE with a BOM, or mostly-NUL output read as
/// UTF-16LE, else the bytes without NULs.
fn decode_windows_cli(raw: &[u8]) -> String {
    let utf16 = |b: &[u8]| {
        let units: Vec<u16> = b
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        char::decode_utf16(units)
            .map(|c| c.unwrap_or('\u{FFFD}'))
            .collect::<String>()
    };
    if raw.len() >= 2 && raw[0] == 0xFF && raw[1] == 0xFE {
        return utf16(&raw[2..]);
    }
    let zeros = raw.iter().skip(1).step_by(2).filter(|&&b| b == 0).count();
    if raw.len() > 4 && zeros > raw.len() / 4 {
        return utf16(raw);
    }
    let kept: Vec<u8> = raw.iter().copied().filter(|&b| b != 0).collect();
    go_lossy(&kept)
}

fn joined(result: &RawResult) -> Vec<u8> {
    let mut all = result.stdout.clone();
    all.push(b'\n');
    all.extend_from_slice(&result.stderr);
    all
}

/// `Service.TerminateWSLDistribution`.
pub fn terminate_distribution(env: &Env, ctx: &RunCtx, distro: &str) -> Result<J, String> {
    let distro = distro.trim();
    if distro.is_empty() {
        return Err("distro is required".into());
    }
    if !capability(env, "canTerminateWslDistribution") {
        return Err(
            "wsl distribution termination is unavailable: canTerminateWslDistribution is false"
                .into(),
        );
    }
    let wsl = windows_interop_command(env, "wsl.exe").ok_or("wsl.exe is unavailable")?;
    let result = run(
        env,
        ctx,
        &[wsl, "--terminate".into(), distro.into()],
        Duration::from_secs(30),
    )?;
    if result.exit_code != 0 {
        return Err(format!(
            "wsl.exe --terminate failed with exit code {}",
            result.exit_code
        ));
    }
    Ok(json!({"distro": distro, "command": ["wsl.exe", "--terminate", distro], "terminated": true}))
}

/// `Service.ShutdownWSL`: the whole subsystem, a wider blast radius than
/// one distribution.
pub fn shutdown(env: &Env, ctx: &RunCtx) -> Result<J, String> {
    if !capability(env, "canShutdownWsl") {
        return Err("wsl shutdown is unavailable: canShutdownWsl is false".into());
    }
    let wsl = windows_interop_command(env, "wsl.exe").ok_or("wsl.exe is unavailable")?;
    let result = run(
        env,
        ctx,
        &[wsl, "--shutdown".into()],
        Duration::from_secs(30),
    )?;
    if result.exit_code != 0 {
        return Err(format!(
            "wsl.exe --shutdown failed with exit code {}",
            result.exit_code
        ));
    }
    Ok(json!({"command": ["wsl.exe", "--shutdown"], "shutdown": true}))
}

/// `currentWSLDistro`: the environment, then the process environ files.
fn current_distro(env: &Env) -> String {
    let name = env.get("WSL_DISTRO_NAME");
    if !name.trim().is_empty() {
        return name.trim().to_string();
    }
    for path in ["/proc/self/environ", "/proc/1/environ"] {
        let Ok(raw) = std::fs::read(path) else {
            continue;
        };
        for item in raw.split(|&b| b == 0) {
            if let Some(value) = item.strip_prefix(b"WSL_DISTRO_NAME=") {
                let value = go_lossy(value);
                if !value.trim().is_empty() {
                    return value.trim().to_string();
                }
                break;
            }
        }
    }
    String::new()
}

/// `parseWSLDistributionState`: the STATE column of `wsl.exe -l -v`.
fn distribution_state(output: &str, distro: &str) -> Option<String> {
    let want = distro.trim().to_lowercase();
    for line in output.split('\n') {
        let line = line.strip_prefix('*').unwrap_or(line).trim();
        if line.is_empty() || line.to_lowercase().starts_with("name ") || line.starts_with("---") {
            continue;
        }
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() >= 2 && fields[0].to_lowercase() == want {
            return Some(fields[1].to_string());
        }
    }
    None
}

/// `registryValue`: the data of one `reg.exe query` value line.
fn registry_value(block: &str, name: &str) -> String {
    for line in block.split('\n') {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() >= 3 && crate::gojson::equal_fold(fields[0], name) {
            return fields[2..].join(" ");
        }
    }
    String::new()
}

/// `parseWSLVHDXPath`: the distribution's `BasePath`, as its `ext4.vhdx`.
fn vhdx_path(output: &str, distro: &str) -> Option<String> {
    let want = distro.trim().to_lowercase();
    for block in output.split("HKEY_") {
        let name = registry_value(block, "DistributionName");
        let base = registry_value(block, "BasePath");
        if name.to_lowercase() != want || base.trim().is_empty() {
            continue;
        }
        let base = base.trim_matches('"').trim_end_matches('\\');
        if base.to_lowercase().ends_with("ext4.vhdx") {
            return Some(base.to_string());
        }
        return Some(format!("{base}\\ext4.vhdx"));
    }
    None
}

fn powershell_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

fn powershell(env: &Env) -> Result<String, String> {
    windows_interop_command(env, "powershell.exe")
        .ok_or_else(|| "powershell.exe is unavailable".into())
}

fn powershell_argv(program: String, script: String) -> Vec<String> {
    vec![
        program,
        "-NoProfile".into(),
        "-NonInteractive".into(),
        "-Command".into(),
        script,
    ]
}

/// `strconv.ParseInt(text, 10, 64)`'s error.
fn parse_int(text: &str) -> Result<i64, String> {
    text.parse::<i64>().map_err(|e| {
        use std::num::IntErrorKind::*;
        let reason = match e.kind() {
            PosOverflow | NegOverflow => "value out of range",
            _ => "invalid syntax",
        };
        format!("strconv.ParseInt: parsing {}: {reason}", quote(text))
    })
}

/// `windowsFileSize`.
fn file_size(env: &Env, ctx: &RunCtx, path: &str) -> Result<i64, String> {
    let program = powershell(env)?;
    let script = format!(
        "(Get-Item -LiteralPath {}).Length",
        powershell_literal(path)
    );
    let result = run(
        env,
        ctx,
        &powershell_argv(program, script),
        Duration::from_secs(15),
    )?;
    if result.exit_code != 0 {
        return Err(format!(
            "Get-Item {path} failed with exit code {}",
            result.exit_code
        ));
    }
    let text = decode_windows_cli(&result.stdout);
    let text = text.trim();
    parse_int(text).map_err(|e| format!("parse VHDX size {}: {e}", quote(text)))
}

/// The fail-closed compact answer (`CompactClosedError.StructuredReport`):
/// the disk was not compacted, and the report says why.
fn compact_closed(report: Map<String, J>, message: &str) -> J {
    let mut structured = Map::new();
    structured.insert("code".into(), J::from("wsl_compact_closed"));
    structured.insert("owner".into(), J::from("capability"));
    structured.insert("message".into(), J::from(message));
    structured.insert("compacted".into(), J::Bool(false));
    structured.insert("sparse".into(), J::Bool(false));
    structured.extend(report);
    json!({
        "content": [{"type": "text", "text": format!("Error: {message}")}],
        "structuredContent": structured,
        "isError": true,
    })
}

/// `optimizeVHDError`: an elevation failure says so, and never suggests a
/// sparse VHDX.
fn optimize_error(exit_code: i32, combined: &str) -> String {
    let message = format!("Optimize-VHD failed with exit code {exit_code}: {combined}");
    let lower = combined.to_lowercase();
    if lower.contains("access is denied")
        || lower.contains("permission denied")
        || (lower.contains("hyper-v") && lower.contains("not"))
    {
        return format!("{message}: Optimize-VHD needs an elevated Hyper-V Administrators token (UAC). Do not enable WSL sparse VHDX");
    }
    message
}

/// The outcome of a compact: a report for `structured_result`, or a full
/// fail-closed tool result.
pub enum Compact {
    Report(J),
    Closed(J),
}

/// `Service.CompactWSLDisk`.
pub fn compact_disk(
    env: &Env,
    ctx: &RunCtx,
    distro: &str,
    dry_run: bool,
) -> Result<Compact, String> {
    let distro = distro.trim();
    if distro.is_empty() {
        return Err("distro is required".into());
    }
    if !capability(env, "canManageWsl") {
        return Err("wsl disk compact is unavailable: canManageWsl is false".into());
    }
    let current = current_distro(env);
    if !current.trim().is_empty() && crate::gojson::equal_fold(current.trim(), distro) {
        let mut report = Map::new();
        report.insert("distro".into(), J::from(distro));
        report.insert("live".into(), J::Bool(true));
        report.insert("sparse".into(), J::Bool(false));
        return Ok(Compact::Closed(compact_closed(report, LIVE_DISTRO)));
    }
    let wsl = windows_interop_command(env, "wsl.exe").ok_or("wsl.exe is unavailable")?;
    // `wslDistributionState`.
    let listed = run(
        env,
        ctx,
        &[wsl, "-l".into(), "-v".into()],
        Duration::from_secs(15),
    )?;
    if listed.exit_code != 0 {
        return Err(format!(
            "wsl.exe -l -v failed with exit code {}",
            listed.exit_code
        ));
    }
    let state = distribution_state(&decode_windows_cli(&joined(&listed)), distro)
        .ok_or_else(|| format!("WSL distribution {} was not found", quote(distro)))?;
    // `wslDistributionVHDX`.
    let reg = windows_interop_command(env, "reg.exe").ok_or("reg.exe is unavailable")?;
    let queried = run(
        env,
        ctx,
        &[
            reg,
            "query".into(),
            r"HKCU\Software\Microsoft\Windows\CurrentVersion\Lxss".into(),
            "/s".into(),
        ],
        Duration::from_secs(15),
    )?;
    if queried.exit_code != 0 {
        return Err(format!(
            "reg.exe query failed with exit code {}",
            queried.exit_code
        ));
    }
    let vhdx = vhdx_path(&decode_windows_cli(&joined(&queried)), distro)
        .ok_or_else(|| format!("VHDX path for distro {} was not found", quote(distro)))?;
    // `wslVHDXLocked`: any answer containing "locked" counts, so Go's own
    // "unlocked" reads as locked too; the exit status is not consulted.
    let program = powershell(env)?;
    let script = format!(
        "try {{ $fs = [System.IO.File]::Open({}, 'Open', 'ReadWrite', 'None'); $fs.Close(); 'unlocked' }} catch {{ 'locked' }}",
        powershell_literal(&vhdx)
    );
    let probe = run(
        env,
        ctx,
        &powershell_argv(program, script),
        Duration::from_secs(15),
    )?;
    let locked = decode_windows_cli(&probe.stdout)
        .trim()
        .to_lowercase()
        .contains("locked");
    let before = file_size(env, ctx, &vhdx)?;
    let mut report = Map::new();
    report.insert("distro".into(), J::from(distro));
    report.insert("state".into(), J::from(state.clone()));
    report.insert("vhdxPath".into(), J::from(vhdx.clone()));
    report.insert("locked".into(), J::Bool(locked));
    report.insert("beforeBytes".into(), J::from(before));
    report.insert("dryRun".into(), J::Bool(dry_run));
    report.insert("sparse".into(), J::Bool(false));
    // `compactPreconditions`.
    if !crate::gojson::equal_fold(state.trim(), "Stopped") {
        let message = format!("distro is {state}; compact requires Stopped. terminate_wsl_distribution is not enough if a sibling WSL VM still holds the VHDX — compact needs shutdown_wsl (kills this agent)");
        return Ok(Compact::Closed(compact_closed(report, &message)));
    }
    if locked {
        return Ok(Compact::Closed(compact_closed(report, VHDX_LOCKED)));
    }
    if dry_run {
        report.insert("wouldCompact".into(), J::Bool(true));
        report.insert("compacted".into(), J::Bool(false));
        return Ok(Compact::Report(J::Object(report)));
    }
    let program = powershell(env)?;
    let script = format!(
        "Import-Module Hyper-V -ErrorAction Stop; Optimize-VHD -Path {} -Mode Full",
        powershell_literal(&vhdx)
    );
    let result = run(
        env,
        ctx,
        &powershell_argv(program, script),
        Duration::from_secs(30 * 60),
    )
    .map_err(|e| format!("Optimize-VHD: {e}"))?;
    if result.exit_code != 0 {
        let mut combined = result.stderr.clone();
        combined.push(b' ');
        combined.extend_from_slice(&result.stdout);
        return Err(optimize_error(result.exit_code, go_lossy(&combined).trim()));
    }
    let after = file_size(env, ctx, &vhdx)?;
    report.insert("afterBytes".into(), J::from(after));
    report.insert("reclaimedBytes".into(), J::from(before.wrapping_sub(after)));
    report.insert("compacted".into(), J::Bool(true));
    Ok(Compact::Report(J::Object(report)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_output_decodes_like_go() {
        let utf16: Vec<u8> = "Ubuntu Stopped 2\n"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        assert_eq!(decode_windows_cli(&utf16), "Ubuntu Stopped 2\n");
        let mut bom = vec![0xFF, 0xFE];
        bom.extend_from_slice(&utf16);
        assert_eq!(decode_windows_cli(&bom), "Ubuntu Stopped 2\n");
        assert_eq!(decode_windows_cli(b"a\0b\xff"), "ab\u{FFFD}");
    }

    #[test]
    fn listings_parse_like_go() {
        let list =
            "  NAME      STATE     VERSION\n* Ubuntu    Running   2\n  Other     Stopped   2\n";
        assert_eq!(
            distribution_state(list, "ubuntu").as_deref(),
            Some("Running")
        );
        assert_eq!(
            distribution_state(list, "other").as_deref(),
            Some("Stopped")
        );
        assert!(distribution_state(list, "none").is_none());
        let reg = "HKEY_CURRENT_USER\\x\\{a}\n    DistributionName    REG_SZ    Other\n    BasePath    REG_SZ    \"C:\\wsl\\Other\\\"\n";
        assert_eq!(
            vhdx_path(reg, "OTHER").as_deref(),
            Some("C:\\wsl\\Other\\ext4.vhdx")
        );
        assert_eq!(
            parse_int("x").unwrap_err(),
            "strconv.ParseInt: parsing \"x\": invalid syntax"
        );
    }
}
