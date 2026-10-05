# M6: recipes and the single plan executor

**In progress, not done. `make parity-verify-m6` does not exist yet and no
parity-harness evidence has been collected for M6.** Everything below is
proven at the Rust unit-test level (171 tests passing, `cargo fmt --check`
and `cargo clippy -- -D warnings` clean) and, for the executable paths, by
real end-to-end runs against a durable SQLite store inside those tests --
not yet by a Go-vs-Rust byte-for-byte comparison, which is what the M6 exit
gate in `milestones.md` actually requires.

## What is implemented

- **The single plan executor** (`crates/host-agent/src/plan/`): a full port
  of Go's `internal/plan` package -- `schema.rs` (document types, canonical
  JSON, `document_hash`, the static validator, interpolation-reference
  checks), `graph.rs` (topological levels, cycle rejection), `assert.rs`
  (assertion/readiness evaluation), `interpolate.rs` (variable/reference
  resolution, Go `fmt.Sprint`-parity string interpolation), and `runner.rs`
  (the executor itself: dispatch, bounded retry with backoff, compensation
  on permanent failure, recovery, `forEach` fan-out with true concurrent
  admission via a hand-rolled semaphore, and the durable wait/resume
  barrier). 46 unit tests across these five files, including fault-injected
  cases: a node that fails once and retries successfully, permanent failure
  with compensation in the correct order, a cycle rejected before any
  dispatch, cancellation before dispatch, and wait-then-resume.
