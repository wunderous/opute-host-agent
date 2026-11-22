# M25 journey observer coverage

The inventory compares host effects reachable through the existing catalog
and journeys with the observer at main commit `98cac2174`. Go remains
frozen and provides the migration inputs used by the existing journeys.

The widened observer records each new kind under an individual effects path.
The e2e-0 probes plant one undeclared change and clean it up. The existing
controls continue to cover an allowed change, fixture restore, schema
validation, and the outcome, confirmation and allowance rules. The gate
checks for the planted effects path as well as the failure. Re-projecting
the draft probe snapshots onto the pre-m25 observer's roots and runtime
fields removes the planted differences from the effects check.

## Observation limits

| Area | Scope |
| --- | --- |
| Files | The observer scans the listed roots and records type, mode, owner, symlink targets and regular-file hashes. Files beyond those roots and changes to ACLs, xattrs, hard-link identity or timestamps require an independent journey assertion. |
| Volatile files and logs | Boot and timer state, randomized systemd private temporary directories, and the harness's own directory are excluded. The secret sweep searches the journal and files for its test markers. |
| Projected state | Agent databases and specified caches are covered by tool reads and the durability and secret-sweep journeys. Incus and Kubernetes use their existing object projections. Container-internal processes are represented by the running container. |
| Processes | The process projection records names and counts by stable cgroup. It omits kernel threads, the inspecting session's own processes, and short-lived udev workers. PID, arguments and environment are excluded. |
| Sockets | The observer records host TCP listeners and stable UDP socket addresses. The ephemeral UDP range is excluded because the DNS and NTP clients use it. Established connections and guest-private sockets are outside the host projection. |
| Kernel and networking | The snapshot compares stable sysctl values, loaded modules, visible mount points and filesystem types, links by name, and routes with metrics. Autotuned counters, per-interface sysctls, route lifetimes, mount options, link configuration and private namespaces are outside this projection. |

## Provider artifact check

The locally built K3s, Cloudflare, and Tailscale executables in both rc8
(0.3.0) and rc5 (0.4.0) matched their release hashes and each opened a
single IPv4 loopback listener when started in a clean temporary directory.

## Findings from the 0.3.0 draft

| Finding | Classification | Work |
| --- | --- | --- |
| P1. Provider executables listened on all host interfaces. | Product defect | D53 binds the shared listener to IPv4 loopback. The failing-first log and the focused Rust test cover the listener. The provider-body journey verifies the installed binary's socket. |
| J1. The copy step changed the existing destination directory mode. | Harness defect | Use `mkdir -p` before copying a fixture. |
| J2. The npm and Podman journeys left temporary files or runtime processes. | Journey defect | Remove the npm compile cache, the Podman fixture's stores and pause process, and the command-test stubs. |
| J3. The harness left PID comparison files under `/tmp`. | Journey defect | Store them with the harness's guest tools. |
| J4. Transient agents, serving workloads, installed providers, guests, and test mounts were absent from the old effect lists. | Journey defect | Name the files, services, processes, listeners and mounts the journeys leave until cleanup. |
| H1. Apt mirror indexes, PackageKit and K3s kernel extensions vary with the mirror, idle timer or fixture state. | Host-dependent | Limit optional allowances to the known effects and record a reason alongside each pattern. |

The 0.4.0 draft also found an intermittent restart journey failure.
Systemd sent SIGKILL to the agent process, then reported an error for an
auxiliary process that had already exited. The journey now targets the
main process so it can observe the service restart.
