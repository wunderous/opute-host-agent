//! M8f: Go's `internal/domain/serving`: a product-neutral serving
//! assignment reconciled into an assignment-scoped user-systemd unit, and
//! ingress discovery for caller-declared endpoints.
//!
//! What the domain needs from others it reaches through its Deps, stated in
//! primitives as in Go: the host domain's agent shell and service state,
//! the incus domain's bridge address, and the kubernetes domain's ingress
//! load balancer (kubectl through the active provider). It runs
//! `systemctl --user is-active` and `kill -0` itself (`exec.Command`).

use crate::config::Env;
use serde_json::{Map, Value as J};
use std::collections::HashSet;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

/// The incus inventory's resource registration.
pub type Registrar = Arc<dyn Fn(&str, Map<String, J>) + Send + Sync>;

/// The serving domain's seams.
#[derive(Clone)]
pub struct Serving {
    pub env: Env,
    pub instance_id: String,
    pub owner_instance: String,
    pub incus: crate::incus::Incus,
    pub register: Registrar,
    pub k8s: crate::clusterops::Kubernetes,
}

/// `ServingAssignmentArgs`.
#[derive(Clone, Debug, Default)]
pub struct AssignmentArgs {
    pub contract_version: String,
    pub assignment_id: String,
    pub generation: i64,
    pub idempotency_key: String,
    pub service: String,
    pub mode: String,
    pub runtime: String,
    /// `mapField`: nil unless an object was sent.
    pub target: Option<Map<String, J>>,
    pub artifact: Option<Map<String, J>>,
    pub endpoints: Vec<J>,
    pub readiness: Vec<J>,
    pub service_unit: String,
    pub desired_state: String,
    pub restart_policy: String,
}

impl AssignmentArgs {
    fn artifact(&self, key: &str) -> Option<&J> {
        self.artifact.as_ref().and_then(|a| a.get(key))
    }
}

/// `servingLaunches`: process-wide launch markers per assignment identity.
fn launches() -> &'static Mutex<HashSet<String>> {
    static LAUNCHES: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    LAUNCHES.get_or_init(Default::default)
}

/// `servingLaunchKey`.
fn launch_key(args: &AssignmentArgs) -> String {
    format!(
        "{}:{}:{}",
        args.assignment_id, args.generation, args.idempotency_key
    )
}

