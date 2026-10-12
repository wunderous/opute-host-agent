## 1. Review

- [x] 1.1 Obtain the owner's decision between option A (extend D12) and option B (match Go). Decided 2026-10-12: option A, recorded as D18.

## 2. Evidence

- [x] 2.1 Capture the unacknowledged-at-shutdown case Go/Rust five times with raw I/O and a verifier that pins the exact difference.
- [x] 2.2 Record the owner decision. Then either declare the divergence for the public suite (A) or change Rust and move the case into the public suite (B). Done: `D18.unacknowledged-callback-child` in `tools/parity/divergences.json`, and the case is in the public provider-lifecycle suite with per-side expectations (Go/Go 3/3, Go/Rust 3/3).
