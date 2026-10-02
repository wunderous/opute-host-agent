//! Host observations reported by `/health`: tested host capabilities and VM
//! inventory capacity (`fingerprint.DetectCapabilities`,
//! `incus.Service.VMInventoryCapacity`), plus the provider command runner
//! they use (`internal/exec.RunCommandContext`).
//!
//! Commands see the agent's effective environment (process environment plus
//! the env file), which in Go is the process environment after `os.Setenv`.

use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Map, Value as J};

use crate::config::Env;
use crate::gojson;

pub struct CommandResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// `exec.LookPath`: a name with a slash must be an executable file; other
/// names are searched in PATH, and a relative match is an error (ErrDot).
pub fn look_path(env: &Env, name: &str) -> Option<PathBuf> {
    let executable = |p: &Path| {
        std::fs::metadata(p)
            .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    };
    if name.contains('/') {
        let p = PathBuf::from(name);
        return executable(&p).then_some(p);
    }
    for dir in env.get("PATH").split(':') {
        let dir = if dir.is_empty() { "." } else { dir };
        let candidate = Path::new(dir).join(name);
        if executable(&candidate) {
            return candidate.is_absolute().then_some(candidate);
        }
    }
    None
}

/// `RunCommandContext` without streaming: start failures and timeouts are
/// results, not errors, exactly as in Go.
pub fn run_command(env: &Env, argv: &[String], timeout: Duration) -> CommandResult {
    if argv.is_empty() {
        return CommandResult {
            exit_code: 1,
            stdout: String::new(),
            stderr: "empty command".into(),
        };
    }
    let program = if argv[0].contains('/') {
        Some(PathBuf::from(&argv[0]))
    } else {
        look_path(env, &argv[0])
    };
    let Some(program) = program else {
        return CommandResult {
            exit_code: 1,
            stdout: String::new(),
            stderr: format!(
                "exec: {}: executable file not found in $PATH",
                crate::goerr::quote(&argv[0])
            ),
        };
    };
    let mut cmd = Command::new(&program);
    cmd.args(&argv[1..])
        .env_clear()
        .envs(env.pairs())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return CommandResult {
                exit_code: 1,
                stdout: String::new(),
                stderr: format!(
                    "fork/exec {}: {}",
                    program.display(),
                    crate::goerr::errno_text(&e)
                ),
            }
        }
    };
    let mut out_pipe = child.stdout.take().expect("piped stdout");
    let mut err_pipe = child.stderr.take().expect("piped stderr");
    let out_reader = std::thread::spawn(move || {
        let mut s = Vec::new();
        let _ = out_pipe.read_to_end(&mut s);
        s
    });
    let err_reader = std::thread::spawn(move || {
        let mut s = Vec::new();
        let _ = err_pipe.read_to_end(&mut s);
        s
    });
    let deadline = Instant::now() + timeout;
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() >= deadline => {
                timed_out = true;
                let pgid = nix::unistd::Pid::from_raw(child.id() as i32);
                let _ = nix::sys::signal::killpg(pgid, nix::sys::signal::Signal::SIGKILL);
                break child.wait().ok();
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(_) => break None,
        }
    };
    let stdout = String::from_utf8_lossy(&out_reader.join().unwrap_or_default()).into_owned();
    let mut stderr = String::from_utf8_lossy(&err_reader.join().unwrap_or_default()).into_owned();
    if timed_out {
        if !stderr.is_empty() {
            stderr.push('\n');
        }
        stderr.push_str(&format!(
            "Error: Command timed out after {}",
            go_duration(timeout)
        ));
        return CommandResult {
            exit_code: 124,
            stdout,
            stderr,
        };
    }
    let exit_code = status.and_then(|s| s.code()).unwrap_or(-1);
    CommandResult {
        exit_code,
        stdout,
        stderr,
    }
}

/// `time.Duration.String` for whole seconds and minutes.
fn go_duration(d: Duration) -> String {
    let secs = d.as_secs();
    if secs >= 60 {
        let (m, s) = (secs / 60, secs % 60);
        return format!("{m}m{s}s");
    }
    format!("{secs}s")
}

fn is_wsl(env: &Env) -> bool {
    if !env.get("WSL_INTEROP").trim().is_empty() || !env.get("WSL_DISTRO_NAME").trim().is_empty() {
        return true;
    }
    std::fs::read_to_string("/proc/version")
        .map(|v| {
            let v = v.to_lowercase();
            v.contains("microsoft") || v.contains("wsl")
        })
        .unwrap_or(false)
}

