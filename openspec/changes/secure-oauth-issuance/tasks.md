## 1. Harness: declared divergences and contract suites

- [ ] 1.1 `compare.divergences` (path, decision, reason). Dropped from
  Go-vs-Rust only. The verifier checks that each cited decision is approved in
  `parity-manifest.json` and that each declared path still differs (stale
  check).
- [ ] 1.2 Contract scenarios (`"contract": true`) run against one
  implementation, with `expect` assertions on status, headers, body fields,
  store rows and file modes. `parity contract` writes a hashed evidence
  bundle.
- [ ] 1.3 A Rust canary mechanism: patched Rust builds must turn the named
  contract scenario red.

## 2. Rust: credentialed client_credentials

- [x] 2.1 `client_credentials` requires a stored secret hash and a matching
  secret (`client_secret_basic` or `client_secret_post`). Public clients get
  `unauthorized_client`.
- [x] 2.2 Provisioned `opute-mcp-host` (when no operator secret) and
  `host-agent-provider` secrets in `<state>/credentials/*.json` (0600 in
  0700, atomic write), hashes only in the store. `oauth rotate-secret`
  revokes the client's tokens.
- [x] 2.3 Audience restriction (`invalid_target`): the request's canonical
  URI or an active public binding endpoint. Bindings arrive with M8; until
  then only the canonical URI.
- [x] 2.4 Migration `oauth-issuance.v1`: revoke pre-existing tokens, make
  `host-agent-bootstrap` public, record the migration. Idempotent.
- [x] 2.5 Failed-grant backoff, and audit lines (with a scan test for
  secrets and tokens).

## 3. Rust: operator-approved authorization_code

- [x] 3.1 The `pending_authorizations` and `consents` tables, user codes and
  limits.
- [x] 3.2 `/oauth/authorize`: validation (client, exact redirect with the
  RFC 8252 loopback rule, PKCE S256, resource), the pending request, and the
  approval page (no approve control; `no-store`, `DENY`,
  `frame-ancestors 'none'`, `no-referrer`). `/oauth/authorize/status`
  answers with a redirect (approved) or `access_denied`.
- [x] 3.3 `/oauth/token` with `grant_type=authorization_code`: single-use
  code, client, redirect and PKCE checks, resource bound to the request.
- [x] 3.4 CLI `opute-host-agent oauth pending|approve|deny|consents|revoke-client|rotate-secret`.
- [x] 3.5 Remembered consent (default 30 days, `0` disables), with revocation.
- [ ] 3.6 Client ID metadata documents fetched over HTTPS from the vetted
  address, with no redirects, size and time limits, and SSRF rules.

## 4. Later milestones

- [ ] 4.1 M3: the `list_oauth_authorizations` / `approve_oauth_authorization`
  admin tools (local bootstrap bearer only).
- [ ] 4.2 M7: the provider environment gains `OPUTE_HOST_AGENT_CLIENT_ID` and
  `OPUTE_HOST_AGENT_CLIENT_SECRET_FILE`, plus the provider-side
  `client_secret_basic` change for public-URL activation evidence.
- [ ] 4.3 M8: in-process probe tokens for the agent's own endpoints, and
  public bindings as allowed audiences.

## 5. Evidence

- [ ] 5.1 An `oauth-issuance` contract suite covering every flow and negative
  case in the spec, with Rust canaries caught.
- [ ] 5.2 The M2 corpus stays Go-vs-Rust clean outside declared D8 paths,
  and the store cross-read still passes.
- [ ] 5.3 The go-sdk real client completes an approved authorization against
  Rust.
