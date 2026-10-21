# M8e: postgres domain (evidence)

M8e ports the Go `internal/domain/postgres` operations to Rust as built-in
providers of five three-role services (D14):

- **sqlite databases:** `ensure_sqlite_database`,
  `get_sqlite_database_status`, `remove_sqlite_database`
- **postgresql service:** `reconcile_postgresql_service`,
  `get_postgresql_service_status`, `remove_postgresql_service`, with the
  service's storage admission
- **postgresql relay:** `release_postgresql_service_relay` and the
  authenticated relay's lifecycle, persisted and restored
- **sql connector:** `ensure_sql_connector`, `get_sql_connector_status`,
  `release_sql_connector`, and the host TCP relays
- **standalone postgresql:** `install_postgresql`, `get_postgresql_status`,
  `delete_postgresql`, `run_sql`

Every kubectl call reaches the cluster through the provider capability
`opute.capability.kubernetes.v1`, served by the pinned Go k3s provider, as
in M8c and M8d. The relay's loopback forward is an Incus proxy device on k1.
The tool names stay Go's ([NOTE.md](NOTE.md)).

Parity is proven at two tiers:

- **T1:** recording shims, 9 postgres-domain scenarios.
- **T2:** disposable twin VMs with a real CloudNativePG cluster in k3s, and
  real relays on the twin's loopback.

The Go authority is `wunderous/host-agents` `ace7013d` (tree `ac17cfb2`). The
Rust candidate is `4c516f55…`, built with `cargo build --release --locked`
from round head `6abe763d`. M8e branches from `claude/m8d-oci-domain`
(PR wunderous/opute-host-agent#18 is not merged).

**Result:** the m8e gate passes, and gates m0–m6 and m8a–m8d still pass.
The cutover gate fails as expected, on 17 owned inventory gaps and four
cutover evidence files that belong to later milestones. All 9
postgres-domain items and the three postgres T2 scenarios moved from
`unverified` to `pass` in both suites.

## Receipts

| What | Receipt |
| --- | --- |
| Validation receipt, recomputed from the raw captures by [`verify.py`](verify.py) | [validation.json](validation.json) |
| Round script (every command) and its raw logs and exit codes | [run.sh](run.sh), [checks/](checks/) |
| T1 Go/Go ×20 and Go/Rust ×5, all 314 manifest items | [go-vs-go](go-vs-go/summary.json), [go-vs-rust](go-vs-rust/summary.json) |
| T2 Go/Go ×20 and Go/Rust ×5, 30 scenarios (14 host, 5 incus, 5 cluster, 3 oci, 3 postgres) | [t2-go-vs-go](t2-go-vs-go/summary.json), [t2-go-vs-rust](t2-go-vs-rust/summary.json), goldens: [base](t2-golden.json), [incus](t2-golden-incus.json), [k3s](t2-golden-k3s.json), [oci](t2-golden-oci.json), [postgres](t2-golden-postgres.json) |
| Go `test/live` oracle inside the twins | [oracles-t2.json](oracles-t2.json) |
| Gate reports, rerun byte for byte by `verify.py` | [m0](verify-m0.json) [m1](verify-m1.json) [m2](verify-m2.json) [m3](verify-m3.json) [m4](verify-m4.json) [m5](verify-m5.json) [m6](verify-m6.json) [m8a](verify-m8a.json) [m8b](verify-m8b.json) [m8c](verify-m8c.json) [m8d](verify-m8d.json) [m8e](verify-m8e.json) [cutover](verify-cutover.json) |
| Contracts, canaries and client oracles | [contracts/](contracts/), [Go canaries](canaries.json), [Rust canaries](rust-canaries.json), [oracles](oracles.json), [go-sdk OAuth client](oauth-sdk-client/receipt.json) |
| M5 durable state, including the secret sweep (16 Go fields, 8 D24 fields) | [m5/](m5/) |
| D14 seam stage | [report](../seam/m8e/report.json) |
| Entry, reachability and the D24 decision | [NOTE.md](NOTE.md), [`write-only-relay-token`](../../openspec/changes/write-only-relay-token/proposal.md) |
| Inventory | [postgres domain](../../baseline/inventory/postgres-domain.json), [env](../../baseline/inventory/env.json), [t2](../../baseline/inventory/t2.json), [gaps](../../baseline/inventory/gaps.json) |

## What passed in round 3

All 31 steps exited 0. The tree was clean before and after, and the binaries
did not change during the round. Nothing else heavy ran on the host.

| Check | Result |
| --- | --- |
| fmt, Clippy `-D warnings`, workspace tests, locked release build | pass; 252 tests |
| `make spec-validate`, catalog source, harness tests | pass; 18 specs |
| Owning Go tests: `internal/domain/postgres` and `internal/tcprelay` ×5; core packages | pass; 190 owning passes, 0 failures |
| T1 Go/Go ×20 | 314/314 |
| T1 Go/Rust ×5 | 314/314 |
| T2 Go/Go ×20 | 30/30 |
| T2 Go/Rust ×5 | 30/30 |
| Go `test/live` oracle in a twin | Rust's per-test outcome equals Go's; negative control passes nothing |
| Go canaries / Rust canaries | 14/14 caught / 24/24 caught |
| Contracts D8, D10, D11, D12, D13 | 24/24, 8/8, 3/3, 3/3, 4/4 |
| M5 crash, shape, migration, cross-read, unknown projection | 200/200, 1555/1555, 6/6, 18/18, 0 failures |
| Secret sweep | 16 Go-marked fields on both sides, 8 D24 fields on Rust; 0 failures |
| Seam verifier | 115 compliant, 35 unported, **0 gaps**; dispatch canary red |

## T1

[`tools/parity/postgres_domain_scenarios.py`](../../tools/parity/postgres_domain_scenarios.py)
generates the matrix. `incus` is answered by the recording shim, so the
trace holds every provider argv and stdin hash on both sides. The manifests
the agent applies (the CNPG HelmChart and Cluster, the consumer Secret) and
the pgpass lines it feeds psql are compared by hash, byte for byte.

- **SQLite:** databases under an explicit root, the default root, a
  relative root and a root that is a file; names refused; removal only with
  confirm. The SQLite library stamps its own version in a file's header (Go
  links 3.53, Rust 3.46), so a database is compared by its schema, journal
  mode and file mode, never its bytes.
