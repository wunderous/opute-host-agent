//! Membership (Go `membership.go`): the native K3s membership read, HA
//! preparation, the sealed join handshake, node retirement and quorum reset.
//!
//! ```text
//!  source: prepare-join -> invitation (memory only) + optional sealed material
//!  destination: get-join-receiver-key -> redeem-join -> join-node (once)
//! ```

use super::decode::{anon, field, items, slice, text, unmarshal, Shape};
use super::{bootstrap, require_provision_target, require_target, run_command, s, Args, Outcome};
use crate::goerr::quote;
use serde_json::{json, Value as J};
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

const JOIN_LABEL: &str = "opute-k3s-join-v1";
const JOIN_MEMBERSHIP_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const JOIN_MEMBERSHIP_POLL: Duration = Duration::from_secs(5);
const QUORUM_LOSS_PROBE_WINDOW: Duration = Duration::from_secs(90);
const QUORUM_LOSS_PROBE_INTERVAL: Duration = Duration::from_secs(10);
const QUORUM_RESET_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const QUORUM_RECOVERY_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const QUORUM_RECOVERY_POLL: Duration = Duration::from_secs(5);
const HA_ENDPOINT_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Clone)]
pub(super) struct Invitation {
    pub(super) destination_host_id: String,
    pub(super) k3s_version: String,
    pub(super) join_token: String,
    pub(super) expires_at: SystemTime,
    pub(super) redeemed: bool,
    pub(super) consumed: bool,
}

pub(super) static INVITATIONS: LazyLock<Mutex<HashMap<String, Invitation>>> = LazyLock::new(Default::default);
static RECEIVER_KEY: OnceLock<rsa::RsaPrivateKey> = OnceLock::new();

/// `membershipObservation`.
#[derive(Default)]
pub(super) struct Observation {
    pub(super) target_uri: String,
    pub(super) cluster_identity: String,
    pub(super) datastore_mode: String,
    pub(super) version: String,
    pub(super) hostname: String,
    pub(super) nodes: Vec<J>,
    pub(super) server_count: i64,
    pub(super) ready_servers: i64,
    pub(super) endpoint_configured: bool,
    pub(super) availability_class: String,
    pub(super) recovery_policy: String,
}

impl Observation {
    fn to_json(&self) -> J {
        let mut object = json!({
            "targetUri": self.target_uri,
            "clusterIdentityKnown": !self.cluster_identity.is_empty(),
            "datastoreMode": self.datastore_mode,
            "version": self.version,
            "nodes": self.nodes,
            "serverCount": self.server_count,
            "readyServers": self.ready_servers,
            "ready": self.ready_servers > 0,
            "endpointConfigured": self.endpoint_configured,
            "availabilityClass": self.availability_class,
            "recoveryPolicy": self.recovery_policy,
        });
        if !self.cluster_identity.is_empty() {
            object["clusterIdentity"] = J::from(self.cluster_identity.as_str());
        }
        if !self.hostname.is_empty() {
            object["hostname"] = J::from(self.hostname.as_str());
        }
        object
    }
}

pub(super) fn receiver_key() -> &'static rsa::RsaPrivateKey {
    RECEIVER_KEY.get_or_init(|| rsa::RsaPrivateKey::new(&mut rand_core::OsRng, 2048).expect("generate RSA key"))
}

pub(super) fn get_join_receiver_key(args: &Args) -> Outcome {
    use rsa::traits::PublicKeyParts as _;
    use sha2::Digest as _;
    let key = receiver_key();
    let der = pkcs1_public_der(&key.n().to_bytes_be(), &key.e().to_bytes_be());
    Ok(json!({
        "targetUri": s(args, "targetUri"),
        "publicKey": raw_url_encode(&der),
        "fingerprint": format!("sha256:{}", hex::encode(sha2::Sha256::digest(&der))),
        "keyAlgorithm": "RSA-OAEP-SHA256",
        "materialDelivery": "sealed-transient-handoff",
    }))
}

pub(super) fn inspect_membership(args: &Args) -> Outcome {
    let mut observation = read_membership(args)?;
    let expected_role = s(args, "expectedRole");
    if !expected_role.is_empty() && expected_role != "server" {
        return Err(format!("unsupported expectedRole {}", quote(&expected_role)));
    }
    let expected_servers = args.int("expectedServers", 0);
    if expected_servers > 0 && observation.server_count < expected_servers {
        return Err(format!(
            "K3s membership has {} server(s), expected at least {expected_servers}",
            observation.server_count
        ));
    }
    if args.bool("requireEmbeddedEtcd", false) && observation.datastore_mode != "embedded-etcd" {
        return Err(format!("K3s target is not using embedded-etcd (observed {})", observation.datastore_mode));
    }
    let endpoint = s(args, "endpoint");
    if !endpoint.is_empty() {
        validate_endpoint(&endpoint)?;
        observation.endpoint_configured = true;
    }
    Ok(observation.to_json())
}

pub(super) fn prepare_ha(args: &Args) -> Outcome {
    let observation = read_membership(args)?;
    if observation.datastore_mode != "embedded-etcd" {
        return Err(format!("server promotion requires embedded-etcd; observed {}", observation.datastore_mode));
    }
    if observation.server_count < 1 {
        return Err("K3s target has no ready server membership".into());
    }
    let endpoint = s(args, "endpoint");
    if !endpoint.is_empty() {
        validate_endpoint(&endpoint)?;
    }
    let mut expected = args.int("expectedServers", 0);
    if expected == 0 {
        expected = 2;
    }
    Ok(json!({
        "targetUri": observation.target_uri,
        "clusterIdentity": observation.cluster_identity,
        "version": observation.version,
        "datastoreMode": observation.datastore_mode,
        "availabilityClass": observation.availability_class,
        "recoveryPolicy": observation.recovery_policy,
        "currentServerCount": observation.server_count,
        "expectedServerCount": expected,
        "readyForServerPromotion": true,
        "endpointConfigured": observation.endpoint_configured,
    }))
}

