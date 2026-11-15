# m18: A4, A2 and A5 on the 0.3.0 candidate, measured then fixed

The m17 tests (D40, D41) ran on `rust-0.3.0-rc4`'s product code (main at
`e76bbf6f2`). The only additions were the tests and `Env::for_child`, which
no product code called yet. `before.log` is that run. `after.log` is the
same tests after the fixes. Test secret values are redacted.

| Finding | Test | Before | Decision |
|---|---|---|---|
| A4 | `hostobs::tests::a_child_never_sees_the_agents_secrets`, `config::tests::every_spawn_site_passes_the_child_environment` | `env` run through the command runner (as the agent runs incus or kubectl) listed `MCP_AUTH_TOKEN`, `OPUTE_OAUTH_CLIENTS`, `CLOUDFLARE_API_TOKEN`, `TAILSCALE_AUTH_KEY` and `OPUTE_REMOTE_AGENT_ID`; the first site without the child environment was `llmops.rs:3048` | D40 for 0.3.0 by D43, [child-process-environment](../../openspec/changes/child-process-environment/proposal.md) |
| A2 | `llmops::tests::oversized_relay_bodies_are_refused_without_reading_them` | no reply before the 33 MiB of a 1 GiB declared body ended | D41 for 0.3.0 by D43, [bound-relay-request-body](../../openspec/changes/bound-relay-request-body/proposal.md) |
| A5 | `providers::tests::oversized_provider_bodies_are_refused_without_reading_them` | no reply before 5 MiB of a 64 MiB declared body ended | D41 for 0.3.0 by D43 |

The two properties in the same run passed before and after. They test
`Env::for_child` and the bounded body read themselves, not whether a site
uses them.

0.3.0 starts processes at seven sites (`llmops`, `identity`, `hostvirt`,
`providers/k3s`, `hostexec`, `servingops`, `hostobs`). 0.4.0's eighth is the
console (D38). All seven pass the child environment after the fix.

O1 (audit retention) stays open in 0.3.0: D42 adds two settings, a contract
change the parity release does not take (D43).