fn windows_interop_command(env: &Env, name: &str) -> bool {
    let mut candidates: Vec<String> = Vec::new();
    match name {
        "powershell.exe" => {
            candidates.push("/mnt/c/Windows/System32/WindowsPowerShell/v1.0/powershell.exe".into())
        }
        "reg.exe" | "wsl.exe" => {
            candidates.push(format!("/mnt/c/Windows/System32/{name}"));
            candidates.push(format!("/mnt/c/Windows/system32/{name}"));
        }
        _ => {}
    }
    candidates.push(name.into());
    candidates.iter().any(|c| look_path(env, c).is_some())
}

/// `fingerprint.DetectCapabilities` on Linux.
pub fn detect_capabilities(env: &Env) -> J {
    let wsl = is_wsl(env);
    let interop = wsl
        && (windows_interop_command(env, "powershell.exe")
            || windows_interop_command(env, "reg.exe"));
    let manage = wsl && windows_interop_command(env, "wsl.exe");
    json!({
        "canInvokeWindowsInterop": interop,
        "canManageWsl": manage,
        "canTerminateWslDistribution": manage,
        "canShutdownWsl": manage,
    })
}

/// The `HealthObserver` closure from `internal/app/app.go`.
pub fn health_observer(
    env: Env,
    instance_id: String,
    ownership_mode: String,
) -> impl Fn() -> Map<String, J> {
    let incus = crate::incus::Incus {
        binary: crate::incus::provider_binary(&env),
        env: env.clone(),
        instance_id,
        ownership_mode,
        agent_id: String::new(),
        tenant_id: String::new(),
    };
    move || {
        let capacity = incus.inventory_capacity().map(|c| {
            (
                c["runningVmCount"].as_i64().unwrap_or(0),
                c["totalVmCount"].as_i64().unwrap_or(0),
            )
        });
        let mut result = Map::new();
        result.insert(
            "capabilities".into(),
            json!({"host": detect_capabilities(&env)}),
        );
        if let Ok((running, total)) = capacity {
            result.insert("runningVmCount".into(), J::from(running));
            result.insert("totalVmCount".into(), J::from(total));
        }
        result
    }
}

// --- host system metadata (heartbeat.ReadHostSystemStats) ---------------------------

/// `heartbeat.PressureStall`: one kernel PSI reading.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PressureStall {
    pub some_avg10: f64,
    pub some_avg60: f64,
    pub some_avg300: f64,
    pub some_total_usec: i64,
    pub full_avg10: f64,
    pub full_avg60: f64,
    pub full_avg300: f64,
    pub full_total_usec: i64,
}

/// PSI readings as their struct encodes (`omitempty` on every field).
pub fn stalls_json(stalls: &std::collections::BTreeMap<String, PressureStall>) -> J {
    let mut out = Map::new();
    for (resource, s) in stalls {
        let mut m = Map::new();
        for (k, v) in [
            ("someAvg10", s.some_avg10),
            ("someAvg60", s.some_avg60),
            ("someAvg300", s.some_avg300),
            ("fullAvg10", s.full_avg10),
            ("fullAvg60", s.full_avg60),
            ("fullAvg300", s.full_avg300),
        ] {
            if v != 0.0 {
                m.insert(k.into(), gojson::float_value(v));
            }
        }
        for (k, v) in [
            ("someTotalUsec", s.some_total_usec),
            ("fullTotalUsec", s.full_total_usec),
        ] {
            if v != 0 {
                m.insert(k.into(), J::from(v));
            }
        }
        out.insert(resource.clone(), J::Object(m));
    }
    J::Object(out)
}

#[derive(Clone, Debug, PartialEq)]
pub struct DiskStats {
    pub mount: String,
    pub total_bytes: i64,
    pub available_bytes: i64,
    pub pressure: String,
}

/// `heartbeat.HostSystemStats`.
#[derive(Clone, Debug, Default)]
pub struct HostSystemStats {
    pub cpu_count: i64,
    pub cpu_quota_cores: f64,
    pub cpu_load: [f64; 3],
    pub memory_total_bytes: i64,
    pub memory_free_bytes: i64,
    pub memory_available_bytes: i64,
    pub memory_used_bytes: i64,
    pub memory_limit_bytes: i64,
    pub memory_usage_bytes: i64,
    pub memory_pressure: String,
    pub memory_events: Option<std::collections::BTreeMap<String, i64>>,
    pub pressure_stalls: Option<std::collections::BTreeMap<String, PressureStall>>,
    pub cgroup_controllers: Vec<String>,
    pub cgroup_enforcement: String,
    pub tasks_current: i64,
    pub tasks_limit: i64,
    pub disk_total_bytes: i64,
    pub disk_available_bytes: i64,
    pub disk_pressure: String,
    pub disk_mount: String,
    pub disk_filesystems: Vec<DiskStats>,
}

