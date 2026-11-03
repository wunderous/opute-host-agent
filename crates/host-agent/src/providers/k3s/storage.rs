//! Guest storage (Go `storage.go`, `storage_registry.go`): disk inspection,
//! unused-image pruning, registry garbage collection and fstrim.

use super::decode::{anon, field, items, slice, text, unmarshal, Shape};
use super::{first_non_empty, optional_string_slice, run_command, s, shell_quote, Args, Outcome};
use crate::goerr::quote;
use serde_json::{json, Value as J};
use std::collections::BTreeSet;
use std::time::{Duration, Instant, SystemTime};

const MIN_GUEST_IMAGE_AGE_SECONDS: i64 = 3600;
const DEFAULT_REGISTRY_NAME: &str = "opute-registry";
const DEFAULT_REGISTRY_NAMESPACE: &str = "registry-system";
const REGISTRY_SCALE_TIMEOUT: Duration = Duration::from_secs(2 * 60);
const REGISTRY_JOB_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const REGISTRY_READY_TIMEOUT: Duration = Duration::from_secs(3 * 60);
const REGISTRY_POLL: Duration = Duration::from_secs(2);
const MISSING_CREATED: &str = "crictl images did not report createdAt; unused images are treated as age-eligible";

pub(super) struct GuestImage {
    pub(super) id: String,
    pub(super) tags: Vec<String>,
    pub(super) digests: Vec<String>,
    pub(super) size_bytes: i64,
    /// Nanoseconds since the Unix epoch.
    pub(super) created_at: Option<i128>,
    pub(super) pinned: bool,
}

struct GuestContainer {
    id: String,
    image: String,
    image_id: String,
    state: String,
}

pub(super) struct Candidate {
    pub(super) id: String,
    pub(super) tags: Vec<String>,
    pub(super) size_bytes: i64,
}

pub(super) type KeepSet = BTreeSet<String>;

fn warning_list(warnings: &[String]) -> J {
    J::from(warnings.to_vec())
}

pub(super) fn inspect_guest_storage(args: &Args) -> Outcome {
    let mut warnings = Vec::new();
    let root = inspect_root_filesystem(args).unwrap_or_else(|e| {
        warnings.push(e);
        json!({})
    });
    // Go walks a map here, in random order; the paths go in key order.
    let (containerd, err) = inspect_guest_paths(
        args,
        &[
            ("containerdBytes", "/var/lib/rancher/k3s/agent/containerd"),
            ("contentBytes", "/var/lib/rancher/k3s/agent/containerd/io.containerd.content.v1.content"),
            ("k3sStorageBytes", "/var/lib/rancher/k3s/storage"),
            ("snapshotsBytes", "/var/lib/rancher/k3s/agent/containerd/io.containerd.snapshotter.v1.overlayfs"),
        ],
    );
    if let Some(e) = err {
        warnings.push(e);
    }
    let pvcs = match inspect_pvc_dirs(args) {
        Ok(pvcs) => J::Array(pvcs),
        Err(e) => {
            warnings.push(e);
            J::Null
        }
    };
    let images = list_guest_images(args)?;
    let mut keep = keep_set_from_running_pods(args)?;
    add_keep_refs(&mut keep, &optional_string_slice(args, "extraKeepTags"));
    let (candidates, skipped, missing_created) =
        plan_unused_images(&images, &keep, MIN_GUEST_IMAGE_AGE_SECONDS, now_nanos());
    if missing_created {
        warnings.push(MISSING_CREATED.into());
    }
    let reclaimable: i64 = candidates.iter().map(|c| c.size_bytes).sum();
    let mut report = json!({
        "targetUri": s(args, "targetUri"),
        "rootFilesystem": root,
        "containerd": containerd,
        "persistentVolumes": pvcs,
        "imageCount": images.len(),
        "keepSet": keep.iter().collect::<Vec<_>>(),
        "reclaimableImageBytes": reclaimable,
        "reclaimableImages": candidates_json(&candidates),
        "keptImages": skipped,
    });
    if args.bool("includeRegistry", false) {
        match inspect_registry_inventory(args) {
            Ok(registry) => report["registry"] = registry,
            Err(e) => warnings.push(e),
        }
    }
    if !warnings.is_empty() {
        report["warnings"] = warning_list(&warnings);
    }
    Ok(report)
}

