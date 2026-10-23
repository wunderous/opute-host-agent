# M8f entry: naming, reachability, Go quirks and credential fields

## Entry (2026-10-21)

M8f branches from `main` after PR wunderous/opute-host-agent#19 (M8e)
merged. The CI parity job fails on `main` for pre-existing reasons (provider
binaries are not built in CI). That is reported, not chased here.

## Naming: kept as Go

The fifteen tools keep Go's names. `ollama`, `llama-cpp` and `openai` appear
as runtime selector values and in two tool names
(`probe_openai_compatible_server`, which names a wire protocol, and the
`local_llm` tools). They are Go's published contract. Renaming them would be
a contract change, which needs its own OpenSpec change and owner decision.

## Reachability: Go's cluster proxy only refuses

`ensure_local_llm_k3s_proxy` and `remove_local_llm_k3s_proxy` take their VM
from the call's resource binding (`vmNameFromBinding`). Their catalog
entries declare no resource argument, so Go never binds one, and every call
refuses with `vmName is invalid` before it renders or applies anything. This
is the same shape as `configure_service_domain` (M8c) and Go's standalone
PostgreSQL tools (M8e). Rust reproduces the refusal in T1
`llm-domain.cluster-proxy` and in the T2 rejected-call rerun. The T2
inventory lists both tools as inert.

## Go behaviour reproduced as found

Each of these is Go's wire or file output, so Rust matches it byte for byte.
They are findings, not changes:

- **Ollama unit, models directory:** `renderOllamaSystemdUnit` appends to
  `lines[:len-3]` in place. When `OLLAMA_MODELS` is set, that overwrites the
  blank line before `[Install]`, and the `Environment=OLLAMA_MODELS=` line
  appears twice, around the context line. The crate test
  `ollama_unit_keeps_gos_slice_aliasing` pins it, and T1 `llm-domain.ollama.unit`
  compares the file.
- **A failed `ollama pull` is not an error:** the shared host command runner
  never returns one. The install goes on to the probe and fails there,
  naming the probe's error.
- **`systemd --user` is always "available":** for the same reason,
  `check_local_llm_prerequisites` reports `systemdUserAvailable: true`
  whatever `systemctl --user show-environment` exits with.
- **The serving launch's own error is dropped:** the agent shell's error
  does not reach `reconcile_serving_assignment`. A supervisor that never
  shows a main PID still reports `started: true`.
- **Map order:** Go ranges over maps for llama manifest validation, serving
  identifiers and the serving environment exports. Which field an error
  names, and the order of the exports, are random when more than one
  applies. Scenarios give at most one invalid field and one variable.

## Credential fields: a finding, no decision

Go's catalog does not mark the relay's `incomingToken`, `upstreamToken` or
`relayToken`, or the cluster proxy's `relayToken` and `bearerKey`, as
`writeOnly`. Of the llm tools' credentials, only
`probe_openai_compatible_server.bearerToken` is marked. As a result:

- Go's schema-driven projection keeps those tokens verbatim in
  `capability_invocations.arguments_json`.
- The relay's persisted state file
  (`<relay dir>/<sessionId>.json`, mode 0600) holds the request arguments
  with Go's field names, tokens in plaintext. This is how a relay is
  restored after a restart.

Rust does the same. M8e's precedent (D24) was an owner decision for the
PostgreSQL relay token. No such decision exists for these fields, and
marking them would change the published schema. This round reproduces Go and
records the finding for the owner. A `writeOnly` mark, and with it the
storage redaction, would follow D24's shape: an OpenSpec change, a D19
comparison rule, and secret-sweep coverage. It would not change the
relay's state file, which needs the token to restore the relay.
