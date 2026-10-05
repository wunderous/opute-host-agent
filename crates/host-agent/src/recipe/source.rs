//! Port of the shared portion of `internal/recipe/recipe.go` and
//! `source.go`: the capability-neutral envelope pieces, source loading
//! (local file / `github:` / raw GitHub URL, with immutable commit pinning
//! and optional sha256 verification), and the canonical hash helper every
//! recipe family shares.

use crate::plan::interpolate::EvalContext;
use crate::plan::schema::{self as plan, validate_json};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::Read;
use std::time::Duration;

const MAX_RECIPE_BYTES: usize = 512 * 1024;
const GITHUB_RAW_HOST: &str = "raw.githubusercontent.com";

fn is_commit_revision(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct InputSpec {
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub schema: Map<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub required: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub secret: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct CompatibilitySpec {
    #[serde(
        rename = "minHostAgentVersion",
        default,
        skip_serializing_if = "String::is_empty"
    )]
    pub min_host_agent_version: String,
    #[serde(
        rename = "requiredTools",
        default,
        skip_serializing_if = "Vec::is_empty"
    )]
    pub required_tools: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct SourceRequest {
    pub source: String,
    pub revision: String,
    pub sha256: String,
    pub require_sha256: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SourceMetadata {
    pub kind: String,
    pub source: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub revision: String,
    #[serde(rename = "rawSha256")]
    pub raw_sha256: String,
    #[serde(rename = "recipeHash")]
    pub recipe_hash: String,
}

pub fn canonical_hash<T: Serialize>(value: &T) -> Result<String, String> {
    let encoded = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    Ok(hash_bytes(&crate::gojson::html_escape_json_bytes(encoded)))
}

fn hash_bytes(value: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(value)))
}

/// Mirrors `recipe.decodeValue`: a leading `{` means JSON, otherwise YAML.
pub fn decode_value<T: for<'de> Deserialize<'de>>(raw: &[u8], kind: &str) -> Result<T, String> {
    let trimmed = String::from_utf8_lossy(raw);
    let trimmed = trimmed.trim_start();
    if trimmed.starts_with('{') {
        serde_json::from_slice(raw).map_err(|e| format!("decode {kind} JSON: {e}"))
    } else {
        serde_yaml::from_slice(raw).map_err(|e| format!("decode {kind} YAML: {e}"))
    }
}

pub fn resolve_inputs(
    specs: &BTreeMap<String, InputSpec>,
    values: &Map<String, Value>,
) -> Result<Map<String, Value>, String> {
    let mut resolved = Map::with_capacity(specs.len());
    for (name, value) in values {
        if !specs.contains_key(name) {
            return Err(format!("unknown recipe input \"{name}\""));
        }
        resolved.insert(name.clone(), value.clone());
    }
    for (name, spec) in specs {
        let mut present = resolved.contains_key(name);
        if !present {
            if let Some(default) = &spec.default {
                resolved.insert(name.clone(), default.clone());
                present = true;
            }
        }
        if !present {
            if spec.required {
                return Err(format!("required recipe input \"{name}\" is missing"));
            }
            continue;
        }
        if !spec.schema.is_empty() {
            let value = resolved.get(name).unwrap().clone();
            validate_json(&spec.schema, &value)
                .map_err(|e| format!("recipe input \"{name}\": {e}"))?;
        }
    }
    Ok(resolved)
}

/// Host-owned context values a recipe cannot override through its own
/// inputs or plan variables -- `tenantId` today.
pub fn reserved_plan_variables(existing: &Map<String, Value>) -> Map<String, Value> {
    let mut variables = existing.clone();
    let tenant_id = std::env::var("OPUTE_TENANT_ID").unwrap_or_default();
    let tenant_id = if tenant_id.trim().is_empty() {
        "local".to_string()
    } else {
        tenant_id.trim().to_string()
    };
    variables.insert("tenantId".to_string(), Value::String(tenant_id));
    variables
}

