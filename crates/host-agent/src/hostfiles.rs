//! The host domain's managed-file mutations (`internal/domain/host/file.go`):
//! `ensure_host_file` and `remove_host_file`. Paths resolve under the same
//! scope rules as `inspect_host_file` (`hostread`): user scope beneath the
//! account home through no symlink, system scope one systemd service unit.

use crate::goerr::{path_error, quote};
use crate::hostread::{home_dir, host_owned_path, sha256_label, systemd_unit_path};
use serde_json::{json, Value as J};
use std::io::{self, Read, Write};
use std::path::Path;

/// `Shared.RequireSharedHostOwner`: when an owner instance is configured,
/// only that instance may change the shared host.
pub fn require_shared_host_owner(
    instance_id: &str,
    owner_instance: &str,
    operation: &str,
) -> Result<(), String> {
    let expected = owner_instance.trim();
    if expected.is_empty() || instance_id.trim() == expected {
        return Ok(());
    }
    // `json.Marshal(SharedHostOwnershipError)`, in field order.
    let mut out =
        String::from("{\"code\":\"shared_host_ownership_required\",\"expectedInstance\":");
    crate::gojson::encode_string(expected, &mut out);
    out.push_str(",\"actualInstance\":");
    crate::gojson::encode_string(instance_id, &mut out);
    out.push_str(",\"operation\":");
    crate::gojson::encode_string(operation.trim(), &mut out);
    out.push_str(",\"remediation\":\"Select the shared-host owner instance or use the approved operator workflow.\"}");
    Err(out)
}

/// The owner check every managed-file mutation runs first.
pub struct Owner<'a> {
    pub instance_id: &'a str,
    pub owner_instance: &'a str,
}

/// `os.ReadFile`, reporting the failing step (`open` or `read`) as Go does.
fn read_file(path: &str) -> Result<Vec<u8>, (io::Error, String)> {
    let mut file = std::fs::File::open(path)
        .map_err(|e| (clone_error(&e), path_error("open", Path::new(path), &e).0))?;
    let mut content = Vec::new();
    file.read_to_end(&mut content)
        .map_err(|e| (clone_error(&e), path_error("read", Path::new(path), &e).0))?;
    Ok(content)
}

fn clone_error(e: &io::Error) -> io::Error {
    match e.raw_os_error() {
        Some(code) => io::Error::from_raw_os_error(code),
        None => io::Error::new(e.kind(), e.to_string()),
    }
}

/// The scope a mutation names, and the path it resolves to.
fn scoped_path(scope: &str, raw: &str) -> Result<(String, String), String> {
    let scope = scope.trim().to_lowercase();
    let scope = if scope.is_empty() {
        "user".to_string()
    } else {
        scope
    };
    let path = match scope.as_str() {
        "user" => {
            let home = home_dir().map_err(|e| format!("resolve home directory: {e}"))?;
            host_owned_path(&home, raw)?
        }
        "system" => systemd_unit_path(raw)?,
        _ => return Err("scope must be user or system".into()),
    };
    Ok((scope, path))
}

