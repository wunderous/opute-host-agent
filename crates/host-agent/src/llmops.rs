//! M8f: Go's `internal/domain/llm`, local model serving: the shared Ollama
//! runtime (the default), the llama.cpp server (build, manifest, systemd
//! unit), the authenticated local-LLM relay, the cluster proxy that
//! publishes a relay into k3s, and the runtime-neutral OpenAI-compatible
//! probe.
//!
//! Host commands run as Go runs them: through the shared host command
//! runner (`HostCommandRunner*`, `hostobs::run_command_*`), the host
//! workload runner (the llama-server build), or `exec.Command` directly
//! (`systemctlUser`, `nvidia-smi` in the llama probe). The cluster proxy
//! reaches the kubernetes domain (`Deps`).
//!
//! As in Go, the domain is built once, on its first call: that is when the
//! persisted relays are restored.

use crate::config::Env;
use crate::goerr::{quote, shell_quote};
use crate::hostobs::CommandResult;
use crate::plan::runner::RunCtx;
use serde_json::{Map, Value as J};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::net::IpAddr;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

const DEFAULT_OLLAMA_PORT: i64 = 11434;
pub const DEFAULT_OLLAMA_MODEL: &str = "hf.co/LiquidAI/LFM2-2.6B-GGUF:Q4_K_M";
const OLLAMA_CONTEXT_MINIMUM: i64 = 512;
const OLLAMA_CONTEXT_MAXIMUM: i64 = 32768;
const OLLAMA_SERVICE_NAME: &str = "opute-ollama.service";
const OLLAMA_NUM_PARALLEL: i64 = 1;
const OLLAMA_MAX_LOADED_MODELS: i64 = 2;

const DEFAULT_LLAMA_SERVER_PORT: i64 = 8080;
const DEFAULT_LLAMA_CONTEXT: i64 = 8192;
const DEFAULT_LLAMA_GPU_LAYERS: i64 = 999;
const DEFAULT_LLAMA_TEMPLATE: &str = "";
const DEFAULT_LLAMA_TEMPLATE_KWARGS: &str = r#"{"enable_thinking":false}"#;
const LLAMA_UNIT: &str = "opute-llama-server.service";

/// The llm domain's seams. It is cheap to clone; the stateful part (`Live`)
/// is built on first use and shared.
#[derive(Clone)]
pub struct Llm {
    pub env: Env,
    pub k8s: crate::clusterops::Kubernetes,
    /// `RelayConfigDir`: where each relay session is persisted.
    pub relay_config_dir: String,
    /// `SharedHostResourceLockDir`: the host-wide inference request lock.
    pub lock_dir: String,
    pub live: Arc<OnceLock<Arc<Live>>>,
}

/// What Go's `llm.New` builds: the relay manager holds live listeners, so it
/// exists once per host service.
pub struct Live {
    pub relay: Arc<RelayManager>,
}

impl Llm {
    /// `Service.Llm()`: the domain, constructed (and its persisted relays
    /// restored) on first use.
    pub fn live(&self) -> &Arc<Live> {
        self.live.get_or_init(|| {
            Arc::new(Live {
                relay: RelayManager::persistent_at(
                    &self.relay_config_dir,
                    &self.lock_dir,
                    &self.env.get("HOME"),
                ),
            })
        })
    }

    /// `os.UserHomeDir`.
    fn home(&self) -> Result<String, String> {
        let home = self.env.get("HOME");
        if home.is_empty() {
            return Err("$HOME is not defined".into());
        }
        Ok(home)
    }

    /// `Shared.HostCommandRunner`: a start failure or timeout is a result,
    /// never an error.
    fn host(&self, argv: &[&str], timeout: Duration) -> CommandResult {
        crate::hostobs::run_command(&self.env, &strings(argv), timeout)
    }

    /// `Shared.HostCommandRunnerContext`: a done context refuses first.
    fn host_ctx(
        &self,
        ctx: &RunCtx,
        argv: &[String],
        timeout: Duration,
    ) -> Result<CommandResult, String> {
        if let Some(err) = ctx.err() {
            return Err(err);
        }
        let deadline = (!timeout.is_zero()).then(|| Instant::now() + timeout);
        Ok(crate::hostobs::run_command_with(
            &self.env,
            argv,
            None,
            deadline,
            timeout,
            Some(ctx),
        ))
    }

    /// `systemctlUser(ctx, args...).CombinedOutput()`: the user manager's
    /// bus is set over the agent's environment.
    fn systemctl_user(&self, args: &[&str]) -> (String, Result<(), String>) {
        let mut env = self.env.clone();
        let uid = nix::unistd::getuid().to_string();
        env.set("XDG_RUNTIME_DIR", &format!("/run/user/{uid}"));
        env.set(
            "DBUS_SESSION_BUS_ADDRESS",
            &format!("unix:path=/run/user/{uid}/bus"),
        );
        let mut argv = vec!["systemctl".to_string(), "--user".to_string()];
        argv.extend(args.iter().map(|a| a.to_string()));
        let (output, result) = crate::hostexec::combined_output(&env, &argv, None);
        (String::from_utf8_lossy(&output).into_owned(), result)
    }
}

fn strings(argv: &[&str]) -> Vec<String> {
    argv.iter().map(|s| s.to_string()).collect()
}

/// `textutil.FirstNonEmpty` over trimmed values.
fn first_non_empty<'a>(values: &[&'a str]) -> &'a str {
    values
        .iter()
        .find(|v| !v.trim().is_empty())
        .copied()
        .unwrap_or("")
}

/// `strconv.Atoi` success.
fn atoi(raw: &str) -> Option<i64> {
    match crate::goerr::atoi(raw) {
        (value, false) => Some(value),
        _ => None,
    }
}

// --- Go JSON decoding into a struct ------------------------------------------------------

/// A JSON object member as `encoding/json` matches it to a struct field: the
/// exact key, else a case-insensitive one.
fn member<'a>(object: &'a Map<String, J>, name: &str) -> Option<&'a J> {
    object.get(name).or_else(|| {
        object
            .iter()
            .find(|(k, _)| crate::gojson::equal_fold(k, name))
            .map(|(_, v)| v)
    })
}

/// A struct's string field: a string, or `null` for the zero value; any
/// other kind fails the decode.
fn string_member(object: &Map<String, J>, name: &str) -> Result<String, ()> {
    match member(object, name) {
        None | Some(J::Null) => Ok(String::new()),
        Some(J::String(s)) => Ok(s.clone()),
        Some(_) => Err(()),
    }
}

/// An `int` field: an integral number in range.
fn int_member(object: &Map<String, J>, name: &str) -> Result<i64, ()> {
    match member(object, name) {
        None | Some(J::Null) => Ok(0),
        Some(J::Number(n)) => n.as_i64().ok_or(()),
        Some(_) => Err(()),
    }
}

fn bool_member(object: &Map<String, J>, name: &str) -> Result<bool, ()> {
    match member(object, name) {
        None | Some(J::Null) => Ok(false),
        Some(J::Bool(b)) => Ok(*b),
        Some(_) => Err(()),
    }
}

/// `json.Unmarshal(data, &v)` where `v` is a struct: `null` leaves it zero,
/// any other non-object kind fails.
fn decode_object(data: &[u8]) -> Result<Map<String, J>, ()> {
    if crate::gojson::syntax_error(data).is_some() {
        return Err(());
    }
    match serde_json::from_slice::<J>(data) {
        Ok(J::Object(map)) => Ok(map),
        Ok(J::Null) => Ok(Map::new()),
        _ => Err(()),
    }
}

/// `json.NewDecoder(body).Decode(&v)`: the first value only; its framing
/// error (`EOF`, `unexpected EOF`, a syntax error) is the decode error.
fn decode_first(body: &[u8]) -> Result<J, String> {
    let end = crate::gojson::first_value(body)?;
    serde_json::from_slice::<J>(&body[..end]).map_err(|e| e.to_string())
}

/// A value decoded into a struct whose field `name` is a slice: an absent or
/// null member is an empty slice; a member of another kind is Go's type
/// error.
fn slice_member<'a>(value: &'a J, name: &str) -> Result<Vec<&'a Map<String, J>>, String> {
    let object = match value {
        J::Object(object) => object,
        J::Null => return Ok(Vec::new()),
        other => {
            return Err(format!(
                "json: cannot unmarshal {} into Go value of type struct",
                json_kind(other)
            ))
        }
    };
    match member(object, name) {
        None | Some(J::Null) => Ok(Vec::new()),
        Some(J::Array(items)) => Ok(items.iter().filter_map(J::as_object).collect()),
        Some(other) => Err(format!(
            "json: cannot unmarshal {} into Go struct field .{name} of type []struct",
            json_kind(other)
        )),
    }
}

/// `UnmarshalTypeError.Value`: the JSON kind found.
fn json_kind(value: &J) -> &'static str {
    match value {
        J::Null => "null",
        J::Bool(_) => "bool",
        J::Number(_) => "number",
        J::String(_) => "string",
        J::Array(_) => "array",
        J::Object(_) => "object",
    }
}

fn text_of(object: &Map<String, J>, name: &str) -> String {
    match member(object, name) {
        Some(J::String(s)) => s.clone(),
        _ => String::new(),
    }
}

fn int_of(object: &Map<String, J>, name: &str) -> i64 {
    match member(object, name) {
        Some(J::Number(n)) => n.as_i64().unwrap_or(0),
        _ => 0,
    }
}

// --- an http.Client with Go's error text -------------------------------------------------

struct Reply {
    status: u16,
    body: Vec<u8>,
}

/// `url.Error.Op`: the method with its first letter kept and the rest lower.
fn url_op(method: &str) -> String {
    let mut chars = method.chars();
    match chars.next() {
        Some(c) => c.to_string() + &chars.as_str().to_lowercase(),
        None => String::new(),
    }
}

/// One request through `http.Client{Timeout: timeout}`: redirects followed,
/// Go's user agent, gzip requested and undone as Go's transport does. The
/// body is read in full up to `limit` bytes.
fn http_request(
    method: &str,
    url: &str,
    headers: &[(&str, &str)],
    body: Option<&[u8]>,
    timeout: Duration,
    limit: u64,
) -> Result<Reply, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout(timeout)
        .redirects(10)
        .user_agent("Go-http-client/1.1")
        .tls_config(crate::hostartifact::tls_config())
        .build();
    let mut request = agent.request(method, url);
    let mut gzip = true;
    for (name, value) in headers {
        if name.eq_ignore_ascii_case("Accept-Encoding") {
            gzip = false;
        }
        request = request.set(name, value);
    }
    if gzip {
        request = request.set("Accept-Encoding", "gzip");
    }
    let outcome = match body {
        Some(body) => request.send_bytes(body),
        None if matches!(method, "POST" | "PUT" | "PATCH") => request.send_bytes(&[]),
        None => request.call(),
    };
    let response = match outcome {
        Ok(response) | Err(ureq::Error::Status(_, response)) => response,
        Err(ureq::Error::Transport(t)) => {
            return Err(format!(
                "{} \"{url}\": {}",
                url_op(method),
                crate::hostartifact::transport_cause(url, &t)
            ))
        }
    };
    let status = response.status();
    let gzipped = gzip
        && response
            .header("Content-Encoding")
            .is_some_and(|v| v.eq_ignore_ascii_case("gzip"));
    let mut raw = Vec::new();
    let _ = response.into_reader().take(limit).read_to_end(&mut raw);
    let body = if gzipped {
        let mut out = Vec::new();
        let _ = flate2::read::GzDecoder::new(&raw[..])
            .take(limit)
            .read_to_end(&mut out);
        out
    } else {
        raw
    };
    Ok(Reply { status, body })
}

fn ok_status(status: u16) -> bool {
    (200..300).contains(&status)
}

const UNLIMITED: u64 = u64::MAX;

// --- shared helpers (types.go) -----------------------------------------------------------

/// `nvidiaSmiCommand`.
fn nvidia_smi() -> String {
    if Path::new("/usr/lib/wsl/lib/nvidia-smi").exists() {
        "/usr/lib/wsl/lib/nvidia-smi".into()
    } else {
        "nvidia-smi".into()
    }
}

/// `cudaUserLibraryPresent`.
#[allow(dead_code)]
fn cuda_user_library_present() -> bool {
    [
        "/usr/lib/wsl/lib/libcuda.so.1",
        "/usr/lib/wsl/lib/libcuda.so",
        "/usr/lib/x86_64-linux-gnu/libcuda.so.1",
        "/usr/lib64/libcuda.so.1",
    ]
    .iter()
    .any(|p| Path::new(p).exists())
}

/// `runtime.GOARCH`.
fn go_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        "x86" => "386",
        "arm" => "arm",
        "riscv64" => "riscv64",
        "powerpc64" => "ppc64le",
        "s390x" => "s390x",
        other => other,
    }
}

/// `runtime.NumCPU`: the CPUs in this process's affinity mask.
fn num_cpu() -> i64 {
    nix::sched::sched_getaffinity(nix::unistd::Pid::from_raw(0))
        .map(|set| {
            (0..nix::sched::CpuSet::count())
                .filter(|&i| set.is_set(i).unwrap_or(false))
                .count() as i64
        })
        .ok()
        .filter(|&n| n > 0)
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |n| n.get() as i64))
}

/// `LocalLLMModelResult`.
#[derive(Clone, Debug, Default)]
pub struct ModelResult {
    pub uri: String,
    pub name: String,
    pub digest: String,
    pub size_bytes: i64,
}

/// `LocalLLMProbeResult`, its fields in Go's order.
#[derive(Clone, Debug, Default)]
pub struct ProbeResult {
    pub runtime: String,
    pub api_base_url: String,
    pub model_ref: String,
    pub effective_model_ref: String,
    pub artifact_uri: String,
    pub artifact_sha256: String,
    pub base_model: String,
    pub revision: String,
    pub tokenizer_revision: String,
    pub chat_template_hash: String,
    pub chat_template_kwargs: String,
    pub quantization: String,
    pub gpu_layers: i64,
    pub cuda_architectures: String,
    pub runtime_build: String,
    pub version: String,
    pub ready: bool,
    pub models: Vec<ModelResult>,
    pub chat_ready: bool,
    pub open_ai_models_ready: bool,
    pub gpu_accelerated: bool,
    pub cuda_enabled: bool,
    pub binary_source: String,
    pub binary_sha256: String,
    pub source_revision: String,
    pub size_vram_bytes: i64,
    pub loaded_model: String,
    pub load_error: String,
    pub remediation_hints: Vec<String>,
    pub context_length: i64,
    pub context_source: String,
    pub context_persisted: bool,
    pub max_parallel: i64,
    pub max_loaded_models: i64,
}

/// Go's `omitempty` members, in field order, as one encoder.
struct Fields(Map<String, J>);

impl Fields {
    fn new() -> Self {
        Fields(Map::new())
    }
    fn text(&mut self, key: &str, value: &str, omitempty: bool) -> &mut Self {
        if !omitempty || !value.is_empty() {
            self.0.insert(key.into(), J::from(value));
        }
        self
    }
    fn int(&mut self, key: &str, value: i64, omitempty: bool) -> &mut Self {
        if !omitempty || value != 0 {
            self.0.insert(key.into(), J::from(value));
        }
        self
    }
    fn uint(&mut self, key: &str, value: u64, omitempty: bool) -> &mut Self {
        if !omitempty || value != 0 {
            self.0.insert(key.into(), J::from(value));
        }
        self
    }
    fn flag(&mut self, key: &str, value: bool, omitempty: bool) -> &mut Self {
        if !omitempty || value {
            self.0.insert(key.into(), J::Bool(value));
        }
        self
    }
    fn list(&mut self, key: &str, values: &[String], omitempty: bool) -> &mut Self {
        if !omitempty || !values.is_empty() {
            self.0.insert(
                key.into(),
                J::Array(values.iter().map(|v| J::from(v.as_str())).collect()),
            );
        }
        self
    }
    fn value(&mut self, key: &str, value: J) -> &mut Self {
        self.0.insert(key.into(), value);
        self
    }
    fn done(&mut self) -> J {
        J::Object(std::mem::take(&mut self.0))
    }
}

pub const MODEL_SHAPE: crate::gojson::Shape = crate::gojson::Shape::Struct(&[
    ("uri", crate::gojson::Shape::Any),
    ("name", crate::gojson::Shape::Any),
    ("digest", crate::gojson::Shape::Any),
    ("sizeBytes", crate::gojson::Shape::Any),
]);

/// `LocalLLMProbeResult`'s field order, for `json.Marshal`.
pub const PROBE_SHAPE: crate::gojson::Shape = crate::gojson::Shape::Struct(&[
    ("runtime", crate::gojson::Shape::Any),
    ("apiBaseUrl", crate::gojson::Shape::Any),
    ("modelRef", crate::gojson::Shape::Any),
    ("effectiveModelRef", crate::gojson::Shape::Any),
    ("artifactUri", crate::gojson::Shape::Any),
    ("artifactSha256", crate::gojson::Shape::Any),
    ("baseModel", crate::gojson::Shape::Any),
    ("revision", crate::gojson::Shape::Any),
    ("tokenizerRevision", crate::gojson::Shape::Any),
    ("chatTemplateHash", crate::gojson::Shape::Any),
    ("chatTemplateKwargs", crate::gojson::Shape::Any),
    ("quantization", crate::gojson::Shape::Any),
    ("gpuLayers", crate::gojson::Shape::Any),
    ("cudaArchitectures", crate::gojson::Shape::Any),
    ("runtimeBuild", crate::gojson::Shape::Any),
    ("version", crate::gojson::Shape::Any),
    ("ready", crate::gojson::Shape::Any),
    ("models", crate::gojson::Shape::List(&MODEL_SHAPE)),
    ("chatReady", crate::gojson::Shape::Any),
    ("openAiModelsReady", crate::gojson::Shape::Any),
    ("gpuAccelerated", crate::gojson::Shape::Any),
    ("cudaEnabled", crate::gojson::Shape::Any),
    ("binarySource", crate::gojson::Shape::Any),
    ("binarySha256", crate::gojson::Shape::Any),
    ("sourceRevision", crate::gojson::Shape::Any),
    ("sizeVramBytes", crate::gojson::Shape::Any),
    ("loadedModel", crate::gojson::Shape::Any),
    ("loadError", crate::gojson::Shape::Any),
    ("remediationHints", crate::gojson::Shape::Any),
    ("contextLength", crate::gojson::Shape::Any),
    ("contextSource", crate::gojson::Shape::Any),
    ("contextPersisted", crate::gojson::Shape::Any),
    ("maxParallel", crate::gojson::Shape::Any),
    ("maxLoadedModels", crate::gojson::Shape::Any),
]);

impl ProbeResult {
    pub fn to_json(&self) -> J {
        let models = self
            .models
            .iter()
            .map(|m| {
                Fields::new()
                    .text("uri", &m.uri, false)
                    .text("name", &m.name, false)
                    .text("digest", &m.digest, true)
                    .int("sizeBytes", m.size_bytes, true)
                    .done()
            })
            .collect();
        Fields::new()
            .text("runtime", &self.runtime, true)
            .text("apiBaseUrl", &self.api_base_url, false)
            .text("modelRef", &self.model_ref, true)
            .text("effectiveModelRef", &self.effective_model_ref, true)
            .text("artifactUri", &self.artifact_uri, true)
            .text("artifactSha256", &self.artifact_sha256, true)
            .text("baseModel", &self.base_model, true)
            .text("revision", &self.revision, true)
            .text("tokenizerRevision", &self.tokenizer_revision, true)
            .text("chatTemplateHash", &self.chat_template_hash, true)
            .text("chatTemplateKwargs", &self.chat_template_kwargs, true)
            .text("quantization", &self.quantization, true)
            .int("gpuLayers", self.gpu_layers, true)
            .text("cudaArchitectures", &self.cuda_architectures, true)
            .text("runtimeBuild", &self.runtime_build, true)
            .text("version", &self.version, true)
            .flag("ready", self.ready, false)
            .value("models", J::Array(models))
            .flag("chatReady", self.chat_ready, true)
            .flag("openAiModelsReady", self.open_ai_models_ready, true)
            .flag("gpuAccelerated", self.gpu_accelerated, true)
            .flag("cudaEnabled", self.cuda_enabled, true)
            .text("binarySource", &self.binary_source, true)
            .text("binarySha256", &self.binary_sha256, true)
            .text("sourceRevision", &self.source_revision, true)
            .int("sizeVramBytes", self.size_vram_bytes, true)
            .text("loadedModel", &self.loaded_model, true)
            .text("loadError", &self.load_error, true)
            .list("remediationHints", &self.remediation_hints, true)
            .int("contextLength", self.context_length, true)
            .text("contextSource", &self.context_source, true)
            .flag("contextPersisted", self.context_persisted, true)
            .int("maxParallel", self.max_parallel, true)
            .int("maxLoadedModels", self.max_loaded_models, true)
            .done()
    }
}

