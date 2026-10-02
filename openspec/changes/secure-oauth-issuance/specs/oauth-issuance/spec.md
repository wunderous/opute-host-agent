## ADDED Requirements

### Requirement: Every issued token is backed by a credential or an operator approval

Host Agent SHALL issue an OAuth access token only when the request proves a
confidential client's registered secret (`client_credentials`) or redeems a
code from an authorization request the host operator approved
(`authorization_code`). No issuing path SHALL accept a request whose only
proof is network location, the `Host` header, the `Origin` header, or
possession of a client identifier.

#### Scenario: client_credentials without a secret
- **WHEN** a caller posts `grant_type=client_credentials` for any client and omits the client secret or sends a wrong one
- **THEN** the token endpoint answers 401 `invalid_client` and no token row is written

#### Scenario: Confidential client with no registered secret
- **WHEN** a client is registered as confidential but has no stored secret hash and a caller requests `client_credentials` for it with any secret
- **THEN** the token endpoint answers 401 `invalid_client`

#### Scenario: Public client cannot use client_credentials
- **WHEN** a caller requests `client_credentials` for `host-agent-bootstrap` or a CIMD client
- **THEN** the token endpoint answers 400 `unauthorized_client` and no token row is written

#### Scenario: Unapproved authorization request
- **WHEN** a client starts `/oauth/authorize` and the operator has not approved the request and no remembered consent applies
- **THEN** no authorization code is issued and the client never receives a redirect carrying a code

### Requirement: Provisioned credentials for built-in machine clients

Host Agent SHALL hold a secret for each built-in confidential client:
`opute-mcp-host` (from `OPUTE_HOST_OAUTH_CLIENT_SECRET` when set, otherwise
generated) and `host-agent-provider` (always generated). Generated secrets
SHALL be at least 256 bits of randomness. The store SHALL keep only their
hashes. The plaintext SHALL be delivered only through files under
`<state>/credentials/` with mode 0600 in a 0700 directory owned by the agent
user. Secrets SHALL NOT appear on any command line, in any MCP result, or in
any log line.

#### Scenario: First start provisions credentials
- **WHEN** Host Agent starts with an empty state directory
- **THEN** `credentials/host-agent-provider.json` and, absent an operator secret, `credentials/opute-mcp-host.json` exist with mode 0600, and `authz.sqlite` holds only their hashes

#### Scenario: Operator-configured Platform secret wins
- **WHEN** `OPUTE_HOST_OAUTH_CLIENT_SECRET` is set
- **THEN** `opute-mcp-host` authenticates with that secret and no `opute-mcp-host.json` file is written

#### Scenario: Rotation revokes outstanding tokens
- **WHEN** the operator rotates a client's secret
- **THEN** a new secret file is written atomically, the old secret stops working, and every token issued to that client is revoked

### Requirement: Provider access paths

Co-located provider processes SHALL keep reaching Host Agent over the
loopback endpoint with the bearer from the environment file their install
recipe gives them; this path SHALL need no token grant. A provider that
reaches Host Agent through a public URL SHALL obtain its token with
`client_credentials` as `host-agent-provider`, authenticating with the secret
from the file named by `OPUTE_HOST_AGENT_CLIENT_SECRET_FILE`, which Host Agent
SHALL include, with `OPUTE_HOST_AGENT_CLIENT_ID`, in the environment it
gives providers. A token request from a provider without that secret SHALL be
refused like any other unauthenticated request.

#### Scenario: Co-located callback
- **WHEN** a Cloudflare or Tailscale provider installed by its recipe calls back to the agent through the loopback endpoint with its environment-file bearer
- **THEN** the call is authorized without any token grant

#### Scenario: Public activation evidence with the provider credential
- **WHEN** the Cloudflare provider verifies a tunnel by authenticated `tools/list` through the public URL after obtaining a token as `host-agent-provider` with its secret
- **THEN** the token is issued for the binding's public endpoint and `tools/list` succeeds

#### Scenario: Provider without the credential
- **WHEN** a provider requests a token through the public URL without client authentication
- **THEN** the token endpoint answers 401 `invalid_client`, no token is written, and the provider reports activation evidence as failed

### Requirement: Self-probes use in-process tokens

When Host Agent probes one of its own MCP resources (activation evidence,
`public-mcp`, the MCP exposure probe), it SHALL mint a token directly in its
store. That token SHALL have a lifetime of at most five minutes and SHALL be
bound to the probed resource, and the agent SHALL revoke it after the probe.
Host Agent SHALL NOT call its own token endpoint for a probe. A probe of an
endpoint that is not one of its own resources SHALL require a caller-supplied
bearer.

#### Scenario: Probing the agent's own public endpoint
- **WHEN** `public-mcp` probes the public endpoint of its active binding without a bearer
- **THEN** the probe succeeds with `authorizationMode` `in-process` and the probe token is revoked afterwards

#### Scenario: Probing a foreign endpoint without a bearer
- **WHEN** the MCP exposure probe targets an endpoint that is not one of this agent's resources and no bearer is supplied
- **THEN** the probe fails without minting any token

### Requirement: Operator approval of interactive authorization requests

