## 1. Decision

- [ ] 1.1 Owner: option A (wire the three commands) or B (withdraw them and their docs).

## 2. Tests first (after the decision)

- [ ] 2.1 CLI tests that fail against 0.5.2: each command calls its tool and prints its result (A), or is unknown (B).

## 3. Change

- [ ] 3.1 A: in-process tool calls in `cli.rs`. B: remove the commands, the help text and the docs page lines.

## 4. Evidence

- [ ] 4.1 A fresh-user run of each command on the dev VM against a release candidate.
