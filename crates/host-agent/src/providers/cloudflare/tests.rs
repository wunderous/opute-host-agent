//! The Go provider's pure unit tests, ported; the rest run against this
//! executable as black boxes (`provider_wire.py`).

use super::mesh::{command_diagnostics, find_mesh_interface, find_mesh_ip, MESH_INSTALL_SCRIPT};
use super::*;

#[test]
fn find_mesh_ip_accepts_ip_command_cidr() {
    let output = "2: enp5s0    inet 10.0.100.56/24 scope global enp5s0\n4: CloudflareWARP    inet 100.96.0.2/32 scope global CloudflareWARP";
    assert_eq!(find_mesh_ip(output), "100.96.0.2");
    assert_eq!(find_mesh_interface(output, "100.96.0.2"), "CloudflareWARP");
}

#[test]
fn find_mesh_ip_rejects_non_mesh_and_cgnat_addresses() {
    assert_eq!(
        find_mesh_ip("2: enp5s0 inet 10.0.100.56/24 scope global enp5s0"),
        ""
    );
    assert_eq!(
        find_mesh_ip("2: enp5s0 inet 100.112.0.1/24 scope global enp5s0"),
        ""
    );
    assert_eq!(find_mesh_ip("(100.111.255.255),"), "100.111.255.255");
}

#[test]
fn diagnostics_keep_the_failure_tail_over_install_chatter() {
    let mut lines: Vec<String> = (0..20)
        .map(|i| format!("Setting up cloudflare-warp-dep-{i} ..."))
        .collect();
    lines.push("Status update: Registration missing".into());
    lines.push(
        "warp-cli error: unable to reach the Zero Trust registration endpoint secret-token".into(),
    );
    lines.push("Status update: Disconnected".into());
    let diagnostics = command_diagnostics(&lines.join("\n"), "secret-token");
    for want in [
        "Registration missing",
        "unable to reach",
        "Disconnected",
        "[redacted]",
    ] {
        assert!(diagnostics.contains(want), "{diagnostics}");
    }
    assert!(!diagnostics.contains("cloudflare-warp-dep-0 "));
    assert!(!diagnostics.contains("secret-token"));
    assert_eq!(
        command_diagnostics("nothing here", ""),
        "guest command returned no diagnostic lines"
    );
}

#[test]
fn diagnostics_cut_lines_at_240_bytes_as_go_does() {
    let line = format!("status {}é", "x".repeat(232));
    let diagnostics = command_diagnostics(&line, "");
    assert!(diagnostics.ends_with("x\u{FFFD}"), "{diagnostics}");
}

#[test]
fn install_script_requires_ipv6_and_bounds_warp_setup() {
    for want in [
        "/proc/net/if_inet6",
        "exit 78",
        "timeout 45s warp-cli --accept-tos connector new",
        "seq 1 150",
    ] {
        assert!(MESH_INSTALL_SCRIPT.contains(want), "{want}");
    }
}

#[test]
fn hostnames_from_args_accepts_two_zones() {
    let args = Args::from_map(
        json!({"hostnames": ["agent.example.test", "mcp.other.test"]})
            .as_object()
            .cloned()
            .unwrap(),
    );
    assert_eq!(
        hostnames_from_args(&args),
        vec!["agent.example.test", "mcp.other.test"]
    );
}

#[test]
fn validate_local_target_accepts_host_agent_mcp() {
    assert!(validate_local_target("host-agent-mcp", "host", "").is_ok());
    assert!(validate_local_target("http://127.0.0.1:3004/mcp", "host", "").is_ok());
    assert!(validate_local_target("https://agent.example.test/mcp", "host", "").is_err());
    assert!(
        validate_local_target("http://10.0.200.1:3005", "host", "host-opute-ha-b-b9234af4").is_ok()
    );
}

#[test]
fn tunnel_names_and_identifiers_are_checked() {
    assert!(api::require_tunnel_name(" ").is_err());
    assert!(api::require_tunnel_name("any-tunnel").is_ok());
    assert!(api::identifier_ok("abc_-1"));
    assert!(!api::identifier_ok("not valid"));
}

/// The typed refusal's code, or None when the call was not refused that way.
fn refusal_code(result: Result<(), String>) -> Option<String> {
    let error = result.err()?;
    let typed = error.strip_prefix(crate::providers::TYPED)?;
    let value: J = serde_json::from_str(typed).ok()?;
    value["code"].as_str().map(str::to_string)
}

#[test]
fn nothing_is_reserved_by_default() {
    api::with_reserved("", || {
        assert!(api::refuse_reserved_hostname("mcp.opute.io").is_ok());
        assert!(api::refuse_reserved_hostname("platform.opute.io").is_ok());
        assert!(
            api::refuse_reserved_hostname(" ").is_err(),
            "a hostname is still required"
        );
    });
}

#[test]
fn reserved_names_come_from_the_operator() {
    api::with_reserved("*.opute.io, exact.example.test", || {
        let code = refusal_code(api::refuse_reserved_hostname("MCP.opute.io"));
        assert_eq!(code.as_deref(), Some("reserved_hostname"));
        let code = refusal_code(api::refuse_reserved_hostname("exact.example.test"));
        assert_eq!(code.as_deref(), Some("reserved_hostname"));
        // A suffix pattern matches names under it, not the bare suffix.
        assert!(api::refuse_reserved_hostname("opute.io").is_ok());
        assert!(api::refuse_reserved_hostname("sub.exact.example.test").is_ok());
        assert!(api::refuse_reserved_hostname("agent.example.test").is_ok());
        let rules = [api::IngressRule {
            hostname: "mcp.opute.io".into(),
            service: "http://127.0.0.1:1".into(),
            origin_request: None,
        }];
        let code = refusal_code(api::refuse_reserved_ingress(&rules));
        assert_eq!(code.as_deref(), Some("reserved_hostname"));
    });
}

#[test]
fn a_reserved_name_is_refused_before_the_cloudflare_api() {
    // No Cloudflare credentials: any call that reached the API would fail
    // on them instead.
    api::with_reserved("*.opute.io", || {
        let args = Args::from_map(
            json!({"hostname": "mcp.opute.io", "tunnelName": "t", "localTarget": "http://127.0.0.1:3014"})
                .as_object()
                .unwrap()
                .clone(),
        );
        let code = refusal_code(api::ensure_dedicated_tunnel(&args).map(|_| ()));
        assert_eq!(code.as_deref(), Some("reserved_hostname"));
        let code = refusal_code(api::unpublish_dedicated_tunnel(&args));
        assert_eq!(code.as_deref(), Some("reserved_hostname"));
    });
}

#[test]
fn every_manifest_operation_is_a_tool() {
    let server = server();
    let names: Vec<&str> = server.tools.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names.len(), 1 + 13 + 4 + 3 + 3 + 1, "{names:?}");
    assert!(names.contains(&"opute.capability.public-ingress.probe"));
}

/// D28 (Q7): null arguments to a host-tunnel operation are refused as no
/// arguments; the process keeps serving (Go's nil-map panic exits it).
#[test]
fn null_host_tunnel_arguments_are_refused() {
    for operation in [
        "opute.capability.tunneling.ensure-host-tunnel",
        "ensure_cloudflared_tunnel",
        "create_cloudflare_tunnel",
    ] {
        let args = Args {
            map: Map::new(),
            nil: true,
        };
        let e = dispatch("", operation, args).unwrap_err();
        assert_eq!(e, "bindingId and localTarget are required", "{operation}");
    }
}
