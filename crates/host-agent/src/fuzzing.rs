//! Byte-level entry points for the fuzz targets under `fuzz/` (m16).
//!
//! Each function takes untrusted bytes the way the agent receives them and
//! must return without panicking, hanging or breaking the oracle it states.
//! `evidence/hardening/entry-points.json` maps every place untrusted bytes
//! enter the agent to one of these targets or to a property test. The
//! `replay` test runs each target over its committed seeds and regressions,
//! so a crash a fuzzer found stays a failing test on the stable toolchain.

use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use serde_json::Value as J;

/// One target: the name its fuzz binary and corpus directory use, and its entry.
pub type Target = (&'static str, fn(&[u8]));

/// Every target.
pub const TARGETS: &[Target] = &[
    ("http_wire", http_wire),
    ("json_document", json_document),
    ("tool_arguments", tool_arguments),
    ("host_archive", host_archive),
    ("tar_stream", tar_stream),
    ("plan_document", plan_document),
    ("host_recipe", host_recipe),
    ("resource_identity", resource_identity),
    ("env_file", env_file),
    ("state_store", state_store),
    ("reservation_file", reservation_file),
    ("credential_file", credential_file),
    ("provider_descriptor", provider_descriptor),
    ("provider_result", provider_result),
    ("incus_output", incus_output),
    ("command_output", command_output),
    ("console_output", console_output),
    ("authz_store", authz_store),
];

static PANICKED: AtomicBool = AtomicBool::new(false);
static SCRATCH: AtomicU64 = AtomicU64::new(0);

/// A panic on any thread (a server task tokio would otherwise swallow) is
/// recorded so the input that caused it fails.
fn watch_panics() {
    static HOOK: OnceLock<()> = OnceLock::new();
    HOOK.get_or_init(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            PANICKED.store(true, Ordering::SeqCst);
            previous(info);
        }));
    });
}

fn root() -> &'static Path {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        let dir = std::env::temp_dir().join(format!("opute-fuzz-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create fuzz root");
        dir
    })
}

/// A fresh directory for one input, removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Scratch {
        let n = SCRATCH.fetch_add(1, Ordering::SeqCst);
        let dir = root().join(format!("s{n}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create scratch");
        Scratch(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        open_up(&self.0);
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Give the owner rwx on every directory below `dir`: an archive may set a
/// directory's mode to anything (0o377 included), as Go's extraction does.
fn open_up(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                open_up(&entry.path());
            }
        }
    }
}

fn text(data: &[u8]) -> String {
    String::from_utf8_lossy(data).into_owned()
}

// --- the MCP and OAuth wire ---------------------------------------------------------

struct Wire {
    addr: std::net::SocketAddr,
    _home: PathBuf,
}

/// One standalone agent composed as `app::run` does, with a state directory
/// under the fuzz root and an empty PATH so no host command can run. The
/// fuzzed agent has no bootstrap token, so no input reaches a tool.
fn wire() -> &'static Wire {
    static WIRE: OnceLock<Wire> = OnceLock::new();
    WIRE.get_or_init(|| compose_wire("wire", &[]))
}

fn compose_wire(name: &str, extra: &[(&str, &str)]) -> Wire {
    {
        let home = root().join(name);
        let empty_path = home.join("empty-path");
        std::fs::create_dir_all(&empty_path).expect("create wire home");
        let at = |p: &str| home.join(p).to_string_lossy().into_owned();
        let env = crate::config::Env::from_pairs([
            ("HOME", at("home")),
            ("XDG_CONFIG_HOME", at("xdg")),
            ("PATH", empty_path.to_string_lossy().into_owned()),
            ("OPUTE_REMOTE_AGENT_ID", "agent-under-fuzz".into()),
            ("OPUTE_AGENT_MODE", "standalone".into()),
            ("OPUTE_STANDALONE_STATE_DIR", at("state")),
            ("HOST_MCP_BIND_HOST", "127.0.0.1".into()),
            ("HOST_MCP_PORT", "1".into()),
        ]);
        let mut env = env;
        for (k, v) in extra {
            env.set(k, v);
        }
        let runtime = crate::app::new_runtime(&env).expect("fuzz runtime");
        let cfg = runtime.config.clone();
        let authz =
            crate::store::AuthzStore::open(&cfg.standalone_state_dir, &cfg.opute_client_secret)
                .expect("fuzz authz");
        let server =
            crate::app::http_server(&cfg, authz, runtime.state.clone()).expect("fuzz server");
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind fuzz listener");
        listener.set_nonblocking(true).expect("nonblocking");
        let addr = listener.local_addr().expect("listener addr");
        std::thread::Builder::new()
            .name("fuzz-wire".into())
            .stack_size(256 << 20)
            .spawn(move || {
                let _runtime = runtime;
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_io()
                    .enable_time()
                    .build()
                    .expect("fuzz tokio");
                let local = tokio::task::LocalSet::new();
                local.block_on(&rt, async move {
                    server.providers.bind(&server);
                    let listener = tokio::net::TcpListener::from_std(listener).expect("listener");
                    loop {
                        if let Ok((stream, _)) = listener.accept().await {
                            let server = server.clone();
                            tokio::task::spawn_local(crate::http1::serve_conn(
                                stream,
                                move |req, body| crate::transport::route(server.clone(), req, body),
                            ));
                        }
                    }
                });
            })
            .expect("spawn fuzz server");
        Wire { addr, _home: home }
    }
}