/// `LocalLLMPrerequisitesResult`'s field order.
pub const PREREQUISITES_SHAPE: crate::gojson::Shape = crate::gojson::Shape::Struct(&[
    ("supported", crate::gojson::Shape::Any),
    ("systemdUserAvailable", crate::gojson::Shape::Any),
    ("architecture", crate::gojson::Shape::Any),
    ("cpuCount", crate::gojson::Shape::Any),
    ("modelsDirectory", crate::gojson::Shape::Any),
    ("memoryBytes", crate::gojson::Shape::Any),
    ("diskAvailableBytes", crate::gojson::Shape::Any),
    ("gpu", crate::gojson::Shape::Any),
    ("gpuMemoryTotalBytes", crate::gojson::Shape::Any),
    ("nvidiaSmiOk", crate::gojson::Shape::Any),
    ("cudaLibraryPresent", crate::gojson::Shape::Any),
    ("cmakePresent", crate::gojson::Shape::Any),
    ("ninjaPresent", crate::gojson::Shape::Any),
    ("cudaCompilerPresent", crate::gojson::Shape::Any),
    ("buildToolchainReady", crate::gojson::Shape::Any),
    ("dxgDevicePresent", crate::gojson::Shape::Any),
    ("llamaServerBinaryPresent", crate::gojson::Shape::Any),
    ("llamaServerCudaEnabled", crate::gojson::Shape::Any),
    ("llamaServerBinarySource", crate::gojson::Shape::Any),
    ("llamaServerBuildRevision", crate::gojson::Shape::Any),
    ("llamaServerBinarySha256", crate::gojson::Shape::Any),
    ("llamaServerServiceActive", crate::gojson::Shape::Any),
    ("ollamaBinaryPresent", crate::gojson::Shape::Any),
    ("ollamaServiceActive", crate::gojson::Shape::Any),
    ("ollamaApiBaseUrl", crate::gojson::Shape::Any),
    ("ollamaModel", crate::gojson::Shape::Any),
    ("ollamaNumParallel", crate::gojson::Shape::Any),
    ("ollamaMaxLoadedModels", crate::gojson::Shape::Any),
    ("runtimeGpuAccelerated", crate::gojson::Shape::Any),
    ("runtimeSizeVramBytes", crate::gojson::Shape::Any),
    ("runtimeLoadedModel", crate::gojson::Shape::Any),
    ("readyForInstall", crate::gojson::Shape::Any),
    ("readyForGpuInference", crate::gojson::Shape::Any),
    ("blockers", crate::gojson::Shape::Any),
    ("remediationHints", crate::gojson::Shape::Any),
]);

/// `LocalLLMPrerequisitesResult` as the Ollama check fills it.
#[derive(Default)]
struct Prerequisites {
    supported: bool,
    systemd_user_available: bool,
    architecture: String,
    cpu_count: i64,
    models_directory: String,
    ollama_binary_present: bool,
    ollama_service_active: bool,
    ollama_api_base_url: String,
    ollama_model: String,
    ollama_num_parallel: i64,
    ollama_max_loaded_models: i64,
    runtime_gpu_accelerated: bool,
    runtime_size_vram_bytes: i64,
    runtime_loaded_model: String,
    ready_for_install: bool,
    ready_for_gpu_inference: bool,
    blockers: Vec<String>,
}

impl Prerequisites {
    fn to_json(&self) -> J {
        Fields::new()
            .flag("supported", self.supported, false)
            .flag("systemdUserAvailable", self.systemd_user_available, false)
            .text("architecture", &self.architecture, false)
            .int("cpuCount", self.cpu_count, false)
            .text("modelsDirectory", &self.models_directory, false)
            .uint("memoryBytes", 0, true)
            .uint("diskAvailableBytes", 0, true)
            .text("gpu", "", true)
            .uint("gpuMemoryTotalBytes", 0, true)
            .flag("nvidiaSmiOk", false, false)
            .flag("cudaLibraryPresent", false, false)
            .flag("cmakePresent", false, false)
            .flag("ninjaPresent", false, false)
            .flag("cudaCompilerPresent", false, false)
            .flag("buildToolchainReady", false, false)
            .flag("dxgDevicePresent", false, true)
            .flag("llamaServerBinaryPresent", false, false)
            .flag("llamaServerCudaEnabled", false, false)
            .text("llamaServerBinarySource", "", true)
            .text("llamaServerBuildRevision", "", true)
            .text("llamaServerBinarySha256", "", true)
            .flag("llamaServerServiceActive", false, false)
            .flag("ollamaBinaryPresent", self.ollama_binary_present, false)
            .flag("ollamaServiceActive", self.ollama_service_active, false)
            .text("ollamaApiBaseUrl", &self.ollama_api_base_url, true)
            .text("ollamaModel", &self.ollama_model, true)
            .int("ollamaNumParallel", self.ollama_num_parallel, true)
            .int("ollamaMaxLoadedModels", self.ollama_max_loaded_models, true)
            .flag("runtimeGpuAccelerated", self.runtime_gpu_accelerated, true)
            .int("runtimeSizeVramBytes", self.runtime_size_vram_bytes, true)
            .text("runtimeLoadedModel", &self.runtime_loaded_model, true)
            .flag("readyForInstall", self.ready_for_install, false)
            .flag("readyForGpuInference", self.ready_for_gpu_inference, false)
            .list("blockers", &self.blockers, true)
            .list("remediationHints", &[], true)
            .done()
    }
}

/// `os.WriteFile`: create or truncate; an existing file keeps its mode.
fn write_file(path: &str, data: &[u8], mode: u32) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(mode)
        .open(path)
        .map_err(|e| crate::goerr::path_error("open", Path::new(path), &e).0)?;
    file.write_all(data)
        .map_err(|e| crate::goerr::path_error("write", Path::new(path), &e).0)
}

fn mkdir_all(path: &str, mode: u32) -> Result<(), String> {
    crate::goerr::mkdir_all(Path::new(path), mode).map_err(|e| e.0)
}

/// `filepath.Dir`.
fn dir_of(path: &str) -> String {
    match path.rfind('/') {
        Some(0) => "/".into(),
        Some(i) => clean(&path[..i]),
        None => ".".into(),
    }
}

/// `filepath.Clean`.
fn clean(path: &str) -> String {
    crate::http1::clean_path(path).pipe(|cleaned| {
        if path.starts_with('/') {
            cleaned
        } else {
            go_clean_relative(path)
        }
    })
}

trait Pipe: Sized {
    fn pipe<T>(self, f: impl FnOnce(Self) -> T) -> T {
        f(self)
    }
}
impl<T> Pipe for T {}

/// `filepath.Clean` for a relative path.
fn go_clean_relative(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if parts.last().is_some_and(|p| *p != "..") {
                    parts.pop();
                } else {
                    parts.push("..");
                }
            }
            other => parts.push(other),
        }
    }
    if parts.is_empty() {
        ".".into()
    } else {
        parts.join("/")
    }
}

/// `filepath.Clean` for any path, absolute paths without the trailing slash
/// `clean_path` keeps.
fn clean_any(path: &str) -> String {
    if path.starts_with('/') {
        let cleaned = crate::http1::clean_path(path);
        if cleaned.len() > 1 {
            cleaned.trim_end_matches('/').to_string()
        } else {
            cleaned
        }
    } else {
        go_clean_relative(path)
    }
}

/// `fileSHA256`.
fn file_sha256(path: &str) -> Result<String, String> {
    let mut file = std::fs::File::open(path)
        .map_err(|e| crate::goerr::path_error("open", Path::new(path), &e).0)?;
    let mut hash = Sha256::new();
    std::io::copy(&mut file, &mut hash)
        .map_err(|e| crate::goerr::path_error("read", Path::new(path), &e).0)?;
    Ok(hex::encode(hash.finalize()))
}

/// `os.Stat` error text.
fn stat(path: &str) -> Result<std::fs::Metadata, String> {
    std::fs::metadata(path).map_err(|e| crate::goerr::path_error("stat", Path::new(path), &e).0)
}

fn not_exist(path: &str) -> bool {
    matches!(std::fs::metadata(path), Err(e) if e.kind() == std::io::ErrorKind::NotFound)
}

// --- the Ollama runtime (ollama.go) ------------------------------------------------------

/// `OllamaModelContextConfig`.
#[derive(Clone, Debug, Default, PartialEq)]
struct ModelContext {
    effective_model_ref: String,
    context_size: i64,
    updated_at: String,
}

/// `OllamaRuntimeConfig`.
#[derive(Clone, Debug, Default)]
struct OllamaConfig {
    port: i64,
    model_ref: String,
    binary_path: String,
    models_directory: String,
    context_size: i64,
    model_contexts: std::collections::BTreeMap<String, ModelContext>,
}

impl OllamaConfig {
    /// `json.MarshalIndent(cfg, "", "  ")`.
    fn marshal_indent(&self) -> String {
        let mut compact = String::from("{");
        let mut push = |key: &str, value: &str, first: bool| {
            if !first {
                compact.push(',');
            }
            crate::gojson::encode_string(key, &mut compact);
            compact.push(':');
            compact.push_str(value);
        };
        let quoted = |s: &str| {
            let mut out = String::new();
            crate::gojson::encode_string(s, &mut out);
            out
        };
        push("port", &self.port.to_string(), true);
        push("modelRef", &quoted(&self.model_ref), false);
        push("binaryPath", &quoted(&self.binary_path), false);
        if !self.models_directory.is_empty() {
            push("modelsDirectory", &quoted(&self.models_directory), false);
        }
        if self.context_size != 0 {
            push("contextSize", &self.context_size.to_string(), false);
        }
        if !self.model_contexts.is_empty() {
            let mut map = String::from("{");
            for (i, (key, value)) in self.model_contexts.iter().enumerate() {
                if i > 0 {
                    map.push(',');
                }
                crate::gojson::encode_string(key, &mut map);
                map.push_str(":{\"effectiveModelRef\":");
                crate::gojson::encode_string(&value.effective_model_ref, &mut map);
                map.push_str(&format!(",\"contextSize\":{}", value.context_size));
                if !value.updated_at.is_empty() {
                    map.push_str(",\"updatedAt\":");
                    crate::gojson::encode_string(&value.updated_at, &mut map);
                }
                map.push('}');
            }
            map.push('}');
            push("modelContexts", &map, false);
        }
        compact.push('}');
        crate::gojson::indent(&compact, "  ")
    }

    /// `json.Unmarshal` into `OllamaRuntimeConfig`.
    fn unmarshal(data: &[u8]) -> Result<Self, ()> {
        let object = decode_object(data)?;
        let mut contexts = std::collections::BTreeMap::new();
        match member(&object, "modelContexts") {
            None | Some(J::Null) => {}
            Some(J::Object(map)) => {
                for (key, value) in map {
                    let entry = match value {
                        J::Null => ModelContext::default(),
                        J::Object(v) => ModelContext {
                            effective_model_ref: string_member(v, "effectiveModelRef")?,
                            context_size: int_member(v, "contextSize")?,
                            updated_at: string_member(v, "updatedAt")?,
                        },
                        _ => return Err(()),
                    };
                    contexts.insert(key.clone(), entry);
                }
            }
            Some(_) => return Err(()),
        }
        Ok(OllamaConfig {
            port: int_member(&object, "port")?,
            model_ref: string_member(&object, "modelRef")?,
            binary_path: string_member(&object, "binaryPath")?,
            models_directory: string_member(&object, "modelsDirectory")?,
            context_size: int_member(&object, "contextSize")?,
            model_contexts: contexts,
        })
    }
}

/// `ollamaModelRefPattern`: `^[A-Za-z0-9][A-Za-z0-9._:/@-]{0,199}$`.
fn ollama_model_ref_ok(value: &str) -> bool {
    let b = value.as_bytes();
    !b.is_empty()
        && b.len() <= 200
        && b[0].is_ascii_alphanumeric()
        && b[1..]
            .iter()
            .all(|c| c.is_ascii_alphanumeric() || b"._:/@-".contains(c))
}

impl Llm {
    /// `ollamaRuntimeConfigPath`.
    fn ollama_config_path(&self) -> Result<String, String> {
        let home = self
            .home()
            .map_err(|e| format!("resolve home directory: {e}"))?;
        Ok(format!("{home}/.config/opute/ollama-runtime.json"))
    }

    /// `ollamaContextSizeFromEnvironment`.
    fn ollama_context_from_env(&self) -> i64 {
        for name in ["OPUTE_OLLAMA_CONTEXT_SIZE", "OLLAMA_CONTEXT_LENGTH"] {
            let raw = self.env.get(name);
            let raw = raw.trim();
            if !raw.is_empty() {
                if let Some(parsed) = atoi(raw) {
                    if (OLLAMA_CONTEXT_MINIMUM..=OLLAMA_CONTEXT_MAXIMUM).contains(&parsed) {
                        return parsed;
                    }
                }
            }
        }
        OLLAMA_CONTEXT_MAXIMUM
    }

    /// `defaultOllamaRuntimeConfig`.
    fn default_ollama_config(&self) -> OllamaConfig {
        let mut port = DEFAULT_OLLAMA_PORT;
        let raw = self.env.get("OPUTE_OLLAMA_PORT");
        if !raw.trim().is_empty() {
            if let Some(parsed) = atoi(raw.trim()) {
                if parsed > 0 && parsed < 65536 {
                    port = parsed;
                }
            }
        }
        let mut binary_path = self.env.get("OPUTE_OLLAMA_BINARY").trim().to_string();
        if binary_path.is_empty() {
            if let Some(found) = crate::hostobs::look_path(&self.env, "ollama") {
                binary_path = found.display().to_string();
            }
        }
        let model = self.env.get("OPUTE_OLLAMA_MODEL");
        OllamaConfig {
            port,
            model_ref: first_non_empty(&[model.trim(), DEFAULT_OLLAMA_MODEL]).to_string(),
            binary_path,
            models_directory: self.env.get("OLLAMA_MODELS").trim().to_string(),
            context_size: self.ollama_context_from_env(),
            model_contexts: Default::default(),
        }
    }

    /// `loadOllamaRuntimeConfig`.
    fn load_ollama_config(&self) -> OllamaConfig {
        let mut cfg = self.default_ollama_config();
        let Ok(path) = self.ollama_config_path() else {
            return cfg;
        };
        let Ok(data) = std::fs::read(&path) else {
            return cfg;
        };
        let Ok(persisted) = OllamaConfig::unmarshal(&data) else {
            return cfg;
        };
        if persisted.port > 0 {
            cfg.port = persisted.port;
        }
        if !persisted.model_ref.trim().is_empty() {
            cfg.model_ref = persisted.model_ref.trim().to_string();
        }
        if !persisted.binary_path.trim().is_empty() {
            cfg.binary_path = persisted.binary_path.trim().to_string();
        }
        if !persisted.models_directory.trim().is_empty() {
            cfg.models_directory = persisted.models_directory.trim().to_string();
        }
        if (OLLAMA_CONTEXT_MINIMUM..=OLLAMA_CONTEXT_MAXIMUM).contains(&persisted.context_size) {
            cfg.context_size = persisted.context_size;
        }
        if !persisted.model_contexts.is_empty() {
            cfg.model_contexts = persisted.model_contexts;
        }
        if cfg.context_size <= 0 {
            cfg.context_size = self.read_ollama_service_context_size();
        }
        cfg
    }

    /// `readOllamaServiceContextSize`.
    fn read_ollama_service_context_size(&self) -> i64 {
        let Ok(home) = self.home() else { return 0 };
        let base = format!("{home}/.config/systemd/user");
        for name in ["ollama.service", OLLAMA_SERVICE_NAME] {
            let Ok(data) = std::fs::read(format!("{base}/{name}")) else {
                continue;
            };
            for line in String::from_utf8_lossy(&data).split('\n') {
                let Some((_, value)) = line.split_once("OLLAMA_CONTEXT_LENGTH=") else {
                    continue;
                };
                let value = value.trim().trim_matches(['"', '\'']);
                if let Some(parsed) = atoi(value) {
                    if (OLLAMA_CONTEXT_MINIMUM..=OLLAMA_CONTEXT_MAXIMUM).contains(&parsed) {
                        return parsed;
                    }
                }
            }
        }
        0
    }

