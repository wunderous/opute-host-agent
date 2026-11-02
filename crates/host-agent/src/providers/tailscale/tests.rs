//! The Go provider's pure unit tests, ported; the rest run against this
//! executable as black boxes (`provider_wire.py`).

use super::live::{find_tailscale_ipv4, normalize_loopback_http, sanitize_hostname};

#[test]
fn sanitize_hostname_maps_runes_and_trims() {
    assert_eq!(sanitize_hostname("  Opute HA/A. "), "opute-ha-a");
    assert_eq!(sanitize_hostname("--x--"), "x");
}

#[test]
fn find_tailscale_ipv4_prefers_the_script_line() {
    assert_eq!(find_tailscale_ipv4("noise 100.70.1.2\nmesh_ip=100.64.0.9\n"), "100.64.0.9");
    assert_eq!(find_tailscale_ipv4("inet 10.0.0.1/24 inet (100.101.2.3/32)"), "100.101.2.3");
    assert_eq!(find_tailscale_ipv4("inet 10.0.0.1/24"), "");
}

#[test]
fn loopback_targets_follow_go() {
    assert_eq!(normalize_loopback_http("8080").unwrap(), "http://127.0.0.1:8080");
    assert_eq!(normalize_loopback_http("http").unwrap(), "http://127.0.0.1:http");
    assert_eq!(normalize_loopback_http("HTTP://localhost:80/x#y").unwrap(), "HTTP://localhost:80/x#y");
    assert_eq!(normalize_loopback_http("http://[::1]:80/").unwrap(), "http://[::1]:80/");
    assert_eq!(normalize_loopback_http("http://127.1/").unwrap_err(), "localTarget must target loopback");
    assert_eq!(normalize_loopback_http("http://LOCALHOST/").unwrap_err(), "localTarget must target loopback");
    assert_eq!(normalize_loopback_http("ftp://localhost/").unwrap_err(), "localTarget must be an HTTP(S) URL");
    assert_eq!(normalize_loopback_http("nope").unwrap_err(), "localTarget must be an HTTP URL or port");
}
