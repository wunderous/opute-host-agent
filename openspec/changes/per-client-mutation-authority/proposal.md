# Mutation authority per client

Status: **accepted (owner, 2026-11-24) as D55**. Target `0.5.0`.

## Why

0.4.0 decides mutations with one switch for the whole agent,
`OPUTE_HOST_ALLOW_MUTATIONS`. Every caller of a running agent gets the same
authority: the bootstrap bearer, every OAuth machine client and every
interactively approved client. To keep 0.3.x and Go rollbacks working, the
recorded unit and the launcher also carry the old `OPUTE_AGENT_MODE` and
`OPUTE_STANDALONE_ALLOW_MUTATIONS`, and the launcher, the npm README and the
site document both generations.

The owner decided on 2026-11-24 that there are no users to keep
compatible. Mutation authority should belong to each client, and the old
settings should go everywhere.

Measured on `rust-0.4.0-rc6`:
- `tools/list` publishes no `annotations` on any of its 138 tools. The
  effect class (54 `read`, 58 `mutation`, 21 `destructive`,
  5 `credential_bearing`) appears only in `_meta.capability.effect`.
- A refused call says only `mutations are disabled; set
  OPUTE_HOST_ALLOW_MUTATIONS=true`.
- OAuth clients have a single scope, `mcp`.

## What Changes

- **Per-client authority.** Each call is admitted with its caller's grant:
  - an OAuth token carries `read` or `mutate`;
  - the bootstrap bearer (`MCP_AUTH_TOKEN`, loopback only) is read-only
    unless the agent was started with `--allow-mutations`;
  - a provider executable's callback has no more authority than the call
    it acts for.

  A `read` call is admitted for every authenticated caller.
- **Scopes.** OAuth scopes are `read` and `mutate`, and `mutate` includes
  `read`:
  - `OPUTE_OAUTH_CLIENTS` entries are `id` (meaning `read`), `id:read` or
    `id:mutate`;
  - a token request may ask for up to the client's grant;
  - an interactive authorization names its scope, which defaults to `read`,
    and `oauth pending` and `oauth approve` show it;
  - the old scope `mcp` is refused at start, and protected-resource
    metadata lists `read` and `mutate`.
- **No mutation setting.** `--allow-mutations` on `serve` (and the bare
  invocation) is the only switch, and it applies to the bootstrap bearer
  only. `OPUTE_HOST_ALLOW_MUTATIONS` is removed. `OPUTE_STANDALONE_ALLOW_MUTATIONS`
  and `OPUTE_AGENT_MODE` appear nowhere: not in the agent, the recorded unit,
  the launcher (its passthrough list, its help and its
  `OPUTE_AGENT_MODE=standalone`), the npm README, the site's notes on
  earlier releases, or the dual-setting design of
  `evidence/cutover/proposed-0.4.0`, which 0.5.0's proposed surfaces
  replace.
- **Refusals name the grant.** A refused call names the tool, its effect,
  the caller's current grant, and the exact grant that would admit it.
- **Annotations.** Every tool in `tools/list` carries
  `annotations.readOnlyHint`, which is `true` exactly when its effect is
  `read`. A non-read tool also carries `annotations.destructiveHint`, which
  is `true` exactly when its effect is `destructive`. Annotations are the
  same for every caller, and `_meta.capability.effect` is unchanged.

## Impact

- **Breaking:** settings, scopes and the CLI change. The release is 0.5.0,
  because a 0.4.x patch must stay compatible.
- **Rollback restores the previous unit with the binary.** The owner
  accepted that a rollback to 0.4.0 or 0.3.0 may come up read-only. Measured
  in m28, a rollback of the binary alone does not come up at all: 0.4.0 and
  0.3.0 exit on the 0.5.0 unit's `--allow-mutations` (`flag provided but not
  defined`). So a rollback puts the 0.4.0 unit back with the binary, and
  0.4.0 or 0.3.0 then serves 0.5.0's state with mutations admitted from that
  unit's settings. The 0.5.0 binary under the 0.4.0 unit comes up read-only.
  `neutral.rollback` asserts all three, with the state intact.
- **Supersedes:**
  - D10's switch (`standalone-read-only-gate`) and its message;
  - the `OPUTE_HOST_ALLOW_MUTATIONS` setting in
    `platform-neutral-host-agent`;
  - the single `mcp` scope of `secure-oauth-issuance`.

  The read-only rule itself is unchanged: an effect counts as `read` only
  as published or declared, never inferred.
- **Product surfaces** ship with 0.5.0 through the m26 release path: the
  unit with `--allow-mutations` in `ExecStart`, the launcher forwarding
  `--allow-mutations`, the npm README and the site docs.
- **Journeys:** every agent that mutates starts with `--allow-mutations`
  or uses a `mutate` client. New journeys cover each caller and its refusal.