pub(super) fn prune_unused_images(args: &Args) -> Outcome {
    let dry_run = args.bool("dryRun", false);
    let min_age = guest_min_age_seconds(args);
    let images = list_guest_images(args)?;
    let containers = list_guest_containers(args)?;
    let mut keep = keep_set_from_running_pods(args)?;
    add_keep_refs(&mut keep, &optional_string_slice(args, "extraKeepTags"));
    for container in &containers {
        if is_running(&container.state) {
            add_keep_refs(&mut keep, &[container.image.clone(), container.image_id.clone()]);
        }
    }
    let before: i64 = images.iter().map(|i| i.size_bytes).sum();
    let exited: Vec<&GuestContainer> =
        containers.iter().filter(|c| !is_running(&c.state) && !c.id.trim().is_empty()).collect();
    let (candidates, skipped, missing_created) = plan_unused_images(&images, &keep, min_age, now_nanos());
    let mut warnings = Vec::new();
    if missing_created {
        warnings.push(MISSING_CREATED.to_string());
    }
    let mut result = json!({
        "targetUri": s(args, "targetUri"),
        "dryRun": dry_run,
        "minAgeSeconds": min_age,
        "keepSet": keep.iter().collect::<Vec<_>>(),
        "beforeImageBytes": before,
        "exitedContainers": exited.len(),
        "reclaimableImages": candidates_json(&candidates),
        "skippedImages": skipped,
    });
    if dry_run {
        result["afterImageBytes"] = J::from(before);
        result["removedImageCount"] = J::from(0);
        result["removedContainerCount"] = J::from(0);
        if !warnings.is_empty() {
            result["warnings"] = warning_list(&warnings);
        }
        return Ok(result);
    }
    let mut removed_containers = 0;
    for container in &exited {
        if let Err(e) = run_guest_crictl(args, &["rm", &container.id]) {
            warnings.push(format!("remove exited container {}: {e}", container.id));
            continue;
        }
        removed_containers += 1;
    }
    let (mut removed_images, mut reclaimed) = (0, 0i64);
    for candidate in &candidates {
        if let Err(e) = run_guest_crictl(args, &["rmi", &candidate.id]) {
            let name = first_non_empty(&[&candidate.tags.join(","), &candidate.id]);
            warnings.push(format!("remove image {name}: {e}"));
            continue;
        }
        removed_images += 1;
        reclaimed += candidate.size_bytes;
    }
    match list_guest_images(args) {
        Ok(after) => result["afterImageBytes"] = J::from(after.iter().map(|i| i.size_bytes).sum::<i64>()),
        Err(e) => {
            warnings.push(e);
            result["afterImageBytes"] = J::from(before - reclaimed);
        }
    }
    result["removedImageCount"] = J::from(removed_images);
    result["removedContainerCount"] = J::from(removed_containers);
    result["reclaimedImageBytes"] = J::from(reclaimed);
    if !warnings.is_empty() {
        result["warnings"] = warning_list(&warnings);
    }
    Ok(result)
}

pub(super) fn trim_guest_storage(args: &Args) -> Outcome {
    let guest_err = match run_guest(args, &["fstrim", "-v", "/"]) {
        Ok(output) => {
            return Ok(json!({
                "targetUri": s(args, "targetUri"),
                "trimmed": true,
                "scope": "guest",
                "output": String::from_utf8_lossy(&output).trim(),
            }))
        }
        Err(e) => e,
    };
    let lowered = guest_err.to_lowercase();
    if !lowered.contains("operation not permitted") && !lowered.contains("fitrim") {
        return Err(format!("fstrim guest root: {guest_err}"));
    }
    let (output, _) = super::spawn_and_wait("sudo", &["-n", "fstrim", "-v", "/"], None, None, true).map_err(
        |(err, message)| format!("fstrim guest root not permitted and host fstrim failed: {err}: {message}"),
    )?;
    Ok(json!({
        "targetUri": s(args, "targetUri"),
        "trimmed": true,
        "scope": "host",
        "guestError": guest_err,
        "output": String::from_utf8_lossy(&output).trim(),
    }))
}

fn inspect_root_filesystem(args: &Args) -> Result<J, String> {
    let output = run_guest(args, &["df", "-B1", "/"]).map_err(|e| format!("df: {e}"))?;
    parse_df_bytes(&String::from_utf8_lossy(&output))
}

fn inspect_guest_paths(args: &Args, paths: &[(&str, &str)]) -> (J, Option<String>) {
    let mut report = serde_json::Map::new();
    let mut first = None;
    for (key, path) in paths {
        match run_guest(args, &["du", "-sb", path]) {
            Ok(output) => {
                if let Some((size, _)) = parse_du_line(String::from_utf8_lossy(&output).trim()) {
                    report.insert((*key).into(), J::from(size));
                }
            }
            Err(e) => {
                first.get_or_insert(e);
            }
        }
    }
    (J::Object(report), first)
}

