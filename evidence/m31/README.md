# m31: the CLI operation commands work

Change: `openspec/changes/cli-operation-commands` (option A, owner,
2026-11-26). Up to 0.5.3, `recipe`, `provider` and `public-mcp` validated
their flags and then always failed with "not implemented in this build yet".
They now call their published tools in process, as Go v0.2.4's wrappers did.

## Tests

`crates/host-agent/tests/cli_operations.rs` runs the binary on a temporary
state (7 tests). With `cli.rs` and `app.rs` at 0.5.3, six of them fail; the
seventh (a held instance refuses) passes on both. `make rust-check` and
`make fuzz FUZZ_SECONDS=20` (18 targets) pass.

## Fresh-user run on the dev VM

- Candidate `rust-0.5.4-rc1`, built from `d5033b489` with `VERSION=0.5.4`
  (SHA256SUMS sha256 `268a7a74…2bfd71`).
- `guest-cli-run.sh` runs as root in the dev VM: it creates user `m31cli`,
  which runs each command on its own instance state, and removes the user
  afterwards. The VM's own agent, which serves minimal.opute.io, is not
  touched. The site answered 200 before and after.
- Inputs: `recipe.yaml` (the test recipe: one `ensure_host_file` node), the
  k3s provider binary from the candidate, a descriptor for it on port 4331,
  and the provider's own activation recipe beside the descriptor
  (`recipes/kubernetes.yaml`, the file its install manifest names, sha256
  `91f0b596…`).
- Resource enforcement is `advisory`. This raw-binary run has no workload
  slice; the npm launcher creates one (D56).
- Transcript: `vm-run.txt`. Each step prints its exit code:

| Step | Command | Result |
| --- | --- | --- |
| validate | `recipe validate --source recipe.yaml --input word=vm` | 0, the validation result |
| applyRefused | `recipe apply …` without the grant | 1, `mutation refused: run_runtime_recipe …`; the file is absent |
| apply | `recipe apply --allow-mutations …` | 0, run `completed`; the file holds `vm` |
| status | `recipe status --run-id RUN` | 0, the same run, `completed` |
| installRefused | `provider install --source provider.json` | 1, `mutation refused: opute.provider.install …` |
| install | `provider install --allow-mutations --activate --source provider.json` | 0, activation run `completed`, runtime `com.opute.k3s` active |
| providerStatus | `provider status --provider com.opute.k3s` (a new process) | 0, `active: true, connected: true`, generation `com.opute.k3s-1` |
| validateProvider | `provider validate --provider com.opute.k3s` | 0, `ready: true` |
| reload | `provider reload --allow-mutations --activate …` | 0, run `completed` |
| publicRefused | `public-mcp …` without the grant | 1, `mutation refused: ensure_public_mcp_tunnel …` |
| besideAgent | `provider status …` while `serve` holds the state | 1, `host agent instance is already in use` |

Not run: `public-mcp --allow-mutations`, which needs a real Cloudflare
tunnel token. Its tool, `ensure_public_mcp_tunnel`, runs as an MCP task. The
command's task wait is the same code path as the refused call above, and
the tool's own journeys cover it.
