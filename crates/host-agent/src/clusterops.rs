//! M8c: Go's `internal/domain/kubernetes` operations, as the built-in
//! provider of the cluster services (decision D14).
//!
//! The Host Agent never runs kubectl itself. Every cluster access is one
//! synchronous call to the single active provider that declares the
//! `kubernetes` capability (`hostmcp/kubernetes_provider.go`), reached
//! through the [`Executor`] seam; this module owns admission-time argument
//! validation, the canonical target, and the result shaping Go performs
//! around those calls. `helm template` is the one host command: it runs on
//! the Host Agent machine through the host's `ensure_host_tool`.

use crate::admission::Resolver;
use crate::goerr::quote;
use crate::hostobs::go_duration;
use crate::incus::first_non_empty;
use crate::resource::Uri;
use serde_json::{json, Map, Value as J};
use std::sync::Arc;
use std::time::Duration;

/// `kubernetesProviderExecutor.Execute`: run one provider operation with
/// the arguments given (the target fields already merged in).
pub type Executor =
    Arc<dyn Fn(&str, Map<String, J>) -> Result<Map<String, J>, String> + Send + Sync>;

/// `Shared.RegisterResource`; failures are ignored where Go ignores them.
pub type Register = Arc<dyn Fn(&str, Map<String, J>) -> Result<(), String> + Send + Sync>;

/// `Deps.EnsureHostTool`: install or verify a host binary, reporting its path.
pub type EnsureTool = Arc<dyn Fn(&str) -> Result<J, String> + Send + Sync>;

/// `Shared.HostCommandRunner`: argv on the host, with a timeout.
pub type RunHost = Arc<dyn Fn(&[String], Duration) -> crate::hostobs::CommandResult + Send + Sync>;

pub const APPLY_MANIFEST: &str = "opute.capability.kubernetes.apply-manifest";
pub const PUT_SECRET: &str = "opute.capability.kubernetes.put-secret";
pub const GET_RESOURCE: &str = "opute.capability.kubernetes.get-resource";
pub const DELETE_RESOURCE: &str = "opute.capability.kubernetes.delete-resource";
pub const GET_RESOURCE_STATUS: &str = "opute.capability.kubernetes.get-resource-status";
pub const LIST_EVENTS: &str = "opute.capability.kubernetes.list-events";
pub const LIST_CLUSTERS: &str = "opute.capability.kubernetes.list-clusters";
pub const EXEC_COMMAND: &str = "opute.capability.kubernetes.exec-command";
pub const INSPECT_GUEST_STORAGE: &str = "opute.capability.kubernetes.inspect-guest-storage";
pub const PRUNE_UNUSED_IMAGES: &str = "opute.capability.kubernetes.prune-unused-images";
pub const GARBAGE_COLLECT_REGISTRY: &str = "opute.capability.kubernetes.garbage-collect-registry";
pub const TRIM_GUEST_STORAGE: &str = "opute.capability.kubernetes.trim-guest-storage";

/// The Kubernetes provider's capability id (`capabilitycontract.Kubernetes`).
pub const CAPABILITY: &str = "opute.capability.kubernetes.v1";

/// The kubernetes domain over the seams it declares.
#[derive(Clone)]
pub struct Kubernetes {
    pub execute: Executor,
    pub resolve: Resolver,
    /// `Shared.TenantID` as configured (possibly empty).
    pub tenant_id: String,
    pub register: Register,
    pub ensure_tool: EnsureTool,
    pub run_host: RunHost,
    /// The process environment `exec.LookPath` searches.
    pub env: crate::config::Env,
}

/// `k8sname.Validate`: `^[A-Za-z0-9][A-Za-z0-9_.:-]*$` after trimming.
pub fn validate_name(value: &str, field: &str) -> Result<(), String> {
    let value = value.trim();
    let ok = value.bytes().enumerate().all(|(i, c)| {
        c.is_ascii_alphanumeric() || (i > 0 && matches!(c, b'_' | b'.' | b':' | b'-'))
    });
    if value.is_empty() || !ok {
        return Err(format!("{field} contains invalid characters"));
    }
    Ok(())
}

/// `clusterIDFromURI`: the cluster's own identifier, or the URI itself.
pub fn cluster_id_from_uri(uri: &str) -> String {
    Uri::parse(uri).map_or_else(|_| uri.to_string(), |parsed| parsed.resource_id)
}

fn strings(values: &[String]) -> J {
    J::Array(values.iter().cloned().map(J::from).collect())
}

impl Kubernetes {
    /// `Shared.EffectiveTenantID`.
    fn effective_tenant(&self) -> &str {
        match self.tenant_id.trim() {
            "" => "local",
            tenant => tenant,
        }
    }

