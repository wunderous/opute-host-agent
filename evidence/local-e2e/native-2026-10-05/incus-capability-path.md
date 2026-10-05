# Existing Incus bootstrap capability and local T2 route

The authority for this trace is the pinned Go source in `baseline/source-lock.json`, commit `ace7013df17528fee1bed13a1d70a132d6c5eb9b`. The owner supplied [DeepSeek's three-role capability design](https://deepseek-harness.github.io/deepseek-harness/en/develop/practice/): the full capability combines request/result definition, implementation and consumer; providers and consumers depend on the definition. Packages should be split only where independently evolving implementations justify it.

The existing Go path is:

| Role | Pinned implementation |
| --- | --- |
| Catalog contract | Runtime-captured `baseline/inventory/catalog/{standalone,platform}.json`: `install_incus_stack`, mutation, heavy class, approval metadata, Incus implementation |
| Typed request | `internal/domain/host/tool.go:InstallIncusStackArgs` |
| Consumer | `internal/tools/dispatch_host.go`: decode arguments, dispatch with mutation/heavy/task-aware policy, return structured result |
| Installer | `internal/domain/host/tool.go:InstallIncusStack`: shared-host ownership check, Linux check, signed Zabbly repository, version candidate check, package installation, group access, daemon startup, default storage/network/profile initialization |
| Runtime capability | `internal/domain/incus`: instance lifecycle and runtime observations through the shared runtime adapter |
| Dependency composition | `internal/hostagent/incus_domain.go`: inject `ReinstallIncusStack` into Incus; it calls the host installation service |
| Recipe composition | `internal/recipe/host_recipe.go`: `host-recipe.v1`, coordinator `host-agent`, mode `local`; expanded typed actions use the existing `host-plan.v1` executor |
| Enforcement recovery | `internal/tools/dispatch_resource.go`: control-class `reconcile_host_resource_policy`, exact canonical host-service URI and policy revision; repairs the bounded systemd projection even while enforcement is unknown |

The captured catalog advertises fewer installer argument properties than the installer struct (notably channel/version/QEMU selection). Preserve the actual runtime schema and dispatcher behavior; do not widen it based solely on a static descriptor or infer a new provider plugin API. The pinned result remains a structured map; do not describe it as a fully typed result model.

`RequireSharedHostOwner` rejects a mismatching configured instance; an unset owner is permitted by the pinned implementation. That check is not a shared-runtime lease. The validation route is a dedicated QEMU VM with a separate exclusive local file lease and isolated disk, identity, credentials and network forwarding. Incus installation and resource-policy reconciliation run through owning typed agent capabilities inside that VM. No shared Incus, Kubernetes, tunnel or deployment state is changed.

The local machine already provides QEMU, KVM access and cloud-init seed tooling. The official Ubuntu 24.04 cloud image is downloaded into ignored test storage, verified against signed SHA256SUMS with the installed Ubuntu cloud-image keyring, and used as a backing image. A dedicated VM bootstrap account has non-interactive privilege only inside that VM. Runtime lease metadata, command lines and image hashes are recorded alongside the test.

Rust's current `tools.rs` does not dispatch `install_incus_stack`; this remains a declared M8 mutation gap. Installing test infrastructure through the pinned Go agent does not claim that Rust mutation capability is complete. Real Incus read/admission observations can still validate the existing M1–M6 Rust boundaries.

Status: the signed cloud image booted successfully under an exclusive local lease. Go policy reconciliation completed through MCP, and Rust independently observed real enforcement as `enforced`. The Go installer returned a durable task, now installing Incus 7.0.1 from the signed LTS repository and QEMU. Rust policy reconciliation refuses at the existing unimplemented host-service adoption boundary; this is a declared M8 mutation/adoption gap, not successful mutation parity. Real Incus inventory and admission validation completed: 104 assertions and 28 structured comparisons across independent Go/Rust clones, before and after restart. See `t2/summary.json`. All test VMs stopped cleanly and released their leases.