/// `servingString`.
fn serving_string(value: Option<&J>) -> String {
    value
        .and_then(J::as_str)
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

/// `servingPort`: an MCP number in range and integral.
fn serving_port(value: Option<&J>) -> i64 {
    match value {
        Some(J::Number(n)) => match n.as_f64() {
            Some(f) if (1.0..=65535.0).contains(&f) && f == f.trunc() => f as i64,
            _ => 0,
        },
        _ => 0,
    }
}

/// `servingIdentifier`: `^[A-Za-z0-9][A-Za-z0-9_.:@/-]{0,127}$`.
fn serving_identifier(value: &str) -> bool {
    let b = value.as_bytes();
    !b.is_empty()
        && b.len() <= 128
        && b[0].is_ascii_alphanumeric()
        && b[1..]
            .iter()
            .all(|c| c.is_ascii_alphanumeric() || b"_.:@/-".contains(c))
}

/// `servingSystemdUnit`: `^[A-Za-z0-9_.@:-]+$`.
fn serving_systemd_unit(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_.@:-".contains(&c))
}

/// `validateServingTarget`.
fn validate_target(target: Option<&Map<String, J>>) -> Result<(), String> {
    let Some(target) = target else {
        return Err("target is required".into());
    };
    if serving_string(target.get("hostId")).is_empty()
        || serving_string(target.get("resourceId")).is_empty()
    {
        return Err("target.hostId and target.resourceId are required".into());
    }
    let kind = serving_string(target.get("kind"));
    let instance_type = serving_string(target.get("instanceType"));
    match kind.as_str() {
        "host" => {
            if instance_type != "host" {
                return Err("host target requires instanceType=host".into());
            }
        }
        "system-container" => {
            if instance_type != "container" && instance_type != "system-container" {
                return Err("system-container target requires a container instanceType".into());
            }
        }
        "kubernetes-cluster" => {
            if instance_type != "kubernetes" {
                return Err("kubernetes-cluster target requires instanceType=kubernetes".into());
            }
        }
        _ => {
            return Err(format!(
                "unsupported or ambiguous target kind {}",
                crate::goerr::quote(&kind)
            ))
        }
    }
    if instance_type == "vm" || kind.to_lowercase().contains("vm") {
        return Err("VM serving targets are not supported".into());
    }
    Ok(())
}

/// `validateServingAssignment`.
fn validate_assignment(args: &AssignmentArgs) -> Result<(), String> {
    let quote = crate::goerr::quote;
    if args.contract_version != "serving-assignment.v1" {
        return Err(format!(
            "unsupported serving assignment contract {}",
            quote(&args.contract_version)
        ));
    }
    for (name, value) in [
        ("assignmentId", &args.assignment_id),
        ("idempotencyKey", &args.idempotency_key),
        ("service", &args.service),
    ] {
        if value.is_empty() || !serving_identifier(value) {
            return Err(format!(
                "{name} is required and must be a bounded identifier"
            ));
        }
    }
    if args.generation < 1 {
        return Err("generation must be positive".into());
    }
    if args.mode != "dev-process" && args.mode != "oci-release" {
        return Err(format!("unsupported serving mode {}", quote(&args.mode)));
    }
    if !matches!(args.runtime.as_str(), "process" | "podman" | "kubernetes") {
        return Err(format!(
            "unsupported serving runtime {}",
            quote(&args.runtime)
        ));
    }
    validate_target(args.target.as_ref())?;
    if args.mode == "dev-process" && args.runtime != "process" {
        return Err("dev-process requires process runtime".into());
    }
    if args.mode == "oci-release"
        && (serving_string(args.artifact("kind")) != "oci"
            || serving_string(args.artifact("digest")).is_empty())
    {
        return Err("oci-release requires an OCI artifact digest".into());
    }
    if args.runtime == "process" {
        if serving_string(args.artifact("kind")) != "source" {
            return Err("process runtime requires a source artifact".into());
        }
        if args
            .artifact("command")
            .and_then(J::as_array)
            .is_none_or(|c| c.is_empty())
        {
            return Err("source artifact command is required".into());
        }
    }
    if args.endpoints.is_empty() || args.readiness.is_empty() {
        return Err("at least one endpoint and readiness check are required".into());
    }
    if args.runtime == "process" {
        let policy = args.restart_policy.as_str();
        if !matches!(policy, "" | "no" | "on-failure" | "always") {
            return Err(format!("unsupported restartPolicy {}", quote(policy)));
        }
        if !args.service_unit.is_empty() {
            if !serving_systemd_unit(&args.service_unit) {
                return Err("serviceUnit must be a valid systemd unit name".into());
            }
            let state = if args.desired_state.is_empty() {
                "restart"
            } else {
                args.desired_state.as_str()
            };
            if state != "start" && state != "restart" {
                return Err(
                    "desiredState must be start or restart when serviceUnit is provided".into(),
                );
            }
        }
    }
    Ok(())
}

/// `shellQuoteServing`.
fn shell_quote_serving(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

/// `servingFileToken`.
fn file_token(value: &str) -> String {
    value.replace(['/', ':', '@', '.'], "-")
}

fn pid_file(assignment: &str) -> String {
    format!("/tmp/serving-assignment-{}.pid", file_token(assignment))
}

fn state_file(assignment: &str) -> String {
    format!(
        "/tmp/serving-assignment-{}.state.json",
        file_token(assignment)
    )
}

fn transient_unit(assignment: &str) -> String {
    format!("host-serving-{}", file_token(assignment))
}

/// `servingAssignmentStateMatches`.
fn state_matches(args: &AssignmentArgs) -> bool {
    let Ok(data) = std::fs::read(state_file(&args.assignment_id)) else {
        return false;
    };
    let Ok(J::Object(state)) = serde_json::from_slice::<J>(&data) else {
        return false;
    };
    let text = |k: &str| state.get(k).and_then(J::as_str).unwrap_or("").to_string();
    text("contractVersion") == args.contract_version
        && text("assignmentId") == args.assignment_id
        && state.get("generation").and_then(J::as_i64) == Some(args.generation)
        && text("idempotencyKey") == args.idempotency_key
}

/// `recordServingAssignmentState`: `json.Marshal` of the state struct.
fn record_state(args: &AssignmentArgs) -> Result<(), String> {
    let s = |v: &str| {
        let mut o = String::new();
        crate::gojson::encode_string(v, &mut o);
        o
    };
    let data = format!(
        "{{\"contractVersion\":{},\"assignmentId\":{},\"generation\":{},\"idempotencyKey\":{}}}",
        s(&args.contract_version),
        s(&args.assignment_id),
        args.generation,
        s(&args.idempotency_key)
    );
    let path = state_file(&args.assignment_id);
    write_file(&path, data.as_bytes())
}

/// `os.WriteFile(path, data, 0600)`.
fn write_file(path: &str, data: &[u8]) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| crate::goerr::path_error("open", std::path::Path::new(path), &e).0)?;
    file.write_all(data)
        .map_err(|e| crate::goerr::path_error("write", std::path::Path::new(path), &e).0)
}