    /// `saveOllamaRuntimeConfig`: a 0600 temporary file renamed into place.
    fn save_ollama_config(&self, cfg: &OllamaConfig) -> Result<(), String> {
        use std::io::Write;
        let path = self.ollama_config_path()?;
        let dir = dir_of(&path);
        mkdir_all(&dir, 0o700)?;
        let data = cfg.marshal_indent();
        let (mut file, temporary) = crate::goerr::create_temp(&dir, ".ollama-runtime-", ".tmp")?;
        let result = (|| {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))
                .map_err(|e| crate::goerr::path_error("chmod", Path::new(&temporary), &e).0)?;
            file.write_all(format!("{data}\n").as_bytes())
                .map_err(|e| crate::goerr::path_error("write", Path::new(&temporary), &e).0)?;
            drop(file);
            std::fs::rename(&temporary, &path).map_err(|e| {
                format!(
                    "rename {temporary} {path}: {}",
                    crate::goerr::errno_text(&e)
                )
            })
        })();
        let _ = std::fs::remove_file(&temporary);
        result
    }

    /// `renderOllamaSystemdUnit`.
    fn render_ollama_unit(cfg: &OllamaConfig) -> Result<String, String> {
        if cfg.context_size != 0
            && !(OLLAMA_CONTEXT_MINIMUM..=OLLAMA_CONTEXT_MAXIMUM).contains(&cfg.context_size)
        {
            return Err(format!(
                "contextSize must be between {OLLAMA_CONTEXT_MINIMUM} and {OLLAMA_CONTEXT_MAXIMUM} tokens"
            ));
        }
        if cfg.binary_path.trim().is_empty() {
            return Err("ollama binary is not installed".into());
        }
        let port = if cfg.port == 0 {
            DEFAULT_OLLAMA_PORT
        } else {
            cfg.port
        };
        let mut lines: Vec<String> = [
            "[Unit]",
            "Description=Opute shared Ollama runtime",
            "After=network-online.target",
            "StartLimitIntervalSec=60s",
            "StartLimitBurst=5",
            "",
            "[Service]",
            "Type=simple",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        lines.push(format!("ExecStart={} serve", cfg.binary_path));
        lines.push(format!("Environment=OLLAMA_HOST=127.0.0.1:{port}"));
        lines.push(format!(
            "Environment=OLLAMA_NUM_PARALLEL={OLLAMA_NUM_PARALLEL}"
        ));
        lines.push(format!(
            "Environment=OLLAMA_MAX_LOADED_MODELS={OLLAMA_MAX_LOADED_MODELS}"
        ));
        for line in [
            "Environment=OLLAMA_KEEP_ALIVE=-1",
            "Slice=opute-workload.slice",
            "KillMode=control-group",
            "MemoryHigh=10G",
            "MemoryMax=11G",
            "MemorySwapMax=1G",
            "CPUQuota=600%",
            "CPUWeight=100",
            "TasksMax=4096",
            "Restart=on-failure",
            "RestartSec=2",
            "",
            "[Install]",
            "WantedBy=default.target",
        ] {
            lines.push(line.into());
        }
        if !cfg.models_directory.trim().is_empty() {
            // Go appends to lines[:len-3] in place: the models line
            // overwrites the blank line before [Install], and the tail it
            // then re-appends starts with that same models line.
            let at = lines.len() - 3;
            lines[at] = format!("Environment=OLLAMA_MODELS={}", cfg.models_directory);
            lines.insert(at, lines[at].clone());
        }
        if cfg.context_size > 0 {
            let at = lines.len() - 3;
            lines.insert(
                at,
                format!("Environment=OLLAMA_CONTEXT_LENGTH={}", cfg.context_size),
            );
        }
        Ok(lines.join("\n") + "\n")
    }

    /// `ensureOllamaRuntime`.
    fn ensure_ollama_runtime(&self, ctx: &RunCtx, cfg: &OllamaConfig) -> Result<(), String> {
        if cfg.binary_path.is_empty() {
            return Err("ollama binary is not installed".into());
        }
        if probe_ollama_api(cfg.port).is_ok() {
            return self.retire_ollama_unit_raced_by_external_owner();
        }
        let unit = Self::render_ollama_unit(cfg)?;
        let home = self.home()?;
        let unit_path = format!("{home}/.config/systemd/user/{OLLAMA_SERVICE_NAME}");
        let old = std::fs::read(&unit_path);
        if old.as_ref().map_or(true, |old| old != unit.as_bytes()) {
            mkdir_all(&dir_of(&unit_path), 0o700)?;
            write_file(&unit_path, unit.as_bytes(), 0o600)?;
            let (output, result) = self.systemctl_user(&["daemon-reload"]);
            if let Err(err) = result {
                return Err(format!(
                    "reload Ollama systemd unit: {err}: {}",
                    output.trim()
                ));
            }
        }
        let (output, result) = self.systemctl_user(&["enable", "--now", OLLAMA_SERVICE_NAME]);
        if let Err(err) = result {
            return Err(format!(
                "start shared Ollama systemd unit: {err}: {}",
                output.trim()
            ));
        }
        wait_for_ollama_api(ctx, cfg.port)
    }

    /// `retireOllamaUnitRacedByExternalOwner`.
    fn retire_ollama_unit_raced_by_external_owner(&self) -> Result<(), String> {
        let home = self.home()?;
        let unit_path = format!("{home}/.config/systemd/user/{OLLAMA_SERVICE_NAME}");
        if std::fs::metadata(&unit_path).is_err() {
            return Ok(());
        }
        if self
            .systemctl_user(&["is-active", "--quiet", OLLAMA_SERVICE_NAME])
            .1
            .is_ok()
        {
            return Ok(());
        }
        let (output, result) = self.systemctl_user(&["disable", "--now", OLLAMA_SERVICE_NAME]);
        if let Err(err) = result {
            return Err(format!(
                "retire raced Ollama unit {OLLAMA_SERVICE_NAME}: {err}: {}",
                output.trim()
            ));
        }
        Ok(())
    }

    /// `CheckOllamaPrerequisites`.
    pub fn check_ollama_prerequisites(&self, ctx: &RunCtx) -> J {
        let cfg = self.load_ollama_config();
        let mut result = Prerequisites {
            supported: true,
            architecture: go_arch().into(),
            cpu_count: num_cpu(),
            models_directory: cfg.models_directory.clone(),
            ollama_api_base_url: format!("http://127.0.0.1:{}/v1", cfg.port),
            ollama_model: cfg.model_ref.clone(),
            ollama_num_parallel: OLLAMA_NUM_PARALLEL,
            ollama_max_loaded_models: OLLAMA_MAX_LOADED_MODELS,
            ..Default::default()
        };
        // The shared runner reports a failed command as a result, so this
        // check always succeeds, as in Go.
        let _ = self.host(
            &["systemctl", "--user", "show-environment"],
            Duration::from_secs(10),
        );
        result.systemd_user_available = true;
        if !cfg.binary_path.is_empty() && std::fs::metadata(&cfg.binary_path).is_ok() {
            result.ollama_binary_present = true;
        }
        let res = self.host(
            &["systemctl", "--user", "is-active", OLLAMA_SERVICE_NAME],
            Duration::from_secs(5),
        );
        result.ollama_service_active = res.stdout.trim() == "active";
        if !result.ollama_service_active && probe_ollama_api(cfg.port).is_ok() {
            result.ollama_service_active = true;
        }
        if result.ollama_service_active {
            let probe = self.probe_ollama(
                ctx,
                &ProbeArgs {
                    include_chat: false,
                    model_ref: cfg.model_ref.clone(),
                },
            );
            result.runtime_gpu_accelerated = probe.gpu_accelerated;
            result.runtime_size_vram_bytes = probe.size_vram_bytes;
            result.runtime_loaded_model = probe.loaded_model;
        }
        result.ready_for_install =
            result.supported && result.systemd_user_available && result.ollama_binary_present;
        result.ready_for_gpu_inference = result.runtime_gpu_accelerated;
        if !result.ollama_binary_present {
            result
                .blockers
                .push("ollama binary is not installed".into());
        }
        result.to_json()
    }

    /// `InstallOllamaModel`.
    pub fn install_ollama_model(
        &self,
        ctx: &RunCtx,
        args: &InstallOllamaArgs,
    ) -> Result<ProbeResult, String> {
        let mut cfg = self.load_ollama_config();
        let model_ref = args.model_ref.trim();
        if !ollama_model_ref_ok(model_ref) {
            return Err("invalid Ollama model reference".into());
        }
        if let Some(port) = args.port.filter(|p| *p > 0) {
            cfg.port = port;
        }
        if args.set_default {
            cfg.model_ref = model_ref.to_string();
        }
        self.save_ollama_config(&cfg)?;
        self.ensure_ollama_runtime(ctx, &cfg)?;
        if cfg.binary_path.is_empty() {
            return Err("ollama binary is not installed".into());
        }
        self.host_ctx(
            ctx,
            &[cfg.binary_path.clone(), "pull".into(), model_ref.into()],
            Duration::from_secs(45 * 60),
        )
        .map_err(|e| format!("pull Ollama model {}: {e}", quote(model_ref)))?;
        let mut effective = model_ref.to_string();
        if let Some(configured) = cfg.model_contexts.get(model_ref) {
            if (OLLAMA_CONTEXT_MINIMUM..=OLLAMA_CONTEXT_MAXIMUM).contains(&configured.context_size)
                && !configured.effective_model_ref.trim().is_empty()
            {
                effective = configured.effective_model_ref.trim().to_string();
            }
        }
        let warm = args.role.trim() != "embedding";
        if warm {
            warm_ollama_model(&cfg, &effective)
                .map_err(|e| format!("warm Ollama model {}: {e}", quote(&effective)))?;
        }
        let probe = self.probe_ollama(
            ctx,
            &ProbeArgs {
                include_chat: warm,
                model_ref: model_ref.into(),
            },
        );
        if !probe.ready || !probe.open_ai_models_ready {
            return Err(format!(
                "Ollama model {} did not become ready: {}",
                quote(model_ref),
                probe.load_error
            ));
        }
        if warm && probe.loaded_model.trim().is_empty() {
            return Err(format!(
                "Ollama model {} did not become resident after warm",
                quote(&effective)
            ));
        }
        Ok(probe)
    }

    /// `StartOllamaRuntime`.
    pub fn start_ollama_runtime(&self, ctx: &RunCtx) -> Result<ProbeResult, String> {
        let cfg = self.load_ollama_config();
        self.ensure_ollama_runtime(ctx, &cfg)?;
        Ok(self.probe_ollama(
            ctx,
            &ProbeArgs {
                include_chat: true,
                model_ref: cfg.model_ref.clone(),
            },
        ))
    }

    /// `readOllamaModelContext`.
    fn read_ollama_model_context(
        &self,
        cfg: &OllamaConfig,
        model_ref: &str,
    ) -> Result<ContextResult, String> {
        let model_ref = model_ref.trim();
        if !ollama_model_ref_ok(model_ref) {
            return Err("invalid Ollama model reference".into());
        }
        let mut effective = model_ref.to_string();
        let mut persisted = false;
        if let Some(configured) = cfg.model_contexts.get(model_ref) {
            if !configured.effective_model_ref.trim().is_empty() {
                effective = configured.effective_model_ref.clone();
                persisted = true;
            }
        }
        let root = format!("http://127.0.0.1:{}", cfg.port);
        let timeout = Duration::from_secs(15);
        let details = ollama_api_request(
            &root,
            "POST",
            "/api/show",
            Some(&json_body(&[("model", J::from(effective.as_str()))])),
            timeout,
        )?;
        let parameters = match &details {
            J::Object(o) => text_of(o, "parameters"),
            _ => String::new(),
        };
        let mut context_size = parse_ollama_context_size(&parameters);
        let mut context_source = String::new();
        if context_size == 0 {
            if let Ok(running) = ollama_api_request(&root, "GET", "/api/ps", None, timeout) {
                if let Ok(models) = slice_member(&running, "models") {
                    for model in models {
                        let name = text_of(model, "name");
                        let length = int_of(model, "context_length");
                        if (ollama_model_names_match(&name, &effective)
                            || ollama_model_names_match(&name, model_ref))
                            && length > 0
                        {
                            context_size = length;
                            context_source = "ollama-runtime".into();
                            break;
                        }
                    }
                }
            }
        }
        if context_size == 0 && cfg.context_size > 0 {
            context_size = cfg.context_size;
            context_source = "ollama-service".into();
        }
        let source = if persisted {
            "managed-model".to_string()
        } else if !context_source.is_empty() {
            context_source
        } else if context_size > 0 {
            "ollama-model".into()
        } else {
            String::new()
        };
        Ok(ContextResult {
            model_ref: model_ref.into(),
            effective_model_ref: effective,
            context_size,
            context_source: source,
            persisted,
            changed: false,
        })
    }

    /// `GetOllamaModelContext`.
    pub fn get_ollama_model_context(&self, model_ref: &str) -> Result<ContextResult, String> {
        let cfg = self.load_ollama_config();
        let model_ref = if model_ref.trim().is_empty() {
            cfg.model_ref.clone()
        } else {
            model_ref.to_string()
        };
        self.read_ollama_model_context(&cfg, &model_ref)
    }

    /// `ConfigureOllamaModelContext`.
    pub fn configure_ollama_model_context(
        &self,
        model_ref: &str,
        context_size: i64,
    ) -> Result<ContextResult, String> {
        let mut cfg = self.load_ollama_config();
        let model_ref = model_ref.trim();
        if !ollama_model_ref_ok(model_ref) {
            return Err("invalid Ollama model reference".into());
        }
        if !(OLLAMA_CONTEXT_MINIMUM..=OLLAMA_CONTEXT_MAXIMUM).contains(&context_size) {
            return Err(format!(
                "contextSize must be between {OLLAMA_CONTEXT_MINIMUM} and {OLLAMA_CONTEXT_MAXIMUM} tokens"
            ));
        }
        let root = format!("http://127.0.0.1:{}", cfg.port);
        let timeout = Duration::from_secs(15);
        ollama_api_request(
            &root,
            "POST",
            "/api/show",
            Some(&json_body(&[("model", J::from(model_ref))])),
            timeout,
        )
        .map_err(|e| format!("inspect Ollama model {}: {e}", quote(model_ref)))?;
        let effective = ollama_model_context_alias(model_ref);
        if let Some(configured) = cfg.model_contexts.get(model_ref) {
            if configured.context_size == context_size
                && configured.effective_model_ref == effective
            {
                if let Ok(mut current) = self.read_ollama_model_context(&cfg, model_ref) {
                    if current.context_size == context_size {
                        current.changed = false;
                        return Ok(current);
                    }
                }
            }
        }
        // `map[string]any{"model", "from", "parameters", "stream"}`, keys
        // sorted as Go marshals a map.
        let mut parameters = Map::new();
        parameters.insert("num_ctx".into(), J::from(context_size));
        ollama_api_request(
            &root,
            "POST",
            "/api/create",
            Some(&json_body(&[
                ("from", J::from(model_ref)),
                ("model", J::from(effective.as_str())),
                ("parameters", J::Object(parameters)),
                ("stream", J::Bool(false)),
            ])),
            timeout,
        )
        .map_err(|e| format!("persist Ollama context for model {}: {e}", quote(model_ref)))?;
        cfg.model_contexts.insert(
            model_ref.to_string(),
            ModelContext {
                effective_model_ref: effective.clone(),
                context_size,
                updated_at: crate::hostobs::rfc3339_now(),
            },
        );
        self.save_ollama_config(&cfg)
            .map_err(|e| format!("persist Ollama model context mapping: {e}"))?;
        Ok(ContextResult {
            model_ref: model_ref.into(),
            effective_model_ref: effective,
            context_size,
            context_source: "managed-model".into(),
            persisted: true,
            changed: true,
        })
    }

    /// `ProbeOllama`.
    pub fn probe_ollama(&self, _ctx: &RunCtx, args: &ProbeArgs) -> ProbeResult {
        let mut cfg = self.load_ollama_config();
        if !args.model_ref.trim().is_empty() {
            cfg.model_ref = args.model_ref.trim().to_string();
        }
        let root = format!("http://127.0.0.1:{}", cfg.port);
        let mut result = ProbeResult {
            runtime: "ollama".into(),
            api_base_url: format!("{root}/v1"),
            model_ref: cfg.model_ref.clone(),
            effective_model_ref: cfg.model_ref.clone(),
            max_parallel: OLLAMA_NUM_PARALLEL,
            max_loaded_models: OLLAMA_MAX_LOADED_MODELS,
            ..Default::default()
        };
        let context = self.read_ollama_model_context(&cfg, &cfg.model_ref);
        if let Ok(context) = &context {
            result.effective_model_ref = context.effective_model_ref.clone();
            result.context_length = context.context_size;
            result.context_source = context.context_source.clone();
            result.context_persisted = context.persisted;
        }
        let effective = result.effective_model_ref.clone();
        let timeout = Duration::from_secs(15);
        let request_json = |path: &str| -> Result<J, String> {
            let reply = http_request(
                "GET",
                &format!("{root}{path}"),
                &[],
                None,
                timeout,
                UNLIMITED,
            )?;
            if !ok_status(reply.status) {
                return Err(format!("Ollama {path} returned HTTP {}", reply.status));
            }
            decode_first(&reply.body)
        };
        let tags = match request_json("/api/tags").and_then(|v| {
            slice_member(&v, "models").map(|models| {
                models
                    .into_iter()
                    .map(|m| ModelResult {
                        uri: String::new(),
                        name: text_of(m, "name"),
                        digest: text_of(m, "digest"),
                        size_bytes: int_of(m, "size"),
                    })
                    .collect::<Vec<_>>()
            })
        }) {
            Ok(tags) => tags,
            Err(err) => {
                result.load_error = err;
                return result;
            }
        };
        result.models = tags;
        let installed = result.models.iter().any(|m| {
            ollama_model_names_match(&m.name, &cfg.model_ref)
                || ollama_model_names_match(&m.name, &effective)
        });
        if let Ok(running) = request_json("/api/ps") {
            if let Ok(models) = slice_member(&running, "models") {
                for model in models {
                    let name = text_of(model, "name");
                    if ollama_model_names_match(&name, &cfg.model_ref)
                        || ollama_model_names_match(&name, &effective)
                    {
                        let vram = int_of(model, "size_vram");
                        result.loaded_model = name;
                        result.size_vram_bytes = vram;
                        result.gpu_accelerated = vram > 0;
                        break;
                    }
                }
            }
        }
        if request_json("/v1/models")
            .and_then(|v| slice_member(&v, "data").map(|_| ()))
            .is_ok()
        {
            result.open_ai_models_ready = true;
        }
        result.ready = installed;
        result.chat_ready = installed && result.open_ai_models_ready;
        if let Err(err) = context {
            if result.load_error.is_empty() {
                result.load_error = err;
            }
        }
        if args.include_chat && !result.chat_ready {
            result
                .remediation_hints
                .push("pull the configured Ollama model and retry the readiness probe".into());
        }
        result
    }
}

/// `ProbeOllamaArgs` / `ProbeLlamaServerArgs`.
#[derive(Clone, Debug, Default)]
pub struct ProbeArgs {
    pub include_chat: bool,
    pub model_ref: String,
}

/// `InstallOllamaModelArgs`.
#[derive(Clone, Debug, Default)]
pub struct InstallOllamaArgs {
    pub model_ref: String,
    pub port: Option<i64>,
    pub role: String,
    pub set_default: bool,
}

/// `OllamaModelContextResult`.
#[derive(Clone, Debug, Default)]
pub struct ContextResult {
    pub model_ref: String,
    pub effective_model_ref: String,
    pub context_size: i64,
    pub context_source: String,
    pub persisted: bool,
    pub changed: bool,
}

impl ContextResult {
    pub fn to_json(&self) -> J {
        Fields::new()
            .text("modelRef", &self.model_ref, false)
            .text("effectiveModelRef", &self.effective_model_ref, false)
            .int("contextSize", self.context_size, true)
            .text("contextSource", &self.context_source, true)
            .flag("persisted", self.persisted, false)
            .flag("changed", self.changed, true)
            .done()
    }
}

/// A JSON request body as `json.Marshal` of a `map[string]...` writes it:
/// keys sorted, HTML escaped.
fn json_body(members: &[(&str, J)]) -> Vec<u8> {
    let mut map = Map::new();
    for (k, v) in members {
        map.insert((*k).into(), v.clone());
    }
    crate::gojson::encode_to_string(&J::Object(map)).into_bytes()
}

/// `probeOllamaAPI`.
fn probe_ollama_api(port: i64) -> Result<(), String> {
    let port = if port <= 0 { DEFAULT_OLLAMA_PORT } else { port };
    let reply = http_request(
        "GET",
        &format!("http://127.0.0.1:{port}/api/tags"),
        &[],
        None,
        Duration::from_secs(2),
        UNLIMITED,
    )?;
    if !ok_status(reply.status) {
        return Err(format!("Ollama API returned HTTP {}", reply.status));
    }
    Ok(())
}

