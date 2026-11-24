# Proposed 0.5.0 install surfaces

0.5.0 gives each caller its own mutation grant (D55,
`per-client-mutation-authority`): the `MCP_AUTH_TOKEN` bearer mutates only
when the agent starts with `--allow-mutations`, and an OAuth client only with
the `mutate` scope. No setting grants it: `OPUTE_HOST_ALLOW_MUTATIONS`,
`OPUTE_STANDALONE_ALLOW_MUTATIONS` and `OPUTE_AGENT_MODE` have no effect.
The files below are against `wunderous/host-agents` at v0.4.0
(`7b74ef30b4633d7e2ab903d96250129401d60405`). The 0.5.0 gates run on them;
the release ships them. Nothing here is published.

- `opute-bootstrap-mcp.service`: the v0.4.0 `deploy/opute-bootstrap-mcp.service`
  without the three settings, with `--allow-mutations` in `ExecStart`. A
  rollback to 0.4.0 or 0.3.x with this unit starts read-only, which the owner
  accepted (D55). The dev VM (`evidence/e2e/devvm/devvm.py`) installs it for
  a 0.5.x release and records its hash.
- `launcher-index.js.patch`: against v0.4.0's `npm/local-host-agent/index.js`.
  The launcher no longer sets `OPUTE_AGENT_MODE`, strips both old mutation
  settings instead of passing them, names neither in its help, and forwards
  `--allow-mutations` (`start [--background] --allow-mutations`).
- `host-agents-surfaces.patch`: the whole host-agents change, the two files
  above included: the launcher's tests (the old settings are stripped, the
  flag is forwarded; the published canary mutates with the flag), the npm and
  top READMEs, and the site's docs generator with its regenerated pages. The
  site's catalog capture needs no change: it serves read-only, without a mode.
