# Unseal a K3s join key sealed with AES-192

Status: decided (owner, 2026-11-07, D28). Owner question Q7
([rust-spec-authority/questions.md](../archive/2026-11-07-rust-spec-authority/questions.md)):
each ported defect is fixed separately.
Implemented after the decision; see tasks.md.

## Why

The K3s provider seals HA join material for a destination host: a random
AES data key encrypts the material with GCM, and the destination's RSA key
(OAEP, SHA-256) encrypts the data key. Go v0.2.4's `unsealJoinMaterial`
opens the data key with `crypto/aes`, which accepts a 16-, 24- or 32-byte
key. The M9 port (`port-provider-executables` 3.4) used ring, which has no
AES-192 GCM, so a 24-byte data key failed with `crypto/aes: invalid key
size 24`. The port recorded the gap without changing it.

Both sealers use a 32-byte key, so no envelope either side produces today
has a 24-byte key; the gap is in what a receiver accepts.

## What Changes

The receiver opens every key size Go's `crypto/aes` accepts. AES-128 and
AES-256 stay on ring; AES-192 uses RustCrypto's `aes-gcm` (0.10), a new
dependency. Sealing is unchanged (AES-256). Error texts are unchanged.

## Impact

- `crates/host-agent/src/providers/k3s/membership.rs` (`open_join_payload`).
- `crates/host-agent/Cargo.toml`, `Cargo.lock`: `aes-gcm` and its
  RustCrypto dependencies.