/// `waitForOllamaAPI`: 45s of 250ms polls.
fn wait_for_ollama_api(ctx: &RunCtx, port: i64) -> Result<(), String> {
    let port = if port <= 0 { DEFAULT_OLLAMA_PORT } else { port };
    let deadline = Instant::now() + Duration::from_secs(45);
    let mut last: Option<String> = None;
    while Instant::now() < deadline {
        match http_request(
            "GET",
            &format!("http://127.0.0.1:{port}/api/tags"),
            &[],
            None,
            Duration::from_secs(2),
            UNLIMITED,
        ) {
            Ok(reply) if ok_status(reply.status) => return Ok(()),
            Ok(reply) => last = Some(format!("Ollama API returned HTTP {}", reply.status)),
            Err(err) => last = Some(err),
        }
        if let Some(err) = ctx.err() {
            return Err(err);
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    let last = last.unwrap_or_else(|| "timeout waiting for Ollama API".into());
    Err(format!("wait for shared Ollama API: {last}"))
}

/// `warmOllamaModel`: one bounded generation so residency shows in /api/ps.
fn warm_ollama_model(cfg: &OllamaConfig, model_ref: &str) -> Result<(), String> {
    let model_ref = model_ref.trim();
    if model_ref.is_empty() {
        return Err("model reference is empty".into());
    }
    let mut options = Map::new();
    options.insert("num_predict".into(), J::from(1));
    ollama_api_request(
        &format!("http://127.0.0.1:{}", cfg.port),
        "POST",
        "/api/generate",
        Some(&json_body(&[
            ("keep_alive", J::from(-1)),
            ("model", J::from(model_ref)),
            ("options", J::Object(options)),
            ("prompt", J::from(".")),
            ("stream", J::Bool(false)),
        ])),
        Duration::from_secs(5 * 60),
    )
    .map(|_| ())
}

/// `ollamaModelContextAlias`: one stable managed model per base model.
fn ollama_model_context_alias(model_ref: &str) -> String {
    let digest = Sha256::digest(model_ref.trim().to_lowercase().as_bytes());
    format!("opute/context-{}", hex::encode(&digest[..8]))
}

/// `parseOllamaContextSize`: the value after `num_ctx`.
fn parse_ollama_context_size(parameters: &str) -> i64 {
    let fields: Vec<&str> = parameters.split_whitespace().collect();
    for pair in fields.windows(2) {
        if pair[0] != "num_ctx" {
            continue;
        }
        if let Some(value) = atoi(pair[1].trim()) {
            if value > 0 {
                return value;
            }
        }
    }
    0
}

/// `ollamaAPIRequest`: a non-2xx reply reports Ollama's own `error` when it
/// has one. The decoded body is returned for callers with a destination.
fn ollama_api_request(
    root: &str,
    method: &str,
    path: &str,
    body: Option<&[u8]>,
    timeout: Duration,
) -> Result<J, String> {
    let headers: &[(&str, &str)] = if body.is_some() {
        &[("Content-Type", "application/json")]
    } else {
        &[]
    };
    // A nil body is an empty reader: no length on a GET, zero on a POST.
    let reply = http_request(
        method,
        &format!("{root}{path}"),
        headers,
        body,
        timeout,
        UNLIMITED,
    )?;
    if !ok_status(reply.status) {
        if let Ok(J::Object(message)) = decode_first(&reply.body) {
            if let Some(J::String(error)) = member(&message, "error") {
                if !error.trim().is_empty() {
                    return Err(format!(
                        "Ollama {path} returned HTTP {}: {error}",
                        reply.status
                    ));
                }
            }
        }
        return Err(format!("Ollama {path} returned HTTP {}", reply.status));
    }
    decode_first(&reply.body)
}

/// `ollamaModelNamesMatch`.
fn ollama_model_names_match(left: &str, right: &str) -> bool {
    let normalize = |value: &str| {
        value
            .strip_suffix(":latest")
            .unwrap_or(value)
            .pipe(|v| v.strip_suffix("@latest").unwrap_or(v))
            .trim()
            .to_lowercase()
    };
    let (a, b) = (normalize(left), normalize(right));
    !a.is_empty()
        && !b.is_empty()
        && (a == b || a.ends_with(&format!("/{b}")) || b.ends_with(&format!("/{a}")))
}

// --- llama-server (server.go, server_build.go) -------------------------------------------

/// `LlamaServerConfig`: the host-owned serving manifest.
#[derive(Clone, Debug, Default)]
struct LlamaConfig {
    port: i64,
    model_ref: String,
    artifact_path: String,
    artifact_uri: String,
    artifact_sha256: String,
    base_model: String,
    revision: String,
    tokenizer_revision: String,
    chat_template_hash: String,
    chat_template: String,
    chat_template_kwargs: String,
    quantization: String,
    context_size: i64,
    gpu_layers: i64,
    binary_path: String,
    binary_version: String,
    binary_sha256: String,
    binary_source: String,
    source_uri: String,
    source_revision: String,
    source_sha256: String,
    cuda_enabled: bool,
    cuda_architectures: String,
    runtime_build: String,
}

impl LlamaConfig {
    /// `json.MarshalIndent(cfg, "", "  ")`.
    fn marshal_indent(&self) -> String {
        let mut out = String::from("{");
        let mut first = true;
        let mut put = |key: &str, value: String| {
            if !first {
                out.push(',');
            }
            first = false;
            crate::gojson::encode_string(key, &mut out);
            out.push(':');
            out.push_str(&value);
        };
        let s = |v: &str| {
            let mut o = String::new();
            crate::gojson::encode_string(v, &mut o);
            o
        };
        put("port", self.port.to_string());
        put("modelRef", s(&self.model_ref));
        put("artifactPath", s(&self.artifact_path));
        if !self.artifact_uri.is_empty() {
            put("artifactUri", s(&self.artifact_uri));
        }
        put("artifactSha256", s(&self.artifact_sha256));
        put("baseModel", s(&self.base_model));
        put("revision", s(&self.revision));
        put("tokenizerRevision", s(&self.tokenizer_revision));
        put("chatTemplateHash", s(&self.chat_template_hash));
        put("chatTemplate", s(&self.chat_template));
        put("chatTemplateKwargs", s(&self.chat_template_kwargs));
        put("quantization", s(&self.quantization));
        put("contextSize", self.context_size.to_string());
        put("gpuLayers", self.gpu_layers.to_string());
        put("binaryPath", s(&self.binary_path));
        put("binaryVersion", s(&self.binary_version));
        put("binarySha256", s(&self.binary_sha256));
        put("binarySource", s(&self.binary_source));
        if !self.source_uri.is_empty() {
            put("sourceUri", s(&self.source_uri));
        }
        put("sourceRevision", s(&self.source_revision));
        put("sourceSha256", s(&self.source_sha256));
        put("cudaEnabled", self.cuda_enabled.to_string());
        if !self.cuda_architectures.is_empty() {
            put("cudaArchitectures", s(&self.cuda_architectures));
        }
        put("runtimeBuild", s(&self.runtime_build));
        out.push('}');
        crate::gojson::indent(&out, "  ")
    }

    /// `json.Unmarshal` into `LlamaServerConfig`.
    fn unmarshal(data: &[u8]) -> Result<Self, ()> {
        let o = decode_object(data)?;
        Ok(LlamaConfig {
            port: int_member(&o, "port")?,
            model_ref: string_member(&o, "modelRef")?,
            artifact_path: string_member(&o, "artifactPath")?,
            artifact_uri: string_member(&o, "artifactUri")?,
            artifact_sha256: string_member(&o, "artifactSha256")?,
            base_model: string_member(&o, "baseModel")?,
            revision: string_member(&o, "revision")?,
            tokenizer_revision: string_member(&o, "tokenizerRevision")?,
            chat_template_hash: string_member(&o, "chatTemplateHash")?,
            chat_template: string_member(&o, "chatTemplate")?,
            chat_template_kwargs: string_member(&o, "chatTemplateKwargs")?,
            quantization: string_member(&o, "quantization")?,
            context_size: int_member(&o, "contextSize")?,
            gpu_layers: int_member(&o, "gpuLayers")?,
            binary_path: string_member(&o, "binaryPath")?,
            binary_version: string_member(&o, "binaryVersion")?,
            binary_sha256: string_member(&o, "binarySha256")?,
            binary_source: string_member(&o, "binarySource")?,
            source_uri: string_member(&o, "sourceUri")?,
            source_revision: string_member(&o, "sourceRevision")?,
            source_sha256: string_member(&o, "sourceSha256")?,
            cuda_enabled: bool_member(&o, "cudaEnabled")?,
            cuda_architectures: string_member(&o, "cudaArchitectures")?,
            runtime_build: string_member(&o, "runtimeBuild")?,
        })
    }
}

/// `InstallLlamaServerModelArgs`.
#[derive(Clone, Debug, Default)]
pub struct InstallLlamaArgs {
    pub model_ref: String,
    pub artifact_path: String,
    pub artifact_sha256: String,
    pub artifact_uri: String,
    pub base_model: String,
    pub revision: String,
    pub tokenizer_revision: String,
    pub chat_template_hash: String,
    pub chat_template: String,
    pub chat_template_kwargs: String,
    pub quantization: String,
    pub context_size: i64,
    pub gpu_layers: i64,
    pub binary_path: String,
    pub binary_version: String,
    pub binary_sha256: String,
    pub binary_source: String,
    pub source_revision: String,
    pub source_sha256: String,
    pub cuda_enabled: bool,
    pub cuda_architectures: String,
    pub binary_build_source_uri: String,
    pub binary_build_source_sha256: String,
    pub binary_build_revision: String,
    pub binary_uri: String,
    pub port: Option<i64>,
}

/// `BuildLlamaServerBinaryArgs`.
#[derive(Clone, Debug, Default)]
pub struct BuildArgs {
    pub source_uri: String,
    pub source_sha256: String,
    pub source_revision: String,
    pub output_path: String,
    pub cuda_architectures: String,
}

/// `LlamaServerBinaryBuildResult`.
#[derive(Clone, Debug, Default)]
pub struct BuildResult {
    pub binary_path: String,
    pub binary_version: String,
    pub binary_sha256: String,
    pub binary_source: String,
    pub source_uri: String,
    pub source_sha256: String,
    pub source_revision: String,
    pub cuda_enabled: bool,
    pub cuda_architectures: String,
    pub runtime_build: String,
}

impl BuildResult {
    pub fn to_json(&self) -> J {
        Fields::new()
            .text("runtime", "llama-cpp", false)
            .text("binaryPath", &self.binary_path, false)
            .text("binaryVersion", &self.binary_version, false)
            .text("binarySha256", &self.binary_sha256, false)
            .text("binarySource", &self.binary_source, false)
            .text("sourceUri", &self.source_uri, false)
            .text("sourceSha256", &self.source_sha256, false)
            .text("sourceRevision", &self.source_revision, false)
            .flag("cudaEnabled", self.cuda_enabled, false)
            .text("cudaArchitectures", &self.cuda_architectures, true)
            .text("buildToolchain", "cmake+ninja+cuda", false)
            .text("runtimeBuild", &self.runtime_build, false)
            .done()
    }
}

/// `isSHA256`.
fn is_sha256(value: &str) -> bool {
    let v = value.trim();
    v.len() == 64 && hex::decode(v).is_ok()
}

/// `filepath.Base`.
fn base_name(path: &str) -> String {
    if path.is_empty() {
        return ".".into();
    }
    let trimmed = path.trim_end_matches('/');
    if trimmed.is_empty() {
        return "/".into();
    }
    trimmed.rsplit('/').next().unwrap_or(trimmed).to_string()
}

/// `llamaModelNameMatches`.
fn llama_model_name_matches(name: &str, model_ref: &str) -> bool {
    let normalize = |value: &str| {
        let value = value.trim();
        let value = value.strip_suffix(":latest").unwrap_or(value);
        let base = base_name(value);
        base.strip_suffix(".gguf")
            .unwrap_or(&base)
            .trim()
            .to_string()
    };
    let (left, right) = (normalize(name), normalize(model_ref));
    !left.is_empty()
        && !right.is_empty()
        && (left == right
            || left.ends_with(&format!("/{right}"))
            || right.ends_with(&format!("/{left}")))
}

/// `llamaSystemdQuote`.
fn llama_systemd_quote(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\\\""))
}

/// `renderLlamaSystemdUnit`.
fn render_llama_unit(cfg: &LlamaConfig) -> String {
    let alias = format!(" --alias {}", llama_systemd_quote(&cfg.model_ref));
    let template = if cfg.chat_template.trim().is_empty() {
        String::new()
    } else {
        format!(
            " --chat-template {}",
            llama_systemd_quote(&cfg.chat_template)
        )
    };
    let kwargs = format!(
        " --chat-template-kwargs {}",
        llama_systemd_quote(&cfg.chat_template_kwargs)
    );
    format!(
        "[Unit]
Description=Opute llama-server runtime
After=network.target
StartLimitIntervalSec=60s
StartLimitBurst=5

[Service]
Type=simple
Environment=CUDA_VISIBLE_DEVICES=0
ExecStart={} --model {}{alias} --host 127.0.0.1 --port {} --jinja{template}{kwargs} --reasoning-budget 0 --ctx-size {} --n-gpu-layers {} --temp 0.1 --top-p 0.9 --seed 42
Slice=opute-workload.slice
KillMode=control-group
MemoryHigh=10G
MemoryMax=11G
MemorySwapMax=1G
CPUQuota=600%
CPUWeight=100
TasksMax=4096
Restart=on-failure
RestartSec=3

[Install]
WantedBy=default.target
",
        cfg.binary_path, cfg.artifact_path, cfg.port, cfg.context_size, cfg.gpu_layers
    )
}

/// `llamaSourceRevisionPattern`: `^[A-Za-z0-9._-]{4,128}$`.
fn llama_revision_ok(value: &str) -> bool {
    (4..=128).contains(&value.len())
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
}

/// `verifyLlamaArtifact`.
fn verify_llama_artifact(path: &str, expected: &str) -> Result<(), String> {
    let path = path.trim();
    if path.is_empty() {
        return Err("llama-server GGUF artifact path is required".into());
    }
    let info = stat(path).map_err(|e| format!("llama-server GGUF artifact is unavailable: {e}"))?;
    if info.is_dir() {
        return Err(format!("llama-server artifact must be a GGUF file: {path}"));
    }
    if expected.trim().is_empty() {
        return Err("llama-server artifact SHA-256 is required".into());
    }
    let actual = file_sha256(path).map_err(|e| format!("hash llama-server artifact: {e}"))?;
    if !actual.eq_ignore_ascii_case(expected.trim()) {
        return Err(format!(
            "llama-server artifact checksum mismatch: expected {expected}, got {actual}"
        ));
    }
    Ok(())
}

/// `downloadVerifiedLlamaFile`.
fn download_verified_llama_file(
    uri: &str,
    destination: &str,
    expected: &str,
    executable: bool,
) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    let uri = uri.trim();
    if !uri.starts_with("https://") {
        return Err("llama-server artifact URI must use HTTPS".into());
    }
    if expected.trim().is_empty() {
        return Err("SHA-256 is required when adopting a downloaded llama-server artifact".into());
    }
    let reply = http_request(
        "GET",
        uri,
        &[],
        None,
        Duration::from_secs(20 * 60),
        UNLIMITED,
    )
    .map_err(|e| format!("download llama-server artifact: {e}"))?;
    if !ok_status(reply.status) {
        return Err(format!(
            "download llama-server artifact: HTTP {}",
            reply.status
        ));
    }
    let dir = dir_of(destination);
    mkdir_all(&dir, 0o700)?;
    let (mut file, temporary) = crate::goerr::create_temp(&dir, ".llama-download-", "")?;
    let result = (|| {
        file.write_all(&reply.body)
            .map_err(|e| crate::goerr::path_error("write", Path::new(&temporary), &e).0)?;
        drop(file);
        let actual = hex::encode(Sha256::digest(&reply.body));
        if !actual.eq_ignore_ascii_case(expected.trim()) {
            return Err(format!(
                "downloaded llama-server artifact checksum mismatch: expected {expected}, got {actual}"
            ));
        }
        let chmod = |mode: u32| {
            std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(mode))
                .map_err(|e| crate::goerr::path_error("chmod", Path::new(&temporary), &e).0)
        };
        chmod(0o600)?;
        if executable {
            chmod(0o700)?;
        }
        std::fs::rename(&temporary, destination).map_err(|e| {
            format!(
                "rename {temporary} {destination}: {}",
                crate::goerr::errno_text(&e)
            )
        })
    })();
    let _ = std::fs::remove_file(&temporary);
    result
}

/// `json.Unmarshal(data, &map[string]any{})`'s error text.
fn unmarshal_map_error(data: &str) -> Option<String> {
    if let Some(err) = crate::gojson::syntax_error(data.as_bytes()) {
        return Some(err);
    }
    match serde_json::from_str::<J>(data) {
        Ok(J::Object(_)) | Ok(J::Null) => None,
        Ok(other) => Some(format!(
            "json: cannot unmarshal {} into Go value of type map[string]interface {{}}",
            json_kind(&other)
        )),
        Err(e) => Some(e.to_string()),
    }
}

impl Llm {
    /// `llamaServerConfigPath`.
    fn llama_config_path(&self) -> Result<String, String> {
        let home = self
            .home()
            .map_err(|e| format!("resolve home directory: {e}"))?;
        Ok(format!("{home}/.config/opute/llama-server.json"))
    }

    /// `defaultLlamaServerConfig`.
    fn default_llama_config(&self) -> LlamaConfig {
        let mut port = DEFAULT_LLAMA_SERVER_PORT;
        let raw = self.env.get("OPUTE_LLAMA_SERVER_PORT");
        if !raw.trim().is_empty() {
            if let Some(parsed) = atoi(raw.trim()) {
                if parsed > 0 && parsed < 65536 {
                    port = parsed;
                }
            }
        }
        let mut binary_path = self.env.get("OPUTE_LLAMA_SERVER_BINARY").trim().to_string();
        if binary_path.is_empty() {
            // `filepath.Join(home, ...)`, with home empty when unknown.
            let home = self.env.get("HOME");
            binary_path = clean_any(&format!(
                "{}{}.local/share/opute/llama-server/llama-server",
                home,
                if home.is_empty() { "" } else { "/" }
            ));
        }
        let get = |name: &str| self.env.get(name).trim().to_string();
        LlamaConfig {
            port,
            model_ref: get("OPUTE_LLAMA_SERVER_MODEL"),
            artifact_path: get("OPUTE_LLAMA_SERVER_ARTIFACT_PATH"),
            base_model: get("OPUTE_LLAMA_SERVER_BASE_MODEL"),
            chat_template: DEFAULT_LLAMA_TEMPLATE.into(),
            chat_template_kwargs: DEFAULT_LLAMA_TEMPLATE_KWARGS.into(),
            context_size: DEFAULT_LLAMA_CONTEXT,
            gpu_layers: DEFAULT_LLAMA_GPU_LAYERS,
            binary_path,
            binary_version: get("OPUTE_LLAMA_SERVER_VERSION"),
            runtime_build: get("OPUTE_LLAMA_SERVER_BUILD"),
            ..Default::default()
        }
    }

    /// `loadLlamaServerConfig`.
    fn load_llama_config(&self) -> LlamaConfig {
        let mut cfg = self.default_llama_config();
        let Ok(path) = self.llama_config_path() else {
            return cfg;
        };
        let Ok(data) = std::fs::read(&path) else {
            return cfg;
        };
        let Ok(p) = LlamaConfig::unmarshal(&data) else {
            return cfg;
        };
        let set = |field: &mut String, value: &str| {
            if !value.trim().is_empty() {
                *field = value.trim().to_string();
            }
        };
        if p.port > 0 {
            cfg.port = p.port;
        }
        set(&mut cfg.model_ref, &p.model_ref);
        set(&mut cfg.artifact_path, &p.artifact_path);
        set(&mut cfg.artifact_uri, &p.artifact_uri);
        set(&mut cfg.artifact_sha256, &p.artifact_sha256);
        set(&mut cfg.base_model, &p.base_model);
        set(&mut cfg.revision, &p.revision);
        set(&mut cfg.tokenizer_revision, &p.tokenizer_revision);
        set(&mut cfg.chat_template_hash, &p.chat_template_hash);
        set(&mut cfg.chat_template, &p.chat_template);
        set(&mut cfg.chat_template_kwargs, &p.chat_template_kwargs);
        set(&mut cfg.quantization, &p.quantization);
        if p.context_size > 0 {
            cfg.context_size = p.context_size;
        }
        if p.gpu_layers > 0 {
            cfg.gpu_layers = p.gpu_layers;
        }
        set(&mut cfg.binary_path, &p.binary_path);
        set(&mut cfg.binary_version, &p.binary_version);
        set(&mut cfg.binary_sha256, &p.binary_sha256);
        set(&mut cfg.binary_source, &p.binary_source);
        set(&mut cfg.source_revision, &p.source_revision);
        set(&mut cfg.source_uri, &p.source_uri);
        set(&mut cfg.source_sha256, &p.source_sha256);
        if p.cuda_enabled {
            cfg.cuda_enabled = true;
        }
        set(&mut cfg.cuda_architectures, &p.cuda_architectures);
        set(&mut cfg.runtime_build, &p.runtime_build);
        cfg
    }