    /// `Service.TargetURI`: a provider instance name's canonical cluster URI.
    pub fn target_uri(&self, name: &str) -> Result<String, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("kubernetes provider instance name is required".into());
        }
        let uri = Uri::new("cluster", self.effective_tenant(), name)?;
        let (resolved, _) = (self.resolve)(&uri.to_string(), "cluster")
            .map_err(|e| format!("resolve Kubernetes target {uri}: {e}"))?;
        Ok(resolved.to_string())
    }

    /// `Service.ExecuteProvider`: the canonical target's coordinates join the
    /// arguments, then the active provider runs the operation.
    pub fn execute_provider(
        &self,
        operation: &str,
        target_uri: &str,
        arguments: Map<String, J>,
    ) -> Result<Map<String, J>, String> {
        let target_uri = target_uri.trim();
        if target_uri.is_empty() {
            return Err("canonical cluster URI is required".into());
        }
        let (uri, coordinates) = (self.resolve)(target_uri, "cluster")?;
        let text = |key: &str| {
            coordinates
                .get(key)
                .and_then(J::as_str)
                .unwrap_or("")
                .trim()
                .to_string()
        };
        let provider = text("providerInstanceName");
        if provider.is_empty() {
            return Err(format!("cluster {uri} has no provider instance coordinate"));
        }
        let mut arguments = arguments;
        arguments.insert("targetUri".into(), J::from(uri.to_string()));
        arguments.insert("providerInstanceName".into(), J::from(provider));
        let instance_type = text("instanceType");
        if !instance_type.is_empty() {
            arguments.insert("instanceType".into(), J::from(instance_type));
        }
        (self.execute)(operation, arguments)
    }

    /// `runProviderCommand`: kubectl through the provider's exec-command.
    /// A failure to name the target is reported as is; a failed execution
    /// carries the call's label.
    pub fn run_kubectl(
        &self,
        name: &str,
        kubectl_args: &[String],
        label: &str,
    ) -> Result<String, String> {
        self.run_kubectl_input(name, kubectl_args, None, label)
    }

    /// `runProviderCommand` with stdin (`RunKubectlWithStdinContext`): the
    /// input travels as the operation's `stdin` argument.
    pub fn run_kubectl_input(
        &self,
        name: &str,
        kubectl_args: &[String],
        input: Option<&[u8]>,
        label: &str,
    ) -> Result<String, String> {
        let target = self.target_uri(name)?;
        let mut args = Map::new();
        args.insert("kubectlArgs".into(), strings(kubectl_args));
        if let Some(input) = input {
            args.insert(
                "stdin".into(),
                J::from(String::from_utf8_lossy(input).into_owned()),
            );
        }
        let out = self
            .execute_provider(EXEC_COMMAND, &target, args)
            .map_err(|e| format!("failed to {label} in {name}: {e}"))?;
        Ok(out
            .get("stdout")
            .and_then(J::as_str)
            .unwrap_or("")
            .trim()
            .to_string())
    }

    pub fn apply_manifest(&self, uri: &str, manifest: &str) -> Result<Map<String, J>, String> {
        if uri.trim().is_empty() {
            return Err("uri is required".into());
        }
        let manifest = manifest.trim();
        if manifest.is_empty() {
            return Err("manifest is required".into());
        }
        if manifest.len() > 4 * 1024 * 1024 {
            return Err("manifest exceeds the 4 MiB limit".into());
        }
        let mut args = Map::new();
        args.insert("manifest".into(), J::from(manifest));
        self.execute_provider(APPLY_MANIFEST, uri, args)
    }

    pub fn delete_resource(&self, args: &ResourceArgs) -> Result<Map<String, J>, String> {
        let (kind, name, namespace) = (args.kind.trim(), args.name.trim(), args.namespace.trim());
        if args.uri.trim().is_empty() || kind.is_empty() || name.is_empty() {
            return Err("uri, kind, and resourceName are required".into());
        }
        validate_name(kind, "kind")?;
        validate_name(name, "resourceName")?;
        if !namespace.is_empty() {
            validate_name(namespace, "namespace")?;
        }
        let mut out = Map::new();
        out.insert("kind".into(), J::from(kind));
        out.insert("resourceName".into(), J::from(name));
        out.insert("namespace".into(), J::from(namespace));
        self.execute_provider(DELETE_RESOURCE, &args.uri, out)
    }

    pub fn get_resource(&self, args: &ResourceArgs) -> Result<Map<String, J>, String> {
        let kind = match args.kind.trim() {
            "" => args.resource_kind.trim(),
            kind => kind,
        };
        let (name, namespace) = (args.name.trim(), args.namespace.trim());
        if args.uri.trim().is_empty() || kind.is_empty() || name.is_empty() {
            return Err("uri, kind, and resourceName are required".into());
        }
        validate_name(kind, "kind")?;
        validate_name(name, "resourceName")?;
        if !namespace.is_empty() {
            validate_name(namespace, "namespace")?;
        }
        let mut out = Map::new();
        out.insert("kind".into(), J::from(kind));
        out.insert("resourceKind".into(), J::from(args.resource_kind.clone()));
        out.insert("resourceName".into(), J::from(name));
        out.insert("namespace".into(), J::from(namespace));
        self.execute_provider(GET_RESOURCE, &args.uri, out)
    }

    pub fn get_resource_status(&self, args: &ResourceArgs) -> Result<Map<String, J>, String> {
        let kind = match args.resource_kind.trim() {
            "" => args.kind.trim(),
            kind => kind,
        };
        if args.uri.trim().is_empty() || kind.is_empty() || args.name.trim().is_empty() {
            return Err("uri, resourceKind, and resourceName are required".into());
        }
        validate_name(kind, "resourceKind")?;
        validate_name(&args.name, "resourceName")?;
        let mut out = Map::new();
        out.insert("kind".into(), J::from(kind));
        out.insert("resourceKind".into(), J::from(kind));
        out.insert("resourceName".into(), J::from(args.name.trim()));
        out.insert("namespace".into(), J::from(args.namespace.trim()));
        self.execute_provider(GET_RESOURCE_STATUS, &args.uri, out)
    }

    pub fn list_events(
        &self,
        uri: &str,
        namespace: &str,
        limit: i64,
    ) -> Result<Map<String, J>, String> {
        if uri.trim().is_empty() {
            return Err("uri is required".into());
        }
        let limit = if limit <= 0 { 50 } else { limit.min(200) };
        let mut out = Map::new();
        out.insert("namespace".into(), J::from(namespace.trim()));
        out.insert("limit".into(), J::from(limit));
        self.execute_provider(LIST_EVENTS, uri, out)
    }

    /// `PutK8sSecret`. Go ranges over the data map, so of several invalid
    /// keys the one it reports is unspecified; this reports the first.
    pub fn put_secret(
        &self,
        uri: &str,
        namespace: &str,
        name: &str,
        data: &Map<String, J>,
    ) -> Result<Map<String, J>, String> {
        if uri.trim().is_empty() || name.trim().is_empty() {
            return Err("uri and name are required".into());
        }
        let namespace = match namespace.trim() {
            "" => "default",
            _ => namespace,
        };
        validate_name(namespace, "namespace")?;
        validate_name(name, "name")?;
        if data.is_empty() {
            return Err("data is required".into());
        }
        for key in data.keys() {
            let ok = !key.is_empty()
                && key
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'));
            if !ok {
                return Err(format!("secret key {} is invalid", quote(key)));
            }
        }
        let mut out = Map::new();
        out.insert("namespace".into(), J::from(namespace));
        out.insert("name".into(), J::from(name.trim()));
        out.insert("data".into(), J::Object(data.clone()));
        self.execute_provider(PUT_SECRET, uri, out)
    }

    /// `ExecKubernetesCommand`: one kubectl sub-command, its output reported
    /// against the cluster's own id.
    pub fn exec_command(
        &self,
        uri: &str,
        command: &str,
        args: &[String],
        stdin: &str,
    ) -> Result<J, String> {
        let uri = uri.trim();
        if uri.is_empty() {
            return Err("uri is required".into());
        }
        let command = command.trim();
        if command.is_empty() {
            return Err("command is required".into());
        }
        let mut kubectl = vec![command.to_string()];
        kubectl.extend(args.iter().cloned());
        let mut payload = Map::new();
        payload.insert("kubectlArgs".into(), strings(&kubectl));
        if !stdin.is_empty() {
            payload.insert("stdin".into(), J::from(stdin));
        }
        let out = self.execute_provider(EXEC_COMMAND, uri, payload)?;
        let mut result = Map::new();
        result.insert("clusterId".into(), J::from(cluster_id_from_uri(uri)));
        result.insert("command".into(), J::from(command));
        result.insert(
            "stdout".into(),
            J::from(out.get("stdout").and_then(J::as_str).unwrap_or("")),
        );
        result.insert("uri".into(), J::from(uri));
        if let Some(code) = out.get("exitCode") {
            result.insert("exitCode".into(), code.clone());
        }
        if let Some(stderr) = out.get("stderr").and_then(J::as_str) {
            if !stderr.is_empty() {
                result.insert("stderr".into(), J::from(stderr));
            }
        }
        Ok(J::Object(result))
    }

    /// `delegateGuestStorage`.
    pub fn guest_storage(
        &self,
        operation: &str,
        args: &GuestStorageArgs,
        include_dry_run: bool,
    ) -> Result<Map<String, J>, String> {
        let uri = args.uri.trim();
        if uri.is_empty() {
            return Err("uri is required".into());
        }
        let mut payload = Map::new();
        payload.insert("includeRegistry".into(), J::Bool(args.include_registry));
        payload.insert("extraKeepTags".into(), strings(&args.extra_keep_tags));
        payload.insert(
            "registryNamespace".into(),
            J::from(args.registry_namespace.trim()),
        );
        payload.insert("registryName".into(), J::from(args.registry_name.trim()));
        if include_dry_run {
            payload.insert("dryRun".into(), J::Bool(args.dry_run));
        }
        if let Some(age) = args.min_age_seconds {
            payload.insert("minAgeSeconds".into(), J::from(age));
        }
        let mut out = self.execute_provider(operation, uri, payload)?;
        out.insert("uri".into(), J::from(uri));
        out.entry("clusterId")
            .or_insert_with(|| J::from(cluster_id_from_uri(uri)));
        Ok(out)
    }

    /// `EnsureHelmNamespace`.
    fn ensure_namespace(&self, name: &str, namespace: &str) -> Result<(), String> {
        if namespace.is_empty() || namespace == "kube-system" {
            return Ok(());
        }
        let get = ["get", "namespace", namespace].map(String::from);
        if self.run_kubectl(name, &get, "get namespace").is_ok() {
            return Ok(());
        }
        let create = ["create", "namespace", namespace].map(String::from);
        self.run_kubectl(name, &create, "create namespace")
            .map(|_| ())
    }

    /// `InstallHelmChart`: a k3s HelmChart manifest, applied through the
    /// provider.
    pub fn install_chart(&self, args: &ChartArgs) -> Result<J, String> {
        let name = args.vm_name.trim();
        if name.is_empty() {
            return Err("vmName is required".into());
        }
        let release = args.release_name.trim();
        if release.is_empty() {
            return Err("releaseName is required".into());
        }
        let source = args.chart_source.trim();
        if source.is_empty() {
            return Err("chartSource is required".into());
        }
        let namespace = match args.namespace.trim() {
            "" => "kube-system",
            namespace => namespace,
        };
        if namespace != "kube-system" {
            self.ensure_namespace(name, namespace)?;
        }
        let manifest = chart_manifest(args);
        let target = self.target_uri(name)?;
        self.apply_manifest(&target, &manifest)?;
        Ok(json!({
            "vmName": name,
            "releaseName": release,
            "chartSource": source,
            "namespace": namespace,
            "status": "installing",
        }))
    }

    /// `RenderHelmTemplate`: `helm template` on this host.
    pub fn render_template(&self, args: &TemplateArgs) -> Result<J, String> {
        let (chart, release) = (args.chart_path.trim(), args.release_name.trim());
        if chart.is_empty() {
            return Err("chartPath is required".into());
        }
        if release.is_empty() {
            return Err("releaseName is required".into());
        }
        let chart = absolute(chart).map_err(|e| format!("resolve chartPath: {e}"))?;
        if !std::fs::metadata(&chart).is_ok_and(|m| m.is_dir()) {
            return Err(format!("chartPath must be an existing directory: {chart}"));
        }
        let tool = (self.ensure_tool)("helm")?;
        let mut helm = tool
            .get("path")
            .and_then(J::as_str)
            .unwrap_or("")
            .to_string();
        if helm.trim().is_empty() {
            helm = look_path_host(&self.env, "helm")?;
        }
        let mut argv = vec![helm, "template".into(), release.into(), chart.clone()];
        if !args.namespace.trim().is_empty() {
            argv.extend(["--namespace".into(), args.namespace.trim().into()]);
        }
        for values in &args.values_files {
            let values = values.trim();
            if values.is_empty() {
                continue;
            }
            let values = absolute(values).map_err(|e| format!("resolve values file: {e}"))?;
            if std::fs::metadata(&values).is_err() {
                return Err(format!("values file not found: {values}"));
            }
            argv.extend(["-f".into(), values]);
        }
        for set in &args.set {
            let set = set.trim();
            if !set.is_empty() {
                argv.extend(["--set".into(), set.into()]);
            }
        }
        let res = (self.run_host)(&argv, Duration::from_secs(600));
        if res.exit_code != 0 {
            return Err(
                first_non_empty(&[&res.stderr, &res.stdout, "helm template failed"]).to_string(),
            );
        }
        Ok(json!({"releaseName": release, "chartPath": chart, "manifest": res.stdout}))
    }

    /// `ConfigureServiceDomain`: an Ingress for one service, applied through
    /// the provider. Go validates the three names in map order, so of
    /// several invalid names the one it reports is unspecified; this checks
    /// namespace, ingressName, serviceName.
    pub fn configure_domain(&self, args: &DomainArgs) -> Result<J, String> {
        if [
            &args.vm_name,
            &args.namespace,
            &args.ingress_name,
            &args.hostname,
            &args.service_name,
        ]
        .iter()
        .any(|v| v.trim().is_empty())
            || args.service_port <= 0
        {
            return Err("vmName, namespace, ingressName, hostname, serviceName, and positive servicePort are required".into());
        }
        validate_name(&args.namespace, "namespace")?;
        validate_name(&args.ingress_name, "ingressName")?;
        validate_name(&args.service_name, "serviceName")?;
        let hostname = args.hostname.trim();
        if hostname.contains(['\r', '\n', '\'', ' ', '\t', '/']) || !hostname.contains('.') {
            return Err("hostname must be a DNS name".into());
        }
        let class = match args.ingress_class.trim() {
            "" => "traefik",
            class => class,
        };
        let manifest = format!(
            "apiVersion: networking.k8s.io/v1\nkind: Ingress\nmetadata:\n  name: {}\n  namespace: {}\nspec:\n  ingressClassName: {class}\n  rules:\n    - host: {hostname}\n      http:\n        paths:\n          - path: /\n            pathType: Prefix\n            backend:\n              service:\n                name: {}\n                port:\n                  number: {}\n",
            args.ingress_name, args.namespace, args.service_name, args.service_port
        );
        let target = self.target_uri(&args.vm_name)?;
        self.apply_manifest(&target, &manifest)?;
        Ok(json!({
            "vmName": args.vm_name,
            "namespace": args.namespace,
            "ingressName": args.ingress_name,
            "hostname": hostname,
            "serviceName": args.service_name,
            "servicePort": args.service_port,
            "ingressClass": class,
            "configured": true,
        }))
    }

    /// `RemoveServiceDomain`: a missing Ingress is already removed.
    pub fn remove_domain(&self, args: &DomainArgs) -> Result<J, String> {
        if [&args.vm_name, &args.namespace, &args.ingress_name]
            .iter()
            .any(|v| v.trim().is_empty())
        {
            return Err("vmName, namespace, and ingressName are required".into());
        }
        validate_name(&args.namespace, "namespace")?;
        validate_name(&args.ingress_name, "ingressName")?;
        let target = self.target_uri(&args.vm_name)?;
        let delete = ResourceArgs {
            uri: target,
            kind: "ingress".into(),
            resource_kind: String::new(),
            name: args.ingress_name.clone(),
            namespace: args.namespace.clone(),
        };
        if let Err(e) = self.delete_resource(&delete) {
            if !e.to_lowercase().contains("not found") {
                return Err(e);
            }
        }
        Ok(json!({
            "vmName": args.vm_name,
            "namespace": args.namespace,
            "ingressName": args.ingress_name,
            "deleted": true,
        }))
    }
}

