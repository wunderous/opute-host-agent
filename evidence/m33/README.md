# m33: the README as written

Goal: `.agents/goals/m33-readme-as-written.md`. Check every promise in README.md for a
newcomer on the published 0.5.4, and fix or correct each one that doesn't hold.

- **Inventory:** [`inventory.md`](inventory.md) lists all 22 claims and how each is
  checked.
- **Journey `e2e9.readme`** (`evidence/e2e/journeys/make_readme.py`, gate
  `e2e-readme`):
  - **Who:** a fresh sudo user, `newcomer`, logged in over SSH on the dev VM (8 vCPU,
    16 GiB), with Node 22 and no linger. One login stays open, as a terminal would.
    Each README step runs in a login of its own.
  - **What it installs:** the published `@opute/host-agent` from npm (`latest`) and
    the v0.5.4 GitHub release, both downloaded during the run.
  - **What stands in for the assistant:** scripted MCP calls, the ones it would make
    for the README's two questions.
  - **Setup:** the VM's own agent is stopped for the run, because it listens on the
    README's port 3014.
  - **One deviation:** `MCP_AUTH_TOKEN` is the harness's random secret, so the
    harness can call the agent. `openssl rand -hex 32` is run on its own and gives 64
    hex digits.
- **CI job `readme-quickstart`:** quickstart steps 1 and 3, read-only, with the
  published package. It runs on `ubuntu-24.04` (Node 22 and 18) and
  `ubuntu-24.04-arm` (Node 22), on every PR, on main and weekly.

## What was found

| # | Finding | Result |
| --- | --- | --- |
| R1 | For a sudo user who isn't in `incus-admin`, step 4's *"Install Incus and create a small Ubuntu VM called sandbox"* fails. `install_virtualization_stack` adds the user to the group, but the running agent keeps the groups it started with. The same call, and every later Incus tool, failed with only "You don't have the needed permissions to talk to the incus daemon". | Defect, fixed: `incus-access-refusal-names-the-remedy`. The error now says to log in again and restart the agent. README step 4 says so too. Failing-first test `a_refused_incus_socket_says_how_to_get_access`. |
| R2 | `delete_vm` for a VM that doesn't exist fails with `invalid resource URI: expected "container", got "vm"`, which doesn't say the VM is missing (draft 1, after R1 stopped the create). | Open observation, no change: the README path isn't affected. |
| D1 | README: "The 0.5.0 catalog has 138 tools". | Corrected to 0.5.4. The live list on 0.5.4 has 138 tools. |
| W1 | www.opute.io and opute.io answered 530 (Cloudflare: origin unreachable) from 00:37Z until at least 01:30Z on 2026-11-28. Every README docs link points there. | For the website owner: the private site deployment serves it, not the dev VM. minimal.opute.io served throughout once restored. |

With the README's corrected step 4 (log out and back in, restart the agent, ask
again), the sandbox VM is created on the published 0.5.4 too (`m33-before-r3`). R1 is
about the first answer, which gave no way forward.

## Runs

| Run | Agent | Result |
| --- | --- | --- |
| `m33-before-r3` | published 0.5.4 (`3b74ecc8…`) | fails only *A refused Incus socket names the remedy*: the error has no remedy. Not a gate run. |
| `m33-r3` | `rust-0.5.5-rc1`, through the published launcher with `OPUTE_HOST_AGENT_BINARY` | **passes**, gate `e2e-readme` |

`rust-0.5.5-rc1` was built from `4645018d3` with `VERSION=0.5.5`. It holds 0.5.4,
m32's F5 (streaming hash) and R1. Its SHA256SUMS hash is `c0d50088…`. `make rust-check`
and `make fuzz FUZZ_SECONDS=20` (18 targets) pass on that commit. Drafts and
journey-defect runs are retired under `.parity/retired/m33/`, each with its cause.

Measured in `m33-r3`:

| Step | Time |
| --- | --- |
| Quickstart 1: `npx -y @opute/host-agent start --background` (npm download included) | 1.5 s |
| A read-only first answer (`tools/list`, `get_host_info`) | 0.2 s, 0.3 s |
| Restart with `--allow-mutations` | 1.6 s |
| First `install_virtualization_stack` (refused socket, with the remedy) | 42 s |
| Log out, then the next login | 31 s, 0.4 s |
| `install_virtualization_stack` again | 6.8 s |
| `create_vm` sandbox (`images:ubuntu/22.04`, running) | 45 s |
| `delete_vm` | 3.6 s |
| Release binary: download, `sha256sum -c`, serve | 3.8 s |

`make e2e-verify E2E_GATE=e2e-readme` re-derives the pass from the committed receipts.

## Not covered

- WSL2 (#78), the Cursor, Claude Desktop and VS Code snippets (#79): left for
  contributors.
- No AI client is connected. The Claude Code command's flags are checked against
  `claude mcp add --help`.
- `make rust-build` isn't rerun here; CI's `rust-suites` job runs it on every PR.
- arm64 runs only the read-only quickstart (CI). The VM path is x64 only.
- Publishing 0.5.5 is for the owner to sign off.