    /// `saveLlamaServerConfig`.
    fn save_llama_config(&self, cfg: &LlamaConfig) -> Result<(), String> {
        let path = self.llama_config_path()?;
        mkdir_all(&dir_of(&path), 0o700)?;
        write_file(
            &path,
            format!("{}\n", cfg.marshal_indent()).as_bytes(),
            0o600,
        )
    }

    /// `validateLlamaServerConfig`.
    fn validate_llama_config(cfg: &LlamaConfig) -> Result<(), String> {
        if cfg.port < 1 || cfg.port > 65535 {
            return Err(format!("invalid llama-server port {}", cfg.port));
        }
        if cfg.model_ref.trim().is_empty() {
            return Err("llama-server modelRef is required".into());
        }
        for (name, value) in [
            ("baseModel", &cfg.base_model),
            ("revision", &cfg.revision),
            ("tokenizerRevision", &cfg.tokenizer_revision),
            ("chatTemplateHash", &cfg.chat_template_hash),
            ("binaryVersion", &cfg.binary_version),
            ("binarySha256", &cfg.binary_sha256),
            ("binarySource", &cfg.binary_source),
            ("sourceRevision", &cfg.source_revision),
            ("sourceSha256", &cfg.source_sha256),
            ("chatTemplateKwargs", &cfg.chat_template_kwargs),
        ] {
            if value.trim().is_empty() {
                return Err(format!(
                    "llama-server {name} is required for a pinned serving manifest"
                ));
            }
        }
        if !is_sha256(&cfg.chat_template_hash) || !is_sha256(&cfg.binary_sha256) {
            return Err(
                "llama-server chat template and binary checksums must be SHA-256 hex digests"
                    .into(),
            );
        }
        if let Some(err) = unmarshal_map_error(&cfg.chat_template_kwargs) {
            return Err(format!(
                "llama-server chatTemplateKwargs must be JSON: {err}"
            ));
        }
        let thinking = match serde_json::from_str::<J>(&cfg.chat_template_kwargs) {
            Ok(J::Object(o)) => o.get("enable_thinking").and_then(J::as_bool),
            _ => None,
        };
        if thinking != Some(false) {
            return Err(
                "llama-server Qwen3.5 serving requires chatTemplateKwargs.enable_thinking=false"
                    .into(),
            );
        }
        verify_llama_artifact(&cfg.artifact_path, &cfg.artifact_sha256)?;
        if cfg.context_size < 512 {
            return Err("llama-server context size must be at least 512".into());
        }
        if cfg.gpu_layers != DEFAULT_LLAMA_GPU_LAYERS {
            return Err(format!(
                "llama-server GPU-only execution requires --n-gpu-layers {DEFAULT_LLAMA_GPU_LAYERS}; got {}",
                cfg.gpu_layers
            ));
        }
        if cfg.binary_source != "host-build" {
            return Err("llama-server production binary must come from the host CUDA build".into());
        }
        if !cfg.cuda_enabled {
            return Err("llama-server binary is not verified as CUDA-enabled".into());
        }
        if cfg.quantization != "Q4_K_M" {
            return Err(format!(
                "llama-server production quantization must be Q4_K_M (NF4 QLoRA-matched), got {}",
                quote(&cfg.quantization)
            ));
        }
        stat(&cfg.binary_path).map_err(|e| format!("llama-server binary is unavailable: {e}"))?;
        let actual =
            file_sha256(&cfg.binary_path).map_err(|e| format!("hash llama-server binary: {e}"))?;
        if !actual.eq_ignore_ascii_case(&cfg.binary_sha256) {
            return Err("llama-server binary checksum mismatch".into());
        }
        Ok(())
    }