/// Resolves a plan's `idempotencyKey` through interpolation against the
/// expanded variables, exactly as `ResolveBaseInputs`/`ResolveHostInputs`
/// do in Go -- a recipe author writes `${vars.inputs.x}-${vars.tenantId}`
/// once and every run of that recipe id gets the same idempotency key.
pub fn resolve_plan_identity(
    expanded: &mut plan::Document,
    variables: &Map<String, Value>,
) -> Result<(), String> {
    let mut args = Map::new();
    args.insert(
        "idempotencyKey".to_string(),
        Value::String(expanded.idempotency_key.clone()),
    );
    let context = EvalContext {
        variables: variables.clone(),
        ..Default::default()
    };
    let identity = crate::plan::interpolate::interpolate_args(&args, &context)
        .map_err(|e| format!("resolve recipe plan identity: {e}"))?;
    if let Some(Value::String(key)) = identity.get("idempotencyKey") {
        expanded.idempotency_key = key.clone();
    }
    Ok(())
}

pub fn sorted_strings(mut values: Vec<String>) -> Vec<String> {
    values.sort();
    values
}

/// `ValidateHostAgentVersion`: compares the numeric major/minor/patch
/// portion of a recipe requirement with the running agent. A development
/// build's version string cannot honestly satisfy a release requirement,
/// so it fails closed rather than parsing a pre-release suffix loosely.
pub fn validate_host_agent_version(minimum: &str, current: &str) -> Result<(), String> {
    let minimum = minimum
        .trim()
        .strip_prefix('v')
        .unwrap_or(minimum.trim())
        .trim();
    if minimum.is_empty() {
        return Ok(());
    }
    let required = parse_version(minimum)
        .ok_or_else(|| format!("invalid minimum host-agent version \"{minimum}\""))?;
    let current_trimmed = current
        .trim()
        .strip_prefix('v')
        .unwrap_or(current.trim())
        .trim();
    let actual = parse_version(current_trimmed).ok_or_else(|| {
        format!("cannot verify minimum host-agent version \"{minimum}\" against \"{current}\"")
    })?;
    for i in 0..3 {
        if actual[i] > required[i] {
            return Ok(());
        }
        if actual[i] < required[i] {
            return Err(format!(
                "recipe requires host-agent version >= {minimum}, running {current}"
            ));
        }
    }
    Ok(())
}

fn parse_version(value: &str) -> Option<[i64; 3]> {
    let parts: Vec<&str> = value.splitn(4, '.').collect();
    if parts.len() < 3 {
        return None;
    }
    let mut result = [0i64; 3];
    for i in 0..3 {
        if parts[i].is_empty() || !parts[i].bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        result[i] = parts[i].parse().ok()?;
    }
    Some(result)
}

pub fn load_source(request: &SourceRequest) -> Result<(Vec<u8>, SourceMetadata), String> {
    let source = request.source.trim();
    if source.is_empty() {
        return Err("recipe source is required".to_string());
    }
    if let Some(reference) = source.strip_prefix("github:") {
        let parsed = parse_github_reference(reference, &request.revision)?;
        return fetch_remote(
            &parsed.url,
            &parsed.revision,
            &request.sha256,
            request.require_sha256,
        );
    }
    if source.starts_with("https://") {
        let parsed = parse_raw_github_url(source, &request.revision)?;
        return fetch_remote(
            &parsed.url,
            &parsed.revision,
            &request.sha256,
            request.require_sha256,
        );
    }
    read_local(source, &request.sha256)
}

struct GithubReference {
    url: String,
    revision: String,
}

fn validate_github_path(parts: &[&str]) -> Result<(), String> {
    for part in parts {
        if part.is_empty() || *part == "." || *part == ".." || part.contains('\\') {
            return Err("recipe source path contains an invalid traversal segment".to_string());
        }
    }
    Ok(())
}

fn parse_github_reference(value: &str, revision: &str) -> Result<GithubReference, String> {
    let value = value.trim();
    let Some((path, commit)) = value.split_once('@') else {
        return Err("GitHub recipe source requires @<40-character-commit-sha>".to_string());
    };
    if !is_commit_revision(commit) {
        return Err("GitHub recipe source requires @<40-character-commit-sha>".to_string());
    }
    let path_parts: Vec<&str> = path.trim_matches('/').split('/').collect();
    if path_parts.len() < 3 || path_parts[0].is_empty() || path_parts[1].is_empty() {
        return Err(
            "GitHub recipe source must be github:<owner>/<repo>/<path>@<commit>".to_string(),
        );
    }
    validate_github_path(&path_parts)?;
    if !revision.trim().is_empty() && revision != commit {
        return Err("GitHub source revision disagrees with source reference".to_string());
    }
    let url = format!(
        "https://{GITHUB_RAW_HOST}/{}/{}/{}/{}",
        path_parts[0],
        path_parts[1],
        commit,
        path_parts[2..].join("/")
    );
    Ok(GithubReference {
        url,
        revision: commit.to_ascii_lowercase(),
    })
}

