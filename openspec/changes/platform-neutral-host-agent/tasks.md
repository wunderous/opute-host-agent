# Tasks: platform-neutral Host Agent

Target 0.4.0. Merge only after the 0.3.0 cutover and its 14-day window.

## 1. Decision

- [x] 1.1 Record the owner decision (Host Agent owner, 2026-11-08).
- [ ] 1.2 Record the Platform, release and deployment owners' sign-off.
- [x] 1.3 Owner answers N1-N7 (design.md): every recommendation adopted
  (2026-11-08).

## 2. Failing-first tests

- [x] 2.1 One serving profile: defaults, an exposed host, the effective
  configuration logged once.
- [x] 2.2 The old variable has no effect: `OPUTE_AGENT_MODE=platform` alone
  starts on the defaults, and nothing printed mentions it. Tests
  (`config.rs`, `app.rs`) were red first: missing field, then a missing
  profile log line.
- [x] 2.3 OAuth clients from `OPUTE_OAUTH_CLIENTS` only; none provisioned by
  default; bearer works; an unlisted client is deprovisioned; a probe with
  no client reports unverified (deprovision shown red by mutation).
- [x] 2.4 Reserved names from `OPUTE_RESERVED_HOSTNAMES` only; refused with
  `reserved_hostname` before any Cloudflare call. Shown red by breaking
  the suffix match (2 of the 4 tests fail).
- [x] 2.5 The boundary test in `make rust-check` (`boundary.rs`). Red
  first (28 findings), enabled once 3.1-3.3 landed; a Platform hostname put
  in a provider fails it with file, line and identifier.

## 3. Implementation (one concern per commit)

- [x] 3.1 The single profile.
- [x] 3.2 Remove mode reading and all its branches (one commit with 3.1:
  the contract suites cannot pass between them). `--mode` is gone (N5): the
  launcher must stop passing it before 0.4.0 ships.
- [x] 3.3 OAuth clients from configuration (N4).
- [x] 3.4 Reserved names from configuration.
- [x] 3.5 Remove Platform names from CLI, help and catalog descriptions,
  with the N6 refusal text and the N7 relay device default.
- [ ] 3.6 Repin the catalog revisions in the manifest (repinned for 3.5;
  again after 3.1).

## 4. Journeys

- [ ] 4.0 Prerequisites outside this repository (owned by the release
  owners): the npm launcher and the recorded deploy unit drop `--mode`; the
  unit adds `OPUTE_HOST_ALLOW_MUTATIONS=true`. The journeys below do not
  wait on them: they start their own 0.4.0 agents with `serve` and
  explicit settings, and leave the harness's guest tool unchanged.

Gate `e2e-neutral` passes on run `e2e-neutral-r2` (2026-11-10), on a
0.4.0 release set built from this branch. r1 is retired: the journeys had
no tools/list step for their own agents.

- [ ] 4.1 e2e-1 runs the single profile only. Partly: `neutral.profile`
  proves the profile's scenarios on 0.4.0 (defaults, a host exposed on
  purpose, the profile log line, the mutation refusal, OAuth clients). The
  e2e-1 to e2e-4 gates still rest on 0.3.x runs. Rerunning them on 0.4.0
  needs the dev VM rebuilt with a deploy unit that has no `--mode` (4.0),
  and is due when this change merges.
- [x] 4.2 A step proves the old variable has no effect (`neutral.profile`:
  with `OPUTE_AGENT_MODE=platform` alone the listen address, the profile
  line, the catalog and the state location equal a plain start, and
  nothing printed names the variable; `OPUTE_STANDALONE_ALLOW_MUTATIONS`
  admits nothing).
- [x] 4.3 Upgrade and rollback across 0.3.x and 0.4.0 with both settings
  sets (`neutral.rollback`, between the two Rust releases; the Go to Rust
  cutover stays in `e2e4.cutover`). Same address, same state, the machine
  client's credential unchanged, 130 then 142 then 130 tools.
- [x] 4.4 The Cloudflare journey proves the reserved-name guard
  (`neutral.reserved`: `reserved_hostname` through the agent; unreserved
  names reach the missing-credentials error).
- [x] 4.5 e2e-5 is redefined in the cutover gate (`gates.json` and the
  contract spec delta); it stays blocked on Platform staging.

## 5. Close

- [x] 5.1 `make rust-check`, `rust-suites`, `spec-validate` and
  `cutover-readiness` pass on this branch (2026-11-10), with the contract,
  seam and arm64 receipts recaptured on the 0.4.0 build. Readiness here
  still counts the e2e-1 to e2e-4 gates from their 0.3.x runs (see 4.1), so
  it does not yet prove those gates for 0.4.0.
- [ ] 5.2 Archive the spec delta into the contract spec.
- [ ] 5.3 PRs merge with CI green, after the 0.3.0 window.
