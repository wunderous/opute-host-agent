## 1. Decision

- [x] 1.1 Decide (owner, 2026-11-24): D55, per-client mutation authority, no compatibility; target 0.5.0.

## 2. Tests first

- [ ] 2.1 Failing tests against 0.4.0's code for every scenario: bearer with and without the flag, `read`/`mutate` clients, two clients, provider callback, `mcp` refused, `invalid_scope`, interactive scope, leftover setting, refusal texts, annotations for all 138 tools.

## 3. Agent

- [ ] 3.1 Grant per caller in `transport.rs` authorization and `tools.rs` admission; `--allow-mutations` in the CLI; remove `OPUTE_HOST_ALLOW_MUTATIONS`.
- [ ] 3.2 Scopes in `oauth.rs`, the token endpoint, consent, `oauth pending`/`approve`, protected-resource metadata.
- [ ] 3.3 Provider callbacks bounded by the delegating call's grant.
- [ ] 3.4 `annotations` in `tools/list` from the effect.
- [ ] 3.5 No remaining `OPUTE_AGENT_MODE`, `OPUTE_STANDALONE_ALLOW_MUTATIONS` or `OPUTE_HOST_ALLOW_MUTATIONS` outside tests that prove they have no effect.

## 4. Surfaces

- [ ] 4.1 `evidence/cutover/proposed-0.5.0/`: the unit (`--allow-mutations` in `ExecStart`, no mutation or mode settings), the launcher (no passthrough, help or `OPUTE_AGENT_MODE` for either setting; forwards `--allow-mutations`), the site capture. Retire `proposed-0.4.0`'s dual design.
- [ ] 4.2 Ship them, the npm README and the site docs (no "earlier releases" notes) with 0.5.0 in host-agents.

## 5. Evidence and release

- [ ] 5.1 Journeys: every mutating agent starts with `--allow-mutations` or a `mutate` client; new journeys per caller and refusal; rollback journeys to 0.4.0 and 0.3.0 assert read-only and intact state.
- [ ] 5.2 New candidate `rust-0.5.0-rc1` (fmt, clippy, 60 s fuzz, stale grep); every gate under one run id; readiness; packet.
- [ ] 5.3 Owner sign-off; publish through the m26 release path.
