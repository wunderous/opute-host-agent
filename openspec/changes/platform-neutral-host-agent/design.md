# Design: platform-neutral Host Agent

## Context

`Config::load` resolves a mode from `OPUTE_AGENT_MODE`; an empty or unknown
value is **platform**. The CLI decides first, though: `serve` takes `--mode`,
then `OPUTE_AGENT_MODE`, and otherwise sets `standalone`, and the
`standalone` command is `serve --mode=standalone`. The npm launcher passes
`--mode=standalone --transport=http` (legacy inventory O-13). So a host
started through the CLI with no mode is standalone today. Only a host that
sets `OPUTE_AGENT_MODE=platform` or `--mode=platform` runs as platform. The
mode selects:

| Concern | platform (opt-in) | standalone (CLI default) |
| --- | --- | --- |
| Bind, port | `0.0.0.0:3004` | `127.0.0.1:3014` |
| Catalog | host definitions (130) | host plus standalone definitions |
| Mutation gate | none | D10 read-only gate unless `OPUTE_STANDALONE_ALLOW_MUTATIONS=true` |
| Default instance id (and so the instance root, state, databases) | `platform` | `standalone` |
| State store opened by `check` | no | yes |
| `local_instance_id` reported | empty | `OPUTE_LOCAL_HOST_AGENT_INSTANCE_ID` |
| `OPUTE_MCP_URL`, `OPUTE_MCP_HEALTH_URL` | allowed | refused |

## Decisions in this change

- The D10 gate (`standalone-read-only-gate`) keeps its classification and
  position. It is keyed on `OPUTE_HOST_ALLOW_MUTATIONS` and refuses with
  `mutations are disabled; set OPUTE_HOST_ALLOW_MUTATIONS=true`. Its
  "Platform mode" scenario no longer exists. That change is not archived, so
  its spec text is amended when it is archived (task 5.2); the contract
  suite already follows this change (`gate.platform-unaffected` removed).
- `OPUTE_MCP_URL` and `OPUTE_MCP_HEALTH_URL` were refused only in
  standalone mode, as Platform settings (legacy inventory K-4). Like the
  mode variables, they now have no effect.
