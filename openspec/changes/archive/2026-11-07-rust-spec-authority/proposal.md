# Rust is specified by its contract, not by Go parity (D27)

## Why

The migration has a complete parity record. Every parity gate passed against
the pinned Go v0.2.4 baseline:
- m0–m6, m8a–m8f, m10, m12a, m12b;
- d25 (the writer lock);
- m9 (the provider executables).

The owner decided (2026-11-04) that the Rust Host Agent is now developed
independently, against its design requirements and specs. Go stops being the
behavioural authority for new work. No new Go baseline is captured, and Rust
is no longer compared with Go.

Most of the contract spec still says "as Go does", and the cutover decision
still requires boundary-matched Go parity evidence. Those statements would
block every change that improves on Go, and they keep a Go toolchain in CI
only to compare against. This change moves the authority to the spec and
its frozen contract record. Where the record does not decide a behaviour, it
names an open question.

## What Changes

- **Authority.** The contract spec and the accepted decisions become the
  authority, together with the frozen v0.2.4 contract record:
  - `baseline/inventory`: catalogs per mode and provider, CLI,
    environment, HTTP, effects, state and plans;
  - `baseline/source-lock.json`;
  - the Rust-only contract suites in `tools/parity/contracts/`.

  Each contract statement that read "as Go does" now reads "as the frozen
  contract record and the accepted decisions define". A behaviour the
  record does not capture is an open question in
  [questions.md](questions.md). It is not settled by consulting Go.
- **Scope rule.** The rule against new capabilities stays. A new capability,
  endpoint, mode, provider, auth path, default or user-visible policy still
  needs its own accepted OpenSpec change.
- **Cutover.** The cutover requirement changes from Go parity evidence to
  spec-based evidence:
  - the E2E journey gates (e2e-1 to e2e-5);
  - the Rust-only contract suites;
  - the state-conversion and rollback evidence;
  - the owners' review.

  Cutover and Go retirement stay separate decisions.
- **Decision record.** `.agents/decisions/rust-cutover-parity-gate.json` is
  amended (D27): its parity evidence requirement is retired, and the
  spec-based gate replaces it.
- **CI.** The Go/Rust comparison leaves CI, and CI no longer installs Go.
  CI runs the Rust-only suites (`make rust-check`, `make rust-suites`):
  - the Rust unit tests;
  - the harness unit tests;
  - the five contract suites (D8, D10, D11, D12, D13).

  The Rust canaries and the go-sdk OAuth client against the Rust agent
  stay local suites, because they need patched builds and a Go module
  cache.
- **Evidence.** The committed parity evidence (m0–m12b, d25 and m9) stays as
  the historical record, and its gates are not rerun. milestones.md and
  AGENTS.md say so.

## Impact

- **Docs:** AGENTS.md, milestones.md and the reimplement change's design and
  tasks now point at the spec as the authority.
- **CI:** `.github/workflows/ci.yml` and `Makefile` (`rust-suites` replaces
  `parity-ci`).
- **Unchanged:** no runtime behaviour changes, and the published catalog,
  CLI and wire are untouched.
