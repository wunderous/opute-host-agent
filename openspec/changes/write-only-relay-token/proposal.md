# Write-only PostgreSQL relay token

## Why

The M8e postgres domain authenticates its loopback relay with a caller's
`relayToken`. It travels in two places:

- the top-level field of `release_postgresql_service_relay`;
- `localRelay.relayToken` of `get_postgresql_service_status`,
  `reconcile_postgresql_service` and `remove_postgresql_service`.

The pinned Go catalog does not mark the field `writeOnly`. Go's
schema-driven projection therefore stores it verbatim in
`capability_invocations.arguments_json`, the durable audit row of every
call. The M8e T1 run found it there, in both Go and the faithful Rust port
(`evidence/m8e/NOTE.md`). The database password and the consumer URLs never
reach a row, because they travel only as provider `stdin`. The relay's
state file holds only the token's sha256.

**Decided (owner, 2026-10-19), recorded as D24.** Asked to keep Go's
behaviour or mark the token `writeOnly`, the owner chose to mark it.

## What Changes

- **Published schemas:** the Rust catalog publishes `relayToken`
  `writeOnly: true` at the four paths above, in every serving mode that
  publishes the tool. The marks are listed in
  `crates/host-agent/catalog/write-only.json`, applied where the D19 name
  map is applied. Go's own definitions, which give each Rust revision its
  Go pair, stay unmarked.
- **Storage:** the existing schema-driven projection redacts the field in
  every durable row: the invocation audit, the task snapshot's argument
  echo, plan documents and active-capability bindings. No new redaction
  code is needed.
- **Unchanged:** validation (`minLength: 32`), the relay's behaviour, its
  state file, results, delivery through `tasks/get`, tool names, effects
  and admission.
- **Catalog revisions:** every Rust catalog revision changes, and each is
  re-pinned with its Go pair, as D19 requires.

## Capabilities

### New Capabilities

- `write-only-relay-token`: the D24 marking and its evidence.

### Modified Capabilities

None. Go is unchanged.

## Impact

- **Divergence:** `D19.neutral-host-tool-names` reads `write-only.json`. On
  the Rust side only, it removes exactly the listed `writeOnly` marks from
  those tools' input schemas. Any other difference stays a diff. The
  durable rows' redacted value is the existing `D13` marker, compared the
  way D13 is.
- **Secret sweep:** Go is swept for Go's marked fields. Rust is swept for
  those and the D24 fields, top-level and nested. The sweep checks that
  Rust's catalog marks each D24 path, and that no canary reaches a row,
  file, log or response.
- **Out of scope:** `ensure_local_llm_k3s_proxy` carries the same
  `localRelay.relayToken`. It belongs to the LLM domain's milestone and
  its own decision.
