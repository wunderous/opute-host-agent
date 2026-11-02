## 1. Decision and interim pin

- [x] 1.1 Record the owner's decision D26: port hostos, cloudflare, tailscale and k3s; ollama stays out (D15). Decided 2026-11-02.
- [ ] 1.2 Take the three Go provider executables from the pinned v0.2.4 GitHub release, checked by its SHA256SUMS, instead of building them from the Go tree; the release checks and dist journeys still pass.

## 2. Shared groundwork

- [ ] 2.1 Give the host-agent crate a library target so provider executables share its MCP server, HTTP and contract code.
- [ ] 2.2 Add the provider-swap mode to the parity harness: one scenario run per provider executable under the same core, compared at the provider boundary and in shim traces; proven with a Go/Go run and a canary.

## 3. Ports (each: inventory pinned, Rust executable on the D14 seam, Go black-box tests, provider swap, T2 where it applies, m9 gate)

- [ ] 3.1 hostos
- [ ] 3.2 cloudflare
- [ ] 3.3 tailscale
- [ ] 3.4 k3s

## 4. Release

- [ ] 4.1 Switch each provider in the release set to its Rust executable when its m9 gate passes.
- [ ] 4.2 Remove the Go toolchain and the Go tree from the release build; the rebuild, release checks and distribution journeys pass.
- [ ] 4.3 Close the M9 inventory gap; update milestones.md and tasks.md.
