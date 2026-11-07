# Open spec questions for the owner (D27)

Until D27, these behaviours were settled by "as Go does". The frozen v0.2.4
record either captures them only incidentally or does not decide them at all.
Each one needs an owner answer before a Rust change may depart from what Rust
does today. Until then, Rust keeps its current behaviour, which matched Go at
the last parity gate. Nothing here is guessed.

| # | Behaviour | Where Go decided it | Question |
|---|-----------|--------------------|----------|
| Q0 | The frozen record itself | `baseline/inventory`, `tools/parity/divergences.json` | Is the frozen v0.2.4 record normative, with every captured field a requirement, or only evidence that the spec should restate in prose? |
| Q1 | Auth refusal statuses and bodies | Go HTTP middleware | Which status codes and `WWW-Authenticate` and body shapes are contract, and which are incidental? |
| Q2 | The legacy handshake method set | Go ADR 0011 | Should the spec list the method set explicitly, so that the ADR is no longer the source? |
| Q3 | Typed admission error shapes | Go admission errors | Are error codes and messages contract, or only codes? |
| Q4 | Recipe canonical hashing and validation outcomes | Go recipe package | Is the canonical hash algorithm and its byte layout part of the contract, for example because the Platform stores the hashes? |
| Q5 | Plan interruption semantics | Go plan executor | What should resume, compensate or stop after a restart, for each node state? |
| Q6 | List order where Go ranged over maps | k3s roles and du paths, tailscale teardown | Is the order unspecified, or should the spec require a sorted order? |
| Q7 | Go defects ported as is | cloudflare null-args panic, k3s AES-192 unseal, proxy environment in 8 core clients | Should each be fixed to a specified behaviour? Each fix needs its own change. |
| Q8 | Exact error text | D20 (zstd messages) | Are error message strings contract? |
| Q9 | Bridged operation terminal status | Go keys the bridge on the task ID, so the status stays `working` | What is the specified terminal status of a bridged operation? |
| Q10 | Release artifact names and SHA256SUMS order | Go release scripts | Is the order of lines in SHA256SUMS contract, or only the names and the hashes? |
| Q11 | Incidental go-sdk wire details | go-sdk v1.7.0: session log lines, server info | Which of these does a client rely on? |
| Q12 | Catalog revision computation | Go catalog | Is the revision a contract value, meaning its algorithm, or only an opaque token that changes when the catalog changes? |
| Q13 | Cutover evidence | `rust-cutover-parity-gate.json` | Are the e2e-1 to e2e-5 journey gates, the contract suites and the conversion and rollback evidence the agreed M13 evidence set? |
| Q14 | What `HOST_MCP_BIND_HOST=0.0.0.0` binds | ADR 0008 and the Go listener; the record never observed a listen address (M0 review, finding 2) | Found in e2e-1 journey 1: the agent opens one IPv6 wildcard socket in dual-stack mode (`*:3014`), so it also accepts IPv6 connections. Should `0.0.0.0` bind IPv4 only, or all addresses? |
| Q15 | Reclaiming a crashed plan run's reservation | `plan-reservation-inheritance` reclaims via a restored task snapshot. A plan's task (id = run id) is persisted only on failure or resume, in Go v0.2.4 as in Rust | Found in e2e-1 journey 7: after a SIGKILL during `run_host_plan`, the run's `heavy` reservation survives the restart and refuses all heavy work until it expires (15 min). M6 masked this with 64 heavy slots. Should restart reclaim it? Proposal: [reclaim-orphaned-plan-run-reservations](../reclaim-orphaned-plan-run-reservations/proposal.md). |
| Q16 | Which Incus project the agent addresses | Go launches with the client's current project but reads instance state with `incus query`, which uses `default` | Found exploring e2e-3a: with the `opute` client switched to another project, `provision_vm` creates the container there, then fails after 3 minutes because its address wait reads `default`. Should the agent always use `default`, or one configured project in every call? Proposal: [incus-project-selection](../incus-project-selection/proposal.md). |
| Q17 | Retiring dead K3s servers after quorum recovery | No spec; the K3s provider (Go `membership.go`, ported in M9) proves `remove-node` quorum from Node Ready conditions | Found in e2e-3a journey 4: after `recover-quorum` resets etcd to one member, the stopped servers still count as members. Once they are NotReady, `remove-node` refuses to retire them (1 ready of 2 remaining). Before that, `recover-quorum` reports them Ready from stale conditions. What must recovery and removal guarantee? Proposal: [k3s-quorum-recovery-retirement](../k3s-quorum-recovery-retirement/proposal.md). |
| Q18 | Implementing `list_secret_keys` | Published in the catalog, but Go v0.2.4 has no handler; Rust fails it closed (`not_implemented`, registered unported) | Found in e2e-3a journey 2, whose pass condition needs it. Implementing it adds a capability Go never had. Should the Host Agent implement it? Proposal: [implement-list-secret-keys](../implement-list-secret-keys/proposal.md). |
| Q19 | A typed stable endpoint for HA K3s | `ensure-ha-endpoint` only verifies a caller's endpoint; no typed operation creates one | Found in e2e-3a journey 1: against a server's own K3s API the probe fails closed (`UnknownIssuer`), and nothing provides an endpoint that survives a server's loss. Should one exist, and where? Proposal: [typed-ha-endpoint](../typed-ha-endpoint/proposal.md). |
| Q20 | The first `ensure_host_service_supervisor` call | Go probes the user bus right after enabling lingering (T2 masks the error text) | Found in e2e-4a journey e2e2.host-ops: on a host with no user manager the first call fails ("user service supervisor bus is unavailable") and a second reports ready, in Go as in Rust. Must one call settle it? Proposal: [settle-user-service-supervisor](../settle-user-service-supervisor/proposal.md). |

The spec's Purpose paragraph still describes behaviour preserved "from the
pinned Go source". OpenSpec deltas cannot change a Purpose, so it is
rewritten when this change is archived (tasks 3.1).