/// `Service.EnsureHostFile`: write a caller-declared managed file atomically.
pub fn ensure_host_file(
    owner: &Owner,
    raw_path: &str,
    content: &str,
    mode: i64,
    scope: &str,
) -> Result<J, String> {
    use std::os::unix::fs::PermissionsExt;
    require_shared_host_owner(owner.instance_id, owner.owner_instance, "ensure_host_file")?;
    let (scope, path) = scoped_path(scope, raw_path)?;
    // `os.FileMode(args.Mode)`: the low 32 bits.
    let perm = if mode == 0 {
        0o600
    } else {
        (mode as u32) & 0o777
    };
    if !(0o600..=0o755).contains(&perm) {
        return Err("mode must be between 0600 and 0755".into());
    }
    let content_sha256 = sha256_label(content.as_bytes());
    let mut changed = true;
    match read_file(&path) {
        Ok(existing) => {
            if let Ok(meta) = std::fs::metadata(&path) {
                if existing == content.as_bytes() && meta.permissions().mode() & 0o777 == perm {
                    changed = false;
                }
            }
        }
        Err((e, _)) if e.kind() == io::ErrorKind::NotFound => {}
        Err((_, text)) => return Err(format!("read managed host file: {text}")),
    }
    if changed {
        let dir = Path::new(&path)
            .parent()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| ".".into());
        crate::goerr::mkdir_all(Path::new(&dir), 0o700)
            .map_err(|e| format!("create managed host file directory: {}", e.0))?;
        let (mut file, temporary) = crate::goerr::create_temp(&dir, ".opute-host-file-", "")
            .map_err(|e| format!("create managed host file temporary: {e}"))?;
        // `defer os.Remove(temporaryPath)`.
        struct Remove<'a>(&'a str);
        impl Drop for Remove<'_> {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(self.0);
            }
        }
        let _cleanup = Remove(&temporary);
        file.set_permissions(std::fs::Permissions::from_mode(perm))
            .map_err(|e| {
                format!(
                    "set managed host file mode: {}",
                    path_error("chmod", Path::new(&temporary), &e).0
                )
            })?;
        file.write_all(content.as_bytes()).map_err(|e| {
            format!(
                "write managed host file: {}",
                path_error("write", Path::new(&temporary), &e).0
            )
        })?;
        drop(file);
        std::fs::rename(&temporary, &path).map_err(|e| {
            format!(
                "install managed host file: rename {temporary} {path}: {}",
                crate::goerr::errno_text(&e)
            )
        })?;
    }
    Ok(json!({
        "path": path,
        "scope": scope,
        "changed": changed,
        "contentSha256": content_sha256,
        "mode": format!("{perm:o}"),
    }))
}

/// `Service.RemoveHostFile`: delete one caller-owned regular file after an
/// explicit confirmation and an optional content hash match.
pub fn remove_host_file(
    owner: &Owner,
    raw_path: &str,
    expected_sha256: &str,
    confirm: bool,
    scope: &str,
) -> Result<J, String> {
    require_shared_host_owner(owner.instance_id, owner.owner_instance, "remove_host_file")?;
    if !confirm {
        return Err("remove_host_file requires confirm=true".into());
    }
    let (scope, path) = scoped_path(scope, raw_path)?;
    let meta = match std::fs::symlink_metadata(&path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Ok(json!({"path": path, "scope": scope, "exists": false, "removed": false}))
        }
        Err(e) => {
            return Err(format!(
                "inspect removable host file: {}",
                path_error("lstat", Path::new(&path), &e).0
            ))
        }
        Ok(meta) => meta,
    };
    if !meta.file_type().is_file() {
        return Err(format!(
            "remove_host_file refuses non-regular path {}",
            quote(&path)
        ));
    }
    let content =
        read_file(&path).map_err(|(_, text)| format!("read removable host file: {text}"))?;
    let observed = sha256_label(&content);
    let expected = expected_sha256.trim();
    if !expected.is_empty() {
        let expected = format!(
            "sha256:{}",
            expected.strip_prefix("sha256:").unwrap_or(expected)
        );
        if !crate::gojson::equal_fold(&expected, &observed) {
            return Err(format!(
                "remove_host_file hash mismatch for {}",
                quote(&path)
            ));
        }
    }
    std::fs::remove_file(&path).map_err(|e| {
        format!(
            "remove host file: {}",
            path_error("remove", Path::new(&path), &e).0
        )
    })?;
    Ok(json!({"path": path, "scope": scope, "exists": true, "removed": true, "sha256": observed}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ownership_error_is_go_json() {
        assert!(require_shared_host_owner("a", "", "x").is_ok());
        assert!(require_shared_host_owner(" a ", "a", "x").is_ok());
        assert_eq!(
            require_shared_host_owner("b<", "a", "ensure_host_file").unwrap_err(),
            "{\"code\":\"shared_host_ownership_required\",\"expectedInstance\":\"a\",\
             \"actualInstance\":\"b\\u003c\",\"operation\":\"ensure_host_file\",\
             \"remediation\":\"Select the shared-host owner instance or use the approved operator workflow.\"}"
        );
    }
}