fn inspect_pvc_dirs(args: &Args) -> Result<Vec<J>, String> {
    let output = run_guest(args, &["bash", "-lc", "du -sb /var/lib/rancher/k3s/storage/* 2>/dev/null || true"])?;
    let text = String::from_utf8_lossy(&output).into_owned();
    let mut pvcs: Vec<(String, i64)> = text
        .trim()
        .split('\n')
        .filter_map(|line| parse_du_line(line.trim()))
        .filter(|(_, path)| !path.is_empty())
        .map(|(size, path)| (path, size))
        .collect();
    pvcs.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(pvcs.into_iter().map(|(path, size)| json!({"path": path, "bytes": size})).collect())
}

fn list_guest_images(args: &Args) -> Result<Vec<GuestImage>, String> {
    let output = run_guest_crictl(args, &["images", "-o", "json"]).map_err(|e| format!("list guest images: {e}"))?;
    parse_cri_images(&output)
}

fn list_guest_containers(args: &Args) -> Result<Vec<GuestContainer>, String> {
    let output = run_guest_crictl(args, &["ps", "-a", "-o", "json"]).map_err(|e| format!("list guest containers: {e}"))?;
    parse_cri_containers(&output)
}

fn keep_set_from_running_pods(args: &Args) -> Result<KeepSet, String> {
    let output = run_guest_kubectl(args, &["get", "pods", "-A", "-o", "json"])
        .map_err(|e| format!("list pods for image keep-set: {e}"))?;
    Ok(keep_set_from_pod_json(&output))
}

/// The images every pod names; an unreadable document keeps nothing.
pub(super) fn keep_set_from_pod_json(raw: &[u8]) -> KeepSet {
    let image = || slice(anon(vec![field("Image", "image", Shape::Str)]));
    let shape = anon(vec![field(
        "Items",
        "items",
        slice(anon(vec![field(
            "Spec",
            "spec",
            anon(vec![
                field("InitContainers", "initContainers", image()),
                field("Containers", "containers", image()),
                field("EphemeralContainers", "ephemeralContainers", image()),
            ]),
        )])),
    )]);
    let mut keep = KeepSet::new();
    let Ok(document) = unmarshal(raw, &shape) else {
        return keep;
    };
    for item in items(&document, "items") {
        for key in ["initContainers", "containers", "ephemeralContainers"] {
            for container in items(&item["spec"], key) {
                add_keep_refs(&mut keep, &[text(container, "image").to_string()]);
            }
        }
    }
    keep
}

fn add_keep_refs(keep: &mut KeepSet, refs: &[String]) {
    for reference in refs {
        keep.extend(image_aliases(reference));
    }
}

/// `imageAliases`: the forms a reference may be matched under.
pub(super) fn image_aliases(reference: &str) -> Vec<String> {
    let reference = reference.trim();
    if reference.is_empty() {
        return Vec::new();
    }
    let mut aliases: Vec<String> = Vec::new();
    let mut add = |value: &str| {
        let value = value.to_lowercase().trim().to_string();
        if !value.is_empty() && !aliases.contains(&value) {
            aliases.push(value);
        }
    };
    add(reference);
    let lowered = reference.to_lowercase();
    let mut trimmed = lowered.as_str();
    if let Some(digest) = trimmed.find("@sha256:") {
        add(&trimmed[..digest]);
        add(&trimmed[digest + 1..]);
        add(&trimmed[digest + "@sha256:".len()..]);
        trimmed = &trimmed[..digest];
    }
    let repo_owned;
    if let Some(repo) = strip_registry_host(trimmed) {
        add(repo);
        repo_owned = repo.to_string();
        trimmed = &repo_owned;
    }
    if let Some(rest) = trimmed.strip_prefix("library/") {
        add(rest);
    }
    aliases
}

fn strip_registry_host(reference: &str) -> Option<&str> {
    let (host, rest) = reference.split_once('/')?;
    (host == "localhost" || host.contains('.') || host.contains(':')).then_some(rest)
}

pub(super) fn plan_unused_images(
    images: &[GuestImage],
    keep: &KeepSet,
    min_age_seconds: i64,
    now: i128,
) -> (Vec<Candidate>, Vec<J>, bool) {
    let mut candidates = Vec::new();
    let mut skipped = Vec::new();
    let mut missing_created = false;
    let skip = |image: &GuestImage, reason: &str| {
        json!({"id": image.id, "tags": image.tags, "sizeBytes": image.size_bytes, "reason": reason})
    };
    for image in images {
        if image.pinned || image_matches_keep_set(image, keep) {
            skipped.push(skip(image, "keep-set"));
            continue;
        }
        if let Some(created) = image.created_at {
            // time.Time.Sub saturates; seconds times 1e9 stay in range here.
            if now - created < i128::from(min_age_seconds) * 1_000_000_000 {
                skipped.push(skip(image, "age-gate"));
                continue;
            }
        } else {
            missing_created = true;
        }
        candidates.push(Candidate { id: image.id.clone(), tags: image.tags.clone(), size_bytes: image.size_bytes });
    }
    (candidates, skipped, missing_created)
}