/// `time.Now().Add(ms)`.
fn now_plus_millis(ms: i64) -> SystemTime {
    let now = SystemTime::now();
    let delta = Duration::from_millis(ms.unsigned_abs());
    if ms >= 0 {
        now + delta
    } else {
        now.checked_sub(delta).unwrap_or(SystemTime::UNIX_EPOCH)
    }
}

pub(super) fn prepare_join(args: &Args) -> Outcome {
    let destination = s(args, "destinationHostId");
    if destination.is_empty() {
        return Err("destinationHostId is required".into());
    }
    let role = super::first_non_empty(&[&s(args, "role"), "server"]);
    if role != "server" {
        return Err("only server joins are supported".into());
    }
    let observation = read_membership(args)?;
    if observation.datastore_mode != "embedded-etcd" {
        return Err(format!("server join requires embedded-etcd; observed {}", observation.datastore_mode));
    }
    let instance = s(args, "providerInstanceName");
    let token_output = run_command(&["exec", &instance, "--", "cat", "/var/lib/rancher/k3s/server/token"], None)
        .map_err(|e| format!("read K3s join material for handshake: {e}"))?;
    let join_token = String::from_utf8_lossy(&token_output).trim().to_string();
    if join_token.is_empty() || super::has_unsafe(&join_token, "\0\r\n") {
        return Err("K3s source returned empty or invalid join material".into());
    }
    let mut expires_in = args.int("expiresInMs", 0);
    if expires_in == 0 {
        expires_in = 15 * 60 * 1000;
    }
    let expires_at = now_plus_millis(expires_in);
    let expires_text = crate::hostobs::rfc3339_nano(expires_at);
    let mut raw = [0u8; 24];
    getrandom::getrandom(&mut raw).map_err(|e| format!("create join redemption reference: {e}"))?;
    let redemption_ref = format!("join-{}", hex::encode(raw));
    INVITATIONS.lock().unwrap().insert(
        redemption_ref.clone(),
        Invitation {
            destination_host_id: destination.clone(),
            k3s_version: observation.version.clone(),
            join_token: join_token.clone(),
            expires_at,
            redeemed: false,
            consumed: false,
        },
    );
    let mut result = json!({
        "targetUri": observation.target_uri,
        "destinationHostId": destination,
        "role": role,
        "clusterIdentity": observation.cluster_identity,
        "expiresAt": expires_text,
        "redemptionRef": redemption_ref,
        "redemptionRequired": true,
        "secretDelivery": "host-agent-handshake",
        "joinMaterialExposed": false,
    });
    let public_key = s(args, "destinationPublicKey");
    if !public_key.is_empty() {
        let material = [
            ("redemptionRef", redemption_ref.as_str()),
            ("sourceTargetUri", observation.target_uri.as_str()),
            ("destinationHostId", destination.as_str()),
            ("clusterIdentity", observation.cluster_identity.as_str()),
            ("k3sVersion", observation.version.as_str()),
            ("joinToken", join_token.as_str()),
            ("expiresAt", expires_text.as_str()),
        ];
        let sealed = seal_join_material(&public_key, &material)
            .map_err(|e| format!("seal join material for destination Host Agent: {e}"))?;
        result["sealedJoinMaterial"] = J::from(sealed);
        result["joinMaterialExposed"] = J::Bool(false);
    }
    Ok(result)
}

/// `json.Marshal` of a struct of string fields, in field order.
fn marshal_strings(fields: &[(&str, &str)]) -> String {
    let mut out = String::from("{");
    for (i, (key, value)) in fields.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        crate::gojson::encode_string(key, &mut out);
        out.push(':');
        crate::gojson::encode_string(value, &mut out);
    }
    out.push('}');
    out
}

