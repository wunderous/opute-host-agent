//! M8d: Go's `internal/domain/oci` operations, as the built-in provider of
//! the OCI services (decision D14).
//!
//! Registry work goes through the kubernetes domain (`clusterops`) and so
//! through the active Kubernetes provider. Builder, build, push and storage
//! work runs the host's container tools exactly as Go's adapters do: Podman
//! is the one runtime adapter, Buildah the legacy build-only path. The
//! selector values here are Go's; D23 maps the published ones at the
//! boundary (`catalog::go_value_arguments`).

use crate::clusterops::{validate_name, Kubernetes, ResourceArgs};
use crate::goerr::quote;
use serde_json::{json, Map, Value as J};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// `exec.CommandContext(...).CombinedOutput()` with an optional deadline.
pub type Command =
    Arc<dyn Fn(&[String], Option<Instant>) -> (Vec<u8>, Result<(), String>) + Send + Sync>;

/// The oci domain over the seams it declares.
#[derive(Clone)]
pub struct Oci {
    pub k8s: Kubernetes,
    /// The process environment `exec.LookPath` searches.
    pub env: crate::config::Env,
    pub command: Command,
    /// Where the local container storage budget is persisted.
    pub storage_policy_path: String,
    pub storage_mu: Arc<Mutex<()>>,
    /// `Shared.InstanceID` and `Shared.SharedHostOwnerInstance`.
    pub instance_id: String,
    pub shared_host_owner_instance: String,
    /// `os.UserHomeDir`, empty when unknown.
    pub home: String,
}

/// `textutil.Default`.
fn or_default(value: &str, fallback: &str) -> String {
    if value.trim().is_empty() {
        fallback.to_string()
    } else {
        value.to_string()
    }
}

// --- registry (registry.go, registry_storage.go) --------------------------------------

/// `InstallOCIRegistryArgs`.
#[derive(Default)]
pub struct RegistryArgs {
    pub vm_name: String,
    pub namespace: String,
    pub name: String,
    pub image: String,
    pub storage_size: String,
    pub storage_class: String,
    pub node_port: i64,
}

fn resource(uri: &str, kind: &str, name: &str, namespace: &str) -> ResourceArgs {
    ResourceArgs {
        uri: uri.into(),
        kind: kind.into(),
        resource_kind: String::new(),
        name: name.into(),
        namespace: namespace.into(),
    }
}

/// `k8sResourceObject`.
fn resource_object(result: &Map<String, J>) -> Map<String, J> {
    match result.get("resource") {
        Some(J::Object(nested)) => nested.clone(),
        _ => result.clone(),
    }
}

/// `nestedMapString`.
fn nested_string(value: &Map<String, J>, keys: &[&str]) -> String {
    let mut current = value;
    for (index, key) in keys.iter().enumerate() {
        let Some(next) = current.get(*key) else {
            return String::new();
        };
        if index == keys.len() - 1 {
            return next.as_str().unwrap_or("").to_string();
        }
        match next.as_object() {
            Some(object) => current = object,
            None => return String::new(),
        }
    }
    String::new()
}

/// `isKubernetesNotFound`.
fn kubernetes_not_found(err: &str) -> bool {
    let message = err.to_lowercase();
    message.contains("notfound") || message.contains("not found")
}

/// `admitPVCSizeChange`.
fn admit_pvc_size_change(
    current: &str,
    requested: &str,
    assert_expand: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    let (current, requested) = (current.trim(), requested.trim());
    if requested.is_empty() || current.is_empty() {
        return Ok(());
    }
    let want = crate::resource::parse_byte_capacity(requested)
        .map_err(|e| format!("storageSize {} is invalid: {e}", quote(requested)))?;
    let have = crate::resource::parse_byte_capacity(current)
        .map_err(|e| format!("observed storage size {} is invalid: {e}", quote(current)))?;
    if want < have {
        return Err(format!(
            "storage shrink from {current} to {requested} is refused; persistent volumes are grow-only"
        ));
    }
    if want == have {
        return Ok(());
    }
    assert_expand()
}

impl Oci {
    pub fn delete_registry(&self, args: &RegistryArgs) -> Result<Map<String, J>, String> {
        let namespace = or_default(&args.namespace, "registry-system");
        if args.vm_name.trim().is_empty() {
            return Err("vmName is required".into());
        }
        validate_name(&namespace, "namespace")?;
        let target = self.k8s.target_uri(&args.vm_name)?;
        self.k8s
            .delete_resource(&resource(&target, "namespace", &namespace, ""))?;
        let mut out = Map::new();
        out.insert("vmName".into(), J::from(args.vm_name.clone()));
        out.insert("namespace".into(), J::from(namespace));
        out.insert("deleted".into(), J::Bool(true));
        Ok(out)
    }

