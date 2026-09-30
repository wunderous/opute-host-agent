# Parity harness

The M0 differential harness from
[milestones.md](../../openspec/changes/reimplement-host-agent-in-rust/milestones.md) §2.
It runs the same versioned scenario against two Host Agent binaries, each in
its own sandbox, and compares what a client, an operator, or the host could
observe. It is Python 3.11 standard library only, so it does not depend on
the Rust workspace that M1 creates.

```text
scenario ─┬─► side a: binary + sandbox (own ID/port/token/state, shims) ─┐
          └─► side b: binary + sandbox (own ID/port/token/state, shims) ─┤
                                                                         ▼
       substitute run variables → parse embedded JSON → sets → typed masks → diff
                                                                         ▼
              evidence bundle (raw .json.gz, diff, iterations, hashes) → verifier
```

## Commands

```sh
make go-reference       # build the pinned Go binary; refuses a commit/tree mismatch
make parity-test        # harness unit tests (comparator + verifier negative tests)
make parity-capture     # regenerate baseline/inventory from Go source + live binary
make parity-go-vs-go    # every scenario, Go vs Go, 20 times
make parity-canaries    # build broken Go variants and prove each is caught
make parity-manifest    # sync parity-manifest.json items with the scenarios
make parity-verify      # fail-closed M0 gate
make parity-verify-cutover   # the whole-agent gate (fails until Rust has evidence)
make parity-m0          # all of the above, in order
```

A Rust candidate is compared the same way once one exists:

```sh
cd tools/parity && python3 -m parity run \
  --left go=../../.parity/bin/go-reference --right rust=PATH/TO/BINARY \
  --suite go-vs-rust --out ../../evidence/<milestone>/go-vs-rust
```

## Layout

| Path | What it is |
| --- | --- |
| `parity/agent.py` | Sandbox, CLI runner, server lifecycle, raw HTTP/MCP 2026-07-28 client |
| `parity/shims.py` | Recording shims for host tools (argv, stdin hash, matched rule) |
| `parity/canon.py` | Run-variable substitution, embedded JSON, declared sets, typed masks, diff |
| `parity/runner.py` | Scenario execution, twin comparison, evidence bundles |
| `parity/capture.py` | Inventory capture into `baseline/inventory/` |
| `parity/canaries.py` | Mutation canaries (patched Go builds that must be caught) |
| `parity/verify.py` | Fail-closed verifier for `parity-manifest.json` gates |
| `scenarios/*.json` | Versioned scenarios; the same file runs against Go and Rust |
| `fixtures/shims/*.json` | Scripted host-tool responses |
| `canaries.json` | Canary patches and the scenario each must turn red |

## Comparison rules

- **Run variables.** Each side's agent ID, port, token, sandbox path, and the
  ADR 0012 tool-name prefix derived from its ID are replaced by `${NAME}` by
  exact literal substitution. The harness derives the prefix itself, so a
  Rust implementation that derives it differently shows up as a diff.
- **Sets.** An array is compared as a set only when a scenario declares it,
  with a note saying why order is not contract (see finding F-2).
- **Typed masks.** A mask names an exact path, a type (`rfc3339`, `uuid`,
  `non-negative-int`, `number`, ...) and a reason. A value of the wrong type,
  or a mask path that matches nothing, is a violation and fails the scenario.
  Regex masks over payloads do not exist.
- **Harness hash.** `agent`, `canon`, `runner`, `shims` and the fixtures are
  hashed into every bundle. Changing them invalidates existing evidence.
- **Status is derived.** The verifier recomputes every status from the hashed
  diff and iteration files. A summary's own `status` field is never trusted.

## Adding a scenario

1. Add it to a file in `scenarios/` with an `id`, `surface`, `owner` and
   `anchors` that point at the Go code or test it covers.
2. `make parity-manifest` to register it as a manifest item.
3. Run it Go vs Go with `--repeat 20`. If it is flaky, find out why before
   adding a set or a mask. A new mask needs a type and a reason that a
   reviewer can check.
