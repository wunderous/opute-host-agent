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

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]

    /// D34: no literal environment value, wherever it sits in a container,
    /// reaches the workload projection.
    #[test]
    fn workload_projection_never_shows_a_value(
        values in prop::collection::vec("zv[A-Za-z0-9]{6,20}", 1..4),
        names in prop::collection::vec("[A-Z_]{1,8}", 1..4),
        containers in 1usize..3,
    ) {
        let env: Vec<J> = names
            .iter()
            .zip(values.iter().cycle())
            .map(|(name, value)| json!({"name": name, "value": value}))
            .collect();
        let container = |i: usize| json!({"name": format!("c{i}"), "image": "r/i:1", "env": env});
        let deployment = json!({
            "metadata": {"name": "w", "namespace": "n"},
            "spec": {"replicas": 1, "template": {"spec": {
                "containers": (0..containers).map(container).collect::<Vec<_>>(),
            }}},
        });
        let shown = crate::clusterops::project_deployment(&deployment).to_string();
        for value in &values {
            prop_assert!(!shown.contains(value.as_str()), "{} in {}", value, shown);
        }
    }

    /// D39: while less than the bound has arrived, the reads of a console,
    /// however its output is cut, return exactly that output.
    #[test]
    fn console_reads_lose_nothing_within_the_bound(
        text in "\\PC{0,400}",
        cuts in prop::collection::vec(0usize..64, 0..12),
        reads in prop::collection::vec(any::<bool>(), 0..12),
    ) {
        use crate::console::{append, drain, Output};
        let bytes = text.as_bytes();
        let mut output = Output::default();
        let mut seen = String::new();
        let mut at = 0;
        for (i, cut) in cuts.iter().enumerate() {
            let end = (at + cut).min(bytes.len());
            append(&mut output.pending, &bytes[at..end]);
            at = end;
            if reads.get(i).copied().unwrap_or(false) {
                seen.push_str(&drain(&mut output).0);
            }
        }
        append(&mut output.pending, &bytes[at..]);
        output.exited = true;
        seen.push_str(&drain(&mut output).0);
        prop_assert_eq!(seen, text);
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, ..ProptestConfig::default() })]

    /// D40: whatever the agent's environment holds, a child gets only
    /// allowlisted names, never one with a secret-like part, and every
    /// allowlisted name that is not secret-like.
    #[test]
    fn child_environment_never_carries_a_secret(
        names in prop::collection::vec(
            (prop::sample::select(vec!["", "LC_", "XDG_", "INCUS_", "OLLAMA_", "OPUTE_", "CLOUDFLARE_", "MCP_"]),
             prop::collection::vec(
                 prop::sample::select(vec!["PATH", "HOME", "TOKEN", "API", "KEY", "SECRET", "AUTH", "CONFIG",
                                           "RUNTIME", "DIR", "PASSWORD", "HTTPS", "PROXY", "MODELS", "ALL"]),
                 1..4)),
            0..24),
        values in prop::collection::vec("[a-z0-9]{0,12}", 24),
    ) {
        let pairs: Vec<(String, String)> = names
            .iter()
            .zip(values.iter())
            .map(|((prefix, parts), value)| (format!("{prefix}{}", parts.join("_")), value.clone()))
            .collect();
        let env = crate::config::Env::from_pairs(pairs.clone());
        let child = env.for_child();
        let secret = |name: &str| {
            name.split('_').any(|p| {
                ["TOKEN", "SECRET", "PASSWORD", "PASSWD", "CREDENTIAL", "CREDENTIALS", "AUTH", "KEY"].contains(&p)
            })
        };
        for (name, _) in child.pairs() {
            prop_assert!(!secret(name), "{} reached a child", name);
            prop_assert!(env.get(name) == child.get(name));
        }
        for (name, value) in &pairs {
            let listed = ["PATH", "HOME", "HTTPS_PROXY", "ALL_PROXY"].contains(&name.as_str())
                || ["LC_", "XDG_", "INCUS_", "OLLAMA_"].iter().any(|p| name.starts_with(p));
            if listed && !secret(name) && env.get(name) == *value {
                prop_assert_eq!(child.get(name), value.clone());
            }
            if name.starts_with("OPUTE_") || name.starts_with("CLOUDFLARE_") || name.starts_with("MCP_") {
                prop_assert!(child.get(name).is_empty(), "{} reached a child", name);
            }
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

    /// D29 and D41: a bounded read returns a body within its limit whole and
    /// refuses a larger one, whether it is sent with a length or chunked.
    #[test]
    fn bounded_reads_return_the_body_or_refuse_it(
        limit in 1u64..4096,
        len in 0usize..8192,
        chunked in any::<bool>(),
    ) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread().enable_io().build().unwrap();
            rt.block_on(async move {
                let listener = tokio::net::TcpListener::from_std(listener).unwrap();
                let (stream, _) = listener.accept().await.unwrap();
                crate::http1::serve_conn(stream, move |_req, mut body| async move {
                    let reply = match body.read_all(Some(limit)).await {
                        Ok(raw) => crate::http1::Response::error(&format!("read {}", raw.len()), 200),
                        Err(crate::http1::BodyError::TooLarge) => crate::http1::Response::error("too large", 413),
                        Err(e) => crate::http1::Response::error(&e.to_string(), 400),
                    };
                    (reply, body)
                })
                .await;
            });
        });
        let mut s = std::net::TcpStream::connect(addr).unwrap();
        s.set_read_timeout(Some(std::time::Duration::from_secs(10))).unwrap();
        let payload = vec![b'x'; len];
        let mut request = b"POST / HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n".to_vec();
        if chunked {
            request.extend_from_slice(b"Transfer-Encoding: chunked\r\n\r\n");
            for piece in payload.chunks(1000) {
                request.extend_from_slice(format!("{:x}\r\n", piece.len()).as_bytes());
                request.extend_from_slice(piece);
                request.extend_from_slice(b"\r\n");
            }
            request.extend_from_slice(b"0\r\n\r\n");
        } else {
            request.extend_from_slice(format!("Content-Length: {len}\r\n\r\n").as_bytes());
            request.extend_from_slice(&payload);
        }
        let _ = s.write_all(&request);
        let mut reply = String::new();
        let _ = s.read_to_string(&mut reply);
        drop(s);
        let _ = server.join();
        if len as u64 <= limit {
            prop_assert!(reply.starts_with("HTTP/1.1 200"), "{}", reply);
            prop_assert!(reply.contains(&format!("read {len}")), "{}", reply);
        } else {
            prop_assert!(reply.starts_with("HTTP/1.1 413"), "{}", reply);
        }
    }
}