fn image_matches_keep_set(image: &GuestImage, keep: &KeepSet) -> bool {
    std::iter::once(&image.id)
        .chain(&image.tags)
        .chain(&image.digests)
        .any(|reference| image_aliases(reference).iter().any(|alias| keep.contains(alias)))
}

fn parse_cri_images(raw: &[u8]) -> Result<Vec<GuestImage>, String> {
    let shape = anon(vec![field(
        "Images",
        "images",
        slice(anon(vec![
            field("ID", "id", Shape::Str),
            field("RepoTags", "repoTags", slice(Shape::Str)),
            field("RepoDigests", "repoDigests", slice(Shape::Str)),
            field("Size", "size", Shape::Any),
            field("CreatedAt", "createdAt", Shape::Any),
            field("Pinned", "pinned", Shape::Bool),
        ])),
    )]);
    let document = unmarshal(raw, &shape).map_err(|e| format!("parse crictl images: {e}"))?;
    Ok(items(&document, "images")
        .iter()
        .map(|item| GuestImage {
            id: text(item, "id").trim().to_string(),
            tags: clean_strings(items(item, "repoTags")),
            digests: clean_strings(items(item, "repoDigests")),
            size_bytes: any_to_i64(&item["size"]),
            created_at: parse_cri_time(&item["createdAt"]),
            pinned: item["pinned"].as_bool().unwrap_or(false),
        })
        .collect())
}

fn parse_cri_containers(raw: &[u8]) -> Result<Vec<GuestContainer>, String> {
    let shape = anon(vec![field(
        "Containers",
        "containers",
        slice(anon(vec![
            field("ID", "id", Shape::Str),
            field("State", "state", Shape::Str),
            field("Image", "image", anon(vec![field("Image", "image", Shape::Str)])),
            field("ImageRef", "imageRef", Shape::Str),
        ])),
    )]);
    let document = unmarshal(raw, &shape).map_err(|e| format!("parse crictl ps: {e}"))?;
    Ok(items(&document, "containers")
        .iter()
        .map(|item| GuestContainer {
            id: text(item, "id").trim().to_string(),
            image: text(&item["image"], "image").trim().to_string(),
            image_id: text(item, "imageRef").trim().to_string(),
            state: text(item, "state").trim().to_string(),
        })
        .collect())
}

/// `strconv.ParseInt(s, 10, 64)`: the value (saturated on a range error)
/// and whether it parsed without error.
fn parse_int(s: &str) -> (i64, bool) {
    let digits = s.strip_prefix(['+', '-']).unwrap_or(s);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return (0, false);
    }
    match s.parse::<i64>() {
        Ok(v) => (v, true),
        Err(_) if s.starts_with('-') => (i64::MIN, false),
        Err(_) => (i64::MAX, false),
    }
}

/// `unixToTime`: seconds, or nanoseconds past 1e12.
fn unix_to_nanos(value: i64) -> i128 {
    if value > 1_000_000_000_000 {
        i128::from(value)
    } else {
        i128::from(value) * 1_000_000_000
    }
}

fn system_time_nanos(t: SystemTime) -> i128 {
    match t.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(d) => d.as_nanos() as i128,
        Err(e) => -(e.duration().as_nanos() as i128),
    }
}

fn now_nanos() -> i128 {
    system_time_nanos(SystemTime::now())
}

pub(super) fn parse_cri_time(value: &J) -> Option<i128> {
    match value {
        J::String(raw) => {
            let text = raw.trim();
            if text.is_empty() || text == "0" {
                return None;
            }
            let (unix, ok) = parse_int(text);
            if ok {
                return (unix > 0).then(|| unix_to_nanos(unix));
            }
            crate::hostobs::parse_rfc3339(text).map(system_time_nanos)
        }
        J::Number(n) => {
            let f = n.as_f64()?;
            (f > 0.0).then(|| unix_to_nanos(f as i64))
        }
        _ => None,
    }
}

pub(super) fn parse_df_bytes(output: &str) -> Result<J, String> {
    let lines: Vec<&str> = output.trim().split('\n').collect();
    if lines.len() < 2 {
        return Err("df output was empty".into());
    }
    let fields: Vec<&str> = lines[lines.len() - 1].split_whitespace().collect();
    if fields.len() < 6 {
        return Err("df output was malformed".into());
    }
    Ok(json!({
        "filesystem": fields[0],
        "totalBytes": parse_int(fields[1]).0,
        "usedBytes": parse_int(fields[2]).0,
        "availBytes": parse_int(fields[3]).0,
        "mount": fields[fields.len() - 1],
    }))
}

