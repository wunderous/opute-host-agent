# Bound the serving relay's and the providers' request bodies

Status: **accepted (Host Agent owner, delegated 2026-11-11) as D41.** Goal m17.

## Why

m16's auth review (A2) found that the model-serving relay reads an
inference request's whole body before it forwards it. A holder of the
relay's bearer can make the agent hold a body of any size, which is the
memory path D29 closed on `/mcp`. While measuring A2, m17 found the same
read in the provider executables' own `/mcp` (A5). They read the whole
body and only then apply the SDK's 4 MiB limit, as `/mcp` did before D29.

Failing-first tests on rust-0.4.0-rc1's code: neither
`llmops::tests::oversized_relay_bodies_are_refused_without_reading_them`
nor `providers::tests::oversized_provider_bodies_are_refused_without_reading_them`
got a reply before the oversized body ended.

## What Changes

- **The relay** reads at most 32 MiB (33554432 bytes) of a request body.
  A larger body gets `413 request body exceeds 33554432 bytes` at once,
  and the rest is not read. The bearer, CIDR and path gates run first,
  as before. Responses, which stream tokens, stay unbounded.
- **A provider's `/mcp`** reads at most the SDK's 4 MiB and answers a
  larger body with the SDK's existing `413 request body exceeds 4194304
  bytes`, before the body is held. This is D29 applied to the provider
  executables. A body within the limit is handled as before.

Why 32 MiB for the relay: it carries OpenAI- and Ollama-style inference
requests, which may hold long contexts, embedding batches or base64 images.
The spec bounds none of them. 32 MiB is eight times the MCP bound and holds
a base64 image of about 24 MB. A larger body is not an inference request
any served model takes.

## Options rejected

- **4 MiB, as on `/mcp`.** No evidence says inference requests stay that
  small, and refusing a long-context request would be a new failure.
- **Stream the body upstream without holding it.** The relay sets
  `Content-Length` and holds the host-wide request lock for the forward.
  Streaming would change the upstream framing, and does not bound what a
  slow peer can make the lock wait for.
- **A setting for the bound.** No caller has asked to tune it. A fixed
  bound needs no new configuration surface.

## Impact

`llmops.rs` (`relay_handle`), `providers/mod.rs` (`handle`), and the two
failing-first tests. A product change: the next candidate is rust-0.4.0-rc2.
