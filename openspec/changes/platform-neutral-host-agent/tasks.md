# Tasks: platform-neutral Host Agent

Target 0.4.0. Merge only after the 0.3.0 cutover and its 14-day window.

## 1. Decision

- [x] 1.1 Record the owner decision (Host Agent owner, 2026-11-08).
- [ ] 1.2 Record the Platform, release and deployment owners' sign-off.
- [ ] 1.3 Owner answers N1-N4 (design.md).

## 2. Failing-first tests

- [ ] 2.1 One serving profile: defaults, an exposed host, the effective
  configuration logged once.
- [ ] 2.2 The old variable has no effect: `OPUTE_AGENT_MODE=platform` alone
  starts on the defaults, and nothing printed mentions it.
- [ ] 2.3 OAuth clients from `OPUTE_OAUTH_CLIENTS` only; none provisioned by
  default; bearer works.
- [ ] 2.4 Reserved names from `OPUTE_RESERVED_HOSTNAMES` only; refused with
  `reserved_hostname` before any Cloudflare call.
- [ ] 2.5 The boundary test in `make rust-check`.

## 3. Implementation (one concern per commit)

- [ ] 3.1 The single profile.
- [ ] 3.2 Remove mode reading and all its branches.
- [ ] 3.3 OAuth clients from configuration (after N4).
- [ ] 3.4 Reserved names from configuration.
- [ ] 3.5 Remove Platform names from CLI, help and catalog descriptions.
- [ ] 3.6 Repin the catalog revisions in the manifest.

## 4. Journeys

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