pub(super) fn parse_du_line(line: &str) -> Option<(i64, String)> {
    let fields: Vec<&str> = line.split_whitespace().collect();
    if fields.len() < 2 {
        return None;
    }
    let (size, ok) = parse_int(fields[0]);
    ok.then(|| (size, fields[1..].join(" ")))
}

fn is_running(state: &str) -> bool {
    let normalized = state.trim().to_uppercase();
    normalized.contains("RUNNING") || normalized == "CONTAINER_CREATED"
}

fn candidates_json(candidates: &[Candidate]) -> Vec<J> {
    candidates
        .iter()
        .map(|c| json!({"id": c.id, "tags": c.tags, "sizeBytes": c.size_bytes, "reason": "unused"}))
        .collect()
}

fn guest_min_age_seconds(args: &Args) -> i64 {
    let value = args.int("minAgeSeconds", 0);
    if value < MIN_GUEST_IMAGE_AGE_SECONDS {
        MIN_GUEST_IMAGE_AGE_SECONDS
    } else {
        value
    }
}

fn clean_strings(values: &[J]) -> Vec<String> {
    values
        .iter()
        .map(|v| v.as_str().unwrap_or("").trim().to_string())
        .filter(|v| !v.is_empty() && v != "<none>")
        .collect()
}

/// `anyToInt64` over a decoded `any`.
fn any_to_i64(value: &J) -> i64 {
    match value {
        J::Number(n) => n.as_f64().map_or(0, |f| f as i64),
        J::String(text) => parse_int(text.trim()).0,
        _ => 0,
    }
}

fn run_guest(args: &Args, argv: &[&str]) -> Result<Vec<u8>, String> {
    let instance = s(args, "providerInstanceName");
    let mut command = vec!["exec", instance.as_str(), "--"];
    command.extend_from_slice(argv);
    run_command(&command, None)
}

fn run_guest_kubectl(args: &Args, kubectl_args: &[&str]) -> Result<Vec<u8>, String> {
    let mut argv = vec!["k3s", "kubectl"];
    argv.extend_from_slice(kubectl_args);
    run_guest(args, &argv)
}

fn run_guest_crictl(args: &Args, crictl_args: &[&str]) -> Result<Vec<u8>, String> {
    if crictl_args.first() == Some(&"rmi") && crictl_args[1..].iter().any(|a| *a == "--all" || *a == "-a") {
        return Err("refusing crictl rmi --all".into());
    }
    let mut argv = vec!["k3s", "crictl"];
    argv.extend_from_slice(crictl_args);
    run_guest(args, &argv)
}

// --- the in-cluster registry -----------------------------------------------------------

pub(super) struct Registry {
    pub(super) namespace: String,
    pub(super) name: String,
    pub(super) image: String,
    pub(super) replicas: i64,
    pub(super) claim_name: String,
    pub(super) config_name: String,
    pub(super) service_ip: String,
    pub(super) service_port: i64,
}

struct Tag {
    repository: String,
    tag: String,
}

fn tags_json(tags: &[&Tag]) -> Vec<J> {
    tags.iter().map(|t| json!({"repository": t.repository, "tag": t.tag})).collect()
}

pub(super) fn garbage_collect_registry(args: &Args) -> Outcome {
    if !args.bool("includeRegistry", false) {
        return Ok(json!({"targetUri": s(args, "targetUri"), "skipped": true, "reason": "includeRegistry is false"}));
    }
    let dry_run = args.bool("dryRun", false);
    let registry = discover_registry(args)?;
    let mut keep = keep_set_from_running_pods(args)?;
    add_keep_refs(&mut keep, &optional_string_slice(args, "extraKeepTags"));
    let tags = list_registry_tags(args, &registry)?;
    let (delete, retained): (Vec<&Tag>, Vec<&Tag>) = tags.iter().partition(|tag| {
        !image_aliases(&format!("{}:{}", tag.repository, tag.tag)).iter().any(|alias| keep.contains(alias))
    });
    let mut result = json!({
        "targetUri": s(args, "targetUri"),
        "dryRun": dry_run,
        "registry": registry.name,
        "namespace": registry.namespace,
        "tagCount": tags.len(),
        "tagsToDelete": tags_json(&delete),
        "retainedTags": tags_json(&retained),
        "originalReplicas": registry.replicas,
    });
    if dry_run {
        result["deletedManifests"] = J::from(0);
        result["garbageCollected"] = J::Bool(false);
        result["restored"] = J::Bool(false);
        return Ok(result);
    }
    let mut deleted = 0;
    let mut warnings = Vec::new();
    for tag in &delete {
        match delete_registry_manifest(args, &registry, tag) {
            Ok(()) => deleted += 1,
            Err(e) => warnings.push(e),
        }
    }
    let restore = if registry.replicas <= 0 { 1 } else { registry.replicas };
    scale_registry(args, &registry, 0)?;
    if let Err(e) = wait_for_registry_replicas(args, &registry, 0) {
        let _ = scale_registry(args, &registry, restore);
        return Err(e);
    }
    if let Err(e) = run_registry_gc_job(args, &registry) {
        let _ = scale_registry(args, &registry, restore);
        return Err(e);
    }
    scale_registry(args, &registry, restore)?;
    wait_for_registry_ready(args, &registry)?;
    match list_registry_tags(args, &registry) {
        Ok(after) => result["retainedAfter"] = J::from(tags_json(&after.iter().collect::<Vec<_>>())),
        Err(e) => warnings.push(e),
    }
    result["deletedManifests"] = J::from(deleted);
    result["garbageCollected"] = J::Bool(true);
    result["restored"] = J::Bool(true);
    result["ready"] = J::Bool(true);
    if !warnings.is_empty() {
        result["warnings"] = warning_list(&warnings);
    }
    Ok(result)
}