    /// `startLlamaServerRuntime`.
    fn start_llama_runtime_with(&self, ctx: &RunCtx, cfg: &LlamaConfig) -> Result<(), String> {
        Self::validate_llama_config(cfg)?;
        self.stop_llama_runtime()
            .map_err(|e| format!("unload current llama-server generation model: {e}"))?;
        let check = self
            .host_ctx(ctx, &[nvidia_smi()], Duration::from_secs(10))
            .map_err(|e| format!("llama-server GPU verification failed: {e}"))?;
        if check.exit_code != 0 {
            return Err(format!(
                "llama-server GPU verification failed: {}",
                first_non_empty(&[&check.stderr, &check.stdout]).trim()
            ));
        }
        self.save_llama_config(cfg)?;
        let released = self.release_recognized_llama_port(ctx, cfg)?;
        let home = self.home()?;
        let unit_path = format!("{home}/.config/systemd/user/{LLAMA_UNIT}");
        mkdir_all(&dir_of(&unit_path), 0o700)?;
        let unit = render_llama_unit(cfg);
        let changed =
            std::fs::read(&unit_path).map_or(true, |previous| previous != unit.as_bytes());
        write_file(&unit_path, unit.as_bytes(), 0o600)?;
        let mut commands: Vec<Vec<&str>> = vec![vec!["daemon-reload"]];
        if changed || released {
            commands.push(vec!["enable", LLAMA_UNIT]);
            commands.push(vec!["restart", LLAMA_UNIT]);
        } else {
            commands.push(vec!["enable", "--now", LLAMA_UNIT]);
        }
        for args in commands {
            let (output, result) = self.systemctl_user(&args);
            if let Err(err) = result {
                return Err(format!(
                    "llama-server systemd operation failed: {err}: {}",
                    output.trim()
                ));
            }
        }
        let deadline = Instant::now() + Duration::from_secs(90);
        while Instant::now() < deadline {
            if llama_ready(cfg) {
                return Ok(());
            }
            if let Some(err) = ctx.err() {
                return Err(err);
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        Err(format!(
            "llama-server did not become ready on 127.0.0.1:{} within 90s",
            cfg.port
        ))
    }

    /// `releaseRecognizedLlamaPort`: only a recognizable Opute local-LLM
    /// process may be stopped; any other owner is a hard failure.
    fn release_recognized_llama_port(
        &self,
        ctx: &RunCtx,
        cfg: &LlamaConfig,
    ) -> Result<bool, String> {
        let command = format!(
            "set -o pipefail
ss -ltnp '( sport = :{} )' 2>/dev/null | sed -n 's/.*pid=\\([0-9][0-9]*\\).*/\\1/p' | sort -u | while read -r pid; do
	  [ -n \"$pid\" ] || continue
	  printf 'PID=%s|' \"$pid\"
	  tr '\\0' ' ' < \"/proc/$pid/cmdline\" 2>/dev/null || true
	  printf '\\n'
done",
            cfg.port
        );
        let result = self
            .host_ctx(
                ctx,
                &["bash".into(), "-lc".into(), command],
                Duration::from_secs(10),
            )
            .map_err(|e| format!("inspect llama-server port ownership: {e}"))?;
        let mut released = false;
        for line in result.stdout.trim().split('\n') {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Some((pid, cmdline)) = line.split_once('|') else {
                return Err(format!(
                    "unable to identify llama-server port owner: {line}"
                ));
            };
            let pid = pid.strip_prefix("PID=").unwrap_or(pid);
            let cmdline = cmdline.trim();
            let managed = cmdline.contains(cfg.binary_path.trim())
                || (cmdline.contains("transformers serve") && cmdline.contains("/home/opute/"));
            if !managed {
                return Err(format!(
                    "llama-server port {} is occupied by an unowned process pid={pid} cmd={cmdline}",
                    cfg.port
                ));
            }
            let kill = format!("kill -TERM {} 2>/dev/null || true", shell_quote(pid));
            self.host_ctx(
                ctx,
                &["bash".into(), "-lc".into(), kill],
                Duration::from_secs(5),
            )
            .map_err(|e| format!("stop stale Opute local-LLM process pid={pid}: {e}"))?;
            released = true;
        }
        if result.stdout.trim().is_empty() {
            return Ok(false);
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            let check = self.host_ctx(
                ctx,
                &[
                    "bash".into(),
                    "-lc".into(),
                    format!(
                        "ss -ltn '( sport = :{} )' 2>/dev/null | tail -n +2",
                        cfg.port
                    ),
                ],
                Duration::from_secs(5),
            );
            if let Ok(check) = check {
                if check.stdout.trim().is_empty() {
                    return Ok(released);
                }
            }
            if let Some(err) = ctx.err() {
                return Err(err);
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        Err(format!(
            "stale Opute local-LLM process still owns port {} after termination request",
            cfg.port
        ))
    }

    /// `InstallLlamaServerModel`.
    pub fn install_llama_model(
        &self,
        ctx: &RunCtx,
        args: &InstallLlamaArgs,
    ) -> Result<ProbeResult, String> {
        let fail = |e: String| e;
        let mut cfg = self.load_llama_config();
        cfg.model_ref = args.model_ref.trim().into();
        cfg.artifact_path = args.artifact_path.trim().into();
        cfg.artifact_uri = args.artifact_uri.trim().into();
        cfg.artifact_sha256 = args.artifact_sha256.trim().into();
        cfg.base_model = args.base_model.trim().into();
        cfg.revision = args.revision.trim().into();
        cfg.tokenizer_revision = args.tokenizer_revision.trim().into();
        cfg.chat_template_hash = args.chat_template_hash.trim().into();
        cfg.chat_template = args.chat_template.trim().into();
        cfg.chat_template_kwargs = args.chat_template_kwargs.trim().into();
        cfg.quantization = args.quantization.trim().into();
        if cfg.chat_template_kwargs.is_empty() {
            cfg.chat_template_kwargs = DEFAULT_LLAMA_TEMPLATE_KWARGS.into();
        }
        if cfg.context_size == 0 {
            cfg.context_size = DEFAULT_LLAMA_CONTEXT;
        }
        if args.context_size > 0 {
            cfg.context_size = args.context_size;
        }
        if args.gpu_layers != 0 {
            cfg.gpu_layers = args.gpu_layers;
        }
        if let Some(port) = args.port {
            cfg.port = port;
        }
        let set = |field: &mut String, value: &str| {
            if !value.trim().is_empty() {
                *field = value.trim().to_string();
            }
        };
        set(&mut cfg.binary_path, &args.binary_path);
        set(&mut cfg.binary_version, &args.binary_version);
        set(&mut cfg.binary_sha256, &args.binary_sha256);
        set(&mut cfg.binary_source, &args.binary_source);
        set(&mut cfg.source_uri, &args.binary_build_source_uri);
        set(&mut cfg.source_sha256, &args.binary_build_source_sha256);
        set(&mut cfg.source_revision, &args.binary_build_revision);
        set(&mut cfg.source_revision, &args.source_revision);
        set(&mut cfg.source_sha256, &args.source_sha256);
        if args.cuda_enabled {
            cfg.cuda_enabled = true;
        }
        set(&mut cfg.cuda_architectures, &args.cuda_architectures);
        if !args.binary_uri.trim().is_empty() {
            return Err(fail(
                "binaryUri is not supported; build llama-server through ensure_local_llm_server_binary"
                    .into(),
            ));
        }
        if cfg.binary_source == "host-build" {
            let usable = std::fs::metadata(&cfg.binary_path).is_ok_and(|m| !m.is_dir());
            if !usable {
                if cfg.source_uri.is_empty()
                    || cfg.source_sha256.is_empty()
                    || cfg.source_revision.is_empty()
                {
                    return Err(fail(
                        "host-built llama-server is missing source URI, checksum, or revision"
                            .into(),
                    ));
                }
                let built = self
                    .ensure_llama_binary(
                        ctx,
                        &BuildArgs {
                            source_uri: cfg.source_uri.clone(),
                            source_sha256: cfg.source_sha256.clone(),
                            source_revision: cfg.source_revision.clone(),
                            output_path: cfg.binary_path.clone(),
                            cuda_architectures: cfg.cuda_architectures.clone(),
                        },
                    )
                    .map_err(fail)?;
                cfg.binary_path = built.binary_path;
                cfg.binary_version = built.binary_version;
                cfg.binary_sha256 = built.binary_sha256;
                cfg.binary_source = built.binary_source;
                cfg.source_revision = built.source_revision;
                cfg.source_sha256 = built.source_sha256;
                cfg.cuda_enabled = built.cuda_enabled;
                cfg.cuda_architectures = built.cuda_architectures;
                cfg.runtime_build = built.runtime_build;
            }
        }
        if cfg.artifact_path.is_empty() && !args.artifact_uri.trim().is_empty() {
            let home = self.home().map_err(fail)?;
            cfg.artifact_path = clean_any(&format!(
                "{home}/.local/share/opute/llama-models/{}.gguf",
                cfg.model_ref
            ));
        }
        if cfg.artifact_path.is_empty() {
            return Err(fail(
                "llama-server artifactPath or HTTPS artifactURI is required".into(),
            ));
        }
        if not_exist(&cfg.artifact_path) && !args.artifact_uri.trim().is_empty() {
            download_verified_llama_file(
                &args.artifact_uri,
                &cfg.artifact_path,
                &cfg.artifact_sha256,
                false,
            )
            .map_err(fail)?;
        }
        if not_exist(&cfg.binary_path) && !args.binary_uri.trim().is_empty() {
            download_verified_llama_file(
                &args.binary_uri,
                &cfg.binary_path,
                &cfg.binary_sha256,
                true,
            )
            .map_err(fail)?;
        }
        if cfg.binary_source == "host-build" && !cfg.cuda_enabled {
            stat(&cfg.binary_path)
                .map_err(|e| fail(format!("inspect managed llama-server binary: {e}")))?;
            let cuda = self
                .verify_llama_cuda_linkage(ctx, &cfg.binary_path)
                .map_err(fail)?;
            if !cuda {
                return Err(fail(
                    "managed llama-server binary is not verified as CUDA-enabled".into(),
                ));
            }
            cfg.cuda_enabled = true;
        }
        self.start_llama_runtime_with(ctx, &cfg).map_err(fail)?;
        let result = self
            .probe_llama(
                ctx,
                &ProbeArgs {
                    include_chat: true,
                    model_ref: cfg.model_ref.clone(),
                },
            )
            .map_err(fail)?;
        if !result.chat_ready || !result.gpu_accelerated || result.size_vram_bytes <= 0 {
            return Err(
                "llama-server must be GPU-resident with nonzero VRAM; CPU fallback is rejected"
                    .into(),
            );
        }
        Ok(result)
    }

    /// `StartLlamaServerRuntime`.
    pub fn start_llama_runtime(&self, ctx: &RunCtx) -> Result<ProbeResult, String> {
        let cfg = self.load_llama_config();
        self.start_llama_runtime_with(ctx, &cfg)?;
        let result = self.probe_llama(
            ctx,
            &ProbeArgs {
                include_chat: true,
                model_ref: cfg.model_ref.clone(),
            },
        )?;
        if !result.chat_ready || !result.gpu_accelerated || result.size_vram_bytes <= 0 {
            return Err(
                "llama-server must be GPU-resident with nonzero VRAM; CPU fallback is rejected"
                    .into(),
            );
        }
        Ok(result)
    }

    /// `StopLlamaServerRuntime`: a unit that does not exist is stopped.
    pub fn stop_llama_runtime(&self) -> Result<(), String> {
        let (output, result) = self.systemctl_user(&["disable", "--now", LLAMA_UNIT]);
        if let Err(err) = result {
            let message = output.trim();
            let lower = message.to_lowercase();
            if lower.contains("not loaded")
                || lower.contains("not found")
                || lower.contains("does not exist")
            {
                return Ok(());
            }
            return Err(format!("stop llama-server systemd unit: {err}: {message}"));
        }
        Ok(())
    }

    /// `ProbeLlamaServer`.
    pub fn probe_llama(&self, _ctx: &RunCtx, args: &ProbeArgs) -> Result<ProbeResult, String> {
        let mut cfg = self.load_llama_config();
        if !args.model_ref.trim().is_empty() {
            cfg.model_ref = args.model_ref.trim().to_string();
        }
        let mut result = ProbeResult {
            api_base_url: format!("http://127.0.0.1:{}/v1", cfg.port),
            model_ref: cfg.model_ref.clone(),
            artifact_uri: cfg.artifact_uri.clone(),
            artifact_sha256: cfg.artifact_sha256.clone(),
            base_model: cfg.base_model.clone(),
            revision: cfg.revision.clone(),
            tokenizer_revision: cfg.tokenizer_revision.clone(),
            chat_template_hash: cfg.chat_template_hash.clone(),
            chat_template_kwargs: cfg.chat_template_kwargs.clone(),
            quantization: cfg.quantization.clone(),
            gpu_layers: cfg.gpu_layers,
            cuda_architectures: cfg.cuda_architectures.clone(),
            runtime_build: cfg.runtime_build.clone(),
            version: cfg.binary_version.clone(),
            cuda_enabled: cfg.cuda_enabled,
            binary_source: cfg.binary_source.clone(),
            binary_sha256: cfg.binary_sha256.clone(),
            source_revision: cfg.source_revision.clone(),
            context_length: cfg.context_size,
            ..Default::default()
        };
        let reply = match http_request(
            "GET",
            &format!("http://127.0.0.1:{}/v1/models", cfg.port),
            &[],
            None,
            Duration::from_secs(15),
            UNLIMITED,
        ) {
            Ok(reply) => reply,
            Err(err) => {
                result.load_error = err;
                return Ok(result);
            }
        };
        let data = match decode_first(&reply.body).and_then(|v| {
            slice_member(&v, "data")
                .map(|d| d.into_iter().map(|m| text_of(m, "id")).collect::<Vec<_>>())
        }) {
            Ok(data) => data,
            Err(err) => {
                result.load_error = err;
                return Ok(result);
            }
        };
        for id in &data {
            result.models.push(ModelResult {
                name: id.clone(),
                ..Default::default()
            });
        }
        let ok = ok_status(reply.status);
        result.ready = ok;
        result.open_ai_models_ready = ok;
        for id in &data {
            if cfg.model_ref.is_empty() || llama_model_name_matches(id, &cfg.model_ref) {
                result.loaded_model = id.clone();
                break;
            }
        }
        if result.ready && !result.loaded_model.is_empty() {
            let argv = vec![
                nvidia_smi(),
                "--query-gpu=memory.used".into(),
                "--format=csv,noheader,nounits".into(),
            ];
            if let Some(stdout) = output(&self.env, &argv) {
                let first = stdout.split('\n').next().unwrap_or("").trim().to_string();
                if let Some(value) = atoi(&first) {
                    result.size_vram_bytes = value.wrapping_mul(1024 * 1024);
                    result.gpu_accelerated = result.size_vram_bytes > 0;
                }
            }
        }
        result.chat_ready = result.ready
            && !result.loaded_model.is_empty()
            && result.gpu_accelerated
            && result.size_vram_bytes > 0;
        Ok(result)
    }

    /// `managedLlamaPath`: absolute, cleaned, and beneath the home.
    fn managed_llama_path(home: &str, requested: &str, default: &str) -> Result<String, String> {
        let mut path = requested.trim().to_string();
        if path.is_empty() {
            path = default.to_string();
        }
        if !path.starts_with('/') {
            return Err("llama-server path must be absolute".into());
        }
        let path = clean_any(&path);
        let home = clean_any(home);
        let inside = home.starts_with('/')
            && (path == home || home == "/" || path.starts_with(&format!("{home}/")));
        if !inside {
            return Err(
                "llama-server path must remain under the host-agent managed home directory".into(),
            );
        }
        Ok(path)
    }

    /// `EnsureLlamaServerBinary`: download, verify, build and verify a CUDA
    /// llama-server, all beneath the managed home.
    pub fn ensure_llama_binary(
        &self,
        ctx: &RunCtx,
        args: &BuildArgs,
    ) -> Result<BuildResult, String> {
        if !args.source_uri.trim().starts_with("https://") {
            return Err("sourceUri must use HTTPS".into());
        }
        if !is_sha256(&args.source_sha256) {
            return Err("sourceSha256 must be a SHA-256 hex digest".into());
        }
        if !llama_revision_ok(args.source_revision.trim()) {
            return Err("sourceRevision contains invalid characters".into());
        }
        let arch = args.cuda_architectures.trim();
        if !arch.is_empty() && !arch.bytes().all(|c| c.is_ascii_digit() || c == b';') {
            return Err(
                "cudaArchitectures must contain only numeric architectures separated by semicolons"
                    .into(),
            );
        }
        let home = self.home()?;
        let managed_root = format!("{home}/.local/share/opute/llama-server");
        let output_path = Self::managed_llama_path(
            &home,
            &args.output_path,
            &format!("{managed_root}/llama-server"),
        )?;
        if let Some(adopted) = self.adopt_existing_llama_binary(&output_path, args) {
            return Ok(adopted);
        }
        let cache_root = clean_any(&format!(
            "{home}/.cache/opute/llama-server/{}",
            args.source_revision.trim()
        ));
        let archive = format!("{cache_root}/llama.cpp.tar.gz");
        let source = format!("{cache_root}/source");
        let build = format!("{cache_root}/build");
        for path in [&archive, &source, &build, &output_path] {
            Self::managed_llama_path(&home, path, path)?;
        }
        let mut missing = Vec::new();
        for tool in ["cmake", "ninja", "nvcc"] {
            let check = self.host_ctx(
                ctx,
                &[
                    "bash".into(),
                    "-lc".into(),
                    format!("command -v {}", shell_quote(tool)),
                ],
                Duration::from_secs(5),
            );
            let present = check.is_ok_and(|c| c.exit_code == 0 && !c.stdout.trim().is_empty());
            if !present {
                missing.push(tool);
            }
        }
        if !missing.is_empty() {
            return Err(format!(
                "CUDA llama-server build toolchain is incomplete; install or provision: {}",
                missing.join(", ")
            ));
        }
        match std::fs::metadata(&archive) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                return Err(format!(
                    "inspect cached llama.cpp source archive: {}",
                    crate::goerr::path_error("stat", Path::new(&archive), &e).0
                ))
            }
            Err(_) => download_verified_llama_file(
                &args.source_uri,
                &archive,
                &args.source_sha256,
                false,
            )?,
            Ok(_) => verify_llama_artifact(&archive, &args.source_sha256)
                .map_err(|e| format!("cached llama.cpp source archive verification failed: {e}"))?,
        }
        let arch_flag = if arch.is_empty() {
            String::new()
        } else {
            format!(" -DCMAKE_CUDA_ARCHITECTURES={arch}")
        };
        let q = |s: &str| shell_quote(s);
        let script = format!(
            "set -euo pipefail
# Keep the revision-scoped CMake cache and objects across MCP reconnects. A
# caller timeout must not turn a resumable host build into another full CUDA
# compilation; the verified archive and revision are the cache identity.
if [ ! -f {build}/CMakeCache.txt ]; then
  rm -rf {source} {build}
  mkdir -p {source} {build}
  tar -xzf {archive} -C {source} --strip-components=1
  cmake -S {source} -B {build} -G Ninja -DCMAKE_BUILD_TYPE=Release -DBUILD_SHARED_LIBS=OFF -DGGML_CUDA=ON -DLLAMA_CURL=OFF{arch_flag}
fi
cmake --build {build} --target llama-server --parallel 4
test -x {build}/bin/llama-server
mkdir -p {outdir}
install -m 0755 {build}/bin/llama-server {output}
",
            build = q(&build),
            source = q(&source),
            archive = q(&archive),
            outdir = q(&dir_of(&output_path)),
            output = q(&output_path),
        );
        let built = crate::hostexec::run_host_workload(
            &self.env,
            &format!("llama-server-build-{output_path}"),
            &["bash".into(), "-lc".into(), script],
            Duration::from_secs(45 * 60),
            Some(ctx),
        )
        .map_err(|e| format!("build CUDA llama-server: {e}"))?;
        if built.exit_code != 0 {
            let mut message = first_non_empty(&[&built.stderr, &built.stdout])
                .trim()
                .to_string();
            if message.is_empty() {
                message = format!("exit code {}", built.exit_code);
            }
            return Err(format!("build CUDA llama-server: {message}"));
        }
        let version = self
            .host_ctx(
                ctx,
                &[output_path.clone(), "--version".into()],
                Duration::from_secs(30),
            )
            .map_err(|e| format!("verify llama-server binary version: {e}"))?;
        if version.exit_code != 0 {
            let mut message = first_non_empty(&[&version.stderr, &version.stdout])
                .trim()
                .to_string();
            if message.is_empty() {
                message = format!("exit code {}", version.exit_code);
            }
            return Err(format!("verify llama-server binary version: {message}"));
        }
        let version = first_non_empty(&[&version.stdout, &version.stderr])
            .trim()
            .to_string();
        if version.is_empty() {
            return Err("llama-server binary returned an empty version".into());
        }
        if !self.verify_llama_cuda_linkage(ctx, &output_path)? {
            return Err("built llama-server does not contain verifiable CUDA linkage".into());
        }
        let sha = file_sha256(&output_path)
            .map_err(|e| format!("hash built llama-server binary: {e}"))?;
        Ok(BuildResult {
            binary_path: output_path,
            binary_version: version,
            binary_sha256: sha,
            binary_source: "host-build".into(),
            source_uri: args.source_uri.trim().into(),
            source_sha256: args.source_sha256.trim().to_lowercase(),
            source_revision: args.source_revision.trim().into(),
            cuda_enabled: true,
            cuda_architectures: arch.into(),
            runtime_build: format!("llama.cpp:{}:cuda", args.source_revision.trim()),
        })
    }

    /// `verifyLlamaServerCudaLinkage`.
    fn verify_llama_cuda_linkage(&self, ctx: &RunCtx, path: &str) -> Result<bool, String> {
        let script = format!(
            "set -o pipefail; {{ ldd {p} 2>/dev/null || true; strings {p} 2>/dev/null || true; }}",
            p = shell_quote(path)
        );
        let res = self
            .host_ctx(
                ctx,
                &["bash".into(), "-lc".into(), script],
                Duration::from_secs(30),
            )
            .map_err(|e| format!("inspect llama-server CUDA linkage: {e}"))?;
        let linkage = format!("{}\n{}", res.stdout, res.stderr).to_lowercase();
        Ok(linkage.contains("ggml-cuda")
            || linkage.contains("cudart")
            || linkage.contains("cublas"))
    }

    /// `adoptExistingLlamaServerBinary`: the persisted manifest already
    /// names a verified build of this revision at this path.
    fn adopt_existing_llama_binary(
        &self,
        output_path: &str,
        args: &BuildArgs,
    ) -> Option<BuildResult> {
        let cfg = self.load_llama_config();
        if cfg.binary_source != "host-build"
            || clean_any(&cfg.binary_path) != clean_any(output_path)
        {
            return None;
        }
        let arch = args.cuda_architectures.trim();
        if cfg.source_revision != args.source_revision.trim()
            || cfg.source_sha256 != args.source_sha256.trim().to_lowercase()
            || (!arch.is_empty() && cfg.cuda_architectures != arch)
        {
            return None;
        }
        if cfg.binary_version.is_empty() || !cfg.cuda_enabled || cfg.binary_sha256.is_empty() {
            return None;
        }
        std::fs::metadata(output_path).ok()?;
        let actual = file_sha256(output_path).ok()?;
        if !actual.eq_ignore_ascii_case(&cfg.binary_sha256) {
            return None;
        }
        Some(BuildResult {
            binary_path: output_path.into(),
            binary_version: cfg.binary_version,
            binary_sha256: cfg.binary_sha256,
            binary_source: cfg.binary_source,
            source_uri: cfg.source_uri,
            source_sha256: cfg.source_sha256,
            source_revision: cfg.source_revision,
            cuda_enabled: cfg.cuda_enabled,
            cuda_architectures: cfg.cuda_architectures,
            runtime_build: cfg.runtime_build,
        })
    }
}

/// `llamaReady`: a configured model is listed on /v1/models.
fn llama_ready(cfg: &LlamaConfig) -> bool {
    let Ok(reply) = http_request(
        "GET",
        &format!("http://127.0.0.1:{}/v1/models", cfg.port),
        &[],
        None,
        Duration::from_secs(3),
        UNLIMITED,
    ) else {
        return false;
    };
    if !ok_status(reply.status) {
        return false;
    }
    let Ok(models) = decode_first(&reply.body).and_then(|v| {
        slice_member(&v, "data")
            .map(|d| d.into_iter().map(|m| text_of(m, "id")).collect::<Vec<_>>())
    }) else {
        return false;
    };
    models
        .iter()
        .any(|id| llama_model_name_matches(id, &cfg.model_ref))
}

/// `exec.Command(...).Output()`: stdout when the command succeeds.
fn output(env: &Env, argv: &[String]) -> Option<String> {
    let program = if argv[0].contains('/') {
        std::path::PathBuf::from(&argv[0])
    } else {
        crate::hostobs::look_path(env, &argv[0])?
    };
    let out = std::process::Command::new(program)
        .args(&argv[1..])
        .env_clear()
        .envs(env.pairs())
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

// --- the local-LLM relay (relay.go, gateway.go, request_lock_unix.go) ---------------------

/// `LocalLLMRelayArgs`. It is persisted as Go marshals it: field names as
/// declared, no tags.
#[derive(Clone, Debug, Default)]
pub struct RelayArgs {
    pub session_id: String,
    pub listen_host: String,
    pub listen_port: i64,
    pub target_host: String,
    pub target_port: i64,
    pub incoming_token: String,
    pub upstream_token: String,
    /// `nil` when the caller sent none.
    pub allowed_source_cidrs: Option<Vec<String>>,
    pub relay_token: String,
    pub allowed_source_ip: String,
}

impl RelayArgs {
    fn cidrs(&self) -> &[String] {
        self.allowed_source_cidrs.as_deref().unwrap_or(&[])
    }

    /// `json.Marshal(args)`.
    fn marshal(&self) -> String {
        let s = |v: &str| {
            let mut o = String::new();
            crate::gojson::encode_string(v, &mut o);
            o
        };
        let cidrs = match &self.allowed_source_cidrs {
            None => "null".to_string(),
            Some(list) => format!(
                "[{}]",
                list.iter().map(|c| s(c)).collect::<Vec<_>>().join(",")
            ),
        };
        format!(
            "{{\"SessionID\":{},\"ListenHost\":{},\"ListenPort\":{},\"TargetHost\":{},\"TargetPort\":{},\"IncomingToken\":{},\"UpstreamToken\":{},\"AllowedSourceCIDRs\":{},\"RelayToken\":{},\"AllowedSourceIP\":{}}}",
            s(&self.session_id),
            s(&self.listen_host),
            self.listen_port,
            s(&self.target_host),
            self.target_port,
            s(&self.incoming_token),
            s(&self.upstream_token),
            cidrs,
            s(&self.relay_token),
            s(&self.allowed_source_ip),
        )
    }

    /// `json.Unmarshal` into `LocalLLMRelayArgs`.
    fn unmarshal(data: &[u8]) -> Result<Self, ()> {
        let o = decode_object(data)?;
        let cidrs = match member(&o, "AllowedSourceCIDRs") {
            None | Some(J::Null) => None,
            Some(J::Array(items)) => Some(
                items
                    .iter()
                    .map(|v| match v {
                        J::String(s) => Ok(s.clone()),
                        J::Null => Ok(String::new()),
                        _ => Err(()),
                    })
                    .collect::<Result<Vec<_>, ()>>()?,
            ),
            Some(_) => return Err(()),
        };
        Ok(RelayArgs {
            session_id: string_member(&o, "SessionID")?,
            listen_host: string_member(&o, "ListenHost")?,
            listen_port: int_member(&o, "ListenPort")?,
            target_host: string_member(&o, "TargetHost")?,
            target_port: int_member(&o, "TargetPort")?,
            incoming_token: string_member(&o, "IncomingToken")?,
            upstream_token: string_member(&o, "UpstreamToken")?,
            allowed_source_cidrs: cidrs,
            relay_token: string_member(&o, "RelayToken")?,
            allowed_source_ip: string_member(&o, "AllowedSourceIP")?,
        })
    }

    /// The incoming credential: the generic field, else the legacy one.
    fn incoming(&self) -> &str {
        if self.incoming_token.is_empty() {
            &self.relay_token
        } else {
            &self.incoming_token
        }
    }
}

/// `net.ParseIP`, with an IPv4-mapped IPv6 address reduced to IPv4 as
/// Go's `To4` does for comparisons.
fn parse_ip(value: &str) -> Option<IpAddr> {
    value.parse::<IpAddr>().ok()
}

fn unmap(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
        v4 => v4,
    }
}

/// `IP.IsUnspecified`.
fn unspecified(ip: IpAddr) -> bool {
    unmap(ip).is_unspecified()
}

/// A parsed `net.IPNet`: the masked network and its prefix.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Cidr {
    network: IpAddr,
    bits: u8,
}

/// `net.ParseCIDR`.
fn parse_cidr(value: &str) -> Option<Cidr> {
    let (addr, mask) = value.split_once('/')?;
    if mask.is_empty() || !mask.bytes().all(|b| b.is_ascii_digit()) || mask.len() > 3 {
        return None;
    }
    let bits: u32 = mask.parse().ok()?;
    let ip = addr.parse::<IpAddr>().ok()?;
    let (ip, bits) = match ip {
        IpAddr::V4(_) if bits <= 32 => (ip, bits),
        IpAddr::V6(v6) if bits <= 128 => match v6.to_ipv4_mapped() {
            Some(v4) => (IpAddr::V4(v4), bits.saturating_sub(96)),
            None => (ip, bits),
        },
        _ => return None,
    };
    Some(Cidr {
        network: mask_ip(ip, bits as u8),
        bits: bits as u8,
    })
}

fn mask_ip(ip: IpAddr, bits: u8) -> IpAddr {
    match ip {
        IpAddr::V4(v4) => {
            let m = if bits == 0 {
                0
            } else {
                u32::MAX << (32 - bits as u32)
            };
            IpAddr::V4((u32::from(v4) & m).into())
        }
        IpAddr::V6(v6) => {
            let m = if bits == 0 {
                0
            } else {
                u128::MAX << (128 - bits as u32)
            };
            IpAddr::V6((u128::from(v6) & m).into())
        }
    }
}

impl Cidr {
    /// `IPNet.Contains`.
    fn contains(&self, ip: IpAddr) -> bool {
        let ip = unmap(ip);
        match (self.network, ip) {
            (IpAddr::V4(_), IpAddr::V4(_)) | (IpAddr::V6(_), IpAddr::V6(_)) => {
                mask_ip(ip, self.bits) == self.network
            }
            _ => false,
        }
    }
}

/// `ValidateLocalLLMRelayArgs`.
pub fn validate_relay_args(args: &RelayArgs) -> Result<(), String> {
    if args.session_id.trim().is_empty() || args.session_id.len() > 128 {
        return Err("sessionId is required".into());
    }
    let listen = parse_ip(args.listen_host.trim());
    if listen.is_none_or(unspecified) || parse_ip(args.target_host.trim()).is_none() {
        return Err("relay hosts must be IP addresses".into());
    }
    if args.target_port < 1
        || args.target_port > 65535
        || args.listen_port < 0
        || args.listen_port > 65535
    {
        return Err("relay port is invalid".into());
    }
    let mut cidrs = args.cidrs().to_vec();
    if cidrs.is_empty() && !args.allowed_source_ip.trim().is_empty() {
        cidrs = vec![format!("{}/32", args.allowed_source_ip)];
    }
    if cidrs.is_empty() {
        return Err("allowedSourceCIDRs is required".into());
    }
    if cidrs.iter().any(|c| parse_cidr(c.trim()).is_none()) {
        return Err("allowedSourceCIDRs contains invalid CIDR".into());
    }
    let mut incoming = args.incoming_token.trim();
    if incoming.is_empty() {
        incoming = args.relay_token.trim();
    }
    if incoming.len() < 32 || incoming.contains(['\r', '\n']) {
        return Err("incomingToken is invalid".into());
    }
    if args.upstream_token.contains(['\r', '\n']) {
        return Err("upstreamToken is invalid".into());
    }
    Ok(())
}

/// `localLLMRelayHostsOverlap`.
fn relay_hosts_overlap(a: &str, b: &str) -> bool {
    let (a, b) = (a.trim(), b.trim());
    let wildcard = |v: &str| v.is_empty() || parse_ip(v).is_some_and(unspecified);
    if wildcard(a) || wildcard(b) {
        return true;
    }
    match (parse_ip(a), parse_ip(b)) {
        (Some(x), Some(y)) => unmap(x) == unmap(y),
        _ => a.eq_ignore_ascii_case(b),
    }
}

/// `localLLMRelayBindingsConflict`.
fn relay_bindings_conflict(
    existing_host: &str,
    existing_port: i64,
    desired_host: &str,
    desired_port: i64,
) -> bool {
    desired_port != 0
        && existing_port == desired_port
        && relay_hosts_overlap(existing_host, desired_host)
}

/// One live relay: its policy and the thread serving its listener.
struct RelaySession {
    id: String,
    allowed: Vec<Cidr>,
    incoming_token: String,
    listen_host: String,
    listen_port: i64,
    target_host: String,
    target_port: i64,
    stop: Mutex<
        Option<(
            tokio::sync::oneshot::Sender<()>,
            std::thread::JoinHandle<()>,
        )>,
    >,
}

impl RelaySession {
    /// `closeLocalLLMRelaySession`: the listener and every connection close
    /// before this returns, so the port is free for a reclaiming bind.
    fn close(&self) {
        let pair = self.stop.lock().ok().and_then(|mut s| s.take());
        if let Some((tx, thread)) = pair {
            let _ = tx.send(());
            let _ = thread.join();
        }
    }
}

/// `localLLMRelayManager`.
pub struct RelayManager {
    sessions: Mutex<Vec<Arc<RelaySession>>>,
    persist: bool,
    config_dir: String,
    lock_path: Option<String>,
    /// `os.UserHomeDir`, for the default relay directory.
    home: String,
}

/// `localLLMRelayCIDRsEqual`.
fn relay_cidrs_equal(existing: &[Cidr], desired: &[String]) -> bool {
    if existing.len() != desired.len() {
        return false;
    }
    let mut normalized = Vec::new();
    for source in desired {
        match parse_cidr(source.trim()) {
            Some(c) => normalized.push(c),
            None => return false,
        }
    }
    existing.iter().all(|c| normalized.contains(c))
}

impl RelayManager {
    /// `newPersistentLocalLLMRelayManagerAtWithLock`: persisted relays are
    /// restored at construction.
    fn persistent_at(config_dir: &str, lock_dir: &str, home: &str) -> Arc<Self> {
        let lock_dir = lock_dir.trim();
        let manager = Arc::new(RelayManager {
            sessions: Mutex::new(Vec::new()),
            persist: true,
            config_dir: config_dir.trim().to_string(),
            lock_path: (!lock_dir.is_empty()).then(|| format!("{lock_dir}/ollama-inference.lock")),
            home: home.to_string(),
        });
        manager.restore();
        manager
    }

    fn sessions(&self) -> std::sync::MutexGuard<'_, Vec<Arc<RelaySession>>> {
        self.sessions.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// `relayConfigDir`: the configured directory, else the account's.
    fn relay_config_dir(&self) -> Result<String, String> {
        if !self.config_dir.trim().is_empty() {
            return Ok(self.config_dir.clone());
        }
        if self.home.is_empty() {
            return Err("resolve home directory: $HOME is not defined".into());
        }
        Ok(clean_any(&format!(
            "{}/.config/opute/local-llm-relays",
            self.home
        )))
    }

    /// `localLLMRelayConfigPathInDir`.
    fn config_path(dir: &str, session_id: &str) -> Result<String, String> {
        if session_id.trim().is_empty() || session_id.contains(['/', '\\', '\r', '\n', '\0']) {
            return Err("sessionId is invalid".into());
        }
        if dir.trim().is_empty() {
            return Err("relay config directory is required".into());
        }
        Ok(clean_any(&format!("{dir}/{session_id}.json")))
    }

    fn persist_args(&self, args: &RelayArgs) -> Result<(), String> {
        let dir = self.relay_config_dir()?;
        let path = Self::config_path(&dir, &args.session_id)?;
        mkdir_all(&dir_of(&path), 0o700)?;
        write_file(&path, format!("{}\n", args.marshal()).as_bytes(), 0o600)
    }

    fn remove_persisted(&self, session_id: &str) {
        let Ok(dir) = self.relay_config_dir() else {
            return;
        };
        let Ok(path) = Self::config_path(&dir, session_id) else {
            return;
        };
        let _ = std::fs::remove_file(path);
    }

    /// `restore`: every `*.json` in the directory, in name order.
    fn restore(&self) {
        let Ok(dir) = self.relay_config_dir() else {
            return;
        };
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return;
        };
        let mut names: Vec<(String, bool)> = entries
            .filter_map(Result::ok)
            .map(|e| {
                let is_dir = e.file_type().is_ok_and(|t| t.is_dir());
                (e.file_name().to_string_lossy().into_owned(), is_dir)
            })
            .collect();
        names.sort();
        for (name, is_dir) in names {
            if is_dir || !go_ext_is(&name, ".json") {
                continue;
            }
            let Ok(content) = std::fs::read(format!("{dir}/{name}")) else {
                continue;
            };
            let Ok(args) = RelayArgs::unmarshal(&content) else {
                continue;
            };
            let _ = self.start(args);
        }
    }

    /// `start`.
    pub fn start(&self, mut args: RelayArgs) -> Result<Map<String, J>, String> {
        validate_relay_args(&args)?;
        if args.cidrs().is_empty() && !args.allowed_source_ip.trim().is_empty() {
            args.allowed_source_cidrs = Some(vec![format!("{}/32", args.allowed_source_ip.trim())]);
        }
        let ready = |id: &str, host: &str, port: i64| {
            let mut out = Map::new();
            out.insert("sessionId".into(), J::from(id));
            out.insert("listenHost".into(), J::from(host));
            out.insert("listenPort".into(), J::from(port));
            out.insert("ready".into(), J::Bool(true));
            out
        };
        let mut stale = None;
        {
            let mut sessions = self.sessions();
            if let Some(index) = sessions.iter().position(|s| s.id == args.session_id) {
                let existing = sessions[index].clone();
                let same = existing.incoming_token == args.incoming()
                    && existing.listen_host == args.listen_host
                    && (args.listen_port == 0 || existing.listen_port == args.listen_port)
                    && existing.target_host == args.target_host
                    && existing.target_port == args.target_port
                    && relay_cidrs_equal(&existing.allowed, args.cidrs());
                if same {
                    return Ok(ready(
                        &existing.id,
                        &existing.listen_host,
                        existing.listen_port,
                    ));
                }
                sessions.remove(index);
                drop(sessions);
                existing.close();
                if self.persist {
                    self.remove_persisted(&args.session_id);
                }
            } else if let Some(index) = sessions.iter().position(|s| {
                relay_bindings_conflict(
                    &s.listen_host,
                    s.listen_port,
                    &args.listen_host,
                    args.listen_port,
                )
            }) {
                stale = Some(sessions.remove(index));
            }
        }
        if let Some(stale) = stale {
            stale.close();
            if self.persist {
                self.remove_persisted(&stale.id);
            }
        }
        let allowed: Vec<Cidr> = args
            .cidrs()
            .iter()
            .filter_map(|c| parse_cidr(c.trim()))
            .collect();
        let address = if args.listen_host.contains(':') {
            format!("[{}]:{}", args.listen_host, args.listen_port)
        } else {
            format!("{}:{}", args.listen_host, args.listen_port)
        };
        let (listener, _, port) = crate::tcprelay::listen(&address)?;
        let policy = Arc::new(Policy {
            allowed: allowed.clone(),
            legacy_cidrs_empty: args.cidrs().is_empty(),
            allowed_source_ip: parse_ip(args.allowed_source_ip.trim()),
            incoming: args.incoming().to_string(),
            upstream_token: args.upstream_token.clone(),
            target_host: args.target_host.clone(),
            target_port: args.target_port,
            lock_path: self.lock_path.clone(),
        });
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let thread = std::thread::Builder::new()
            .name(format!("llm-relay-{}", args.session_id))
            .spawn(move || serve_relay(listener, policy, rx))
            .map_err(|e| e.to_string())?;
        let session = Arc::new(RelaySession {
            id: args.session_id.clone(),
            allowed,
            incoming_token: args.incoming().to_string(),
            listen_host: args.listen_host.clone(),
            listen_port: i64::from(port),
            target_host: args.target_host.clone(),
            target_port: args.target_port,
            stop: Mutex::new(Some((tx, thread))),
        });
        self.sessions().push(session.clone());
        if self.persist {
            if let Err(err) = self.persist_args(&args) {
                session.close();
                self.sessions().retain(|s| !Arc::ptr_eq(s, &session));
                return Err(err);
            }
        }
        Ok(ready(&args.session_id, &args.listen_host, i64::from(port)))
    }

    /// `stop`: the persisted session goes either way.
    pub fn stop(&self, id: &str) -> bool {
        let session = {
            let mut sessions = self.sessions();
            sessions
                .iter()
                .position(|s| s.id == id)
                .map(|i| sessions.remove(i))
        };
        let found = session.is_some();
        if let Some(session) = session {
            session.close();
        }
        if self.persist {
            self.remove_persisted(id);
        }
        found
    }

    /// `stopAll`.
    pub fn stop_all(&self) {
        let ids: Vec<String> = self.sessions().iter().map(|s| s.id.clone()).collect();
        for id in ids {
            self.stop(&id);
        }
    }
}

/// `filepath.Ext(name) == ext`.
fn go_ext_is(name: &str, ext: &str) -> bool {
    let base = name.rsplit('/').next().unwrap_or(name);
    base.rfind('.').map(|i| &base[i..]) == Some(ext)
}

/// What a relay's handler decides with.
struct Policy {
    allowed: Vec<Cidr>,
    legacy_cidrs_empty: bool,
    allowed_source_ip: Option<IpAddr>,
    incoming: String,
    upstream_token: String,
    target_host: String,
    target_port: i64,
    lock_path: Option<String>,
}

/// The relay's `http.Server`: one thread, closed with every connection when
/// the session stops.
fn serve_relay(
    listener: std::net::TcpListener,
    policy: Arc<Policy>,
    stop: tokio::sync::oneshot::Receiver<()>,
) {
    let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return;
    };
    let local = tokio::task::LocalSet::new();
    local.block_on(&runtime, async move {
        let _ = listener.set_nonblocking(true);
        let Ok(listener) = tokio::net::TcpListener::from_std(listener) else {
            return;
        };
        let mut stop = stop;
        loop {
            tokio::select! {
                _ = &mut stop => return,
                accepted = listener.accept() => {
                    if let Ok((stream, _)) = accepted {
                        let policy = policy.clone();
                        tokio::task::spawn_local(crate::http1::serve_conn(stream, move |req, body| {
                            relay_handle(policy.clone(), req, body)
                        }));
                    }
                }
            }
        }
    });
    // Dropping the set and the runtime closes the listener and every
    // connection still open.
    drop(local);
    drop(runtime);
}