`/oauth/authorize` SHALL validate the client, the exact redirect URI, PKCE
`S256` and the resource. It SHALL then record a pending authorization request
with a random identifier, a short user code and an expiry of at most ten
minutes, and show an approval page. Approval or denial SHALL be possible only
through `opute-host-agent oauth approve|deny`, or through the
`approve_oauth_authorization` admin tool (published with the catalog) called
with the bootstrap bearer on a local `Host`. No route on the HTTP listener SHALL approve a request. The
approval page SHALL contain no approval control and SHALL forbid framing and
caching.

#### Scenario: Operator approves from the CLI
- **WHEN** a CIMD client starts authorization and the operator runs `opute-host-agent oauth approve <user code>` before expiry
- **THEN** the waiting page redirects to the registered redirect URI with a single-use code, `iss` and `state`, and the code redeems with the matching PKCE verifier for the requested resource

#### Scenario: Operator denies
- **WHEN** the operator runs `opute-host-agent oauth deny <user code>`
- **THEN** the client is redirected with `error=access_denied` and no code exists for that request

#### Scenario: Request expires
- **WHEN** ten minutes pass without a decision
- **THEN** the request is marked expired, the page reports expiry, and a later approval of that user code fails

#### Scenario: Approval is not reachable over HTTP
- **WHEN** any HTTP request to the listener, authenticated or not, attempts to change a pending request's status
- **THEN** the request has no effect on the pending authorization

#### Scenario: Remembered consent
- **WHEN** the operator approved a client for a resource with consent remembered and the same client authorizes the same resource before the consent expires
- **THEN** the code is issued without a pending step, and revoking the consent makes the next authorization pending again and revokes that client's tokens for the resource

### Requirement: Issued tokens name only resources this agent serves

The `resource` bound to an issued token SHALL be either the canonical MCP URI
of the issuing request or the endpoint of an active public MCP binding of
this agent. Any other resource SHALL be refused with `invalid_target`.

#### Scenario: Minting for an arbitrary host
- **WHEN** an authenticated client requests a token for `https://unrelated.example/mcp`
- **THEN** the token endpoint answers 400 `invalid_target` and writes no token row

#### Scenario: Minting for the active public binding
- **WHEN** the provider client requests a token for the endpoint of an active public MCP binding through the loopback listener
- **THEN** the token is issued with that resource

### Requirement: Redirect, client metadata and abuse hardening

Redirect URIs SHALL match a registered value exactly. The one exception is
loopback redirects, which SHALL match on scheme, host and path with any port.
Client ID metadata documents SHALL be fetched over HTTPS without redirects,
from the address vetted at resolution time, and SHALL be refused for
loopback, private, link-local, unspecified, multicast and metadata addresses.
Repeated failed grants SHALL be rate limited per client and per remote
address. The limit SHALL apply only to failing requests: a request that
proves a valid secret or redeems a valid code SHALL be served even while its
client or address is limited, because callers behind a tunnel share the
loopback address. Pending authorization requests SHALL be capped in total and
per client.

#### Scenario: Redirect prefix trick
- **WHEN** a registered client's request uses `http://127.0.0.1.attacker.example/cb`
- **THEN** authorization is refused as an invalid redirect

#### Scenario: CIMD DNS rebinding
- **WHEN** a client metadata host resolves to a public address at check time and to a private address afterwards
- **THEN** the fetch connects only to the vetted public address

#### Scenario: Failures from a shared address do not lock out a valid client
- **WHEN** repeated failed grants from the loopback address put it under backoff and the provider then requests a token with its valid secret from the same address
- **THEN** the token is issued, while a further failing request answers 429 `slow_down`

#### Scenario: Pending request flood
- **WHEN** a client already has the maximum number of pending requests
- **THEN** further authorization requests answer `temporarily_unavailable` without creating rows

### Requirement: Issuance is audited without secrets

Host Agent SHALL log one structured line per token issuance, denial,
approval, consent change, rotation and revocation. Each line SHALL carry the
client identifier, grant, resource, outcome and remote address, and SHALL NOT
contain a secret, token, authorization code or PKCE verifier.

#### Scenario: Audit line content
- **WHEN** a provider obtains a token
- **THEN** one log line records `client_id=host-agent-provider`, the grant, the resource and success, and a scan of the log finds neither the secret nor the token

### Requirement: Migration revokes pre-existing tokens

On the first start of a build that implements this capability, Host Agent
SHALL revoke every token row written before the migration, make
`host-agent-bootstrap` a public client, provision any missing built-in
secrets, and record the migration. The migration SHALL be idempotent, SHALL
delete no data, and SHALL leave every pre-existing table and column with its
Go meaning so that the Go agent can still open the store.

#### Scenario: Upgrade with live tokens
- **WHEN** a host with unexpired tokens starts this build for the first time
- **THEN** those tokens are rejected with 401, and bootstrap-bearer access keeps working

#### Scenario: Idempotent migration
- **WHEN** the build restarts after the migration
- **THEN** no further tokens are revoked and no credential is regenerated

#### Scenario: Go can still open the store
- **WHEN** the Go agent opens an `authz.sqlite` that this build migrated
- **THEN** it starts, and bearer validation of a token issued by this build gives the same outcome in both
