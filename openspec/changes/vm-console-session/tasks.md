## 1. Decision

- [x] 1.1 Owner decisions: the serial console (D38) and pull output (D39). Decided 2026-11-11.
- [x] 1.2 Add D38 and D39 to the migration's owner decision table.

## 2. Implementation

- [x] 2.1 A console service on the D14 seam offering the three tools; the seam verifier reports them compliant.
- [ ] 2.2 `send_console_input`'s output schema gains `output` and `closed`; the catalog revision is re-pinned.
- [x] 2.3 Unit tests against a stand-in console, failing first: one session per instance, pull and the 64 KiB bound, close, resize, shutdown.
- [x] 2.4 Console output joins `evidence/hardening/entry-points.json`, covered by a fuzz target or property.

## 3. Evidence

- [ ] 3.1 A console journey on a dev-VM guest: open, read the prompt, resize, detach, no process left, run on the 0.4.0 candidate.
