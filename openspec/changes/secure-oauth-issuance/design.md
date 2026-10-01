# Design: credentialed OAuth issuance in the Rust Host Agent

## 1. Principle

> A token is issued only to a caller that proves something the agent already
> trusts: a secret it provisioned, a secret the operator configured, or the
> operator's own approval given through a channel the requester cannot reach.

The agent never has to tell a legitimate caller from an attacker by where a
request came from. Peer address, `Host`, `Origin` and "came through the
tunnel" are all forgeable or meaningless behind cloudflared. They remain
defence in depth, never the gate.

## 2. Callers and how each is served

```text
 caller                              how the Rust agent serves it
 ----------------------------------  ------------------------------------------------
 F1 local client, bootstrap          Bearer MCP_AUTH_TOKEN, local Host only (A-6)
 F2 Opute Platform (opute-mcp-host)  client_credentials + secret (operator-configured,
                                     else provisioned file)
 F3 provider callbacks               loopback + env-file bearer when co-located;
    (Cloudflare, Tailscale)          client_credentials + host-agent-provider secret
                                     through a public URL
 F4 tunnel activation evidence (P-4) same as F3 through the public URL
 F5 agent self-probe                 token minted in-process for the probe target;
    (public-mcp, probe tool)         no HTTP grant
 F6 interactive OAuth client         authorize -> pending request -> operator
    (CIMD, native loopback)          approves -> code
 F7 revoke (RFC 7009)                by token value
```

| Flow | Proof required | Where the proof comes from | Who can obtain it |
| --- | --- | --- | --- |
| F1 | bootstrap bearer, local `Host` | `MCP_AUTH_TOKEN` (env/env file) | whoever controls the agent's environment |
| F2 | `opute-mcp-host` secret | `OPUTE_HOST_OAUTH_CLIENT_SECRET`, or `credentials/opute-mcp-host.json` | Platform's secret store / the host operator |
| F3, F4 | `host-agent-provider` secret | `credentials/host-agent-provider.json`, passed by the provider contract | the agent's providers |
| F5 | none over the wire | the agent's own store | the agent process |
| F6 | operator approval of one pending request | `opute-host-agent oauth approve <code>` (and, from M3, the `approve_oauth_authorization` tool with the bootstrap bearer on a local `Host`) | the host operator |

## 3. Mechanisms

### 3.1 Confidential clients and secrets

- `clients.secret_hash` is required for `client_credentials`. A
  confidential client with an empty hash gets `invalid_client` (401),
  whatever secret it sends. A public client gets `unauthorized_client` (400).
- Secrets are 256-bit random values, encoded as `ohs_` + 64 hex. Only
  SHA-256 hashes are stored. Comparison is constant time on the hashes.
- Client authentication accepts `client_secret_basic` and
  `client_secret_post` (as advertised). `none` stays advertised only for
  public clients on `authorization_code`.
- Built-in clients:

| Client | Type | Grants | Secret source |
| --- | --- | --- | --- |
| `opute-mcp-host` | confidential | `client_credentials` | `OPUTE_HOST_OAUTH_CLIENT_SECRET` if set; otherwise generated on first start and written to `credentials/opute-mcp-host.json` |
| `host-agent-provider` (new) | confidential | `client_credentials` | generated on first start, `credentials/host-agent-provider.json` |
| `host-agent-bootstrap` | **public** (holds no secret) | `authorization_code` + PKCE, with approval | none |
| CIMD clients | public | `authorization_code` + PKCE, with approval | none |

### 3.2 Credential files

```text
<state dir>/credentials/            0700, owner = agent user
  opute-mcp-host.json               0600  {"client_id","client_secret","token_endpoint_auth_method":"client_secret_basic","rotated_at"}
  host-agent-provider.json          0600  same shape
```

- They are written atomically (temp file, fsync, rename) only when the stored
  hash is missing or rotation is requested. They are never logged or returned
  by a tool, and never placed on argv.
- **Provider contract.** Providers run as systemd units created by their
  install recipes, with `OPUTE_HOST_AGENT_ENDPOINT` set to the agent's local
  `mcpEndpoint` and the agent's `EnvironmentFile`.
  - **Co-located callbacks (unchanged).** Providers call back over loopback
    with the bearer in that environment file, the F1 path. They need no token
    grant and keep working against Rust as they are.
  - **Public-URL access (changed).** Tunnel activation evidence (P-4) must
    traverse the tunnel. A provider authenticates as `host-agent-provider`,
    using `client_secret_basic` with the secret from the file named by
    `OPUTE_HOST_AGENT_CLIENT_SECRET_FILE`. Rust adds that variable, and
    `OPUTE_HOST_AGENT_CLIENT_ID`, to the environment it gives providers.
  - **Without the credential, fail closed.** A provider that has not adopted
    this contract gets `invalid_client` and reports activation evidence as
    failed. It is never served an unauthenticated token. The provider-side
    change is a few lines and ships with providers for the Rust agent (M7, or
    M9 when a provider is ported).

