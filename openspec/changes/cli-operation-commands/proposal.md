# The CLI operation commands work

Status: **option A decided by the owner on 2026-11-26** ("Take your
recommended actions"). Implemented in m31.

## Why

The agent binary accepts three commands that the public docs list on the
Configuration page (`opute-host-agent public-mcp …`, `recipe …`,
`provider …`). In every Rust release up to 0.5.3, each one validates its
flags exactly as Go did and then always fails:

    recipe operations is not implemented in this build yet (planned in M6)
    provider operations is not implemented in this build yet (planned in M7)
    public MCP exposure is not implemented in this build yet (planned in M8a)

In Go v0.2.4 each was a thin wrapper. It started the agent's MCP server
in-process, called one published tool, waited for a run where one is
started, and printed the structured result as JSON:

| Command | Tools it called |
| --- | --- |
| `public-mcp` | `ensure_public_mcp_tunnel` |
| `recipe validate \| apply \| status` (`--kind runtime\|tunnel`) | `validate_runtime_recipe` / `run_runtime_recipe` / `get_runtime_recipe_run`, and the tunnel equivalents |
| `provider install \| validate \| status \| reload` | `opute.provider.install` / `.validate` / `.status` / `.reload`, waiting on the activation run |

Every one of those tools ships in 0.5.3 and passes its journeys.

## Options

**A. Wire them (chosen).** Restore Go's wrappers: an in-process
`tools/call` through the same admission, mutation authority and plan runner
as any client, with the same flags, waits and JSON output.

**B. Withdraw them.** Remove the three commands from the CLI and the docs
page.

## Decisions within A

- **Mutation authority (D55).** Go's wrappers called the dispatcher with no
  session, so they mutated whenever `OPUTE_HOST_ALLOW_MUTATIONS` allowed the
  agent to. 0.5.0 removed that setting: the bootstrap bearer mutates only on
  an agent started with `--allow-mutations`. The operator running a command
  is that bearer, so each of the three commands takes the same flag,
  `--allow-mutations`. Without it a mutating call is refused with the
  standard `mutation_refused` text; read calls (`recipe validate`,
  `recipe status`, `provider validate`, `provider status`) need nothing.
  This is the D55 rule applied to the CLI, not a new grant.
- **Task-run tools.** `ensure_public_mcp_tunnel` runs as an MCP task. The
  command awaits the task and prints its result, as Go's synchronous
  dispatch printed the tool's result.
- **One process per instance (D25).** As in Go v0.2.4, a command opens the
  agent's state and its instance lock, so it refuses while an agent holds
  that state. The help text says so.

## Impact

- `crates/host-agent/src/cli.rs`: the three commands call their tools;
  `app::with_local_server` composes the server `run` serves, with no
  listener, and shares `run`'s shutdown.
- `crates/host-agent/tests/cli_operations.rs`: the binary's commands, end
  to end on a temporary state.
- host-agents' Configuration page: the examples gain `--allow-mutations`.