/// `filepath.Abs`.
pub(crate) fn absolute(path: &str) -> Result<String, String> {
    let path = std::path::Path::new(path);
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| crate::goerr::errno_text(&e))?
            .join(path)
    };
    Ok(clean(&joined.to_string_lossy()))
}

/// `filepath.Clean` for an absolute path.
pub(crate) fn clean(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }
    format!("/{}", parts.join("/"))
}

/// `lookupHostTool`.
fn look_path_host(env: &crate::config::Env, name: &str) -> Result<String, String> {
    crate::hostobs::look_path(env, name)
        .map(|p| p.display().to_string())
        .ok_or_else(|| {
            format!(
                "host tool {name} is not available: exec: {}: executable file not found in $PATH",
                quote(name)
            )
        })
}

/// `renderHelmChartManifest`.
fn chart_manifest(args: &ChartArgs) -> String {
    let namespace = match args.namespace.trim() {
        "" => "kube-system",
        namespace => namespace,
    };
    let mut lines = vec![
        "apiVersion: helm.cattle.io/v1".to_string(),
        "kind: HelmChart".into(),
        "metadata:".into(),
        format!("  name: {}", args.release_name),
        "  namespace: kube-system".into(),
        "spec:".into(),
        format!("  chart: {}", args.chart_source),
    ];
    if !args.repo.trim().is_empty() {
        lines.push(format!("  repo: {}", args.repo.trim()));
    }
    lines.push(format!("  targetNamespace: {namespace}"));
    let values = args.values.trim();
    if !values.is_empty() {
        lines.push("  valuesContent: |".into());
        lines.push(
            values
                .trim_end_matches('\n')
                .split('\n')
                .map(|line| format!("    {line}"))
                .collect::<Vec<_>>()
                .join("\n"),
        );
    }
    lines.join("\n")
}

/// `HelmValuesYAML`: a string as given, an object as indented JSON.
pub fn helm_values(raw: Option<&J>) -> String {
    match raw {
        Some(J::String(s)) => s.trim().to_string(),
        Some(value @ J::Object(_)) => {
            let compact = crate::gojson::encode_to_string(value);
            crate::gojson::indent(&compact, "  ")
        }
        _ => String::new(),
    }
}

/// `K8sResourceArgs`.
pub struct ResourceArgs {
    pub uri: String,
    pub kind: String,
    pub resource_kind: String,
    pub name: String,
    pub namespace: String,
}

/// `GuestStorageArgs`.
#[derive(Default)]
pub struct GuestStorageArgs {
    pub uri: String,
    pub include_registry: bool,
    pub dry_run: bool,
    pub min_age_seconds: Option<i64>,
    pub extra_keep_tags: Vec<String>,
    pub registry_namespace: String,
    pub registry_name: String,
}

/// `InstallHelmChartArgs`.
pub struct ChartArgs {
    pub vm_name: String,
    pub release_name: String,
    pub chart_source: String,
    pub namespace: String,
    pub repo: String,
    pub values: String,
}

/// `RenderHelmTemplateArgs`.
pub struct TemplateArgs {
    pub chart_path: String,
    pub release_name: String,
    pub values_files: Vec<String>,
    pub set: Vec<String>,
    pub namespace: String,
}

/// `ConfigureServiceDomainArgs`.
#[derive(Default)]
pub struct DomainArgs {
    pub vm_name: String,
    pub namespace: String,
    pub ingress_name: String,
    pub hostname: String,
    pub service_name: String,
    pub service_port: i64,
    pub ingress_class: String,
}

/// An unrecovered Go panic: nothing in the Go agent recovers a handler
/// panic, so the process exits 2 after printing the panic. The list
/// handlers' unchecked type assertions reach this on malformed provider
/// output; the Rust agent ends the same way.
fn go_panic(message: &str) -> ! {
    eprintln!("panic: {message}\n\ngoroutine 1 [running]:");
    std::process::exit(2)
}

/// Go's `%T`-style name of a decoded JSON value, for an interface
/// conversion panic.
fn dynamic_type(value: Option<&J>) -> &'static str {
    match value {
        None | Some(J::Null) => "nil",
        Some(J::Bool(_)) => "bool",
        Some(J::Number(_)) => "float64",
        Some(J::String(_)) => "string",
        Some(J::Array(_)) => "[]interface {}",
        Some(J::Object(_)) => "map[string]interface {}",
    }
}

/// `x.(map[string]any)`, panicking as Go does.
fn assert_object(value: Option<&J>) -> &Map<String, J> {
    match value {
        Some(J::Object(map)) => map,
        other => go_panic(&format!(
            "interface conversion: interface {{}} is {}, not map[string]interface {{}}",
            dynamic_type(other)
        )),
    }
}

/// `x.(string)`, panicking as Go does.
fn assert_string(value: Option<&J>) -> &str {
    match value {
        Some(J::String(text)) => text,
        other => go_panic(&format!(
            "interface conversion: interface {{}} is {}, not string",
            dynamic_type(other)
        )),
    }
}

/// `clusterScopedK8sResources`.
const CLUSTER_SCOPED: [&str; 4] = [
    "namespaces",
    "ingressclasses",
    "storageclasses",
    "clusterissuers",
];

impl Kubernetes {
    /// `getKubernetesList`: `kubectl get <resource> -o json` through the
    /// provider, answered with its items.
    fn get_list(&self, name: &str, resource: &str, namespace: &str) -> Result<Vec<J>, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("vmName is required".into());
        }
        let mut argv = vec!["get".to_string(), resource.to_string()];
        if !namespace.is_empty() {
            argv.extend(["-n".into(), namespace.into()]);
        } else if !CLUSTER_SCOPED.contains(&resource) {
            argv.push("--all-namespaces".into());
        }
        argv.extend(["-o".into(), "json".into()]);
        let stdout = self.run_kubectl(name, &argv, &format!("list {resource}"))?;
        if !stdout.starts_with('{') {
            return Err(format!("expected JSON output while listing {resource}"));
        }
        let parsed: J = crate::gojson::parse(stdout.as_bytes())
            .map(|node| crate::transport::go_any(&node))
            .map_err(|_| {
                crate::gojson::syntax_error(stdout.as_bytes())
                    .unwrap_or_else(|| "unexpected end of JSON input".into())
            })?;
        match parsed.get("items") {
            Some(J::Array(items)) => Ok(items.clone()),
            _ => Err(format!(
                "invalid Kubernetes {resource} response: missing items array"
            )),
        }
    }

    /// `ListNamespaces`, `ListStorageClasses`, `ListIngressClasses`.
    pub fn list_names(&self, name: &str, resource: &str) -> Result<Vec<String>, String> {
        let items = self.get_list(name, resource, "")?;
        Ok(items
            .iter()
            .map(|item| {
                let meta = assert_object(assert_object(Some(item)).get("metadata"));
                assert_string(meta.get("name")).to_string()
            })
            .collect())
    }

    /// `ListCertificateIssuers`: cluster issuers (unless a namespace narrows
    /// the question), then namespaced issuers.
    pub fn list_issuers(&self, name: &str, namespace: &str) -> Result<Vec<J>, String> {
        let mut out = Vec::new();
        if namespace.trim().is_empty() {
            out.extend(self.issuer_kind(name, "clusterissuers", "ClusterIssuer", "")?);
        }
        out.extend(self.issuer_kind(name, "issuers", "Issuer", namespace)?);
        Ok(out)
    }

    /// `listIssuerKind`: an absent CRD is an empty list.
    fn issuer_kind(
        &self,
        name: &str,
        resource: &str,
        kind: &str,
        namespace: &str,
    ) -> Result<Vec<J>, String> {
        let items = match self.get_list(name, resource, namespace) {
            Ok(items) => items,
            Err(e) if missing_resource_type(&e) => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        let text = |v: Option<&J>| v.and_then(J::as_str).unwrap_or("").to_string();
        Ok(items
            .iter()
            .filter_map(J::as_object)
            .map(|item| {
                let meta = item.get("metadata").and_then(J::as_object);
                let field = |key: &str| text(meta.and_then(|m| m.get(key)));
                let mut row = Map::new();
                row.insert("name".into(), J::from(field("name")));
                row.insert("namespace".into(), J::from(field("namespace")));
                row.insert("kind".into(), J::from(kind));
                row.insert("ready".into(), J::Bool(issuer_ready(item)));
                row.insert("age".into(), J::from(k8s_age(&field("creationTimestamp"))));
                let spec = item.get("spec").and_then(J::as_object);
                if let Some(solver) = ["acme", "ca", "selfSigned", "vault", "venafi"]
                    .into_iter()
                    .find(|key| spec.is_some_and(|s| s.contains_key(*key)))
                {
                    row.insert("type".into(), J::from(solver));
                }
                J::Object(row)
            })
            .collect())
    }

    /// `ListServices`: each service registered under its canonical URI.
    pub fn list_services(&self, name: &str, namespace: &str) -> Result<Vec<J>, String> {
        let items = self.get_list(name, "services", namespace)?;
        let mut out = Vec::new();
        for item in &items {
            let meta = assert_object(assert_object(Some(item)).get("metadata"));
            let service = assert_string(meta.get("name")).to_string();
            let ns = assert_string(meta.get("namespace")).to_string();
            let mut row = Map::new();
            row.insert("name".into(), J::from(service.clone()));
            row.insert("namespace".into(), J::from(ns.clone()));
            if let Ok(uri) = Uri::new(
                "service",
                &self.tenant_id,
                &format!("{name}/{ns}/{service}"),
            ) {
                let uri = uri.to_string();
                row.insert("uri".into(), J::from(uri.clone()));
                let mut coordinates = Map::new();
                coordinates.insert("providerInstanceName".into(), J::from(name));
                coordinates.insert("namespace".into(), J::from(ns));
                coordinates.insert("serviceName".into(), J::from(service));
                let _ = (self.register)(&uri, coordinates);
            }
            out.push(J::Object(row));
        }
        Ok(out)
    }

    /// `ListPods`: a pod without a uid has no stable URI, so the list fails.
    pub fn list_pods(&self, name: &str, namespace: &str) -> Result<Vec<J>, String> {
        let items = self.get_list(name, "pods", namespace)?;
        let mut out = Vec::new();
        for raw in &items {
            let pod = Pod::decode(raw);
            if pod.uid.trim().is_empty() {
                return Err(format!(
                    "the Kubernetes pod {}/{} has no metadata.uid; refusing to issue an unstable pod URI",
                    quote(&pod.namespace),
                    quote(&pod.name)
                ));
            }
            let ready = pod.statuses.iter().all(|(ready, _)| *ready);
            let restarts: i64 = pod.statuses.iter().map(|(_, count)| count).sum();
            let mut row = Map::new();
            row.insert("name".into(), J::from(pod.name.clone()));
            row.insert("namespace".into(), J::from(pod.namespace.clone()));
            row.insert("kind".into(), J::from("pod"));
            row.insert(
                "status".into(),
                J::from(if pod.phase.trim().is_empty() {
                    "Unknown"
                } else {
                    pod.phase.as_str()
                }),
            );
            row.insert("ready".into(), J::Bool(ready));
            row.insert("restarts".into(), J::from(restarts));
            row.insert("age".into(), J::from(k8s_age(&pod.created)));
            if !pod.ip.is_empty() {
                row.insert("ip".into(), J::from(pod.ip.clone()));
            }
            if !pod.node.is_empty() {
                row.insert("node".into(), J::from(pod.node.clone()));
            }
            let id = format!("{name}/{}/{}/{}", pod.namespace, pod.name, pod.uid);
            if let Ok(uri) = Uri::new("pod", &self.tenant_id, &id) {
                let uri = uri.to_string();
                row.insert("uri".into(), J::from(uri.clone()));
                let mut coordinates = Map::new();
                coordinates.insert("providerInstanceName".into(), J::from(name));
                coordinates.insert("namespace".into(), J::from(pod.namespace));
                coordinates.insert("podName".into(), J::from(pod.name));
                coordinates.insert("uid".into(), J::from(pod.uid));
                coordinates.insert("clusterResource".into(), J::from(name));
                let _ = (self.register)(&uri, coordinates);
            }
            out.push(J::Object(row));
        }
        Ok(out)
    }

    /// `ListDeployments`.
    pub fn list_deployments(&self, name: &str, namespace: &str) -> Result<Vec<J>, String> {
        let items = self.get_list(name, "deployments", namespace)?;
        Ok(items
            .iter()
            .map(|raw| {
                let d = Deployment::decode(raw);
                let status = if d.ready >= d.desired && d.ready > 0 {
                    "ready"
                } else {
                    "pending"
                };
                json!({
                    "name": d.name,
                    "namespace": d.namespace,
                    "ready": d.ready,
                    "desired": d.desired,
                    "available": d.available,
                    "unavailable": d.unavailable,
                    "age": k8s_age(&d.created),
                    "status": status,
                })
            })
            .collect())
    }
}

