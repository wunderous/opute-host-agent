# The launcher creates the workload slice

Status: **accepted (owner, 2026-11-24) as D56**. Published as `0.5.1`
(launcher only) on 2026-11-24.

## Why

Under the default fail-closed resource enforcement
(`OPUTE_HOST_RESOURCE_ENFORCEMENT=fail-closed`), the agent refuses every call
whose operation class is not `control` with
`host_resource_enforcement_unknown` until the systemd slice
`opute-workload.slice` exists with resource limits. Installed agents get the
slice from their install path. The npm launcher creates none, and it strips
`OPUTE_HOST_RESOURCE_ENFORCEMENT`.

Measured on 2026-11-24 with the published 0.5.0 on a fresh Ubuntu 24.04 user
(wunderous/opute-host-agent#76):
- `npx -y @opute/host-agent start --background --allow-mutations` comes up;
- the first `ensure_host_file` is refused with
  `host_resource_enforcement_unknown`;
- after the user writes the slice by hand and runs
  `systemctl --user daemon-reload`, the same call succeeds.

So a newcomer's first change fails, and the README had to teach them to
write a systemd unit.

## What Changes

- On `start` with `--allow-mutations`, in the foreground or with
  `--background`, the launcher makes sure the user's
  `opute-workload.slice` exists before it starts the agent:
  - **The slice exists** (`systemctl --user cat opute-workload.slice`
    succeeds), in the user's configuration or anywhere else systemd loads
    it from. Nothing is written, so an operator's stricter slice is kept.
  - **It does not exist.** The launcher writes
    `$XDG_CONFIG_HOME/systemd/user/opute-workload.slice`
    (`~/.config/systemd/user/` by default) with the text the agent's own
    `reconcile_host_resource_policy` renders for the slice under
    `opute-host-resource-policy.v2`, at mode `0644`. It then runs
    `systemctl --user daemon-reload` and prints one line on stderr naming
    the file.
  - **No systemd user manager answers** (`systemctl --user` fails). The
    launcher exits 1 before it starts the agent. The message says that
    changes need a systemd user session, and names the slice and its
    limits so the operator can create it.
- Without `--allow-mutations` the launcher touches no slice, and `start`
  behaves exactly as in 0.5.0.
- Stdout is unchanged: `start --background` still prints only the MCP URL.

## Impact

- **Not changed:** the agent binary, its fail-closed default, its limits and
  its admission. 0.5.1 republishes the 0.5.0 release files under a 0.5.1
  pin; only the npm package changes.
- **New on the host:** one user unit file, created only on request
  (`--allow-mutations`) and never overwritten. Removing it brings back
  0.5.0's behavior.
- **Rollback:** 0.5.0's launcher ignores the file and works with it, so a
  rollback needs no cleanup.
- **Docs:** step 4 of the README and the npm README drop the manual slice
  block. Each keeps a note on what the launcher creates and how to set
  lower limits.
