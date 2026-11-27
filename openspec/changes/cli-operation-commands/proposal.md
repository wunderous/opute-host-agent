# The CLI operation commands work, or are withdrawn

Status: **proposed (m30), awaiting the owner's decision.** Not implemented.

## Why

The agent binary accepts three commands that the public docs list on the
Configuration page (`opute-host-agent public-mcp …`, `recipe …`,
`provider …`). In every Rust release up to 0.5.2, each one validates its
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

Every one of those tools ships in 0.5.2 and passes its journeys. The npm
launcher, the README and the recipes use none of the three commands, and no
issue has reported them. The only place they appear is that docs page.

## Options

**A. Wire them (recommended).** Restore Go's wrappers: an in-process
`tools/call` through the same admission, mutation authority and plan runner
as any client, with the same flags, waits and JSON output. Behavior then
matches the docs and Go, and no capability, tool or default is added. Cost:
one module and a fresh-user run of each command on the dev VM.

**B. Withdraw them.** Remove the three commands from the CLI and the docs
page. The binary would answer `unknown command` with the remaining list.
This is a visible contract change to the documented CLI. A user who
scripted them against Go loses them, although none can be using them on Rust
today, since they never worked there.

The recommendation is A: the docs already promise these commands, the work
is small, and it keeps the Go contract this repository preserves.

## Impact

- A: `crates/host-agent/src/cli.rs` (the three `not_yet_implemented` calls),
  CLI tests, and a journey step per command.
- B: `cli.rs`, its tests, the `help` text, and host-agents'
  `site/scripts/generate-docs.ts` Configuration page.