fn ratio_pressure(available: i64, total: i64, critical: f64, warning: f64) -> String {
    if total <= 0 {
        return "unknown".into();
    }
    let ratio = available as f64 / total as f64;
    if ratio < critical {
        "critical".into()
    } else if ratio < warning {
        "warning".into()
    } else {
        "normal".into()
    }
}

/// `strconv.ParseFloat` for the forms the kernel writes.
fn parse_f64(s: &str) -> Option<f64> {
    s.parse::<f64>().ok()
}

fn read_trimmed(path: &Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
}

const CGROUP_ROOT: &str = "/sys/fs/cgroup";

/// `filepath.Clean` for absolute paths.
fn clean_path(p: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    format!("/{}", parts.join("/"))
}

/// `currentCgroupDirectories`: this process's cgroup and its ancestors,
/// then the mount root.
pub fn current_cgroup_directories() -> Vec<PathBuf> {
    let Ok(contents) = std::fs::read_to_string("/proc/self/cgroup") else {
        return vec![PathBuf::from(CGROUP_ROOT)];
    };
    let mut dirs: Vec<PathBuf> = Vec::new();
    let mut add = |root: &str, relative: &str| {
        let relative = relative.trim().trim_start_matches('/');
        let relative = if relative.is_empty() { "." } else { relative };
        let candidate = clean_path(&format!("{root}/{relative}"));
        let base = clean_path(root);
        if !(candidate == base || candidate.starts_with(&format!("{base}/"))) {
            return;
        }
        let candidate = PathBuf::from(candidate);
        if !dirs.contains(&candidate) {
            dirs.push(candidate);
        }
    };
    let add_hierarchy = |root: &str, relative: &str, add: &mut dyn FnMut(&str, &str)| {
        let mut relative = relative.trim().trim_matches('/').to_string();
        if relative.is_empty() {
            add(root, ".");
            return;
        }
        loop {
            add(root, &relative);
            let parent = match relative.rfind('/') {
                Some(i) => relative[..i].trim_matches('/').to_string(),
                None => String::new(),
            };
            if parent.is_empty() || parent == relative {
                add(root, ".");
                return;
            }
            relative = parent;
        }
    };
    for line in contents.split('\n') {
        let parts: Vec<&str> = line.splitn(3, ':').collect();
        if parts.len() != 3 {
            continue;
        }
        if parts[0] == "0" {
            add_hierarchy(CGROUP_ROOT, parts[2], &mut add);
            continue;
        }
        for controller in parts[1].trim().split(',') {
            let controller = controller.trim();
            if controller.is_empty() {
                continue;
            }
            add_hierarchy(&format!("{CGROUP_ROOT}/{controller}"), parts[2], &mut add);
        }
    }
    add(CGROUP_ROOT, ".");
    dirs
}

fn read_cpu_quota_cores(dirs: &[PathBuf]) -> f64 {
    let mut effective = 0.0;
    for dir in dirs {
        if let Some(contents) = read_trimmed(&dir.join("cpu.max")) {
            let fields: Vec<&str> = contents.split_whitespace().collect();
            if fields.len() >= 2 && fields[0] != "max" {
                if let (Some(q), Some(p)) = (parse_f64(fields[0]), parse_f64(fields[1])) {
                    if q > 0.0 && p > 0.0 {
                        let cores = q / p;
                        if effective == 0.0 || cores < effective {
                            effective = cores;
                        }
                    }
                }
            }
        }
    }
    if effective > 0.0 {
        return effective;
    }
    let file = |name: &str| {
        dirs.iter()
            .find_map(|d| read_trimmed(&d.join(name)).and_then(|s| parse_f64(&s)))
            .unwrap_or(0.0)
    };
    let (quota, period) = (file("cpu.cfs_quota_us"), file("cpu.cfs_period_us"));
    if quota <= 0.0 || period <= 0.0 {
        0.0
    } else {
        quota / period
    }
}

fn read_memory_limit(dirs: &[PathBuf]) -> i64 {
    let mut effective = 0;
    for dir in dirs {
        for name in ["memory.max", "memory.limit_in_bytes"] {
            let Some(value) = read_trimmed(&dir.join(name)) else {
                continue;
            };
            if value.is_empty() || value == "max" {
                continue;
            }
            if let Ok(parsed) = value.parse::<i64>() {
                if parsed > 0 && (effective == 0 || parsed < effective) {
                    effective = parsed;
                }
            }
        }
    }
    effective
}

