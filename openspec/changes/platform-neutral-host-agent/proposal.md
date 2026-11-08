# Platform-neutral Host Agent

Status: decided by the Host Agent owner (2026-11-08); sign-off by the
Platform, release and deployment owners is pending and is recorded here when
given. Target: Rust 0.4.0, after the 0.3.0 cutover and its 14-day rollback
window. This change merges only after that window.

## Why

The Host Agent executes explicit typed work on a host and reports what it
observed. Opute Platform owns intent, authorization, orchestration, routing
and semantic outcomes. The agent must not know Platform exists: no mode, no
default, no name, no identity and no code path that refers to it.

Platform leaks into the agent today in five places:

| Leak | Where |
| --- | --- |
| A mode setting that switches bind, port, catalog and mutation policy | `config.rs`, `app.rs`, `catalog.rs`, `tools.rs` |
| Defaults chosen for Platform (`0.0.0.0:3004`; 3014 "to avoid the platform dogfood host") | `config.rs` |
| Platform names in a provider (Cloudflare refuses `opute.io` tunnels and hosts) | `providers/cloudflare/api.rs` |
| Platform client identities and tenancy (`opute-mcp-host`, `OPUTE_TENANT_ID`) | `oauth.rs`, `mcpprobe.rs`, `config.rs` |
| Mode-dependent behaviour in tools, recipes and providers | `hostpublic.rs`, `provider/runtime.rs`, `host_recipe_mcp.rs`, `cli.rs`, others |

## What Changes

- One serving profile, configured only by generic settings:
  `HOST_MCP_BIND_HOST` (default `127.0.0.1`), `HOST_MCP_PORT` (default
  `3014`), `OPUTE_HOST_ALLOW_MUTATIONS` (default `false`) and the configured
  authentication (required). The agent logs its effective bind, port and
  mutation policy once at start.
- `OPUTE_AGENT_MODE` and `OPUTE_STANDALONE_ALLOW_MUTATIONS` stop existing:
  never read, validated, refused, logged or documented. They have no effect,
  like any variable the agent does not define.
- OAuth machine clients come only from `OPUTE_OAUTH_CLIENTS` (ids and
  scopes), empty by default. No orchestrator-specific client id, tenant,
  enrollment, routing or callback remains.
- Reserved hostnames come only from `OPUTE_RESERVED_HOSTNAMES` (exact names
  and `*.`-suffix patterns), empty by default. A reserved name is refused with
  the typed refusal `reserved_hostname` before any effect.
- A boundary test in `make rust-check` fails when non-test Rust source, a
  published schema or description, the default configuration or the CLI help
  contains a Platform-specific identifier.
- Contract spec: "Compatible modes" keeps one profile; "Host Agent and
  Platform ownership stays separate" becomes "The Host Agent knows no
  orchestrator"; gate e2e-5 becomes "an external orchestrator client on a host
  configured by explicit settings", still run in Platform staging.

## Non-goals

- The `opute` name stays: the product name, the binary, the `OPUTE_` prefix
  and resource URIs are not Platform leaks.
- `OPUTE_REMOTE_AGENT_ID` stays an exact opaque id, reported unchanged and
  never interpreted.
- No new feature for Platform and no change to Platform's repository.
- 0.3.0 ships on the current contract; its cutover is unaffected.

## Migration

- 0.3.0 ships unchanged; the cutover and the 14-day window with Go
  selectable proceed as D28 decided.
- Platform owners move every Platform-managed host to explicit settings
  (unit files or charts) before 0.4.0 rolls out. Deleting `OPUTE_AGENT_MODE`
  is cleanup; 0.4.0 ignores it.
- 0.4.0 rollout checks each host: the logged effective bind, port and
  mutation policy, and that Platform's health check reaches it. A host
  missing its settings falls back to loopback with mutations off: safe, but
  unreachable.
- Rollback from 0.4.0 to 0.3.x or Go keeps both settings sets on the host:
  old binaries read `OPUTE_AGENT_MODE`, new ones ignore it.
- Release notes list the settings that no longer have an effect and their
  replacements.

## Decisions

| Decision | Resolution |
| --- | --- |
| Timing | 0.4.0, after the 0.3.0 cutover window |
| The old mode variable | Does not exist: ignored like any unknown variable, never refused or aliased |
| Reserved-name guard | Kept, as operator configuration, empty by default |
| OAuth clients | Operator configuration, empty by default |
| Deployment change and the new e2e-5 | Owned by the Platform owners |

Seven behaviours the proposal does not state are open questions (design.md,
N1-N7). Their parts are not implemented until the owner answers.

## Capabilities

### New Capabilities

- `platform-neutral-host-agent`: one serving profile, no mode, ordinary
  clients, configured reserved names, and the boundary test.

### Modified Capabilities

- `host-agent-contract`: compatible configuration, ownership, cutover gate
  e2e-5.
