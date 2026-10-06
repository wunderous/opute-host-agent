# Refuse null provider input-request entries

## Why

The pinned Go SDK v1.7.0 accepts a null value while decoding its raw input-request map, then dereferences that value at `mcp/protocol.go:106`. The isolated provider-result oracle reproduces a nil-pointer panic for `{"inputRequests":{"fixture-request":null}}`; the process exits with status 2. This establishes a decoder panic, not a claim about every enclosing server's panic recovery.

The migration requires a separate owner decision for a user-visible policy change. This proposal is **pending owner review**. It does not authorize runtime promotion, cutover, or a parity waiver.

## What Changes

After the raw request map passes typed decoding, Rust refuses any null input-request entry before method dispatch with the decode error `nil InputRequest entry`. It does not panic or fulfill any input request from that result. Later well-formed results continue decoding normally. All non-null input-request shapes, typed errors, and mode inference remain governed by the pinned Go contract. Go is unchanged. Differential evidence must identify this single deliberate difference instead of classifying it as parity.

## Capabilities

### New Capabilities

- `provider-input-refusal`: refusal of null entries in provider result input-request maps.

### Modified Capabilities

None.

## Impact

The internal Rust provider result decoder contains the proposed refusal and a regression check, but public provider activation remains closed. The evidence bundle is `evidence/m7/null-input-request/`. Owner approval is required before treating the refusal as the accepted migration contract.
