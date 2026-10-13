# M8a: host domain (evidence)

M8a ports the Go `internal/domain/host` mutating operations to Rust. Each one
is a built-in provider of a three-role service (D14), and parity with the
pinned Go baseline is proven at two tiers:

- **T1:** recording shims prove argv, stdin and results.
- **T2:** disposable twin VMs prove real effects, cleanup and residue, and
  that refused calls leave the host unchanged.

The Go authority is `wunderous/host-agents` `ace7013d` (tree `ac17cfb2`), with
reference binary `71924b62…`. The Rust candidate is `b190b597…`, built with
`cargo build --release --locked` from round head `58c8492b`.

**Result:** the m8a gate passes, and gates m0–m6 still pass. The cutover gate
fails as expected, on 30 owned inventory gaps and four cutover evidence files
that belong to later milestones.

## Receipts

| What | Receipt |
| --- | --- |
| Validation receipt, recomputed from the raw captures by [`verify.py`](verify.py) | [validation.json](validation.json) |
| Round script (every command) and its raw logs and exit codes | [run.sh](run.sh), [checks/](checks/) |
| T1 Go/Go ×20 and Go/Rust ×5, all 256 manifest items | [go-vs-go](go-vs-go/summary.json), [go-vs-rust](go-vs-rust/summary.json) |
| T2 Go/Go ×20 and Go/Rust ×5, 14 host scenarios | [t2-go-vs-go](t2-go-vs-go/summary.json), [t2-go-vs-rust](t2-go-vs-rust/summary.json), [golden image](t2-golden.json) |
| Gate reports, rerun byte for byte by `verify.py` | [m0](verify-m0.json) [m1](verify-m1.json) [m2](verify-m2.json) [m3](verify-m3.json) [m4](verify-m4.json) [m5](verify-m5.json) [m6](verify-m6.json) [m8a](verify-m8a.json) [cutover](verify-cutover.json) |
| Contracts, canaries and client oracles | [contracts/](contracts/), [Go canaries](canaries.json), [Rust canaries](rust-canaries.json), [oracles](oracles.json), [go-sdk OAuth client](oauth-sdk-client/receipt.json) |
| M5 durable state, including the 16-field secret sweep | [m5/](m5/) |
| D14 seam stage | [report](../seam/m8a/report.json), [receipt](../seam/m8a-validation.json) |
| Inventory | [effects](../../baseline/inventory/effects.json), [t2](../../baseline/inventory/t2.json), [gaps](../../baseline/inventory/gaps.json) |

## What passed in round 7

All 30 steps exited 0. The tree was clean before and after, and the binaries
did not change during the round.

| Check | Result |
| --- | --- |
| fmt, Clippy `-D warnings`, workspace tests, locked release build | pass; 233 tests |
| `make spec-validate`, catalog source, harness tests | pass |
| Owning Go tests: host domain, `hostruntime` and `resource` ×5; core packages | pass; 580 host passes |
| T1 Go/Go ×20 | 256/256 |
| T1 Go/Rust ×5 | 256/256 |
| T2 Go/Go ×20 | 14/14 |
| T2 Go/Rust ×5 | 14/14 |
| Go canaries / Rust canaries | 14/14 caught / 24/24 caught |
| Contracts D8, D10, D11, D12, D13 | 24/24, 8/8, 3/3, 3/3, 4/4 |
| M5 crash, shape, migration, cross-read, unknown projection | 200/200, 1265/1265, 6/6, 18/18, 0 failures |
| Secret sweep | 16 marked fields, 0 failures |
| Seam verifier | 56 compliant, 94 unported, **0 gaps**; dispatch canary red (`list_operations`) |

## T2 sandbox

`tools/parity/parity/t2.py` boots two KVM twins from one golden qcow2 for each
(scenario, iteration) pair. The golden is Ubuntu noble `20260926` with every
timer disabled. The agent runs as `ubuntu` in a transient unit. Each twin is
snapshotted before and after each step, recording:

- file trees (path, mode, owner, sha256)
- systemd units and `systemctl show`
- nftables and iptables state
- users and groups, and linger

The two deltas are then compared. A scenario that declares `expectEmpty`
requires an empty delta, not just equal ones. Each iteration keeps its raw
receipts (`a{i}`, `b{i}`, `diff{i}`), hashed in the summary.
`t2_verify.check` re-derives every verdict from those receipts.

The 14 scenarios cover these operations, each run to create, mutate, read and
delete where the operation allows:

- user and system services
- workload units
- the resource policy (user and system agent units)
- connection restart
- the public MCP tunnel and quick tunnel
- host tools (apt)
- Incus stack install and uninstall, in the owned sandbox only
- archives, artifacts and the agent build
- bridge recovery
- the no-effect Linux tools

`admission.rejected-no-effect` reruns every host call that M4 refuses: 23
gated tools, a stale catalog revision and 12 binding variants. Its delta
stays empty on both twins.

No shared Incus, Kubernetes, tunnel or deployment state was touched. Tunnels
use the TLS stub inside the twin.

## Normalizations and divergences

Every normalization is declared in a scenario or in
[divergences.json](../../tools/parity/divergences.json). None was added to
make a Go/Rust difference disappear without a cause.

- **D19** (owner decision, `openspec/changes/neutral-host-tool-names`): Rust
  publishes neutral names for the eight mapped host tools and keeps the Go
  names as hidden aliases. That gives it different catalog revisions. The
  rule rewrites the Rust side back to Go's spelling, using the one name map
  and the revision pairs pinned in the manifest.
  - From this round it is also applied to the durable rows in M5 shape and to
    the projected catalog in the secret sweep. Those rows record the same
    names and revisions, and the rule is recorded there only when it rewrites
    something.
  - A 14th provider-fixture revision pair was pinned: the rival catalog after
    a family replacement, observed one-to-one in all five iterations.
- **D13:** Rust stores some unschematized arguments as `[redacted]`. M5 shape
  now aligns Rust's audit rows with the Go rows they match before D13's
  positional drop. A row that matches nothing stays where it is and still
  diffs.
- **Embedded numbers:** a port substituted inside a JSON column now parses
  back as that side's own number, as it does on the wire, instead of falling
  back to raw-text comparison.
- **T2:** these masks come from the scenario file:
  - the supervisor user-bus race and workload stderr accounting
  - Go's temporary-file suffix, Incus keys and certificates, and the Incus
    CLI's random example image
  - live metrics and the reservations hash
  - the systemd-run invocation id, which the driver normalizes
  - the coordinator ledger, which is excluded under `expectEmpty`

## Defects fixed while porting

- **Rust:** a managed temporary file that fails to open now reports the name
  it tried, as Go's `os.CreateTemp` does (`goerr::create_temp`).
- **Rust:** host-internal capabilities (`agent_shell`, `exec_command`,
  `ensure_host_firewall_rule`) now get an invocation audit row, as Go records
  every registered capability. M5 shape found the missing rows.
- **Harness:** each fix is committed separately.
  - the K16 canary patch target
  - D19 in the durable comparisons
  - leased ports for real Go providers (port-0 race)
  - letting `systemd-fsckd` leave before a twin is used
  - refusing to start a round without room in `TMPDIR`

## Rounds

Only round 7 counts. Earlier rounds were stopped or retired. Their failures
were recorded, never relabelled, and each cause was fixed before a full rerun
from a clean tree.

| Round | Outcome | Cause |
| --- | --- | --- |
| 1 | stopped | pinned Go toolchain not on `PATH` |
| 2 | stopped | PSI `someAvg10` presence race (Go `omitempty`) in `provider-real.k3s` |
| 3 | stopped | K16 canary target stale; D19 missing from the durable comparisons; Rust audit rows missing |
| 4 | retired | `systemd-fsckd` idle exit inside T2 scenarios (Go/Go and Go/Rust) |
| 5 | retired | `/tmp` tmpfs inodes exhausted by leaked sandboxes (shape, seam) |
| 6 | retired | real-provider port-0 race: T1 Go/Go 255/256 |
| 7 | **pass** | |

## Open, owned gaps

These come from [gaps.json](../../baseline/inventory/gaps.json): 30 gaps, 0
unowned.

- **M8a, `unverified`: `compact_wsl_disk`, `shutdown_wsl`,
  `terminate_wsl_distribution`.** They need a WSL2 runner: their effects act
  on the Windows host. They are T1-shim proven.
- **M8b:** `exec_command` and `run_instance_command` at T2, which need nested
  Incus. They are T1 proven.
- **Command effect tiers:**
  - T2-sandbox: `apt-cache`, `apt-get`, `bash`, `curl`, `dpkg`, `dpkg-query`,
    `install`, `rm`, `sudo`, `systemctl`, `usermod`.
  - `kill` stays with M8f.
  - `OPUTE_HOST_AGENT_ELEVATED` is covered (firewall elevated path,
    fail-closed).
- **Other domains:** M8 environment reads for other domains (19), plus M8b–M8f
  and M10 items.

Note for M8b: `reset_incus_stack` is an incus-domain tool and is not in
D19's eight-tool host map. M8b's naming change has to cover it.