/// `url.PathUnescape` for `URL.Path`.
fn unescape_path(escaped: &str) -> String {
    let bytes = escaped.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(&escaped[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `subtle.ConstantTimeCompare` after a length check.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// The relay handler: source CIDRs, the inference paths, the incoming
/// bearer, then the upstream credential and the host-wide request lock.
async fn relay_handle(
    policy: Arc<Policy>,
    req: crate::http1::Request,
    mut body: crate::http1::Body,
) -> (crate::http1::Response, crate::http1::Body) {
    use crate::http1::Response;
    let remote = unmap(req.remote_addr.ip());
    let mut allowed = policy.allowed.iter().any(|c| c.contains(remote));
    if policy.legacy_cidrs_empty {
        if let Some(ip) = policy.allowed_source_ip {
            allowed = unmap(ip) == remote;
        }
    }
    if !allowed {
        return (Response::error("forbidden", 403), body);
    }
    let path = unescape_path(&req.escaped_path);
    let inference_path = path.starts_with("/v1/")
        || path == "/v1"
        || path == "/api/ps"
        || path == "/api/tags"
        || path == "/api/version"
        || path == "/api/embed";
    if !inference_path {
        return (Response::error("404 page not found", 404), body);
    }
    let auth = req.headers.get("Authorization").to_string();
    let want = format!("Bearer {}", policy.incoming);
    if !constant_time_eq(auth.as_bytes(), want.as_bytes()) {
        return (Response::error("unauthorized", 401), body);
    }
    let guard =
        if req.method == "POST" && (path.starts_with("/v1/") || path.starts_with("/api/embed")) {
            match acquire_request_lock(policy.lock_path.as_deref()).await {
                Ok(guard) => guard,
                Err(_) => return (Response::error("inference request cancelled", 408), body),
            }
        } else {
            None
        };
    let payload = match body.read_all(None).await {
        Ok(payload) => payload,
        Err(_) => return (Response::error("upstream unavailable", 502), body),
    };
    let response = proxy(&policy, &req, payload, guard).await;
    (response, body)
}

type RequestLock = nix::fcntl::Flock<std::fs::File>;

/// `hostRequestLock.acquire`: an exclusive `flock`, polled every 25ms.
async fn acquire_request_lock(path: Option<&str>) -> Result<Option<RequestLock>, String> {
    use std::os::unix::fs::OpenOptionsExt;
    let Some(path) = path else { return Ok(None) };
    mkdir_all(&dir_of(path), 0o700)?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| format!("open shared Ollama request lock: {e}"))?;
    loop {
        match nix::fcntl::Flock::lock(file, nix::fcntl::FlockArg::LockExclusiveNonblock) {
            Ok(lock) => return Ok(Some(lock)),
            Err((back, nix::errno::Errno::EWOULDBLOCK)) => file = back,
            Err((_, errno)) => {
                return Err(format!(
                    "acquire shared Ollama request lock: {}",
                    errno.desc().to_lowercase()
                ))
            }
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// `removeHopByHopHeaders`.
fn remove_hop_by_hop(headers: &mut crate::http1::Headers) {
    let named: Vec<String> = headers
        .values("Connection")
        .iter()
        .flat_map(|v| v.split(',').map(|f| f.trim().to_string()))
        .filter(|f| !f.is_empty())
        .collect();
    for field in named {
        headers.del(&field);
    }
    for h in [
        "Connection",
        "Proxy-Connection",
        "Keep-Alive",
        "Proxy-Authenticate",
        "Proxy-Authorization",
        "Te",
        "Trailer",
        "Transfer-Encoding",
        "Upgrade",
    ] {
        headers.del(h);
    }
}

/// A streamed upstream body: chunks, or the error that aborts the reply.
struct ChannelBody {
    rx: tokio::sync::mpsc::Receiver<std::io::Result<Vec<u8>>>,
    pending: Vec<u8>,
}

impl tokio::io::AsyncRead for ChannelBody {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        use std::task::Poll;
        loop {
            if !self.pending.is_empty() {
                let n = self.pending.len().min(buf.remaining());
                buf.put_slice(&self.pending[..n]);
                self.pending.drain(..n);
                return Poll::Ready(Ok(()));
            }
            match self.rx.poll_recv(cx) {
                Poll::Ready(Some(Ok(chunk))) => {
                    if chunk.is_empty() {
                        continue;
                    }
                    self.pending = chunk;
                }
                Poll::Ready(Some(Err(e))) => return Poll::Ready(Err(e)),
                Poll::Ready(None) => return Poll::Ready(Ok(())),
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

/// `httputil.ReverseProxy` for one request to the relay's target, with the
/// response streamed back as it arrives (`FlushInterval: -1`).
async fn proxy(
    policy: &Policy,
    req: &crate::http1::Request,
    payload: Vec<u8>,
    guard: Option<RequestLock>,
) -> crate::http1::Response {
    use crate::http1::Response;
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt};
    let unavailable = || Response::error("upstream unavailable", 502);
    let host = format!("{}:{}", policy.target_host, policy.target_port);
    let Some(ip) = parse_ip(&policy.target_host) else {
        return unavailable();
    };
    let Ok(port) = u16::try_from(policy.target_port) else {
        return unavailable();
    };
    let Ok(mut upstream) = tokio::net::TcpStream::connect((ip, port)).await else {
        return unavailable();
    };
    // The outgoing request (`Director`, `removeHopByHopHeaders`,
    // `X-Forwarded-For`, then the transport's own headers).
    let mut headers = req.headers.clone();
    let trailers = headers.values("Te").iter().any(|v| {
        v.split(',')
            .any(|t| t.trim().eq_ignore_ascii_case("trailers"))
    });
    remove_hop_by_hop(&mut headers);
    if trailers {
        headers.set("Te", "trailers");
    }
    headers.del("Authorization");
    if !policy.upstream_token.trim().is_empty() {
        headers.set("Authorization", format!("Bearer {}", policy.upstream_token));
    }
    let client_ip = req.remote_addr.ip().to_string();
    if headers.has("X-Forwarded-For") {
        let prior = headers.values("X-Forwarded-For").join(", ");
        headers.set("X-Forwarded-For", format!("{prior}, {client_ip}"));
    } else {
        headers.set("X-Forwarded-For", client_ip);
    }
    let mut target = req.escaped_path.clone();
    if !req.raw_query.is_empty() {
        target.push('?');
        target.push_str(&req.raw_query);
    }
    let mut head = format!("{} {target} HTTP/1.1\r\nHost: {host}\r\n", req.method);
    let user_agent = headers.get("User-Agent").to_string();
    if !user_agent.is_empty() {
        head.push_str(&format!("User-Agent: {user_agent}\r\n"));
    }
    let chunked_body = req.content_length().is_none() && !payload.is_empty();
    if chunked_body {
        head.push_str("Transfer-Encoding: chunked\r\n");
    } else if !payload.is_empty() || matches!(req.method.as_str(), "POST" | "PUT" | "PATCH") {
        head.push_str(&format!("Content-Length: {}\r\n", payload.len()));
    }
    let mut keys: Vec<(String, Vec<String>)> = headers
        .iter()
        .filter(|(k, _)| {
            !matches!(
                *k,
                "Host" | "User-Agent" | "Content-Length" | "Transfer-Encoding" | "Trailer"
            )
        })
        .map(|(k, v)| (k.to_string(), v.to_vec()))
        .collect();
    keys.sort_by(|a, b| a.0.cmp(&b.0));
    for (k, vv) in &keys {
        for v in vv {
            head.push_str(&format!("{k}: {}\r\n", v.replace(['\r', '\n'], " ").trim()));
        }
    }
    let requested_gzip = headers.get("Accept-Encoding").is_empty()
        && headers.get("Range").is_empty()
        && req.method != "HEAD";
    if requested_gzip {
        head.push_str("Accept-Encoding: gzip\r\n");
    }
    head.push_str("\r\n");
    let mut wire = head.into_bytes();
    if chunked_body {
        wire.extend_from_slice(format!("{:x}\r\n", payload.len()).as_bytes());
        wire.extend_from_slice(&payload);
        wire.extend_from_slice(b"\r\n0\r\n\r\n");
    } else {
        wire.extend_from_slice(&payload);
    }
    if upstream.write_all(&wire).await.is_err() {
        return unavailable();
    }
    let mut reader = tokio::io::BufReader::new(upstream);
    // The response head.
    let mut status_line = String::new();
    let status = loop {
        status_line.clear();
        if reader.read_line(&mut status_line).await.unwrap_or(0) == 0 {
            return unavailable();
        }
        let mut parts = status_line.trim_end().splitn(3, ' ');
        let (Some(proto), Some(code)) = (parts.next(), parts.next()) else {
            return unavailable();
        };
        if !proto.starts_with("HTTP/") {
            return unavailable();
        }
        let Ok(code) = code.parse::<u16>() else {
            return unavailable();
        };
        // Interim 1xx replies (other than 101) are skipped.
        if (100..200).contains(&code) && code != 101 {
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
                    return unavailable();
                }
                if line.trim_end().is_empty() {
                    break;
                }
            }
            continue;
        }
        break code;
    };
    let mut response_headers = crate::http1::Headers::default();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
            return unavailable();
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            let key = crate::http1::canonical_key(k.trim());
            response_headers.add(&key, v.trim().to_string());
        }
    }
    let te_chunked = response_headers
        .values("Transfer-Encoding")
        .iter()
        .any(|v| v.to_ascii_lowercase().contains("chunked"));
    let no_body =
        req.method == "HEAD" || status == 204 || status == 304 || (100..200).contains(&status);
    let length = if te_chunked {
        None
    } else {
        response_headers
            .get("Content-Length")
            .trim()
            .parse::<u64>()
            .ok()
    };
    if te_chunked {
        response_headers.del("Content-Length");
    }
    let gzipped = requested_gzip
        && response_headers
            .get("Content-Encoding")
            .eq_ignore_ascii_case("gzip");
    if gzipped {
        response_headers.del("Content-Encoding");
        response_headers.del("Content-Length");
    }
    remove_hop_by_hop(&mut response_headers);
    let mut response = Response::new(status);
    response.headers = response_headers;
    if no_body {
        drop(guard);
        return response;
    }
    let (tx, rx) = tokio::sync::mpsc::channel::<std::io::Result<Vec<u8>>>(16);
    tokio::task::spawn_local(async move {
        let _guard = guard;
        let abort = |tx: &tokio::sync::mpsc::Sender<std::io::Result<Vec<u8>>>| {
            let _ = tx.try_send(Err(std::io::Error::other("upstream body")));
        };
        let mut collected = Vec::new();
        let emit = |chunk: Vec<u8>, collected: &mut Vec<u8>| -> Option<Vec<u8>> {
            if gzipped {
                collected.extend_from_slice(&chunk);
                None
            } else {
                Some(chunk)
            }
        };
        if te_chunked {
            loop {
                let mut size = String::new();
                if reader.read_line(&mut size).await.unwrap_or(0) == 0 {
                    return abort(&tx);
                }
                let size = size
                    .trim()
                    .split(';')
                    .next()
                    .unwrap_or("")
                    .trim()
                    .to_string();
                let Ok(n) = usize::from_str_radix(&size, 16) else {
                    return abort(&tx);
                };
                if n == 0 {
                    // Trailers, if any, end at the blank line.
                    loop {
                        let mut line = String::new();
                        if reader.read_line(&mut line).await.unwrap_or(0) == 0
                            || line.trim_end().is_empty()
                        {
                            break;
                        }
                    }
                    break;
                }
                let mut chunk = vec![0u8; n];
                if reader.read_exact(&mut chunk).await.is_err() {
                    return abort(&tx);
                }
                let mut crlf = [0u8; 2];
                if reader.read_exact(&mut crlf).await.is_err() {
                    return abort(&tx);
                }
                if let Some(chunk) = emit(chunk, &mut collected) {
                    if tx.send(Ok(chunk)).await.is_err() {
                        return;
                    }
                }
            }
        } else {
            let mut remaining = length;
            let mut buf = vec![0u8; 32 * 1024];
            loop {
                let want = match remaining {
                    Some(0) => break,
                    Some(n) => buf.len().min(n as usize),
                    None => buf.len(),
                };
                let n = match reader.read(&mut buf[..want]).await {
                    Ok(0) if remaining.is_none() => break,
                    Ok(0) | Err(_) => return abort(&tx),
                    Ok(n) => n,
                };
                if let Some(r) = remaining.as_mut() {
                    *r -= n as u64;
                }
                if let Some(chunk) = emit(buf[..n].to_vec(), &mut collected) {
                    if tx.send(Ok(chunk)).await.is_err() {
                        return;
                    }
                }
            }
        }
        if gzipped {
            let mut out = Vec::new();
            if flate2::read::GzDecoder::new(&collected[..])
                .read_to_end(&mut out)
                .is_err()
            {
                return abort(&tx);
            }
            let _ = tx.send(Ok(out)).await;
        }
    });
    response.stream = Some(Box::new(ChannelBody {
        rx,
        pending: Vec::new(),
    }));
    response
}

impl Llm {
    /// `EnsureLocalLLMRelay`.
    pub fn ensure_relay(&self, args: RelayArgs) -> Result<Map<String, J>, String> {
        self.live().relay.start(args)
    }

    /// `RemoveLocalLLMRelay`.
    pub fn remove_relay(&self, id: &str) -> J {
        let removed = self.live().relay.stop(id.trim());
        let mut out = Map::new();
        out.insert("sessionId".into(), J::from(id));
        out.insert("removed".into(), J::Bool(removed));
        J::Object(out)
    }
}

// --- the cluster proxy (relay.go, gateway.go) --------------------------------------------

/// `LocalLLMK3sProxyArgs`.
#[derive(Clone, Debug, Default)]
pub struct K3sProxyArgs {
    pub vm_name: String,
    pub namespace: String,
    pub secret_name: String,
    pub config_map_name: String,
    pub deployment_name: String,
    pub service_name: String,
    pub container_image: String,
    pub node_port: i64,
    pub relay_host: String,
    pub relay_port: i64,
    pub relay_token: String,
    pub bearer_key: String,
}

/// `safeGatewayIdentifier`: `^[a-z0-9][a-z0-9.-]{0,62}$`.
fn safe_gateway_identifier(value: &str) -> bool {
    let b = value.as_bytes();
    !b.is_empty()
        && b.len() <= 63
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b[1..]
            .iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'.' | b'-'))
}

/// `RenderLocalLLMK3sProxyManifest`.
fn render_k3s_proxy_manifest(args: &K3sProxyArgs) -> Result<String, String> {
    if !safe_gateway_identifier(&args.vm_name.trim().to_lowercase()) {
        return Err("vmName is invalid".into());
    }
    let pick = |v: &str, d: &str| {
        if v.is_empty() {
            d.to_string()
        } else {
            v.to_string()
        }
    };
    let namespace = pick(&args.namespace, "opute-llm");
    let secret = pick(&args.secret_name, "opute-llm-proxy-credentials");
    let config_map = pick(&args.config_map_name, "opute-llm-proxy-config");
    let deployment = pick(&args.deployment_name, "opute-llm-proxy");
    let service = pick(&args.service_name, "opute-llm-proxy");
    let image = pick(&args.container_image, "nginx:1.27.0");
    for (name, value) in [
        ("namespace", &namespace),
        ("secretName", &secret),
        ("configMapName", &config_map),
        ("deploymentName", &deployment),
        ("serviceName", &service),
    ] {
        if !safe_gateway_identifier(&value.trim().to_lowercase()) {
            return Err(format!("{name} is invalid"));
        }
    }
    if image.contains(['\r', '\n']) || image.trim().is_empty() {
        return Err("containerImage is invalid".into());
    }
    if args.node_port < 30000 || args.node_port > 32767 {
        return Err("nodePort must be in the NodePort range".into());
    }
    if parse_ip(&args.relay_host).is_none() || args.relay_port < 1 || args.relay_port > 65535 {
        return Err("relay endpoint is invalid".into());
    }
    if args.relay_token.len() < 32
        || args.bearer_key.len() < 32
        || format!("{}{}", args.relay_token, args.bearer_key).contains(['\r', '\n'])
    {
        return Err("proxy credentials are invalid".into());
    }
    Ok(format!(
        "apiVersion: v1
kind: Namespace
metadata:
  name: {namespace}
---
apiVersion: v1
kind: Secret
metadata:
  name: {secret}
  namespace: {namespace}
type: Opaque
stringData:
  relay-token: PLACEHOLDER_RELAY_TOKEN
  bearer-key: PLACEHOLDER_BEARER_KEY
---
apiVersion: v1
kind: ConfigMap
metadata:
  name: {config_map}
  namespace: {namespace}
data:
  nginx.conf.template: |
    events {{}}
    http {{
      server {{
        listen 8080;
        location = /healthz {{ return 200 'ok'; add_header Content-Type text/plain; }}
        location ^~ /api/ {{ return 404; }}
        location / {{
          if ($http_authorization != \"Bearer ${{BEARER_KEY}}\") {{ return 401; }}
          proxy_http_version 1.1;
          proxy_set_header Authorization \"Bearer ${{RELAY_TOKEN}}\";
          proxy_set_header Host $host;
          proxy_buffering off;
          proxy_request_buffering off;
          proxy_read_timeout 3600s;
          proxy_pass http://${{RELAY_HOST}}:${{RELAY_PORT}};
        }}
      }}
    }}
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: {deployment}
  namespace: {namespace}
spec:
  replicas: 1
  selector:
    matchLabels: {{app: {deployment}}}
  template:
    metadata: {{labels: {{app: {deployment}}}}}
    spec:
      containers:
      - name: nginx
        image: {image}
        ports: [{{containerPort: 8080}}]
        env:
        - name: RELAY_TOKEN
          valueFrom: {{secretKeyRef: {{name: {secret}, key: relay-token}}}}
        - name: BEARER_KEY
          valueFrom: {{secretKeyRef: {{name: {secret}, key: bearer-key}}}}
        - name: RELAY_HOST
          value: {relay_host}
        - name: RELAY_PORT
          value: {relay_port}
        command: [\"/bin/sh\", \"-c\"]
        args: [\"envsubst '${{RELAY_TOKEN}} ${{BEARER_KEY}} ${{RELAY_HOST}} ${{RELAY_PORT}}' < /etc/nginx-template/nginx.conf.template > /etc/nginx/nginx.conf && exec nginx -g 'daemon off;'\"]
        volumeMounts: [{{name: nginx-template, mountPath: /etc/nginx-template, readOnly: true}}]
        readinessProbe: {{httpGet: {{path: /healthz, port: 8080}}}}
      volumes: [{{name: nginx-template, configMap: {{name: {config_map}}}}}]
---
apiVersion: v1
kind: Service
metadata:
  name: {service}
  namespace: {namespace}
spec:
  type: NodePort
  selector: {{app: {service}}}
  ports:
  - port: 8080
    targetPort: 8080
    nodePort: {node_port}
",
        relay_host = quote(&args.relay_host),
        relay_port = args.relay_port,
        node_port = args.node_port,
    ))
}

impl Llm {
    /// `EnsureLocalLLMK3sProxy`.
    pub fn ensure_k3s_proxy(&self, args: &K3sProxyArgs) -> Result<J, String> {
        let vm = args.vm_name.trim();
        if vm.is_empty() {
            return Err("vmName is invalid".into());
        }
        let manifest = render_k3s_proxy_manifest(args)?
            .replace("PLACEHOLDER_RELAY_TOKEN", &quote(&args.relay_token))
            .replace("PLACEHOLDER_BEARER_KEY", &quote(&args.bearer_key));
        let target = self.k8s.target_uri(vm)?;
        self.k8s.apply_manifest(&target, &manifest)?;
        let mut out = Map::new();
        out.insert("vmName".into(), J::from(vm));
        out.insert("nodePort".into(), J::from(args.node_port));
        out.insert("namespace".into(), J::from(args.namespace.as_str()));
        out.insert("serviceName".into(), J::from(args.service_name.as_str()));
        out.insert("ready".into(), J::Bool(true));
        Ok(J::Object(out))
    }

    /// `RemoveLocalLLMK3sProxy`: the proxy's namespace goes with it.
    pub fn remove_k3s_proxy(&self, vm_name: &str, namespace: &str) -> Result<J, String> {
        let vm = vm_name.trim();
        if !safe_gateway_identifier(vm) {
            return Err("vmName is invalid".into());
        }
        let mut namespace = namespace.trim().to_string();
        if namespace.is_empty() {
            namespace = "opute-llm".into();
        }
        if !safe_gateway_identifier(&namespace.to_lowercase()) {
            return Err("namespace is invalid".into());
        }
        let target = self.k8s.target_uri(vm)?;
        self.k8s.delete_resource(&crate::clusterops::ResourceArgs {
            uri: target,
            kind: "namespace".into(),
            resource_kind: String::new(),
            name: namespace.clone(),
            namespace: String::new(),
        })?;
        let mut out = Map::new();
        out.insert("vmName".into(), J::from(vm));
        out.insert("namespace".into(), J::from(namespace));
        out.insert("removed".into(), J::Bool(true));
        Ok(J::Object(out))
    }
}

// --- the runtime-neutral probe (runtime_probe.go) ----------------------------------------

/// `RuntimeObservation`'s field order.
pub const OBSERVATION_SHAPE: crate::gojson::Shape = crate::gojson::Shape::Struct(&[
    ("servingContract", crate::gojson::Shape::Any),
    ("apiBaseUrl", crate::gojson::Shape::Any),
    ("modelRef", crate::gojson::Shape::Any),
    ("models", crate::gojson::Shape::Any),
    ("ready", crate::gojson::Shape::Any),
    ("endpointReady", crate::gojson::Shape::Any),
    ("chatReady", crate::gojson::Shape::Any),
    ("streamingChatReady", crate::gojson::Shape::Any),
    ("openAiModelsReady", crate::gojson::Shape::Any),
    ("loadError", crate::gojson::Shape::Any),
    ("remediationHints", crate::gojson::Shape::Any),
]);

/// `normalizeOpenAIBaseURL`: scheme and host required, query and fragment
/// dropped, the path ending in `/v1`.
fn normalize_openai_base_url(raw: &str) -> Result<String, String> {
    let raw = raw.trim();
    let invalid = || "endpoint must be an absolute HTTP(S) URL".to_string();
    if raw.bytes().any(|c| c < 0x20 || c == 0x7f) || raw.contains(' ') {
        return Err(invalid());
    }
    let Some((scheme, rest)) = raw.split_once(':') else {
        return Err(invalid());
    };
    let scheme_ok = scheme
        .bytes()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic())
        && scheme
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'+' | b'-' | b'.'));
    if !scheme_ok {
        return Err(invalid());
    }
    let scheme = scheme.to_ascii_lowercase();
    let Some(rest) = rest.strip_prefix("//") else {
        return Err(invalid());
    };
    let rest = rest.split('#').next().unwrap_or("");
    let rest = rest.split('?').next().unwrap_or("");
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    if host.is_empty() {
        return Err(invalid());
    }
    if scheme != "http" && scheme != "https" {
        return Err("endpoint scheme must be http or https".into());
    }
    let mut path = unescape_path(path).trim_end_matches('/').to_string();
    if !path.ends_with("/v1") {
        path.push_str("/v1");
    }
    let joined = format!("{scheme}://{authority}{}", crate::http1::escape_path(&path));
    Ok(joined.trim_end_matches('/').to_string())
}

/// `runtimeModelMatches`.
fn runtime_model_matches(left: &str, right: &str) -> bool {
    let l = left.strip_suffix(":latest").unwrap_or(left).trim();
    let r = right.strip_suffix(":latest").unwrap_or(right).trim();
    !l.is_empty()
        && !r.is_empty()
        && (l == r || l.ends_with(&format!("/{r}")) || r.ends_with(&format!("/{l}")))
}

/// `ProbeOpenAICompatibleArgs`.
#[derive(Clone, Debug, Default)]
pub struct OpenAiProbeArgs {
    pub endpoint: String,
    pub model_ref: String,
    pub include_chat: bool,
    pub bearer_token: String,
}

/// `ProbeOpenAICompatibleServer`.
pub fn probe_openai_compatible(args: &OpenAiProbeArgs) -> Result<J, String> {
    let base = normalize_openai_base_url(&args.endpoint)?;
    let mut model_ref = args.model_ref.trim().to_string();
    let mut models: Vec<String> = Vec::new();
    let fields = |models: &[String],
                  model_ref: &str,
                  ready: bool,
                  endpoint: bool,
                  chat: bool,
                  streaming: bool,
                  openai: bool,
                  error: &str,
                  hints: &[String]| {
        Fields::new()
            .text("servingContract", "openai-chat.v1", false)
            .text("apiBaseUrl", &base, false)
            .text("modelRef", model_ref, true)
            .value(
                "models",
                J::Array(
                    models
                        .iter()
                        .map(|m| {
                            let mut o = Map::new();
                            o.insert("name".into(), J::from(m.as_str()));
                            J::Object(o)
                        })
                        .collect(),
                ),
            )
            .flag("ready", ready, false)
            .flag("endpointReady", endpoint, true)
            .flag("chatReady", chat, true)
            .flag("streamingChatReady", streaming, true)
            .flag("openAiModelsReady", openai, true)
            .text("loadError", error, true)
            .list("remediationHints", hints, true)
            .done()
    };
    let timeout = Duration::from_secs(60);
    let token = args.bearer_token.trim();
    let auth = format!("Bearer {token}");
    let headers: Vec<(&str, &str)> = if token.is_empty() {
        vec![]
    } else {
        vec![("Authorization", auth.as_str())]
    };
    let reply = match http_request(
        "GET",
        &format!("{base}/models"),
        &headers,
        None,
        timeout,
        2 * 1024 * 1024,
    ) {
        Ok(reply) => reply,
        Err(err) => {
            return Ok(fields(
                &models,
                &model_ref,
                false,
                false,
                false,
                false,
                false,
                &err,
                &[],
            ))
        }
    };
    if !ok_status(reply.status) {
        let err = format!("model discovery returned HTTP {}", reply.status);
        return Ok(fields(
            &models,
            &model_ref,
            false,
            false,
            false,
            false,
            false,
            &err,
            &[],
        ));
    }
    let decoded = match crate::gojson::syntax_error(&reply.body) {
        Some(err) => Err(err),
        None => serde_json::from_slice::<J>(&reply.body)
            .map_err(|e| e.to_string())
            .and_then(|v| {
                slice_member(&v, "data")
                    .map(|d| d.into_iter().map(|m| text_of(m, "id")).collect::<Vec<_>>())
            }),
    };
    let ids = match decoded {
        Ok(ids) => ids,
        Err(err) => {
            let err = format!("decode model discovery response: {err}");
            return Ok(fields(
                &models,
                &model_ref,
                false,
                false,
                false,
                false,
                false,
                &err,
                &[],
            ));
        }
    };
    for id in ids {
        if !id.trim().is_empty() {
            models.push(id);
        }
    }
    let mut ready = !models.is_empty();
    let mut hints = Vec::new();
    if model_ref.is_empty() && !models.is_empty() {
        model_ref = models[0].clone();
    }
    if !model_ref.is_empty() {
        match models.iter().find(|m| runtime_model_matches(m, &model_ref)) {
            Some(m) => model_ref = m.clone(),
            None => {
                ready = false;
                hints.push(
                    "make the requested model available at the configured endpoint".to_string(),
                );
            }
        }
    }
    let (mut chat, mut streaming) = (false, false);
    if args.include_chat && ready {
        streaming = probe_streaming_chat(&base, &model_ref, token, timeout);
        chat = streaming;
        if !chat {
            hints.push("verify the endpoint supports streaming /v1/chat/completions".into());
        }
    }
    Ok(fields(
        &models, &model_ref, ready, true, chat, streaming, true, "", &hints,
    ))
}

/// `probeStreamingChat`: a streamed completion with content and an end.
fn probe_streaming_chat(base: &str, model: &str, token: &str, timeout: Duration) -> bool {
    use std::io::BufRead;
    let mut message = Map::new();
    message.insert(
        "content".into(),
        J::from("Reply with the single word READY."),
    );
    message.insert("role".into(), J::from("user"));
    let payload = json_body(&[
        ("max_tokens", J::from(256)),
        ("messages", J::Array(vec![J::Object(message)])),
        ("model", J::from(model)),
        ("reasoning_effort", J::from("none")),
        ("stream", J::Bool(true)),
        ("temperature", J::from(0)),
    ]);
    let agent = ureq::AgentBuilder::new()
        .timeout(timeout)
        .redirects(10)
        .user_agent("Go-http-client/1.1")
        .tls_config(crate::hostartifact::tls_config())
        .build();
    let mut request = agent
        .post(&format!("{base}/chat/completions"))
        .set("Content-Type", "application/json")
        // Go's transport asks for gzip and decodes it transparently.
        .set("Accept-Encoding", "gzip");
    if !token.is_empty() {
        request = request.set("Authorization", &format!("Bearer {token}"));
    }
    let response = match request.send_bytes(&payload) {
        Ok(response) => response,
        Err(_) => return false,
    };
    if !ok_status(response.status()) {
        return false;
    }
    let gzipped = response
        .header("Content-Encoding")
        .is_some_and(|v| v.eq_ignore_ascii_case("gzip"));
    let body: Box<dyn Read + Send> = if gzipped {
        Box::new(flate2::read::GzDecoder::new(response.into_reader()))
    } else {
        response.into_reader()
    };
    let reader = std::io::BufReader::new(body.take(2 * 1024 * 1024));
    let (mut content, mut completed) = (false, false);
    for line in reader.split(b'\n') {
        let Ok(line) = line else { return false };
        if line.len() > 256 * 1024 {
            return false;
        }
        let line = String::from_utf8_lossy(&line);
        let line = line.trim();
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data == "[DONE]" {
            completed = true;
            continue;
        }
        let Ok(J::Object(chunk)) = serde_json::from_str::<J>(data) else {
            continue;
        };
        if chunk.contains_key("error") {
            return false;
        }
        let Some(choice) = chunk
            .get("choices")
            .and_then(J::as_array)
            .and_then(|c| c.first())
        else {
            continue;
        };
        if choice
            .get("finish_reason")
            .and_then(J::as_str)
            .is_some_and(|r| !r.is_empty())
        {
            completed = true;
        }
        if choice
            .get("delta")
            .and_then(|d| d.get("content"))
            .and_then(J::as_str)
            .is_some_and(|c| !c.trim().is_empty())
        {
            content = true;
        }
    }
    content && completed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(models: &str) -> OllamaConfig {
        OllamaConfig {
            port: 11434,
            model_ref: DEFAULT_OLLAMA_MODEL.into(),
            binary_path: "/usr/bin/ollama".into(),
            models_directory: models.into(),
            context_size: 16384,
            model_contexts: Default::default(),
        }
    }

    #[test]
    fn ollama_unit_keeps_gos_slice_aliasing() {
        // Without a models directory the context line goes before the
        // blank line that precedes [Install].
        let plain = Llm::render_ollama_unit(&config("")).unwrap();
        assert!(plain.ends_with(
            "RestartSec=2\nEnvironment=OLLAMA_CONTEXT_LENGTH=16384\n\n[Install]\nWantedBy=default.target\n"
        ));
        // With one, Go's in-place append overwrites that blank line and
        // repeats the models line around the context line.
        let models = Llm::render_ollama_unit(&config("/srv/models")).unwrap();
        assert!(models.ends_with(
            "RestartSec=2\nEnvironment=OLLAMA_MODELS=/srv/models\nEnvironment=OLLAMA_CONTEXT_LENGTH=16384\n\
             Environment=OLLAMA_MODELS=/srv/models\n[Install]\nWantedBy=default.target\n"
        ));
    }
}
