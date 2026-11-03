//! The Go provider's focused tests (`main_test.go`, `membership_*_test.go`,
//! `storage_test.go`), where they test a pure function. Those that drive a
//! command runner are covered by the wire corpus against a fake `incus`.

use super::bootstrap::{checksum_for_binary, escape_version};
use super::membership::*;
use super::storage::*;
use super::*;
use std::time::{Duration, SystemTime};

fn args(value: J) -> Args {
    Args::from_map(value.as_object().cloned().unwrap())
}

#[test]
fn manifest_declares_neutral_capability_and_operations() {
    let manifest = serde_json::to_value(manifest()).unwrap();
    assert_eq!(manifest["provides"], json!([{"id": KUBERNETES, "version": 1}]));
    let ids: Vec<String> = operations().iter().map(|o| o.id.clone()).collect();
    assert_eq!(ids.len(), 28);
    for operation in operations() {
        let want = if operation.effect == "read" { "sync_only" } else { "bridged" };
        assert_eq!(operation.task_support, want, "{}", operation.id);
    }
    assert_eq!(manifest["teardown"]["id"], "opute.provider.teardown");
    assert_eq!(manifest["teardown"]["taskSupport"], "sync_only");
    let provision = operations().into_iter().find(|o| o.id == op::PROVISION).unwrap();
    assert_eq!(provision.input_schema.unwrap()["properties"]["clusterInit"]["const"], true);
    let prune = operations().into_iter().find(|o| o.id == op::PRUNE_UNUSED_IMAGES).unwrap();
    let cost = serde_json::to_value(prune.resource_cost).unwrap();
    assert_eq!(cost, json!({"class": "heavy", "cpuCores": 2.0, "memoryBytes": 2147483648i64, "tasks": 8}));
}

#[test]
fn teardown_rejects_unowned_system_service_path() {
    assert!(!service_name_ok("../passwd.service"));
    assert!(service_name_ok("opute-provider-k3s-p15-test.service"));
    assert!(!service_name_ok(".service"));
}

#[test]
fn install_exec_binds_native_traffic_to_the_declared_overlay() {
    let install = server_install_exec(
        &args(json!({"nodeIp": "100.96.1.10", "advertiseAddress": "100.96.1.10", "flannelIface": "CloudflareWARP", "tlsSan": "100.96.1.10"})),
        true,
    )
    .unwrap();
    assert_eq!(
        install,
        "server --cluster-init --node-ip 100.96.1.10 --advertise-address 100.96.1.10 --tls-san 100.96.1.10 --flannel-iface CloudflareWARP"
    );
    assert!(server_install_exec(&args(json!({"nodeIp": "not-an-ip"})), false).is_err());
    assert_eq!(
        server_install_exec(&args(json!({"nodeIp": "::ffff:10.0.0.1"})), false).unwrap(),
        "server --node-ip 10.0.0.1"
    );
    let container = server_install_exec(&args(json!({"instanceType": "container"})), false).unwrap();
    assert!(container.contains("--kubelet-arg=feature-gates=KubeletInUserNamespace=true"));
}

#[test]
fn parse_k3s_version_returns_the_installer_release_token() {
    assert_eq!(parse_k3s_version("k3s version v1.31.8+k3s1 (33429f76) go version go1.25.5"), "v1.31.8+k3s1");
    assert_eq!(parse_k3s_version("\n"), "");
}

#[test]
fn join_invitation_is_one_time_and_requires_redemption() {
    let reference = "join-test-one-time".to_string();
    INVITATIONS.lock().unwrap().insert(
        reference.clone(),
        Invitation {
            destination_host_id: String::new(),
            k3s_version: String::new(),
            join_token: "transient-token".into(),
            expires_at: SystemTime::now() + Duration::from_secs(60),
            redeemed: false,
            consumed: false,
        },
    );
    let a = args(json!({"redemptionRef": reference}));
    assert!(join_token_for_node(&a).is_err());
    INVITATIONS.lock().unwrap().get_mut(&reference).unwrap().redeemed = true;
    assert_eq!(join_token_for_node(&a).unwrap(), ("transient-token".into(), reference.clone(), String::new()));
    assert_eq!(join_token_for_node(&a).unwrap_err(), "join redemption reference is expired or already consumed");
    INVITATIONS.lock().unwrap().remove(&reference);
}

