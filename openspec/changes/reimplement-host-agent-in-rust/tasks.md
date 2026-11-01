# Tasks: behavior-preserving Rust Host Agent

These are ordered implementation gates, not a claim that implementation has
started. A task is complete only when its named artifact and boundary-level
evidence are committed. The pinned Go revision is the initial baseline; rebase
it deliberately if the production Go contract changes. Do not import the
abandoned Rust port.

[milestones.md](milestones.md) groups these gates into ordered milestones and
defines the differential end-to-end harness and evidence each one requires.

## 0. Freeze and enumerate the behavior to preserve

- [x] 0.1 In the new repository, create a machine-readable parity manifest
  keyed to `baseline/source-lock.json`: source SHA/tree, schema/catalog
  revisions, mode, active provider set, tested artifact versions, and evidence
  run IDs. Gate: an absent, stale, or mismatched key makes the verifier fail.
  Done (2026-10-12): `parity-manifest.json` and `tools/parity verify`; the r8 gates rerun byte for byte in `evidence/local-e2e/native-2026-10-12-r8/validation.json`.
- [ ] 0.2 Inventory the Go CLI, flags, environment precedence, defaults,
  startup/shutdown behavior, endpoints, auth modes, protocol methods, errors,
  installation, systemd, npm launcher, Linux archives, and operator commands.
  Own the capture in `baseline/`; compare it against Go source/tests and a
  packaged Go binary. Gate: every public surface has a named owner and test.
- [x] 0.3 Capture authenticated `tools/list` and schema projections from the
  pinned Go binary for each supported serving mode and each configured
  provider fixture. Record conditional availability, effects, approval,
  idempotency, resource bindings, and catalog revision. Gate: no descriptor
  is accepted solely because it appears in a static list or count.
  Done (2026-10-12): `baseline/inventory/catalog/` for every mode and provider cell, compared Go/Rust in the r8 suites (`evidence/local-e2e/native-2026-10-12-r8/validation.json`).
- [x] 0.4 Inventory recipes, plan node states, retries, readiness,
  compensation, callbacks, Tasks/cancellation, provider generations, and all
  local durable stores. Record Go fixtures and expected wire/state outcomes.
  Gate: unknown state or unsupported fixture remains an explicit gap.
  Done (2026-10-12): `baseline/inventory/{plans,state,runtime-gaps}.json`; every remaining gap is owned by M8 or M10 (`baseline/inventory/gaps.json`).
- [x] 0.5 Review the new parity-gate invariant against the Go ADRs, the
  Platform owner, and the current public release parity decision. Anchor its
  typed decision and fail-closed verifier in this repository before writing a
  Rust runtime seam. Gate: preserved, introduced, and retired invariants have
  an owner, exception path, revision rule, and checkable evidence.
  Done (2026-10-12): reviewed in `.agents/decisions/rust-cutover-parity-gate.json` (accepted), with the `requireCutoverEvidence` rule in the verifier.

## 1. Establish the smallest maintainable Rust ownership structure

- [x] 1.1 Create a Rust workspace with a binary and only the contract, core,
  host, and transport modules or crates justified by actual dependency edges.
  Add reproducible formatting, lint, unit-test, and build commands. Gate: no
  Go package-by-package translation or speculative public trait layer.
- [x] 1.2 Import or generate versioned descriptors and schemas from the
  pinned authoritative contracts. One descriptor source must drive Rust
  validation, MCP publication, and parity comparison. Gate: deterministic
  generation and a failing diff for any unreviewed contract drift.
- [x] 1.3 Implement explicit composition and lifecycle ordering for config,
  state, auth, catalog, providers, and listeners. Gate: focused tests prove
  partial startup cleans up in reverse order and never exposes a half-ready
  MCP endpoint.

Evidence for 1.1, 1.3 and 2.1: [evidence/m1/README.md](../../../evidence/m1/README.md)
(`make parity-verify-m1`). Evidence for 1.2: [evidence/m3/README.md](../../../evidence/m3/README.md)
(`make catalog-source` regenerates the committed source from the pinned Go
tree; `make parity-ci` fails on drift; `make parity-verify-m3`).

## 2. Prove identity, transport, and read-only parity

- [x] 2.1 Match Go CLI/config parsing, explicit opaque
  `OPUTE_REMOTE_AGENT_ID`, standalone/platform defaults, and fail-closed
  startup. Gate: differential CLI and configuration fixtures, including
  missing/ambiguous identity and environment precedence, pass.
- [x] 2.2 Implement `/health` and authenticated `/mcp` with the exact supported
  auth modes, MCP 2026-07-28 behavior, and the default-off bounded ADR 0011
  legacy handshake exception. Gate: separate-process wire captures compare
  status, headers, discovery, errors, and forbidden legacy method behavior.
  Evidence: [evidence/m2/README.md](../../../evidence/m2/README.md). OAuth
  token issuance is deferred by owner decision D8.