/// Raw bytes on one connection to the agent's HTTP listener. Oracle: the
/// agent answers or closes within 10 s and no thread panics.
pub fn http_wire(data: &[u8]) {
    watch_panics();
    let wire = wire();
    let mut stream = std::net::TcpStream::connect(wire.addr).expect("connect fuzz server");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("read timeout");
    // A peer that closes early is an ordinary outcome, not a finding.
    let _ = stream.write_all(data);
    let _ = stream.shutdown(std::net::Shutdown::Write);
    let mut reply = Vec::new();
    match stream.read_to_end(&mut reply) {
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
            ) =>
        {
            panic!("the agent neither answered nor closed within 10s")
        }
        _ => {}
    }
    assert!(!PANICKED.load(Ordering::SeqCst), "a server thread panicked");
}

// --- documents ----------------------------------------------------------------------

/// Oracle: Go's scanner (`json.Valid`) and the tree parser agree on whether
/// the bytes are one JSON document, and a parsed document re-encodes to
/// bytes that parse to the same value.
pub fn json_document(data: &[u8]) {
    let parsed = crate::gojson::parse(data);
    let scanned = crate::gojson::syntax_error(data);
    assert_eq!(
        parsed.is_ok(),
        scanned.is_none(),
        "parser and scanner disagree: {scanned:?}"
    );
    let _ = crate::gojson::first_value(data);
    let _ = crate::gojson::validate_syntax(data);
    if let Ok(node) = parsed {
        let value = crate::transport::go_any(&node);
        let encoded = crate::gojson::encode_to_string(&value);
        let again = crate::gojson::parse(encoded.as_bytes()).expect("re-encoded JSON parses");
        assert_eq!(
            crate::transport::go_any(&again),
            value,
            "re-encoding changed the value"
        );
    }
}

/// A JSON object as the arguments of one published tool, chosen by the first
/// byte, through the gate `tools/call` applies before dispatch.
pub fn tool_arguments(data: &[u8]) {
    let Some((&pick, rest)) = data.split_first() else {
        return;
    };
    let snapshot = crate::catalog::for_mode(pick & 0x80 == 0);
    if snapshot.tools.is_empty() {
        return;
    }
    let descriptor = &snapshot.tools[pick as usize % snapshot.tools.len()];
    let Ok(J::Object(args)) = serde_json::from_slice::<J>(rest) else {
        return;
    };
    let input = descriptor
        .input_schema
        .as_object()
        .cloned()
        .unwrap_or_default();
    let published = crate::catalog::published_value_arguments(&descriptor.name, &args);
    let checked =
        crate::schema::arguments_for_validation(&input, published.as_ref().unwrap_or(&args));
    let _ = crate::schema::validate(&input, &J::Object(checked));
    let _ = crate::catalog::renamed_field_arguments(&descriptor.name, &args);
}

/// A plan document, decoded and statically validated against the catalog.
pub fn plan_document(data: &[u8]) {
    let Ok(doc) = crate::plan::schema::decode_bytes(data) else {
        return;
    };
    let snapshot = crate::catalog::for_mode(true);
    let capabilities = crate::plan_mcp::capabilities_from_snapshot(snapshot);
    if crate::plan::schema::validate(&doc, &capabilities, &snapshot.revision).is_ok() {
        let _ = crate::plan::schema::document_hash(&doc);
    }
}

/// A host recipe document and its envelope checks.
pub fn host_recipe(data: &[u8]) {
    if let Ok(doc) = crate::recipe::host_recipe::decode_host(data) {
        let _ = crate::recipe::host_recipe::validate_host_envelope(&doc);
    }
}