pub(super) fn raw_url_encode(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// `base64.RawURLEncoding.DecodeString`: CR and LF are skipped, any other
/// byte outside the alphabet (padding included) is corrupt input.
fn raw_url_decode(text: &str) -> Result<Vec<u8>, String> {
    let bytes = text.as_bytes();
    let mut kept = Vec::with_capacity(bytes.len());
    let mut in_quantum = 0usize;
    for (i, &b) in bytes.iter().enumerate() {
        if b.is_ascii_alphanumeric() || b == b'-' || b == b'_' {
            kept.push(b);
            in_quantum = (in_quantum + 1) % 4;
        } else if b != b'\r' && b != b'\n' {
            return Err(format!("illegal base64 data at input byte {i}"));
        }
    }
    if in_quantum == 1 {
        return Err(format!("illegal base64 data at input byte {}", bytes.len() - 1));
    }
    use base64::engine::{general_purpose::GeneralPurpose, GeneralPurposeConfig};
    use base64::Engine as _;
    let engine = GeneralPurpose::new(
        &base64::alphabet::URL_SAFE,
        GeneralPurposeConfig::new()
            .with_decode_allow_trailing_bits(true)
            .with_decode_padding_mode(base64::engine::DecodePaddingMode::RequireNone),
    );
    engine.decode(&kept).map_err(|e| e.to_string())
}

/// `x509.MarshalPKCS1PublicKey`: SEQUENCE { INTEGER n, INTEGER e }.
pub(super) fn pkcs1_public_der(n: &[u8], e: &[u8]) -> Vec<u8> {
    fn integer(magnitude: &[u8]) -> Vec<u8> {
        let start = magnitude.iter().position(|&b| b != 0).unwrap_or(magnitude.len().saturating_sub(1));
        let mut content = magnitude[start..].to_vec();
        if content.is_empty() {
            content.push(0);
        }
        if content[0] & 0x80 != 0 {
            content.insert(0, 0);
        }
        tlv(0x02, &content)
    }
    let mut body = integer(n);
    body.extend(integer(e));
    tlv(0x30, &body)
}

fn tlv(tag: u8, content: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    let len = content.len();
    if len < 0x80 {
        out.push(len as u8);
    } else {
        let bytes: Vec<u8> = len.to_be_bytes().iter().copied().skip_while(|&b| b == 0).collect();
        out.push(0x80 | bytes.len() as u8);
        out.extend(bytes);
    }
    out.extend_from_slice(content);
    out
}

// --- encoding/asn1, as x509.ParsePKCS1PublicKey drives it --------------------------------

struct TagLength {
    class: u8,
    tag: i64,
    length: usize,
    compound: bool,
}

const PARAMS: &str = "{optional:false explicit:false application:false private:false defaultValue:<nil> tag:<nil> stringType:0 timeType:0 set:false omitEmpty:false}";

fn syntax(message: &str) -> String {
    format!("asn1: syntax error: {message}")
}

fn structural(message: &str) -> String {
    format!("asn1: structure error: {message}")
}

fn base128(bytes: &[u8], mut offset: usize) -> Result<(i64, usize), String> {
    let mut value: i64 = 0;
    let mut shifted = 0;
    while offset < bytes.len() {
        if shifted == 5 {
            return Err(structural("base 128 integer too large"));
        }
        value <<= 7;
        let b = bytes[offset];
        if shifted == 0 && b == 0x80 {
            return Err(syntax("integer is not minimally encoded"));
        }
        value |= i64::from(b & 0x7f);
        offset += 1;
        if b & 0x80 == 0 {
            if value > i64::from(i32::MAX) {
                return Err(structural("base 128 integer too large"));
            }
            return Ok((value, offset));
        }
        shifted += 1;
    }
    Err(syntax("truncated base 128 integer"))
}

fn tag_and_length(bytes: &[u8], mut offset: usize) -> Result<(TagLength, usize), String> {
    let b = bytes[offset];
    offset += 1;
    let mut t = TagLength { class: b >> 6, tag: i64::from(b & 0x1f), length: 0, compound: b & 0x20 != 0 };
    if t.tag == 0x1f {
        let (tag, next) = base128(bytes, offset)?;
        offset = next;
        if tag < 0x1f {
            return Err(syntax("non-minimal tag"));
        }
        t.tag = tag;
    }
    if offset >= bytes.len() {
        return Err(syntax("truncated tag or length"));
    }
    let b = bytes[offset];
    offset += 1;
    if b & 0x80 == 0 {
        t.length = usize::from(b & 0x7f);
    } else {
        let count = usize::from(b & 0x7f);
        if count == 0 {
            return Err(syntax("indefinite length found (not DER)"));
        }
        for _ in 0..count {
            if offset >= bytes.len() {
                return Err(syntax("truncated tag or length"));
            }
            let b = bytes[offset];
            offset += 1;
            if t.length >= 1 << 23 {
                return Err(structural("length too large"));
            }
            t.length = (t.length << 8) | usize::from(b);
            if t.length == 0 {
                return Err(structural("superfluous leading zeros in length"));
            }
        }
        if t.length < 0x80 {
            return Err(structural("non-minimal length"));
        }
    }
    Ok((t, offset))
}

/// `parseField` for a universal, non-optional field: its content and the
/// offset after it.
fn expect<'a>(
    bytes: &'a [u8],
    offset: usize,
    tag: i64,
    compound: bool,
    type_name: &str,
) -> Result<(&'a [u8], usize), String> {
    if offset == bytes.len() {
        return Err(syntax("sequence truncated"));
    }
    let (t, offset) = tag_and_length(bytes, offset)?;
    if t.class != 0 || t.tag != tag || t.compound != compound {
        return Err(structural(&format!(
            "tags don't match ({tag} vs {{class:{} tag:{} length:{} isCompound:{}}}) {PARAMS} {type_name} @{offset}",
            t.class, t.tag, t.length, t.compound
        )));
    }
    if offset + t.length > bytes.len() {
        return Err(syntax("data truncated"));
    }
    Ok((&bytes[offset..offset + t.length], offset + t.length))
}

fn check_integer(bytes: &[u8]) -> Result<(), String> {
    if bytes.is_empty() {
        return Err(structural("empty integer"));
    }
    if bytes.len() > 1 && ((bytes[0] == 0 && bytes[1] & 0x80 == 0) || (bytes[0] == 0xff && bytes[1] & 0x80 == 0x80)) {
        return Err(structural("integer not minimally-encoded"));
    }
    Ok(())
}

/// Whether the DER is a PKIX SubjectPublicKeyInfo `asn1.Unmarshal` accepts.
fn is_pkix(der: &[u8]) -> bool {
    let pkix = || -> Result<(), String> {
        let (info, _) = expect(der, 0, 16, true, "publicKeyInfo")?;
        let (algorithm, offset) = expect(info, 0, 16, true, "AlgorithmIdentifier")?;
        let (oid, after_oid) = expect(algorithm, 0, 6, false, "ObjectIdentifier")?;
        if oid.is_empty() {
            return Err(String::new());
        }
        let mut at = 0;
        while at < oid.len() {
            at = base128(oid, at)?.1;
        }
        if after_oid < algorithm.len() {
            let (t, inner) = tag_and_length(algorithm, after_oid)?;
            if inner + t.length > algorithm.len() {
                return Err(String::new());
            }
        }
        let (bits, _) = expect(info, offset, 3, false, "BitString")?;
        let padding = *bits.first().ok_or_else(String::new)?;
        if padding > 7 || (bits.len() == 1 && padding > 0) || bits[bits.len() - 1] & ((1u8 << padding) - 1) != 0 {
            return Err(String::new());
        }
        Ok(())
    };
    pkix().is_ok()
}

/// `x509.ParsePKCS1PublicKey`: the modulus and exponent.
pub(super) fn parse_pkcs1_public_key(der: &[u8]) -> Result<(Vec<u8>, i64), String> {
    let parsed = || -> Result<(Vec<u8>, i64, bool, usize), String> {
        let (body, rest) = expect(der, 0, 16, true, "pkcs1PublicKey")?;
        let (n, offset) = expect(body, 0, 2, false, "")?;
        check_integer(n)?;
        let (e, _) = expect(body, offset, 2, false, "int")?;
        check_integer(e)?;
        if e.len() > 8 {
            return Err(structural("integer too large"));
        }
        let mut exponent: i64 = 0;
        for &b in e {
            exponent = (exponent << 8) | i64::from(b);
        }
        exponent <<= 64 - 8 * e.len() as u32;
        exponent >>= 64 - 8 * e.len() as u32;
        let negative = n[0] & 0x80 != 0;
        Ok((n.to_vec(), exponent, negative, rest))
    };
    let (n, e, negative, rest) = match parsed() {
        Ok(v) => v,
        Err(e) => {
            if is_pkix(der) {
                return Err("x509: failed to parse public key (use ParsePKIXPublicKey instead for this key format)".into());
            }
            return Err(e);
        }
    };
    if rest < der.len() {
        return Err(syntax("trailing data"));
    }
    if negative || n.iter().all(|&b| b == 0) || e <= 0 {
        return Err("x509: public key contains zero or negative value".into());
    }
    if e > i64::from(i32::MAX) {
        return Err("x509: public key contains large public exponent".into());
    }
    Ok((n, e))
}

