# M2 evidence: HTTP transport, authentication, MCP wire

This file records M2 from
[milestones.md](../../openspec/changes/reimplement-host-agent-in-rust/milestones.md)
(task 2.2). The evidence is in [`evidence/current/`](../current/).
Reproduce it with `make parity-m2` (Rust 1.94.1, Go 1.24.7 with the go1.25.4
toolchain the reference binary was built with, Python 3.11).

## Gate results

| Check | Result |
| --- | --- |
| `make rust-check` | fmt, clippy `-D warnings`, 61 unit tests: pass |
| `make parity-test` | 63 harness tests (comparator, divergences, verifier, contracts, corpus freshness): pass |
| Go vs Go, ×20 | 44/44 scenarios clean on all 20 iterations |
| Canaries | 8/8 caught, each by its named scenario (plus declared co-failures) |
| **Go vs Rust, ×5** | **37/37 CLI, config, lifecycle, HTTP, MCP wire and auth items pass** on all 5 iterations, outside the declared D8 divergences (38/44 overall: `state.schema-after-start` also passes). The 6 failing items are catalog (4), `get_host_info` (M3) and admission (M4). |
| Go oracle tests against Rust | `standalone-startup` (M1) and `real-client-modern` (M2, the official go-sdk client) pass against Go and Rust and fail against the `/bin/true` control |
| D8 contract suite and Rust canaries | 24/24 contract scenarios; 15/15 canaries caught ([details](../oauth-issuance/README.md)) |
| `make parity-verify-m1`, `make parity-verify-m2` | **PASS** |
| `make parity-verify-cutover` | **FAIL, by design** (later-milestone items) |

## The wire corpus

`tools/parity/parity/corpus.py` generates `scenarios/wire.json`
(2654 requests; the harness tests fail if the committed file is stale).
Each family varies one dimension exhaustively:

| Scenario | What varies |
| --- | --- |
| `wire.routing` | 22 paths × 7 HTTP methods, plus raw framing cases (no Host, HTTP/1.0, absolute URI, duplicate Host, bad version, `OPTIONS *`, lowercase and unknown methods) |
| `wire.transport` | 10 Content-Type × 10 Accept values, for an agent-handled and an SDK-handled method, plus repeated Accept lines |
| `wire.envelope` | 45 malformed or unusual JSON-RPC bodies (batches, BOM, duplicate keys, escaped method, 2000-deep nesting, every id type) and body sizes around the SDK's 4 MiB cap |
| `wire.methods.legacy-{off,on}` | every inventoried MCP method (27) × every header and `_meta` variant (24 to 29), with the ADR 0011 flag off and on, so the whole method list is exercised in both flag states |
| `wire.framing` | raw HTTP/1.x: chunked bodies (extensions, trailers, bad sizes), Content-Length conflicts, Transfer-Encoding variants, Expect, obs-fold and invalid headers, request-line errors, pipelining, HTTP/1.0 keep-alive, 431, large redirects (chunked responses) |
| `wire.sdk-edges.legacy-{off,on}` | the SDK's typed parameter decoding (segmentio error texts), `_meta` client info and capabilities, pagination cursors, legacy `initialize` versions |
| `wire.auth.{standalone,platform}` | Authorization × Host against seeded token rows (valid, wrong resource, wrong scope, expired, revoked, public hosts, forwarded https), revocation and reuse, metadata documents |
| `wire.origin.*` | Origin × Host, including an interface address of the machine, which is the one case where the SDK's DNS-rebinding guard fires, with and without `OPUTE_MCP_DISABLE_LOCALHOST_PROTECTION` |
| `authz.store-cross-read` | each side's `authz.sqlite` is served and revoked by the *other* binary, then read back: Go→Rust→Go equals Rust→Go→Rust |

Raw responses are compared with every header except `Date`, in order, so
framing (Content-Length, Transfer-Encoding, Connection) is part of the
comparison, not only status and body.

## What the Rust agent does at M2

| Module | Scope |
| --- | --- |
| `http1` | A port of the Go `net/http` server behaviour the contract depends on: request-line, header and Host validation with Go's error texts (400, 431, 501, 505), body framing (Content-Length, chunked, Expect: 100-continue, 417), the go1.22+ `ServeMux` (path cleaning redirects, segment-wise matching), and `chunkWriter.writeHeader` response encoding (header order, Content-Length vs chunked, keep-alive and close) |
| `gojson` | The envelope decoder with `encoding/json` semantics: case-insensitive fields, last duplicate wins, 10000-deep nesting, invalid UTF-8 accepted, BOM and trailing data rejected |
| `transport` | `/health`, `/mcp` (Origin, bearer authorization, envelope, retired handshake, the ADR 0011 bypass, modern header and `_meta` validation, `server/discover`, `tasks/*`, `resources/*`), PRM and AS metadata, revocation |
| `mcpsdk` | The go-sdk v1.7.0 stateless JSON handler: DNS-rebinding guard, version, Content-Type and Accept checks, 4 MiB cap, JSON-RPC decoding, era gating, segmentio-typed params, cursors, and status mapping |
| `hostobs` | The `/health` observer: Incus VM capacity through a Go-equivalent command runner, and WSL capability detection |

## What validation caught (and what changed)

1. **Server framing.** hyper and other servers answer malformed requests
   themselves (for example `HTTP/9.9` with 400 instead of Go's 505), so the
   HTTP/1 layer is a port of `net/http` rather than a library.
2. **Response header order and bytes.** Go writes handler headers sorted,
   then its own (`Date`, `Content-Length`, `Content-Type`, `Connection`,
   `Transfer-Encoding`); the SDK writes JSON without a trailing newline.
   Both show up once raw responses are compared header by header.
3. **Two JSON decoders.** The agent decodes the envelope with
   `encoding/json`, the SDK with segmentio/encoding; they disagree on ids
   (`1.5` vs `1`, `true` accepted vs rejected), and their error texts differ.
4. **`method not found` rewriting.** jsonrpc2 rewrites every -32601 handler
   error to `method not found: "<method>"`, which is why `ping` under
   2026-07-28 does not read "not supported in the new protocol".
5. **Cursor bytes.** A hand-derived gob prefix was one byte short; the
   expected bytes now come from Go's encoder, and a valid cursor is a corpus
   case.

## Scope changes and open items

- **OAuth token issuance (decision D8).** At the M2 merge, issuance answered
  501. The owner then approved a Rust-only divergence: issuance follows the
  `secure-oauth-issuance` change, verified by its own contract suite rather
  than Go parity. See [../oauth-issuance/README.md](../oauth-issuance/README.md).
  The corpus does not exercise issuance.
- **MCPGODEBUG (finding F-7, decision D9).** The Go agent inherits the SDK's
  compatibility switches from its environment. Rust implements the default
  behaviour and does not reproduce them, pending the owner's confirmation.
- **Moved to M4:** the packaged Go tests that assert catalog contents and a
  task round trip (`TestPackagedShapeStandaloneHTTPContract`,
  `TestStandaloneHTTPIsolationAndShutdown`), `tasks/get` inline results, and
  the input-required round trip.
- **Moved to M3:** third-party real clients (Codex and others), which need
  the catalog. M2 uses the official go-sdk client.
- **Not yet ported:** `subscriptions/listen` with notifications, a
  long-lived event stream (Rust answers method not found; the corpus covers
  only its parameter validation). It lands with notifications in M4.