#[test]
fn sealed_join_material_round_trips_only_to_the_receiver_key() {
    use rsa::traits::PublicKeyParts as _;
    let key = receiver_key();
    let public = raw_url_encode(&pkcs1_public_der(&key.n().to_bytes_be(), &key.e().to_bytes_be()));
    let sealed = seal_join_material(
        &public,
        &[("redemptionRef", "join-sealed-test"), ("joinToken", "transient-token"), ("expiresAt", "x")],
    )
    .unwrap();
    assert!(!sealed.contains("transient-token"));
    let material = unseal_join_material(&sealed).unwrap();
    assert_eq!(material["redemptionRef"], "join-sealed-test");
    assert_eq!(material["joinToken"], "transient-token");
    assert_eq!(material["sourceTargetUri"], "");
}

#[test]
fn pkcs1_parse_reports_go_asn1_errors() {
    assert_eq!(parse_pkcs1_public_key(b"").unwrap_err(), "asn1: syntax error: sequence truncated");
    assert_eq!(
        parse_pkcs1_public_key(&[0x02, 0x01, 0x01]).unwrap_err(),
        "asn1: structure error: tags don't match (16 vs {class:0 tag:2 length:1 isCompound:false}) {optional:false explicit:false application:false private:false defaultValue:<nil> tag:<nil> stringType:0 timeType:0 set:false omitEmpty:false} pkcs1PublicKey @2"
    );
    assert_eq!(parse_pkcs1_public_key(&[0x30, 0x05, 0x02]).unwrap_err(), "asn1: syntax error: data truncated");
    let der = pkcs1_public_der(&[0xc1; 128], &[0x01, 0x00, 0x01]);
    let mut trailing = der.clone();
    trailing.push(0);
    assert_eq!(parse_pkcs1_public_key(&trailing).unwrap_err(), "asn1: syntax error: trailing data");
    assert_eq!(parse_pkcs1_public_key(&der).unwrap().1, 65537);
}

#[test]
fn target_validation_rejects_fallback_shapes() {
    for value in [
        json!({"targetUri": "vm:local:k3s", "providerInstanceName": "k3s"}),
        json!({"targetUri": "cluster:local:k3s"}),
        json!({"targetUri": "cluster:local:k3s", "providerInstanceName": "k3s", "instanceType": "display-name"}),
    ] {
        assert!(require_target(&args(value)).is_err());
    }
    assert!(require_provision_target(&args(json!({"targetUri": "vm:local:k3s", "providerInstanceName": "k3s"}))).is_ok());
    assert!(require_provision_target(&args(json!({"targetUri": "container:local:k3s", "providerInstanceName": "k3s"}))).is_ok());
    for value in [
        json!({"targetUri": "cluster:local:k3s", "providerInstanceName": "k3s"}),
        json!({"targetUri": "vm:local:k3s"}),
        json!({"targetUri": "vm:local:k3s", "providerInstanceName": "k3s", "instanceType": "display-name"}),
    ] {
        assert!(require_provision_target(&args(value)).is_err());
    }
    assert!(resource_arguments(&args(json!({"kind": "Deployment"}))).is_err());
    assert!(resource_arguments(&args(json!({"resourceName": "cloudflared"}))).is_err());
    assert_eq!(
        string_slice_argument(&args(json!({"kubectlArgs": ["get", "nodes"]})), "kubectlArgs").unwrap(),
        vec!["get", "nodes"]
    );
    assert!(string_slice_argument(&args(json!({"kubectlArgs": ["get", "bad\narg"]})), "kubectlArgs").is_err());
}

