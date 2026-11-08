# Owner decisions of 2026-11-07 (D28)

## Why

`rust-spec-authority` (D27) left every behaviour that only Go v0.2.4 decided
as an owner question (Q0-Q28), and the cutover items in tasks.md 4.4 and
5.1-6.4 needed owner decisions. On 2026-11-07 the owner accepted the
recommendations made for each of them. This change records those answers
and writes the ones that need no code into the contract spec. The answers
that need code are carried by their own changes, each turned from
"proposed" into "decided".

Owner: the Host Agent owner (wunderous). Date: 2026-11-07. These decisions
are not reopened by later work; a behaviour they do not state is a new
question.

## Decisions

| # | Question | Decision | Carried by |
|---|----------|----------|-----------|
| Q0 | The frozen record | Evidence, not normative. The prose spec and accepted decisions are normative; the record decides only what the prose leaves to it. | this change (spec) |
| Q1 | Auth refusal shapes | HTTP status codes are contract; bodies and message text are not. | this change (spec) |
| Q2 | Legacy handshake method set | The spec lists it. | this change (spec) |
| Q3 | Typed admission errors | Error codes are contract; messages are not. | this change (spec) |
| Q4 | Recipe canonical hash | Open: the owner confirms whether Platform stores hashes. | questions.md |
| Q5 | Plan interruption | A restart leaves an interrupted run's truthful durable state; nothing is resumed or compensated implicitly. | `truthful-interrupted-plan` |
| Q6 | List order from Go maps | Unspecified unless a requirement states an order. | this change (spec) |
| Q7 | Go defects ported as is | Fix each, separately: `refuse-null-cloudflare-tunnel-arguments`, `k3s-aes192-join-key`, `core-client-proxy-environment`. | those changes |
| Q8 | Error text | Not contract. | this change (spec) |
| Q9 | Bridged operation status | The terminal status of its task. | `truthful-operation-status` |
| Q10 | SHA256SUMS order | Names and hashes are contract; line order is not. | this change (spec) |
| Q11 | go-sdk incidentals | Not contract (session log lines, server info beyond name). | this change (spec) |
| Q12 | Catalog revision | An opaque token that changes when the catalog changes. | this change (spec) |
| Q13 | Cutover evidence | The e2e gates, the Rust-only contract suites, and the state-conversion and rollback evidence; e2e-5 joins once Platform staging exists and is recorded blocked until then. | this change (spec) |
| Q14 | `0.0.0.0` | Binds IPv4 only. | `ipv4-wildcard-bind` |
| Q15 | Orphaned plan reservation | Reclaimed on restart. | `reclaim-orphaned-plan-run-reservations` |
| Q16 | Incus project | Always `default`. | `incus-project-selection` |
| Q17 | K3s retirement after recovery | Removal waits for recovery to settle. | `k3s-quorum-recovery-retirement` |
| Q18, Q19 | New capabilities | Deferred; not cutover blockers. | their proposals |
| Q20 | Supervisor first call | One call settles it. | `settle-user-service-supervisor` |
| Q21 | Undeclared inputs | Declared. | `declare-read-inputs` |
| Q22 | Nullable output fields | Honoured. | `nullable-output-schemas` |
| Q23 | System service restart | The tool describes user services only. | `privileged-host-service-restart` |
| Q24 | Stopped cluster | Listed as stopped. | `report-stopped-clusters` |
| Q25 | Operation row status | The task's terminal status. | `truthful-operation-status` |
| Q26 | Container runtime stderr | Reports parse stdout only. | `container-runtime-stdout` |
| Q27 | Serving launch | Runs with the user bus; a launch that does not start fails. | `truthful-serving-launch` |
| Q28 | Standalone service domain | The `uri` descriptor is published. | `bind-standalone-service-domain` |

Cutover items:
- **Rollback window (6.2):** 14 days. Go stays installed and selectable
  through it; Go retirement is a later, separate decision.
- **First Rust package (5.2):** version 0.3.0 with the Rust catalog. npm
  publishing stays postponed (D25 rehearsal note); nothing is published by
  this change.
- **Writer lock (4.4):** already decided as D25 option A and implemented in
  Go v0.2.4 and Rust (`state-directory-writer-lock`); this decision only
  confirms it.

## What Changes

- The contract spec states the answers that need no code (Q0-Q3, Q6, Q8,
  Q10-Q13) and the cutover decisions.
- questions.md marks every question answered, with its carrier.
- No runtime behaviour changes in this change.

## Impact

- `openspec/specs/host-agent-contract/spec.md` after archive.
- `rust-spec-authority/questions.md`.