/// Resource URIs and byte capacities. Oracle: a parsed URI prints back to a
/// string that parses to the same URI.
pub fn resource_identity(data: &[u8]) {
    let s = text(data);
    if let Ok(uri) = crate::resource::Uri::parse(&s) {
        let again = crate::resource::Uri::parse(&uri.to_string()).expect("printed URI parses");
        assert_eq!(again.to_string(), uri.to_string());
    }
    let _ = crate::resource::parse_byte_capacity(&s);
    let _ = crate::incus::parse_capacity_bytes(&s);
}

// --- files the agent reads ---------------------------------------------------------

/// The `--env-file` the service unit names.
pub fn env_file(data: &[u8]) {
    let scratch = Scratch::new();
    let path = scratch.0.join("host-agent.env");
    std::fs::write(&path, data).expect("write env file");
    let mut env =
        crate::config::Env::from_pairs([("HOME", scratch.0.to_string_lossy().into_owned())]);
    if env.load_env_file(&path.to_string_lossy()).is_ok() {
        let _ = crate::config::Config::load(&env).validate();
    }
}

/// A damaged `state.db`. Oracle: opening it returns a value or an error.
pub fn state_store(data: &[u8]) {
    let scratch = Scratch::new();
    std::fs::write(scratch.0.join("state.db"), data).expect("write state.db");
    if let Ok(mut store) = crate::store::StateStore::open(&scratch.0) {
        let _ = store.list_task_snapshots();
        let _ = store.list_operations(10);
        store.close();
    }
}

/// A damaged host `reservations.json`, read by admission.
pub fn reservation_file(data: &[u8]) {
    let scratch = Scratch::new();
    std::fs::write(scratch.0.join("reservations.json"), data).expect("write reservations");
    let coordinator = crate::resource::Coordinator {
        lock_dir: scratch.0.clone(),
        disk_paths: vec![],
        policy_revision: String::new(),
        min_available_memory_bytes: 0,
        min_available_disk_bytes: 0,
        cpu_capacity_cores: 0.0,
        memory_capacity_bytes: 0,
        disk_capacity_bytes: 0,
        task_capacity: 0,
        enforcement_env: crate::config::Env::from_pairs([("PATH", "/nonexistent")]),
        max_normal: 2,
        max_heavy: 1,
        fail_closed: true,
    };
    let _ = coordinator.read_records();
}

/// A credential file under `credentials/`.
pub fn credential_file(data: &[u8]) {
    let scratch = Scratch::new();
    let path = scratch.0.join("client.json");
    std::fs::write(&path, data).expect("write credential");
    let _ = crate::oauth::read_credential(&path);
}

// --- what providers hand back ------------------------------------------------------

/// An external provider's descriptor or install manifest.
pub fn provider_descriptor(data: &[u8]) {
    let Ok(node) = crate::gojson::parse(data) else {
        return;
    };
    if let Ok(d) = crate::provider::definition::decode_descriptor(&node) {
        let _ = crate::provider::definition::validate_descriptor(&d);
    }
    if let Ok(m) = crate::provider::definition::decode_manifest(&node) {
        let _ = crate::provider::definition::encode_manifest(&m);
    }
}

/// A provider's structured result, projected before it is published.
pub fn provider_result(data: &[u8]) {
    if let Ok(J::Object(source)) = serde_json::from_slice::<J>(data) {
        let _ = crate::provider::result::project(&source);
    }
}

/// `incus list --format json` output, and an instance's devices as
/// `incus query` returns them (D35, D36).
pub fn incus_output(data: &[u8]) {
    if let Ok(items) = crate::incus::decode_list(&text(data)) {
        for item in &items {
            let _ = item.config_value("limits.memory");
        }
    }
    if let Ok(doc) = serde_json::from_slice::<J>(data) {
        let devices = |key: &str| doc.get(key).and_then(J::as_object).cloned().unwrap_or_default();
        let (own, expanded) = (devices("devices"), devices("expanded_devices"));
        for device in expanded.keys().chain(own.keys()) {
            let _ = crate::incusops::bridged_nic_needed(&expanded, "vm", device, "br0");
            let _ = crate::incusops::nic_removal_needed(&own, &expanded, "vm", device);
        }
    }
}

