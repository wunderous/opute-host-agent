# Credentialed OAuth token issuance in the Rust Host Agent

## Why

Host Agent is the OAuth 2.1 resource server and the co-located
authorization server for its own `/mcp` resource (Go ADR 0008, A-1). The
Rust implementation adopts a stricter token issuance contract than the pinned
Go baseline. Every token it issues must be backed by a credential the agent
trusts, or by the host operator's explicit approval given through a channel
the requester cannot reach.

Issuance is the one place where the migration deliberately does not copy
Go. The owner decided (2026-10-01) that the Rust port may deviate from Go
where the deviation is an improvement. The Go agent is not changed by this
work. The deviation is declared, scoped to issuance, and verified by its own
contract suite. All other `/mcp` and OAuth resource-server behaviour stays
under Go parity.

## What Changes

- **`client_credentials`** requires confidential-client authentication with
  a registered secret. A client without a registered secret, and every public
  client, is refused.
- **Provisioned secrets for built-in machine clients.** `opute-mcp-host`
  (from `OPUTE_HOST_OAUTH_CLIENT_SECRET`, otherwise generated) and a new
  `host-agent-provider`.
  - The store holds only hashes.
  - The plaintext is delivered only through 0600 files in
    `<state>/credentials/`, never on argv, in MCP results or in logs.
  - `oauth rotate-secret` rotates a secret and revokes the client's tokens.
- **`authorization_code` requires operator approval.**
  - `/oauth/authorize` validates the request and records a pending
    authorization with a user code. It shows a page with no approve control
    and waits.
  - The operator approves or denies with
    `opute-host-agent oauth approve|deny <code>`, which works on the state
    directory.
  - Approval can be remembered per client and resource, and revoked.
- **Audience restriction.** An issued token may name only a resource this
  agent serves: the issuing request's canonical `/mcp` URI, or the endpoint
  of an active public MCP binding.
- **Hardening.**
  - Exact redirect matching, with the RFC 8252 loopback-port rule.
  - Client ID metadata fetches pinned to the vetted address, with no
    redirects.
  - Backoff on failed grants, and limits on pending requests.
  - Audit lines without secrets.
- **Self-probes** of the agent's own endpoints mint short-lived tokens in
  process and never call the token endpoint.
- **Provider contract.**
  - Co-located providers keep calling back over loopback with the bearer
    from their environment file, unchanged.
  - A provider that must reach the agent through its public URL (tunnel
    activation evidence) authenticates as `host-agent-provider`, with the
    secret file named by `OPUTE_HOST_AGENT_CLIENT_SECRET_FILE`.
- **Migration.** The first start of a build with this contract revokes
  token rows written before it.
- **Declared divergence.**
  - The parity harness gains declared divergences: comparison paths excluded
    from Go-vs-Rust only when they cite an approved decision (D8).
  - It also gains single-implementation contract scenarios that assert the
    Rust behaviour directly.

## Capabilities

### New Capabilities

- `oauth-issuance`: the credential, approval, audience and hardening rules
  for every token the Rust Host Agent issues.

### Modified Capabilities

- `host-agent-contract`: "No capability change during the language
  migration" admits owner-approved, declared divergences, of which
  `oauth-issuance` is the first. "Compatible authenticated MCP surface" names
  `oauth-issuance` as the Rust issuance contract.

## Non-goals

- No change to the Go Host Agent.
- No change to bearer validation, the bootstrap token's local scope (A-6),
  token lifetime, PRM and AS metadata, revocation by token value, or the MCP
  wire.
- No dynamic client registration, no refresh tokens, no product IdP.
- No change to provider resource delegations
  (`host-resource-delegation.v1`).

## Impact

- **Rust:** `authz` store and handlers, the CLI, the probe, and the
  provider launch environment (when providers land in M7).
- **Parity harness:** divergence declarations, a contract suite, and an
  issuance gate.
- **Opute Platform:** must authenticate as `opute-mcp-host` with its
  configured secret when talking to a Rust host.
- **Providers:** the Cloudflare provider's public-URL activation check needs
  the provider credential file before it works against a Rust host. Until
  then that one check fails closed with an explicit error, and co-located
  callbacks are unaffected.
- **Operators:** of interactive OAuth clients approve each new client once.
