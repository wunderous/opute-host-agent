# M8e entry: naming, reachability and credential fields (D24)

## Entry (2026-10-19)

PR wunderous/opute-host-agent#18 (M8d) is not merged. M8e branches from
`claude/m8d-oci-domain` at `e311f144`, the M8d round 4 head, and builds on
it. The CI parity job of #18 fails for pre-existing reasons (provider
binaries are not built in CI), reported there and not chased here.

## Naming: kept as Go

The fourteen tools keep Go's names. `postgresql`, `sqlite` and `sql` name
database engines and a protocol, as `oci` names an open format, so no
neutral name applies. No tool name or selector value publishes a vendor
name, and no owner question was needed.

Go's result fields `cnpgSecretName`, `credentialState: "cnpg-owned"` and
`postgresVersion: "cloudnativepg"` name the operator Go installs. They are
Go's wire output and stay byte for byte.

## Reachability: Go's standalone tools only refuse

`install_postgresql`, `get_postgresql_status`, `delete_postgresql` and
`run_sql` take their VM from the call's resource binding
(`vmNameFromBinding`), and their catalog entries declare no resource
argument. Go therefore never binds a VM. Every call refuses before it
reaches the provider:

- `install_postgresql`: "vmName is required"
- `run_sql`: "vmName and sql are required"
- the status and delete tools: "kubernetes provider instance name is
  required"

This is the same shape as `configure_service_domain` in M8c. Rust
reproduces the refusals: T1 `postgres-domain.standalone`, and the T2
rejected-call rerun. The T2 inventory lists the three mutating tools as
inert, with this reason.

The goal's T2 check that "data written by run_sql survives a reconcile"
therefore cannot use `run_sql`. The harness writes and reads the row with
its own client instead: psql in the primary, as the pod's postgres user.
Its bytes also reach PostgreSQL through the SQL connector and the relay,
checked by a loopback probe.

## Credential fields: D24 marks the relay token

- **Database password and URLs:** never stored. The password travels only
  as provider `stdin` (the pgpass file psql reads), and the consumer Secret
  manifest is applied through `stdin`. The trace keeps only their sha256.
  Neither Go nor Rust writes them to a row, log or file.
- **Relay state file:** holds the token's sha256 (`tokenHash`), never the
  token.
- **`relayToken`:** Go's catalog does not mark it `writeOnly`, in
  `release_postgresql_service_relay` or under `localRelay` in the three
  service tools. Go's schema-driven projection keeps it verbatim in
  `capability_invocations.arguments_json`; the first Rust port did the
  same.

### Owner decision (2026-10-19)

Asked "keep Go's behaviour" or "mark writeOnly", the owner chose **mark
writeOnly**. It is recorded as D24 (OpenSpec change
`write-only-relay-token`):

- Rust publishes the four paths `writeOnly`
  (`crates/host-agent/catalog/write-only.json`), so every durable row
  carries the redaction marker in the token's place.
- `D19.neutral-host-tool-names` removes exactly those marks on the Rust side
  for the comparison with Go.
- The secret sweep covers Go's 16 marked fields on both sides, and the 8
  D24 fields (four tools in each mode, top-level and nested) on Rust.

`ensure_local_llm_k3s_proxy` carries the same `localRelay.relayToken`. It is
out of scope here and left to the LLM domain's milestone.