/// `servingLaunchCommand`.
fn launch_command(pid_file: &str, assignment: &str, command: &str, restart_policy: &str) -> String {
    let unit = transient_unit(assignment);
    let log = file_token(assignment);
    let resource = " --property=Slice=opute-workload.slice --property=MemoryHigh=10G --property=MemoryMax=11G --property=MemorySwapMax=1G --property=CPUQuota=600% --property=CPUWeight=100 --property=TasksMax=4096 --property=StartLimitIntervalSec=60s --property=StartLimitBurst=5";
    let restart = if restart_policy == "on-failure" || restart_policy == "always" {
        format!(" --property=Restart={restart_policy} --property=RestartSec=2s")
    } else {
        String::new()
    };
    format!(
        "pidfile={}; unit={}; systemctl --user stop \"$unit\" >/dev/null 2>&1 || true; systemctl --user reset-failed \"$unit\" >/dev/null 2>&1 || true; rm -f \"$pidfile\"; systemd-run --user --unit=\"$unit\" --collect --no-block --property=KillMode=control-group{resource}{restart} /bin/bash -lic {} >/tmp/serving-assignment-{log}-supervisor.log 2>&1; for _ in $(seq 1 50); do pid=$(systemctl --user show \"$unit\" -p MainPID --value 2>/dev/null || true); if [[ \"$pid\" =~ ^[1-9][0-9]*$ ]] && kill -0 \"$pid\" 2>/dev/null; then printf '%s\\n' \"$pid\" >\"$pidfile\"; exit 0; fi; sleep 0.1; done; echo \"serving supervisor did not expose a live main PID for $unit\" >&2; systemctl --user status \"$unit\" --no-pager 2>&1 || true; exit 1",
        shell_quote_serving(pid_file),
        shell_quote_serving(&unit),
        shell_quote_serving(command),
    )
}

/// `servingArtifactCommand`: the caller's environment exported, then the
/// command run from its working directory. Go ranges over the environment
/// map, so its order is unspecified; this one exports in key order.
fn artifact_command(artifact: Option<&Map<String, J>>) -> Result<String, String> {
    let get = |k: &str| artifact.and_then(|a| a.get(k));
    let source_dir = serving_string(get("sourceDir"));
    if source_dir.is_empty() {
        return Err("source artifact sourceDir is required".into());
    }
    let mut working_dir = serving_string(get("workingDir"));
    if working_dir.is_empty() {
        working_dir = source_dir;
    }
    let Some(raw) = get("command")
        .and_then(J::as_array)
        .filter(|c| !c.is_empty())
    else {
        return Err("source artifact command is required".into());
    };
    let mut command = Vec::new();
    for value in raw {
        match value.as_str() {
            Some(v) if !v.trim().is_empty() => command.push(shell_quote_serving(v)),
            _ => return Err("source artifact command entries must be non-empty strings".into()),
        }
    }
    let mut environment = String::new();
    if let Some(J::Object(raw)) = get("environment") {
        for (key, value) in raw {
            let valid = key
                .bytes()
                .next()
                .is_some_and(|c| c.is_ascii_alphabetic() || c == b'_')
                && key.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_');
            if !valid {
                return Err(format!(
                    "source artifact environment key {} is invalid",
                    crate::goerr::quote(key)
                ));
            }
            let Some(value) = value.as_str() else {
                return Err(format!(
                    "source artifact environment value {} must be a string",
                    crate::goerr::quote(key)
                ));
            };
            environment.push_str(&format!(" export {key}={};", shell_quote_serving(value)));
        }
    }
    Ok(format!(
        "{environment}cd {} && exec {}",
        shell_quote_serving(&working_dir),
        command.join(" ")
    ))
}

