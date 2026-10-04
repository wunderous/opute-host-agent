# Owner decision required: unmarked projection and pinned Go parity

The live two-binary probe in `outcomes.json` proves a conflict between M5's
planning text and the behavior migration is required to preserve. These are
isolated generated canaries, not customer credentials.

| Accepted call | Unmarked value | Durable sink in both Go and Rust |
| --- | --- | --- |
| `get_host_info` with an extra `unmarked` argument | `M5_UNMARKED_FIELD_CANARY_7e35d8bc` | Invocation `arguments_json` |
| `request_task_input` with an extra `unmarked` argument | Same canary | Operation `task_snapshot_json.toolArgs` |
| `get_vm_info` over an Incus shim | `state.incusStatus` contains the canary; its object schema declares `additionalProperties: true` | Invocation `result_json.structured` and `observation_json.structured` |

An unknown capability is rejected without operation, plan or invocation rows
on both binaries. Known write-only fields use a different, explicit schema
projection; this finding must not be confused with an X3 write-only leak.

The pinned Go `internal/hostmcp/evidence_redaction.go` recursively preserves a
value when no child schema marks it write-only. `server.go` then persists that
projection in `recordCapabilityInvocation`. Rust currently preserves the same
behavior. The state store deliberately treats those JSON envelopes as opaque.

M5's `milestones.md` says an unmarked result field "must never be persisted
verbatim". The repository's `AGENTS.md` says: "Implement no new product
capability, endpoint, mode, provider, auth path, default, or user-visible policy
as part of the language migration. An actual contract change needs a separate
OpenSpec change and owner decision."

The owner must choose one of these concrete outcomes:

1. **Preserve pinned Go behavior and clarify M5's planning requirement.**
   Schema-authorized open objects and permitted extra arguments retain Go's
   behavior; write-only fields and unknown capability/schema projections keep
   their existing protections. Unknown-projection evidence verifies the exact
   Go boundary instead of demanding a contradictory new policy.
2. **Approve a separate stricter durable-projection contract.** Unknown fields
   are rejected or redacted before persistence, with a separately reviewed
   OpenSpec change and explicit parity divergence. This changes durable data
   for open objects such as host measurements and provider observations;
   those impacts must be part of the decision and its validation.

Until that decision is made, the unknown-projection gate must report failure.
Matching Go and Rust behavior does not satisfy the current stricter M5 text.
