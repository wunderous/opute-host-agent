# M8e entry: naming, reachability and credential fields

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

## Credential fields: Go stores the relay token (owner question)

The goal asks that credential fields be marked and covered by the M5 secret
sweep, with none reaching a row unredacted. Go conflicts with that:

- **Database password and URLs:** never stored. The password travels only
  as provider `stdin` (the pgpass file psql reads), and the consumer Secret
  manifest is applied through `stdin`. The trace keeps only their sha256.
  Neither Go nor Rust writes them to a row, log or file.
- **Relay state file:** holds the token's sha256 (`tokenHash`), never the
  token.
- **`relayToken`:** Go's catalog does not mark it `writeOnly`, in
  `release_postgresql_service_relay` or under `localRelay` in the three
  service tools. Go's schema-driven projection therefore keeps it verbatim
  in `capability_invocations.arguments_json`. Rust does the same, so the
  M5 shape comparison passes.

The catalog-driven secret sweep exercises only `writeOnly` fields. It
finds none in this domain, so the field count stays at 16.

Hiding the token in rows means marking it `writeOnly`. That changes
`tools/list` from Go's, which is a contract change. AGENTS.md reserves such
a change for an owner decision with its own OpenSpec change, as D8, D13 and
D23 were. The proposed change, not implemented:

- **Change:** mark `relayToken` `writeOnly` in the four input schemas, as a
  declared divergence. Rust's stored projection would then redact it, as
  for every marked field.
- **Harness:** the secret sweep gains the nested-field fixture it now
  refuses (`nested catalog marking requires a nested field fixture`) for
  `localRelay.relayToken`.
- **Parity:** the D19 divergence maps the schema back for `tools/list`
  comparison.

Until the owner decides, Rust keeps Go's behaviour.