fn inspect_registry_inventory(args: &Args) -> Outcome {
    let registry = discover_registry(args)?;
    let tags = match list_registry_tags(args, &registry) {
        Ok(tags) => tags,
        Err(e) => return Ok(json!({"name": registry.name, "namespace": registry.namespace, "error": e})),
    };
    let repos: BTreeSet<&str> = tags.iter().map(|t| t.repository.as_str()).collect();
    Ok(json!({"name": registry.name, "namespace": registry.namespace, "tagCount": tags.len(), "repoCount": repos.len()}))
}

/// `mapString`: a nested string, trimmed.
fn map_string(object: &J, path: &[&str]) -> String {
    super::nested_string(object, path).unwrap_or_default().trim().to_string()
}

fn discover_registry(args: &Args) -> Result<Registry, String> {
    let want_name = first_non_empty(&[&s(args, "registryName"), DEFAULT_REGISTRY_NAME]);
    let want_namespace = s(args, "registryNamespace");
    let output = run_guest_kubectl(args, &["get", "deploy", "-A", "-o", "json"])
        .map_err(|e| format!("list registry deployments: {e}"))?;
    let document = unmarshal(&output, &anon(vec![field("Items", "items", slice(Shape::MapAny))]))
        .map_err(|e| format!("parse registry deployments: {e}"))?;
    let found = items(&document, "items").iter().find(|item| {
        let name = map_string(item, &["metadata", "name"]);
        let namespace = map_string(item, &["metadata", "namespace"]);
        if !want_namespace.is_empty() && namespace != want_namespace {
            return false;
        }
        name == want_name || (name == "registry" && namespace == DEFAULT_REGISTRY_NAMESPACE)
    });
    let Some(found) = found else {
        return Err(format!("registry deployment {} was not found", quote(&want_name)));
    };
    let volumes = super::nested_value(found, &["spec", "template", "spec", "volumes"])
        .and_then(J::as_array)
        .cloned()
        .unwrap_or_default();
    let volume_name = |kind: &str, key: &str| {
        volumes
            .iter()
            .filter_map(|v| v.get(kind).filter(|c| c.is_object()))
            .map(|c| c.get(key).and_then(J::as_str).unwrap_or("").trim().to_string())
            .find(|name| !name.is_empty())
            .unwrap_or_default()
    };
    let image = super::nested_value(found, &["spec", "template", "spec", "containers"])
        .and_then(J::as_array)
        .and_then(|c| c.first())
        .and_then(|c| c.get("image"))
        .and_then(J::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let mut registry = Registry {
        namespace: map_string(found, &["metadata", "namespace"]),
        name: map_string(found, &["metadata", "name"]),
        image,
        replicas: super::nested_number(found, &["spec", "replicas"]) as i64,
        claim_name: volume_name("persistentVolumeClaim", "claimName"),
        config_name: volume_name("configMap", "name"),
        service_ip: String::new(),
        service_port: 0,
    };
    if registry.replicas < 0 {
        registry.replicas = 1;
    }
    let service = run_guest_kubectl(args, &["-n", &registry.namespace, "get", "svc", &registry.name, "-o", "json"])
        .map_err(|e| format!("get registry service: {e}"))?;
    let svc = unmarshal(&service, &Shape::MapAny).map_err(|e| format!("parse registry service: {e}"))?;
    registry.service_ip = map_string(&svc, &["spec", "clusterIP"]);
    registry.service_port = super::nested_value(&svc, &["spec", "ports"])
        .and_then(J::as_array)
        .and_then(|ports| ports.first())
        .map(|port| super::nested_number(port, &["port"]))
        .filter(|p| *p > 0.0)
        .map_or(5000, |p| p as i64);
    if registry.service_ip.parse::<std::net::IpAddr>().is_err() || registry.service_port <= 0 {
        return Err(format!("registry service {}/{} has no cluster IP", registry.namespace, registry.name));
    }
    Ok(registry)
}

fn list_registry_tags(args: &Args, registry: &Registry) -> Result<Vec<Tag>, String> {
    let catalog_raw = registry_http(args, registry, "GET", "/v2/_catalog")?;
    let catalog = unmarshal(&catalog_raw, &anon(vec![field("Repositories", "repositories", slice(Shape::Str))]))
        .map_err(|e| format!("parse registry catalog: {e}"))?;
    let mut tags = Vec::new();
    for repo in items(&catalog, "repositories") {
        let repo = repo.as_str().unwrap_or("");
        if repo.is_empty() || super::has_unsafe(repo, "\0\r\n ") {
            continue;
        }
        let list_raw = registry_http(args, registry, "GET", &format!("/v2/{repo}/tags/list"))?;
        let listed = unmarshal(
            &list_raw,
            &anon(vec![field("Name", "name", Shape::Str), field("Tags", "tags", slice(Shape::Str))]),
        )
        .map_err(|e| format!("parse tags for {repo}: {e}"))?;
        for tag in items(&listed, "tags") {
            let tag = tag.as_str().unwrap_or("");
            if !tag.trim().is_empty() {
                tags.push(Tag { repository: repo.to_string(), tag: tag.to_string() });
            }
        }
    }
    tags.sort_by(|a, b| a.repository.cmp(&b.repository).then_with(|| a.tag.cmp(&b.tag)));
    Ok(tags)
}

fn delete_registry_manifest(args: &Args, registry: &Registry, tag: &Tag) -> Result<(), String> {
    let digest = registry_manifest_digest(args, registry, tag)?;
    registry_http(args, registry, "DELETE", &format!("/v2/{}/manifests/{digest}", tag.repository)).map(|_| ())
}

fn registry_manifest_digest(args: &Args, registry: &Registry, tag: &Tag) -> Result<String, String> {
    let script = format!(
        "curl -sI -H 'Accept: application/vnd.oci.image.manifest.v1+json, application/vnd.docker.distribution.manifest.v2+json' {} | tr -d '\\r' | awk -F': ' 'tolower($1)==\"docker-content-digest\"{{print $2}}'",
        registry_url(registry, &format!("/v2/{}/manifests/{}", tag.repository, tag.tag))
    );
    let output = run_guest(args, &["bash", "-lc", &script])
        .map_err(|e| format!("resolve digest for {}:{}: {e}", tag.repository, tag.tag))?;
    let digest = String::from_utf8_lossy(&output).trim().to_string();
    if !digest.starts_with("sha256:") {
        return Err(format!("registry did not return a digest for {}:{}", tag.repository, tag.tag));
    }
    Ok(digest)
}

fn registry_http(args: &Args, registry: &Registry, method: &str, path: &str) -> Result<Vec<u8>, String> {
    let endpoint = registry_url(registry, path);
    let script = format!("curl -sf --max-time 30 -X {} {}", shell_quote(method), shell_quote(&endpoint));
    run_guest(args, &["bash", "-lc", &script]).map_err(|e| format!("registry {method} {path}: {e}"))
}

fn registry_url(registry: &Registry, path: &str) -> String {
    let path = if path.starts_with('/') { path.to_string() } else { format!("/{path}") };
    let host = if registry.service_ip.contains(':') {
        format!("[{}]", registry.service_ip)
    } else {
        registry.service_ip.clone()
    };
    format!("http://{host}:{}{path}", registry.service_port)
}

fn scale_registry(args: &Args, registry: &Registry, replicas: i64) -> Result<(), String> {
    run_guest_kubectl(
        args,
        &["-n", &registry.namespace, "scale", &format!("deploy/{}", registry.name), &format!("--replicas={replicas}")],
    )
    .map(|_| ())
    .map_err(|e| format!("scale registry to {replicas}: {e}"))
}

/// fmt's `%v` for a float64: shortest digits, exponent form below 1e-4 and
/// from 1e21.
pub(super) fn go_v_float(f: f64) -> String {
    let scientific = format!("{f:e}");
    let (mantissa, exponent) = scientific.split_once('e').unwrap_or((&scientific, "0"));
    let exponent: i32 = exponent.parse().unwrap_or(0);
    if f != 0.0 && !(-4..21).contains(&exponent) {
        let sign = if exponent < 0 { '-' } else { '+' };
        format!("{mantissa}e{sign}{:02}", exponent.abs())
    } else {
        format!("{f}")
    }
}

fn wait_for_registry_replicas(args: &Args, registry: &Registry, want: i64) -> Result<(), String> {
    let deadline = Instant::now() + REGISTRY_SCALE_TIMEOUT;
    let mut last = None;
    while Instant::now() < deadline {
        match run_guest_kubectl(args, &["-n", &registry.namespace, "get", "deploy", &registry.name, "-o", "json"]) {
            Err(e) => last = Some(e),
            Ok(output) => match unmarshal(&output, &Shape::MapAny) {
                Err(e) => last = Some(e),
                Ok(deploy) => {
                    let replicas = super::nested_number(&deploy, &["status", "replicas"]);
                    if replicas as i64 == want {
                        return Ok(());
                    }
                    last = Some(format!("registry replicas = {}, want {want}", go_v_float(replicas)));
                }
            },
        }
        std::thread::sleep(REGISTRY_POLL);
    }
    Err(last.unwrap_or_else(|| format!("timed out waiting for registry replicas={want}")))
}

fn wait_for_registry_ready(args: &Args, registry: &Registry) -> Result<(), String> {
    let deadline = Instant::now() + REGISTRY_READY_TIMEOUT;
    let mut last = None;
    while Instant::now() < deadline {
        let target = format!("deploy/{}", registry.name);
        match run_guest_kubectl(
            args,
            &["-n", &registry.namespace, "wait", "--for=condition=available", &target, "--timeout=15s"],
        ) {
            Ok(_) => return Ok(()),
            Err(e) => last = Some(e),
        }
        std::thread::sleep(REGISTRY_POLL);
    }
    Err(last.unwrap_or_else(|| "timed out waiting for registry Ready".into()))
}

fn run_registry_gc_job(args: &Args, registry: &Registry) -> Result<(), String> {
    let namespace = registry.namespace.as_str();
    let _ = run_guest_kubectl(args, &["-n", namespace, "delete", "job", "opute-registry-gc", "--ignore-not-found=true"]);
    let manifest = registry_gc_job_manifest(registry);
    let instance = s(args, "providerInstanceName");
    run_command(&["exec", &instance, "--", "k3s", "kubectl", "apply", "-f", "-"], Some(manifest.as_bytes()))
        .map_err(|e| format!("apply registry garbage-collect job: {e}"))?;
    let deadline = Instant::now() + REGISTRY_JOB_TIMEOUT;
    let mut last = None;
    while Instant::now() < deadline {
        match run_guest_kubectl(
            args,
            &["-n", namespace, "wait", "--for=condition=complete", "job/opute-registry-gc", "--timeout=15s"],
        ) {
            Ok(_) => return Ok(()),
            Err(e) => last = Some(e),
        }
        let failed = run_guest_kubectl(
            args,
            &["-n", namespace, "get", "job", "opute-registry-gc", "-o", "jsonpath={.status.failed}"],
        )
        .unwrap_or_default();
        let failed = String::from_utf8_lossy(&failed).trim().to_string();
        if !failed.is_empty() && failed != "0" {
            let logs = run_guest_kubectl(args, &["-n", namespace, "logs", "job/opute-registry-gc"]).unwrap_or_default();
            return Err(format!(
                "registry garbage-collect job failed: {}",
                String::from_utf8_lossy(&logs).trim()
            ));
        }
        std::thread::sleep(REGISTRY_POLL);
    }
    Err(last.unwrap_or_else(|| "timed out waiting for registry garbage-collect job".into()))
}

pub(super) fn registry_gc_job_manifest(registry: &Registry) -> String {
    let image = first_non_empty(&[&registry.image, "registry:3"]);
    let claim = first_non_empty(&[&registry.claim_name, &registry.name]);
    let (config_mounts, config_volume) = match registry.config_name.trim() {
        "" => (String::new(), String::new()),
        name => (
            "\n        - name: config\n          mountPath: /etc/distribution".to_string(),
            format!("\n      - name: config\n        configMap:\n          name: {name}"),
        ),
    };
    format!(
        "apiVersion: batch/v1
kind: Job
metadata:
  name: opute-registry-gc
  namespace: {}
spec:
  ttlSecondsAfterFinished: 300
  backoffLimit: 1
  template:
    spec:
      restartPolicy: Never
      containers:
      - name: gc
        image: {image}
        command: [\"registry\", \"garbage-collect\", \"--delete-untagged\", \"/etc/distribution/config.yml\"]
        volumeMounts:
        - name: data
          mountPath: /var/lib/registry{config_mounts}
      volumes:
      - name: data
        persistentVolumeClaim:
          claimName: {claim}{config_volume}
",
        registry.namespace
    )
}

