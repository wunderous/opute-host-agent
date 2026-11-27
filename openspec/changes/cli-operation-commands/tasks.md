## 1. Decision

- [x] 1.1 Owner: option A (wire the three commands), 2026-11-26.

## 2. Tests first

- [x] 2.1 `tests/cli_operations.rs`: each command calls its tool and prints its result, a mutating call needs `--allow-mutations`, a tool error exits non-zero with no JSON, and a held instance refuses. Six of the seven fail against 0.5.3's code (the instance-lock guard passes on both).

## 3. Change

- [x] 3.1 In-process tool calls in `cli.rs` through `app::with_local_server`; the help text names `--allow-mutations` and the instance rule.

## 4. Evidence

- [x] 4.1 A fresh-user run of each command on the dev VM against `rust-0.5.4-rc1`: `evidence/m31` (`public-mcp --allow-mutations` needs a real tunnel token and is not run).
- [ ] 4.2 host-agents' Configuration page names `--allow-mutations` for the three commands, in the release that ships them.
