# Deterministic storage crash corpus

Reproduce with the `storage-crash` command in `tools/parity`, passing the pinned
Go reference, Rust candidate, `--seeds 200`, and this output directory. Set
`TMPDIR` to an owned disk-backed directory with sufficient space.

The test processes call the real operation, task-snapshot and plan store methods.
Private SQLite triggers hold each selected BEFORE/AFTER statement after spilling
uncommitted WAL pages. Observing the spill proves that the checkpoint was reached
before SIGKILL. The 200 seeds cover 16 checkpoints, with seeded delays after the
spill. Normal agent processes then reopen the database and serve the restored
task through MCP.

Acceptance requires unchanged committed rows, an empty rolled-back trigger
marker, SQLite integrity, the original pending task, interrupted operations and
plans marked unknown, the prior completed plan and active selection preserved,
and identical full recovered schemas/rows and MCP status. All raw observations
are retained; the verifier recomputes these assertions independently.

These are isolated storage tests. They do not prove provider effects, the M6
executor, shared-runtime recovery or production cutover.