fn read_memory_usage(dirs: &[PathBuf]) -> i64 {
    for dir in dirs {
        for name in ["memory.current", "memory.usage_in_bytes"] {
            if let Some(v) = read_trimmed(&dir.join(name)).and_then(|s| s.parse::<i64>().ok()) {
                if v >= 0 {
                    return v;
                }
            }
        }
    }
    0
}

fn read_pressure_stalls() -> Option<std::collections::BTreeMap<String, PressureStall>> {
    let mut out = std::collections::BTreeMap::new();
    for resource in ["cpu", "memory", "io"] {
        let Ok(contents) = std::fs::read_to_string(format!("/proc/pressure/{resource}")) else {
            continue;
        };
        let mut stall = PressureStall::default();
        for line in contents.split('\n') {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() < 2 {
                continue;
            }
            let full = fields[0] == "full";
            if fields[0] != "some" && !full {
                continue;
            }
            for field in &fields[1..] {
                let Some((key, value)) = field.split_once('=') else {
                    continue;
                };
                let Some(value) = parse_f64(value).filter(|v| v.is_finite()) else {
                    continue;
                };
                match (key, full) {
                    ("avg10", false) => stall.some_avg10 = value,
                    ("avg10", true) => stall.full_avg10 = value,
                    ("avg60", false) => stall.some_avg60 = value,
                    ("avg60", true) => stall.full_avg60 = value,
                    ("avg300", false) => stall.some_avg300 = value,
                    ("avg300", true) => stall.full_avg300 = value,
                    ("total", false) => stall.some_total_usec = value as i64,
                    ("total", true) => stall.full_total_usec = value as i64,
                    _ => {}
                }
            }
        }
        if stall != PressureStall::default() {
            out.insert(resource.to_string(), stall);
        }
    }
    (!out.is_empty()).then_some(out)
}

fn read_memory_events(dirs: &[PathBuf]) -> Option<std::collections::BTreeMap<String, i64>> {
    for dir in dirs {
        let Ok(contents) = std::fs::read_to_string(dir.join("memory.events")) else {
            continue;
        };
        let mut out = std::collections::BTreeMap::new();
        for line in contents.split('\n') {
            let fields: Vec<&str> = line.split_whitespace().collect();
            if fields.len() != 2 {
                continue;
            }
            if let Ok(v) = fields[1].parse::<i64>() {
                if v >= 0 {
                    out.insert(fields[0].to_string(), v);
                }
            }
        }
        if !out.is_empty() {
            return Some(out);
        }
    }
    None
}

fn read_cgroup_enforcement(dirs: &[PathBuf]) -> (Vec<String>, String) {
    for dir in dirs {
        if let Ok(raw) = std::fs::read_to_string(dir.join("cgroup.controllers")) {
            let controllers: Vec<String> = raw.split_whitespace().map(str::to_string).collect();
            if controllers.is_empty() {
                return (controllers, "unknown".into());
            }
            let has = |c: &str| controllers.iter().any(|x| x == c);
            let exists = |n: &str| dir.join(n).exists();
            if has("memory")
                && has("cpu")
                && exists("memory.max")
                && exists("memory.current")
                && exists("cpu.max")
            {
                return (controllers, "enforced".into());
            }
            return (controllers, "unknown".into());
        }
        if dir.join("memory.limit_in_bytes").exists() && dir.join("cpu.cfs_quota_us").exists() {
            return (vec!["cpu".into(), "memory".into()], "enforced".into());
        }
    }
    (Vec::new(), "unsupported".into())
}

fn read_cgroup_tasks(dirs: &[PathBuf]) -> (i64, i64) {
    let mut current = 0;
    for dir in dirs {
        let Some(text) = read_trimmed(&dir.join("pids.current")) else {
            continue;
        };
        if text == "max" {
            break;
        }
        if let Ok(v) = text.parse::<i64>() {
            if v >= 0 {
                current = v;
                break;
            }
        }
    }
    let mut limit = 0;
    for dir in dirs {
        let Some(text) = read_trimmed(&dir.join("pids.max")) else {
            continue;
        };
        if text == "max" {
            continue;
        }
        if let Ok(v) = text.parse::<i64>() {
            if v > 0 && (limit == 0 || v < limit) {
                limit = v;
            }
        }
    }
    (current, limit)
}

fn read_meminfo() -> Option<(i64, i64)> {
    let contents = std::fs::read_to_string("/proc/meminfo").ok()?;
    let kb = |line: &str| {
        line.split_whitespace()
            .nth(1)
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(0)
    };
    let (mut total, mut available) = (0, 0);
    for line in contents.lines() {
        if line.starts_with("MemTotal:") {
            total = kb(line);
        } else if line.starts_with("MemAvailable:") {
            available = kb(line);
        }
    }
    if total <= 0 {
        return None;
    }
    Some((total * 1024, available.max(0) * 1024))
}

