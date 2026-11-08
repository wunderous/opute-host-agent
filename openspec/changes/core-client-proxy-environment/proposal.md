# Honour the proxy environment in the core HTTP clients

Status: decided (owner, 2026-11-07, D28). Owner question Q7
([rust-spec-authority/questions.md](../archive/2026-11-07-rust-spec-authority/questions.md)):
each ported defect is fixed separately.
Implemented after the decision; see tasks.md.

## Why

M9 recorded that eight core HTTP clients built on ureq ignore
`HTTP_PROXY`, `HTTPS_PROXY` and `NO_PROXY`, where Go v0.2.4's clients use
`http.DefaultTransport`, which honours them (`http.ProxyFromEnvironment`;
loopback and `localhost` are never proxied). The provider clients already
honour them (`providers::proxy_for`).

Checked against the pinned Go tree, client by client:

| Rust | Go | Go transport | Proxy |
| --- | --- | --- | --- |
| `hostread` probe | `domain/host/http_probe.go` | default; IPv4 fallback has its own `Transport` | yes; fallback no |
| `mcpprobe` | `mcpprobe/probe.go` | default | yes |
| `hostservice` bridge health | `domain/host/types.go` (`http.DefaultClient`) | default | yes |
| `hostartifact` | `domain/host/artifact.go` | default | yes |
| `recipe/source` | `recipe/recipe.go` | default | yes |
| `llmops` (two clients) | `domain/llm/*.go` | default | yes |
| `servingops` readiness | `domain/serving/assignment.go` | default | loopback only, never proxied |
| `oauth` client metadata | `authz/service.go` | `Transport{Proxy: nil}` | no |

So six clients had the defect. `servingops` probes only `127.0.0.1`, and
`oauth` is right not to proxy (its pinned, vetted dial is the SSRF guard).

## What Changes

The six clients pick the proxy for their URL as Go's default transport
does, through one helper (`providers::env_proxy`). ureq chooses one proxy
per agent, so a redirect to another host keeps the first host's choice;
the recipe source's hops all stay on one host.

## Impact

- `crates/host-agent/src/providers/support.rs` (`env_proxy`), `hostread.rs`,
  `mcpprobe.rs`, `hostservice.rs`, `hostartifact.rs`, `recipe/source.rs`,
  `llmops.rs`.