- **SQL connector:** reference counts, force release, Go's listener
  addresses (`::` for the wildcard, its errors for `::1:0` and port 70000),
  and Go's own output-schema refusal of `release_sql_connector`.
- **Service:** a ready estate (status, steady reconcile, consumer Secret and
  restarts), a fresh cluster (the ordered install and a missing database
  created), storage admission, argument refusals, and removal.
- **Relay:** persisted, restored after a restart on the domain's first call,
  released only with its token; its state file holds the token's sha256.
- **Standalone:** Go's refusals (see Reachability).

## T2 sandbox

The PostgreSQL golden (`7ae211bd…`) is an overlay of the k3s golden
(`481c5443…`):

- **Operator:** CloudNativePG chart `0.29.1` (sha256 `b53d3991…`), rendered
  by helm with Go's own values under the release name Go's HelmChart gives
  it. The golden's helm controller stays disabled, so Go's HelmChart is
  stored and compared, never fetched.
- **Images:** `cloudnative-pg:1.30.1` (`sha256:923c267e…`) and Go's operand
  `postgresql:16-minimal-trixie` (`sha256:f76ba026…`), pulled once by digest
  at build time and tagged as Go names them. A throwaway Cluster proved the
  operand starts with the image list unchanged before the golden was sealed.
- **k1** keeps the OCI golden's reserved address.

The agent serves with the relay directory and SQLite root under
`/home/ubuntu/.opute`, so both effects are in the twin's file snapshot. The
snapshot adds CNPG Clusters and their claims, records a Secret's
`last-applied-configuration` annotation by sha256 as it records its data,
and records a SQLite file by type and mode.

The three postgres scenarios:

- `service.lifecycle`: reconcile from nothing, then a row written by the
  harness's own psql in the primary. A status call with `localRelay` starts
  the relay; a loopback probe sends the token line and a PostgreSQL
  SSLRequest and gets `S`, and a wrong token is closed without an answer.
  The SQL connector then chains to the relay and answers `S` too. A second
  reconcile follows, and the row reads back. The relay is released only
  with its token, the connector is released, and removal retains, then
  deletes.