fn read_load_average() -> [f64; 3] {
    let Ok(contents) = std::fs::read_to_string("/proc/loadavg") else {
        return [0.0; 3];
    };
    let fields: Vec<&str> = contents.split_whitespace().collect();
    if fields.len() < 3 {
        return [0.0; 3];
    }
    let mut out = [0.0; 3];
    for i in 0..3 {
        match parse_f64(fields[i]) {
            Some(v) if v.is_finite() && v >= 0.0 => out[i] = v,
            _ => return [0.0; 3],
        }
    }
    out
}

/// `defaultDiskPaths`: the root filesystem and the user's home.
pub fn default_disk_paths(env: &Env) -> Vec<String> {
    let mut paths = vec!["/".to_string()];
    let home = env.get("HOME");
    if !home.trim().is_empty() {
        paths.push(clean_path(home.trim()));
    }
    paths
}

fn read_disk_stats(paths: &[String]) -> Vec<DiskStats> {
    let mut seen = Vec::new();
    let mut out = Vec::new();
    for path in paths {
        let trimmed = path.trim();
        if trimmed.is_empty() {
            continue;
        }
        let path = if trimmed.starts_with('/') {
            clean_path(trimmed)
        } else {
            trimmed.to_string()
        };
        if path == "." || seen.contains(&path) {
            continue;
        }
        seen.push(path.clone());
        let Ok(st) = nix::sys::statfs::statfs(path.as_str()) else {
            continue;
        };
        if st.blocks() == 0 {
            continue;
        }
        // f_bsize's type differs between targets (i64 on x86_64 Linux).
        #[allow(clippy::unnecessary_cast)]
        let block_size = st.block_size() as i64;
        let total = st.blocks() as i64 * block_size;
        let available = st.blocks_available() as i64 * block_size;
        if total <= 0 || available < 0 {
            continue;
        }
        out.push(DiskStats {
            pressure: ratio_pressure(available, total, 0.05, 0.10),
            mount: path,
            total_bytes: total,
            available_bytes: available,
        });
    }
    out
}

/// `runtime.NumCPU`: the CPUs in this process's affinity mask.
fn num_cpu() -> i64 {
    let Ok(set) = nix::sched::sched_getaffinity(nix::unistd::Pid::from_raw(0)) else {
        return 1;
    };
    let n = (0..nix::sched::CpuSet::count())
        .filter(|&i| set.is_set(i).unwrap_or(false))
        .count() as i64;
    n.max(1)
}

impl HostSystemStats {
    /// `ReadHostSystemStatsForPaths`.
    pub fn read(paths: &[String]) -> HostSystemStats {
        let dirs = current_cgroup_directories();
        let mut stats = HostSystemStats {
            cpu_count: num_cpu(),
            ..HostSystemStats::default()
        };
        if let Some((total, free)) = read_meminfo() {
            stats.memory_total_bytes = total;
            stats.memory_free_bytes = free;
            stats.memory_available_bytes = free;
            stats.memory_used_bytes = (total - free).max(0);
            stats.memory_pressure = ratio_pressure(free, total, 0.10, 0.20);
        }
        stats.cpu_quota_cores = read_cpu_quota_cores(&dirs);
        stats.cpu_load = read_load_average();
        stats.memory_limit_bytes = read_memory_limit(&dirs);
        stats.memory_usage_bytes = read_memory_usage(&dirs);
        stats.memory_events = read_memory_events(&dirs);
        stats.pressure_stalls = read_pressure_stalls();
        (stats.cgroup_controllers, stats.cgroup_enforcement) = read_cgroup_enforcement(&dirs);
        (stats.tasks_current, stats.tasks_limit) = read_cgroup_tasks(&dirs);
        stats.disk_filesystems = read_disk_stats(paths);
        for disk in &stats.disk_filesystems {
            if stats.disk_mount.is_empty() || disk.available_bytes < stats.disk_available_bytes {
                stats.disk_mount = disk.mount.clone();
                stats.disk_total_bytes = disk.total_bytes;
                stats.disk_available_bytes = disk.available_bytes;
                stats.disk_pressure = disk.pressure.clone();
            }
        }
        stats
    }