fn parse_raw_github_url(raw: &str, revision: &str) -> Result<GithubReference, String> {
    let url = url::Url::parse(raw)
        .map_err(|_| "recipe URL must be an HTTPS raw.githubusercontent.com URL".to_string())?;
    if url.scheme() != "https" || url.host_str() != Some(GITHUB_RAW_HOST) {
        return Err("recipe URL must be an HTTPS raw.githubusercontent.com URL".to_string());
    }
    let parts: Vec<&str> = url.path().trim_matches('/').split('/').collect();
    if parts.len() < 4
        || parts[0].is_empty()
        || parts[1].is_empty()
        || !is_commit_revision(parts[2])
    {
        return Err("raw GitHub recipe URL must contain an immutable commit revision".to_string());
    }
    validate_github_path(&parts[..2])?;
    validate_github_path(&parts[3..])?;
    if !revision.trim().is_empty() && revision != parts[2] {
        return Err("GitHub source revision disagrees with URL".to_string());
    }
    let mut clean = url.clone();
    clean.set_query(None);
    clean.set_fragment(None);
    Ok(GithubReference {
        url: clean.to_string(),
        revision: parts[2].to_ascii_lowercase(),
    })
}

fn read_local(path: &str, expected: &str) -> Result<(Vec<u8>, SourceMetadata), String> {
    let metadata =
        std::fs::symlink_metadata(path).map_err(|e| format!("stat recipe source: {e}"))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err("recipe source must be a regular, non-symlink file".to_string());
    }
    let file = std::fs::File::open(path).map_err(|e| format!("open recipe source: {e}"))?;
    let mut raw = Vec::new();
    file.take(MAX_RECIPE_BYTES as u64 + 1)
        .read_to_end(&mut raw)
        .map_err(|e| format!("read recipe source: {e}"))?;
    let raw = read_bounded(raw)?;
    let raw_hash = hash_bytes(&raw);
    verify_expected_hash(expected, &raw_hash)?;
    let clean_path = std::path::Path::new(path).to_string_lossy().to_string();
    Ok((
        raw,
        SourceMetadata {
            kind: "path".to_string(),
            source: clean_path,
            raw_sha256: raw_hash,
            ..Default::default()
        },
    ))
}

fn fetch_remote(
    raw_url: &str,
    revision: &str,
    expected: &str,
    require_hash: bool,
) -> Result<(Vec<u8>, SourceMetadata), String> {
    if require_hash && expected.trim().is_empty() {
        return Err("remote recipe mutation requires an expected sha256".to_string());
    }
    if !is_commit_revision(revision) {
        return Err(
            "remote recipe source requires an immutable 40-character commit revision".to_string(),
        );
    }
    let raw = fetch_with_validated_redirects(raw_url)?;
    let raw = read_bounded(raw)?;
    let raw_hash = hash_bytes(&raw);
    verify_expected_hash(expected, &raw_hash)?;
    Ok((
        raw,
        SourceMetadata {
            kind: "github".to_string(),
            source: raw_url.to_string(),
            revision: revision.to_ascii_lowercase(),
            raw_sha256: raw_hash,
            ..Default::default()
        },
    ))
}

/// Follows redirects by hand (ureq's redirect handling has no per-hop
/// callback) so every hop can be checked against the same host allowlist
/// Go's `http.Client.CheckRedirect` enforces -- a redirect that would leave
/// `raw.githubusercontent.com` is refused rather than followed.
fn fetch_with_validated_redirects(start_url: &str) -> Result<Vec<u8>, String> {
    let agent = ureq::AgentBuilder::new()
        .redirects(0)
        .timeout(Duration::from_secs(30))
        .build();
    let mut url = start_url.to_string();
    for _ in 0..10 {
        let response = match agent.get(&url).call() {
            Ok(response) => response,
            Err(ureq::Error::Status(status, _)) => {
                return Err(format!("fetch recipe source: HTTP {status}"))
            }
            Err(ureq::Error::Transport(e)) => return Err(format!("fetch recipe source: {e}")),
        };
        let status = response.status();
        if (300..400).contains(&status) {
            let Some(location) = response.header("Location").map(str::to_string) else {
                return Err(format!("fetch recipe source: HTTP {status}"));
            };
            let next = url::Url::parse(&url)
                .and_then(|base| base.join(&location))
                .map_err(|e| e.to_string())?;
            if next.scheme() != "https" || next.host_str() != Some(GITHUB_RAW_HOST) {
                return Err("recipe redirect leaves raw.githubusercontent.com".to_string());
            }
            url = next.to_string();
            continue;
        }
        let mut body = Vec::new();
        response
            .into_reader()
            .take(MAX_RECIPE_BYTES as u64 + 1)
            .read_to_end(&mut body)
            .map_err(|e| format!("read recipe source: {e}"))?;
        return Ok(body);
    }
    Err("fetch recipe source: too many redirects".to_string())
}