impl Serving {
    /// `exec.Command(name, args...).Run() == nil`.
    fn run_ok(&self, argv: &[&str]) -> bool {
        let Some(program) = crate::hostobs::look_path(&self.env, argv[0]) else {
            return false;
        };
        std::process::Command::new(program)
            .args(&argv[1..])
            .env_clear()
            .envs(self.env.pairs())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }

    /// `servingSystemdUnitActive`.
    fn unit_active(&self, unit: &str) -> bool {
        self.run_ok(&["systemctl", "--user", "is-active", "--quiet", unit])
    }

    /// `servingProcessAlive`: the pid file names a live process.
    fn process_alive(&self, pid_file: &str) -> bool {
        let Ok(data) = std::fs::read_to_string(pid_file) else {
            return false;
        };
        match crate::goerr::atoi(data.trim()) {
            (pid, false) if pid >= 1 => self.run_ok(&["kill", "-0", &pid.to_string()]),
            _ => false,
        }
    }

    /// `claimServingLaunch`: one launch per assignment identity, reclaimed
    /// once the process it started is gone.
    fn claim_launch(&self, args: &AssignmentArgs) -> bool {
        let key = launch_key(args);
        let mut started = launches().lock().unwrap_or_else(|p| p.into_inner());
        if started.contains(&key) {
            if self.process_alive(&pid_file(&args.assignment_id)) {
                return false;
            }
            started.remove(&key);
        }
        started.insert(key);
        true
    }

