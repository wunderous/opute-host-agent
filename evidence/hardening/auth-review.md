# Authenticated paths, walked against the spec (m16)

Every route that authenticates or issues a credential, checked against the
contract spec's "Compatible authenticated MCP surface" and "One exact Host
Agent identity", and against `secure-oauth-issuance` (D8). Code references
are to `crates/host-agent/src` at the m16 branch. "Fuzzed" means the
`http_wire` target reaches the path without a credential (17 targets; see
`entry-points.json`).

## Findings

| Id | Where | What | State |
|---|---|---|---|
| F2 | `transport::basic_auth` (`/oauth/token`) | An `Authorization` header with a multi-byte character in its first six bytes was sliced inside that character and panicked. Found by `http_wire` in 30 s. It ran before the authz lock was taken, so only that connection's task died: the peer got no reply and the agent kept serving. | Fixed (bytes are compared); the input is `fuzz/regressions/http_wire/basic-auth-split-char` |
| A1 | `transport::mcp` | After the bearer gate, the whole body was read into memory before the SDK's 4 MiB check. | Fixed as D29 (`bound-mcp-request-body`) |
| A2 | `llmops` serving relay | The relay's incoming bearer is checked in constant time; the request body it forwards to the local model is read whole, without a bound. Only a holder of the relay's own bearer can send one. | Open observation: the relay forwards inference payloads, whose size the spec does not bound. Not changed in m16. |
| A3 | bootstrap bearer, A-6 | The bootstrap bearer is accepted only on a local `Host`, as the design says. `Host` is sent by the client, so this limits the token's audience (it is refused through a public tunnel, whose `Host` is the public name); it does not limit which peer may use it. A peer that holds `MCP_AUTH_TOKEN` and can reach a non-loopback bind can name a local `Host`. | As specified; noted for the owner. Platform mode binds per its documented defaults. |

## Paths

| Path | Spec | Checked | Result |
|---|---|---|---|
| `GET /health` | open liveness endpoint | no credential read | as specified; fuzzed |
| `/mcp` method gate | OPTIONS 204, other non-POST 405 | `transport::mcp` | as specified; fuzzed |
| `/mcp` Origin | DNS-rebinding guard | `origin_allowed`: empty Origin passes; on a local `Host` only loopback origins; otherwise the request's own origin | as specified; fuzzed |
| `/mcp` bearer: bootstrap | `MCP_AUTH_TOKEN`, local `Host` only (A-6) | `authorize`: constant-time compare, then `is_local_host_address(Host)` else 403 | as specified; see A3 |
| `/mcp` bearer: issued token | reject missing or invalid credentials | SHA-256 of the token looked up; revoked or expired → 401; resource ≠ this request's canonical `/mcp` → 403; scope other than `mcp` → 403 | as specified; fuzzed (401 path) |
| `/mcp` body | (D29) | at most 4 MiB read, then 413 | fixed in m16 |
| `/mcp` legacy handshake | default off; bounded method list (Q2) | `is_retired_handshake`, `skip_modern_validation` | unchanged in m16 |
| `/.well-known/*` | open metadata | no credential read | fuzzed |
| `/oauth/authorize` | PKCE, exact redirect, operator approval | S256 only; redirect exact, or RFC 8252 loopback with any port (`redirect_allowed`, `loopback_parts`: `http://` only, no userinfo, host 127.0.0.1, [::1] or localhost); pending requests capped at 20 in total and 3 per client, 600 s TTL; ids are 16 random bytes | as specified; fuzzed |
| operator approval | no HTTP route approves | approval only through `opute-host-agent oauth approve` | as specified |
| `/oauth/authorize/status` | — | returns the code only for an approved request id; redeeming it needs the PKCE verifier | as specified |
| `/oauth/token` client_credentials | backed by a credential | confidential client only; secret compared as constant-time SHA-256 digests; a client with no registered secret cannot authenticate; resource must be one this agent serves | as specified; F2 fixed; fuzzed |
| `/oauth/token` authorization_code | single-use, PKCE | the code is marked used before any other check; client, redirect, resource and verifier must match; 600 s code TTL | as specified |
| `/oauth/token` abuse | rate limit failing grants per client and per address; a valid grant is served even while limited | `Backoff` keyed `client:` and `remote:`; checked only after a grant fails (429 `slow_down`), so a valid secret or code is issued from a limited address | as specified; the fuzzer drove it into backoff within a second, and the wire kept answering |
| `/oauth/revoke` | — | revokes by token hash; knowing a token already grants its use, so revoking it unauthenticated takes nothing from its holder | as specified |
| issued tokens | — | `oha_` + 32 random bytes; stored only as SHA-256; 3600 s TTL | as specified |
| credentials at rest | provisioned secrets | `credentials/<client>.json` 0600 in a 0700 directory, written by temp file and rename | as specified; `credential_file` fuzzed |
| serving relay bearer | — | constant-time compare (`llmops`) | as specified; see A2 |