/// `isMissingResourceType`.
fn missing_resource_type(error: &str) -> bool {
    let text = error.to_lowercase();
    text.contains("doesn't have a resource type")
        || text.contains("the server could not find the requested resource")
        || text.contains("could not find the requested resource")
}

/// `issuerReady`: the `Ready` condition, absent meaning not ready.
fn issuer_ready(item: &Map<String, J>) -> bool {
    let conditions = item
        .get("status")
        .and_then(J::as_object)
        .and_then(|s| s.get("conditions"))
        .and_then(J::as_array);
    for condition in conditions.into_iter().flatten().filter_map(J::as_object) {
        if condition.get("type").and_then(J::as_str) == Some("Ready") {
            return condition.get("status").and_then(J::as_str) == Some("True");
        }
    }
    false
}

/// A Go struct field decoded by `json.Unmarshal` into a `struct`, errors
/// ignored: a value of the wrong type leaves the zero value.
/// Keys match a field case-insensitively, and the input was marshalled
/// from a map (keys sorted), so of several matches the last sorted wins.
fn field<'a>(object: Option<&'a J>, key: &str) -> Option<&'a J> {
    let map = object?.as_object()?;
    map.iter()
        .filter(|(k, _)| crate::gojson::equal_fold(k, key))
        .max_by(|a, b| a.0.cmp(b.0))
        .map(|(_, v)| v)
}
fn field_str(object: Option<&J>, key: &str) -> String {
    field(object, key)
        .and_then(J::as_str)
        .unwrap_or("")
        .to_string()
}
/// An `int` field: a non-integral or out-of-range number fails to decode
/// and leaves zero.
fn field_int(object: Option<&J>, key: &str) -> i64 {
    match field(object, key) {
        Some(J::Number(n)) => go_int(n).unwrap_or(0),
        _ => 0,
    }
}

/// A decoded JSON number as Go's `int`: integral and in range.
fn go_int(n: &serde_json::Number) -> Option<i64> {
    n.as_i64().or_else(|| {
        n.as_f64()
            .filter(|f| {
                f.fract() == 0.0 && *f >= -9.223_372_036_854_776e18 && *f < 9.223_372_036_854_776e18
            })
            .map(|f| f as i64)
    })
}

/// `k8sPodItem`.
struct Pod {
    name: String,
    namespace: String,
    uid: String,
    created: String,
    node: String,
    phase: String,
    ip: String,
    statuses: Vec<(bool, i64)>,
}
impl Pod {
    fn decode(raw: &J) -> Pod {
        let meta = field(Some(raw), "metadata");
        let status = field(Some(raw), "status");
        let statuses = match field(status, "containerStatuses") {
            Some(J::Array(items)) => items
                .iter()
                .map(|cs| {
                    let ready = matches!(field(Some(cs), "ready"), Some(J::Bool(true)));
                    (ready, field_int(Some(cs), "restartCount"))
                })
                .collect(),
            _ => Vec::new(),
        };
        Pod {
            name: field_str(meta, "name"),
            namespace: field_str(meta, "namespace"),
            uid: field_str(meta, "uid"),
            created: field_str(meta, "creationTimestamp"),
            node: field_str(field(Some(raw), "spec"), "nodeName"),
            phase: field_str(status, "phase"),
            ip: field_str(status, "podIP"),
            statuses,
        }
    }
}

/// `k8sDeploymentItem`.
struct Deployment {
    name: String,
    namespace: String,
    created: String,
    desired: i64,
    ready: i64,
    available: i64,
    unavailable: i64,
}
impl Deployment {
    fn decode(raw: &J) -> Deployment {
        let meta = field(Some(raw), "metadata");
        let status = field(Some(raw), "status");
        Deployment {
            name: field_str(meta, "name"),
            namespace: field_str(meta, "namespace"),
            created: field_str(meta, "creationTimestamp"),
            desired: field_int(field(Some(raw), "spec"), "replicas"),
            ready: field_int(status, "readyReplicas"),
            available: field_int(status, "availableReplicas"),
            unavailable: field_int(status, "unavailableReplicas"),
        }
    }
}

/// `k8sAge`: minutes under an hour, hours under two days, then days.
pub fn k8s_age(created: &str) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as i128);
    age_at(created, now)
}

/// `k8sAge` at `now` (Unix nanoseconds).
fn age_at(created: &str, now: i128) -> String {
    let Some(at) = parse_rfc3339(created) else {
        return "unknown".into();
    };
    // `int(elapsed.Minutes())` truncates toward zero.
    let minutes = ((now - at) as f64 / 6e10) as i64;
    if minutes < 60 {
        return format!("{minutes}m");
    }
    let hours = minutes / 60;
    if hours < 48 {
        return format!("{hours}h");
    }
    format!("{}d", hours / 24)
}

/// `time.Parse(time.RFC3339, s)` as Unix nanoseconds: a fractional second
/// is accepted after the seconds, as Go accepts it.
pub(crate) fn parse_rfc3339(s: &str) -> Option<i128> {
    let b = s.as_bytes();
    let digits = |from: usize, n: usize| -> Option<i64> {
        let part = b.get(from..from + n)?;
        part.iter().all(u8::is_ascii_digit).then(|| {
            part.iter()
                .fold(0i64, |acc, d| acc * 10 + i64::from(d - b'0'))
        })
    };
    if b.len() < 20
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let (year, month, day) = (digits(0, 4)?, digits(5, 2)?, digits(8, 2)?);
    let (hour, minute, second) = (digits(11, 2)?, digits(14, 2)?, digits(17, 2)?);
    let mut i = 19;
    let mut nanos = 0i64;
    if b.get(i) == Some(&b'.') {
        let start = i + 1;
        let mut end = start;
        while end < b.len() && b[end].is_ascii_digit() {
            end += 1;
        }
        if end == start {
            return None;
        }
        let frac = &s[start..end.min(start + 9)];
        nanos = format!("{frac:0<9}").parse().ok()?;
        i = end;
    }
    let offset = match b.get(i)? {
        b'Z' if i + 1 == b.len() => 0,
        sign @ (b'+' | b'-') if i + 6 == b.len() && b[i + 3] == b':' => {
            let (h, m) = (digits(i + 1, 2)?, digits(i + 4, 2)?);
            if h > 23 || m > 59 {
                return None;
            }
            let total = h * 3600 + m * 60;
            if *sign == b'-' {
                -total
            } else {
                total
            }
        }
        _ => return None,
    };
    let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let month_days = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if !(1..=12).contains(&month)
        || day < 1
        || day > month_days[(month - 1) as usize]
        || hour > 23
        || minute > 59
        || second > 59
    {
        return None;
    }
    // Days from the civil date (Howard Hinnant's algorithm).
    let (y, m) = if month <= 2 {
        (year - 1, month + 9)
    } else {
        (year, month - 3)
    };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * m + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let seconds = days * 86_400 + hour * 3600 + minute * 60 + second - offset;
    Some(i128::from(seconds) * 1_000_000_000 + i128::from(nanos))
}

