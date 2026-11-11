# m17: the three open observations, measured then fixed

The five failing-first tests ran on rust-0.4.0-rc1's code (commit
`4f97fed96`). The only change before that run was a `prune_audit` stub that
prunes nothing, which is what rc1 does. `before.log` is that run, and
`after.log` is the same tests after the fixes. Test secret values are
redacted.

| Finding | Test | Before | Decision |
|---|---|---|---|
| A4 | `hostobs::tests::a_child_never_sees_the_agents_secrets`, `config::tests::every_spawn_site_passes_the_child_environment` | `env` run through the command runner listed `CLOUDFLARE_API_TOKEN`, `MCP_AUTH_TOKEN` and `OPUTE_*`; the first site without the child environment was `llmops.rs:3048` | D40, [child-process-environment](../../openspec/changes/child-process-environment/proposal.md) |
| A2 | `llmops::tests::oversized_relay_bodies_are_refused_without_reading_them` | no reply before the 33 MiB of a 1 GiB declared body ended | D41, [bound-relay-request-body](../../openspec/changes/bound-relay-request-body/proposal.md) |
| A5 | `providers::tests::oversized_provider_bodies_are_refused_without_reading_them` | no reply before 5 MiB of a 64 MiB declared body ended | D41 |
| O1 | `store::tests::audit_rows_are_pruned_by_age_and_count_but_never_while_referenced` | all 31 rows kept | D42, [audit-retention](../../openspec/changes/audit-retention/proposal.md) |
