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

## Open questions (owner)

The proposal leaves these unstated. Each changes durable state, identity or
the published catalog, so none is guessed. Each lists a recommendation.

| # | Question | Why it matters | Recommendation |
| --- | --- | --- | --- |
| N1 | Which catalog does the single profile publish? | Today platform publishes the host definitions only, and standalone adds its own. One profile can publish only one catalog. | Today's standalone catalog, which every CLI host without a mode already publishes, with the D10 gate on `OPUTE_HOST_ALLOW_MUTATIONS`. Platform-mode hosts gain the standalone tools and a new revision. |
| N2 | Where does a host's state live by default? | The default instance id is `platform` or `standalone`, so a Platform host's state is under `instances/platform`. A 0.4.0 default of `standalone` would start such a host on empty state. | Keep `standalone` as an opaque default id (it is a directory name, not a mode). The migration adds `OPUTE_HOST_AGENT_INSTANCE=platform` to the explicit settings of every Platform-managed host. |
| N3 | Do tenants stay? | The proposal forbids tenant fields, but the tenant is a segment of the canonical resource URI (`opute://type/tenant/id`), is stored in durable rows, and is named by the contract spec ("tenant-scoped canonical resource URIs"). Resource URIs are a non-goal. | Keep the URI segment and `OPUTE_TENANT_ID` as a generic resource namespace (default `local`), and exempt it from the boundary test. Removing it is a resource-identity change for its own proposal. |
| N4 | What does the agent's own public-route probe authenticate as? | `opute-mcp-host` is also the built-in confidential client that `ensure_public_mcp_tunnel` and the MCP probe use to mint an audience-bound token from the published host. Removing it with no configured client breaks that verification. | Move the probe to a client named in `OPUTE_OAUTH_CLIENTS`. With none configured the probe reports `unverified`, rather than failing or minting with a built-in id. |
| N5 | What happens to `--mode` and the `standalone` command? | The published npm launcher passes `--mode=standalone`, and the launcher is a compatibility surface owned until an explicit cutover. Removing the flag makes `flag provided but not defined` fail every launcher start; keeping it reads a mode setting, which the proposal forbids. | Ship the launcher change first (serve with no `--mode`), then remove `--mode` in 0.4.0. Keep `standalone` as a plain alias of `serve` (O-14 already names one spelling as the goal), and add no mode behind it. Until the launcher change ships, 0.4.0 stays held. |
| N6 | What happens to the host recipe value `coordinator: platform`? | `host-recipe.v1` documents name their coordinator. A Host Agent runs `coordinator: host-agent` / `mode: local` and refuses `coordinator: platform` / `mode: distributed`, telling the author to submit it to the Platform coordinator. The value is part of the recipe format, and recipe files in the field use it. | Keep the document value (it names who coordinates, not how this agent behaves). Make the refusal text neutral ("submit it to its distributed coordinator"), and exempt the constant from the boundary test with a recorded reason. Renaming it is a recipe-format change for its own proposal. |
| N7 | Does the PostgreSQL relay's default Incus device name change? | `opute-platform-postgres-rw` is the default device name for the host forward. Existing VMs carry that device; a new default would add a second device next to it and leave the old one behind. | Keep the name for existing devices: adopt a device with the old name when present, and use a neutral default (`opute-postgres-rw`) only when none exists. |
