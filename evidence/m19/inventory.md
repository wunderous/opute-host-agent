# m19: where each secret the agent holds goes

Static review of `main` (0.3.0, `rust-0.3.0-rc5`'s product code) on 2026-11-17.
Each suspect is measured by `e2e6.secret-sweep` or, where the dev VM cannot
reach it, by a unit test. "Clean" means the review found no path off its
place; the journey still searches for it.

## Secret classes

| Class | Source | Where it should go |
|---|---|---|
| Bearer (`MCP_AUTH_TOKEN`) | the agent's env file | the agent's environment; provider units by `EnvironmentFile` |
| OAuth client secrets (`OPUTE_OAUTH_CLIENTS`, `OPUTE_HOST_OAUTH_CLIENT_SECRET`) | env | the store as SHA-256 only |
| OAuth codes and access tokens | issued | the store as SHA-256 only; the caller |
| LLM relay tokens (`incomingToken`, `upstreamToken`, `relayToken`) | tool input | the relay's 0600 args file; the upstream `Authorization` header |
| LLM K3s proxy relay token and bearer key | tool input | a Kubernetes Secret, by stdin |
| PostgreSQL relay token (D24) | tool input | the relay's 0600 state file as SHA-256 only |
| PostgreSQL credentials | a Kubernetes Secret | `pgpass` by stdin; the consumer Secret by stdin |
| Provider credentials (`CLOUDFLARE_API_TOKEN`, `TAILSCALE_AUTH_KEY`, `TAILSCALE_API_KEY`) | env | in-process HTTP only |
| Public-MCP tunnel token | tool input | a 0600 env file read by the unit |
| Cloudflare tunnel run token | the Cloudflare API | a Kubernetes Secret (kubernetes), a 0600 file (container), the unit (host) |
| Tailscale auth key | env or `TAILSCALE_AUTH_KEY_FILE` | the target, by stdin |
| K3s join token | the server | the joining node, by stdin into `K3S_TOKEN`; sealed join material |
| Cluster-agent bridge token | tool input | the cluster agent's config file |
| Provider install token | tool input (`writeOnly`) | the provider binding |
| Registry auth | `REGISTRY_AUTH_FILE` (a path) | children keep the path only; the agent holds no registry secret |

## Suspects

| Id | Class | Path | Code |
|---|---|---|---|
| S1 | Public-MCP tunnel token | cloudflared's argv: the unit runs `sh -c 'exec cloudflared … --token "$OPUTE_CLOUDFLARED_TUNNEL_TOKEN"'`, so the shell expands the token onto argv | `hostpublic.rs` (`ensure_public_mcp_tunnel`) |
| S2 | Tunnel run token, host placement | the unit's `ExecStart=… tunnel run --token <token>` puts it on cloudflared's argv | `providers/cloudflare/mod.rs` (host connector unit) |
| S3 | Tailscale auth key | `tailscale up --authkey="$auth_key"` puts it on argv in the target | `providers/tailscale/live.rs` (join script) |
| S4 | Bridge token | the cluster agent's config travels base64-encoded inside the install script, which is argv of `bash -lc` (host and `incus exec`) and of `sudo -n bash -lc`; the config file is written with the shell's umask | `clusterops.rs` (`agent_install_script`, `privileged`, `install`) |
| S5 | LLM relay tokens | `ensure_local_llm_relay` declares `upstreamToken` without `writeOnly` and `incomingToken` (required) with no property, so D13's schema-driven redaction keeps both | `llmops.rs` (relay tool schema) |

Clean on review: OAuth (hashes only), PostgreSQL credentials and relay token
(stdin, hash at 0600), K3s join token (stdin, sealed material), Kubernetes
manifests and Secrets (stdin), relay args, LLM config and units (0600),
provider API calls (in-process HTTP).
