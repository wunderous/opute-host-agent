//! `opute-provider-hostos`, the host-platform provider (Go
//! `plugins/platform/hostos`). A read-only generation: it observes the
//! operating system and CPU of the machine it runs on, never mutates the
//! host, and so declares no recipes and no teardown plan.
//!
//! Service Definition: `opute.capability.host-platform` with `detect` and
//! `validate`, as the install manifest declares them. Service Provider: the
//! two handlers below over `hostobs::detect_platform`. Consumer: the Host
//! Agent's provider binding.

use super::{go_text, structured, Server, Tool};
use crate::goerr::quote;
use crate::gojson::{equal_fold, kind_name, Node, Value};
use crate::mcpsdk::ToolCallOutcome;
use crate::provider::definition::{
    CapabilityRef, GoSlice, InstallManifest, Operation, ServiceDefinition, ValidationRef,
};
use crate::provider::ProviderRef;
use serde_json::{json, Map, Value as J};

const PROVIDER_ID: &str = "com.opute.hostos";
const PROVIDER_VERSION: &str = "1.0.0";
const CAPABILITY_ID: &str = "opute.capability.host-platform.v1";
const SERVICE_ID: &str = "opute.capability.host-platform";
const DETECT: &str = "opute.capability.host-platform.detect";
const VALIDATE: &str = "opute.capability.host-platform.validate";
const CONTRACT_VERSION: &str = "host-platform.v1";
const KINDS: [&str; 5] = ["linux", "macos", "windows-native", "wsl1", "wsl2"];
const FAMILIES: [&str; 6] = ["x86-64", "x86", "arm64", "arm", "apple-silicon", "unknown"];

pub fn main() -> ! {
    super::run(server(), "4321", |port| {
        format!("Opute host platform provider listening on :{port}/mcp")
    })
}

pub fn server() -> Server {
    let manifest = install_manifest();
    let manifest_text = go_text(serde_json::to_string(&manifest).expect("manifest serializes"));
    let manifest_value = serde_json::to_value(&manifest).expect("manifest serializes");
    Server {
        name: "opute-provider-hostos",
        version: PROVIDER_VERSION,
        capabilities: json!({"tools": {"listChanged": true}}),
        tools: vec![
            Tool {
                name: "opute.provider.get_install_manifest",
                description: "Read the host platform provider installation manifest",
                input_schema: json!({"type": "object"}),
                output_schema: Some(json!({"type": "object"})),
                handler: Box::new(move |_| structured(manifest_text.clone(), manifest_value.clone())),
            },
            Tool {
                name: DETECT,
                description: "Detect the operating system and CPU identity of the host running the agent, distinguishing native Windows, macOS, WSL1/WSL2, and native Linux.",
                input_schema: json!({"type": "object", "properties": {}}),
                output_schema: Some(platform_schema()),
                handler: Box::new(|_| {
                    let platform = detect();
                    structured(go_text(platform_text(&platform)), platform)
                }),
            },
            Tool {
                name: VALIDATE,
                description: "Validate a caller-declared expected host kind or CPU family against the observed host.",
                input_schema: validate_input_schema(),
                output_schema: Some(json!({"type": "object"})),
                handler: Box::new(|arguments| match validate_args(arguments) {
                    // A handler error: go-sdk answers with a JSON-RPC error
                    // whose code is 0.
                    Err(e) => ToolCallOutcome::Protocol(0, e, None),
                    Ok(args) => {
                        let result = validate_host(&args, detect());
                        structured(go_text(validate_text(&result)), result)
                    }
                }),
            },
        ],
    }
}

fn detect() -> J {
    crate::hostobs::detect_platform(&crate::config::Env::from_process())
}

fn object(value: J) -> Option<Map<String, J>> {
    match value {
        J::Object(map) => Some(map),
        _ => None,
    }
}

fn install_manifest() -> InstallManifest {
    let operation = |id: &str, description: &str, input: J, output: J| Operation {
        id: id.into(),
        version: 1,
        description: description.into(),
        input_schema: object(input),
        output_schema: object(output),
        effect: "read".into(),
        idempotent: true,
        supports_readiness: true,
        task_support: "sync_only".into(),
        ..Operation::default()
    };
    InstallManifest {
        schema: "opute-provider-install-manifest.v1".into(),
        provider: ProviderRef {
            id: PROVIDER_ID.into(),
            version: PROVIDER_VERSION.into(),
        },
        provides: GoSlice(Some(vec![CapabilityRef {
            id: CAPABILITY_ID.into(),
            version: 1,
        }])),
        services: GoSlice(Some(vec![ServiceDefinition {
            id: SERVICE_ID.into(),
            capability_id: CAPABILITY_ID.into(),
            version: 1,
            operations: GoSlice(Some(vec![
                operation(
                    DETECT,
                    "Detect the operating system and CPU identity of the host running the agent.",
                    json!({"type": "object", "properties": {}}),
                    platform_schema(),
                ),
                operation(
                    VALIDATE,
                    "Validate a caller-declared expected host kind or CPU family against the observed host.",
                    validate_input_schema(),
                    json!({"type": "object", "required": ["contractVersion", "ready", "platform"]}),
                ),
            ])),
            ..ServiceDefinition::default()
        }])),
        validation: ValidationRef {
            capability: CAPABILITY_ID.into(),
            operation: VALIDATE.into(),
        },
        ..InstallManifest::default()
    }
}