- [x] 2.3 Implement catalog publication, `tools/list`, read-only host tools,
  structured results, and typed admission. Gate: valid and invalid credentials,
  `get_host_info {}`, resource kind, tenant scope, and default mutation denial
  match Go in isolated fixtures; no mutation is used to satisfy this gate.
  Evidence: catalog, read-only tools and default mutation denial in
  [evidence/m3/README.md](../../../evidence/m3/README.md); typed admission,
  canonical resource kinds, tenant scope and the admission matrix in
  [evidence/m4/README.md](../../../evidence/m4/README.md). Refused task-aware
  calls write no operation record, by owner decision D12.

## 3. Prove provider and task lifecycle parity

- [x] 3.1 Implement provider manifest/schema/dependency validation and
  candidate, active, and draining generation transitions. Gate: failed
  candidate activation leaves the prior catalog and provider usable;
  generation and catalog revisions change only under the Go contract.
  Progress: private publication, retirement, restore, executable descriptor
  ownership, immutable retained handles, and native schema compilation are
  implemented. The owning Go function and actual SDK tool listing expose a
  competing-family displacement failure before schema validation; the separate
  `atomic-provider-family-publication` proposal is pending owner review. Public
  activation, durable rollback, unchanged provider processes and the M7 gate
  remain open.
  The private activation owner now composes readiness, admission, publication,
  durable generation rows, mounting, predecessor disposal and rollback in the
  pinned order. Its metadata/durable matrix covers 170 cases over five
  iterations; caller-abort ownership has a focused regression. Connected adapter
  activation, mount rollback, replacement and explicit candidate cleanup now
  pass 24 cases over five iterations, including retained-connection probes and
  actual loopback MCP request comparison. Provider process execution, public
  wiring, enclosing recipe cleanup and restart remain required.
  Public catalog readers now share the existing publication owner, while MCP
  listing retains its own last-completed projection, including the pinned
  no-op retirement behavior. Public provider lifecycle command composition
  and executable dispatch are still required.
  A server-owned provider runtime now serves provider status and validation
  through the shared owners, with 120 exact comparisons over five iterations
  ([evidence/m7/README.md](../../../evidence/m7/README.md)). Public install,
  activation and teardown commands remain required.
  Public install, reload, teardown (normal, forced, idempotent), generation
  routed calls, in-flight replacement and restart restore now match Go over a
  35-scenario fake-provider matrix, five iterations each, inside 137/137
  public scenarios. Callbacks, bridged tasks, supervision and the five real
  providers remain required.
  Done for M7: the five pinned Go providers run unchanged through the public
  lifecycle (install with their own recipes, catalog cell, a provider-reached
  call, validation, teardown) and match Go over five iterations, with no agent
  descendants or orphaned shims. A flapping readiness probe leaves the prior
  catalog and a later install activates. `inspect_host_service` and
  host-service adoption are ported. Owner decisions still pending, with Go's
  behaviour kept or the difference reported:
  `atomic-provider-family-publication` (Go ordering kept),
  `reject-null-provider-input-requests` and `unadmitted-callback-task-record`
  (open divergences). Evidence: `provider-journeys-validation.json` in
  [evidence/m7/README.md](../../../evidence/m7/README.md).
  Done (2026-10-12): D15 settles hostos and ollama, D16 is implemented; see `evidence/m7/README.md` (M7 exit) and `evidence/local-e2e/native-2026-10-12-r8/validation.json`.
- [x] 3.2 Implement generation-bound admission, callback primitive routing,
  readiness, task bridge, cancellation, and bounded disposal. Gate: wire and
  integration fixtures cover replacement during in-flight work, timeout,
  cancellation, stale revision, callback rejection, and reverse-order cleanup.
  Progress: the admission part (stale catalog revision, the host resource
  coordinator, reservation leases), the task bridge for built-in tools
  (`tasks/get`, `input_required`, cooperative cancellation) and timeouts are
  done in M4 ([evidence/m4/README.md](../../../evidence/m4/README.md)).
  Provider generations, callback routing, readiness, replacement during
  in-flight work and bounded disposal remain open for M7.
  Done for M7: signed callback admission with Go's refusals, delegated child
  tasks started on acknowledgement, bridged tasks (task extension required,
  `tasks/cancel` refused as in Go), shutdown cancelling unacknowledged and
  working tasks and aborting in-flight provider calls, in-flight replacement,
  readiness faults, and normal, forced and idempotent teardown. Reverse-order
  disposal is proven against Go's own container functions (385/385, T0) and
  in each provider's teardown plan. A shutdown before a delegated child is
  acknowledged is the open divergence `unadmitted-callback-task-record`.
  Done (2026-10-12): D17 and D18 applied; the lifecycle matrix passes Go/Go and Go/Rust in `evidence/local-e2e/native-2026-10-12-r8/validation.json`.
