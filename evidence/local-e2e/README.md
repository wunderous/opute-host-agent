# Local E2E validation, 2026-10-02

All three requested local flows pass on Ubuntu-26.04 under WSL2:

| Flow | Evidence |
| --- | --- |
| Start the agent, authenticate, discover and list its catalog | Official Go MCP SDK handshake and catalog suites pass against Go and Rust. Their `/bin/true` controls fail. |
| Invoke read-only tools and observe exact host identity and capacity | All 67 differential scenarios pass across five repetitions, including host information, capacity, VM inventory, modes and catalog revisions. |
| Reject invalid or forbidden calls, then continue reading and restart | D11 passes in standalone, mutations-enabled standalone and platform modes. Every rejection preserves complete logical SQLite state and leaves an empty command trace. Existing accepted history survives; authenticated reads and restart succeed. The official SDK session also reads again after schema and mutation rejection. |

## Results

- Rust formatting, clippy with warnings denied, and 75 unit tests pass.
- All 73 Python harness tests pass; catalog source matches the pinned Go tree.
- Go self-comparison: 67/67 scenarios across 20 repetitions.
- Go/Rust comparison: 67/67 scenarios across five repetitions.
- Go canaries: 10/10 caught. Rust canaries: 21/21 caught.
- D8 OAuth contracts: 24/24. D10 read-only gate: 8/8. D11 rejection: 3/3.
- OpenSpec structural validation: 5/5.
- [M1](../current/verify-m1.json), [M2](../current/verify-m2.json) and
  [M3](../current/verify-m3.json) gates pass, with no stale evidence.
- [Cutover](../current/verify-cutover.json) fails solely because 80 owned
  inventory gaps remain. These local results do not authorize production
  cutover or Go retirement.

## Fixes and the approved divergence

Rust omitted Go's user-service command environment setup. Restoring the user
bus for `systemctl --user` and `systemd-run --user` fixes the enforcement
observation mismatch. Explicit environment values and unrelated commands
remain unchanged; regression tests execute temporary child programs.

The live `system.tasks.current` kernel counter is sampled separately by Go
and Rust. Typed masks require its presence and a non-negative integer;
enforcement verdicts, configured limits and catalog data compare exactly.
The reference compiler is pinned to Go 1.25.4, preserving the selected
baseline's HTTP redirect semantics.

The owner approved [D11](../../openspec/changes/reject-without-audit-writes/proposal.md):
rejected calls produce zero audit writes. Startup provisioning, accepted
calls and D8 OAuth issuance auditing are outside this decision. The pinned
Go implementation is unchanged. Differential comparison declares only the
rejected invocation count difference; the Rust contract checks schema and
all row contents, including same-count updates. Canary K21 injects a rejected
invocation audit row and fails in all three modes.

## Provenance and isolation

- Go source commit: `ace7013df17528fee1bed13a1d70a132d6c5eb9b`.
- Go source tree: `ac17cfb298f789095e7a3af2219837e3490f798a`.
- Go binary SHA-256: `71924b62bf09a659489327f6ffe66057f239596fa476ad36f02fc0b63fb62215`.
- Rust binary SHA-256: `6524dc94464a7a897f5b2e3eebaad6b159ce1c7de330692166dbd5bb591e9252`.
- Rust source is repository HEAD `8fa32ae1e59892e8b67efea7e650bcc3f909e660`
  plus the uncommitted changes accompanying this report.
- Rust 1.93.1, Go 1.25.4 and Python 3.14.4.

Processes use temporary state, credentials and Incus recording fixtures.
Real-client overlays also isolate HOME, config, instance root and identity.
Host observations read the local kernel and existing user manager; no shared
Incus, Kubernetes, tunnel or deployment topology was changed.

Canonical results: [comparison](../current/go-vs-rust/summary.json),
[self-comparison](../current/go-vs-go/summary.json),
[official clients](../current/go-oracles.json),
[D11](../current/contracts/reject-without-audit-writes/summary.json),
[Go canaries](../current/canaries.json), and
[Rust canaries](../current/rust-canaries.json). Summaries retain binary,
scenario and raw-observation hashes. The corrected rejection source anchor
was refreshed with a fresh 20-repeat self-comparison before verification;
the rest of the self-comparison evidence was preserved.

Reproduce using `make parity-m3`, `make spec-validate`, and
`make parity-verify-cutover` (the last command intentionally fails until the
remaining migration inventory is covered). Clippy must be available on PATH.