/// A Go type a JSON value is decoded into, for `json.Unmarshal`'s errors.
#[derive(Clone, Copy)]
enum Ty {
    Str,
    Int,
    /// `*int` and `*bool` decode as their element and report its type.
    Bool,
    Strings,
    List(&'static str, &'static [(&'static str, Ty)]),
}

const NODE: &[(&str, Ty)] = &[
    ("name", Ty::Str),
    ("status", Ty::Str),
    ("roles", Ty::Strings),
    ("age", Ty::Str),
    ("version", Ty::Str),
];
const DETAIL: &[(&str, Ty)] = &[
    ("uri", Ty::Str),
    ("id", Ty::Str),
    ("name", Ty::Str),
    ("status", Ty::Str),
    ("provider", Ty::Str),
    ("infraProvider", Ty::Str),
    ("version", Ty::Str),
    ("nodeCount", Ty::Int),
    ("apiEndpoint", Ty::Str),
    ("ipv4", Ty::Strings),
    ("cpu", Ty::Int),
    ("memory", Ty::Str),
    ("disk", Ty::Str),
    ("agentReady", Ty::Bool),
    ("nodes", Ty::List("ClusterNode", NODE)),
    ("logs", Ty::Strings),
    ("nodeInventoryAvailable", Ty::Bool),
    ("vmName", Ty::Str),
    ("hostId", Ty::Str),
    ("instanceType", Ty::Str),
];
const LIST: &[(&str, Ty)] = &[
    ("clusters", Ty::List("ClusterDetail", DETAIL)),
    ("total", Ty::Int),
];

/// The `omitempty` fields of `clusterinfo.ClusterDetail`.
const DETAIL_OMIT: &[&str] = &[
    "provider",
    "infraProvider",
    "version",
    "nodeCount",
    "apiEndpoint",
    "ipv4",
    "cpu",
    "memory",
    "disk",
    "agentReady",
    "nodeInventoryAvailable",
    "vmName",
    "hostId",
    "instanceType",
];

/// The member order `json.Marshal` writes for `clusterinfo.ClusterListResult`.
pub static CLUSTER_LIST: crate::gojson::Shape = {
    use crate::gojson::Shape::{Any, List, Struct};
    const NODE_SHAPE: crate::gojson::Shape = Struct(&[
        ("name", Any),
        ("status", Any),
        ("roles", Any),
        ("age", Any),
        ("version", Any),
    ]);
    const DETAIL_SHAPE: crate::gojson::Shape = Struct(&[
        ("uri", Any),
        ("id", Any),
        ("name", Any),
        ("status", Any),
        ("provider", Any),
        ("infraProvider", Any),
        ("version", Any),
        ("nodeCount", Any),
        ("apiEndpoint", Any),
        ("ipv4", Any),
        ("cpu", Any),
        ("memory", Any),
        ("disk", Any),
        ("agentReady", Any),
        ("nodes", List(&NODE_SHAPE)),
        ("logs", Any),
        ("nodeInventoryAvailable", Any),
        ("vmName", Any),
        ("hostId", Any),
        ("instanceType", Any),
    ]);
    Struct(&[("clusters", List(&DETAIL_SHAPE)), ("total", Any)])
};

fn kind(value: &J) -> &'static str {
    match value {
        J::Null => "null",
        J::Bool(_) => "bool",
        J::Number(_) => "number",
        J::String(_) => "string",
        J::Array(_) => "array",
        J::Object(_) => "object",
    }
}

/// `json.Unmarshal` of a value marshalled from a map into the struct
/// `fields` describe: unknown keys are ignored, keys match a field
/// case-insensitively (the last sorted match wins), a type mismatch leaves
/// the zero value, and the first mismatch in document order is the error.
fn decode_struct(
    value: &Map<String, J>,
    name: &str,
    fields: &[(&str, Ty)],
    path: &str,
    first: &mut Option<String>,
) -> Map<String, J> {
    let mut out = Map::new();
    for (key, item) in value {
        let Some((field, ty)) = fields.iter().find(|(f, _)| f == key).or_else(|| {
            fields
                .iter()
                .find(|(f, _)| crate::gojson::equal_fold(f, key))
        }) else {
            continue;
        };
        let path = if path.is_empty() {
            (*field).to_string()
        } else {
            format!("{path}.{field}")
        };
        if let Some(decoded) = decode_value(item, *ty, name, &path, first) {
            out.insert((*field).to_string(), decoded);
        }
    }
    out
}

fn mismatch(first: &mut Option<String>, what: String, name: &str, path: &str, ty: &str) {
    first.get_or_insert_with(|| {
        format!("json: cannot unmarshal {what} into Go struct field {name}.{path} of type {ty}")
    });
}

/// One field's value; `None` leaves the field at its zero value.
fn decode_value(item: &J, ty: Ty, name: &str, path: &str, first: &mut Option<String>) -> Option<J> {
    if item.is_null() {
        return None;
    }
    match (ty, item) {
        (Ty::Str, J::String(_)) | (Ty::Bool, J::Bool(_)) => Some(item.clone()),
        (Ty::Int, J::Number(n)) => match go_int(n) {
            Some(i) => Some(J::from(i)),
            None => {
                mismatch(
                    first,
                    format!("number {}", crate::gojson::encode_to_string(item)),
                    name,
                    path,
                    "int",
                );
                None
            }
        },
        (Ty::Strings, J::Array(items)) => Some(J::Array(
            items
                .iter()
                .map(|v| match v {
                    J::String(_) => v.clone(),
                    J::Null => J::from(""),
                    other => {
                        mismatch(first, kind(other).into(), name, path, "string");
                        J::from("")
                    }
                })
                .collect(),
        )),
        (Ty::List(element, fields), J::Array(items)) => Some(J::Array(
            items
                .iter()
                .map(|v| match v {
                    J::Object(map) => J::Object(decode_struct(map, element, fields, path, first)),
                    J::Null => J::Object(Map::new()),
                    other => {
                        mismatch(
                            first,
                            kind(other).into(),
                            name,
                            path,
                            &format!("clusterinfo.{element}"),
                        );
                        J::Object(Map::new())
                    }
                })
                .collect(),
        )),
        (ty, other) => {
            let type_name = match ty {
                Ty::Str => "string".to_string(),
                Ty::Int => "int".into(),
                Ty::Bool => "bool".into(),
                Ty::Strings => "[]string".into(),
                Ty::List(element, _) => format!("[]clusterinfo.{element}"),
            };
            mismatch(first, kind(other).into(), name, path, &type_name);
            None
        }
    }
}

/// A decoded `ClusterDetail` with its zero values and `omitempty` applied,
/// as `json.Marshal` writes it.
fn detail_value(mut decoded: Map<String, J>) -> Map<String, J> {
    for key in ["uri", "id", "name", "status"] {
        decoded.entry(key).or_insert_with(|| J::from(""));
    }
    for key in ["nodes", "logs"] {
        decoded.entry(key).or_insert(J::Null);
    }
    if let Some(J::Array(nodes)) = decoded.get_mut("nodes") {
        for node in nodes.iter_mut() {
            if let J::Object(node) = node {
                for key in ["name", "status", "age", "version"] {
                    node.entry(key).or_insert_with(|| J::from(""));
                }
                node.entry("roles").or_insert(J::Null);
            }
        }
    }
    decoded.retain(|key, value| {
        let empty = match value {
            J::String(s) => s.is_empty(),
            J::Array(a) => a.is_empty(),
            _ => key == "nodeCount" && value == &J::from(0),
        };
        !(empty && DETAIL_OMIT.contains(&key.as_str()))
    });
    decoded
}

/// The cluster inventory's seams: the provider and the registry. It holds
/// no resolver, so the admission resolver can adopt through it.
#[derive(Clone)]
pub struct Inventory {
    pub execute: Executor,
    pub register: Register,
    pub tenant_id: String,
}

impl Kubernetes {
    pub fn inventory(&self) -> Inventory {
        Inventory {
            execute: self.execute.clone(),
            register: self.register.clone(),
            tenant_id: self.tenant_id.clone(),
        }
    }

    pub fn list_clusters(&self, source: &str) -> Result<J, String> {
        self.inventory().list_clusters(source)
    }
}

impl Inventory {
    /// `Shared.EffectiveTenantID`.
    fn effective_tenant(&self) -> &str {
        match self.tenant_id.trim() {
            "" => "local",
            tenant => tenant,
        }
    }

    /// `ListKubernetesClusters`: the provider's inventory, each cluster
    /// given its canonical URI and registered.
    pub fn list_clusters(&self, source: &str) -> Result<J, String> {
        let mut args = Map::new();
        args.insert("source".into(), J::from(source.trim()));
        let out = (self.execute)(LIST_CLUSTERS, args)?;
        let mut first = None;
        let decoded = decode_struct(&out, "ClusterListResult", LIST, "", &mut first);
        if let Some(error) = first {
            return Err(error);
        }
        let mut clusters: Vec<Map<String, J>> = match decoded.get("clusters") {
            Some(J::Array(items)) => items
                .iter()
                .map(|c| detail_value(c.as_object().cloned().unwrap_or_default()))
                .collect(),
            _ => Vec::new(),
        };
        let listed = matches!(decoded.get("clusters"), Some(J::Array(_)));
        let mut total = decoded.get("total").and_then(J::as_i64).unwrap_or(0);
        if total == 0 {
            total = clusters.len() as i64;
        }
        for cluster in &mut clusters {
            let name = cluster
                .get("name")
                .and_then(J::as_str)
                .unwrap_or("")
                .to_string();
            if name.trim().is_empty() {
                continue;
            }
            let uri = Uri::new("cluster", self.effective_tenant(), &name)?.to_string();
            cluster.insert("uri".into(), J::from(uri.clone()));
            cluster.insert("id".into(), J::from(uri.clone()));
            if cluster
                .get("vmName")
                .and_then(J::as_str)
                .unwrap_or("")
                .is_empty()
            {
                cluster.insert("vmName".into(), J::from(name.clone()));
            }
            let instance_type = cluster
                .get("instanceType")
                .and_then(J::as_str)
                .unwrap_or("")
                .to_string();
            if instance_type != "vm" && instance_type != "container" {
                return Err(format!(
                    "the Kubernetes provider returned unsupported instance type for cluster {}",
                    quote(&name)
                ));
            }
            let mut coordinates = Map::new();
            coordinates.insert("providerInstanceName".into(), J::from(name.clone()));
            coordinates.insert("displayName".into(), J::from(name.clone()));
            coordinates.insert("instanceType".into(), J::from(instance_type));
            coordinates.insert("vmName".into(), J::from(name));
            (self.register)(&uri, coordinates)?;
        }
        let clusters = if listed {
            J::Array(clusters.into_iter().map(J::Object).collect())
        } else {
            J::Null
        };
        Ok(json!({"clusters": clusters, "total": total}))
    }

    /// `adoptResource` for a cluster URI: coordinates for a cluster the
    /// provider lists, nothing for one it does not.
    pub fn adopt_cluster(&self, uri: &Uri) -> Result<Option<Map<String, J>>, String> {
        let listed = self
            .list_clusters("")
            .map_err(|e| format!("adopt cluster {uri}: {e}"))?;
        let clusters = listed
            .get("clusters")
            .and_then(J::as_array)
            .cloned()
            .unwrap_or_default();
        for cluster in clusters.iter().filter_map(J::as_object) {
            let text = |key: &str| {
                cluster
                    .get(key)
                    .and_then(J::as_str)
                    .unwrap_or("")
                    .to_string()
            };
            let name = text("name");
            if !crate::gojson::equal_fold(name.trim(), &uri.resource_id) {
                continue;
            }
            let vm_name = match text("vmName").trim() {
                "" => name.clone(),
                vm => vm.to_string(),
            };
            let mut coordinates = Map::new();
            coordinates.insert("providerInstanceName".into(), J::from(name.clone()));
            coordinates.insert("displayName".into(), J::from(name));
            coordinates.insert("instanceType".into(), J::from(text("instanceType")));
            coordinates.insert("vmName".into(), J::from(vm_name));
            return Ok(Some(coordinates));
        }
        Ok(None)
    }
}

// --- cluster agent (internal/domain/cluster) ----------------------------------------

const AGENT_BINARY: &str = "/usr/local/lib/opute/cluster-agent";
const AGENT_CONFIG: &str = "/etc/opute/cluster-agent.json";
const AGENT_SERVICE: &str = "opute-cluster-agent";
const AGENT_DOWNLOAD_SECONDS: u64 = 180;
const AGENT_INSTALL_TIMEOUT: Duration = Duration::from_secs(600);
const AGENT_SERVICE_WAIT: Duration = Duration::from_secs(180);
const SYSTEMCTL: &str = "/usr/bin/systemctl";

/// The cluster agent's seams: the instance provider (guest exec, devices,
/// instance kind), the process environment, and the guest bridge relays it
/// keeps alive.
#[derive(Clone)]
pub struct ClusterAgent {
    pub instances: crate::incusops::Instances,
    pub env: crate::config::Env,
    pub relays: Arc<crate::tcprelay::Manager>,
}

/// `InstallClusterAgentArgs`.
#[derive(Default)]
pub struct AgentArgs {
    pub vm_name: String,
    pub cluster_id: String,
    pub cluster_name: String,
    pub agent_id: String,
    pub bridge_token: String,
    pub bridge_url: String,
    pub bridge_port: i64,
    pub api_endpoint: String,
    pub provider_id: String,
    pub resource_id: String,
    pub source: String,
}

/// `normalizeClusterAgentArch`.
fn agent_arch(machine: &str) -> &'static str {
    match machine.trim().to_lowercase().as_str() {
        "aarch64" | "arm64" => "arm64",
        _ => "x64",
    }
}

/// `renderClusterAgentServiceUnit`.
fn agent_unit() -> String {
    format!("[Unit]\nDescription=Opute Cluster Agent\nAfter=network-online.target\nWants=network-online.target\n\n[Service]\nType=simple\nUser=root\nExecStart={AGENT_BINARY}\nRestart=always\nRestartSec=5\nEnvironment=OPUTE_AGENT_CONFIG={AGENT_CONFIG}\n\n[Install]\nWantedBy=multi-user.target\n")
}

/// `renderClusterAgentInstallScript`.
fn agent_install_script(bridge_url: &str, arch: &str, config: &str) -> String {
    use base64::Engine;
    let b64 = |s: &str| base64::engine::general_purpose::STANDARD.encode(s.as_bytes());
    let artifact = format!(
        "{}/artifacts/cluster-agent/{arch}.gz",
        bridge_url.trim_end_matches('/')
    );
    let q = crate::goerr::shell_quote;
    [
        "set -euo pipefail".to_string(),
        "mkdir -p /usr/local/lib/opute /etc/opute".into(),
        format!("systemctl stop {AGENT_SERVICE} 2>/dev/null || true"),
        "if ! command -v curl >/dev/null 2>&1; then apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y curl; fi".into(),
        "if ! command -v gunzip >/dev/null 2>&1; then apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y gzip; fi".into(),
        format!("curl -sfL --max-time {AGENT_DOWNLOAD_SECONDS} {} | gunzip > {AGENT_BINARY}", q(&artifact)),
        format!("chmod 755 {AGENT_BINARY}"),
        format!("printf '%s' {} | base64 -d > {AGENT_CONFIG}", q(&b64(config))),
        format!(
            "printf '%s' {} | base64 -d > /etc/systemd/system/{AGENT_SERVICE}.service",
            q(&b64(&agent_unit()))
        ),
        "systemctl daemon-reload".into(),
        format!("systemctl enable --now {AGENT_SERVICE}"),
        format!("systemctl restart {AGENT_SERVICE}"),
    ]
    .join("\n")
}

/// `wrapHostPrivilegedShellBody`.
fn privileged(body: &str) -> String {
    let escaped = body.replace('\'', "'\"'\"'");
    [
        "set -euo pipefail".to_string(),
        "if command -v sudo >/dev/null 2>&1 && sudo -n true 2>/dev/null; then".into(),
        format!("  sudo -n bash -lc '{escaped}'"),
        "elif [ \"$(id -u)\" -eq 0 ]; then".into(),
        format!("  bash -lc '{escaped}'"),
        "else".into(),
        "  echo 'passwordless sudo is required to install the cluster agent on the host OS' >&2"
            .into(),
        "  exit 127".into(),
        "fi".into(),
    ]
    .join("\n")
}

/// `clusterAgentConfig`, marshalled as Go writes it.
fn agent_config(bridge: &str, args: &AgentArgs) -> String {
    use crate::gojson::encode_string;
    let mut out = String::from("{");
    let mut member = |key: &str, value: &str, omit_empty: bool| {
        if omit_empty && value.is_empty() {
            return;
        }
        if out.len() > 1 {
            out.push(',');
        }
        encode_string(key, &mut out);
        out.push(':');
        encode_string(value, &mut out);
    };
    member("bridgeUrl", bridge, false);
    member("bridgeMcpUrl", &format!("{bridge}/mcp"), false);
    member("bridgeToken", args.bridge_token.trim(), false);
    member("clusterId", args.cluster_id.trim(), false);
    member("clusterName", args.cluster_name.trim(), false);
    member("agentId", args.agent_id.trim(), false);
    member("apiEndpoint", args.api_endpoint.trim(), true);
    member("providerId", args.provider_id.trim(), true);
    member("resourceId", args.resource_id.trim(), true);
    out.push_str(",\"pollIntervalSeconds\":5}");
    out
}

/// `bridgeURLHost`.
fn bridge_url_host(url: &str) -> String {
    let url = url.trim();
    if url.is_empty() {
        return String::new();
    }
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    let host = rest.split('/').next().unwrap_or("");
    let host = host.split(':').next().unwrap_or("");
    host.trim_matches(|c| c == '[' || c == ']').to_string()
}

/// `isLoopbackBridgeURL`: Go keeps the brackets in the parsed host, so a
/// bracketed address is trimmed before parsing.
fn loopback_url(url: &str) -> bool {
    if url.trim().is_empty() {
        return false;
    }
    bridge_url_host(url)
        .parse::<std::net::IpAddr>()
        .is_ok_and(|ip| ip.is_loopback())
}

/// `heartbeat.PrimaryLANIPv4` (Unix): the first IPv4 address of the first
/// up, non-loopback interface, interfaces in index order.
pub fn primary_lan_ipv4() -> String {
    first_ipv4(|name, flags| {
        let _ = name;
        flags.contains(nix::net::if_::InterfaceFlags::IFF_UP)
            && !flags.contains(nix::net::if_::InterfaceFlags::IFF_LOOPBACK)
    })
}

/// `heartbeat.IncusBridgeIPv4`: the first IPv4 address on the Incus bridge.
pub fn incus_bridge_ipv4(env: &crate::config::Env) -> String {
    let network = match env.get("OPUTE_INCUS_NETWORK_NAME").trim() {
        "" => "incusbr0".to_string(),
        name => name.to_string(),
    };
    first_ipv4(|name, flags| {
        name == network && flags.contains(nix::net::if_::InterfaceFlags::IFF_UP)
    })
}

fn first_ipv4(accept: impl Fn(&str, nix::net::if_::InterfaceFlags) -> bool) -> String {
    let Ok(addresses) = nix::ifaddrs::getifaddrs() else {
        return String::new();
    };
    let mut found: Vec<(u32, std::net::Ipv4Addr)> = Vec::new();
    for entry in addresses {
        if !accept(&entry.interface_name, entry.flags) {
            continue;
        }
        let Some(ip) = entry
            .address
            .as_ref()
            .and_then(|a| a.as_sockaddr_in())
            .map(|a| a.ip())
        else {
            continue;
        };
        if ip.is_loopback() {
            continue;
        }
        let index =
            nix::net::if_::if_nametoindex(entry.interface_name.as_str()).unwrap_or(u32::MAX);
        found.push((index, ip));
    }
    // A stable sort keeps the kernel's order within an interface.
    found.sort_by_key(|(index, _)| *index);
    found
        .first()
        .map(|(_, ip)| ip.to_string())
        .unwrap_or_default()
}

/// `firstBridgeIPv4`.
pub(crate) fn first_bridge_ipv4(ips: &[String]) -> String {
    ips.iter()
        .find(|ip| ip.starts_with("10.123."))
        .or_else(|| {
            ips.iter()
                .find(|ip| !ip.is_empty() && !ip.starts_with("10.42.") && !ip.starts_with("127."))
        })
        .or(ips.first())
        .cloned()
        .unwrap_or_default()
}

impl ClusterAgent {
    fn env_or(&self, key: &str, fallback: &str) -> String {
        match self.env.get(key).trim() {
            "" => fallback.to_string(),
            value => value.to_string(),
        }
    }