/// A guest's console output (D39), cut into reads by the input's own bytes.
/// Oracle: the buffer never exceeds 64 KiB, and every read is whole UTF-8
/// once the console has exited.
pub fn console_output(data: &[u8]) {
    use crate::console::{append, drain, Output, BUFFER};
    let mut output = Output::default();
    let mut rest = data;
    while let Some((&cut, tail)) = rest.split_first() {
        let (chunk, next) = tail.split_at((cut as usize).min(tail.len()));
        append(&mut output.pending, chunk);
        assert!(output.pending.len() <= BUFFER, "console buffer over its bound");
        if cut % 3 == 0 {
            drain(&mut output);
        }
        rest = next;
    }
    output.exited = true;
    let (_, closed) = drain(&mut output);
    assert!(closed && output.pending.is_empty());
}

/// Output of every other host command, registry or API the agent reads,
/// one parser per selector byte. Oracle: each returns without panicking.
pub fn command_output(data: &[u8]) {
    use crate::providers::{cloudflare, k3s};
    let Some((&pick, raw)) = data.split_first() else {
        return;
    };
    let s = text(raw);
    match pick % 36 {
        0 => drop(crate::hostobs::parse_systemd_properties(&s)),
        1 => drop(crate::hostobs::parse_os_release(&s)),
        2 => drop(crate::hostobs::parse_cpuinfo(&s)),
        3 => drop(crate::hostvirt::candidate_from(&s)),
        4 => drop(crate::hostservice::scan_port(&s, 80)),
        5 => drop(crate::hostservice::decode_health(raw)),
        6 => drop(crate::hostwsl::decode_windows_cli(raw)),
        7 => drop(crate::hostwsl::distribution_state(&s, "Ubuntu")),
        8 => drop(crate::hostwsl::vhdx_path(&s, "Ubuntu")),
        9 => drop(crate::hostwsl::parse_int(&s)),
        10 => drop(crate::incus::parse_mounts(&s)),
        11 => drop(crate::incusops::decode_error(&s, "vm-a")),
        12 => drop(crate::incusops::parse_instance_type(&s)),
        13 => drop(crate::incusops::evaluate_gpu_probe(&s)),
        14 => drop(crate::incusops::guest_address(&s)),
        15 => drop(crate::incusops::guest_gateway(&s)),
        16 => drop(crate::ociops::parse_json_map(raw)),
        17 => drop(crate::ociops::parse_storage_entries(raw)),
        18 => drop(crate::ociops::parse_build_cache(raw)),
        19 => drop(crate::ociops::parse_images(raw)),
        20 => drop(crate::postgresops::unmarshal(&s)),
        21 => drop(crate::postgresops::unmarshal_map(&s)),
        22 => drop(crate::postgresops::unmarshal_struct(&s)),
        23 => drop(crate::postgresops::proxy_listen_port(&s)),
        24 => drop(crate::llmops::decode_object(raw)),
        25 => drop(crate::llmops::decode_first(raw)),
        26 => drop(k3s::membership::parse_native_membership(raw)),
        27 => drop(k3s::membership::parse_cluster_node_snapshot(raw)),
        28 => drop(k3s::membership::raw_url_decode(&s)),
        29 => {
            drop(k3s::storage::keep_set_from_pod_json(raw));
            // D34: a Deployment as `kubectl get deployment -o json` prints it.
            if let Ok(deployment) = serde_json::from_slice::<J>(raw) {
                drop(crate::clusterops::project_deployment(&deployment));
            }
        }
        30 => drop(k3s::storage::parse_cri_images(raw)),
        31 => drop(k3s::storage::parse_cri_containers(raw)),
        32 => {
            drop(k3s::storage::parse_df_bytes(&s));
            drop(k3s::storage::parse_du_line(&s));
        }
        33 => {
            drop(cloudflare::api::decode_first(raw));
            drop(cloudflare::mesh::find_mesh_ip(&s));
            drop(cloudflare::mesh::find_mesh_interface(&s, "100.96.0.1"));
        }
        34 => {
            drop(crate::mcpprobe::decode(raw));
            drop(crate::clusterops::privileged(&s));
        }
        _ => drop(crate::recipe::source::parse_raw_github_url(&s, "main")),
    }
}

/// A damaged `authz.sqlite`.
pub fn authz_store(data: &[u8]) {
    let scratch = Scratch::new();
    std::fs::write(scratch.0.join("authz.sqlite"), data).expect("write authz.sqlite");
    if let Ok(mut store) = crate::store::AuthzStore::open(&scratch.0, "fuzz-secret") {
        store.close();
    }
}

// --- archives ----------------------------------------------------------------------

