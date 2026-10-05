# Native local validation, 2026-10-05

**Native validation complete. No cutover claim.** This run checks M1–M5 on
native Ubuntu 26.04.1 x64, kernel 7.0.0-38, with isolated identities,
credentials, ports, state directories and recording fixtures.
M6 is assumed passed by owner instruction; the supplied result was
102/102 Go/Rust scenarios, five repetitions. Historical local M6 observations remain visible. Their fixture gaps
were corrected without changing the M6 runtime or rewriting old evidence.

## Requirements and evidence

| Requirement | Authoritative evidence and current state |
| --- | --- |
| Pinned Go authority | `baseline/source-lock.json`; pinned source/tree and Go binary verified. Catalog generation check passes. Final capture: 15 files, 76 explicitly owned remaining gaps, zero unowned gaps and zero runtime-observation gaps. |
| Rust structure/build | Formatting, clippy and release build passed; 174 unit tests passed, two dedicated fixture tests intentionally ignored by the unit command and exercised by their drivers. |
| Harness can fail | 140 harness tests pass, including independent process port leases, required outcomes, cancellation snapshots and shape exemption provenance. Fresh Go canaries: 14/14 caught, no waivers. Rust canaries: 24/24 caught, including the corrected K23 patch anchor. |
| M1 CLI/config/lifecycle | Fresh Go self-comparison and differential each pass all 102 cases (20 and five repeats respectively); startup oracle and provenance-bound M1 gate pass. |
| M2 HTTP/auth/MCP | Versioned wire matrix, independent modern client, D8 OAuth contract passes 24/24; provenance-bound M2 gate passes. |
| M3 catalog/read-only | Generated descriptors match pinned source. Fresh client oracles pass for both binaries; full matrix and provenance-bound M3 gate pass. Independent native host observations are retained below. |
| M4 admission/tasks | Native saturation and cancellation harness gaps corrected; both scenarios passed five Go/Rust and 20 Go/Go diagnostic repeats. D10–D12 contracts pass (8/8, 3/3, 3/3); fresh provenance-bound M4 gate passes. |
| M5 durable state/redaction | Fresh full eligible shape matrix, bidirectional cold reads (18/18 passed), 200 crash seeds (200/200 passed), secret sweep, unknown projections (four cases, no failures), migrations (6/6) and D13 contract (4/4) pass. Final shape: 495/495 rows pass; provenance-bound M5 gate passes. |
| M6 recipes/executor | Accepted under explicit owner assumption. Historical fixture failures are retained; the corrected restart barrier passes 20 self and five differential repetitions. See `evidence/m6/README.md`. |
| Real external effects | This host has no Incus installation and no configured workload slice enforcement. Native inventory returns a truthful tool error; capacity reports enforcement unknown. Disposable real-runtime tests still need their owning typed capability and runtime lease; fixture parity is not T2 evidence. |
| Completion audit | Gates and raw evidence must be rechecked against final hashes; all remaining external blockers must be reported. No gate is inferred from a summary or unit tests alone. |

## Native observations and machine configuration

[`native-admission.json`](native-admission.json) records real native admission
with both binaries: default policy refuses normal work when enforcement is
unknown; explicit advisory policy inspects a test-owned file successfully,
checks its content hash, and leaves it unchanged. Both processes read the
configured 24 GiB memory ceiling. Reproduce with
`python3 tools/parity/native_readonly.py --go .parity/bin/go-reference
--rust target/release/opute-host-agent --env-file ~/.config/opute/host-agent.env
--memory-bytes 25769803776 --out <report.json>` (one command line).

[`native-read-only.json`](native-read-only.json) records separate Go and
Rust processes reading this machine without Incus shims: authentication,
discovery, catalog, host identity, platform and capacity. Wrong tokens are
rejected, explicit identity is preserved, and platform fields agree on
native Linux. Incus inventory fails truthfully because Incus is absent.

[`machine-memory-capacity.json`](machine-memory-capacity.json) records both
agents reporting an effective memory limit of 25769803776 bytes (24 GiB)
when started with the user's `~/.config/opute/host-agent.env`. The file is
mode 0600. No installed non-test agent service was found; the explicit
`--env-file` argument loads it. Isolated admission fixtures configure
64 GiB to isolate slot semantics, subject to actual physical/cgroup caps.
The pinned product default is unchanged.

[`native-observations-index.json`](native-observations-index.json) hashes
these independent observations. Full differential, contract, oracle,
canary and M5 bundles retain their own driver and raw-observation hashes.
All final bundles were inspected and accepted by their provenance-bound verifiers.

The baseline remains Go commit `ace7013df17528fee1bed13a1d70a132d6c5eb9b`
and tree `ac17cfb298f789095e7a3af2219837e3490f798a`; there is no contract
rebase. Shared Incus, Kubernetes, tunnel and deployment state are unchanged.
Once this validation goal is complete, the owner authorized a new goal for
implementation and validation of M7–M13, with cutover and retirement decisions
remaining with their owners.

The final verifier reports `verify-m0.json` through `verify-m5.json` all pass. This completes the fixture and provenance gates. [Existing Incus capability path](incus-capability-path.md) records the owner-directed real-runtime bootstrap route; dedicated VM validation completed successfully.

## Real-runtime supplement

[`t2/summary.json`](t2/summary.json) records 104 native assertions and 28 exact structured Go/Rust comparisons across independently leased QEMU clones, before and after agent restart. The pinned Go agent installed Incus and QEMU, reconciled real systemd workload enforcement, and provisioned a test-owned container through its existing typed capabilities. Both implementations then passed real inventory, binding, file-hash, saturation, control-plane and release checks without executable shims. Three VMs shut down cleanly; leases were released, no QEMU processes remained, and disposable overlays were removed. The verified golden image remains in ignored test storage for future isolated runs.

The top-level native host remains unchanged by Incus provisioning. ARM, clean published-artifact installation, Platform staging, provider-generation lifecycle and unimplemented domain mutations belong to subsequent milestones. Rust Incus installation and host-service adoption remain explicitly owned M8 gaps; they are not claimed as implemented here.
