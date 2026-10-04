# Catalog-derived secret sink sweep

Reproduce with `secret-sweep` in `tools/parity`, passing the pinned Go reference,
Rust candidate and this output directory. Set `TMPDIR` to an owned disk-backed
directory. The driver derives the fields from the actual pinned catalog in
standalone and platform modes: eight fields per mode, 16 unique case canaries.

Test composition seams call actual argument, task, invocation and plan
projections and store methods without executing domain effects. The normal
agent binaries restore the resulting tasks and expose completed result/status
through `tasks/get`. The pinned list/result methods are unsupported; their
captured refusals are checked explicitly.

The raw corpus retains every scanned state file as bytes, including live SQLite
WAL and SHM, plus fixture logs, projected output, server logs and HTTP responses.
The independent verifier checks hashes, recomputes canary absence, derives
coverage from both catalogs, requires all durable writes and exact projected
contents, and compares Go/Rust projections. Empty or missing sinks cannot pass.

This does not settle D13: unmarked fields follow a separate pinned Go behavior
whose conflict with the M5 text requires an owner decision. Nor does this prove
the later M6 executor or M7 provider lifecycle.
