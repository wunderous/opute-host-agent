# Provider executables listen on loopback only

Status: **accepted (Host Agent owner, 2026-11-21) as D53**. Goal m25, finding P1. Applies to 0.3.0 from `rust-0.3.0-rc8` and to 0.4.0 from `rust-0.4.0-rc5`.

## Why

Each provider executable (hostos, cloudflare, tailscale, k3s) serves its MCP
handler as Go's did, with `http.ListenAndServe(":"+port, handler)`: on every
interface, IPv4 and IPv6, with no authentication. D26 ported that faithfully.

m25 widened the journey observer to listening sockets, and every journey that
installs a provider showed `tcp *:4320 opute-provider-`. On the dev VM, an
Incus container on `incusbr0`, holding no credential, called the K3s provider
at the host's bridge address:
- `tools/list` answered with all 30 tools, among them `exec-command`,
  `provision`, `remove`, `put-secret` and `teardown`;
- `opute.capability.kubernetes.list-clusters` ran, which runs `incus` as the
  provider's user (opute, an Incus administrator).

Any guest on a bridge, or any peer that can reach the host, could drive a
provider with the provider user's authority. Go v0.2.4 has the same exposure.

The Host Agent itself reaches its providers at `127.0.0.1` (the install
descriptors and every journey use `http://127.0.0.1:<port>/mcp`).

## What Changes

A provider executable listens on `127.0.0.1:<port>` only. The port, its
default per provider and `OPUTE_PROVIDER_PORT` are unchanged. A listen
failure names `127.0.0.1:<port>` instead of `:<port>`. Nothing else changes:
no new setting, tool, input or output.

## Options rejected

- **Require a bearer token on every provider request.** The agent already
  sends one, but a provider would need a new setting to know which token to
  accept: a larger contract change, and the network exposure remains.
- **Keep Go's listener and record the finding.** Leaves every host that runs
  a provider open to its guests.

## Impact

- `providers/mod.rs` (`listen`), shared by all four providers.
- The test `providers::tests::a_provider_listens_on_loopback_only`, which
  fails on rc7's listener (`evidence/m25/before-d53.log`).
- A provider meant to be reached from another host needs a local proxy; no
  journey, recipe or install descriptor does that.
- The 0.3.0 sign-off of 2026-11-21 (on `rust-0.3.0-rc7`) is reopened for the
  new candidate.
- Go v0.2.4 hosts keep the exposure until they move to a release with D53;
  firewalling the provider ports covers them meanwhile.
