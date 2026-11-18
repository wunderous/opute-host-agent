# A secret stays where its tool needs it

Status: **accepted (Host Agent owner, delegated 2026-11-11) as D46.** Goal m19. Applies to 0.3.0 from `rust-0.3.0-rc6` and to 0.4.0 from `rust-0.4.0-rc4`.

## Why

D40 (m17) closed one path a secret took off its place: a child's
environment. A4 was found by chance. Nothing checked the others: a child's
command line (`/proc/PID/cmdline` is readable by every local user), the
modes of files the agent writes, the journal, the store, tool results and
errors, and the unit and env files the agent installs.

## Inventory

Every secret class the agent holds, where it comes from and where it should
go, is in `evidence/m19/inventory.md`. The classes: the bearer, OAuth client
secrets and the codes and tokens issued, the LLM relay tokens, the LLM K3s
proxy credentials, the PostgreSQL relay token (D24) and credentials, the
provider credentials (`CLOUDFLARE_API_TOKEN`, `TAILSCALE_AUTH_KEY`,
`TAILSCALE_API_KEY`), the public-MCP tunnel token, the Cloudflare tunnel run
token, the Tailscale auth key, the K3s join token, the cluster agent's
bridge token, the provider install token and registry auth.

## Measurement

The journey `e2e6.secret-sweep` (gate e2e-6) runs a transient agent of the
release candidate holding a marker for each class it can reach, from a 0600
env file. It runs the OAuth client-credentials grant, an LLM relay, a host
cluster agent install and a public MCP connector, then searches, raw and
base64-encoded at each alignment:

- the argv of every process the agent's tree executes (`strace -f`) and of
  every process on the host (`bpftrace` on `execve`), so the processes
  systemd starts for the agent's units are seen too;
- every process's environment;
- the whole journal;
- every file under `/etc`, `/opt`, `/tmp`, `/var/tmp`, `/home`, `/root`,
  `/run`, `/usr/local` and `/srv`, with its mode and owner, the agent's
  state directory counting as the store;
- every MCP response.

Paths the dev VM cannot reach (a Cloudflare connector placed by the
provider, an HA connector in a guest, a Tailscale join) have unit tests.

On `rust-0.3.0-rc5` (run `m19-before-030`) and `rust-0.4.0-rc3` (run
`m19-before-040`) the sweep found the same five things, and the unit tests
two more paths:

| Finding | Where | Decision |
|---|---|---|
| S1 | the public-MCP connector's unit runs `cloudflared … --token "$OPUTE_CLOUDFLARED_TUNNEL_TOKEN"` through `sh -c`, so the token is on cloudflared's argv | D47 |
| S2 | the Cloudflare provider's host-placed connector unit has the run token in `ExecStart` | D48 |
| S6 | the Cloudflare HA connector in a guest has it in `ExecStart` of a unit written with the shell's umask (0644) | D48 |
| S3 | the Tailscale join runs `tailscale up --authkey="$auth_key"` | D49 |
| S4 | the cluster agent's config, bridge token included, travels base64-encoded inside the install script, which is argv of `bash -lc`, `sudo -n bash -lc` and `incus exec`; sudo logs it to the journal; the file is written 0644 | D50 |
| S5 | `ensure_local_llm_relay.upstreamToken` is declared without `writeOnly`, so the store keeps it (D13) | D51 |

Clean: the bearer and the provider credentials (only the agent's own
process holds them, D40), OAuth (the store keeps SHA-256 only; 0.4.0's
generated client secrets and the provider client's are in 0600 credential
files), the relay's args file (0600), every response, PostgreSQL
credentials and relay token, the K3s join token and Kubernetes Secrets
(all by stdin), and registry auth (a path only).

## What Changes

Three rules, checked by `e2e6.secret-sweep` on every candidate:

- A secret goes only where its tool needs it: a file, stdin or the
  environment of the one process that uses it, never a command line.
- A file the agent writes or installs that holds a secret is 0600 or
  stricter and owned by the agent's user (on a guest, by root).
- The journal, the store and every response carry a secret only redacted
  (D13): a credential input is published `writeOnly`.

Each finding is fixed under its own decision, D47 to D51. No tool, endpoint,
mode or setting is added; tool results do not change.

## Options rejected

- **Fix only what the sweep found, without a rule.** The next tool that
  writes a unit or runs a script would bring the same paths back; the
  journey would have nothing to judge it by.
- **A denylist of command lines in a source test.** A secret reaches argv
  through shell expansion, `printf` and base64, none of which a source
  pattern sees. The sweep measures the running candidate.
- **Hiding argv with `hidepid`.** It is a host mount option the agent does
  not own, and it leaves the journal and file modes as they are.

## Impact

`evidence/m19`, the journey and its guest tool
(`evidence/e2e/guest-tools/secretsweep.py`), and the harness's `excludes`
assertion. The fixes are in D47 to D51.