    pub fn install_registry(&self, args: &RegistryArgs) -> Result<Map<String, J>, String> {
        let vm_name = args.vm_name.trim();
        if vm_name.is_empty() {
            return Err("vmName is required".into());
        }
        let namespace = or_default(&args.namespace, "registry-system");
        let name = or_default(&args.name, "local-registry");
        let image = or_default(&args.image, "registry:3");
        let storage_size = or_default(&args.storage_size, "20Gi");
        let storage_class = or_default(&args.storage_class, "local-path");
        validate_name(&namespace, "namespace")?;
        validate_name(&name, "name")?;
        let invalid = |v: &str| v.contains(['\r', '\n', '\'']) || v.trim().is_empty();
        if invalid(&image) {
            return Err("image is invalid".into());
        }
        if invalid(&storage_size) {
            return Err("storageSize is invalid".into());
        }
        if invalid(&storage_class) {
            return Err("storageClass is invalid".into());
        }
        let node_port = if args.node_port == 0 {
            30500
        } else {
            args.node_port
        };
        if !(30000..=32767).contains(&node_port) {
            return Err("nodePort must be between 30000 and 32767".into());
        }
        let target = self.k8s.target_uri(vm_name)?;
        self.admit_registry_storage(&target, &namespace, &name, &storage_class, &storage_size)?;
        let manifest = format!(
            "apiVersion: v1
kind: Namespace
metadata:
  name: {namespace}
---
apiVersion: v1
kind: PersistentVolumeClaim
metadata:
  name: {name}-data
  namespace: {namespace}
spec:
  accessModes: [\"ReadWriteOnce\"]
  storageClassName: {storage_class}
  resources:
    requests:
      storage: {storage_size}
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: {name}
  namespace: {namespace}
spec:
  replicas: 1
  selector:
    matchLabels:
      app.kubernetes.io/name: {name}
  template:
    metadata:
      labels:
        app.kubernetes.io/name: {name}
    spec:
      containers:
        - name: registry
          image: {image}
          ports:
            - name: registry
              containerPort: 5000
          volumeMounts:
            - name: data
              mountPath: /var/lib/registry
          readinessProbe:
            httpGet:
              path: /v2/
              port: registry
      volumes:
        - name: data
          persistentVolumeClaim:
            claimName: {name}-data
---
apiVersion: v1
kind: Service
metadata:
  name: {name}
  namespace: {namespace}
spec:
  type: NodePort
  selector:
    app.kubernetes.io/name: {name}
  ports:
    - name: registry
      port: 5000
      targetPort: registry
      nodePort: {node_port}
"
        );
        let mut out = self.k8s.apply_manifest(&target, &manifest)?;
        out.insert("namespace".into(), J::from(namespace));
        out.insert("name".into(), J::from(name));
        out.insert("nodePort".into(), J::from(node_port));
        out.insert("storageSize".into(), J::from(storage_size));
        out.insert("storageClass".into(), J::from(storage_class));
        out.insert(
            "endpointHint".into(),
            J::from(format!("<vm-ip>:{node_port}")),
        );
        Ok(out)
    }

    pub fn registry_status(&self, args: &RegistryArgs) -> Result<Map<String, J>, String> {
        let namespace = or_default(&args.namespace, "registry-system");
        let name = or_default(&args.name, "local-registry");
        let target = self.k8s.target_uri(&args.vm_name)?;
        let deployment =
            self.k8s
                .get_resource(&resource(&target, "deployment", &name, &namespace))?;
        let mut out = Map::new();
        out.insert("vmName".into(), J::from(args.vm_name.clone()));
        out.insert("namespace".into(), J::from(namespace));
        out.insert("name".into(), J::from(name));
        out.insert("status".into(), J::from("installed"));
        out.insert(
            "deployment".into(),
            deployment.get("resource").cloned().unwrap_or(J::Null),
        );
        Ok(out)
    }

    /// `admitRegistryStorage`.
    fn admit_registry_storage(
        &self,
        target: &str,
        namespace: &str,
        name: &str,
        storage_class: &str,
        storage_size: &str,
    ) -> Result<(), String> {
        let pvc = match self.k8s.get_resource(&resource(
            target,
            "pvc",
            &format!("{name}-data"),
            namespace,
        )) {
            Ok(pvc) => pvc,
            Err(e) if kubernetes_not_found(&e) => return Ok(()),
            Err(e) => return Err(e),
        };
        let object = resource_object(&pvc);
        let current = nested_string(&object, &["spec", "resources", "requests", "storage"]);
        admit_pvc_size_change(&current, storage_size, || {
            let mut class_name = nested_string(&object, &["spec", "storageClassName"])
                .trim()
                .to_string();
            if class_name.is_empty() {
                class_name = storage_class.to_string();
            }
            self.assert_storage_class_expands(target, &class_name)
        })
    }

    /// `assertStorageClassExpands`.
    fn assert_storage_class_expands(&self, target: &str, class_name: &str) -> Result<(), String> {
        let class_name = class_name.trim();
        if class_name.is_empty() {
            return Err("storageSize cannot grow: StorageClass is unknown, so allowVolumeExpansion cannot be verified".into());
        }
        let class = self
            .k8s
            .get_resource(&resource(target, "storageclass", class_name, ""))
            .map_err(|e| {
                format!(
                    "storageSize cannot grow: StorageClass {} could not be read: {e}",
                    quote(class_name)
                )
            })?;
        if resource_object(&class).get("allowVolumeExpansion") == Some(&J::Bool(true)) {
            return Ok(());
        }
        Err(format!(
            "storageSize cannot grow: StorageClass {} does not allow volume expansion",
            quote(class_name)
        ))
    }
}

// --- commands (container_runtime.go, build_and_push.go) ------------------------------

impl Oci {
    /// `exec.LookPath` (`Shared.ContainerLookPath`), as a path string.
    fn look_path(&self, name: &str) -> Option<String> {
        crate::hostobs::look_path(&self.env, name).map(|p| p.display().to_string())
    }

    /// `hostexec.Run`: combined output; a failure folds the output into the
    /// error when there is any.
    fn run(&self, argv: &[String]) -> Result<Vec<u8>, String> {
        let (output, result) = (self.command)(argv, None);
        match result {
            Ok(()) => Ok(output),
            Err(err) => {
                let text = String::from_utf8_lossy(&output).trim().to_string();
                if text.is_empty() {
                    Err(err)
                } else {
                    Err(format!("{err}: {text}"))
                }
            }
        }
    }

    /// `runStreamingCommand`: combined output, folded into the error.
    fn run_streaming(&self, argv: &[String], deadline: Option<Instant>) -> Result<(), String> {
        let (output, result) = (self.command)(argv, deadline);
        let text = String::from_utf8_lossy(&output).trim().to_string();
        result.map_err(|err| {
            if text.is_empty() {
                err
            } else {
                format!("{err}: {text}")
            }
        })
    }

    fn podman(&self, path: &str, args: &[&str]) -> Result<Vec<u8>, String> {
        let mut argv = vec![path.to_string()];
        argv.extend(args.iter().map(|a| a.to_string()));
        self.run(&argv)
    }

    /// `podmanRuntimeAdapter.Ready`.
    fn podman_ready(&self, path: &str) -> Result<(), String> {
        self.podman(path, &["info", "--format", "json"])
            .map(|_| ())
            .map_err(|e| format!("Podman is not ready: {e}"))
    }

