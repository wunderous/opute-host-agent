# m19: secret exposure sweep

Goal m19 asked where every secret the agent holds goes, measured on both
release lines before anything changed. The decisions are D46 to D51
(`openspec/changes/secret-placement` and the five changes it lists).

| File | What |
|---|---|
| `inventory.md` | every secret class, where it comes from and where it should go; the suspects from review, what was measured, and an observation |
| `before.log` | the five failing-first unit tests (S1 to S5) on rc5's product code (`c2f9947ee`) with behaviour-preserving seams |
| `before-040.log` | the same five on rust-0.4.0-rc3's product code |
| `before-s6.log` | the S6 test on rc5's `mesh.rs` |
| `before-rc5-sweep.log`, `before-rc3-sweep.log` | the verdict lines of the before runs |
| `fuzz.txt` | the 60 s fuzz pass on the rc6 product code |

The journey is `e2e6.secret-sweep` (gate e2e-6), with its guest tool
`evidence/e2e/guest-tools/secretsweep.py`. It ran before the fixes as
`evidence/e2e/runs/m19-before-030` (rust-0.3.0-rc5) and, on the
`rust-0.4.0` line, `m19-before-040` (rust-0.4.0-rc3). Both failed only their
pass conditions, with the same findings: S1, S4 and S5. The rc5 before run
pinned the guest tool before it learned to start a 0.4.0 binary (commit
`157630bc6`); its own `journey.json` and receipt record what ran.

A first rc5 before run was retired (`.parity/retired/m19-before-030-first`,
not committed): the same findings, but the guest tool changed after it.

## Findings

| Id | Path | Decision | Check |
|---|---|---|---|
| S1 | public-MCP tunnel token on cloudflared's argv | D47 | journey, `hostpublic` test |
| S2 | Cloudflare host-placed connector's run token in `ExecStart` | D48 | `cloudflare` test |
| S3 | Tailscale auth key on `tailscale up`'s argv | D49 | `tailscale` test |
| S4 | cluster agent's bridge token on argv, in sudo's journal entry, config 0644 | D50 | journey, `clusterops` test |
| S5 | LLM relay's upstream token in the store | D51 | journey, `catalog` test |
| S6 | Cloudflare HA connector's run token in `ExecStart`, unit 0644 | D48 | `cloudflare` test |
