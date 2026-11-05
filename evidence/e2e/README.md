# E2E dev VM and journey harness (e2e-0)

Since decision D27 the Rust Host Agent is judged against its spec, not
against Go. The goals e2e-1 to e2e-5 prove the spec on a production-like
host: this dev VM. This directory holds the VM tooling, the journey runner,
the verifier and the committed receipts.

| Path | What it is |
| --- | --- |
| `devvm/devvm.py` | lease, build, start, stop, restore, ssh, state |
| `devvm/guest/state.py` | the VM state document (the T2 snapshot plus the dev VM's roots and the `opute-e2e` project) |
| `run.py` | runs one journey once and writes its receipt |
| `verify.py` | re-derives a gate from the committed receipts |
| `journeys/` | journey definitions |
| `gates.json` | which run and journeys each gate requires, and their expected verdicts |
| `runs/<run>/<journey>/` | receipts (committed) |
| `tests/` | offline harness tests (`make e2e-test`, in CI) |
| `mcpwire.py` | the MCP/HTTP client, used on the host and copied into the guest |
| `guest-tools/e2etool.py` | guest helpers: transient agents under `/tmp/e2e-agents`, read-only database probes, the documented IDE config, a slow endpoint |
| `clients/hostagentclient/` | the e2e-1 client on the published Go library `pkg/hostagentclient` (built offline by `build.sh`) |
| `journeys/make_e2e1.py` | writes the e2e-1 journeys, with their hash pins |

## The VM

One qemu/KVM VM, run as the user. It needs no root and no sudo on the host,
and changes nothing on the host outside `.parity/` in the main checkout.
- **Resources:** 8 vCPU, 16 GiB of memory and a 100 GiB disk.
- **Disk:** `.parity/e2e/devvm/base.qcow2` is a new overlay on the pinned
  `golden-incus`. The golden is checked against its record and never
  written.
- **Ports:** SSH is forwarded to `127.0.0.1:42222` and the agent's MCP port
  to `127.0.0.1:43014`. Nothing else is reachable.
- **Incus:** storage pool `opute-e2e` (btrfs, 60 GiB), bridge `opute-e2e0`
  (10.0.120.1/24) and project `opute-e2e`, whose default profile uses both.
  Every E2E guest goes in that project and is named `e2e-*`.
- **The agent:** the Rust release binary from the local release set, after
  checking it against SHA256SUMS. It is installed production-style:
  - the user `opute`;
  - Go's recorded `deploy/opute-bootstrap-mcp.service` as the system unit,
    and Go's `deploy/opute-host-agent-sudoers`. Both are checked against
    `baseline/inventory/distribution.json`;
  - `/etc/opute/host-agent.env` (root:opute, 0640), loaded through the
    drop-in `e2e.conf`. It holds the test identity `e2e-devvm-agent` and a
    generated test token.
- **Go:** the published Go v0.2.4 release set
  (`.parity/releases/go-published-v0.2.4`, refused unless its SHA256SUMS
  hash is `baseline/source-lock.json`'s) is in
  `/opt/opute-e2e/releases/go/v0.2.4`. It is only the migration source for
  e2e-1 journey 8 and e2e-4, and is not installed.
- **The token:** it exists only in `.parity/e2e/devvm/token` (0600) and in
  the guest's env file. The runner redacts it from every record and refuses
  to write a receipt that carries it.

## The lease, and the T2 resource rule

The dev VM and the T2 twins share this host's memory:
- the dev VM takes 16 GiB;
- the T2 twins have a 20 GiB budget;
- the host has 30 GiB.

They therefore share one lease, `.parity/t2/dev-vm.lease.json`:
- **acquire:** `devvm.py lease acquire --holder NAME [--hours H]` (at most
  12 h). It refuses while any T2 twin is running, or while another holder
  has the lease;
- **effect on T2:** while the lease is held and unexpired, `Twin` in
  `tools/parity/parity/t2.py` refuses to start a twin;
- **enforcement:** every `devvm.py` command except `lease`, and `run.py`,
  refuse to run without the lease;
- **release:** `devvm.py lease release --holder NAME` once the VM is
  stopped.

## Build, run, restore

```bash
python3 evidence/e2e/devvm/devvm.py lease acquire --holder my-session
```

```bash
make e2e-devvm-build
```

`make e2e-devvm-build` is the one build command. It first builds the Rust
release set (`tools/release/build-artifacts.sh`, under `.parity/e2e/release`),
then the VM:
- it boots the new overlay and runs the provisioning recipe (`PROVISION` in
  `devvm.py`; each command is logged with its exit status);
- it reboots once and records `baseline.json`: the guest state, the agent's
  `/health`, the unit and the `tools/list` hash;
- it powers off and makes `base.qcow2` read-only. That file is the snapshot.

`build.json` records the golden, the release and deploy hashes, the recipe
log, the baseline hash and the timings. It holds no secret. A built VM is
never rebuilt in place.

```bash
python3 evidence/e2e/devvm/devvm.py start
```

`start` boots `run.qcow2`, an overlay of the snapshot. Its companions:
- `devvm.py status`;
- `devvm.py ssh -- CMD`;
- `devvm.py stop`.

```bash
python3 evidence/e2e/devvm/devvm.py restore --check
```

`restore` discards `run.qcow2`, recreates it from the snapshot and boots
it. With `--check`, it compares the restored state, `/health` and
`tools/list` with the baseline.

## Journeys

A journey declares:
- **inputs:** what it starts from;
- **requirements:** the spec requirements it checks. Each has a pass
  condition: assertions over recorded steps. A requirement with no
  assertion is refused;
- **effects:** the state-diff paths it may change (`allow`), and what may
  remain after cleanup (`persist`);
- **steps:**
  - `mcp`: a JSON-RPC request to the forwarded agent, to a guest port
    (`port`), or to a self-check stub. It accepts `capture`, `until`
    polling, `auth` (`token`, `none`, `var:`, `secret:` or `literal:`),
    `meta` and `modern: false` for a legacy client;
  - `http`: a plain HTTP request, such as the OAuth endpoints;
  - `health`;
  - `guest`: a root shell command. JSON stdout is recorded as `json`, and
    `schemaResults` validates a client's reported structured results;
  - `copy`: a hash-pinned `inputs.files` entry into the VM;
  - `restore`.

  `${NAME}` expands a captured value, `${SECRET:name}` a per-run random
  secret, and `${TOKEN}` the agent's token. All three are redacted from
  every record;
- **cleanup:** the steps that undo it;
- **maxMinutes:** at most 30.

```bash
python3 evidence/e2e/run.py JOURNEY --run RUN_ID
```

Each journey runs once. The runner refuses an existing receipt directory.
It records:
- the transcript;
- the VM state before, after and after cleanup;
- a result per requirement;
- the schema validation of every MCP response. The JSON-RPC envelope is
  checked, and a `tools/call` result is checked against the `outputSchema`
  published by the same session's `tools/list`. An unsupported schema
  keyword fails closed;
- the effects diff: changes outside `allow` fail the journey, and residue
  after cleanup outside `persist` fails it too;
- the timings.

A requirement whose outcome depends on an open owner question
(`openspec/changes/rust-spec-authority/questions.md`) names it in
`openQuestion`. Its assertions are recorded as observations, and its outcome
is `unverified`, never a pass or a fail. Every journey starts from the
snapshot, through a checked restore in preflight.

A journey fails on any of: `pass-condition`, `schema`, `undeclared-effect`,
`cleanup-residue`, `harness-error` or `time-limit`.

```bash
make e2e-verify E2E_GATE=e2e-0
```

`verify.py` re-derives a gate from the committed receipts. It checks every
file against `receipt.json` and the journey against the committed one. It
recomputes `results.json` from the transcript and the states, and requires
the verdict `gates.json` expects. A missing, altered or token-bearing
receipt fails. The gates e2e-1 to e2e-5 stay blocked until their goals
record runs.

## The e2e-0 self-checks

The run `e2e-0-r4` proves the harness can pass and fail (r3 ran on the
first harness; r4 reran it after e2e-1 extended the runner):

| Journey | Expected |
| --- | --- |
| `selfcheck.noop` (authenticated read-only first call) | pass |
| `selfcheck.declared-effect` | pass |
| `selfcheck.undeclared-effect` | fail: `undeclared-effect` |
| `selfcheck.broken-pass` | fail: `pass-condition` |
| `selfcheck.schema-violation` (a loopback stub declared as input) | fail: `schema` |
| `selfcheck.restore` (a change, then restore) | pass, with the restored state equal to the baseline |
| `selfcheck.step-kinds` (copy, secrets, http, guest-port mcp, polling, captures) | pass |

## Rules

These come from the e2e goals:
- **Authority:** specs are the authority.
- **Each kind of gap has one remedy:**

  | Gap | Remedy |
  | --- | --- |
  | Rust spec violation | fix it with a failing-first test |
  | Unclear spec | an OpenSpec proposal and an owner question |
  | Harness defect | fix it, and retire the bad run under `.parity/retired/` with its cause |
  | Missing runner or account | an owned gap |

- **Runs:** each journey runs once, and no step runs over 30 minutes.
- **Credentials:** test credentials only.
- **Out of bounds:** no production host, Platform, tunnel, DNS, registry or
  identity.