    /// `resolveContainerRuntime`: the ready Podman adapter's path.
    fn resolve_runtime(&self, requested: &str) -> Result<String, String> {
        let requested = match requested.trim().to_lowercase() {
            r if r.is_empty() => "auto".to_string(),
            r => r,
        };
        if requested != "auto" && requested != "podman" {
            return Err("runtime must be one of auto or podman".into());
        }
        let path = self.look_path("podman");
        if requested == "podman" && path.is_none() {
            return Err("podman is not installed".into());
        }
        if let Some(path) = path {
            match self.podman_ready(&path) {
                Ok(()) => return Ok(path),
                Err(e) if requested == "podman" => return Err(e),
                Err(_) => {}
            }
        }
        if requested == "auto" {
            return Err("no ready OCI container runtime found; install and start Podman".into());
        }
        Err("podman is unavailable".into())
    }
}

/// `appendOciBuildArgs`: sorted `--build-arg key=value` pairs.
fn append_build_args(argv: &mut Vec<String>, build_args: &Map<String, J>) {
    let mut keys: Vec<&String> = build_args.keys().collect();
    keys.sort();
    for key in keys {
        let value = build_args[key].as_str().unwrap_or("");
        argv.push("--build-arg".into());
        argv.push(format!("{key}={value}"));
    }
}

/// `splitOciImageRef`: the repository and whether the reference is pinned
/// by digest.
fn split_image_ref(reference: &str) -> (String, bool) {
    let reference = reference.trim();
    if reference.is_empty() {
        return (String::new(), false);
    }
    if let Some(at) = reference.find('@') {
        return (reference[..at].to_string(), true);
    }
    let last_slash = reference.rfind('/').map_or(-1, |i| i as i64);
    if let Some(colon) = reference.rfind(':') {
        if colon as i64 > last_slash {
            return (reference[..colon].to_string(), false);
        }
    }
    (reference.to_string(), false)
}

/// `safeOciImageRef`: `^[A-Za-z0-9][A-Za-z0-9._/:@-]*$`.
fn safe_image_ref(image: &str) -> bool {
    image.bytes().enumerate().all(|(i, c)| {
        c.is_ascii_alphanumeric() || (i > 0 && matches!(c, b'.' | b'_' | b'/' | b':' | b'@' | b'-'))
    }) && !image.is_empty()
}

/// `validateOciBuildArgs`, in Go's map order (unordered; the first failure
/// Go meets is not determined, so names are checked in sorted order).
fn validate_build_args(build_args: &Map<String, J>) -> Result<(), String> {
    for (name, value) in build_args {
        let ok =
            name.bytes().enumerate().all(|(i, c)| {
                c == b'_' || c.is_ascii_alphabetic() || (i > 0 && c.is_ascii_digit())
            }) && !name.is_empty();
        if !ok {
            return Err(format!("buildArgs contains invalid name {}", quote(name)));
        }
        if value.as_str().unwrap_or("").contains(['\0', '\r', '\n']) {
            return Err(format!(
                "buildArgs[{name}] contains an invalid control character"
            ));
        }
    }
    Ok(())
}

// --- builder (builder.go) --------------------------------------------------------------

fn builder_result(builder: &str, path: &str, already_available: bool) -> Map<String, J> {
    let mut out = Map::new();
    out.insert("builder".into(), J::from(builder));
    out.insert("runtime".into(), J::from(builder));
    out.insert("path".into(), J::from(path));
    out.insert("available".into(), J::Bool(true));
    out.insert("alreadyAvailable".into(), J::Bool(already_available));
    out
}

impl Oci {
    pub fn ensure_builder(&self, builder: &str) -> Result<Map<String, J>, String> {
        let mut builder = builder.trim().to_lowercase();
        if builder.is_empty() || builder == "auto" {
            if let Some(path) = self.look_path("podman") {
                if self.podman_ready(&path).is_ok() {
                    return Ok(builder_result("podman", &path, true));
                }
            }
            for candidate in ["buildah", "buildkitd"] {
                if let Some(path) = self.look_path(candidate) {
                    let name = if candidate == "buildkitd" {
                        "buildkit"
                    } else {
                        candidate
                    };
                    return Ok(builder_result(name, &path, true));
                }
            }
            builder = "podman".into();
        }
        if !matches!(builder.as_str(), "podman" | "buildah" | "buildkit") {
            return Err("builder must be one of auto, podman, buildah, or buildkit".into());
        }
        if builder == "podman" {
            if let Some(path) = self.look_path("podman") {
                self.podman_ready(&path)?;
                return Ok(builder_result("podman", &path, true));
            }
        }
        let command = if builder == "buildkit" {
            "buildkitd"
        } else {
            builder.as_str()
        };
        if let Some(path) = self.look_path(command) {
            return Ok(builder_result(&builder, &path, true));
        }
        let package = if builder == "buildkit" {
            "moby-buildkit"
        } else {
            builder.as_str()
        };
        if self.look_path("apt-get").is_none() {
            return Err(format!(
                "{builder} is not installed and apt-get is unavailable; install package {} through the host OS package manager",
                quote(package)
            ));
        }
        let deadline = Instant::now() + Duration::from_secs(5 * 60);
        crate::hostexec::run_privileged_package(&self.env, deadline, "apt-get", &["update"])
            .map_err(|e| format!("update apt package indexes: {e}"))?;
        crate::hostexec::run_privileged_package(
            &self.env,
            deadline,
            "apt-get",
            &["install", "-y", package],
        )
        .map_err(|e| format!("install OCI builder {builder}: {e}"))?;
        let path = self.look_path(command).ok_or_else(|| {
            format!(
                "OCI builder {builder} was installed but {command} is still unavailable: exec: {}: executable file not found in $PATH",
                quote(command)
            )
        })?;
        Ok(builder_result(&builder, &path, false))
    }
}

// --- staging (stage_build_context.go) --------------------------------------------------

use crate::clusterops::{absolute, clean};

/// `base64.StdEncoding.DecodeString`: CR and LF are skipped, and a corrupt
/// input reports Go's offset.
fn std_base64_decode(text: &str) -> Result<Vec<u8>, String> {
    use base64::Engine;
    crate::gojson::validate_base64(text.as_bytes())?;
    let stripped: String = text.chars().filter(|c| !matches!(c, '\r' | '\n')).collect();
    base64::engine::general_purpose::STANDARD
        .decode(stripped)
        .map_err(|e| e.to_string())
}

fn path_under_any(path: &str, roots: &[String]) -> bool {
    let clean_path = clean(path);
    roots.iter().any(|root| {
        let root = clean(root);
        clean_path == root || clean_path.starts_with(&format!("{root}/"))
    })
}

/// `StageBuildContextArgs`.
pub struct StageArgs {
    pub dest_dir: String,
    pub files: Vec<(String, String)>,
    pub file_encoding: String,
}

impl Oci {
    pub fn stage_build_context(&self, args: &StageArgs) -> Result<Map<String, J>, String> {
        let dest = args.dest_dir.trim();
        if dest.is_empty() {
            return Err("destDir is required".into());
        }
        if args.files.is_empty() {
            return Err("files is required and must be non-empty".into());
        }
        let abs_dest = absolute(dest).map_err(|e| format!("resolve destDir: {e}"))?;
        let mut roots = Vec::new();
        if !self.home.is_empty() {
            roots.push(format!("{}/.opute/build-contexts", clean(&self.home)));
            roots.push(format!(
                "{}/.opute/standalone-bootstrap/build-contexts",
                clean(&self.home)
            ));
        }
        roots.push("/tmp/opute-build-contexts".into());
        roots.push("/var/tmp/opute-build-contexts".into());
        if !path_under_any(&abs_dest, &roots) {
            return Err("destDir must be under an allowlisted build-contexts directory".into());
        }
        let encoding = match args.file_encoding.trim().to_lowercase() {
            e if e.is_empty() => "utf8".to_string(),
            e => e,
        };
        if encoding != "utf8" && encoding != "base64" {
            return Err("fileEncoding must be utf8 or base64".into());
        }
        crate::goerr::mkdir_all(Path::new(&abs_dest), 0o700)
            .map_err(|e| format!("create destDir: {e}"))?;
        let mut written = Vec::new();
        for (rel, content) in &args.files {
            let rel = rel.trim();
            if rel.is_empty() || rel.contains("..") || rel.starts_with('/') {
                return Err(format!("invalid file path {}", quote(rel)));
            }
            let target = clean(&format!("{abs_dest}/{rel}"));
            if !target.starts_with(&format!("{abs_dest}/")) && target != abs_dest {
                return Err(format!("file path escapes destDir: {}", quote(rel)));
            }
            let parent = Path::new(&target)
                .parent()
                .map(PathBuf::from)
                .unwrap_or_default();
            crate::goerr::mkdir_all(&parent, 0o700)
                .map_err(|e| format!("create parent for {rel}: {e}"))?;
            let body = if encoding == "base64" {
                std_base64_decode(content).map_err(|e| format!("decode base64 for {rel}: {e}"))?
            } else {
                content.as_bytes().to_vec()
            };
            write_file(&target, &body, 0o600).map_err(|e| format!("write {rel}: {e}"))?;
            written.push(J::from(rel));
        }
        let mut out = Map::new();
        out.insert("destDir".into(), J::from(abs_dest));
        out.insert("fileCount".into(), J::from(written.len()));
        out.insert("files".into(), J::Array(written));
        out.insert("fileEncoding".into(), J::from(encoding));
        Ok(out)
    }
}

/// `os.WriteFile`: create or truncate with `mode` (before umask).
fn write_file(path: &str, body: &[u8], mode: u32) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(mode)
        .open(path)
        .map_err(|e| crate::goerr::path_error("open", Path::new(path), &e).0)?;
    file.write_all(body)
        .map_err(|e| crate::goerr::path_error("write", Path::new(path), &e).0)
}

// --- build and push (build_and_push.go) ------------------------------------------------

/// `BuildAndPushOciImageArgs` (the dispatcher never sets `untagAfterPush`).
#[derive(Default)]
pub struct BuildArgs {
    pub context_dir: String,
    pub dockerfile: String,
    pub image: String,
    pub builder: String,
    pub insecure_registry: bool,
    pub platform: String,
    pub build_args: Map<String, J>,
}