    /// `resolveBridgeURLFromEnv`.
    fn bridge_url_from_env(&self) -> String {
        for key in ["OPUTE_PLATFORM_PUBLIC_URL", "OPUTE_PLATFORM_URL"] {
            let value = self.env.get(key);
            if !value.trim().is_empty() {
                return value.trim().trim_end_matches('/').to_string();
            }
        }
        format!(
            "http://127.0.0.1:{}",
            self.env_or("PLATFORM_MCP_PORT", "9093")
        )
    }

    /// `defaultBridgePort`.
    fn default_port(&self) -> i64 {
        let value = self.env_or("PLATFORM_MCP_PORT", "");
        match crate::goerr::atoi(&value) {
            (port, true) if port > 0 => port,
            _ => 9093,
        }
    }

    fn vm_exec(
        &self,
        vm: &str,
        argv: &[&str],
        timeout: Duration,
    ) -> Result<crate::hostobs::CommandResult, String> {
        let argv: Vec<String> = argv.iter().map(|s| s.to_string()).collect();
        self.instances.incus.run_vm_exec(
            vm,
            &argv,
            None,
            timeout,
            &crate::plan::runner::RunCtx::background(),
        )
    }

    /// `probeBridgeHealthFromGuest`.
    fn probe_from_guest(&self, vm: &str, base: &str) -> bool {
        let health = format!("{}/health", base.trim_end_matches('/'));
        let script = format!(
            "curl -sf -o /dev/null {}",
            crate::goerr::shell_quote(&health)
        );
        self.vm_exec(vm, &["bash", "-lc", &script], Duration::from_secs(15))
            .is_ok_and(|r| r.exit_code == 0)
    }

