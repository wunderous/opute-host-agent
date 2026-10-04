# Tasks: behavior-preserving Rust Host Agent

These are ordered implementation gates, not a claim that implementation has
started. A task is complete only when its named artifact and boundary-level
evidence are committed. The pinned Go revision is the initial baseline; rebase
it deliberately if the production Go contract changes. Do not import the
abandoned Rust port.

[milestones.md](milestones.md) groups these gates into ordered milestones and
defines the differential end-to-end harness and evidence each one requires.

## 0. Freeze and enumerate the behavior to preserve

- [ ] 0.1 In the new repository, create a machine-readable parity manifest
  keyed to `baseline/source-lock.json`: source SHA/tree, schema/catalog
  revisions, mode, active provider set, tested artifact versions, and evidence
  run IDs. Gate: an absent, stale, or mismatched key makes the verifier fail.
- [ ] 0.2 Inventory the Go CLI, flags, environment precedence, defaults,
  startup/shutdown behavior, endpoints, auth modes, protocol methods, errors,
  installation, systemd, npm launcher, Linux archives, and operator commands.
  Own the capture in `baseline/`; compare it against Go source/tests and a
  packaged Go binary. Gate: every public surface has a named owner and test.
- [ ] 0.3 Capture authenticated `tools/list` and schema projections from the
  pinned Go binary for each supported serving mode and each configured
  provider fixture. Record conditional availability, effects, approval,
  idempotency, resource bindings, and catalog revision. Gate: no descriptor
  is accepted solely because it appears in a static list or count.
- [ ] 0.4 Inventory recipes, plan node states, retries, readiness,
  compensation, callbacks, Tasks/cancellation, provider generations, and all
  local durable stores. Record Go fixtures and expected wire/state outcomes.
  Gate: unknown state or unsupported fixture remains an explicit gap.
- [ ] 0.5 Review the new parity-gate invariant against the Go ADRs, the
  Platform owner, and the current public release parity decision. Anchor its
  typed decision and fail-closed verifier in this repository before writing a
  Rust runtime seam. Gate: preserved, introduced, and retired invariants have
  an owner, exception path, revision rule, and checkable evidence.

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

- [ ] 3.1 Implement provider manifest/schema/dependency validation and
  candidate, active, and draining generation transitions. Gate: failed
  candidate activation leaves the prior catalog and provider usable;
  generation and catalog revisions change only under the Go contract.
- [ ] 3.2 Implement generation-bound admission, callback primitive routing,
  readiness, task bridge, cancellation, and bounded disposal. Gate: wire and
  integration fixtures cover replacement during in-flight work, timeout,
  cancellation, stale revision, callback rejection, and reverse-order cleanup.
  Progress: the admission part (stale catalog revision, the host resource
  coordinator, reservation leases), the task bridge for built-in tools
  (`tasks/get`, `input_required`, cooperative cancellation) and timeouts are
  done in M4 ([evidence/m4/README.md](../../../evidence/m4/README.md)).
  Provider generations, callback routing, readiness, replacement during
  in-flight work and bounded disposal remain open for M7.
- [ ] 3.3 Port every supported provider integration and conditional catalog
  projection behind neutral typed contracts. Gate: the equivalent configured
  Go and Rust fixture has no missing, extra, or differently shaped tool; an
  unavailable provider is reported truthfully, not silently omitted.

## 4. Prove execution and durable-state parity

- [ ] 4.1 Port all supported host primitives and domain operations behind
  typed seams; maintain `vm:` versus `container:` target separation. Gate:
  focused contract tests and isolated external-effect comparisons cover
  success, failure, cleanup, and absence of unauthorized side effects.
- [ ] 4.2 Implement recipe validation, canonical hashing, typed plan
  extraction, one executor, readiness, bounded retry, compensation, and
  recovery. Gate: Go/Rust fixtures agree on plans, node state transitions,
  errors, cancellation, and externally observed outcome; no provider adds a
  second workflow runner.
- [ ] 4.3 Reproduce durable operation, task, plan, generation, and observation
  semantics with schema-derived redaction. Gate: restart and secret-bearing
  fixtures preserve truthful state and leave no credentials in records, logs,
  or client-visible evidence; unknown projection fails closed.
- [ ] 4.4 Build a versioned, dry-run Go-state conversion and a rollback
  procedure using read-only copies. Gate: counts, identities, hashes, terminal
  and in-flight state, revisions, permissions, and redaction reconcile; an
  ambiguous record blocks conversion. Never allow two writers on one state
  directory or discard Rust-era accepted work on rollback.

## 5. Prove the distributed operator surface

- [ ] 5.1 Reproduce Linux and WSL2 packaging, archive names/checksums,
  installer/systemd behavior, and the published npm launch path at their
  owning repositories. Gate: clean-machine install, launch, stop, and upgrade
  runs match the pinned Go operator contract and preserve explicit identity
  and authentication.
- [ ] 5.2 Update public tutorial, release catalog, schemas, and website claims
  only when a tested Rust package and catalog revision are ready. Keep public
  image publishing and private site rollout in their current owners. Gate:
  published-package read-only canary, generated-site validation, and live
  first-success client run agree with the selected artifact.
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
- [ ] 6.2 Rehearse stop, snapshot, convert, start Rust, resume/reconcile,
  restart, and rollback on isolated state with a documented in-flight-work
  policy and bounded rollback window. Gate: both forward and permitted reverse
  paths preserve accepted operations and exact identity.
- [ ] 6.3 Review the complete manifest with Host Agent, Platform, release,
  and deployment owners before any production selection. Gate: Go remains the
  production owner until the whole-agent gate passes and the responsible owner
  explicitly authorizes cutover through its normal rollout path.
- [ ] 6.4 After the rollback window, make a separate, evidenced retirement
  decision for Go and a separate ownership decision for releases and the
  website. Gate: no Go artifact or registration is removed by this spec alone.
