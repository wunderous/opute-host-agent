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
  unit adds `OPUTE_HOST_ALLOW_MUTATIONS=true`. The e2e harness's guest tool
  stops passing `--mode` (old binaries default to the same profile).

- [ ] 4.1 e2e-1 runs the single profile only.
- [ ] 4.2 A step proves the old variable has no effect.
- [ ] 4.3 e2e4.cutover rolls back across 0.4.0 and 0.3.x with both settings
  sets.
- [ ] 4.4 The Cloudflare journey proves the reserved-name guard.
- [ ] 4.5 e2e-5 is redefined in the cutover gate.

## 5. Close

- [ ] 5.1 `make rust-check`, `rust-suites`, `spec-validate` and
  `cutover-readiness` pass.
- [ ] 5.2 Archive the spec delta into the contract spec.
- [ ] 5.3 PRs merge with CI green, after the 0.3.0 window.