- **The `host-recipe.v1` envelope** (`crates/host-agent/src/recipe/`):
  source loading (local file, `github:` reference with 40-char commit
  pinning, raw GitHub URL, with redirect validation and optional sha256
  verification), input resolution against declared `InputSpec`s, the
  host-local restrictions (no wait nodes, a single target host, no emitted
  events -- Go's `ValidateHostLocalPlan`/`rejectHostLocalEventBindings`),
  and `ValidateHostAgentVersion`'s numeric major/minor/patch comparison
  (ported to `recipe/source.rs`, fails closed on an unparseable version the
  way Go's comment says a development build honestly must). 18 unit tests.
- **Plan/recipe evidence redaction** (`crates/host-agent/src/plan_evidence.rs`,
  new this session): a port of `internal/hostmcp/evidence_redaction.go`'s
  plan-specific pieces -- `redact_evidence_by_schema` (schema-driven,
  `writeOnly`-aware, fails closed on an unknown tool's arguments),
  `redact_plan_evidence`/`redact_plan_document` (secret-input-name-aware
  plan-document projection), `redact_plan_run_state` (durable `RunState`
  projected through each node's own capability output schema, with
  `secret`-marked context entries withheld entirely), and
  `recipe_secret_name_set`/`redacted_metadata`/`recipe_hash`/
  `persisted_recipe_hash`. 5 unit tests.
- **The MCP surface**, all routed through the one `Runner`
  (`crates/host-agent/src/plan_mcp.rs`, `crates/host-agent/src/
  host_recipe_mcp.rs`):
  - `validate_host_plan` -- pure, no persistence.
  - `run_host_plan` / `get_host_plan_run` -- durable, async, with the
    `FindPlan`/`CreatePlan` idempotency dance, catalog-revision
    reconciliation, and real end-to-end execution against SQLite
    (`store.rs`'s `PlanRecord`/`update_plan`/`create_plan`).
  - `validate_host_local_recipe` / `run_host_local_recipe` -- sourcing,
    input resolution, plan expansion, the host-local-targets-only-this-host
    check (`plan::interpolate::interpolate_args` resolving each node's
    `${vars.inputs...}` target reference), then handing the expanded plan to
    the exact same `handle_run_host_plan_with_metadata` the bare path uses.
  - `handle_run_host_plan` was generalized into `handle_run_host_plan_with_
    metadata(server, args, recipe_metadata: Option<&Map>, task_name,
    task_description)` so the bare path and the recipe path are one
    function, not two maintained in parallel -- `recipe_metadata: None`
    collapses every redaction helper to a no-op.
  - 8 `plan_mcp` tests and 3 `host_recipe_mcp` tests run real plans to
    completion against a `transport::Server` built through the actual
    startup path (`app::new_runtime` -> `AuthzStore::open` ->
    `app::http_server`), including one that proves a declared secret input
    never reaches the task's stored arguments, the final run result, or the
    durable `plan_json`/`recipe_json` columns.
- **The structural single-executor test**
  (`crates/host-agent/src/structural_test.rs`): a static, text-scanning test
  in the exact style of Go's own `test/contract/architecture_test.go` -- it
  fails if any file outside `plan/runner.rs` (the executor's own module) and
  `plan_mcp.rs` (the one production call site, `spawn_plan_execution`)
  constructs a `plan::runner::Runner`. Verified to actually fail (not
  vacuously pass) by temporarily inserting a `Runner {` literal into an
  unrelated file and confirming the test catches it, then reverting.

## What is not implemented

- **The recipe-corpus parity harness** (the M6 exit gate's actual
  requirement): a Go-vs-Rust driver comparing `validate`'s outcome and
  canonical hash byte-for-byte across every recipe in `plugins/*/recipes/**`
  and `test/fixtures/**`, plus a synthetic, credential-free shape of the
  private site-deploy recipe. No such driver exists in `tools/parity/parity/`
  yet, and none of the existing recipe fixtures under those paths have been
  inventoried against the new Rust validator.
- **The invalid-recipe corpus**: generated mutations (bad binding, unknown
  node, cycle, wrong schema version, stale catalog revision, duplicate IDs,
  interpolation type errors) proven to produce an identical typed error on
  both sides and pass X2. Not started.
- **Execution-trace comparison**: the ordered node state-transition log and
  shim trace compared against Go for the four named fault scripts (retry
  succeeds, permanent failure with compensation order, readiness-never-true
  timeout, cancel mid-node). The Rust side's behavior for all four is
  covered by `runner.rs`'s own unit tests; none of it has been compared
  against a Go reference run.
- **Restart-mid-plan / reconcile comparison**: kill during node *k*, restart,
  run reconcile, and confirm the resume/compensate/stop decision and final
  durable state match Go's. Not started. (The Rust side's wait/resume
  machinery -- `Runner::resume`, `validate_resume` -- exists and is unit
  tested, but there is no Rust-side `reconcile` entry point wired to MCP
  yet, and no restart scenario has been exercised.)
- **The resource-reservation lease** around a launched plan run (Go's
  `claimReservationLease`/`lease.bindTask`, and the reservation-ownership
  inheritance rules in `handleRunHostPlanWithMetadata`): not ported. A plan
  run today is admitted and audited like any other capability call but does
  not itself hold or inherit a reservation, and `cancelHostPlan` (Go's
  `tasks/cancel`-adjacent plan-cancel path) has no Rust counterpart yet.
  Both gaps are noted in `plan_mcp.rs`'s module doc comment so they are not
  silently absorbed.
- **M7-shaped provider-activation branches** inside
  `handleRunHostPlanWithMetadata` (`activation`, `providerTeardownInputs`,
  `activateCompletedProviderCandidate`) and the `runtime-recipe.v1` /
  `tunnel-recipe.v1` families themselves: out of M6 scope by the plan's own
  split (M7 can run in parallel with M6), correctly left unported.
- **The CLI `recipe` surface**: Go's `cli.go`'s `runRecipe` only supports
  `--kind runtime|tunnel` (never `host` -- host-recipe has no CLI surface in
  Go), so the Rust `cli.rs` scaffold's existing `--kind` choices are already
  correct and need no change for host-recipe; the runtime/tunnel
  dispatch-to-MCP-tool pass-through itself has not been implemented (those
  underlying MCP tools are M7-scoped and currently report
  `not_implemented`).
- **The M6 parity-verify gate**: no `parity-verify-m6` Make target, no
  `gate: "m6"` entry in the Python verifier, and therefore no possibility of
  an actual reported pass yet. `make parity-verify-m6` does not exist as a
  command today.

## Divergences

None declared yet. No Go-vs-Rust comparison has been run, so no divergence
has had the opportunity to surface. The two gaps above (resource-reservation
lease, plan-cancel) are implementation gaps, not behavioral divergences --
there is no Rust behavior yet to compare against Go's, so neither needs (or
could yet have) a D# decision, an OpenSpec change, or a Rust-canary
contract. One will be needed if and when either gap is closed with an
intentionally different Rust behavior rather than a straight port.