fn bit_len(magnitude: &[u8]) -> usize {
    match magnitude.iter().position(|&b| b != 0) {
        Some(i) => (magnitude.len() - i - 1) * 8 + (8 - magnitude[i].leading_zeros() as usize),
        None => 0,
    }
}

/// `rsa.EncryptOAEP(sha256, rand, key, msg, label)` with Go's key checks.
fn encrypt_oaep(n: &[u8], e: i64, msg: &[u8]) -> Result<Vec<u8>, String> {
    let bits = bit_len(n);
    if bits < 1024 {
        return Err(format!(
            "crypto/rsa: {bits}-bit keys are insecure (see https://go.dev/pkg/crypto/rsa#hdr-Minimum_key_size)"
        ));
    }
    if n.last().is_some_and(|b| b & 1 == 0) {
        return Err("crypto/rsa: public modulus is even".into());
    }
    if e < 2 {
        return Err("crypto/rsa: public exponent too small or negative".into());
    }
    if e & 1 == 0 {
        return Err("crypto/rsa: public exponent is even".into());
    }
    let key = rsa::RsaPublicKey::new_with_max_size(
        rsa::BigUint::from_bytes_be(n),
        rsa::BigUint::from(e as u64),
        usize::MAX,
    )
    .map_err(|err| format!("crypto/rsa: {err}"))?;
    key.encrypt(&mut rand_core::OsRng, rsa::Oaep::new_with_label::<sha2::Sha256, _>(JOIN_LABEL), msg)
        .map_err(|_| "crypto/rsa: message too long for RSA key size".to_string())
}

pub(super) fn seal_join_material(public_key: &str, material: &[(&str, &str)]) -> Result<String, String> {
    let der = raw_url_decode(public_key).map_err(|e| format!("decode destination public key: {e}"))?;
    let (n, e) = parse_pkcs1_public_key(&der).map_err(|e| format!("parse destination public key: {e}"))?;
    let payload = marshal_strings(material);
    let mut data_key = [0u8; 32];
    getrandom::getrandom(&mut data_key).map_err(|e| format!("create join material key: {e}"))?;
    let mut nonce = [0u8; 12];
    getrandom::getrandom(&mut nonce).map_err(|e| format!("create join material nonce: {e}"))?;
    let encrypted_key = encrypt_oaep(&n, e, &data_key).map_err(|e| format!("encrypt join material key: {e}"))?;
    let key = ring::aead::LessSafeKey::new(
        ring::aead::UnboundKey::new(&ring::aead::AES_256_GCM, &data_key).expect("AES-256 key"),
    );
    let mut sealed = payload.into_bytes();
    key.seal_in_place_append_tag(ring::aead::Nonce::assume_unique_for_key(nonce), ring::aead::Aad::empty(), &mut sealed)
        .expect("seal");
    let envelope = marshal_strings(&[
        ("key", &raw_url_encode(&encrypted_key)),
        ("nonce", &raw_url_encode(&nonce)),
        ("ciphertext", &raw_url_encode(&sealed)),
    ]);
    Ok(raw_url_encode(envelope.as_bytes()))
}

fn string_struct(name: &'static str, fields: &[(&'static str, &'static str)]) -> Shape {
    Shape::Struct { name, fields: fields.iter().map(|(go, json)| field(go, json, Shape::Str)).collect() }
}

/// `unsealJoinMaterial`: the material's fields, by JSON name.
pub(super) fn unseal_join_material(sealed: &str) -> Result<J, String> {
    let envelope_bytes = raw_url_decode(sealed).map_err(|e| format!("decode sealed join material: {e}"))?;
    let envelope_shape = string_struct(
        "sealedJoinEnvelope",
        &[("Key", "key"), ("Nonce", "nonce"), ("Ciphertext", "ciphertext")],
    );
    let envelope = unmarshal(&envelope_bytes, &envelope_shape)
        .map_err(|e| format!("decode sealed join material envelope: {e}"))?;
    let encrypted_key =
        raw_url_decode(text(&envelope, "key")).map_err(|e| format!("decode sealed join material key: {e}"))?;
    let nonce = raw_url_decode(text(&envelope, "nonce")).map_err(|e| format!("decode sealed join material nonce: {e}"))?;
    let ciphertext = raw_url_decode(text(&envelope, "ciphertext"))
        .map_err(|e| format!("decode sealed join material ciphertext: {e}"))?;
    let data_key = receiver_key()
        .decrypt_blinded(&mut rand_core::OsRng, rsa::Oaep::new_with_label::<sha2::Sha256, _>(JOIN_LABEL), &encrypted_key)
        .map_err(|_| "decrypt sealed join material key: crypto/rsa: decryption error".to_string())?;
    let algorithm = match data_key.len() {
        16 => &ring::aead::AES_128_GCM,
        32 => &ring::aead::AES_256_GCM,
        // AES-192 (a 24-byte key) is a recorded gap: ring has no AES-192 GCM.
        n => return Err(format!("create sealed join material cipher: crypto/aes: invalid key size {n}")),
    };
    if nonce.len() != 12 {
        super::super::go_panic("crypto/cipher: incorrect nonce length given to GCM");
    }
    let key = ring::aead::LessSafeKey::new(ring::aead::UnboundKey::new(algorithm, &data_key).expect("AES key"));
    let mut buffer = ciphertext;
    let nonce = ring::aead::Nonce::try_assume_unique_for_key(&nonce).expect("nonce");
    let payload = key
        .open_in_place(nonce, ring::aead::Aad::empty(), &mut buffer)
        .map_err(|_| "decrypt sealed join material: cipher: message authentication failed".to_string())?
        .to_vec();
    let material_shape = string_struct(
        "sealedJoinMaterial",
        &[
            ("RedemptionRef", "redemptionRef"),
            ("SourceTargetURI", "sourceTargetUri"),
            ("DestinationHostID", "destinationHostId"),
            ("ClusterIdentity", "clusterIdentity"),
            ("K3sVersion", "k3sVersion"),
            ("JoinToken", "joinToken"),
            ("ExpiresAt", "expiresAt"),
        ],
    );
    unmarshal(&payload, &material_shape).map_err(|e| format!("decode sealed join material: {e}"))
}

