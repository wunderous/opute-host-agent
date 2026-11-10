# Bound the MCP request body before it is read

## Why

m16's auth review (finding A1) found that `/mcp` read a request's whole body
into memory once its bearer was accepted, and only then applied the SDK's
4 MiB limit with `413 request body exceeds 4194304 bytes`. One authenticated
request, or one from the holder of the local bootstrap bearer, could make the
agent hold a body of any size: a memory-exhaustion crash path.

**Decided (owner, 2026-11-10), recorded as D29.** Bound the read.

## What Changes

After the method, Origin and bearer gates, `/mcp` reads at most 4 MiB of
body. A body larger than that gets the SDK's existing reply at once,
`413 request body exceeds 4194304 bytes`, before the JSON-RPC envelope is
parsed; the rest of it is not read, and the connection closes after the reply
as it does for any unread body over 256 KiB. A body of 4 MiB or less is
treated exactly as before.

The one observable difference: an oversized body that is also malformed, or
whose headers the SDK would refuse, now gets 413 instead of the 400 an
earlier gate gave. Go is unchanged.

## Capabilities

### New Capabilities

- `bound-mcp-request-body`: the request-body bound on `/mcp`.

### Modified Capabilities

None.

## Impact

`crates/host-agent/src/transport.rs` (`mcp`), with the failing-first test
`fuzzing::tests::oversized_mcp_bodies_are_refused_without_reading_them`.
A product change: the next candidate is rust-0.3.0-rc2.
