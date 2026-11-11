//! Property tests (m16): invariants the contract spec states for every input,
//! checked on generated schemas, values and identifiers rather than on fixed
//! examples.
//!
//! - Redaction (D13): delivery hides exactly the `writeOnly` values; storage
//!   hides those and every value no schema entry names; storage is never
//!   less strict than delivery; both are idempotent.
//! - Argument validation: a value built to satisfy a schema is accepted, and
//!   dropping a required field is refused.
//! - Canonical resource identity: a URI the agent accepts prints to a string
//!   that parses back to the same identity.

use proptest::prelude::*;
use serde_json::{json, Map, Value as J};

use crate::evidence::{redact_for_delivery, redact_for_storage};

const REDACTED: &str = "[redacted]";

fn key() -> impl Strategy<Value = String> {
    "[a-c]{1,2}"
}

fn scalar() -> impl Strategy<Value = J> {
    prop_oneof![
        Just(J::Null),
        any::<bool>().prop_map(J::from),
        any::<i32>().prop_map(J::from),
        "[a-z ]{0,6}".prop_map(J::from),
    ]
}

/// Any JSON value, including shapes no schema describes.
fn value() -> impl Strategy<Value = J> {
    scalar().prop_recursive(4, 24, 4, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..4).prop_map(J::Array),
            prop::collection::btree_map(key(), inner, 0..4)
                .prop_map(|m| J::Object(m.into_iter().collect())),
        ]
    })
}

/// A schema of the shapes the catalog uses: typed leaves (some `writeOnly`),
/// open `{}`, objects with properties, `required` and every form of
/// `additionalProperties`, and arrays with `items`.
fn schema() -> impl Strategy<Value = Map<String, J>> {
    let leaf = prop_oneof![
        Just(json!({"type": "string"})),
        Just(json!({"type": "string", "writeOnly": true})),
        Just(json!({"type": "integer"})),
        Just(json!({"type": "boolean"})),
        Just(json!({})),
    ];
    leaf.prop_recursive(3, 16, 3, |inner| {
        prop_oneof![
            (
                prop::collection::btree_map(key(), inner.clone(), 0..3),
                prop::sample::subsequence(vec!["a", "b", "c", "aa"], 0..3),
                prop_oneof![
                    Just(None),
                    Just(Some(J::Bool(true))),
                    Just(Some(J::Bool(false))),
                    inner.clone().prop_map(Some),
                ],
            )
                .prop_map(|(props, required, additional)| {
                    let required: Vec<J> = required
                        .into_iter()
                        .filter(|r| props.contains_key(*r))
                        .map(J::from)
                        .collect();
                    let mut s = json!({"type": "object", "properties": props});
                    if !required.is_empty() {
                        s["required"] = J::Array(required);
                    }
                    if let Some(a) = additional {
                        s["additionalProperties"] = a;
                    }
                    s
                }),
            inner.prop_map(|items| json!({"type": "array", "items": items})),
        ]
    })
    .prop_map(|s| s.as_object().cloned().unwrap_or_default())
}

/// The schema a child value is projected through, as the spec states it:
/// its `properties` entry, else a typed `additionalProperties`.
fn child<'a>(schema: &'a Map<String, J>, key: &str) -> Option<&'a Map<String, J>> {
    schema
        .get("properties")
        .and_then(|p| p.get(key))
        .and_then(J::as_object)
        .or_else(|| schema.get("additionalProperties").and_then(J::as_object))
}

/// Check one projection against the spec, walking value, schema and output
/// together. `storage` selects D13's fail-closed rule for unnamed values.
fn check(input: &J, schema: Option<&Map<String, J>>, out: &J, storage: bool) -> Result<(), String> {
    let hidden = match schema {
        None => storage,
        Some(s) => s.get("writeOnly") == Some(&J::Bool(true)),
    };
    if hidden {
        return if out == &J::from(REDACTED) {
            Ok(())
        } else {
            Err(format!("{input} was not redacted: {out}"))
        };
    }
    match (input, out, schema) {
        (J::Object(i), J::Object(o), Some(s)) => {
            if i.len() != o.len() {
                return Err(format!("keys changed: {input} -> {out}"));
            }
            for (k, v) in i {
                let ov = o.get(k).ok_or_else(|| format!("key {k} dropped"))?;
                check(v, child(s, k), ov, storage)?;
            }
            Ok(())
        }
        (J::Array(i), J::Array(o), Some(s)) => {
            if i.len() != o.len() {
                return Err("array length changed".into());
            }
            let items = s.get("items").and_then(J::as_object);
            i.iter()
                .zip(o)
                .try_for_each(|(a, b)| check(a, items, b, storage))
        }
        _ if input == out => Ok(()),
        _ => Err(format!("{input} changed to {out}")),
    }
}

