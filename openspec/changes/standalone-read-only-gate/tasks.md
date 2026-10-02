# Tasks: standalone read-only gate (D10)

## 1. Rust

- [x] 1.1 `catalog::standalone_read_only`: published tools by descriptor
  effect; unpublished tools only with a declared `read` effect; the Go
  standalone mutation list always refused. Unit test pins the outcome for
  every tool in both catalogs.
- [x] 1.2 `tools::call` applies it at the position of Go's gate, with Go's
  error result.

## 2. Harness and evidence

- [x] 2.1 Decision D10 in `parity-manifest.json` and the milestones decision
  table.
- [x] 2.2 Contract suite `standalone-read-only-gate`: sweep the published
  catalog (every non-read tool refused, every read tool not refused), the
  unpublished tools, mutations allowed, stale revision precedence, and no
  state rows afterwards.
- [x] 2.3 Rust canaries K16-K20: a name-list gate, an inferred read for
  unpublished tools, a gate that ignores the allow flag, a gate in platform
  mode, and a gate that refuses reads each turn the suite red. Go's list is
  kept as an explicit refusal; every tool on it has a non-read effect (unit
  test), so no canary can isolate that line.

Evidence: [evidence/standalone-gate/README.md](../../../evidence/standalone-gate/README.md).
- [x] 2.4 Go-vs-Rust stays green on every surface, and the catalog revisions
  are unchanged.
