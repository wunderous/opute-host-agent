# Outcomes when the host fails underneath the agent

## Why

m16 attacks the agent with host faults on the dev VM (gate e2e-6). The
contract spec states the outcome of a kill or a stop during a plan or task
(`truthful-interrupted-plan`), but not of a full state directory, a damaged
state store, a provider command that hangs or prints garbage, or a clock
stepped back. A journey must not guess those outcomes.

**Decided (owner, 2026-11-10), recorded as D31.** The outcomes below are the
expected ones; anything else is a finding.

## What Changes

No product behaviour is changed by this decision; it states what the agent
must do, and e2e-6 checks it on the release candidate:
- the state directory is full: a call that must write durable state is
  refused with a typed error and leaves no half-written record; reads keep
  working; once space is freed, writes succeed again without a restart;
- `state.db` is damaged: startup refuses, exits non-zero, binds no listener
  and writes nothing; with the store restored, the agent starts and its
  records are intact;
- a provider command hangs or prints garbage: the call ends in a typed tool
  error within the command's timeout, and the agent keeps serving;
- the clock steps back: the agent keeps serving; reservations and tokens
  stay valid.

## Capabilities

### New Capabilities

- `host-fault-outcomes`: the decided outcomes of host faults.

### Modified Capabilities

None.

## Impact

`evidence/e2e/journeys/make_e2e6.py` and gate `e2e-6` in
`evidence/e2e/gates.json`.