/// The source token stays memory-only and is handed to join-node once.
pub(super) fn redeem_join(args: &Args) -> Outcome {
    let reference = s(args, "redemptionRef");
    if reference.is_empty() {
        return Err("redemptionRef is required".into());
    }
    let mut invitations = INVITATIONS.lock().unwrap();
    let mut invitation = match invitations.get(&reference) {
        Some(invitation) => invitation.clone(),
        None => {
            let sealed = s(args, "sealedJoinMaterial");
            if sealed.is_empty() {
                return Err("join redemption reference is unknown or belongs to another Host Agent".into());
            }
            let material = unseal_join_material(&sealed)?;
            let m = |k: &str| text(&material, k).to_string();
            if m("redemptionRef") != reference
                || m("sourceTargetUri").is_empty()
                || m("joinToken").is_empty()
                || m("k3sVersion").is_empty()
            {
                return Err("sealed join material does not match the redemption reference".into());
            }
            let destination = s(args, "destinationHostId");
            if destination.is_empty() || destination != m("destinationHostId") {
                return Err("sealed join material is bound to another destination Host Agent".into());
            }
            let expires_at = match crate::hostobs::parse_rfc3339(&m("expiresAt")) {
                Some(t) if SystemTime::now() <= t => t,
                _ => return Err("sealed join material is expired".into()),
            };
            let source = s(args, "sourceTargetUri");
            if !source.is_empty() && source != m("sourceTargetUri") {
                return Err("sealed join material is bound to another source target".into());
            }
            Invitation {
                destination_host_id: m("destinationHostId"),
                k3s_version: m("k3sVersion"),
                join_token: m("joinToken"),
                expires_at,
                redeemed: false,
                consumed: false,
            }
        }
    };
    if invitation.consumed || SystemTime::now() > invitation.expires_at {
        return Err("join redemption reference is expired or already consumed".into());
    }
    let destination = s(args, "destinationHostId");
    if !destination.is_empty() && destination != invitation.destination_host_id {
        return Err("join redemption reference is bound to another destination Host Agent".into());
    }
    invitation.redeemed = true;
    let expires = crate::hostobs::rfc3339_nano(invitation.expires_at);
    invitations.insert(reference.clone(), invitation);
    Ok(json!({"redemptionRef": reference, "redeemed": true, "expiresAt": expires, "joinMaterialExposed": false}))
}

pub(super) fn join_node(args: &Args) -> Outcome {
    if s(args, "role") != "server" {
        return Err("join-node requires role=server".into());
    }
    if args.bool("clusterInit", false) {
        return Err("join-node refuses cluster-init=true".into());
    }
    if s(args, "redemptionRef").is_empty() {
        return Err("redemptionRef is required".into());
    }
    let (token, invitation_ref, version) = join_token_for_node(args)?;
    if version.is_empty() {
        return Err("K3s version is required when join material comes from an external Host Agent handshake".into());
    }
    if super::has_unsafe(&version, super::UNSAFE_VERSION) {
        return Err("K3s version contains unsafe characters".into());
    }
    let address = s(args, "serverAddress");
    validate_endpoint(&address)?;
    let instance = s(args, "providerInstanceName");
    // Refuse a bad target before any installer side effect; the token
    // reaches the guest only as transient installer input.
    let install_exec = super::server_install_exec(args, false)?;
    // The installer's exit is one attempt's report: a first-start etcd join
    // can exit fatally and be restarted into a completed join, so the
    // membership observation is the authority.
    let install = bootstrap::install_k3s_server(&instance, &version, &install_exec, &address, &token);
    let observation = match await_joined_membership(args) {
        Ok(observation) => observation,
        Err(e) => {
            return Err(match install {
                Err(install_err) => format!("join K3s server: {install_err}"),
                Ok(()) => format!("verify joined K3s server: {e}"),
            })
        }
    };
    if !invitation_ref.is_empty() {
        INVITATIONS.lock().unwrap().remove(&invitation_ref);
    }
    Ok(json!({
        "targetUri": observation.target_uri,
        "joined": true,
        "clusterIdentity": observation.cluster_identity,
        "version": observation.version,
        "datastoreMode": observation.datastore_mode,
        "availabilityClass": observation.availability_class,
        "recoveryPolicy": observation.recovery_policy,
        "serverCount": observation.server_count,
        "readyServers": observation.ready_servers,
        "installerExitedNonZero": install.is_err(),
    }))
}

/// `joinTokenForNode`: the token, the invitation it consumed, the version.
pub(super) fn join_token_for_node(args: &Args) -> Result<(String, String, String), String> {
    let reference = s(args, "redemptionRef");
    if reference.is_empty() {
        return Err("redemptionRef is required".into());
    }
    let provided = s(args, "joinToken");
    let provided_version = s(args, "version");
    let mut invitations = INVITATIONS.lock().unwrap();
    if let Some(invitation) = invitations.get_mut(&reference) {
        if invitation.consumed || SystemTime::now() > invitation.expires_at {
            return Err("join redemption reference is expired or already consumed".into());
        }
        if !invitation.redeemed {
            return Err("join redemption reference has not been redeemed by the destination Host Agent".into());
        }
        if !provided.is_empty() && provided != invitation.join_token {
            return Err("join token does not match the redeemed invitation".into());
        }
        if !provided_version.is_empty() && provided_version != invitation.k3s_version {
            return Err("K3s version does not match the source invitation".into());
        }
        invitation.consumed = true;
        return Ok((invitation.join_token.clone(), reference, invitation.k3s_version.clone()));
    }
    if provided.is_empty() || super::has_unsafe(&provided, "\0\r\n") {
        return Err("joinToken is required as transient secret material".into());
    }
    Ok((provided, String::new(), provided_version))
}

