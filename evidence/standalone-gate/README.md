# Standalone read-only gate evidence (decision D10)

This file records the Rust implementation of
[`standalone-read-only-gate`](../../openspec/changes/standalone-read-only-gate/proposal.md).
It is a declared divergence from the Go baseline, approved as decision D10 in
[milestones.md](../../openspec/changes/reimplement-host-agent-in-rust/milestones.md).
This work does not change the Go agent.

The evidence is in [`evidence/current/`](../current/), under
`contracts/standalone-read-only-gate/` and `rust-canaries.json`. Reproduce it
with `make parity-contracts parity-rust-canaries`, or run `make parity-m3`
for the whole gate.

## The rule

With standalone mode on and `OPUTE_STANDALONE_ALLOW_MUTATIONS` not enabled:

```text
 tools/call name ─► published by this server? ─yes─► descriptor effect == read ? ─► run
                          │                                   └─ no ─► refuse
                          no (internal, dispatchable)
                          └─► effect *declared* read ? ─yes─► run
                                         └─ no / only inferred ─► refuse
 (Go's standalone mutation list: always refused)
```

| | Rust (D10) |
| --- | --- |
| Error result | `Error: standalone mutations are disabled; set OPUTE_STANDALONE_ALLOW_MUTATIONS=true`, the same text Go uses |
| Pipeline position | after the catalog revision check and argument decoding; before lifecycle routing, tasks, admission and dispatch (Go's gate position) |
| Catalog | unchanged; names, descriptors, effects and revision stay under Go parity (X5) |
| Platform mode and standalone with mutations allowed | unchanged |

The gate reads the same effect classification that `tools/list` publishes to
clients (`_meta.capability.privilege`). What a client is told a tool does
and what the gate allows cannot drift apart. When an effect is not known to
be `read`, the gate fails closed.

## How it is verified

```text
 Go-vs-Rust twin runs (unchanged)          Rust-only contract suite (8 scenarios)
 ┌───────────────────────────────┐   ┌───────────────────────────────────────────┐
 │ catalog cells byte-identical  │   │ sweep: list the agent's own catalog, call │
 │ → the sweep's tool selection  │──►│ every non-read tool (refused) and every   │
 │   is the Go catalog           │   │ read tool (not refused); internal tools;  │
 │ every other surface ×5        │   │ mutations allowed; platform; stale        │
 └───────────────────────────────┘   │ revision first; no trace, no rows         │
                                     └────────────────────┬──────────────────────┘
                                                          ▼ must go red under
                                              5 Rust canaries (K16–K20)
```

| Scenario | Spec scenario |
| --- | --- |
| `gate.published-non-read-refused` | all 87 published non-read tools are refused |
| `gate.published-read-not-refused` | none of the 55 published read tools is refused; `get_host_info` still succeeds |
| `gate.unpublished-without-declared-read` | the 7 internal tools without a declared `read` effect are refused |
| `gate.unpublished-with-declared-read` | an internal tool with a declared `read` effect is not refused |
| `gate.mutations-allowed` | with `OPUTE_STANDALONE_ALLOW_MUTATIONS=true`, no published or internal tool is refused |
| `gate.platform-unaffected` | platform mode refuses no non-read tool at the gate |
| `gate.refused-leaves-no-trace` | after refusing every non-read and internal tool with plausible arguments, the shim trace and the operations, plan_runs and capability_invocations tables are empty |
| `gate.stale-revision-first` | a stale catalog revision and undecodable arguments are still reported first |

Each canary is a one-line patch that must turn its named scenario red:

| Canary | Patch | Caught by |
| --- | --- | --- |
| K16 | the gate refuses only the enumerated mutation list | `gate.published-non-read-refused` |
| K17 | internal tools pass on an inferred `read` effect | `gate.unpublished-without-declared-read` |
| K18 | the gate ignores `OPUTE_STANDALONE_ALLOW_MUTATIONS` | `gate.mutations-allowed` |
| K19 | the gate also fires in platform mode | `gate.platform-unaffected` |
| K20 | the gate refuses read tools | `gate.published-read-not-refused` |

A harness test (`tests/test_contract.py`) fails if the suite's internal
tool list differs from the pinned catalog source's unpublished internal
definitions. A new internal tool therefore cannot go untested.

## Results

@RESULTS@

## Notes

- The suite is not run against the Go reference. In Go, some of the swept
  tools would execute host commands. The canaries provide the negative
  control instead.
- Go's mutation list is kept as an explicit refusal in
  `catalog::standalone_read_only`. Today every tool on it also has a non-read
  effect, which the unit test `standalone_read_only_gate` asserts, so no
  canary can isolate that line.