### 3.3 In-process tokens for self-probes (F5)

The Rust MCP exposure probe (the port of Go `mcpprobe`, arriving with
`public-mcp` in M8) never mints over HTTP.

- When the target is one of this agent's own resources (a local canonical URI
  or an active public binding endpoint), the agent inserts a token row
  directly: client `host-agent-probe`, scope `mcp`, TTL 5 minutes, resource =
  target. It probes, then revokes the row.
- When the target is not this agent's, the caller must supply a bearer, and
  there is no implicit minting.
- `authorizationMode` in the probe result reads `in-process` in place of
  `client-credentials`.

### 3.4 Operator-approved authorization (F6)

```text
 browser / MCP client            Host Agent                       operator (trusted channel)
 --------------------            ----------                       --------------------------
 GET /oauth/authorize ...  --->  validate client, exact redirect,
                                 PKCE S256, resource ∈ served
                                 create pending request
                                   id (128-bit), user code ABCD-EFGH,
                                   expires 10 min
                          <---   200 approval page: client, redirect
                                 host, resource, user code;
                                 polls /oauth/authorize/status
                                                                   $ opute-host-agent oauth pending
                                                                   ABCD-EFGH  claude.ai  https://h/mcp
                                                                   $ opute-host-agent oauth approve ABCD-EFGH
                                 mark approved (or denied)  <----
 GET status?request=id     --->  approved: 302 redirect_uri?code&iss&state
                                 denied/expired: redirect error=access_denied
 POST /oauth/token (code,  --->  single use, client, redirect, PKCE,
   verifier)                     resource bound to the request
                          <---   access token
```

- The approval page has **no approve control**. Approval only arrives
  out-of-band, so a cross-site request, clickjacking or a forged form can't
  grant anything. The page is served with `Cache-Control: no-store`,
  `X-Frame-Options: DENY`, `Content-Security-Policy: default-src 'none';
  frame-ancestors 'none'` and `Referrer-Policy: no-referrer`.
- **Trusted channels.**
  - The CLI (`oauth pending|approve|deny|revoke-client|rotate-secret`) operates
    on `authz.sqlite` directly. Being able to open the state directory *is* the
    trust boundary.
  - The `approve_oauth_authorization` / `list_oauth_authorizations` admin
    tools require the bootstrap bearer on a local `Host`, the same trust as F1.
    They are published with the catalog (M3). Until then, the CLI is the only
    channel.
  - Neither channel is reachable through a tunnel by a token the requester
    could obtain.
- **Remembered consent.** An approval can be recorded for
  `(client_id, resource)` for N days (default 30, `0` disables). A later
  authorization for the same pair then skips the pending step. Consent is
  listed and revocable, and revoking it also revokes that client's tokens for
  the resource.
- **Limits.** At most 20 pending requests in total and 3 per client; when
  full, new requests get `temporarily_unavailable`. Status polling is limited
  to one request a second per request id.

### 3.5 Audience restriction

The `resource` of an issued token must be one of these:

1. the canonical MCP URI of the issuing request (`http(s)://<Host>/mcp`,
   unchanged rule A-5);
2. the endpoint of an active public MCP binding recorded by `public-mcp` or
   a tunneling activation.

Anything else gets `invalid_target` (RFC 8707). This closes "mint for any
Host you will later send", even for an authenticated client.

### 3.6 Remaining hardening

- **Redirects.** Exact string match against registered redirect URIs, with
  one exception: loopback redirects (`http://127.0.0.1`, `http://[::1]`,
  `http://localhost`) match any port, per RFC 8252 §7.3. The prefix-match
  fallback for registered clients is removed.
- **CIMD.** HTTPS only, no redirects, 1 MiB limit, 5 s timeout. DNS is
  resolved once and the connection goes to the vetted address (no rebinding
  between check and use). Loopback, private, link-local, unspecified,
  multicast and metadata addresses are rejected, and so are `localhost`
  names.
- **Failed grants.** Exponential backoff per client and per remote address
  after 5 failures in 60 s; the backoff applies only to failures.
- **Audit.** One structured log line per issuance, denial, approval,
  rotation and revocation, carrying client id, grant, resource, outcome and
  remote address. It never contains a secret, token, code or verifier.
- **Rotation.** `oauth rotate-secret <client>` writes a new secret and
  revokes every token issued to that client.

## 4. Data model (authz.sqlite)

Additive. Every existing table and column keeps its Go meaning, so Go can
still open a store Rust has migrated (decision D1); Go ignores the new tables.