pub(super) fn ensure_ha_endpoint(args: &Args) -> Outcome {
    let endpoint = s(args, "endpoint");
    validate_endpoint(&endpoint)?;
    let observation = read_membership(args)?;
    if observation.datastore_mode != "embedded-etcd" {
        return Err(format!("stable HA endpoint requires embedded-etcd; observed {}", observation.datastore_mode));
    }
    if observation.ready_servers < 1 {
        return Err("stable HA endpoint requires at least one ready server".into());
    }
    let status = probe_ha_endpoint(&endpoint).map_err(|e| format!("probe stable HA endpoint: {e}"))?;
    Ok(json!({
        "targetUri": observation.target_uri,
        "endpoint": endpoint.trim_end_matches('/'),
        "endpointAvailable": true,
        "endpointStatusCode": status,
        "clusterIdentity": observation.cluster_identity,
        "serverCount": observation.server_count,
        "readyServers": observation.ready_servers,
        "datastoreMode": observation.datastore_mode,
        "availabilityClass": observation.availability_class,
        "recoveryPolicy": observation.recovery_policy,
    }))
}

/// The probe runs from the provider host, with the host's certificate roots.
fn probe_ha_endpoint(endpoint: &str) -> Result<u16, String> {
    let url = format!("{}/version", endpoint.trim_end_matches('/'));
    let response = super::super::support::http_do(super::super::support::HttpRequest {
        method: "GET",
        url: &url,
        headers: Vec::new(),
        body: None,
        timeout: Some(HA_ENDPOINT_TIMEOUT),
        https_redirects: None,
        limit: 4096,
    })?;
    let status = response.status;
    if !((200..400).contains(&status) || status == 401 || status == 403) {
        return Err(format!("stable HA endpoint returned invalid HTTP status {status}"));
    }
    Ok(status)
}

fn find_node<'a>(nodes: &'a [J], name: &str) -> Option<&'a J> {
    nodes.iter().find(|node| node.get("name").and_then(J::as_str) == Some(name))
}

fn is_ready(node: &J) -> bool {
    node.get("ready") == Some(&J::Bool(true))
}

/// Retire a server: deleting its node object retires its etcd member. The
/// quorum proof is computed against the membership that would remain.
pub(super) fn remove_node(args: &Args) -> Outcome {
    let node_name = s(args, "nodeName");
    if node_name.is_empty() || s(args, "role") != "server" {
        return Err("remove-node requires nodeName and role=server".into());
    }
    let observation = read_membership(args)?;
    if observation.datastore_mode != "embedded-etcd" {
        return Err(format!("server removal requires embedded-etcd; observed {}", observation.datastore_mode));
    }
    if !observation.hostname.is_empty() && observation.hostname == node_name {
        return Err(format!(
            "refusing to remove {}: it is the server this operation is executing through",
            quote(&node_name)
        ));
    }
    let Some(member) = find_node(&observation.nodes, &node_name).cloned() else {
        return Err(format!(
            "node {} is not a member of the cluster at {}",
            quote(&node_name),
            observation.target_uri
        ));
    };
    let (remaining_servers, remaining_ready) = quorum_after_removal(&observation, &member)?;
    run_command(
        &["exec", &s(args, "providerInstanceName"), "--", "k3s", "kubectl", "delete", "node", &node_name, "--wait=true"],
        None,
    )
    .map_err(|e| format!("retire K3s server {}: {e}", quote(&node_name)))?;
    let after = read_membership(args).map_err(|e| format!("read membership after retiring {}: {e}", quote(&node_name)))?;
    if find_node(&after.nodes, &node_name).is_some() {
        return Err(format!("K3s server {} is still a member after removal", quote(&node_name)));
    }
    if after.ready_servers * 2 <= after.server_count {
        return Err(format!(
            "K3s cluster lost quorum after removing {}: {} ready of {} server(s)",
            quote(&node_name),
            after.ready_servers,
            after.server_count
        ));
    }
    Ok(json!({
        "targetUri": after.target_uri,
        "clusterIdentity": after.cluster_identity,
        "removedNode": node_name,
        "removedNodeWasReady": is_ready(&member),
        "datastoreMode": after.datastore_mode,
        "availabilityClass": after.availability_class,
        "recoveryPolicy": after.recovery_policy,
        "serverCountBefore": observation.server_count,
        "readyServersBefore": observation.ready_servers,
        "projectedServerCount": remaining_servers,
        "projectedReadyServers": remaining_ready,
        "serverCount": after.server_count,
        "readyServers": after.ready_servers,
        "quorumRetained": true,
    }))
}

/// The servers that stay ready must remain a strict majority of the servers
/// that stay members.
pub(super) fn quorum_after_removal(observation: &Observation, member: &J) -> Result<(i64, i64), String> {
    let remaining_servers = observation.server_count - 1;
    let mut remaining_ready = observation.ready_servers;
    if is_ready(member) {
        remaining_ready -= 1;
    }
    if remaining_servers < 1 {
        return Err("refusing server removal without source-cluster quorum proof: it is the last server of the cluster".into());
    }
    if remaining_ready * 2 <= remaining_servers {
        return Err(format!(
            "refusing server removal without source-cluster quorum proof: removal would leave {remaining_ready} ready of {remaining_servers} server(s)"
        ));
    }
    Ok((remaining_servers, remaining_ready))
}

pub(super) fn read_membership(args: &Args) -> Result<Observation, String> {
    read_membership_for_target(args, require_target)
}

