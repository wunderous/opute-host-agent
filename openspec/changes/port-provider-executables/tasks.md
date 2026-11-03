## 1. Decision and interim pin

- [x] 1.1 Record the owner's decision D26: port hostos, cloudflare, tailscale and k3s; ollama stays out (D15). Decided 2026-11-02.
- [x] 1.2 Take the three Go provider executables from the pinned v0.2.4 GitHub release, checked by its SHA256SUMS, instead of building them from the Go tree; the release checks and dist journeys still pass. Done in `182d8d0f3`; superseded by 4.1 once the ports passed.

## 2. Shared groundwork

- [x] 2.1 Give the host-agent crate a library target so provider executables share its MCP server, HTTP and contract code. Done in `33ff37a95`.
- [x] 2.2 Add the provider-swap mode to the parity harness: one scenario run per provider executable under the same core, compared at the provider boundary and in shim traces; proven with a Go/Go run and a canary. T1 in `d3db80c17` (a one-byte canary of the Rust hostos fails the swap), T2 in `37c51af6a`; the provider boundary itself is compared by the wire corpora (`ef8cc8cb0`, `tools/parity/provider_wire.py`).

## 3. Ports (each: inventory pinned, Rust executable on the D14 seam, Go black-box tests, provider swap, T2 where it applies, m9 gate)

No Go test drives a provider executable as a black box: the tests under
`plugins/*/cmd/*` run in-package (the Tailscale `mcp_wire_test.go` serves the
handler through `httptest`). The wire corpora stand in for them: every
method of the go-sdk server table in both protocol eras, malformed requests,
the transport guards and every provider tool, recorded from the Go and the
Rust executable and compared step by step (Go recorded twice). The Go unit
tests' cases are ported as Rust unit tests in each provider module.

- [x] 3.1 hostos (`c62b87ccc`): wire corpus 270/270 steps equal; provider-real swap passes; gate `m9-hostos`.
- [x] 3.2 cloudflare (`33bb4584f`): wire corpus 602/602 and API corpus 508/508 against a fake Cloudflare API (every request equal); provider-real swap passes; gate `m9-cloudflare`. The Go nil-arguments panic is ported as is ([`refuse-null-cloudflare-tunnel-arguments`](../refuse-null-cloudflare-tunnel-arguments/proposal.md), awaiting the owner). The HA artifact is gzipped by flate2, so its staged bytes differ from Go's while its digest field is the same.
- [x] 3.3 tailscale (`66b02dc6d`): wire corpus 488/488 and live corpus 444/444 against a fake Tailscale API and Host Agent; provider-real swap passes; gate `m9-tailscale`. One teardown step sends the same requests in another order: Go ranges over a map there, and Go/Go shows it too.
- [x] 3.4 k3s (`5d6e11fb1`): wire corpus 808/808 against a scripted K3s guest behind a fake `incus` and `sudo`; join material seals and unseals across Go and Rust; provider-real swap and T2 swap (cluster, oci and postgres goldens) pass; gate `m9-k3s`. Recorded, not changed: node roles are listed sorted where Go's map order is random; an AES-192 join key cannot be unsealed (ring has no AES-192-GCM); RSA decryption is blinded (RUSTSEC-2023-0071); `inspect-guest-storage` reports `du` paths in Go's map order.

Not covered here, as owned gaps: live Cloudflare and Tailscale accounts; T2
for hostos, cloudflare and tailscale (no T2 scenario starts them; the host
tunnel scenarios run `cloudflared` from the core); the Rust ports do not
propagate a call's cancellation to their Host Agent callbacks or guest
commands (recorded gap).

## 4. Release

- [x] 4.1 Switch each provider in the release set to its Rust executable when its m9 gate passes. Done in `4470098bd`: Go's names and SHA256SUMS order, static x86_64 musl.
- [x] 4.2 Remove the Go toolchain and the Go tree from the release build; the rebuild, release checks and distribution journeys pass. `build-artifacts.sh` uses cargo and zig only; evidence/m9 rebuilds the set byte for byte and passes the release checks and dist journeys.
- [x] 4.3 Close the M9 inventory gap; update milestones.md and tasks.md. The distribution inventory names each provider artifact as the Rust port under its m9 gate.