#[test]
fn native_membership_uses_ready_condition_and_role_labels() {
    let nodes = parse_native_membership(br#"{"items":[
      {"metadata":{"name":"server-a","labels":{"node-role.kubernetes.io/etcd":"true","node-role.kubernetes.io/control-plane":"true"}},"status":{"conditions":[{"type":"Ready","status":"True"}],"nodeInfo":{"kubeletVersion":"v1.31.8+k3s1"}}},
      {"metadata":{"name":"server-b","labels":{"node-role.kubernetes.io/control-plane":"true"}},"status":{"conditions":[{"type":"MemoryPressure","status":"False"},{"type":"Ready","status":"False"}]}}
    ]}"#)
    .unwrap();
    assert_eq!(
        nodes[0],
        json!({"name": "server-a", "roles": ["control-plane", "etcd"], "ready": true, "status": "Ready", "version": "v1.31.8+k3s1"})
    );
    assert_eq!(nodes[1], json!({"name": "server-b", "roles": ["control-plane"], "ready": false, "status": "NotReady"}));
    assert_eq!(
        parse_native_membership(br#"{"items":[{"metadata":{"name":7}}]}"#).unwrap_err(),
        "invalid native K3s membership response: json: cannot unmarshal number into Go struct field .items.metadata.name of type string"
    );
    assert_eq!(
        parse_native_membership(br#"{"items":[{"metadata":{"name":" "}}]}"#).unwrap_err(),
        "native K3s membership contained a node without a name"
    );
}

#[test]
fn endpoint_validation_rejects_path_query_and_fragment() {
    assert!(validate_endpoint("https://k3s.example.test:6443/path").is_err());
    assert!(validate_endpoint("https://k3s.example.test:6443?token=secret").is_err());
    assert!(validate_endpoint("https://k3s.example.test:6443#x").is_err());
    assert!(validate_endpoint("https://k3s.example.test:6443").is_ok());
}

fn observation(servers: i64, ready: i64, hostname: &str, nodes: Vec<J>) -> Observation {
    Observation { server_count: servers, ready_servers: ready, hostname: hostname.into(), nodes, ..Default::default() }
}

#[test]
fn quorum_after_removal_refuses_to_break_the_cluster() {
    let ready = json!({"name": "server-c", "ready": true});
    let not_ready = json!({"name": "server-c", "ready": false});
    assert_eq!(quorum_after_removal(&observation(3, 3, "", vec![]), &ready).unwrap(), (2, 2));
    assert_eq!(quorum_after_removal(&observation(3, 2, "", vec![]), &not_ready).unwrap(), (2, 2));
    assert!(quorum_after_removal(&observation(1, 1, "", vec![]), &ready).is_err());
    assert_eq!(
        quorum_after_removal(&observation(3, 2, "", vec![]), &ready).unwrap_err(),
        "refusing server removal without source-cluster quorum proof: removal would leave 1 ready of 2 server(s)"
    );
}

#[test]
fn quorum_recovery_is_admissible_only_when_wedged_and_acknowledged() {
    assert!(quorum_recovery_admissible(true, false, true).is_ok());
    assert!(quorum_recovery_admissible(true, false, false).unwrap_err().contains("acknowledgeSingleMemberReset=true"));
    assert!(quorum_recovery_admissible(false, false, true).unwrap_err().contains("requires embedded-etcd"));
    assert!(quorum_recovery_admissible(true, true, true).unwrap_err().contains("within 1m30s"));
    assert_eq!(truncate_probe_error(&format!("  {}  ", "x".repeat(600))).len(), 512);
}

#[test]
fn joined_membership_settles_on_the_joining_node_itself() {
    let node = |name: &str, ready: bool| json!({"name": name, "ready": ready});
    let source = vec![node("source-a", true), node("source-b", true)];
    let mut joining = source.clone();
    joining.push(node("joiner", true));
    let mut not_ready = source.clone();
    not_ready.push(node("joiner", false));
    assert!(joined_settled(&observation(3, 3, "joiner", joining.clone())));
    assert!(!joined_settled(&observation(2, 2, "joiner", source)));
    assert!(!joined_settled(&observation(3, 2, "joiner", not_ready)));
    assert!(!joined_settled(&observation(1, 1, "joiner", vec![node("joiner", true)])));
    assert!(!joined_settled(&observation(3, 3, "", joining)));
}

#[test]
fn keep_set_includes_init_and_ephemeral_containers() {
    let keep = keep_set_from_pod_json(br#"{"items":[{"spec":{
        "initContainers":[{"image":"docker.io/library/busybox:1.36"}],
        "containers":[{"image":"10.0.100.66:30500/opute/platform:abc"}],
        "ephemeralContainers":[{"image":"registry:3"}]}}]}"#);
    for want in ["busybox:1.36", "opute/platform:abc", "registry:3"] {
        assert!(keep.contains(want), "{want}: {keep:?}");
    }
    assert!(keep_set_from_pod_json(b"{").is_empty());
}