    /// `systemMetadata`: the JSON-safe snapshot (`nil` when empty).
    pub fn metadata(&self) -> Option<Map<String, J>> {
        let mut m = Map::new();
        let int = |m: &mut Map<String, J>, k: &str, v: i64| {
            if v > 0 {
                m.insert(k.into(), J::from(v));
            }
        };
        int(&mut m, "cpuCount", self.cpu_count);
        int(&mut m, "memoryTotalBytes", self.memory_total_bytes);
        int(&mut m, "memoryFreeBytes", self.memory_free_bytes);
        int(&mut m, "memoryAvailableBytes", self.memory_available_bytes);
        for (k, v) in [
            ("cpuQuotaCores", self.cpu_quota_cores),
            ("cpuLoad1m", self.cpu_load[0]),
            ("cpuLoad5m", self.cpu_load[1]),
            ("cpuLoad15m", self.cpu_load[2]),
        ] {
            if v > 0.0 {
                m.insert(k.into(), gojson::float_value(v));
            }
        }
        int(&mut m, "memoryUsedBytes", self.memory_used_bytes);
        int(&mut m, "memoryLimitBytes", self.memory_limit_bytes);
        int(&mut m, "memoryUsageBytes", self.memory_usage_bytes);
        if !self.memory_pressure.is_empty() {
            m.insert(
                "memoryPressure".into(),
                J::from(self.memory_pressure.clone()),
            );
        }
        if self.memory_limit_bytes > 0 || self.memory_usage_bytes > 0 {
            m.insert(
                "cgroupMemory".into(),
                json!({"limitBytes": self.memory_limit_bytes, "usageBytes": self.memory_usage_bytes}),
            );
        }
        if let Some(events) = self.memory_events.as_ref().filter(|e| !e.is_empty()) {
            m.insert("memoryEvents".into(), json!(events));
        }
        if let Some(stalls) = self.pressure_stalls.as_ref().filter(|s| !s.is_empty()) {
            m.insert("psi".into(), stalls_json(stalls));
        }
        if !self.cgroup_controllers.is_empty() {
            m.insert("cgroupControllers".into(), json!(self.cgroup_controllers));
        }
        if !self.cgroup_enforcement.is_empty() {
            m.insert(
                "enforcement".into(),
                J::from(self.cgroup_enforcement.clone()),
            );
        }
        if self.tasks_current > 0 || self.tasks_limit > 0 {
            m.insert(
                "tasks".into(),
                json!({"current": self.tasks_current, "limit": self.tasks_limit}),
            );
        }
        int(&mut m, "diskTotalBytes", self.disk_total_bytes);
        int(&mut m, "diskAvailableBytes", self.disk_available_bytes);
        if !self.disk_pressure.is_empty() {
            m.insert("diskPressure".into(), J::from(self.disk_pressure.clone()));
        }
        if !self.disk_mount.is_empty() {
            m.insert("diskMount".into(), J::from(self.disk_mount.clone()));
        }
        if !self.disk_filesystems.is_empty() {
            let list: Vec<J> = self
                .disk_filesystems
                .iter()
                .map(|d| {
                    json!({"mount": d.mount, "totalBytes": d.total_bytes,
                           "availableBytes": d.available_bytes, "pressure": d.pressure})
                })
                .collect();
            m.insert("diskFilesystems".into(), J::Array(list));
        }
        (!m.is_empty()).then_some(m)
    }
}

// --- time ---------------------------------------------------------------------------