/// The membership read under the caller's target guard.
pub(super) fn read_membership_for_target(args: &Args, guard: fn(&Args) -> Result<(), String>) -> Result<Observation, String> {
    guard(args)?;
    let instance = s(args, "providerInstanceName");
    let version_output = run_command(&["exec", &instance, "--", "k3s", "--version"], None)?;
    let nodes_output = run_command(&["exec", &instance, "--", "k3s", "kubectl", "get", "nodes", "-o", "json"], None)
        .map_err(|e| format!("read native K3s membership: {e}"))?;
    let nodes = parse_native_membership(&nodes_output)?;
    let mut identity = String::new();
    if let Ok(output) = run_command(&["exec", &instance, "--", "sha256sum", "/var/lib/rancher/k3s/server/token"], None) {
        let text = String::from_utf8_lossy(&output).into_owned();
        if let Some(first) = text.split_whitespace().next().filter(|f| f.len() == 64) {
            identity = format!("sha256:{first}");
        }
    }
    let mut datastore = "unknown";
    if run_command(&["exec", &instance, "--", "test", "-d", "/var/lib/rancher/k3s/server/db/etcd/member"], None).is_ok() {
        datastore = "embedded-etcd";
    } else if run_command(&["exec", &instance, "--", "test", "-f", "/var/lib/rancher/k3s/server/db/state.db"], None).is_ok() {
        datastore = "sqlite";
    }
    let hostname = run_command(&["exec", &instance, "--", "hostname"], None).unwrap_or_default();
    let (mut ready_servers, mut server_count) = (0, 0);
    for node in &nodes {
        let roles: Vec<&str> = node["roles"].as_array().map_or(Vec::new(), |r| r.iter().filter_map(J::as_str).collect());
        if roles.is_empty() || roles.contains(&"control-plane") || roles.contains(&"etcd") {
            server_count += 1;
            if is_ready(node) {
                ready_servers += 1;
            }
        }
    }
    let requested = s(args, "datastoreMode");
    let (availability, recovery) = classify_two_node_availability(datastore, server_count, &requested);
    Ok(Observation {
        target_uri: s(args, "targetUri"),
        cluster_identity: identity,
        datastore_mode: if requested.is_empty() { datastore.to_string() } else { requested },
        version: super::parse_k3s_version(&String::from_utf8_lossy(&version_output)),
        hostname: String::from_utf8_lossy(&hostname).trim().to_string(),
        nodes,
        server_count,
        ready_servers,
        endpoint_configured: !s(args, "endpoint").is_empty(),
        availability_class: availability.into(),
        recovery_policy: recovery.into(),
    })
}

fn classify_two_node_availability(observed: &str, server_count: i64, requested: &str) -> (&'static str, &'static str) {
    let mode = if requested.trim().is_empty() { observed } else { requested.trim() };
    match mode {
        "external-datastore" => ("control-plane-write-continuity", "datastore-failover"),
        "embedded-etcd-two-node" | "embedded-etcd" if server_count <= 2 => {
            ("serving-continuity-only", "acknowledge-single-member-reset")
        }
        "embedded-etcd-two-node" | "embedded-etcd" => ("control-plane-write-continuity", "etcd-quorum"),
        _ => ("unproven", "none"),
    }
}

fn native_membership_shape() -> Shape {
    anon(vec![field(
        "Items",
        "items",
        slice(anon(vec![
            field(
                "Metadata",
                "metadata",
                anon(vec![field("Name", "name", Shape::Str), field("Labels", "labels", Shape::MapStr)]),
            ),
            field(
                "Status",
                "status",
                anon(vec![
                    field(
                        "Conditions",
                        "conditions",
                        slice(anon(vec![field("Type", "type", Shape::Str), field("Status", "status", Shape::Str)])),
                    ),
                    field("NodeInfo", "nodeInfo", anon(vec![field("KubeletVersion", "kubeletVersion", Shape::Str)])),
                ]),
            ),
        ])),
    )])
}

/// `parseNativeMembership`. Roles come from a Go map's keys, in random
/// order there; here they are sorted.
pub(super) fn parse_native_membership(raw: &[u8]) -> Result<Vec<J>, String> {
    let list = unmarshal(raw, &native_membership_shape()).map_err(|e| format!("invalid native K3s membership response: {e}"))?;
    let mut nodes = Vec::new();
    for item in items(&list, "items") {
        let metadata = &item["metadata"];
        let name = text(metadata, "name");
        if name.trim().is_empty() {
            return Err("native K3s membership contained a node without a name".into());
        }
        let mut roles: Vec<&str> = metadata["labels"]
            .as_object()
            .map(|labels| labels.keys().filter_map(|k| k.strip_prefix("node-role.kubernetes.io/")).collect())
            .unwrap_or_default();
        roles.sort_unstable();
        let mut ready = false;
        for condition in items(&item["status"], "conditions") {
            if text(condition, "type") == "Ready" {
                ready = text(condition, "status") == "True";
            }
        }
        let mut node = json!({"name": name, "roles": roles, "ready": ready, "status": if ready { "Ready" } else { "NotReady" }});
        let kubelet = text(&item["status"]["nodeInfo"], "kubeletVersion");
        if !kubelet.is_empty() {
            node["version"] = J::from(kubelet);
        }
        nodes.push(node);
    }
    Ok(nodes)
}

/// `parseClusterNodeSnapshot`: the nodes and how many are ready.
pub(super) fn parse_cluster_node_snapshot(raw: &[u8]) -> Result<(Vec<J>, usize), String> {
    let nodes = parse_native_membership(raw)?;
    let ready = nodes.iter().filter(|n| is_ready(n)).count();
    Ok((nodes, ready))
}

/// `validateEndpoint`: an absolute http(s) URL with no path, query or fragment.
pub(super) fn validate_endpoint(value: &str) -> Result<(), String> {
    let valid = crate::mcpprobe::GoUrl::parse(value.trim()).is_some_and(|u| {
        (u.scheme == "https" || u.scheme == "http")
            && !u.host.is_empty()
            && u.path.is_empty()
            && u.raw_query.is_empty()
            && u.fragment.is_empty()
    });
    if !valid || super::has_unsafe(value, "\0\r\n") {
        return Err("endpoint must be an absolute http or https URL without a path or query".into());
    }
    Ok(())
}