/// Storage never shows a value delivery hides: every place they differ,
/// storage holds the redaction marker.
fn at_least_as_strict(storage: &J, delivery: &J) -> bool {
    if storage == &J::from(REDACTED) {
        return true;
    }
    match (storage, delivery) {
        (J::Object(s), J::Object(d)) => {
            s.len() == d.len()
                && s.iter()
                    .all(|(k, v)| d.get(k).is_some_and(|dv| at_least_as_strict(v, dv)))
        }
        (J::Array(s), J::Array(d)) => {
            s.len() == d.len() && s.iter().zip(d).all(|(a, b)| at_least_as_strict(a, b))
        }
        _ => storage == delivery,
    }
}

/// A value that satisfies `schema`, built from it.
fn conforming(schema: Map<String, J>) -> BoxedStrategy<J> {
    let kind = schema
        .get("type")
        .and_then(J::as_str)
        .unwrap_or("")
        .to_string();
    match kind.as_str() {
        "string" => "[a-z]{0,5}".prop_map(J::from).boxed(),
        "integer" => any::<i32>().prop_map(J::from).boxed(),
        "boolean" => any::<bool>().prop_map(J::from).boxed(),
        "array" => {
            let items = schema
                .get("items")
                .and_then(J::as_object)
                .cloned()
                .unwrap_or_default();
            prop::collection::vec(conforming(items), 0..3)
                .prop_map(J::Array)
                .boxed()
        }
        "object" => {
            let props: Vec<(String, Map<String, J>)> = schema
                .get("properties")
                .and_then(J::as_object)
                .map(|p| {
                    p.iter()
                        .map(|(k, v)| (k.clone(), v.as_object().cloned().unwrap_or_default()))
                        .collect()
                })
                .unwrap_or_default();
            let required: Vec<String> = schema
                .get("required")
                .and_then(J::as_array)
                .map(|r| {
                    r.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            let fields: Vec<BoxedStrategy<Option<(String, J)>>> = props
                .into_iter()
                .map(|(k, s)| {
                    let present = conforming(s).prop_map({
                        let k = k.clone();
                        move |v| Some((k.clone(), v))
                    });
                    if required.contains(&k) {
                        present.boxed()
                    } else {
                        prop_oneof![Just(None), present].boxed()
                    }
                })
                .collect();
            fields
                .prop_map(|fs| J::Object(fs.into_iter().flatten().collect()))
                .boxed()
        }
        _ => value().boxed(),
    }
}

fn schema_and_conforming() -> impl Strategy<Value = (Map<String, J>, J)> {
    schema().prop_flat_map(|s| (Just(s.clone()), conforming(s)))
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]

    #[test]
    fn delivery_hides_exactly_write_only((s, v) in schema_and_conforming(), noise in value()) {
        for input in [&v, &noise] {
            let out = redact_for_delivery(input, Some(&s));
            prop_assert_eq!(check(input, Some(&s), &out, false), Ok(()));
        }
    }

    #[test]
    fn storage_hides_write_only_and_unnamed((s, v) in schema_and_conforming(), noise in value()) {
        for input in [&v, &noise] {
            let out = redact_for_storage(input, Some(&s));
            prop_assert_eq!(check(input, Some(&s), &out, true), Ok(()));
        }
        prop_assert_eq!(redact_for_storage(&noise, None), J::from(REDACTED));
    }

    #[test]
    fn storage_is_never_less_strict_than_delivery(s in schema(), v in value()) {
        let storage = redact_for_storage(&v, Some(&s));
        let delivery = redact_for_delivery(&v, Some(&s));
        prop_assert!(at_least_as_strict(&storage, &delivery), "{} vs {}", storage, delivery);
    }

    #[test]
    fn redaction_is_idempotent(s in schema(), v in value()) {
        let once = redact_for_delivery(&v, Some(&s));
        prop_assert_eq!(redact_for_delivery(&once, Some(&s)), once);
        let once = redact_for_storage(&v, Some(&s));
        prop_assert_eq!(redact_for_storage(&once, Some(&s)), once);
    }

    #[test]
    fn conforming_arguments_are_accepted((s, v) in schema_and_conforming()) {
        prop_assert_eq!(crate::schema::validate(&s, &v), Ok(()), "schema {}", J::Object(s.clone()));
    }

    #[test]
    fn a_missing_required_field_is_refused((s, v) in schema_and_conforming()) {
        let required: Vec<String> = s.get("required").and_then(J::as_array)
            .map(|r| r.iter().filter_map(|x| x.as_str().map(String::from)).collect())
            .unwrap_or_default();
        if let (Some(field), J::Object(mut object)) = (required.first(), v) {
            object.remove(field);
            prop_assert!(crate::schema::validate(&s, &J::Object(object)).is_err());
        }
    }

    #[test]
    fn validation_is_total(s in schema(), v in value()) {
        let _ = crate::schema::validate(&s, &v);
        let _ = crate::schema::validate_output(&s, &v);
    }

    #[test]
    fn accepted_resource_uris_round_trip(
        kind in prop::sample::select(crate::resource::KNOWN_TYPES.to_vec()),
        tenant in "[a-z][a-z0-9-]{0,8}",
        id in "\\PC{1,12}",
    ) {
        if let Ok(uri) = crate::resource::Uri::new(kind, &tenant, &id) {
            let printed = uri.to_string();
            let parsed = crate::resource::Uri::parse(&printed);
            prop_assert!(parsed.is_ok(), "{:?} does not parse: {:?}", printed, parsed.err());
            prop_assert_eq!(parsed.unwrap().to_string(), printed);
        }
    }

    #[test]
    fn parsed_resource_uris_are_canonical(s in "\\PC{0,24}") {
        if let Ok(uri) = crate::resource::Uri::parse(&s) {
            let printed = uri.to_string();
            prop_assert_eq!(crate::resource::Uri::parse(&printed).map(|u| u.to_string()), Ok(printed));
        }
    }
}

/// Finding F1 (m16), fixed by D30: `Uri::new` accepted an id with Unicode
/// whitespace other than space, tab, CR and LF (U+000B, U+000C, U+0085,
/// U+2003, ...), and `Uri::parse` of its printed form trimmed it, so the
/// identity changed. Such an id is now refused by both.
#[test]
fn resource_ids_with_unicode_whitespace_are_refused() {
    for id in [
        "a\u{b}",
        "a\u{c}",
        "a\u{85}",
        "a\u{2003}",
        "\u{3000}a",
        "a\u{2003}b",
    ] {
        assert!(
            crate::resource::Uri::new("vm", "t", id).is_err(),
            "new accepted {id:?}"
        );
        assert!(
            crate::resource::Uri::parse(&format!("vm:t:{id}x")).is_err(),
            "parse accepted {id:?}"
        );
    }
    assert_eq!(
        crate::resource::Uri::parse(" vm:t:a\n")
            .unwrap()
            .to_string(),
        "vm:t:a"
    );
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]

    /// A connector diagnostic line never carries the connector's secret,
    /// wherever it falls in the command output.
    #[test]
    fn diagnostics_never_show_the_secret(
        secret in "[A-Za-z0-9_-]{8,40}",
        before in "[a-z :]{0,30}",
        after in "[a-z :]{0,30}",
        word in prop::sample::select(vec!["status", "error", "failed", "registration", "inet"]),
        lines in prop::collection::vec("[a-z ]{0,20}", 0..4),
    ) {
        let mut output = lines.join("\n");
        output.push_str(&format!("\n{before}{word} {secret}{after}\n"));
        let shown = crate::providers::cloudflare::mesh::command_diagnostics(&output, &secret);
        prop_assert!(!shown.contains(&secret), "{:?}", shown);
    }
}