impl Oci {
    pub fn build_and_push(&self, args: &BuildArgs) -> Result<Map<String, J>, String> {
        let context_dir = args.context_dir.trim();
        let image = args.image.trim();
        if context_dir.is_empty() {
            return Err("contextDir is required".into());
        }
        if image.is_empty() {
            return Err("image is required".into());
        }
        if !safe_image_ref(image) || image.contains([' ', '\t', '\r', '\n']) {
            return Err("image reference contains invalid characters".into());
        }
        let (image_repo, digest_only) = split_image_ref(image);
        let untag_after_push = true;
        let abs_context = absolute(context_dir).map_err(|e| format!("resolve contextDir: {e}"))?;
        if !std::fs::metadata(&abs_context).is_ok_and(|m| m.is_dir()) {
            return Err(format!(
                "contextDir must be an existing directory: {abs_context}"
            ));
        }
        let dockerfile = match args.dockerfile.trim() {
            "" => "Dockerfile",
            d => d,
        };
        let dockerfile_path = if dockerfile.starts_with('/') {
            dockerfile.to_string()
        } else {
            clean(&format!("{abs_context}/{dockerfile}"))
        };
        if std::fs::metadata(&dockerfile_path).is_err() {
            return Err(format!("dockerfile not found: {dockerfile_path}"));
        }
        validate_build_args(&args.build_args)?;

        let info = self.ensure_builder(&args.builder)?;
        let mut builder = info["builder"].as_str().unwrap_or("").to_string();
        let mut adapter: Option<String> = None;
        if builder == "podman" {
            adapter = Some(self.resolve_runtime("podman")?);
        } else if builder == "buildkit" {
            if let Ok(path) = self.resolve_runtime("auto") {
                adapter = Some(path);
                builder = "podman".into();
            } else if self.look_path("buildah").is_some() {
                builder = "buildah".into();
            } else {
                return Err("Podman is required for runtime-backed OCI storage".into());
            }
        }
        let before = self
            .enforce_storage_policy(&builder)
            .map_err(|e| format!("enforce OCI storage policy before build: {e}"))?;

        let platform = args.platform.trim();
        let build_deadline = Instant::now() + Duration::from_secs(45 * 60);
        if let Some(path) = &adapter {
            let mut argv: Vec<String> =
                [path.as_str(), "build", "-f", &dockerfile_path, "-t", image]
                    .iter()
                    .map(|s| s.to_string())
                    .collect();
            if !platform.is_empty() {
                argv.extend(["--platform".to_string(), platform.to_string()]);
            }
            append_build_args(&mut argv, &args.build_args);
            argv.push(abs_context.clone());
            self.run_streaming(&argv, Some(build_deadline))
                .map_err(|e| format!("build image: {e}"))?;
        } else if builder == "buildah" {
            let mut argv: Vec<String> = ["buildah", "bud", "-f", &dockerfile_path, "-t", image]
                .iter()
                .map(|s| s.to_string())
                .collect();
            if !platform.is_empty() {
                argv.extend(["--platform".to_string(), platform.to_string()]);
            }
            append_build_args(&mut argv, &args.build_args);
            argv.push(abs_context.clone());
            self.run_streaming(&argv, Some(build_deadline))
                .map_err(|e| format!("build image: {e}"))?;
        } else {
            return Err(format!(
                "unsupported builder {} for build_and_push_oci_image",
                quote(&builder)
            ));
        }

        let push_deadline = Instant::now() + Duration::from_secs(30 * 60);
        let program = adapter.clone().unwrap_or_else(|| "buildah".into());
        let mut push = vec![program, "push".to_string()];
        if args.insecure_registry {
            push.push("--tls-verify=false".into());
        }
        push.push(image.to_string());
        self.run_streaming(&push, Some(push_deadline))
            .map_err(|e| format!("push image: {e}"))?;

        let mut result = Map::new();
        result.insert("image".into(), J::from(image));
        result.insert("builder".into(), J::from(builder.clone()));
        result.insert("runtime".into(), J::from(builder.clone()));
        result.insert("contextDir".into(), J::from(abs_context));
        result.insert("dockerfile".into(), J::from(dockerfile_path));
        result.insert("insecureRegistry".into(), J::Bool(args.insecure_registry));
        result.insert("pushed".into(), J::Bool(true));
        result.insert("untagAfterPush".into(), J::Bool(untag_after_push));
        match &adapter {
            None => {
                result.insert(
                    "untagSkippedReason".into(),
                    J::from("legacy buildah path has no runtime adapter"),
                );
            }
            Some(_) if digest_only => {
                result.insert(
                    "untagSkippedReason".into(),
                    J::from("digest-pinned reference has no tag to remove"),
                );
            }
            Some(path) => {
                let untag = vec![
                    path.clone(),
                    "image".to_string(),
                    "untag".to_string(),
                    image.to_string(),
                ];
                match self.run_streaming(&untag, None) {
                    Err(e) => {
                        result.insert("untagWarning".into(), J::from(e));
                    }
                    Ok(()) => {
                        result.insert("untaggedImage".into(), J::from(image));
                        result.insert("untaggedRepository".into(), J::from(image_repo));
                    }
                }
            }
        }
        if !args.build_args.is_empty() {
            result.insert("buildArgCount".into(), J::from(args.build_args.len()));
        }
        if let Some(before) = before {
            result.insert("ociStorageBeforeBuild".into(), J::Object(before));
        }
        match self.enforce_storage_policy(&builder) {
            Err(e) => {
                result.insert("ociStoragePolicyWarning".into(), J::from(e));
            }
            Ok(Some(after)) => {
                result.insert("ociStorageAfterBuild".into(), J::Object(after));
            }
            Ok(None) => {}
        }
        Ok(result)
    }
}

// --- storage (storage.go, container_runtime.go) ----------------------------------------

const MIN_AGE_SECONDS: i64 = 60 * 60;
const MAX_AGE_SECONDS: i64 = 365 * 24 * 60 * 60;
const MIN_BUDGET_BYTES: i64 = 1 << 30;

/// `ociStoragePolicy`.
#[derive(Clone, Debug, Default, PartialEq)]
struct Policy {
    runtime: String,
    max_bytes: i64,
    min_age_seconds: i64,
}

fn default_policy() -> Policy {
    Policy {
        min_age_seconds: MIN_AGE_SECONDS,
        ..Policy::default()
    }
}

fn validate_policy(policy: &Policy) -> Result<(), String> {
    if policy.max_bytes < 0 {
        return Err("maxBytes must be zero or positive".into());
    }
    if policy.max_bytes != 0 && policy.max_bytes < MIN_BUDGET_BYTES {
        return Err(format!(
            "maxBytes must be zero or at least {MIN_BUDGET_BYTES}"
        ));
    }
    if policy.min_age_seconds < MIN_AGE_SECONDS || policy.min_age_seconds > MAX_AGE_SECONDS {
        return Err(format!(
            "minAgeSeconds must be between {MIN_AGE_SECONDS} and {MAX_AGE_SECONDS}"
        ));
    }
    if !policy.runtime.is_empty() && policy.runtime != "auto" && policy.runtime != "podman" {
        return Err("runtime must be one of auto or podman".into());
    }
    Ok(())
}

fn validate_cleanup(policy: &Policy, max_bytes: Option<i64>) -> Result<(), String> {
    if policy.min_age_seconds < MIN_AGE_SECONDS || policy.min_age_seconds > MAX_AGE_SECONDS {
        return Err(format!(
            "minAgeSeconds must be between {MIN_AGE_SECONDS} and {MAX_AGE_SECONDS}"
        ));
    }
    if let Some(max) = max_bytes {
        if max < 0 {
            return Err("maxBytes must be zero or positive".into());
        }
        if max != 0 && max < MIN_BUDGET_BYTES {
            return Err(format!(
                "maxBytes must be zero or at least {MIN_BUDGET_BYTES}"
            ));
        }
    }
    Ok(())
}