```sql
-- existing: clients, codes, tokens (unchanged columns)
CREATE TABLE IF NOT EXISTS pending_authorizations (
  id TEXT PRIMARY KEY,            -- 128-bit random, hex
  user_code TEXT NOT NULL UNIQUE, -- ABCD-EFGH, uppercase, no ambiguous characters
  client_id TEXT NOT NULL,
  redirect_uri TEXT NOT NULL,
  resource TEXT NOT NULL,
  code_challenge TEXT NOT NULL,
  state TEXT NOT NULL DEFAULT '',
  status TEXT NOT NULL,           -- pending | approved | denied | expired | redeemed
  decided_by TEXT NOT NULL DEFAULT '', -- cli | admin-tool | consent
  created_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS consents (
  client_id TEXT NOT NULL,
  resource TEXT NOT NULL,
  granted_at INTEGER NOT NULL,
  expires_at INTEGER NOT NULL,
  PRIMARY KEY (client_id, resource)
);
-- clients: client_type for host-agent-bootstrap becomes 'public', confidential = 0
```

Migration (idempotent, on open):

1. Create the new tables.
2. Set `host-agent-bootstrap` to public.
3. Provision the missing built-in secrets.
4. **Revoke every token row present before the migration**, recording the
   migration in a `schema_migrations` row (`oauth-issuance.v1`). The token TTL is one hour, so
   at worst a caller re-authenticates once.

## 5. Invariant delta

| Kind | Invariant | Evidence |
| --- | --- | --- |
| Preserve | A-1..A-7, S-4..S-6, T-*, unauthenticated `/health`, bearer validation semantics | M2 wire corpus stays Go-vs-Rust clean outside declared divergences |
| Strengthen | A-3: "an OAuth access token issued for this resource" now means issued *to an authenticated client or an approved request* | `oauth-issuance` contract suite |
| Strengthen | A-5: the issuing request may only name a served resource | `invalid_target` cases |
| Introduce | O-1 no issuing path without a credential or operator approval | canary K1: accepting an empty secret turns the contract suite red |
| Introduce | O-2 operator approval only via channels unreachable by the requester | canary K15: an approve route on the HTTP listener turns the contract suite red |
| Introduce | O-3 secrets live only as hashes in the store and in 0600 files | contract file-mode, hash-only and log-scan checks |
| Diverge (D8) | Go's `client_credentials` and `/oauth/authorize` behaviour is not reproduced | divergence declarations cite D8; stale-divergence check |

## 6. Declared divergence in the parity harness

Rust intentionally differs from Go here, so Go-vs-Rust evidence must neither
hide the difference nor fail on it forever.

- **Divergence declarations.** Each divergent difference is defined once in
  `tools/parity/divergences.json` (id, `decision`, `reason`, a path, and what
  to drop there: list items by field, object keys, or log lines) and named
  by id in a scenario's `compare.divergences`. The comparator drops that
  content from both sides of a Go-vs-Rust comparison and records each
  declaration in the evidence bundle. Declarations never apply to Go-vs-Go.
  The verifier fails if a declaration cites a decision that
  `parity-manifest.json` does not list as approved, if a declared divergence
  was not applied, or if it is stale (the dropped content is identical on
  both sides).
- **Contract suite.** `tools/parity/contracts/oauth-issuance.json` runs
  against one implementation and asserts expected values (status, header
  and JSON fields, CLI exit, store rows, file modes, log content). Steps can
  capture values (a provisioned secret, a user code, a code) for later
  steps. The suite runs every flow in §2 and every negative case in the
  spec against Rust, and the `m2` and `cutover` gates require it to pass.
- **Canaries for Rust.** `tools/parity/rust-canaries.json` patches the Rust
  source; each patched build must turn its named contract scenario red,
  while the unpatched build stays green. The mutations include accepting an
  empty secret, an HTTP approval route, accepting any resource, implicit
  consent, prefix redirect matching, world-readable credential files, code
  replay, and a secret in the audit log.

## 7. Rollback and data preservation

- The migration only revokes tokens and adds tables and rows. Go and older
  Rust builds ignore the new tables, so the store remains openable by both
  (D1).
- **Rolling back to Go** returns issuance to the Go contract instead of this
  one. The cutover review must record that consequence.
- **Credential loss.** If the credential files are deleted, the next start
  generates new secrets and revokes the affected clients' tokens.

## 8. Alternatives considered

| Alternative | Why not |
| --- | --- |
| Gate issuance on peer address or `Host` | Behind cloudflared every request is loopback; `Host` is caller-controlled |
| Approve button on the authorize page, protected by the bootstrap token typed into it | Puts the admin secret into a page served through a public tunnel; phishable; CSRF surface |
| Disable `/oauth/authorize` permanently | Removes the documented CIMD flow for remote MCP clients (ADR 0008, S-4) |
| Issue provider tokens through the public URL without a client secret | Indistinguishable from any other caller; cannot be made secure |
| Signed, self-contained provider tokens (no store row) | Revocation (A-4) would need a deny list anyway; the store already exists and is cross-read verified |
| Reuse `MCP_AUTH_TOKEN` for providers over the public URL | A-6: the bootstrap token is local-audience only, and providers already avoid sending it to public routes |
| Refresh tokens to soften approvals | Not needed with remembered consent; adds a long-lived credential |
