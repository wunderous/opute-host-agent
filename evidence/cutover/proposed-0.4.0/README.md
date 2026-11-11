# Proposed 0.4.0 install surfaces

0.4.0 removes `--mode` and reads `OPUTE_HOST_ALLOW_MUTATIONS` in place of
`OPUTE_STANDALONE_ALLOW_MUTATIONS` (`platform-neutral-host-agent`, design
N5). The recorded deploy unit and the published npm launcher still pass
`--mode`, so 0.4.0 cannot start from either until their owners change them
(that change's task 4.0). The owners decided (2026-11-11) that the 0.4.0
gates run on the files proposed here; the release owners still ship them.
Nothing here is published.

- `opute-bootstrap-mcp.service`: Go v0.2.4's `deploy/opute-bootstrap-mcp.service`
  without `--mode standalone`, with `OPUTE_HOST_ALLOW_MUTATIONS=true` added
  next to the old settings. Both settings sets stay, so a rollback to 0.3.x
  or Go reads `OPUTE_AGENT_MODE` and the old mutation setting, and 0.4.0
  ignores them. The dev VM (`evidence/e2e/devvm/devvm.py`) installs it for a
  0.4.x release, and records its hash.
- `launcher-index.js.patch`: against Go v0.2.4's `npm/local-host-agent/index.js`.
  The launcher starts the binary without `--mode=standalone`, passes
  `OPUTE_HOST_ALLOW_MUTATIONS` through, and names it in its help. The
  journeys' `opute-host-agent-0.4.0.tgz` is that tree with the patch applied
  and the version set to 0.4.0, packed locally.
