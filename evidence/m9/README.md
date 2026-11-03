# M9: the provider executables in Rust (evidence)

D26 (owner, 2026-11-02) ports four Go provider executables to Rust:
hostos, cloudflare, tailscale and k3s. ollama stays the Go reference and
leaves the release set (D15).

Go stays the authority for each provider. A port is compared with the
pinned Go executable at its MCP boundary, and under one Rust core in the
scenarios that start it. Every suite the gates re-derive also runs again,
on the final Rust core. That core is built with the provider ports: the
shared library split and the provider client's proxy rule changed it.

Nothing was published, no tag was pushed, and no shared Incus, Kubernetes,
registry, tunnel or deployment state was touched. No real Cloudflare or
Tailscale account was reached:
- the API corpora run against loopback fakes (`tools/parity/provider-wire/`);
- every other host goes to a dead proxy;
- the k3s corpus drives a scripted K3s guest behind fake `incus` and `sudo`.

**Pins**

- **Go authority:** `wunderous/host-agents` v0.2.4, `c9f9e775`.
- **Go reference:** `.parity/bin/go-reference`, `2e2b5486…`, the same
  binary as D25. Go/Go T2 is recaptured here in D25's eight shards (×3):
  M9's T2 provider swap changed the T2 harness hash, so the verifier
  refuses D25's shards as stale.
- **Rust candidate:** `dist/host-agent-linux-x64`, `d55314ae…`. It is built
  by `tools/release/build-artifacts.sh` with cargo and zig only, with no Go
  toolchain and no Go tree.
- **Executables:** both sets are listed in `checks/providers.go.sha256` and
  `checks/providers.rust.sha256`.
  - Go: built by `build-go-providers.sh` from the pinned tree.
  - Rust: the shipped k3s, cloudflare and tailscale come from `dist/`;
    hostos comes from the same build.
- **Script:** [run.sh](run.sh), in four phases: `round`, `providers`, `soak`
  and `gates`.

**Result**

- **Gates:** m0–m6, m8a–m8f, m10, m12a, m12b, d25, m9 and the four
  per-provider gates (`m9-hostos`, `m9-cloudflare`, `m9-tailscale` and
  `m9-k3s`) pass.
- **Seam:** the verifier reports no gap (130 compliant, 20 unported, as in
  D25).
- **Cutover:** the gate fails, as expected, on two things:
  - 13 owned inventory gaps;
  - the four M13 cutover evidence files.

## Rounds

| Round | Head | Result | Notes |
| --- | --- | --- | --- |
| 1 | `1f574ef0` | Retired (`.parity/retired/m9-round-1`) | `cargo fmt --check` failed: the provider ports were never formatted. Fixed in `6d718546` |
| 2 | `6d718546` | Retired (`m9-round-2`) | The Go release set was copied from the D25 worktree. Go records its build directory in its binaries, so the rebuild could not match. It was rebuilt in this checkout |
| 3 round + providers | `6d718546` | Every step passes | |
| soak 1 | `6d718546` | Retired (`m9-soak-1`) | Growth failed on one sample taken during the first SIGTERM restart. The growth check now skips restart windows (`171a7be5`, owner decision 2026-11-03), and the D25 and M12b evaluations were re-derived under it (`cc1c44e0`). Thresholds unchanged |
| soak 2 | `cc1c44e0` | Every check passes | 15 minutes, seed 1204 |
| gates 1 | after `32215223` | Retired (`m9-gates-1`) | The four per-provider m9 gates passed. Every T2 gate refused D25's Go/Go shards as stale: M9's T2 provider swap changed the T2 harness hash |
| t2-go-go | after `32215223` | All eight Go/Go shards pass (×3) | host, incus, serving-llm, cluster-a, cluster-b, oci-a, oci-b, postgres |
| gates 2 | after the above | Every gate passes except cutover, as expected | |

## Provider evidence

| Provider | Wire corpus, Rust against Go | Provider swap | T2 swap | Released | Gate |
| --- | --- | --- | --- | --- | --- |
| hostos | 270/270 steps equal | provider-real.hostos passes | no T2 scenario starts it | no | m9-hostos |
| cloudflare | 602/602, plus the API corpus 508/508 with every fake API request equal | provider-real.cloudflare passes | no T2 scenario starts it | yes | m9-cloudflare |
| tailscale | 488/488, plus the live corpus 444/444 | provider-real.tailscale passes | no T2 scenario starts it | yes | m9-tailscale |
| k3s | 808/808 | provider-real.k3s and .restart pass | cluster, oci and postgres goldens pass | yes | m9-k3s |

Details behind the table:

- **Go self-check:** each corpus is recorded twice from Go, and the two
  recordings are equal.
- **Request order:** a step whose fake requests differ only in order passes
  only where the manifest declares that Go ranges over a map there, and
  Go/Go shows the same reorder. Two places are declared:
  - tailscale's teardown finalize;
  - k3s's `inspect-guest-storage`.
- **Provider swap:** both sides run the Rust core; the left starts Go's
  providers and the right starts Rust's.
- **T2 swap:** in the twins whose goldens carry k3s, Go's k3s runs on the
  left and Rust's on the right.
- **No black-box Go tests:** no Go test drives a provider executable (the
  tests under `plugins/*/cmd/*` run in-package). The corpora and the ported
  unit tests stand in for them.

## Recorded, not changed (Go is the authority)

- **cloudflare:**
  - Go's nil-arguments panic is ported as is; the proposal
    [`refuse-null-cloudflare-tunnel-arguments`](../../openspec/changes/refuse-null-cloudflare-tunnel-arguments/proposal.md)
    waits for an owner decision;
  - the HA artifact is gzipped by flate2: its staged bytes differ from Go's,
    while its digest field is the same.
- **k3s:**
  - node roles are listed sorted, where Go's map order is random;
  - an AES-192 join key cannot be unsealed (ring has no AES-192-GCM);
  - RSA decryption is blinded (RUSTSEC-2023-0071).
- **All ports:** a call's cancellation is not propagated to Host Agent
  callbacks or guest commands.
- **Core HTTP clients:** eight core HTTP clients ignore the proxy
  environment, where Go's default transport honours it. This is a follow-up
  outside M9.
