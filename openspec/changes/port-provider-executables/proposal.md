# Port the provider executables to Rust (M9)

## Why

The Rust core reached parity with the Go core (M1–M8, M10, M12a, M12b and
D25), but the Rust release set still ships Go executables:
`tools/release/build-artifacts.sh` builds `opute-provider-k3s`,
`opute-provider-cloudflare` and `opute-provider-tailscale` from the pinned Go
tree with Go's own Makefile targets. Releasing Rust therefore needs a Go
toolchain and the Go source, and the providers that carry most of the product
behaviour (Kubernetes, tunnels, mesh) stay in the codebase the migration is
meant to retire.

Decision D2 deferred these ports: providers sit behind a versioned process
contract, so the Go executables could keep running under a Rust core, and
porting them was not needed to prove core parity. That proof now exists.

**Decided (owner, 2026-11-02): port, recorded as D26.** D2 moves from "defer,
per provider" to "port hostos, cloudflare, tailscale and k3s". ollama stays out
(D15).

## What Changes

- **Interim (no behaviour change).** The release set takes the three Go
  provider executables from the pinned `wunderous/host-agents` v0.2.4 GitHub
  release, verified against that release's `SHA256SUMS`, instead of building
  them from the Go tree. The release build stops needing Go before any port
  lands.
- **Ports, in order:** hostos (proves the method), cloudflare, tailscale, k3s.
  Each is a Rust executable with the Go executable's name, flags, environment,
  wire behaviour, errors, exit codes, files and external commands, on the D14
  three-role seam.
- **Validation per provider (M9):** under the same Rust core, the Go executable
  and the Rust executable are swapped and compared at the provider boundary
  (`tools/list`, the install manifest, calls, callbacks into the Host Agent,
  events), in shim traces and in T2 effects; the provider's own Go tests that
  drive the binary run against the Rust binary as black boxes.
- **Release switch:** a provider's Rust executable replaces the pinned Go one
  in the release set only when its m9 gate passes. When all three have
  switched, the release build has no Go toolchain and no Go tree.

No capability, tool, mode, provider, auth path, default or user-visible
policy is added. A Go provider defect is ported as is, recorded, and goes to
its own proposal.

## Impact

- New Rust executables in the workspace; the host-agent crate gains a library
  target so the provider executables share its MCP server, HTTP and contract
  code instead of copying it.
- `tools/release/build-artifacts.sh`, the dist checks and the release
  reproducibility check change twice: once for the pinned Go executables, once
  per provider switch.
- The parity harness gains a provider-swap mode and one m9 gate per provider.
- Owned gaps that remain: live Cloudflare and Tailscale accounts, arm64 and
  Windows runtimes, ollama (D15).