/// `json.Unmarshal` into `ociStoragePolicy`: field names match
/// case-insensitively, unknown fields are ignored, a mistyped field fails.
fn decode_policy(data: &[u8], mut policy: Policy) -> Result<Policy, String> {
    if let Some(e) = crate::gojson::syntax_error(data) {
        return Err(e);
    }
    let value: J = serde_json::from_slice(data).map_err(|e| e.to_string())?;
    let J::Object(object) = value else {
        if value.is_null() {
            return Ok(policy);
        }
        return Err(format!(
            "json: cannot unmarshal {} into Go value of type oci.ociStoragePolicy",
            kind_name(&value)
        ));
    };
    for (key, item) in &object {
        let field = ["runtime", "maxBytes", "minAgeSeconds"]
            .into_iter()
            .find(|f| crate::gojson::equal_fold(f, key));
        let Some(field) = field else { continue };
        if item.is_null() {
            continue;
        }
        let mistyped = |go: &str| {
            format!(
                "json: cannot unmarshal {} into Go struct field ociStoragePolicy.{field} of type {go}",
                kind_name(item)
            )
        };
        match field {
            "runtime" => {
                policy.runtime = item.as_str().ok_or_else(|| mistyped("string"))?.to_string();
            }
            "maxBytes" => policy.max_bytes = item.as_i64().ok_or_else(|| mistyped("int64"))?,
            _ => policy.min_age_seconds = item.as_i64().ok_or_else(|| mistyped("int64"))?,
        }
    }
    Ok(policy)
}

/// `json.MarshalIndent(policy, "", "  ")`.
fn encode_policy(policy: &Policy) -> String {
    let mut compact = String::from("{");
    if !policy.runtime.is_empty() {
        compact.push_str("\"runtime\":");
        crate::gojson::encode_string(&policy.runtime, &mut compact);
        compact.push(',');
    }
    compact.push_str(&format!(
        "\"maxBytes\":{},\"minAgeSeconds\":{}}}",
        policy.max_bytes, policy.min_age_seconds
    ));
    crate::gojson::indent(&compact, "  ")
}

/// `containerStorageCategory`.
#[derive(Clone, Default)]
struct Category {
    supported: bool,
    bytes: i64,
    reclaimable_bytes: i64,
    reason: String,
}

/// `containerStorageReport`.
#[derive(Clone, Default)]
struct Report {
    runtime: String,
    executable: String,
    storage_path: String,
    storage_driver: String,
    categories: std::collections::BTreeMap<String, Category>,
    total_bytes: i64,
    reclaimable_bytes: i64,
    unsupported: Option<Vec<String>>,
    warnings: Option<Vec<String>>,
    policy: Policy,
}

fn strings_or_null(values: &Option<Vec<String>>) -> J {
    match values {
        Some(values) => J::Array(values.iter().cloned().map(J::from).collect()),
        None => J::Null,
    }
}

impl Report {
    fn category(&self, name: &str) -> Category {
        self.categories.get(name).cloned().unwrap_or_default()
    }

    /// `containerStorageReport.toMap`.
    fn to_map(&self) -> Map<String, J> {
        let mut categories = Map::new();
        for name in ["images", "containers", "volumes", "buildCache"] {
            let category = self.category(name);
            let mut entry = Map::new();
            entry.insert("supported".into(), J::Bool(category.supported));
            entry.insert("bytes".into(), J::from(category.bytes));
            entry.insert(
                "reclaimableBytes".into(),
                J::from(category.reclaimable_bytes),
            );
            if !category.reason.is_empty() {
                entry.insert("reason".into(), J::from(category.reason));
            }
            categories.insert(name.into(), J::Object(entry));
        }
        let mut out = Map::new();
        out.insert("runtime".into(), J::from(self.runtime.clone()));
        out.insert("executable".into(), J::from(self.executable.clone()));
        out.insert("storagePath".into(), J::from(self.storage_path.clone()));
        out.insert("storageDriver".into(), J::from(self.storage_driver.clone()));
        out.insert("accounting".into(), J::from("runtime-reported"));
        out.insert("categories".into(), J::Object(categories));
        out.insert("totalBytes".into(), J::from(self.total_bytes));
        out.insert("reclaimableBytes".into(), J::from(self.reclaimable_bytes));
        out.insert(
            "unsupportedCategories".into(),
            strings_or_null(&self.unsupported),
        );
        out.insert("warnings".into(), strings_or_null(&self.warnings));
        out.insert(
            "policy".into(),
            json!({"maxBytes": self.policy.max_bytes, "minAgeSeconds": self.policy.min_age_seconds}),
        );
        out
    }
}

/// The JSON kind `json.UnmarshalTypeError` names.
fn kind_name(value: &J) -> &'static str {
    match value {
        J::Object(_) => "object",
        J::Array(_) => "array",
        J::String(_) => "string",
        J::Bool(_) => "bool",
        J::Null => "null",
        J::Number(_) => "number",
    }
}

/// `json.Unmarshal` of runtime output: Go's error text for invalid JSON.
fn unmarshal(data: &str) -> Result<J, String> {
    if data.is_empty() {
        return Err("unexpected end of JSON input".into());
    }
    if let Some(e) = crate::gojson::syntax_error(data.as_bytes()) {
        return Err(e);
    }
    serde_json::from_str(data).map_err(|e| e.to_string())
}

/// `parseJSONMap`.
fn parse_json_map(output: &[u8]) -> Result<Map<String, J>, String> {
    match unmarshal(String::from_utf8_lossy(output).trim())? {
        J::Object(object) => Ok(object),
        J::Null => Ok(Map::new()),
        other => Err(format!(
            "json: cannot unmarshal {} into Go value of type map[string]interface {{}}",
            kind_name(&other)
        )),
    }
}

fn as_entry(value: &J, ty: &str) -> Result<Map<String, J>, String> {
    match value {
        J::Object(object) => Ok(object.clone()),
        J::Null => Ok(Map::new()),
        other => Err(format!(
            "json: cannot unmarshal {} into Go value of type {ty}",
            kind_name(other)
        )),
    }
}

/// `parseStorageEntries`.
fn parse_storage_entries(output: &[u8]) -> Result<Vec<Map<String, J>>, String> {
    let trimmed = String::from_utf8_lossy(output).trim().to_string();
    if trimmed.is_empty() {
        return Err("empty storage usage output".into());
    }
    if trimmed.starts_with('[') {
        let value = unmarshal(&trimmed)?;
        let J::Array(items) = value else {
            unreachable!("a JSON value starting with [ is an array")
        };
        return items
            .iter()
            .map(|item| as_entry(item, "oci.containerJSONEntry"))
            .collect();
    }
    if trimmed.starts_with('{') {
        if let Ok(J::Object(object)) = unmarshal(&trimmed) {
            if object.contains_key("Type") {
                return Ok(vec![object]);
            }
            let mut entries = Vec::new();
            for (key, raw) in object {
                if let J::Object(mut entry) = raw {
                    entry.insert("Type".into(), J::from(key));
                    entries.push(entry);
                }
            }
            if !entries.is_empty() {
                return Ok(entries);
            }
        }
    }
    let mut entries = Vec::new();
    for line in trimmed.split('\n') {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut entry = as_entry(&unmarshal(line)?, "oci.containerJSONEntry")?;
        if !entry.contains_key("Type") && entry.len() == 1 {
            let (key, raw) = entry
                .iter()
                .next()
                .map(|(k, v)| (k.clone(), v.clone()))
                .unwrap();
            if let J::Object(mut nested) = raw {
                nested.insert("Type".into(), J::from(key));
                entry = nested;
            }
        }
        entries.push(entry);
    }
    if entries.is_empty() {
        return Err("no storage usage entries".into());
    }
    Ok(entries)
}

