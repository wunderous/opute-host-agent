# Bind IPv4 only for HOST_MCP_BIND_HOST=0.0.0.0

Status: decided (owner, 2026-11-07, D28). Owner question Q14
([rust-spec-authority/questions.md](../archive/2026-11-07-rust-spec-authority/questions.md)).
Implemented after the decision; see tasks.md.

## Why

e2e-1 journey `e2e1.system-unit` observed that `HOST_MCP_BIND_HOST=0.0.0.0`
opens one IPv6 wildcard socket in dual-stack mode (`*:3014`), so the agent
also accepts IPv6 connections. That is Go's `net.Listen` choice
(`favoriteAddrFamily`), which the Rust listener copied. ADR 0008 and the
configuration reference name `0.0.0.0`, an IPv4 address; nothing said the
agent would listen on IPv6. Platform mode binds `0.0.0.0` by default.

## What Changes

An explicit `0.0.0.0` host binds an `AF_INET` socket: IPv4 only. An empty
host and `::` keep one dual-stack socket, as before; the decision did not
cover them. Every other address is unchanged.

## Impact

- `crates/host-agent/src/app.rs` (`go_listen`).
- `e2e1.system-unit` asserts an IPv4 listener (`0.0.0.0:3014`, family
  `inet`) instead of recording Q14.