fn platform_schema() -> J {
    json!({"type": "object", "required": ["contractVersion", "os", "kind", "cpu"], "properties": {
        "contractVersion": {"type": "string", "const": CONTRACT_VERSION},
        "os": {"type": "string", "enum": ["linux", "macos", "windows"]},
        "kind": {"type": "string", "enum": KINDS},
        "cpu": {"type": "object", "required": ["architecture", "family"], "properties": {
            "architecture": {"type": "string"},
            "family": {"type": "string", "enum": FAMILIES},
            "vendor": {"type": "string"},
            "model": {"type": "string"},
            "series": {"type": "string"},
            "variant": {"type": "string"},
            "logicalCores": {"type": "integer"},
        }},
    }})
}

fn validate_input_schema() -> J {
    json!({"type": "object", "properties": {
        "expectedKind": {"type": "string", "enum": KINDS},
        "expectedCPUFamily": {"type": "string", "enum": FAMILIES},
    }})
}

#[derive(Default)]
struct ValidateArgs {
    expected_kind: String,
    expected_cpu_family: String,
}

/// `json.Unmarshal(arguments, &validateArgs)` when arguments are present:
/// keys match field names without case, null leaves a field unset, and a
/// value of another kind is Go's `UnmarshalTypeError`.
fn validate_args(arguments: Option<&Node>) -> Result<ValidateArgs, String> {
    let mut args = ValidateArgs::default();
    let Some(node) = arguments else {
        return Ok(args);
    };
    let entries = match &node.value {
        Value::Null => return Ok(args),
        Value::Object(entries) => entries,
        other => {
            return Err(format!(
                "json: cannot unmarshal {} into Go value of type main.validateArgs",
                kind_name(other)
            ))
        }
    };
    let mut first_error = None;
    for (key, item) in entries {
        let Some(field) = ["expectedKind", "expectedCPUFamily"]
            .into_iter()
            .find(|f| equal_fold(f, key))
        else {
            continue;
        };
        match &item.value {
            Value::Null => {}
            Value::String(s) if field == "expectedKind" => args.expected_kind = s.clone(),
            Value::String(s) => args.expected_cpu_family = s.clone(),
            other => {
                first_error.get_or_insert_with(|| {
                    format!(
                        "json: cannot unmarshal {} into Go struct field validateArgs.{field} of type string",
                        kind_name(other)
                    )
                });
            }
        }
    }
    first_error.map_or(Ok(args), Err)
}

/// `validateHost` fails closed: an expectation that does not match the
/// observed host is reported as not ready with the exact mismatch, never
/// coerced into a nearby host kind.
fn validate_host(args: &ValidateArgs, platform: J) -> J {
    let observed = |pointer: &str| {
        platform
            .pointer(pointer)
            .and_then(J::as_str)
            .unwrap_or("")
            .to_string()
    };
    let mut mismatches = Vec::new();
    let kind = observed("/kind");
    if !args.expected_kind.is_empty() && args.expected_kind != kind {
        mismatches.push(format!(
            "expected host kind {}, observed {}",
            quote(&args.expected_kind),
            quote(&kind)
        ));
    }
    let family = observed("/cpu/family");
    if !args.expected_cpu_family.is_empty() && args.expected_cpu_family != family {
        mismatches.push(format!(
            "expected CPU family {}, observed {}",
            quote(&args.expected_cpu_family),
            quote(&family)
        ));
    }
    json!({
        "contractVersion": CONTRACT_VERSION,
        "capability": CAPABILITY_ID,
        "ready": mismatches.is_empty(),
        "mismatches": mismatches,
        "platform": platform,
    })
}