- `OPUTE_OAUTH_CLIENTS` is comma-separated `id` or `id:scopes`; `mcp` is
  the only scope the issuer has, so another scope, a duplicate, an empty
  entry or a built-in id fails the start. Each listed client gets a
  generated secret in the credentials directory, as provisioning already
  did, and `oauth rotate-secret` accepts it. A machine client no longer
  listed is deprovisioned at start (tokens revoked, row and credential
  removed), which also removes the earlier default client from upgraded
  hosts. `OPUTE_HOST_OAUTH_CLIENT_SECRET` (that client's operator secret)
  has no effect.
- The agent's probes (`ensure_public_mcp_tunnel`, the quick tunnel, the
  Cloudflare provider's public check) mint as the first listed client with
  its secret. Before this change they sent the old client id without a
  secret, which the issuance contract refuses. With no client listed, a
  probe sends nothing and its observation is `verified: false`,
  `authorizationMode: "unverified"`. An origin probe does not fail on that.
  The provider's callback over a public route still needs a client.
- N5 applies equally to the recorded deploy unit
  (`deploy/opute-bootstrap-mcp.service`, pinned by the distribution
  contract). It runs `--mode standalone` and enables mutations with
  `OPUTE_STANDALONE_ALLOW_MUTATIONS=true`. On 0.4.0 it would fail to start
  (unknown flag) and, once fixed, would lose mutations. Before 0.4.0
  ships, both the npm launcher and the deploy unit drop `--mode`, and the
  unit sets `OPUTE_HOST_ALLOW_MUTATIONS=true` next to the old variable
  (both settings sets, as for rollback). Those files belong to their own
  repositories and change at an explicit cutover. This change records the
  prerequisite and does not edit them.
- The parity runner's `platform` profile, used by contract cases that need
  mutations, is now the one profile with `OPUTE_HOST_ALLOW_MUTATIONS=true`.

- The single profile takes the defaults the proposal lists, which are the
  standalone ones. Admission follows the D10 gate keyed on
  `OPUTE_HOST_ALLOW_MUTATIONS`.
- Mode reading is deleted, not aliased: nothing parses `OPUTE_AGENT_MODE`,
  so it cannot be refused or logged.
- The reserved-name guard moves from constants to the operator setting
  `OPUTE_RESERVED_HOSTNAMES`: comma-separated, each an exact,
  case-insensitive name or `*.suffix`, which matches any name under `suffix`
  and never the bare suffix. It checks the requested hostnames and the
  existing ingress of the tunnel being changed, before any write, and refuses
  with an error result whose structured content is
  `{code: "reserved_hostname", message, hostname, pattern}`. The Platform
  tunnel-name guard goes: a tunnel name is no longer refused for what it is,
  only required. A Platform tunnel stays protected through its ingress once
  its hostnames are reserved. The M9 provider-wire corpus still names the
  old tunnel; it is historical Go-parity evidence and is not rerun.
- The boundary test is a Rust test in the host-agent crate, run by
  `make rust-check`. It scans `src/` outside `#[cfg(test)]` modules and test
  files, every descriptor's description and schema, `Config::load` of an empty
  environment, and the CLI help text. A finding names the file or surface and
  the identifier.

## Questions the proposal left open

The proposal left these unstated. Each changes durable state, identity or
the published catalog, so none was guessed. The owner adopted every
recommendation (2026-11-08); the last column is now the decision.

| # | Question | Why it matters | Decision |
| --- | --- | --- | --- |
| N1 | Which catalog does the single profile publish? | Today platform publishes the host definitions only, and standalone adds its own. One profile can publish only one catalog. | Today's standalone catalog, which every CLI host without a mode already publishes, with the D10 gate on `OPUTE_HOST_ALLOW_MUTATIONS`. Platform-mode hosts gain the standalone tools and a new revision. |
| N2 | Where does a host's state live by default? | The default instance id is `platform` or `standalone`, so a Platform host's state is under `instances/platform`. A 0.4.0 default of `standalone` would start such a host on empty state. | Keep `standalone` as an opaque default id (it is a directory name, not a mode). The migration adds `OPUTE_HOST_AGENT_INSTANCE=platform` to the explicit settings of every Platform-managed host. |
| N3 | Do tenants stay? | The proposal forbids tenant fields, but the tenant is a segment of the canonical resource URI (`opute://type/tenant/id`), is stored in durable rows, and is named by the contract spec ("tenant-scoped canonical resource URIs"). Resource URIs are a non-goal. | Keep the URI segment and `OPUTE_TENANT_ID` as a generic resource namespace (default `local`), and exempt it from the boundary test. Removing it is a resource-identity change for its own proposal. |
| N4 | What does the agent's own public-route probe authenticate as? | `opute-mcp-host` is also the built-in confidential client that `ensure_public_mcp_tunnel` and the MCP probe use to mint an audience-bound token from the published host. Removing it with no configured client breaks that verification. | Move the probe to a client named in `OPUTE_OAUTH_CLIENTS`. With none configured the probe reports `unverified`, rather than failing or minting with a built-in id. |
| N5 | What happens to `--mode` and the `standalone` command? | The published npm launcher passes `--mode=standalone`, and the launcher is a compatibility surface owned until an explicit cutover. Removing the flag makes `flag provided but not defined` fail every launcher start; keeping it reads a mode setting, which the proposal forbids. | Ship the launcher change first (serve with no `--mode`), then remove `--mode` in 0.4.0. Keep `standalone` as a plain alias of `serve` (O-14 already names one spelling as the goal), and add no mode behind it. Until the launcher change ships, 0.4.0 stays held. |
| N6 | What happens to the host recipe value `coordinator: platform`? | `host-recipe.v1` documents name their coordinator. A Host Agent runs `coordinator: host-agent` / `mode: local` and refuses `coordinator: platform` / `mode: distributed`, telling the author to submit it to the Platform coordinator. The value is part of the recipe format, and recipe files in the field use it. | Keep the document value (it names who coordinates, not how this agent behaves). Make the refusal text neutral ("submit it to its distributed coordinator"), and exempt the constant from the boundary test with a recorded reason. Renaming it is a recipe-format change for its own proposal. |
| N7 | Does the PostgreSQL relay's default Incus device name change? | `opute-platform-postgres-rw` is the default device name for the host forward. Existing VMs carry that device; a new default would add a second device next to it and leave the old one behind. | Keep the name for existing devices: adopt a device with the old name when present, and use a neutral default (`opute-postgres-rw`) only when none exists. |

## Found by the journeys (for the Platform owners)

- **A machine client's secret.** 0.3.x accepts an operator-supplied
  secret for `opute-mcp-host` (`OPUTE_HOST_OAUTH_CLIENT_SECRET`) and then
  writes no credential file. 0.4.0 ignores that variable. A host that lists
  the client in `OPUTE_OAUTH_CLIENTS` keeps a *generated* credential across
  upgrade and rollback (`neutral.rollback`), but a host that relied on the
  operator-supplied secret gets a newly generated one at its first 0.4.0
  start, and its orchestrator must read the new credential file. The
  migration notes need this step; whether 0.4.0 should accept
  operator-supplied secrets is a question for the owners.
- **A host that lists no client** loses `opute-mcp-host` at its first
  0.4.0 start; 0.3.x provisions it again, with a new secret, on rollback.
- **Catalog size.** A host that ran in platform mode publishes 130 tools
  on 0.3.x and 142 on 0.4.0 (N1), and 130 again after rollback.