    /// `readVMDefaultGateway`.
    fn default_gateway(&self, vm: &str) -> String {
        let script = "ip route show default | awk '{print $3; exit}'";
        match self.vm_exec(vm, &["bash", "-lc", script], Duration::from_secs(15)) {
            Ok(r) if r.exit_code == 0 => {
                let gateway = r.stdout.trim();
                if gateway.parse::<std::net::IpAddr>().is_ok() {
                    gateway.to_string()
                } else {
                    String::new()
                }
            }
            _ => String::new(),
        }
    }

    /// `discoverCpcContainerName`.
    fn cpc_container(&self) -> String {
        [
            "OPUTE_K3S_VM",
            "OPUTE_DOGFOOD_GATEWAY_VM",
            "OPUTE_K3S_CONTAINER",
        ]
        .into_iter()
        .map(|key| self.env.get(key).trim().to_string())
        .find(|name| !name.is_empty())
        .unwrap_or_default()
    }

    /// `resolveCpcContainerBridgeIPv4`.
    fn cpc_bridge_ipv4(&self) -> String {
        let container = self.cpc_container();
        if container.is_empty() {
            return String::new();
        }
        match self.instances.instance_type(&container) {
            Ok(kind) if kind.eq_ignore_ascii_case("container") => {}
            _ => return String::new(),
        }
        let register = |_: &str, _: Map<String, J>| {};
        match self
            .instances
            .incus
            .get_vm_info(&container, true, &register)
        {
            Ok(info) => {
                let ips: Vec<String> = info
                    .get("ipv4")
                    .and_then(J::as_array)
                    .map(|a| a.iter().filter_map(J::as_str).map(String::from).collect())
                    .unwrap_or_default();
                first_bridge_ipv4(&ips)
            }
            Err(_) => String::new(),
        }
    }

    /// `readGuestBridgeListenHost`.
    fn read_listen_host(&self) -> String {
        let host = self.env.get("OPUTE_PLATFORM_GUEST_HOST");
        if !host.trim().is_empty() {
            return host.trim().to_string();
        }
        let bridge = incus_bridge_ipv4(&self.env);
        if !bridge.is_empty() {
            return bridge;
        }
        primary_lan_ipv4().trim().to_string()
    }

    /// `resolveGuestBridgeListenHost`.
    fn listen_host(&self) -> String {
        let host = self.env.get("OPUTE_PLATFORM_GUEST_HOST");
        if !host.trim().is_empty() {
            return host.trim().to_string();
        }
        let ip = self.cpc_bridge_ipv4();
        if !ip.is_empty() {
            return ip;
        }
        self.read_listen_host()
    }

    /// `ensureCpcContainerGuestBridgeProxy`.
    fn cpc_proxy(&self, container: &str, listen: &str, port: i64) -> Result<(), String> {
        let (container, listen) = (container.trim(), listen.trim());
        if container.is_empty() || listen.is_empty() || port <= 0 {
            return Err("invalid CPC guest bridge proxy target".into());
        }
        let kind = self.instances.instance_type(container)?;
        if !kind.eq_ignore_ascii_case("container") {
            return Err(format!(
                "instance {} is {}, not a system container",
                quote(container),
                quote(&kind)
            ));
        }
        let device = format!("platform-bridge-{port}");
        let proxy = format!("listen=tcp:{listen}:{port},connect=tcp:127.0.0.1:{port}");
        self.instances.ensure_instance_device(
            container,
            &device,
            &[
                "config", "device", "add", container, &device, "proxy", &proxy,
            ],
        )
    }

