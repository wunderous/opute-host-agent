# m19: where each secret the agent holds goes

Static review of `main` (0.3.0, `rust-0.3.0-rc5`'s product code) on 2026-11-17, then measured.
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
| S5 | LLM relay tokens | `ensure_local_llm_relay` declares `upstreamToken` without `writeOnly`, so D13's schema-driven redaction keeps it (`incomingToken` is not declared, so the fail-closed rule redacts it) | `catalog/source.json` (relay tool schema) |
| S6 | Tunnel run token, HA connector | the guest unit's `ExecStart=… run --token $tunnel_token`, written with the shell's umask (0644) | `providers/cloudflare/mesh.rs` (`HA_CONNECTOR_SCRIPT`) |

S6 was found while fixing S2 (a grep for `--token`), not by the first review.

## Measured

`e2e6.secret-sweep` on `rust-0.3.0-rc5` (run `m19-before-030`) and
`rust-0.4.0-rc3` (run `m19-before-040`) found, on both:

| Path | Markers |
|---|---|
| argv | `tunnelToken` (cloudflared, S1); `bridgeToken` (bash, sudo, printf, S4) |
| journal | `bridgeToken` (sudo's log of its command line, S4) |
| loose files | `/etc/opute/cluster-agent.json` 0644 (S4) |
| store | `upstreamToken` (`state.db-wal`, S5) |
| other environments | none |
| responses | none |

Files holding a secret where they should: the public-MCP token file (0600),
the relay's args file (0600), the provider client's and 0.4.0's generated
client's credential files (0600). The unit tests for S1 to S6
(`before.log`, `before-040.log`, `before-s6.log`) failed on rc5's and
rc3's code.

## Decisions

D46 states the rules; D47 (S1), D48 (S2, S6), D49 (S3), D50 (S4) and
D51 (S5) fix the findings. See `openspec/changes/secret-placement`.

## Observation, not a finding

`providers/tailscale/live.rs` defaults the auth key file to a developer's
home path (`DEFAULT_AUTH_KEY_FILE`). It holds no secret and exposes none;
it is out of this goal's scope.

Clean on review: OAuth (hashes only), PostgreSQL credentials and relay token
(stdin, hash at 0600), K3s join token (stdin, sealed material), Kubernetes
manifests and Secrets (stdin), relay args, LLM config and units (0600),
provider API calls (in-process HTTP).