/// The joining node itself is a ready member of a multi-server view.
pub(super) fn joined_settled(observation: &Observation) -> bool {
    !observation.hostname.is_empty()
        && observation.server_count >= 2
        && find_node(&observation.nodes, &observation.hostname).is_some_and(is_ready)
}

/// Hold join-node open until the joining node itself is a ready member: a
/// server count alone can be met by the servers already there.
fn await_joined_membership(args: &Args) -> Result<Observation, String> {
    let deadline = Instant::now() + JOIN_MEMBERSHIP_TIMEOUT;
    loop {
        let read = read_membership_for_target(args, require_provision_target);
        if let Ok(observation) = &read {
            if joined_settled(observation) {
                return read;
            }
        }
        if Instant::now() > deadline {
            return Err(match read {
                Err(e) => format!("joined K3s server did not become a ready member within 10m0s: {e}"),
                Ok(o) => format!(
                    "joined K3s server {} did not become a ready member within 10m0s: {} ready of {} server(s)",
                    quote(&o.hostname),
                    o.ready_servers,
                    o.server_count
                ),
            });
        }
        std::thread::sleep(JOIN_MEMBERSHIP_POLL);
    }
}

/// Restore a single-server etcd majority for a cluster whose embedded etcd
/// lost the peers it needs. Fail-closed: embedded etcd observed on the guest
/// filesystem, the API server unreachable across a settling window, and the
/// collapse acknowledged.
pub(super) fn recover_quorum(args: &Args) -> Outcome {
    let instance = s(args, "providerInstanceName");
    let acknowledged = args.bool("acknowledgeSingleMemberReset", false);
    let embedded_etcd =
        run_command(&["exec", &instance, "--", "test", "-d", "/var/lib/rancher/k3s/server/db/etcd/member"], None).is_ok();
    let (answered, probes, last_probe_error) = probe_api_server_during_window(&instance);
    quorum_recovery_admissible(embedded_etcd, answered, acknowledged)?;
    let hostname = run_command(&["exec", &instance, "--", "hostname"], None).unwrap_or_default();
    let survivor = String::from_utf8_lossy(&hostname).trim().to_string();
    run_command(&["exec", &instance, "--", "systemctl", "stop", "k3s"], None)
        .map_err(|e| format!("stop K3s before cluster reset on {}: {e}", quote(&instance)))?;
    // A half-rewritten datastore is left stopped, with the original error.
    super::run_program("incus", &["exec", &instance, "--", "k3s", "server", "--cluster-reset"], None, Some(QUORUM_RESET_TIMEOUT))
        .map_err(|(err, message)| {
            format!("reset K3s etcd membership on {}: incus K3s command failed: {err}: {message}", quote(&instance))
        })?;
    run_command(&["exec", &instance, "--", "systemctl", "start", "k3s"], None)
        .map_err(|e| format!("start K3s after cluster reset on {}: {e}", quote(&instance)))?;
    let observation = await_recovered_membership(args)?;
    if observation.datastore_mode != "embedded-etcd" {
        return Err(format!(
            "K3s datastore is {} after cluster reset; expected embedded-etcd",
            observation.datastore_mode
        ));
    }
    Ok(json!({
        "targetUri": observation.target_uri,
        "clusterIdentity": observation.cluster_identity,
        "datastoreMode": observation.datastore_mode,
        "availabilityClass": observation.availability_class,
        "recoveryPolicy": observation.recovery_policy,
        "recoveredServer": survivor,
        "serverCount": observation.server_count,
        "readyServers": observation.ready_servers,
        "quorumRestored": true,
        "unreachableProbes": probes,
        "lastProbeError": last_probe_error,
        "nodes": observation.nodes,
    }))
}

/// Whether the API server served the membership read within the window.
fn probe_api_server_during_window(instance: &str) -> (bool, i64, String) {
    let deadline = Instant::now() + QUORUM_LOSS_PROBE_WINDOW;
    let mut probes = 0;
    loop {
        probes += 1;
        let last = match run_command(&["exec", instance, "--", "k3s", "kubectl", "get", "nodes", "-o", "json"], None) {
            Ok(_) => return (true, probes, String::new()),
            Err(e) => truncate_probe_error(&e),
        };
        if Instant::now() > deadline {
            return (false, probes, last);
        }
        std::thread::sleep(QUORUM_LOSS_PROBE_INTERVAL);
    }
}

pub(super) fn truncate_probe_error(message: &str) -> String {
    let message = message.trim();
    if message.len() > 512 {
        // A byte cut, as Go slices it; a split rune becomes U+FFFD per byte.
        return String::from_utf8_lossy(&message.as_bytes()[..512]).into_owned();
    }
    message.to_string()
}

pub(super) fn quorum_recovery_admissible(embedded_etcd: bool, answered: bool, acknowledged: bool) -> Result<(), String> {
    if !acknowledged {
        return Err("recover-quorum requires acknowledgeSingleMemberReset=true: the reset collapses etcd membership to the surviving server".into());
    }
    if !embedded_etcd {
        return Err("recover-quorum requires embedded-etcd: no etcd member directory is present on the guest".into());
    }
    if answered {
        return Err("refusing cluster reset: the K3s API server served membership within 1m30s, so the cluster has quorum; use remove-node to retire a departed server".into());
    }
    Ok(())
}

/// Hold the reset open until the server is a ready member of its own cluster.
fn await_recovered_membership(args: &Args) -> Result<Observation, String> {
    let deadline = Instant::now() + QUORUM_RECOVERY_TIMEOUT;
    loop {
        let read = read_membership(args);
        if let Ok(o) = &read {
            if o.server_count >= 1 && o.ready_servers * 2 > o.server_count {
                return read;
            }
        }
        if Instant::now() > deadline {
            return Err(match read {
                Err(e) => format!("K3s did not regain a readable majority within 10m0s after cluster reset: {e}"),
                Ok(o) => format!(
                    "K3s did not regain a majority within 10m0s after cluster reset: {} ready of {} server(s)",
                    o.ready_servers, o.server_count
                ),
            });
        }
        std::thread::sleep(QUORUM_RECOVERY_POLL);
    }
}