/// A JSON value with its object keys in the given order (Go struct field
/// order); keys not listed follow, sorted.
fn ordered(value: &J, order: &[&str]) -> String {
    match value {
        J::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_by_key(|k| {
                (
                    order.iter().position(|o| o == k).unwrap_or(usize::MAX),
                    k.as_str(),
                )
            });
            let fields: Vec<String> = keys
                .into_iter()
                .map(|k| format!("{}:{}", J::String(k.clone()), ordered(&map[k], order)))
                .collect();
            format!("{{{}}}", fields.join(","))
        }
        J::Array(items) => format!(
            "[{}]",
            items
                .iter()
                .map(|i| ordered(i, order))
                .collect::<Vec<_>>()
                .join(",")
        ),
        other => other.to_string(),
    }
}

/// `hostplatform.Platform` (with `CPU` and `WSL`) in struct field order.
const PLATFORM_ORDER: [&str; 20] = [
    "contractVersion",
    "os",
    "kind",
    "kernel",
    "kernelVersion",
    "distribution",
    "distributionVersion",
    "wsl",
    "cpu",
    "evidence",
    "architecture",
    "family",
    "vendor",
    "model",
    "series",
    "variant",
    "logicalCores",
    "version",
    "distro",
    "interop",
];

fn platform_text(platform: &J) -> String {
    ordered(platform, &PLATFORM_ORDER)
}

/// The validation result is a Go map (sorted keys) holding the platform
/// struct.
fn validate_text(result: &J) -> String {
    let J::Object(map) = result else {
        return result.to_string();
    };
    let fields: Vec<String> = map
        .iter()
        .map(|(k, v)| {
            let value = if k == "platform" {
                platform_text(v)
            } else {
                v.to_string()
            };
            format!("{}:{value}", J::String(k.clone()))
        })
        .collect();
    format!("{{{}}}", fields.join(","))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wsl2() -> J {
        json!({"contractVersion": CONTRACT_VERSION, "os": "linux", "kind": "wsl2",
               "kernel": "6.18.33.2-microsoft-standard-WSL2",
               "wsl": {"version": 2, "interop": false}, "cpu": {"architecture": "amd64", "family": "x86-64"}})
    }

    fn args(kind: &str, family: &str) -> ValidateArgs {
        ValidateArgs {
            expected_kind: kind.into(),
            expected_cpu_family: family.into(),
        }
    }

    // Go TestInstallManifestIsValid.
    #[test]
    fn install_manifest_is_valid() {
        let expected = ProviderRef {
            id: PROVIDER_ID.into(),
            version: PROVIDER_VERSION.into(),
        };
        crate::provider::definition::validate_install_manifest(&install_manifest(), &expected)
            .unwrap();
    }

    // Go TestValidateHostFailsClosedOnMismatch.
    #[test]
    fn validate_host_fails_closed_on_mismatch() {
        assert_eq!(validate_host(&args("wsl2", ""), wsl2())["ready"], true);
        let linux = validate_host(&args("linux", ""), wsl2());
        assert_eq!(linux["ready"], false);
        assert_eq!(
            linux["mismatches"],
            json!(["expected host kind \"linux\", observed \"wsl2\""])
        );
        assert_eq!(
            validate_host(&args("", "apple-silicon"), wsl2())["ready"],
            false
        );
    }

    // Go TestNewServerBuilds.
    #[test]
    fn server_builds() {
        assert_eq!(server().tools.len(), 3);
    }

    #[test]
    fn texts_keep_go_field_order() {
        assert_eq!(
            platform_text(&wsl2()),
            r#"{"contractVersion":"host-platform.v1","os":"linux","kind":"wsl2","kernel":"6.18.33.2-microsoft-standard-WSL2","wsl":{"version":2,"interop":false},"cpu":{"architecture":"amd64","family":"x86-64"}}"#
        );
        let text = validate_text(&validate_host(&args("", ""), wsl2()));
        assert!(text.starts_with(r#"{"capability":"opute.capability.host-platform.v1","contractVersion":"host-platform.v1","mismatches":[],"platform":{"contractVersion""#));
    }

    #[test]
    fn arguments_decode_like_encoding_json() {
        let parse = |s: &str| crate::gojson::parse(s.as_bytes()).unwrap();
        assert_eq!(
            validate_args(Some(&parse(r#"{"EXPECTEDKIND":"wsl1"}"#)))
                .unwrap()
                .expected_kind,
            "wsl1"
        );
        assert_eq!(
            validate_args(Some(&parse("[]"))).err().unwrap(),
            "json: cannot unmarshal array into Go value of type main.validateArgs"
        );
        assert_eq!(
            validate_args(Some(&parse(r#"{"expectedKind":5}"#))).err().unwrap(),
            "json: cannot unmarshal number into Go struct field validateArgs.expectedKind of type string"
        );
    }
}
