## ADDED Requirements

### Requirement: Mutation authority belongs to the caller

The Host Agent SHALL admit a `tools/call` whose effect is not `read` only
when the caller's own grant includes mutations (owner decision D55). An
OAuth access token SHALL carry the scope `read` or `mutate`, and only
`mutate` admits mutations. The bootstrap bearer (`MCP_AUTH_TOKEN`, accepted
on loopback only) SHALL admit mutations only when the agent was started with
`--allow-mutations`; the flag SHALL NOT change any OAuth token's authority.
A provider executable's callback to a host tool SHALL have no more authority
than the call it acts for. Whether a tool counts as `read` SHALL follow
`standalone-read-only-gate`: the published effect, or a declared effect for
an unpublished tool, never an inferred one. Every authenticated caller SHALL
be admitted to `read` tools.

#### Scenario: Bearer without the flag
- **WHEN** the agent runs without `--allow-mutations` and a caller with the bootstrap bearer calls a tool whose effect is not `read`
- **THEN** the call is refused before any effect

#### Scenario: Bearer with the flag
- **WHEN** the agent runs with `--allow-mutations` and a caller with the bootstrap bearer calls a mutation
- **THEN** the mutation gate does not refuse the call

#### Scenario: Read client
- **WHEN** an OAuth client whose token carries `read` calls a mutation, whether or not the agent runs with `--allow-mutations`
- **THEN** the call is refused before any effect

#### Scenario: Mutate client
- **WHEN** an OAuth client whose token carries `mutate` calls a mutation on an agent started without `--allow-mutations`
- **THEN** the mutation gate does not refuse the call

#### Scenario: Two clients on one agent
- **WHEN** a `read` client and a `mutate` client call the same mutation on one agent
- **THEN** only the `mutate` client's call is admitted

#### Scenario: Provider callback
- **WHEN** a provider executable calls back into a host mutation on behalf of a call made with a `read` grant
- **THEN** the callback is refused before any effect

### Requirement: OAuth scopes are read and mutate

The agent SHALL define exactly two OAuth scopes, `read` and `mutate`, where
`mutate` includes `read`. An `OPUTE_OAUTH_CLIENTS` entry SHALL be `id`,
`id:read` or `id:mutate`; `id` alone grants `read`. Any other scope,
including the former `mcp`, SHALL fail the start with an error naming the
two valid scopes. A token request SHALL be issued at most the client's
grant, and the client's grant when it names none. An interactive
authorization SHALL name its scope, defaulting to `read`. The operator's
`oauth pending` and `oauth approve` SHALL show the scope being approved, and
a remembered consent SHALL cover only the approved scope. Protected-resource
metadata SHALL list `read` and `mutate`.

#### Scenario: Former scope
- **WHEN** `OPUTE_OAUTH_CLIENTS` lists `ci:mcp`
- **THEN** the agent refuses to start and names `read` and `mutate`

#### Scenario: Asking above the grant
- **WHEN** a client granted `read` requests a token with scope `mutate`
- **THEN** the token endpoint refuses with `invalid_scope` and issues nothing

#### Scenario: Interactive approval
- **WHEN** an interactive client asks for `mutate` and the operator lists pending requests
- **THEN** the listing shows `mutate`, and an approval issues a `mutate` token only for that request's client and resource

### Requirement: No mutation setting other than the flag

`--allow-mutations` on `serve` and on the bare invocation SHALL be the only
switch for mutation authority. The agent SHALL NOT read
`OPUTE_HOST_ALLOW_MUTATIONS`, `OPUTE_STANDALONE_ALLOW_MUTATIONS` or
`OPUTE_AGENT_MODE`. They SHALL have no effect, and the agent's code, help,
schemas, errors, the recorded unit, the launcher, its README and the site
SHALL NOT mention them. At start, the agent SHALL log once whether the
bootstrap bearer may mutate.

#### Scenario: A leftover setting
- **WHEN** the agent starts with `OPUTE_HOST_ALLOW_MUTATIONS=true` and without `--allow-mutations`
- **THEN** the bootstrap bearer is read-only

### Requirement: A refusal names the grant that admits the call

A refused mutation SHALL return an error result that names the tool, its
effect, the caller's current grant, and the exact grant that would admit
the call:
- for the bootstrap bearer, `--allow-mutations`;
- for a listed client, `OPUTE_OAUTH_CLIENTS=<id>:mutate` and a token with
  scope `mutate`;
- for an interactive client, re-authorization with scope `mutate` and the
  operator's approval;
- for a provider callback, the grant of the call it acts for.

#### Scenario: Read client refused
- **WHEN** client `ci`, holding `read`, calls `create_vm`
- **THEN** the error names `create_vm`, its effect, `read`, `OPUTE_OAUTH_CLIENTS=ci:mutate` and the scope `mutate`

#### Scenario: Bearer refused
- **WHEN** the bootstrap bearer calls `create_vm` on an agent started without `--allow-mutations`
- **THEN** the error names `create_vm`, its effect, the read-only bearer and `--allow-mutations`

### Requirement: Tool annotations follow the effect

Every tool in `tools/list` SHALL carry `annotations.readOnlyHint`, `true`
exactly when its published effect is `read`. Every tool whose effect is not
`read` SHALL also carry `annotations.destructiveHint`, `true` exactly when
its effect is `destructive`. Annotations SHALL be the same for every caller,
and SHALL agree with `_meta.capability.effect`.

#### Scenario: Effects and hints agree
- **WHEN** a client lists the tools
- **THEN** every `read` tool has `readOnlyHint: true`, every `destructive` tool has `readOnlyHint: false` and `destructiveHint: true`, and every `mutation` or `credential_bearing` tool has `readOnlyHint: false` and `destructiveHint: false`
