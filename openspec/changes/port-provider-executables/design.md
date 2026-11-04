# Design: provider executables in Rust

## Inventory (pinned v0.2.4, captured from the running Go executables)

| Provider | Tools | Capabilities provided | Services (operations) | Recipes | Teardown |
|---|---|---|---|---|---|
| hostos | 3 | host-platform.v1 | host-platform (2) | none | no |
| cloudflare | 25 | tunneling.v1, mesh-runtime.v1, mesh-membership.v1, public-ingress.v1 | tunneling (13), mesh-runtime (4), mesh-membership (3), public-ingress (3) | 5 | yes |
| tailscale | 15 | mesh-runtime.v1, mesh-membership.v1, private-mesh.v1, public-ingress.v1 | mesh-runtime (4), mesh-membership (3), private-mesh (3), public-ingress (3) | 5 | yes |
| k3s | 30 | kubernetes.v1 | kubernetes (28) | 1 | yes |

Every provider serves go-sdk v1.7.0's stateless, JSON-response Streamable HTTP
handler on `:$OPUTE_PROVIDER_PORT/mcp` (`serve`), and publishes its contract
through `opute.provider.get_install_manifest`.

Boundaries each port must reproduce:

- **hostos:** reads `pkg/hostplatform.Detect` (OS, host kind, CPU identity);
  read-only, no external commands.
- **cloudflare:** a Host Agent client (`pkg/hostagentclient`) through
  `OPUTE_HOST_AGENT_ENDPOINT` and `OPUTE_HOST_AGENT_BEARER_TOKEN`; resource
  identity (`internal/resourceid`), Kubernetes names (`k8sname`), shell
  commands through `sh -c`.
- **tailscale:** a Host Agent client as above; `OPUTE_HOST_AGENT_ID`,
  `OPUTE_PEER_MESH_IP`, `OPUTE_PUBLIC_LOCAL_TARGET`,
  `OPUTE_TAILSCALE_BACKEND`, `OPUTE_TAILSCALE_ENV_FILE` and its live-drive
  switches.
- **k3s:** runs `incus` and `sudo` directly; the largest service (28
  operations).

## Where the code lives

The host-agent crate becomes a library plus binaries. The Rust core already
reproduces go-sdk's stateless handler (`mcpsdk.rs`, `http1.rs`), host
observation (`hostobs.rs`) and the provider contract types (`provider/`), so a
provider executable is a thin `main` plus one ownership-focused module per
provider, registered on the D14 seam:

- Service Definition: the operations the install manifest declares;
- Service Provider: the provider's implementation, registered in the one
  service context;
- Consumer: the Host Agent's provider binding, unchanged.

No Go package is translated one for one, and no trait layer is added without
a real boundary.

## Interim pin of the Go executables

`build-artifacts.sh` stops running Go's Makefile for the providers. It
downloads nothing at build time: the three executables are fetched once from
the v0.2.4 GitHub release into `.parity/releases/go-published-v0.2.4`, checked
against that release's `SHA256SUMS`, and copied into the release set. The
release `SHA256SUMS` keeps Go's names and order. A rebuild still reproduces
the Rust binaries byte for byte; the provider executables are compared by
hash with the published ones.

The interim pin ended when the release set switched to the Rust ports
(task 4.1); its fetch script is removed.

## Validation (one m9 gate per provider)

1. Provider contract: `tools/list`, `initialize` and the install manifest from
   the Rust executable equal Go's after no masks.
2. Provider swap under the same Rust core: the provider-lifecycle and
   provider-real scenarios run once with the Go executable and once with the
   Rust one; the harness compares the core's observations, the provider
   boundary traffic (calls, callbacks into the Host Agent, events) and shim
   traces. Go/Go runs at most 3 times.
3. T2: k3s in its nested-Incus and k3s goldens; the tunnel providers with
   shimmed `cloudflared` and `tailscale`, no real accounts.
4. The provider's own Go tests that drive the executable, run against the
   Rust executable as black boxes.

No validation step runs over 30 minutes; suites are sharded.
