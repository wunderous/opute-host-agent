//! The Go provider's pure unit tests, ported; the rest run against this
//! executable as black boxes (`provider_wire.py`).

use super::live::{enroll_script, find_tailscale_ipv4, normalize_loopback_http, sanitize_hostname};

#[test]
fn sanitize_hostname_maps_runes_and_trims() {
    assert_eq!(sanitize_hostname("  Opute HA/A. "), "opute-ha-a");
    assert_eq!(sanitize_hostname("--x--"), "x");
}

#[test]
fn find_tailscale_ipv4_prefers_the_script_line() {
    assert_eq!(
        find_tailscale_ipv4("noise 100.70.1.2\nmesh_ip=100.64.0.9\n"),
        "100.64.0.9"
    );
    assert_eq!(
        find_tailscale_ipv4("inet 10.0.0.1/24 inet (100.101.2.3/32)"),
        "100.101.2.3"
    );
    assert_eq!(find_tailscale_ipv4("inet 10.0.0.1/24"), "");
}

#[test]
fn loopback_targets_follow_go() {
    assert_eq!(
        normalize_loopback_http("8080").unwrap(),
        "http://127.0.0.1:8080"
    );
    assert_eq!(
        normalize_loopback_http("http").unwrap(),
        "http://127.0.0.1:http"
    );
    assert_eq!(
        normalize_loopback_http("HTTP://localhost:80/x#y").unwrap(),
        "HTTP://localhost:80/x#y"
    );
    assert_eq!(
        normalize_loopback_http("http://[::1]:80/").unwrap(),
        "http://[::1]:80/"
    );
    assert_eq!(
        normalize_loopback_http("http://127.1/").unwrap_err(),
        "localTarget must target loopback"
    );
    assert_eq!(
        normalize_loopback_http("http://LOCALHOST/").unwrap_err(),
        "localTarget must target loopback"
    );
    assert_eq!(
        normalize_loopback_http("ftp://localhost/").unwrap_err(),
        "localTarget must be an HTTP(S) URL"
    );
    assert_eq!(
        normalize_loopback_http("nope").unwrap_err(),
        "localTarget must be an HTTP URL or port"
    );
}

/// m19 (S3): the auth key reaches `tailscale up` through a file read from
/// stdin, never its arguments (any user in the target reads
/// `/proc/PID/cmdline`).
#[test]
fn the_auth_key_never_reaches_tailscales_arguments() {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path().display();
    let stub = |name: &str, body: &str| {
        let path = dir.path().join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    };
    stub(
        "tailscale",
        &format!(
            "printf '%s\\n' \"$@\" >> {d}/argv\ncase \"$1\" in\n  up) for a in \"$@\"; do case \"$a\" in --auth-key=file:*) cat \"${{a#--auth-key=file:}}\" > {d}/key;; esac; done;;\n  ip) echo 100.64.0.7;;\nesac"
        ),
    );
    stub("tailscaled", "exit 0");
    stub("pgrep", "exit 0");
    let key = "tskey-auth-0123456789abcdef0123456789";
    let mut child = std::process::Command::new("bash")
        .arg("-c")
        .arg(enroll_script("node-a"))
        .env("PATH", format!("{d}:/usr/bin:/bin"))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    writeln!(child.stdin.take().unwrap(), "{key}").unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("mesh_ip=100.64.0.7"));
    let argv = std::fs::read_to_string(dir.path().join("argv")).unwrap();
    assert!(argv.contains("up\n"), "{argv}");
    assert!(!argv.contains(key), "the auth key is on tailscale's argv");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("key")).unwrap(),
        key
    );
}