/// `normalizeStorageCategory`.
fn normalize_category(value: &str) -> &'static str {
    let key: String = value
        .trim()
        .chars()
        .filter(|c| !matches!(c, ' ' | '-' | '_'))
        .collect::<String>()
        .to_lowercase();
    match key.as_str() {
        "images" | "image" => "images",
        "containers" | "container" => "containers",
        "volumes" | "volume" | "localvolumes" => "volumes",
        "buildcache" | "cache" => "buildCache",
        _ => "",
    }
}

/// `firstString`.
fn first_string(value: &Map<String, J>, keys: &[&str]) -> String {
    keys.iter()
        .find_map(|key| {
            value
                .get(*key)
                .and_then(J::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
        })
        .unwrap_or("")
        .to_string()
}

/// `firstStringSlice`.
fn first_string_slice(value: &Map<String, J>, keys: &[&str]) -> Vec<String> {
    for key in keys {
        let Some(J::Array(raw)) = value.get(*key) else {
            continue;
        };
        let names: Vec<String> = raw
            .iter()
            .filter_map(J::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        if !names.is_empty() {
            return names;
        }
    }
    Vec::new()
}

/// `parseByteMetric`.
fn parse_byte_metric(value: &J) -> Option<i64> {
    match value {
        J::Number(n) => n.as_f64().map(|f| f as i64),
        J::String(s) => {
            let s = s.trim_start();
            let digits_end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
            if digits_end == 0 {
                return None;
            }
            let mut end = digits_end;
            if s[end..].starts_with('.') {
                let fraction = s[end + 1..]
                    .find(|c: char| !c.is_ascii_digit())
                    .unwrap_or(s.len() - end - 1);
                if fraction > 0 {
                    end += 1 + fraction;
                }
            }
            let number: f64 = s[..end].parse().ok()?;
            let rest = s[end..].trim_start().to_lowercase();
            let unit_len = {
                let b = rest.as_bytes();
                let mut i = 0;
                if i < b.len() && b"kmgtpe".contains(&b[i]) {
                    i += 1;
                }
                if i < b.len() && b[i] == b'i' {
                    i += 1;
                }
                if i < b.len() && b[i] == b'b' {
                    i + 1
                } else {
                    0
                }
            };
            let multiplier = match &rest[..unit_len] {
                "kb" => 1e3,
                "kib" => 1024.0,
                "mb" => 1e6,
                "mib" => 1_048_576.0,
                "gb" => 1e9,
                "gib" => 1_073_741_824.0,
                "tb" => 1e12,
                "tib" => 1_099_511_627_776.0,
                "pb" => 1e15,
                "pib" => 1_125_899_906_842_624.0,
                _ => 1.0,
            };
            Some((number * multiplier) as i64)
        }
        _ => None,
    }
}

/// `firstInt64`.
fn first_int64(value: &Map<String, J>, keys: &[&str]) -> i64 {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(parse_byte_metric))
        .unwrap_or(0)
}

fn bool_value(value: Option<&J>) -> bool {
    value == Some(&J::Bool(true))
}

/// `parseBuildCacheUsage`.
fn parse_build_cache(output: &[u8]) -> Result<(i64, i64), String> {
    let mut total = 0;
    let mut reclaimable = 0;
    for entry in parse_storage_entries(output)? {
        let size = first_int64(&entry, &["Size", "size", "RawSize", "rawSize"]);
        total += size;
        if bool_value(entry.get("Reclaimable")) || bool_value(entry.get("reclaimable")) {
            reclaimable += size;
        } else {
            let value = first_int64(&entry, &["RawReclaimable", "rawReclaimable"]);
            if value > 0 {
                reclaimable += value;
            }
        }
    }
    Ok((total, reclaimable))
}

/// `normalizeContainerStorageReport`.
fn normalize_report(
    runtime: &str,
    executable: &str,
    info: &[u8],
    storage: &[u8],
    cache: &[u8],
    policy: &Policy,
) -> Result<Report, String> {
    let info =
        parse_json_map(info).map_err(|e| format!("parse {runtime} runtime information: {e}"))?;
    let mut report = Report {
        runtime: runtime.into(),
        executable: executable.into(),
        policy: policy.clone(),
        ..Report::default()
    };
    report.storage_path = first_string(
        &info,
        &["DockerRootDir", "GraphRoot", "graphRoot", "RootDir"],
    );
    report.storage_driver = first_string(&info, &["Driver", "GraphDriverName", "graphDriverName"]);
    let store = ["Store", "store"]
        .iter()
        .find_map(|key| info.get(*key).and_then(J::as_object));
    if let Some(store) = store {
        if report.storage_path.is_empty() {
            report.storage_path =
                first_string(store, &["GraphRoot", "graphRoot", "RunRoot", "runRoot"]);
        }
        if report.storage_driver.is_empty() {
            report.storage_driver =
                first_string(store, &["GraphDriverName", "graphDriverName", "Driver"]);
        }
    }
    let entries = parse_storage_entries(storage)
        .map_err(|e| format!("parse {runtime} storage usage: {e}"))?;
    for entry in entries {
        let name = normalize_category(&first_string(&entry, &["Type", "type", "Name", "name"]));
        if name.is_empty() {
            continue;
        }
        report.categories.insert(
            name.into(),
            Category {
                supported: true,
                bytes: first_int64(
                    &entry,
                    &["RawSize", "rawSize", "Size", "size", "Total", "total"],
                ),
                reclaimable_bytes: first_int64(
                    &entry,
                    &[
                        "RawReclaimable",
                        "rawReclaimable",
                        "Reclaimable",
                        "reclaimable",
                    ],
                ),
                reason: String::new(),
            },
        );
    }
    for name in ["images", "containers", "volumes"] {
        report.categories.entry(name.into()).or_insert(Category {
            supported: true,
            ..Category::default()
        });
    }
    if !report.categories.contains_key("buildCache") {
        report.categories.insert(
            "buildCache".into(),
            Category {
                supported: false,
                reason: "runtime does not expose a safe machine-readable build-cache operation"
                    .into(),
                ..Category::default()
            },
        );
        report.unsupported = Some(vec!["buildCache".into()]);
        report.warnings = Some(vec![
            "build-cache inspection and pruning is unavailable for this runtime".into(),
        ]);
    }
    if !cache.is_empty() {
        if let Ok((bytes, reclaimable)) = parse_build_cache(cache) {
            report.categories.insert(
                "buildCache".into(),
                Category {
                    supported: true,
                    bytes,
                    reclaimable_bytes: reclaimable,
                    reason: String::new(),
                },
            );
            report.unsupported = None;
            report.warnings = None;
        }
    }
    for category in report.categories.values() {
        if category.supported {
            report.total_bytes += category.bytes;
            report.reclaimable_bytes += category.reclaimable_bytes;
        }
    }
    Ok(report)
}

/// `containerImage`.
#[derive(Clone)]
struct Image {
    id: String,
    created: i64,
    containers: i64,
    size: i64,
    names: Vec<String>,
}

/// `parseImageTime`: the layouts Go tries, as Unix seconds.
fn parse_image_time(value: &str) -> i64 {
    let value = value.trim();
    if value.is_empty() {
        return 0;
    }
    let seconds = |text: &str| {
        crate::clusterops::parse_rfc3339(text).map(|nanos| nanos.div_euclid(1_000_000_000) as i64)
    };
    if let Some(seconds) = seconds(value) {
        return seconds;
    }
    // "2006-01-02 15:04:05 -0700 MST" and "2006-01-02 15:04:05 -0700".
    let mut parts = value.split(' ');
    let (Some(date), Some(clock), Some(zone)) = (parts.next(), parts.next(), parts.next()) else {
        return 0;
    };
    let rest: Vec<&str> = parts.collect();
    if rest.len() > 1 {
        return 0;
    }
    let candidate = format!(
        "{date}T{clock}{}:{}",
        &zone.get(..3).unwrap_or(""),
        &zone.get(3..).unwrap_or("")
    );
    if zone.len() != 5 || !(zone.starts_with('+') || zone.starts_with('-')) {
        return 0;
    }
    seconds(&candidate).unwrap_or(0)
}

/// `parseContainerImages`.
fn parse_images(output: &[u8]) -> Result<Vec<Image>, String> {
    Ok(parse_storage_entries(output)?
        .iter()
        .map(|entry| {
            let mut created = first_int64(entry, &["Created", "created"]);
            if created == 0 {
                created = parse_image_time(&first_string(entry, &["CreatedAt", "createdAt"]));
            }
            Image {
                id: first_string(entry, &["Id", "ID", "id"]),
                created,
                containers: first_int64(entry, &["Containers", "containers"]),
                size: first_int64(entry, &["Size", "size"]),
                names: first_string_slice(entry, &["Names", "names", "RepoTags", "repoTags"]),
            }
        })
        .collect())
}

/// `selectOciPruneCandidates`.
fn prune_candidates(images: &[Image], cutoff: i64) -> Vec<Image> {
    let mut candidates: Vec<Image> = images
        .iter()
        .filter(|image| {
            !image.id.trim().is_empty()
                && image.containers == 0
                && image.created > 0
                && image.created < cutoff
        })
        .cloned()
        .collect();
    candidates.sort_by_key(|image| image.created);
    candidates
}

/// `describeSkippedImage`.
fn describe_skipped(image: &Image, err: &str) -> String {
    if image.names.len() > 1 {
        return format!(
            "skipped image {}: still referenced by {}; retire a tag deliberately, cleanup never force-removes a shared image",
            image.id,
            image.names.join(", ")
        );
    }
    format!("skipped image {}: {err}", image.id)
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

impl Oci {
    fn load_policy(&self) -> Result<Policy, String> {
        let policy = default_policy();
        let path = self.storage_policy_path.trim();
        if path.is_empty() {
            return Ok(policy);
        }
        let data = match std::fs::read(path) {
            Ok(data) => data,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(policy),
            Err(e) => {
                return Err(format!(
                    "read OCI storage policy: {}",
                    crate::goerr::path_error("open", Path::new(path), &e).0
                ))
            }
        };
        let mut policy =
            decode_policy(&data, policy).map_err(|e| format!("parse OCI storage policy: {e}"))?;
        if policy.min_age_seconds == 0 {
            policy.min_age_seconds = MIN_AGE_SECONDS;
        }
        validate_policy(&policy).map_err(|e| format!("invalid OCI storage policy: {e}"))?;
        Ok(policy)
    }

    fn save_policy(&self, policy: &Policy) -> Result<(), String> {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;
        validate_policy(policy)?;
        let path = self.storage_policy_path.trim();
        if path.is_empty() {
            return Err("OCI storage policy path is not configured".into());
        }
        let data = encode_policy(policy);
        let dir = Path::new(path)
            .parent()
            .map(PathBuf::from)
            .unwrap_or_default();
        crate::goerr::mkdir_all(&dir, 0o700)
            .map_err(|e| format!("create OCI storage policy directory: {e}"))?;
        let (mut tmp, tmp_path) =
            crate::goerr::create_temp(&dir.display().to_string(), ".oci-storage-policy-", ".tmp")
                .map_err(|e| format!("create OCI storage policy temporary file: {e}"))?;
        let cleanup = scopeguard(tmp_path.clone());
        tmp.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|e| {
                format!(
                    "protect OCI storage policy: {}",
                    crate::goerr::path_error("chmod", Path::new(&tmp_path), &e).0
                )
            })?;
        tmp.write_all(data.as_bytes()).map_err(|e| {
            format!(
                "write OCI storage policy: {}",
                crate::goerr::path_error("write", Path::new(&tmp_path), &e).0
            )
        })?;
        drop(tmp);
        std::fs::rename(&tmp_path, path).map_err(|e| {
            format!(
                "install OCI storage policy: rename {tmp_path} {path}: {}",
                crate::goerr::errno_text(&e)
            )
        })?;
        drop(cleanup);
        Ok(())
    }

    /// `podmanRuntimeAdapter.Inspect`.
    fn inspect(&self, path: &str, policy: &Policy) -> Result<Report, String> {
        let info = self
            .podman(path, &["info", "--format", "json"])
            .map_err(|e| format!("inspect Podman runtime: {e}"))?;
        let storage = self
            .podman(path, &["system", "df", "--format", "json"])
            .map_err(|e| format!("inspect Podman storage: {e}"))?;
        normalize_report("podman", path, &info, &storage, &[], policy)
    }

    fn list_images(&self, path: &str) -> Result<Vec<Image>, String> {
        let output = self
            .podman(path, &["images", "--format", "json"])
            .map_err(|e| format!("list Podman images: {e}"))?;
        parse_images(&output)
    }

    pub fn inspect_storage(&self, runtime: &str) -> Result<Map<String, J>, String> {
        let mut policy = self.load_policy()?;
        if !runtime.trim().is_empty() {
            policy.runtime = runtime.trim().to_lowercase();
        }
        let path = self.resolve_runtime(&policy.runtime)?;
        Ok(self.inspect(&path, &policy)?.to_map())
    }

    pub fn cleanup_storage(
        &self,
        runtime: &str,
        max_bytes: Option<i64>,
        min_age_seconds: Option<i64>,
        dry_run: bool,
    ) -> Result<Map<String, J>, String> {
        crate::hostfiles::require_shared_host_owner(
            &self.instance_id,
            &self.shared_host_owner_instance,
            "cleanup_container_storage",
        )?;
        let mut policy = self.load_policy()?;
        if !runtime.is_empty() {
            policy.runtime = runtime.trim().to_lowercase();
        }
        if let Some(age) = min_age_seconds {
            policy.min_age_seconds = age;
        }
        validate_cleanup(&policy, max_bytes)?;
        let path = self.resolve_runtime(&policy.runtime)?;
        let _guard = self.storage_mu.lock().unwrap_or_else(|e| e.into_inner());
        self.cleanup_locked(&path, policy, max_bytes, dry_run)
    }

    /// `cleanupContainerStorageLocked`.
    fn cleanup_locked(
        &self,
        path: &str,
        mut policy: Policy,
        max_bytes: Option<i64>,
        dry_run: bool,
    ) -> Result<Map<String, J>, String> {
        if let Some(max) = max_bytes {
            policy.max_bytes = max;
        }
        let before = self.inspect(path, &policy)?;
        let mut result = Map::new();
        result.insert("runtime".into(), J::from("podman"));
        result.insert("cleanupScope".into(), json!(["images", "buildCache"]));
        result.insert("dryRun".into(), J::Bool(dry_run));
        result.insert("pruneAttempted".into(), J::Bool(false));
        result.insert("prunedImageCount".into(), J::from(0));
        result.insert("estimatedReclaimableBytes".into(), J::from(0));
        result.insert("maxBytes".into(), J::from(policy.max_bytes));
        result.insert("minAgeSeconds".into(), J::from(policy.min_age_seconds));
        result.insert("before".into(), J::Object(before.to_map()));
        let mut warnings: Vec<String> = before.warnings.clone().unwrap_or_default();
        let cutoff = now_unix() - policy.min_age_seconds;
        let images = self.list_images(path)?;
        let candidates = prune_candidates(&images, cutoff);
        result.insert("candidateImageCount".into(), J::from(candidates.len()));
        let estimated_image_bytes: i64 = candidates.iter().map(|image| image.size).sum();
        result.insert("estimatedImageBytes".into(), J::from(estimated_image_bytes));
        let budget = max_bytes.unwrap_or(0);
        let mut current = before.clone();
        let mut pruned = 0i64;
        let mut attempted = false;
        let mut estimated = 0i64;
        if dry_run {
            estimated = estimated_image_bytes + before.category("buildCache").reclaimable_bytes;
        } else {
            for image in &candidates {
                if budget > 0 && current.total_bytes <= budget {
                    break;
                }
                if let Err(e) = self.podman(path, &["image", "rm", &image.id]) {
                    warnings.push(describe_skipped(image, &e));
                    continue;
                }
                attempted = true;
                pruned += 1;
                match self.inspect(path, &policy) {
                    Ok(refreshed) => current = refreshed,
                    Err(e) => {
                        warnings.push(format!("refresh storage after image {}: {e}", image.id));
                        break;
                    }
                }
                if budget > 0 && current.total_bytes <= budget {
                    break;
                }
            }
        }
        let mut after = current;
        if !dry_run {
            match self.inspect(path, &policy) {
                Ok(refreshed) => after = refreshed,
                Err(e) => warnings.push(format!("refresh storage after image cleanup: {e}")),
            }
        }
        let cache_supported = after.category("buildCache").supported;
        if dry_run {
            result.insert("buildCachePruneSupported".into(), J::Bool(cache_supported));
        } else if cache_supported || budget == 0 {
            // `podmanRuntimeAdapter.PruneBuildCache` always refuses.
            warnings.push(
                "Podman build-cache inspection and pruning is unavailable through a safe machine-readable operation"
                    .into(),
            );
        }
        if !dry_run {
            match self.inspect(path, &policy) {
                Ok(refreshed) => after = refreshed,
                Err(e) => warnings.push(format!("refresh storage after cache cleanup: {e}")),
            }
        }
        result.insert("pruneAttempted".into(), J::Bool(attempted));
        result.insert("prunedImageCount".into(), J::from(pruned));
        result.insert("estimatedReclaimableBytes".into(), J::from(estimated));
        result.insert("after".into(), J::Object(after.to_map()));
        if !warnings.is_empty() {
            result.insert(
                "warnings".into(),
                J::Array(warnings.into_iter().map(J::from).collect()),
            );
        }
        Ok(result)
    }

    pub fn configure_storage(
        &self,
        runtime: &str,
        max_bytes: Option<i64>,
        min_age_seconds: Option<i64>,
        prune_now: bool,
    ) -> Result<Map<String, J>, String> {
        crate::hostfiles::require_shared_host_owner(
            &self.instance_id,
            &self.shared_host_owner_instance,
            "configure_oci_storage",
        )?;
        let mut policy = self.load_policy()?;
        if !runtime.is_empty() {
            policy.runtime = runtime.trim().to_lowercase();
        }
        if let Some(max) = max_bytes {
            policy.max_bytes = max;
        }
        if let Some(age) = min_age_seconds {
            policy.min_age_seconds = age;
        }
        validate_policy(&policy)?;
        let _guard = self.storage_mu.lock().unwrap_or_else(|e| e.into_inner());
        self.save_policy(&policy)?;
        let path = self.resolve_runtime(&policy.runtime)?;
        if !prune_now {
            let mut out = self.inspect(&path, &policy)?.to_map();
            out.insert("policyUpdated".into(), J::Bool(true));
            return Ok(out);
        }
        let max = policy.max_bytes;
        self.cleanup_locked(&path, policy, Some(max), false)
    }

    /// `enforceOciStoragePolicy`: run by image builds only.
    fn enforce_storage_policy(&self, builder: &str) -> Result<Option<Map<String, J>>, String> {
        if self.storage_policy_path.trim().is_empty() || (builder != "podman" && builder != "auto")
        {
            return Ok(None);
        }
        let _guard = self.storage_mu.lock().unwrap_or_else(|e| e.into_inner());
        let policy = self.load_policy()?;
        if policy.max_bytes == 0 {
            return Ok(None);
        }
        let path = self.resolve_runtime(builder)?;
        let max = policy.max_bytes;
        self.cleanup_locked(&path, policy, Some(max), false)
            .map(Some)
    }
}

/// `defer os.Remove(tmpPath)`.
struct TempGuard(String);

impl Drop for TempGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn scopeguard(path: String) -> TempGuard {
    TempGuard(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_refs_split_like_go() {
        assert_eq!(
            split_image_ref("reg:5000/app:v1"),
            ("reg:5000/app".into(), false)
        );
        assert_eq!(
            split_image_ref("reg:5000/app"),
            ("reg:5000/app".into(), false)
        );
        assert_eq!(split_image_ref("app@sha256:ab"), ("app".into(), true));
        assert!(safe_image_ref("reg:5000/app:v1"));
        assert!(!safe_image_ref("-app"));
    }

    #[test]
    fn byte_metrics_parse_like_go() {
        assert_eq!(parse_byte_metric(&json!("1.5GB")), Some(1_500_000_000));
        assert_eq!(parse_byte_metric(&json!("2 KiB")), Some(2048));
        assert_eq!(parse_byte_metric(&json!("12")), Some(12));
        assert_eq!(parse_byte_metric(&json!("x")), None);
        assert_eq!(parse_byte_metric(&json!(3.9)), Some(3));
    }

    #[test]
    fn policy_round_trips_as_go_encodes_it() {
        let policy = Policy {
            runtime: "podman".into(),
            max_bytes: 1 << 30,
            min_age_seconds: 3600,
        };
        assert_eq!(
            encode_policy(&policy),
            "{\n  \"runtime\": \"podman\",\n  \"maxBytes\": 1073741824,\n  \"minAgeSeconds\": 3600\n}"
        );
        let decoded = decode_policy(encode_policy(&policy).as_bytes(), default_policy()).unwrap();
        assert_eq!(decoded, policy);
        let folded = decode_policy(br#"{"MAXBYTES": 0}"#, default_policy()).unwrap();
        assert_eq!(folded.min_age_seconds, MIN_AGE_SECONDS);
    }

    #[test]
    fn storage_entries_accept_every_shape_go_does() {
        let array = parse_storage_entries(br#"[{"Type":"Images","RawSize":10}]"#).unwrap();
        assert_eq!(array.len(), 1);
        let object = parse_storage_entries(br#"{"Images":{"Size":"1kB"}}"#).unwrap();
        assert_eq!(object[0]["Type"], "Images");
        let lines =
            parse_storage_entries(b"{\"Type\":\"Images\"}\n{\"Volumes\":{\"Size\":1}}").unwrap();
        assert_eq!(lines[1]["Type"], "Volumes");
    }
}