/// A `tar.zst` host archive. The first byte picks whether the rest is the
/// compressed stream itself or a tar stream compressed here. Oracle: nothing
/// is created outside the destination, and no symlink inside it points out.
pub fn host_archive(data: &[u8]) {
    let Some((&mode, rest)) = data.split_first() else {
        return;
    };
    let scratch = Scratch::new();
    let archive = scratch.0.join("in.tar.zst");
    let bytes = if mode & 1 == 0 {
        rest.to_vec()
    } else {
        zstd_raw_frame(rest)
    };
    std::fs::write(&archive, bytes).expect("write archive");
    let dest = scratch.0.join("dest");
    std::fs::create_dir_all(&dest).expect("create destination");
    let _ = crate::hostarchive::extract_for_fuzzing(&archive, &dest);
    for entry in std::fs::read_dir(&scratch.0)
        .expect("list scratch")
        .flatten()
    {
        let name = entry.file_name();
        assert!(
            name == "dest" || name == "in.tar.zst",
            "extraction created {name:?} outside the destination"
        );
    }
    open_up(&dest);
    check_contained(&dest, &dest);
}

/// A zstd frame of raw (stored) blocks, so the fuzzer can mutate the tar
/// stream directly: magic, a frame header with a 2 MiB window, then blocks of
/// at most 128 KiB.
fn zstd_raw_frame(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x28, 0xb5, 0x2f, 0xfd, 0x00, 0x58];
    let mut chunks: Vec<&[u8]> = data.chunks(128 << 10).collect();
    if chunks.is_empty() {
        chunks.push(&[]);
    }
    let last = chunks.len() - 1;
    for (i, chunk) in chunks.iter().enumerate() {
        let header = (i == last) as u32 | ((chunk.len() as u32) << 3);
        out.extend_from_slice(&header.to_le_bytes()[..3]);
        out.extend_from_slice(chunk);
    }
    out
}

fn check_contained(dest: &Path, dir: &Path) {
    for entry in std::fs::read_dir(dir).expect("list destination").flatten() {
        let path = entry.path();
        let kind = entry.file_type().expect("file type");
        if kind.is_symlink() {
            let link = std::fs::read_link(&path).expect("read link");
            let resolved = lexical(&path.parent().unwrap_or(dest).join(link));
            assert!(
                resolved.starts_with(dest),
                "symlink {path:?} points outside the destination: {resolved:?}"
            );
        } else if kind.is_dir() {
            check_contained(dest, &path);
        }
    }
}

fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// The tar reader on its own, without decompression.
pub fn tar_stream(data: &[u8]) {
    let mut reader = crate::gotar::Reader::new(data);
    for _ in 0..10_000 {
        match reader.next() {
            Ok(Some(_)) => {
                let mut sink = Vec::new();
                if (&mut reader).take(1 << 20).read_to_end(&mut sink).is_err() {
                    return;
                }
            }
            _ => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every target over its committed seeds and regressions: a crash a
    /// fuzzer found is replayed here on the stable toolchain.
    #[test]
    fn replay() {
        let fuzz = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fuzz");
        let mut ran = 0;
        for (name, target) in TARGETS {
            for dir in ["seeds", "regressions"] {
                let Ok(entries) = std::fs::read_dir(fuzz.join(dir).join(name)) else {
                    continue;
                };
                for entry in entries.flatten() {
                    let data = std::fs::read(entry.path()).unwrap();
                    target(&data);
                    ran += 1;
                }
            }
        }
        assert!(ran >= TARGETS.len(), "every target has at least one seed");
    }

    /// The wire target reaches the composed agent: `/health` answers 200 and
    /// an unauthenticated `/mcp` call is refused with 401.
    #[test]
    fn wire_answers() {
        let ask = |raw: &[u8]| {
            let mut s = std::net::TcpStream::connect(wire().addr).unwrap();
            s.write_all(raw).unwrap();
            s.shutdown(std::net::Shutdown::Write).unwrap();
            let mut reply = String::new();
            s.read_to_string(&mut reply).unwrap();
            reply
        };
        assert!(ask(b"GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").starts_with("HTTP/1.1 200"));
        let call = b"POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 2\r\n\r\n{}";
        assert!(ask(call).starts_with("HTTP/1.1 401"));
    }

    /// D29: an authenticated body declared larger than 4 MiB is refused with
    /// 413 once 4 MiB has been read, without waiting for (or holding) the
    /// rest; a body at the limit is read in full.
    #[test]
    fn oversized_mcp_bodies_are_refused_without_reading_them() {
        static TOKEN_WIRE: OnceLock<Wire> = OnceLock::new();
        let wire = TOKEN_WIRE.get_or_init(|| {
            compose_wire("wire-token", &[("MCP_AUTH_TOKEN", "test-bootstrap-token")])
        });
        let send = |declared: usize, sent: usize| {
            let mut s = std::net::TcpStream::connect(wire.addr).unwrap();
            s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let head = format!(
                "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: Bearer test-bootstrap-token\r\n\
                 Content-Type: application/json\r\nAccept: application/json, text/event-stream\r\n\
                 Content-Length: {declared}\r\n\r\n"
            );
            s.write_all(head.as_bytes()).unwrap();
            let _ = s.write_all(&vec![b' '; sent]);
            // The write side stays open: the reply must not wait for the rest.
            let mut reply = vec![0u8; 4096];
            let n = s.read(&mut reply).unwrap_or(0);
            String::from_utf8_lossy(&reply[..n]).into_owned()
        };
        let over = send(64 << 20, 5 << 20);
        assert!(
            over.starts_with("HTTP/1.1 413"),
            "no 413 before the body ended: {over:?}"
        );
        assert!(
            over.contains("request body exceeds 4194304 bytes"),
            "{over}"
        );
        let at_limit = send(4 << 20, 4 << 20);
        assert!(at_limit.starts_with("HTTP/1.1 "), "{at_limit:?}");
        assert!(!at_limit.starts_with("HTTP/1.1 413"), "{at_limit}");
    }

    /// F4 (m16, gate e2e-6 draft): `/health` runs `incus list`; with an
    /// `incus` that never exits, it must not stall other connections.
    #[test]
    fn a_hanging_incus_does_not_stall_other_requests() {
        use std::os::unix::fs::PermissionsExt;
        let stub = root().join("incus-hang");
        std::fs::write(&stub, "#!/bin/sh\nexec /bin/sleep 60\n").unwrap();
        std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
        static HANG_WIRE: OnceLock<Wire> = OnceLock::new();
        let wire = HANG_WIRE.get_or_init(|| {
            compose_wire(
                "wire-hang",
                &[("OPUTE_INCUS_BINARY_PATH", stub.to_str().unwrap())],
            )
        });
        let addr = wire.addr;
        // A health check that waits on the hanging command.
        std::thread::spawn(move || {
            if let Ok(mut s) = std::net::TcpStream::connect(addr) {
                let _ = s.write_all(b"GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n");
                let _ = s.read(&mut [0u8; 64]);
            }
        });
        std::thread::sleep(Duration::from_millis(300));
        let mut s = std::net::TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        s.write_all(b"POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 2\r\n\r\n{}")
            .unwrap();
        let mut reply = [0u8; 64];
        let n = s.read(&mut reply).unwrap_or(0);
        let reply = String::from_utf8_lossy(&reply[..n]);
        assert!(
            reply.starts_with("HTTP/1.1 401"),
            "no answer while /health waited: {reply:?}"
        );
    }

    /// evidence/hardening/entry-points.json names every target, and every
    /// entry it lists is covered by targets and properties that exist.
    #[test]
    fn inventory() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let raw = std::fs::read(root.join("../../evidence/hardening/entry-points.json")).unwrap();
        let doc: J = serde_json::from_slice(&raw).unwrap();
        let properties = std::fs::read_to_string(root.join("src/properties.rs")).unwrap();
        let mut listed = std::collections::BTreeSet::new();
        for entry in doc["entries"].as_array().unwrap() {
            let id = entry["id"].as_str().unwrap();
            let fuzz: Vec<&str> = entry["fuzz"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(J::as_str)
                .collect();
            let props: Vec<&str> = entry["properties"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(J::as_str)
                .collect();
            assert!(
                !fuzz.is_empty() || !props.is_empty(),
                "{id} has no fuzz target or property test"
            );
            for t in &fuzz {
                assert!(
                    TARGETS.iter().any(|(n, _)| n == t),
                    "{id}: no fuzz target {t}"
                );
                assert!(
                    root.join(format!("../../fuzz/fuzz_targets/{t}.rs"))
                        .is_file(),
                    "{id}: no fuzz binary {t}"
                );
                listed.insert(t.to_string());
            }
            for p in &props {
                assert!(
                    properties.contains(&format!("fn {p}(")),
                    "{id}: no property {p}"
                );
            }
        }
        for (name, _) in TARGETS {
            assert!(
                listed.contains(*name),
                "target {name} is not in the inventory"
            );
        }
    }
}