- [x] 3.3 Port every supported provider integration and conditional catalog
  projection behind neutral typed contracts. Gate: the equivalent configured
  Go and Rust fixture has no missing, extra, or differently shaped tool; an
  unavailable provider is reported truthfully, not silently omitted.
  Done (2026-10-12) for the supported providers (ollama out of scope, D15): each real provider's catalog is compared Go/Rust in its journey; `evidence/m7/README.md`.

## 4. Prove execution and durable-state parity

- [ ] 4.1 Port all supported host primitives and domain operations behind
  typed seams; maintain `vm:` versus `container:` target separation. Gate:
  focused contract tests and isolated external-effect comparisons cover
  success, failure, cleanup, and absence of unauthorized side effects.
  Each operation is a built-in provider of a three-role service and the
  seam verifier passes for its domain (decision D14,
  [`three-role-capability-seam`](../three-role-capability-seam/tasks.md)).
  8a (host) done 2026-10-13: T1 and T2 parity green, the seam verifier
  reports no gap, and the m8a gate passes
  ([`evidence/m8a`](../../../evidence/m8a/README.md)). The WSL tools are
  `unverified` at T2 pending a WSL2 runner. 8b (incus) done 2026-10-16:
  T1 and T2 parity green with a nested Incus, the Go `test/live` oracle
  matches Go's own outcome, the seam verifier reports no gap, and the m8b
  gate passes ([`evidence/m8b`](../../../evidence/m8b/README.md)). 8c
  (kubernetes/cluster) done 2026-10-17: T1 and T2 parity green against the
  pinned k3s provider and a real k3s, registry and helm in the twins; the
  seam verifier reports no gap, and the m8c gate passes
  ([`evidence/m8c`](../../../evidence/m8c/README.md)). 8d (oci) done
  2026-10-18: T1 and T2 parity green with real rootless podman and buildah
  pushing only to a registry in the twin's k3s; the seam verifier reports no
  gap, and the m8d gate passes
  ([`evidence/m8d`](../../../evidence/m8d/README.md)). 8e (postgres)
  done 2026-10-20: T1 and T2 parity green against the pinned k3s provider
  and a real CloudNativePG cluster in the twins, with D24 marking the relay
  token write-only; the seam verifier reports no gap, and the m8e gate
  passes ([`evidence/m8e`](../../../evidence/m8e/README.md)). 8f
  (llm/serving) done 2026-10-22: T1 and T2 parity green with a CPU-only
  model stub, real `systemd --user` units, a real relay and a real serving
  unit in the twins; the seam verifier reports no gap, and the m8f gate
  passes ([`evidence/m8f`](../../../evidence/m8f/README.md)). 4.1 stays
  open only for the three WSL tools, `unverified` at T2 pending a WSL2
  runner.
- [x] 4.2 Implement recipe validation, canonical hashing, typed plan
  extraction, one executor, readiness, bounded retry, compensation, and
  recovery. Gate: Go/Rust fixtures agree on plans, node state transitions,
  errors, cancellation, and externally observed outcome; no provider adds a
  second workflow runner.
- [x] 4.3 Reproduce durable operation, task, plan, generation, and observation
  semantics with schema-derived redaction. Gate: restart and secret-bearing
  fixtures preserve truthful state and leave no credentials in records, logs,
  or client-visible evidence; unknown projection fails closed.
