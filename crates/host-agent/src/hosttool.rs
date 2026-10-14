//! `ensure_host_tool` (`internal/domain/host/tool.go`): a small allowlist of
//! generic build/runtime tools, installed through the package manager under
//! non-interactive sudo, or user-locally for Bun and Helm.

use std::time::{Duration, Instant};

use serde_json::{json, Value as J};

use crate::config::Env;
use crate::goerr::shell_quote;
use crate::hostexec::{run_host_until, run_privileged_package};
use crate::hostobs::look_path;
use crate::hostread::home_dir;
use crate::incus::first_non_empty;

/// `hostToolPackages`.
fn package_for(tool: &str) -> Option<&'static str> {
    Some(match tool {
        "gcc" => "gcc",
        "g++" => "g++",
        "go" => "golang-go",
        "podman" => "podman",
        "buildah" => "buildah",
        "buildkitd" => "moby-buildkit",
        "cloudflared" => "cloudflared",
        "cmake" => "cmake",
        "ninja" => "ninja-build",
        "nvcc" => "nvidia-cuda-toolkit",
        _ => return None,
    })
}

fn available(tool: &str, path: &str, already: bool) -> J {
    json!({"tool": tool, "path": path, "available": true, "alreadyAvailable": already})
}

/// `Service.EnsureHostTool`.
pub fn ensure_host_tool(
    env: &Env,
    owner: &crate::hostfiles::Owner,
    tool: &str,
) -> Result<J, String> {
    crate::hostfiles::require_shared_host_owner(
        owner.instance_id,
        owner.owner_instance,
        "ensure_host_tool",
    )?;
    let tool = tool.trim().to_lowercase();
    if tool == "bun" {
        return ensure_bun(env);
    }
    if tool == "helm" {
        return ensure_helm(env);
    }
    let Some(package) = package_for(&tool) else {
        return Err("tool must be one of bun, gcc, g++, go, podman, buildah, buildkitd, cloudflared, helm, cmake, ninja, or nvcc".into());
    };
    if let Some(path) = look_path(env, &tool) {
        return Ok(available(&tool, &path.display().to_string(), true));
    }
    if look_path(env, "apt-get").is_none() {
        return Err(format!(
            "{tool} is not installed and apt-get is unavailable"
        ));
    }
    let deadline = Instant::now() + Duration::from_secs(5 * 60);
    run_privileged_package(env, deadline, "apt-get", &["update"])
        .map_err(|e| format!("update apt package indexes: {e}"))?;
    run_privileged_package(env, deadline, "apt-get", &["install", "-y", package])
        .map_err(|e| format!("install host tool {tool}: {e}"))?;
    match look_path(env, &tool) {
        Some(path) => Ok(available(&tool, &path.display().to_string(), false)),
        None => Err(format!(
            "host tool {tool} was installed but remains unavailable: exec: {}: executable file not found in $PATH",
            crate::goerr::quote(&tool)
        )),
    }
}

fn user_home(tool: &str) -> Result<String, String> {
    match home_dir() {
        Ok(home) if !home.trim().is_empty() => Ok(home),
        _ => Err(format!(
            "{tool} is not installed and user home is unavailable for a user-local install"
        )),
    }
}

/// A user-local installer script, run on the host under a ten-minute context.
fn run_installer(env: &Env, script: String, fallback: &str) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(10 * 60);
    let argv = vec!["bash".to_string(), "-lc".to_string(), script];
    let result = run_host_until(env, &argv, deadline);
    if result.exit_code != 0 {
        return Err(first_non_empty(&[&result.stderr, &result.stdout, fallback]).to_string());
    }
    Ok(())
}

/// `ensureBunTool`: `~/.bun/bin/bun`, installed by the upstream script.
fn ensure_bun(env: &Env) -> Result<J, String> {
    let home = user_home("bun")?;
    let bun = format!("{}/.bun/bin/bun", home.trim_end_matches('/'));
    let bun = crate::hostread::clean(&bun);
    if std::fs::metadata(&bun).is_ok() {
        return Ok(available("bun", &bun, true));
    }
    run_installer(
        env,
        format!(
            "curl -fsSL https://bun.sh/install | bash -s -- --no-modify-shell && test -x {}",
            shell_quote(&bun)
        ),
        "bun install failed",
    )?;
    std::fs::metadata(&bun).map_err(|e| {
        format!(
            "bun was installed but remains unavailable: {}",
            crate::goerr::path_error("stat", std::path::Path::new(&bun), &e).0
        )
    })?;
    Ok(available("bun", &bun, false))
}

/// `ensureHelmTool`: PATH first, else `~/.local/bin/helm` from the upstream
/// script.
fn ensure_helm(env: &Env) -> Result<J, String> {
    if let Some(path) = look_path(env, "helm") {
        return Ok(available("helm", &path.display().to_string(), true));
    }
    let home = user_home("helm")?;
    let dir = crate::hostread::clean(&format!("{}/.local/bin", home.trim_end_matches('/')));
    run_installer(
        env,
        format!(
            "mkdir -p {dir} && curl -fsSL https://raw.githubusercontent.com/helm/helm/main/scripts/get-helm-3 | HELM_INSTALL_DIR={dir} USE_SUDO=false bash",
            dir = shell_quote(&dir)
        ),
        "helm install failed",
    )?;
    let path = format!("{dir}/helm");
    if let Err(e) = std::fs::metadata(&path) {
        return match look_path(env, "helm") {
            Some(found) => Ok(available("helm", &found.display().to_string(), false)),
            None => Err(format!(
                "helm was installed but remains unavailable: {}",
                crate::goerr::path_error("stat", std::path::Path::new(&path), &e).0
            )),
        };
    }
    Ok(available("helm", &path, false))
}
