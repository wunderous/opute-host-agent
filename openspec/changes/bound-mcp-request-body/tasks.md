## 1. Decision

- [x] 1.1 Owner decision between bounding the read and recording the finding. Decided 2026-11-10 as D29: bound it.
- [x] 1.2 Add D29 to the migration's owner decision table.

## 2. Implementation and evidence

- [x] 2.1 Read at most 4 MiB in `transport::mcp`; a larger body gets the SDK's 413 before the envelope.
- [x] 2.2 Failing-first test: a body declared as 64 MiB, of which 5 MiB is sent, is answered 413 without waiting for the rest; a 4 MiB body is read.
- [ ] 2.3 Gate e2e-6 checks the 413 and the agent's memory on the release candidate.