fn read_bounded(raw: Vec<u8>) -> Result<Vec<u8>, String> {
    if raw.len() > MAX_RECIPE_BYTES {
        return Err(format!("recipe source exceeds {MAX_RECIPE_BYTES} bytes"));
    }
    if String::from_utf8_lossy(&raw).trim().is_empty() {
        return Err("recipe source is empty".to_string());
    }
    Ok(raw)
}

fn verify_expected_hash(expected: &str, actual: &str) -> Result<(), String> {
    let expected = expected.trim().trim_start_matches("sha256:");
    if expected.is_empty() {
        return Ok(());
    }
    if expected.len() != 64 || !expected.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("recipe sha256 must be a 64-character hexadecimal digest".to_string());
    }
    let expected_full = format!("sha256:{}", expected.to_ascii_lowercase());
    if !expected_full.eq_ignore_ascii_case(actual) {
        return Err(format!(
            "recipe sha256 mismatch: expected {expected_full}, got {actual}"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_agent_version_check_matches_go() {
        assert!(validate_host_agent_version("", "1.2.3").is_ok());
        assert!(validate_host_agent_version("v1.2.3", "1.2.3").is_ok());
        assert!(validate_host_agent_version("1.2.3", "1.3.0").is_ok());
        assert!(validate_host_agent_version("1.2.3", "1.2.2").is_err());
        assert!(validate_host_agent_version("1.2.x", "1.2.3").is_err());
        assert!(validate_host_agent_version("1.2.3", "dev").is_err());
    }

    #[test]
    fn github_reference_requires_a_40_char_commit() {
        assert!(parse_github_reference("owner/repo/path.yaml@short", "").is_err());
        let parsed = parse_github_reference(
            "owner/repo/path.yaml@0123456789abcdef0123456789abcdef01234567",
            "",
        )
        .unwrap();
        assert_eq!(parsed.url, "https://raw.githubusercontent.com/owner/repo/0123456789abcdef0123456789abcdef01234567/path.yaml");
    }

    #[test]
    fn github_path_rejects_traversal() {
        assert!(parse_github_reference(
            "owner/../repo@0123456789abcdef0123456789abcdef01234567",
            ""
        )
        .is_err());
    }

    #[test]
    fn local_source_rejects_missing_file() {
        assert!(read_local("/nonexistent/path/to/recipe.yaml", "").is_err());
    }

    #[test]
    fn local_source_reads_and_hashes_a_real_file() {
        let dir =
            std::env::temp_dir().join(format!("host-agent-recipe-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("recipe.json");
        std::fs::write(&path, b"{\"x\":1}").unwrap();
        let (raw, metadata) = read_local(path.to_str().unwrap(), "").unwrap();
        assert_eq!(raw, b"{\"x\":1}");
        assert_eq!(metadata.kind, "path");
        assert!(metadata.raw_sha256.starts_with("sha256:"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn expected_hash_mismatch_is_rejected() {
        assert!(verify_expected_hash("sha256:deadbeef", &hash_bytes(b"x")).is_err());
        assert!(verify_expected_hash("", &hash_bytes(b"x")).is_ok());
        let actual = hash_bytes(b"x");
        assert!(verify_expected_hash(actual.trim_start_matches("sha256:"), &actual).is_ok());
    }

    #[test]
    fn resolve_inputs_rejects_unknown_names() {
        let specs = BTreeMap::new();
        let mut values = Map::new();
        values.insert("ghost".to_string(), Value::Bool(true));
        assert!(resolve_inputs(&specs, &values).is_err());
    }

    #[test]
    fn resolve_inputs_applies_defaults_and_enforces_required() {
        let mut specs = BTreeMap::new();
        specs.insert(
            "a".to_string(),
            InputSpec {
                required: true,
                ..Default::default()
            },
        );
        assert!(resolve_inputs(&specs, &Map::new()).is_err());
    }
}