- [ ] 4.4 Build a versioned, dry-run Go-state conversion and a rollback
  procedure using read-only copies. Gate: counts, identities, hashes, terminal
  and in-flight state, revisions, permissions, and redaction reconcile; an
  ambiguous record blocks conversion. Never allow two writers on one state
  directory or discard Rust-era accepted work on rollback.
  *M12a note (2026-10-24), partial: the box stays unticked.*
  - **Proven by the `m12a` gate (`evidence/m12a`):** no conversion is
    needed (Rust starts on Go's stopped state as is); forward and rollback on
    checksummed copies keep every task, Rust-era work included, and the
    snapshot is never written.
  - **Still open:** the writer lock. The pinned Go agent takes none, so a
    refusal needs its own OpenSpec change and owner decision. Real-host state
    copies were not used.

Evidence for 4.2 and 4.3: [native validation](../../../evidence/local-e2e/native-2026-10-05/README.md), with provenance-bound M5/M6 gates and explicit owner acceptance of M6. Subsequent domain operations remain open under 4.1.

## 5. Prove the distributed operator surface

- [ ] 5.1 Reproduce Linux and WSL2 packaging, archive names/checksums,
  installer/systemd behavior, and the published npm launch path at their
  owning repositories. Gate: clean-machine install, launch, stop, and upgrade
  runs match the pinned Go operator contract and preserve explicit identity
  and authentication.
  *M10 note (2026-10-24), Linux done, still open: the box stays unticked.*
  - **Proven by the `m10` gate (`evidence/m10`):**
    - Archive names and checksums.
    - Install, launch, stop, uninstall and Go → Rust → Go on clean Ubuntu
      24.04 and Debian 12 twins, against Go.
    - The npm launch path from a locally packed tarball.
  - **Still open, as owned gaps:**
    - WSL2 packaging and install, and the Windows artifact.
    - The arm64 clean-machine run.
    - The published npm path at its owning repository: nothing is
      published before cutover.
- [ ] 5.2 Update public tutorial, release catalog, schemas, and website claims
  only when a tested Rust package and catalog revision are ready. Keep public
  image publishing and private site rollout in their current owners. Gate:
  published-package read-only canary, generated-site validation, and live
  first-success client run agree with the selected artifact.
  *M10 note (2026-10-24), not started: no public claim changed.*
  - **Proven by M10:**
    - Generated-site validation: Go's `check-site` passes, and the Rust
      preview site equals Go's after the declared renames.
    - The local readonly canary against the Rust artifact.
  - **Blocking:** Go's release policy refuses to publish the Rust catalog
    under the stable `0.2.3` package, so a tested Rust package needs a new
    version and catalog decision first.
- [ ] 5.3 Exercise Platform registration, routing, authorization boundary,
  exact agent ID, task correlation, and semantic-outcome separation using a
  dedicated Rust validation identity. Gate: an actual client and Platform
  E2E run show typed execution and durable evidence without attributing a
  Platform or model decision to the Host Agent.

## 6. Decide cutover from complete evidence

- [ ] 6.1 Run differential Go/Rust contract, CLI, MCP wire, provider, plan,
  durable-state, package, client, and isolated external-effect suites against
  the selected current Go revision. Gate: the provenance-bound verifier fails
  on any missing, stale, malformed, redacted-away, or failing item; blocked
  live checks remain unverified.
  *M12a note (2026-10-24), partial: the box stays unticked.* Every
  contract, CLI, wire, provider, plan, durable-state, package and isolated
  T2 suite reran against the rebuilt release artifact (`evidence/m12a`).
  The client suites need M11, and `parity-verify-cutover` still fails on
  its owned gaps.
  *M12b note (2026-11-01), partial: the box stays unticked.* The 15-minute
  Go/Rust soak passed (`evidence/m12b`, the `m12b` gate): equal outcome
  classes, latency and growth within the declared thresholds, no journal
  errors, and equal final state. The client suites still need M11.
- [ ] 6.2 Rehearse stop, snapshot, convert, start Rust, resume/reconcile,
  restart, and rollback on isolated state with a documented in-flight-work
  policy and bounded rollback window. Gate: both forward and permitted reverse
  paths preserve accepted operations and exact identity.
  *M12a note (2026-10-24), partial: the box stays unticked.*
  - **Proven by the `m12a` gate:** stop, snapshot, start Rust, restart and
    roll back to Go on isolated copies, with the exact identity and every
    accepted operation kept. The in-flight policy is Go's: drain in-flight
    requests, cancel running tasks and wait for them (D12 for tasks still in
    binding).
  - **Still open:** the bounded rollback window is a cutover decision (M13),
    and the rehearsal on staging needs M11.
- [ ] 6.3 Review the complete manifest with Host Agent, Platform, release,
  and deployment owners before any production selection. Gate: Go remains the
  production owner until the whole-agent gate passes and the responsible owner
  explicitly authorizes cutover through its normal rollout path.
- [ ] 6.4 After the rollback window, make a separate, evidenced retirement
  decision for Go and a separate ownership decision for releases and the
  website. Gate: no Go artifact or registration is removed by this spec alone.

## 7. Revise delivered capabilities onto the three-role seam

- [x] 7.1 Revise every capability delivered in M1–M7 onto the three-role
  seam (decision D14). Tasks and gates live in
  [`three-role-capability-seam`](../three-role-capability-seam/tasks.md),
  section 4. Gate: the seam verifier reports no gap and every affected
  milestone's parity captures rerun green. By owner decision D14 this gates
  4.1: M8 does not start until it is done. Done 2026-10-11: the seam
  verifier reports no gap and gates m0–m6 pass (`evidence/seam/README.md`).