- `sqlite.lifecycle` (base golden): create, find, refuse and remove.
- `admission.rejected-no-effect`: reruns every postgres call that M4
  refuses, aimed at k1, the twin's loopback and its files. Its delta stays
  empty on both twins.

No shared Incus, Kubernetes, registry, tunnel or deployment state was
touched, and no real database.

## Reachability

Go's `install_postgresql`, `get_postgresql_status`, `delete_postgresql` and
`run_sql` take their VM from a resource binding that their catalog entries
never declare, so every call refuses before reaching the provider. Rust
reproduces each refusal (T1 `postgres-domain.standalone`, the T2 rejected
rerun), and the T2 inventory lists the three mutations inert. The data
check therefore uses the harness's own client, not `run_sql`
([NOTE.md](NOTE.md)).

## Normalizations and divergences

Every normalization is declared in a scenario or in
[divergences.json](../../tools/parity/divergences.json).

- **D24** (owner decision, `openspec/changes/write-only-relay-token`):
  Rust publishes `relayToken` `writeOnly` at the four paths listed in
  `crates/host-agent/catalog/write-only.json`. Every durable row then holds
  the redaction marker where Go keeps the token. The crate test
  `d24_marks_the_relay_token_write_only` checks the marks and the storage
  redaction. `D19.neutral-host-tool-names` removes exactly those marks for
  the comparison with Go, and every Rust catalog revision is re-pinned with
  its Go pair.
- **T2 masks** come from the scenario file: CNPG's generated credentials
  and certificates (Secret data, by sha256), owner reference uids, the proxy
  device's ClusterIP and loopback port, the relay state file that names
  that port, and a claim's deletion time.

## Defects fixed while proving parity

- **Rust:** the SQLite root's error names the existing non-directory
  component, as Go's `MkdirAll` does.
- **Harness:** each fix is committed separately.
  - a T1 tree records a SQLite file by type and mode
  - the T2 snapshot does the same, projects CNPG Clusters and claims, and
    hashes a Secret's last-applied annotation
  - the secret sweep builds nested cases and sweeps D24's fields on Rust
  - live PSI samples are `omitempty` wherever they are masked (round 1)
  - the relay probe reports a refused token as closed, however it ends
    (round 2)

## Rounds

Only round 3 counts. Rounds 1 and 2 were stopped and retired. Their failures
were recorded, never relabelled, and their raw logs are kept under
`.parity/retired/m8e-round*/`.

| Round | Outcome | Cause |
| --- | --- | --- |
| 1 | retired | T1 Go/Go, `incus-domain.standalone.status` #8. See below. |
| 2 | retired | T2 Go/Go, `t2.postgres-domain.service.lifecycle` #18. See below. |
| 3 | **pass** | |

**Round 1:** one Go side omitted `system.psi.io.fullAvg60`. Go omits a PSI
field whose live sample is zero. The M8b scenario masked the value but not
its presence, so the developer host's I/O pressure under the round's load
decided the outcome. It now carries host-read's `omitempty` rules, as does
`config.env.resource-policy`.

**Round 2:** the wrong-token probe printed `ConnectionResetError` on one Go
twin and `b''` on the other. The relay refuses the token line and closes
with the probe's SSLRequest possibly unread, so which one the client sees is
timing. Both now print `closed`. The scenario passed Go/Go ×20 alone before
round 3.

## Open, owned gaps

These come from [gaps.json](../../baseline/inventory/gaps.json): 17 gaps, 0
unowned. No M8e gap remains:

- `OPUTE_HOST_AGENT_RELAY_DIR` and `OPUTE_HOST_AGENT_SQLITE_ROOT` are
  covered.
- Every mutating postgres tool is covered at T2, except Go's three unbound
  standalone tools, which are inert.

| Owner | Gaps |
| --- | --- |
| M8a | 3: `compact_wsl_disk`, `shutdown_wsl`, `terminate_wsl_distribution`, which need a WSL2 runner |
| M8f | 2 |
| M10 | 1 |
| M8, environment reads for later domains | 11 |
