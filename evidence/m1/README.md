# M1 evidence: Rust skeleton, CLI, configuration, lifecycle

This file records M1 from
[milestones.md](../../openspec/changes/reimplement-host-agent-in-rust/milestones.md)
(tasks 1.1, 1.3 and 2.1). The evidence is in [`evidence/current/`](../current/).
Reproduce it with `make parity-m1` (Rust 1.94.1, Go 1.24.7, Python 3.11).

## Gate results

| Check | Result |
| --- | --- |
| `make rust-check` | `cargo fmt --check`, `clippy -D warnings`, 31 unit tests: pass |
| `make parity-test` | 35 harness tests (comparator plus verifier negative tests, including surface-scoped gates and oracle checks): pass |
| Go vs Go, ×20 | 30/30 scenarios clean on all 20 iterations |
| Canaries | 6/6 caught, each by its named scenario only |
| **Go vs Rust, ×5** | **17/17 CLI, config and lifecycle items pass** on all 5 iterations. The other 13 items (MCP wire, auth, catalog, health, read-only tools, admission, state after an MCP call) belong to M2 to M5 and fail as expected, because this build answers HTTP with 501. |
| Go oracle tests against Rust | `TestStandaloneInvalidConfigurationExitsBeforeMCP` and `TestDeprecatedTransportFlagIsRejectOnly` pass against Go and Rust; the `/bin/true` negative control fails them, which proves the overlay runs the given binary |
| `make parity-verify-m1` | **PASS** |
| `make parity-verify-cutover` | **FAIL, by design** (13 later-milestone items) |

## What the Rust agent does at M1

`crates/host-agent` is one crate. Per design.md, crates are split only at a
real dependency boundary, and M1 has none yet.

| Module | Scope |
| --- | --- |
| `goflag` | Go `flag` semantics: `-x`/`--x`/`-x=v`/`-x v`, `--`, bool flags, `-h`, `PrintDefaults` layout and `(default …)` rendering, and error text printed then returned (`failf`) |
| `cli` | Command dispatch identical to `internal/cli`: the implicit `serve`, `standalone`, `--version`. `recipe`, `provider` and `public-mcp` validate arguments, token files and configuration exactly as Go does, then report that the operation is planned for M6, M7 or M8a |
| `config` | `config.Load`/`Validate` with Go's defaults (standalone 127.0.0.1:3014, platform 0.0.0.0:3004), `envValue` quote stripping, `LoadEnvFile` precedence (it never overrides a non-blank variable), `Atoi` overflow clamping, `ParseCIDR`, and float NaN/Inf. The process environment is **never mutated**: the CLI builds an explicit overlay, which fixes legacy-inventory I-17 without changing behavior |
| `identity` | The `/etc/machine-id` fingerprint, WSL execution context, Windows MachineGuid through interop with the Go cache file |
| `store` | `state.db` and `authz.sqlite` opened with DDL **copied verbatim from Go** (`ddl.rs`), the same migrations and ALTER order, built-in OAuth clients upserted, and the WAL checkpointed on close |
| `app` | Ordered startup: validate → coordinator lock dir (0700) → state → authz → "HTTP transport listening" → bind. Reverse-order cleanup on failure; SIGINT/SIGTERM exit 0 |

## New scenarios (11 added, 30 total)

- `cli.dispatch`: 42 invocations. Implicit/explicit/padded commands, `--version`
  with extra arguments, `--`, positional stop, bad syntax, invalid bool, a
  missing argument, `-h`/`--help`/`-help=1` per flag set, and every
  pre-runtime error path of `recipe`, `provider` and `public-mcp` (token
  file missing, loose permissions, empty, absent, a directory).
- `config.env-precedence`: 17 invocations comparing env file vs process env vs `--env` vs
  `OPUTE_HOST_AGENT_ENV_FILE`, blank values, quoting, and mode precedence
  (flag > env > default standalone).
- `config.validation-matrix`: 41 `--check` cases covering every `Validate`
  branch that is reachable through the environment.
- `lifecycle.*`: standalone serve and platform serve stopped with SIGTERM
  (listening sockets, files and modes, SQLite schema and row counts, server
  log), and restart idempotence. Fault injection covers a blocked lock dir, a
  blocked state dir, a state dir that is a file, `state.db` or `authz.sqlite`
  as a directory, the port in use, `::1:PORT` ("too many colons") and port
  70000. Every fault case also asserts that no listener ever appears.
- `defaults.*`: the default bind host per mode, default ports 3014 and 3004
  (run exclusively), quoted bind host and port, and the state location under
  XDG, HOME, a named instance and an instance root.

## What validation caught (and what changed)

1. **SQLite open-error text.** Go's modernc driver prints `unable to open
   database file (14)`, while rusqlite adds the path. The Rust store now
   reports SQLite's own message.
2. **A harness port race.** Go vs Go failed in one of 20 iterations (2 of
   600 comparisons, both in the same iteration). The
   harness asked the kernel for port 0, which a concurrent sandbox or an
   outgoing client connection could reuse. Sandbox ports now come from a
   reserved range below the ephemeral range and are handed out once. The
   runner also keeps the first failing iteration's diff, so a rare flake
   stays diagnosable from evidence.
3. **Shutdown log.** Lifecycle scenarios now compare the server log with
   timestamps stripped. Go logs only the "listening" line, and so does Rust.

## Harness additions

- Steps: `write`, `mkdir`, `occupy`, `listeners` (reads `/proc/<pid>/fd` and
  `/proc/net/tcp*`). Scenarios can be `exclusive` for fixed ports. New
  `serverLog` collection. `run --surfaces`.
- `parity oracle`: runs Go's black-box tests against any binary through a
  recorded overlay (`tools/parity/oracles.json`), always with a negative
  control.
- Verifier: `requirePass` can be scoped to surfaces. `requireOracles` checks
  that both recorded binaries pass and that the control fails. A new `m1`
  gate.
- CI: `.github/workflows/ci.yml` runs `rust-check`, `spec-validate` and
  `parity-ci`. `parity-ci` verifies the committed evidence, then re-proves
  M1 parity and the oracle tests with a freshly built Rust binary.

## Still open for later milestones

- HTTP routes (`/health`, `/mcp`, OAuth) arrive in M2. Until then the
  listener answers 501.
- The M1 spec item "SIGINT/SIGTERM during a slow shim call" needs a
  long-running tool call, which comes with M4 (tasks and cancellation).
- `recipe`, `provider` and `public-mcp` operations: M6, M7 and M8a.
- Logs use UTC timestamps. The harness strips the timestamp, and Go uses
  local time, which is also UTC under systemd defaults.
