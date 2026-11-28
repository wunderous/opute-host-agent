# m33 inventory: every README claim and its check

README.md as of `e15ef42a8` (0.5.4 published). Each claim is checked by:
- **J**: journey `e2e9.readme`, a fresh sudo user on the dev VM with the published
  npm package;
- **CI**: the `readme-quickstart` job, with the published package on x64 (Node 22 and
  18) and arm64 (Node 22);
- **L**: a local check, listed below;
- **E**: earlier evidence;
- **out of scope**, with the reason.

| # | Claim | Check | Result |
| --- | --- | --- | --- |
| 1 | CI and npm badges, the MCP and GitHub links resolve | L: `curl` each link | 200. npmjs.com answers 403 to curl (bot block). The www.opute.io links answered 530 during the run, see note 1 |
| 2 | The 0.5.0 catalog has 138 tools | J, CI: live `tools/list` | 138 on 0.5.4. **Corrected** to "0.5.4" |
| 3 | Listens on `127.0.0.1:3014` only | J, CI | holds |
| 4 | Every client is read-only until it holds a grant; the bearer mutates only with `--allow-mutations` | J, CI: `create_vm` before step 4 | refused with `mutation_refused` |
| 5 | A refused call says which grant would admit it | J, CI | names `--allow-mutations` and the OAuth `mutate` scope |
| 6 | OAuth clients get `read` or `mutate`; interactive `mutate` waits for `opute-host-agent oauth approve` | L: `oauth approve -h` on the 0.5.4 binary; E: `e2e1.auth` | the command exists |
| 7 | Each tool declares `readOnlyHint` and `destructiveHint` | J: every tool in the live list | 0 tools without `readOnlyHint`, 0 change tools without `destructiveHint` |
| 8 | Secrets redacted by schema; reset/remove checks ownership and confirmation | E: `e2e6.secret-sweep`, `e2e2.host-ops` | out of scope here (no journey change) |
| 9 | Linux x64 or arm64, Node.js 18 or newer | J (x64, Node 22), CI (x64 Node 22 and 18, arm64 Node 22) | see the evidence README. The launcher declares `"node": ">=18"` |
| 9a | WSL2 | issue #78, left for contributors | out of scope |
| 10 | Quickstart 1's three lines start the agent; the launcher downloads the binary for the CPU and verifies its checksum | J: the running binary's SHA-256 is the release's; CI: the binary's architecture is the runner's | holds |
| 11 | `status` and `stop` | J | holds |
| 12 | Claude Code: `claude mcp add --transport http … --header` | L: `claude mcp add --help` names `--transport` and `-H, --header` | holds. No client is connected in the journey |
| 12a | Cursor and Claude Desktop JSON; VS Code tutorial | issue #79, left for contributors | out of scope |
| 13 | Quickstart 3: the assistant calls `get_host_info` | J, CI | holds |
| 14 | Quickstart 4: restart with `--allow-mutations`; the launcher creates `opute-workload.slice`; this needs a systemd user session, which an SSH login has | J: over a real SSH login, no linger | holds |
| 15 | "Install Incus and create a small Ubuntu VM called sandbox"; `install_virtualization_stack` installs Incus when the user has the privileges | J | **failed on 0.5.4 (R1)**, see the evidence README |
| 16 | Example recipes exist and run on every release candidate | L: files exist; E: gates `e2e-7` (site), `e2e-local-llm` | holds |
| 17 | Each GitHub release ships x64 and arm64 binaries, the three providers, `SHA256SUMS` and a CycloneDX SBOM | L: v0.5.4's asset list | holds |
| 18 | The release binary, checked against `SHA256SUMS`, serves MCP over HTTP on 127.0.0.1:3014 with no flags | J: downloaded from v0.5.4; L: the binary with no flags logs `HTTP transport listening addr=127.0.0.1:…` | holds |
| 19 | `make rust-build` builds `target/release/opute-host-agent` | E: CI's `rust-suites` job runs `rust-build` on every PR | holds, not rerun here |
| 20 | Transport is Streamable HTTP only; stdio isn't supported | L: `--transport stdio` | refused: `only Streamable HTTP (http) is supported` |
| 21 | Releases are accepted on journeys, including rollback | E: `evidence/e2e/gates.json`, `e2e4.upgrade-chain` | holds |
| 22 | CONTRIBUTING, SECURITY, CODE_OF_CONDUCT, LICENSE and docs files exist | L | all present |

Notes:
1. www.opute.io and opute.io answered HTTP 530 (Cloudflare: origin unreachable) from
   the first check at 00:37Z until at least 01:30Z on 2026-11-28, also after
   minimal.opute.io was serving again from the dev VM. The docs site isn't served from
   that VM: the private site deployment serves it, and the website owner owns it.
   Every README docs link depends on it. Reported; not changed here.