#[test]
fn plan_unused_images_honours_keep_set_and_age_gate() {
    let keep: KeepSet = ["nginx:latest".to_string()].into();
    let now: i128 = 1_789_732_800 * 1_000_000_000;
    let image = |id: &str, tag: &str, size: i64, age_minutes: Option<i128>| GuestImage {
        id: id.into(),
        tags: vec![tag.into()],
        digests: vec![],
        size_bytes: size,
        created_at: age_minutes.map(|m| now - m * 60 * 1_000_000_000),
        pinned: false,
    };
    let images = vec![
        image("sha256:keep", "nginx:latest", 10, None),
        image("sha256:fresh", "stale:old", 20, Some(30)),
        image("sha256:old", "stale:older", 40, Some(120)),
        image("sha256:undated", "untagged", 5, None),
    ];
    let (candidates, skipped, missing) = plan_unused_images(&images, &keep, 3600, now);
    assert!(missing);
    assert_eq!(candidates.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["sha256:old", "sha256:undated"]);
    assert_eq!(skipped.len(), 2);
}

#[test]
fn image_aliases_strip_digest_registry_host_and_library() {
    assert_eq!(
        image_aliases("Docker.io/library/Busybox@sha256:AB"),
        ["docker.io/library/busybox@sha256:ab", "docker.io/library/busybox", "sha256:ab", "ab", "library/busybox", "busybox"]
    );
}

#[test]
fn cri_times_and_du_and_df_parse_as_go_does() {
    assert_eq!(parse_cri_time(&json!("1700000000")), Some(1_700_000_000i128 * 1_000_000_000));
    assert_eq!(parse_cri_time(&json!("1700000000000000000")), Some(1_700_000_000_000_000_000));
    assert_eq!(parse_cri_time(&json!("0")), None);
    assert_eq!(parse_cri_time(&json!(-1.0)), None);
    assert_eq!(parse_du_line("123\t/var/lib/a b"), Some((123, "/var/lib/a b".into())));
    assert_eq!(parse_du_line("x /a"), None);
    assert_eq!(
        parse_df_bytes("Filesystem 1B-blocks Used Available Use% Mounted\n/dev/sda1 100 40 60 40% /").unwrap(),
        json!({"filesystem": "/dev/sda1", "totalBytes": 100, "usedBytes": 40, "availBytes": 60, "mount": "/"})
    );
    assert_eq!(parse_df_bytes("only").unwrap_err(), "df output was empty");
    assert_eq!(go_v_float(1.0), "1");
    assert_eq!(go_v_float(1.5), "1.5");
    assert_eq!(go_v_float(1e21), "1e+21");
    assert_eq!(go_v_float(0.00001), "1e-05");
}

#[test]
fn registry_gc_job_omits_config_map_when_the_deploy_has_none() {
    let registry = |config: &str| Registry {
        namespace: "opute-registry".into(),
        name: "opute-registry".into(),
        image: "registry:3".into(),
        replicas: 1,
        claim_name: "opute-registry-data".into(),
        config_name: config.into(),
        service_ip: String::new(),
        service_port: 0,
    };
    let with = registry_gc_job_manifest(&registry("opute-registry"));
    assert!(with.contains("configMap:") && with.contains("mountPath: /etc/distribution"));
    let without = registry_gc_job_manifest(&registry(""));
    assert!(!without.contains("configMap:") && !without.contains("mountPath: /etc/distribution"));
    assert!(without.contains("claimName: opute-registry-data\n"));
}

#[test]
fn release_checksums_and_version_escape() {
    let line = format!("{}  k3s\n", "a".repeat(64));
    assert_eq!(checksum_for_binary(&line).unwrap(), "a".repeat(64));
    let bad = format!("{}g  *dist/k3s\n", "a".repeat(63));
    assert_eq!(
        checksum_for_binary(&bad).unwrap_err(),
        "K3s release checksum is not hexadecimal: encoding/hex: invalid byte: U+0067 'g'"
    );
    assert!(checksum_for_binary("x k3s").is_err());
    assert_eq!(escape_version("v1.31.8+k3s1"), "v1.31.8%2Bk3s1");
}