    fn release_launch(args: &AssignmentArgs) {
        launches()
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&launch_key(args));
    }

    /// The host domain's `RunAgentShell`: only an empty command fails.
    fn run_agent_shell(&self, command: &str) -> Result<(), String> {
        let command = command.trim();
        if command.is_empty() {
            return Err("command is required".into());
        }
        let _ = crate::hostobs::run_command(
            &self.env,
            &["bash".into(), "-lc".into(), command.into()],
            Duration::ZERO,
        );
        Ok(())
    }

    /// `ReconcileServingAssignment`.
    pub fn reconcile_assignment(&self, args: &AssignmentArgs) -> Result<J, String> {
        validate_assignment(args)?;
        let mut result = Map::new();
        result.insert(
            "contractVersion".into(),
            J::from(args.contract_version.as_str()),
        );
        result.insert("assignmentId".into(), J::from(args.assignment_id.as_str()));
        result.insert("generation".into(), J::from(args.generation));
        result.insert(
            "idempotencyKey".into(),
            J::from(args.idempotency_key.as_str()),
        );
        result.insert("service".into(), J::from(args.service.as_str()));
        result.insert("runtime".into(), J::from(args.runtime.as_str()));
        result.insert("status".into(), J::from("accepted"));
        if args.runtime == "process" {
            let restart_policy = if args.restart_policy.is_empty() {
                "no".to_string()
            } else {
                args.restart_policy.clone()
            };
            result.insert("restartPolicy".into(), J::from(restart_policy.as_str()));
            let command = artifact_command(args.artifact.as_ref())?;
            let mut all_ready = true;
            for raw in &args.readiness {
                let Some(check) = raw.as_object() else {
                    return Err("readiness entries must be objects".into());
                };
                let endpoint_name = serving_string(check.get("endpointName"));
                let path = serving_string(check.get("path"));
                for endpoint in args.endpoints.iter().filter_map(J::as_object) {
                    if serving_string(endpoint.get("name")) == endpoint_name {
                        let port = serving_port(endpoint.get("port"));
                        let protocol = serving_string(endpoint.get("protocol"));
                        let (ready, _) = probe_endpoint(&protocol, port, &path);
                        all_ready = all_ready && ready;
                    }
                }
            }
            let pid = pid_file(&args.assignment_id);
            let pre_start = serving_string(args.artifact("preStartCommand"));
            let managed_active = self.unit_active(&transient_unit(&args.assignment_id));
            let adopt = all_ready && !managed_active;
            if adopt && !self.process_alive(&pid) && pre_start.is_empty() {
                return Err("declared serving endpoint is ready but is not owned by its host-systemd supervisor; preStartCommand is required for explicit adoption".into());
            }
            let changed = !state_matches(args);
            if !all_ready || adopt || changed {
                if managed_active && !changed && !adopt {
                    result.insert("starting".into(), J::Bool(true));
                } else {
                    let alive = self.process_alive(&pid);
                    if alive && !adopt && (!changed || !managed_active) {
                        result.insert("starting".into(), J::Bool(true));
                    } else if self.claim_launch(args) {
                        if !alive {
                            let _ = std::fs::remove_file(&pid);
                        }
                        if adopt && !pre_start.is_empty() {
                            if let Err(err) = self.run_agent_shell(&pre_start) {
                                Self::release_launch(args);
                                return Err(format!(
                                    "source artifact pre-start command failed: {err}"
                                ));
                            }
                        }
                        let launch =
                            launch_command(&pid, &args.assignment_id, &command, &restart_policy);
                        if let Err(err) = self.run_agent_shell(&launch) {
                            Self::release_launch(args);
                            return Err(err);
                        }
                        if let Err(err) = record_state(args) {
                            Self::release_launch(args);
                            return Err(format!("record serving assignment state: {err}"));
                        }
                        result.insert("started".into(), J::Bool(true));
                    } else {
                        result.insert("starting".into(), J::Bool(true));
                    }
                }
            }
        }
        if args.runtime == "process" && !args.service_unit.is_empty() {
            let owner = crate::hostfiles::Owner {
                instance_id: &self.instance_id,
                owner_instance: &self.owner_instance,
            };
            let service = crate::hostservice::set_host_service_state(
                &self.env,
                &owner,
                &args.service_unit,
                &args.desired_state,
                "user",
            )?;
            result.insert("service".into(), service);
        }
        let mut readiness = Vec::new();
        for raw in &args.endpoints {
            let Some(endpoint) = raw.as_object() else {
                return Err("endpoint entries must be objects".into());
            };
            let name = serving_string(endpoint.get("name"));
            let protocol = serving_string(endpoint.get("protocol"));
            let port = serving_port(endpoint.get("port"));
            let mut path = "/".to_string();
            for check in args.readiness.iter().filter_map(J::as_object) {
                if serving_string(check.get("endpointName")) == name
                    && !serving_string(check.get("path")).is_empty()
                {
                    path = serving_string(check.get("path"));
                }
            }
            let (ready, message) = probe_endpoint(&protocol, port, &path);
            let mut row = Map::new();
            row.insert("name".into(), J::from(name));
            row.insert("ready".into(), J::Bool(ready));
            row.insert("message".into(), J::from(message));
            readiness.push(J::Object(row));
        }
        result.insert("readiness".into(), J::Array(readiness));
        result.insert("readinessSource".into(), J::from("host-agent-local-probe"));
        Ok(J::Object(result))
    }

    /// `DiscoverServiceIngress`: the caller's endpoint bindings with the
    /// observed ingress address; no hostname is inferred.
    pub fn discover_ingress(
        &self,
        vm_name: &str,
        endpoints: &[(String, String)],
        ingress_namespace: &str,
        ingress_service: &str,
    ) -> Result<J, String> {
        if vm_name.trim().is_empty() || endpoints.is_empty() {
            return Err("vmName and endpoints are required".into());
        }
        if endpoints
            .iter()
            .any(|(n, h)| n.trim().is_empty() || h.trim().is_empty())
        {
            return Err("each ingress endpoint requires name and hostname".into());
        }
        let vm = vm_name.trim();
        let or_default = |v: &str, d: &str| {
            if v.trim().is_empty() {
                d.to_string()
            } else {
                v.trim().to_string()
            }
        };
        let namespace = or_default(ingress_namespace, "kube-system");
        let service = or_default(ingress_service, "traefik");
        let register = self.register.clone();
        let info = self.incus.get_vm_info(vm, true, &move |uri, coordinates| {
            register(uri, coordinates)
        })?;
        let ips: Vec<String> = info
            .get("ipv4")
            .and_then(J::as_array)
            .map(|a| a.iter().filter_map(J::as_str).map(String::from).collect())
            .unwrap_or_default();
        let bridge = crate::clusterops::first_bridge_ipv4(&ips);
        if bridge.is_empty() {
            return Err(format!("no bridge IPv4 found for target {vm}"));
        }
        let (lb_ip, lb_host) = self.ingress_load_balancer(vm, &namespace, &service);
        let ingress_ip = if lb_ip.is_empty() {
            bridge.clone()
        } else {
            lb_ip.clone()
        };
        let mut out = Map::new();
        out.insert("vmName".into(), J::from(vm));
        out.insert("bridgeIp".into(), J::from(bridge));
        out.insert("ingressNamespace".into(), J::from(namespace));
        out.insert("ingressService".into(), J::from(service));
        out.insert("loadBalancerIp".into(), J::from(lb_ip));
        out.insert("loadBalancerHostname".into(), J::from(lb_host));
        out.insert("ingressIp".into(), J::from(ingress_ip));
        out.insert(
            "endpoints".into(),
            J::Array(
                endpoints
                    .iter()
                    .map(|(name, hostname)| {
                        let mut row = Map::new();
                        row.insert("name".into(), J::from(name.as_str()));
                        row.insert("hostname".into(), J::from(hostname.as_str()));
                        J::Object(row)
                    })
                    .collect(),
            ),
        );
        out.insert("note".into(), J::from("Use the caller-declared endpoint bindings with the observed ingress address; no product hostname was inferred."));
        Ok(J::Object(out))
    }

    /// The ingress service's published load balancer, if any
    /// (`loadBalancerFromService`).
    fn ingress_load_balancer(&self, vm: &str, namespace: &str, service: &str) -> (String, String) {
        let args: Vec<String> = ["get", "svc", service, "-n", namespace, "-o", "json"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let Ok(stdout) = self.k8s.run_kubectl(vm, &args, "discover ingress service") else {
            return (String::new(), String::new());
        };
        if stdout.is_empty() {
            return (String::new(), String::new());
        }
        let Ok(J::Object(decoded)) = serde_json::from_str::<J>(&stdout) else {
            return (String::new(), String::new());
        };
        let first = decoded
            .get("status")
            .and_then(|s| s.get("loadBalancer"))
            .and_then(|l| l.get("ingress"))
            .and_then(J::as_array)
            .and_then(|i| i.first());
        let field = |k: &str| {
            first
                .and_then(|f| f.get(k))
                .and_then(J::as_str)
                .unwrap_or("")
                .to_string()
        };
        (field("ip"), field("hostname"))
    }
}

