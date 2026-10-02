# Tasks: standalone read-only gate (D10)

## 1. Rust

- [x] 1.1 `catalog::standalone_read_only`: published tools by descriptor
  effect; unpublished tools only with a declared `read` effect; the Go
  standalone mutation list always refused. Unit test pins the outcome for
  every tool in both catalogs.
- [x] 1.2 `tools::call` applies it at the position of Go's gate, with Go's
  error result.

## 2. Harness and evidence

- [ ] 2.1 Decision D10 in `parity-manifest.json` and the milestones decision
  table.
- [ ] 2.2 Contract suite `standalone-read-only-gate`: sweep the published
  catalog (every non-read tool refused, every read tool not refused), the
  unpublished tools, mutations allowed, stale revision precedence, and no
  state rows afterwards.
- [ ] 2.3 Rust canaries: a name-list gate, an inferred-read gate for
  unpublished tools, and a gate that ignores the mutation list must each turn
  the suite red.
- [ ] 2.4 Go-vs-Rust stays green on every surface, and the catalog revisions
  are unchanged.