    fn start_relay(
        &self,
        id: &str,
        listen: &str,
        port: i64,
        target: &str,
        target_port: i64,
    ) -> Result<(), String> {
        match self.relays.start(id, listen, port, target, target_port) {
            Ok(_) => Ok(()),
            Err(e) if e.contains("already active") || e.contains("already in use") => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// `ensureGuestBridgeReachability`: a proxy device on the CPC container,
    /// or an in-process relay from the guest-facing address.
    fn ensure_reachability(&self, bridge_url: &str, bridge_port: i64) -> Result<(), String> {
        let port = if bridge_port <= 0 {
            self.default_port()
        } else {
            bridge_port
        };
        let listen = self.listen_host();
        if listen.is_empty() {
            return Err("guest bridge listen host is unavailable".into());
        }
        let container = self.cpc_container();
        if !container.is_empty() && self.cpc_proxy(&container, &listen, port).is_ok() {
            return Ok(());
        }
        let configured = bridge_url_host(bridge_url);
        if !configured.is_empty() && !loopback_url(bridge_url) && configured != listen {
            let id = format!("guest-bridge:{listen}:{port}->{configured}:{port}");
            return self.start_relay(&id, &listen, port, &configured, port);
        }
        if listen.trim().is_empty() || port <= 0 {
            return Err("invalid guest bridge relay listen address".into());
        }
        let id = format!("guest-bridge:{}:{port}", listen.trim());
        self.start_relay(&id, listen.trim(), port, "127.0.0.1", port)
    }

    /// `resolveBridgeEndpointForVM`: the configured bridge when the guest
    /// reaches it, else the first reachable candidate address.
    fn bridge_endpoint(
        &self,
        vm: &str,
        bridge_url: &str,
        bridge_port: i64,
    ) -> Result<String, String> {
        let bridge_url = bridge_url.trim().trim_end_matches('/');
        if !bridge_url.is_empty()
            && !loopback_url(bridge_url)
            && self.probe_from_guest(vm, bridge_url)
        {
            return Ok(bridge_url.to_string());
        }
        let port = if bridge_port <= 0 {
            self.default_port()
        } else {
            bridge_port
        };
        let mut candidates: Vec<String> = Vec::new();
        let mut push = |host: &str| {
            let host = host.trim();
            if host.is_empty() {
                return;
            }
            let url = format!("http://{host}:{port}");
            if !candidates.contains(&url) {
                candidates.push(url);
            }
        };
        push(&self.default_gateway(vm));
        push(&self.listen_host());
        let cpc = self.cpc_bridge_ipv4();
        if !cpc.is_empty() {
            push(&cpc);
        }
        push(primary_lan_ipv4().trim());
        // `resolveHyperVDefaultSwitchIPv4` is empty off Windows.
        if let Ok(r) = self.vm_exec(
            vm,
            &["getent", "hosts", "host.lan"],
            Duration::from_secs(10),
        ) {
            if r.exit_code == 0 {
                if let Some(first) = r.stdout.split_whitespace().next() {
                    push(first);
                }
            }
        }
        push(self.env_or("OPUTE_PLATFORM_GUEST_HOST", "").trim());
        for candidate in &candidates {
            if self.probe_from_guest(vm, candidate) {
                return Ok(candidate.trim_end_matches('/').to_string());
            }
        }
        if candidates.is_empty() {
            return Err(format!(
                "failed to resolve bridge endpoint for VM {}: no gateway candidates",
                quote(vm)
            ));
        }
        Err(format!(
            "failed to resolve reachable bridge endpoint for VM {} (probed {} candidate(s) on port {port})",
            quote(vm),
            candidates.len()
        ))
    }

    /// `readVMMachineArch`.
    fn vm_arch(&self, vm: &str) -> Result<&'static str, String> {
        let r = self.vm_exec(vm, &["uname", "-m"], Duration::from_secs(30))?;
        if r.exit_code != 0 {
            return Err(format!(
                "uname -m failed in {vm}: {}",
                first_non_empty(&[&r.stderr, &r.stdout, "unknown error"])
            ));
        }
        Ok(agent_arch(&r.stdout))
    }

    /// `RunAgentShellWithTimeout`.
    fn host_shell(
        &self,
        command: &str,
        timeout: Duration,
    ) -> Result<crate::hostobs::CommandResult, String> {
        let command = command.trim();
        if command.is_empty() {
            return Err("command is required".into());
        }
        let argv = ["bash", "-lc", command].map(String::from);
        Ok(crate::hostobs::run_command(&self.env, &argv, timeout))
    }

    /// `WaitForSystemdActive`.
    fn wait_host_service(&self, service: &str, timeout: Duration) -> Result<(), String> {
        let deadline = std::time::Instant::now() + timeout;
        let argv = [SYSTEMCTL, "is-active", service].map(String::from);
        while std::time::Instant::now() < deadline {
            let r = crate::hostobs::run_command(&self.env, &argv, Duration::ZERO);
            if r.exit_code == 0 && r.stdout.trim() == "active" {
                return Ok(());
            }
            std::thread::sleep(Duration::from_secs(2));
        }
        Err(format!(
            "systemd service '{service}' did not become active within {}",
            go_duration(timeout)
        ))
    }

    /// `WaitForVMServiceActive`.
    fn wait_vm_service(&self, vm: &str, service: &str, timeout: Duration) -> Result<(), String> {
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() < deadline {
            if let Ok(r) = self.vm_exec(
                vm,
                &[SYSTEMCTL, "is-active", service],
                Duration::from_secs(30),
            ) {
                if r.exit_code == 0 && r.stdout.trim() == "active" {
                    return Ok(());
                }
            }
            std::thread::sleep(Duration::from_secs(3));
        }
        Err(format!(
            "VM service '{service}' on '{vm}' did not become active within {}",
            go_duration(timeout)
        ))
    }

    /// `InstallClusterAgent`: in the named guest, or on the host for a
    /// cluster that names no VM (`k3s-host`, `external`).
    pub fn install(&self, args: &AgentArgs) -> Result<J, String> {
        for (value, field) in [
            (&args.cluster_id, "clusterId"),
            (&args.cluster_name, "clusterName"),
            (&args.agent_id, "agentId"),
            (&args.bridge_token, "bridgeToken"),
        ] {
            if value.trim().is_empty() {
                return Err(format!("{field} is required"));
            }
        }
        let vm = args.vm_name.trim();
        let host_native = vm.is_empty() && (args.source == "k3s-host" || args.source == "external");
        if vm.is_empty() && !host_native {
            return Err(format!(
                "vmName is required for VM-based cluster agent install (source {})",
                quote(&args.source)
            ));
        }
        if host_native {
            let mut bridge = args.bridge_url.trim().to_string();
            if bridge.is_empty() {
                bridge = self.bridge_url_from_env();
            }
            if bridge.trim().is_empty() {
                return Err("bridgeUrl is required for host-native cluster agent install".into());
            }
            let arch = match self.host_shell("uname -m", Duration::from_secs(30)) {
                Ok(r) if r.exit_code == 0 => agent_arch(&r.stdout),
                Ok(r) => {
                    return Err(format!(
                        "failed to read host architecture: {}",
                        first_non_empty(&[&r.stderr, &r.stdout])
                    ))
                }
                Err(e) => return Err(format!("failed to read host architecture: {e}")),
            };
            let bridge = bridge.trim_end_matches('/').to_string();
            let config = agent_config(&bridge, args);
            let script = privileged(&agent_install_script(&bridge, arch, &config));
            let r = self.host_shell(&script, AGENT_INSTALL_TIMEOUT)?;
            if r.exit_code != 0 {
                return Err(first_non_empty(&[
                    &r.stderr,
                    &r.stdout,
                    "host cluster agent install failed",
                ])
                .to_string());
            }
            self.wait_host_service(AGENT_SERVICE, AGENT_SERVICE_WAIT)?;
            return Ok(json!({
                "host": true,
                "bridgeUrl": bridge,
                "serviceName": AGENT_SERVICE,
                "status": "active",
                "arch": arch,
            }));
        }
        self.instances
            .wait_for_exec_ready(vm, Duration::from_secs(300))?;
        let mut bridge = args.bridge_url.trim().to_string();
        if bridge.is_empty() {
            bridge = self.bridge_url_from_env();
        }
        // A failed setup is reported through progress only.
        let _ = self.ensure_reachability(&bridge, args.bridge_port);
        let listen = self.listen_host();
        if !listen.is_empty() {
            let port = if args.bridge_port <= 0 {
                self.default_port()
            } else {
                args.bridge_port
            };
            let guest_bridge = format!("http://{listen}:{port}");
            if bridge.is_empty() || loopback_url(&bridge) || !self.probe_from_guest(vm, &bridge) {
                bridge = guest_bridge;
            }
        }
        let resolved = self.bridge_endpoint(vm, &bridge, args.bridge_port)?;
        let arch = self.vm_arch(vm)?;
        let config = agent_config(&resolved, args);
        let script = agent_install_script(&resolved, arch, &config);
        let r = self.vm_exec(vm, &["bash", "-lc", &script], AGENT_INSTALL_TIMEOUT)?;
        if r.exit_code != 0 {
            return Err(first_non_empty(&[
                &r.stderr,
                &r.stdout,
                "cluster agent install failed in VM",
            ])
            .to_string());
        }
        self.wait_vm_service(vm, AGENT_SERVICE, AGENT_SERVICE_WAIT)?;
        Ok(json!({
            "vmName": vm,
            "bridgeUrl": resolved,
            "serviceName": AGENT_SERVICE,
            "status": "active",
            "arch": arch,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GO: &str = include_str!("../../../tools/parity/cluster-format-oracle/expected.json");

    fn go() -> J {
        serde_json::from_str(GO).unwrap()
    }

    fn chart(release: &str, source: &str, namespace: &str, repo: &str, values: &str) -> ChartArgs {
        ChartArgs {
            vm_name: String::new(),
            release_name: release.into(),
            chart_source: source.into(),
            namespace: namespace.into(),
            repo: repo.into(),
            values: values.into(),
        }
    }

    #[test]
    fn formatting_matches_go() {
        let go = go();
        assert_eq!(
            chart_manifest(&chart("redis", "bitnami/redis", "", "", "")),
            go["chart_plain"]
        );
        assert_eq!(
            chart_manifest(&chart(
                "redis",
                "redis",
                " data ",
                " https://charts.example/x ",
                "a: 1\nb:\n  c: <x>&\n\n"
            )),
            go["chart_full"]
        );
        let values = json!({"z": 1, "a": {"b": "<&>", "c": [1.5, true, null]}, "e": {}, "f": []});
        assert_eq!(helm_values(Some(&values)), go["values_map"]);
        let args = AgentArgs {
            bridge_token: "t<o>k'en".into(),
            cluster_id: "c1".into(),
            cluster_name: "n".into(),
            agent_id: "a".into(),
            provider_id: "p".into(),
            ..AgentArgs::default()
        };
        let config = agent_config("http://10.0.0.1:9093", &args);
        assert_eq!(config, go["config"]);
        let script = agent_install_script("http://10.0.0.1:9093/", "arm64", &config);
        assert_eq!(script, go["script"]);
        assert_eq!(privileged(&script), go["privileged"]);
        // 2026-10-17T12:00:00Z.
        let now = parse_rfc3339("2026-10-17T12:00:00Z").unwrap();
        for (created, age) in go["ages"].as_object().unwrap() {
            assert_eq!(age_at(created, now), age.as_str().unwrap(), "{created}");
        }
    }

    #[test]
    fn names_validate_as_go() {
        for ok in ["a", "A1", "my-app", "x_y.z:w", " padded "] {
            assert!(validate_name(ok, "f").is_ok(), "{ok}");
        }
        for bad in ["", " ", "-a", ".a", "a b", "a/b", "ä"] {
            assert_eq!(
                validate_name(bad, "kind").unwrap_err(),
                "kind contains invalid characters",
                "{bad}"
            );
        }
    }

    #[test]
    fn cluster_list_decodes_with_go_errors() {
        let decode = |input: J| {
            let mut first = None;
            let out = decode_struct(
                input.as_object().unwrap(),
                "ClusterListResult",
                LIST,
                "",
                &mut first,
            );
            (out, first)
        };
        let cases = [
            (json!({"clusters": [{"cpu": "2", "name": "a"}], "total": 1}),
             "json: cannot unmarshal string into Go struct field ClusterDetail.clusters.cpu of type int"),
            (json!({"clusters": [{"nodes": [{"roles": "x"}], "name": 3}], "total": "1"}),
             "json: cannot unmarshal number into Go struct field ClusterDetail.clusters.name of type string"),
            (json!({"clusters": {"a": 1}}),
             "json: cannot unmarshal object into Go struct field ClusterListResult.clusters of type []clusterinfo.ClusterDetail"),
            (json!({"clusters": [{"NAME": "b", "Cpu": 1.5}], "total": 1}),
             "json: cannot unmarshal number 1.5 into Go struct field ClusterDetail.clusters.cpu of type int"),
            (json!({"total": 2.5}),
             "json: cannot unmarshal number 2.5 into Go struct field ClusterListResult.total of type int"),
            (json!({"clusters": [1]}),
             "json: cannot unmarshal number into Go struct field ClusterListResult.clusters of type clusterinfo.ClusterDetail"),
        ];
        for (input, error) in cases {
            assert_eq!(decode(input.clone()).1.as_deref(), Some(error), "{input}");
        }
        let (out, first) = decode(
            json!({"clusters": [{"NAME": "b", "instanceType": "vm", "extra": 1}], "total": 1}),
        );
        assert!(first.is_none());
        assert_eq!(out["clusters"][0]["name"], "b");
        assert!(out["clusters"][0].get("extra").is_none());
    }

    #[test]
    fn bridge_urls_parse_as_go() {
        // Go cuts at the first colon, inside the brackets.
        assert_eq!(bridge_url_host("https://[::1]:9093/x"), "");
        assert_eq!(bridge_url_host("10.0.0.1:9093"), "10.0.0.1");
        assert!(loopback_url("http://127.0.0.1:9093"));
        assert!(!loopback_url("localhost-free.example"));
        assert!(!loopback_url("http://10.0.0.1:9093"));
        assert_eq!(
            first_bridge_ipv4(&["10.42.0.1".into(), "192.168.1.5".into()]),
            "192.168.1.5"
        );
        assert_eq!(
            first_bridge_ipv4(&["1.2.3.4".into(), "10.123.0.9".into()]),
            "10.123.0.9"
        );
        assert_eq!(first_bridge_ipv4(&["10.42.0.1".into()]), "10.42.0.1");
    }
}