/// `time.Now().UTC().Format(time.RFC3339)`.
pub fn rfc3339_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (y, mo, d) = crate::app::civil_from_days(secs.div_euclid(86_400));
    let rem = secs.rem_euclid(86_400);
    format!(
        "{y:04}-{mo:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// `time.Now().UTC().Format(time.RFC3339Nano)`: trailing zeros trimmed.
pub fn rfc3339_nano_now() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs() as i64;
    let (y, mo, d) = crate::app::civil_from_days(secs.div_euclid(86_400));
    let rem = secs.rem_euclid(86_400);
    let mut frac = format!("{:09}", now.subsec_nanos());
    while frac.ends_with('0') {
        frac.pop();
    }
    let frac = if frac.is_empty() {
        String::new()
    } else {
        format!(".{frac}")
    };
    format!(
        "{y:04}-{mo:02}-{d:02}T{:02}:{:02}:{:02}{frac}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// `time.Parse(time.RFC3339Nano, s)` for the forms the agent writes.
pub fn parse_rfc3339(s: &str) -> Option<std::time::SystemTime> {
    let b = s.as_bytes();
    if b.len() < 20
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let num = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, mo, d, h, mi, se) = (
        num(0..4)?,
        num(5..7)?,
        num(8..10)?,
        num(11..13)?,
        num(14..16)?,
        num(17..19)?,
    );
    let mut rest = &s[19..];
    let mut nanos = 0i64;
    if let Some(frac) = rest.strip_prefix('.') {
        let digits: String = frac.chars().take_while(char::is_ascii_digit).collect();
        if digits.is_empty() || digits.len() > 9 {
            return None;
        }
        nanos = format!("{digits:0<9}").parse().ok()?;
        rest = &frac[digits.len()..];
    }
    let offset = match rest {
        "Z" => 0,
        tz if tz.len() == 6 && (tz.starts_with('+') || tz.starts_with('-')) => {
            let sign = if tz.starts_with('-') { -1 } else { 1 };
            sign * (tz.get(1..3)?.parse::<i64>().ok()? * 3600
                + tz.get(4..6)?.parse::<i64>().ok()? * 60)
        }
        _ => return None,
    };
    // days_from_civil
    let yy = if mo <= 2 { y - 1 } else { y };
    let era = yy.div_euclid(400);
    let yoe = yy - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let secs = days * 86_400 + h * 3600 + mi * 60 + se - offset;
    let base = std::time::UNIX_EPOCH;
    if secs >= 0 {
        Some(base + Duration::new(secs as u64, nanos as u32))
    } else {
        base.checked_sub(Duration::from_secs((-secs) as u64))
            .map(|t| t + Duration::from_nanos(nanos as u64))
    }
}

// --- workload enforcement probe (host.ObserveHostResourceEnforcement) --------------

const SYSTEMCTL: &str = "/usr/bin/systemctl";
const SYSTEMD_RUN: &str = "/usr/bin/systemd-run";
const WORKLOAD_SLICE: &str = "opute-workload.slice";

fn parse_systemd_properties(output: &str) -> std::collections::BTreeMap<String, String> {
    output
        .split('\n')
        .filter_map(|line| line.split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect()
}

fn parse_systemd_limit(value: &str) -> Option<i64> {
    let mut value = value.trim().to_lowercase();
    if value.is_empty() || value == "max" || value == "infinity" || value == "inf" {
        return None;
    }
    let mut multiplier: i64 = 1;
    for (suffix, factor) in [
        ("k", 1i64 << 10),
        ("m", 1 << 20),
        ("g", 1 << 30),
        ("t", 1 << 40),
    ] {
        if let Some(stripped) = value.strip_suffix(suffix) {
            value = stripped.trim().to_string();
            multiplier = factor;
            break;
        }
    }
    let parsed: i64 = value.parse().ok()?;
    if parsed <= 0 || parsed > i64::MAX / multiplier {
        return None;
    }
    Some(parsed * multiplier)
}

/// `parseSystemdMicroseconds`: `us`, `ms` or `s` suffixes, else a plain limit.
fn parse_systemd_microseconds(value: &str) -> Option<i64> {
    let value = value.trim().to_lowercase();
    if value.is_empty() || value == "max" || value == "infinity" || value == "inf" {
        return None;
    }
    for (suffix, multiplier) in [("us", 1f64), ("ms", 1_000.0), ("s", 1_000_000.0)] {
        let Some(number) = value.strip_suffix(suffix) else {
            continue;
        };
        let parsed: f64 = number.trim().parse().ok()?;
        if parsed <= 0.0 || parsed > i64::MAX as f64 / multiplier {
            return None;
        }
        return Some((parsed * multiplier) as i64);
    }
    parse_systemd_limit(&value)
}

fn workload_properties_enforced(p: &std::collections::BTreeMap<String, String>) -> bool {
    let get = |k: &str| p.get(k).map(String::as_str).unwrap_or("");
    let limits: [(&str, i64); 6] = [
        ("MemoryHigh", 10 << 30),
        ("MemoryMax", 11 << 30),
        ("MemorySwapMax", 1 << 30),
        ("CPUQuotaPerSecUSec", 6_000_000),
        ("CPUWeight", 100),
        ("TasksMax", 4096),
    ];
    limits.iter().all(|(name, maximum)| {
        let value = if *name == "CPUQuotaPerSecUSec" {
            parse_systemd_microseconds(get(name))
        } else {
            parse_systemd_limit(get(name))
        };
        value.is_some_and(|v| v > 0 && v <= *maximum)
    })
}

fn cgroup_controls_available(control_group: &str) -> bool {
    let group = control_group.trim();
    if group.is_empty() || !group.starts_with('/') {
        return false;
    }
    let path = clean_path(&format!("{CGROUP_ROOT}/{}", group.trim_start_matches('/')));
    if !(path == CGROUP_ROOT || path.starts_with(&format!("{CGROUP_ROOT}/"))) {
        return false;
    }
    ["memory.max", "cpu.max", "pids.max"]
        .iter()
        .all(|n| Path::new(&path).join(n).exists())
}

fn systemd_scopes() -> Vec<&'static str> {
    if let Ok(v) = std::fs::read_to_string("/proc/self/cgroup") {
        if v.contains("/user.slice/") {
            return vec!["user"];
        }
        if v.contains("/system.slice/") {
            return vec!["system"];
        }
    }
    vec!["user", "system"]
}

/// `ObserveHostResourceEnforcement`: whether the workload slice's limits are
/// configured and its cgroup controls exist. Like Go, it may start a no-op
/// member of the slice so systemd materializes the cgroup.
pub fn observe_enforcement(env: &Env) -> String {
    let run = |argv: Vec<String>| run_command(env, &argv, Duration::from_secs(5));
    for scope in systemd_scopes() {
        let systemctl = |args: &[&str]| {
            let mut argv = vec![SYSTEMCTL.to_string()];
            if scope == "user" {
                argv.push("--user".into());
            }
            argv.extend(args.iter().map(|s| s.to_string()));
            let res = run(argv);
            (res.exit_code == 0).then(|| parse_systemd_properties(&res.stdout))
        };
        let Some(properties) = systemctl(&[
            "show",
            WORKLOAD_SLICE,
            "--property=ControlGroup,MemoryHigh,MemoryMax,MemorySwapMax,CPUQuotaPerSecUSec,CPUWeight,TasksMax",
        ]) else {
            continue;
        };
        if !workload_properties_enforced(&properties) {
            continue;
        }
        if cgroup_controls_available(
            properties
                .get("ControlGroup")
                .map(String::as_str)
                .unwrap_or(""),
        ) {
            return "enforced".into();
        }
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let mut probe = vec![SYSTEMD_RUN.to_string()];
        if scope == "user" {
            probe.push("--user".into());
        }
        probe.extend([
            format!("--unit=opute-host-resource-probe-{nanos}"),
            "--wait".into(),
            "--collect".into(),
            "--pipe".into(),
            format!("--property=Slice={WORKLOAD_SLICE}"),
            "/usr/bin/true".into(),
        ]);
        if run(probe).exit_code != 0 {
            continue;
        }
        if let Some(refreshed) = systemctl(&["show", WORKLOAD_SLICE, "--property=ControlGroup"]) {
            if cgroup_controls_available(
                refreshed
                    .get("ControlGroup")
                    .map(String::as_str)
                    .unwrap_or(""),
            ) {
                return "enforced".into();
            }
        }
    }
    "unknown".into()
}

// --- agent installation (host.describeAgentInstallation) ----------------------------

pub fn agent_installation(
    env: &Env,
    agent_id: &str,
    instance_id: &str,
    instance_root: &str,
    mcp_port: i64,
) -> J {
    let mut m = Map::new();
    let put = |m: &mut Map<String, J>, k: &str, v: String| {
        if !v.is_empty() {
            m.insert(k.into(), J::from(v));
        }
    };
    put(&mut m, "agentId", agent_id.trim().to_string());
    put(&mut m, "instanceId", instance_id.trim().to_string());
    let instance_root = instance_root.trim().to_string();
    put(&mut m, "instanceRoot", instance_root.clone());
    let home = env.get("HOME").trim().to_string();
    let home = if home.is_empty() {
        nix::unistd::User::from_uid(nix::unistd::geteuid())
            .ok()
            .flatten()
            .map(|u| u.dir.to_string_lossy().into_owned())
            .unwrap_or_default()
    } else {
        home
    };
    if !instance_root.is_empty() {
        let candidate = Path::new(&instance_root).join("host-agent.env");
        if std::fs::metadata(&candidate).is_ok_and(|m| m.is_file()) {
            put(
                &mut m,
                "environmentFile",
                candidate.to_string_lossy().into_owned(),
            );
        }
    }
    if !home.is_empty() {
        put(&mut m, "homeDir", home.clone());
        put(
            &mut m,
            "providerRoot",
            format!("{home}/.local/share/opute/providers"),
        );
    }
    let system = nix::unistd::geteuid().is_root();
    put(
        &mut m,
        "serviceScope",
        if system { "system" } else { "user" }.into(),
    );
    if system {
        put(&mut m, "serviceUnitDir", "/etc/systemd/system".into());
        put(&mut m, "serviceWantedBy", "multi-user.target".into());
    } else {
        if !home.is_empty() {
            put(
                &mut m,
                "serviceUnitDir",
                format!("{home}/.config/systemd/user"),
            );
        }
        put(&mut m, "serviceWantedBy", "default.target".into());
    }
    if mcp_port > 0 {
        put(
            &mut m,
            "mcpEndpoint",
            format!("http://127.0.0.1:{mcp_port}/mcp"),
        );
    }
    J::Object(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_print_like_go() {
        assert_eq!(go_duration(Duration::from_secs(45)), "45s");
        assert_eq!(go_duration(Duration::from_secs(120)), "2m0s");
    }
}