/// `probeServingEndpoint`.
fn probe_endpoint(protocol: &str, port: i64, path: &str) -> (bool, String) {
    if !(1..=65535).contains(&port) {
        return (false, "invalid port".into());
    }
    if protocol == "tcp" {
        let address = std::net::SocketAddr::from(([127, 0, 0, 1], port as u16));
        return match std::net::TcpStream::connect_timeout(&address, Duration::from_secs(2)) {
            Ok(_) => (true, "tcp connection accepted".into()),
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {
                (false, format!("dial tcp 127.0.0.1:{port}: i/o timeout"))
            }
            Err(e) => (
                false,
                format!(
                    "dial tcp 127.0.0.1:{port}: connect: {}",
                    crate::goerr::errno_text(&e)
                ),
            ),
        };
    }
    if protocol != "http" && protocol != "https" {
        return (true, "readiness delegated to runtime".into());
    }
    let path = if path.is_empty() { "/" } else { path };
    let url = format!("{protocol}://127.0.0.1:{port}{path}");
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(3))
        .redirects(10)
        .user_agent("Go-http-client/1.1")
        .tls_config(crate::hostartifact::tls_config())
        .build();
    match agent.get(&url).set("Accept-Encoding", "gzip").call() {
        Ok(response) | Err(ureq::Error::Status(_, response)) => {
            let status = response.status();
            ((200..500).contains(&status), format!("HTTP {status}"))
        }
        Err(ureq::Error::Transport(t)) => (
            false,
            format!(
                "Get \"{url}\": {}",
                crate::hostartifact::transport_cause(&url, &t)
            ),
        ),
    }
}
