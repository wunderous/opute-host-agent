# A child process gets only the environment its tool needs

Status: **accepted (Host Agent owner, delegated 2026-11-11) as D40.** Goal m17.

## Why

m15 found (A4) that the agent starts every child process with its whole
environment. The agent's environment holds its bearer (`MCP_AUTH_TOKEN`),
the OAuth clients (`OPUTE_OAUTH_CLIENTS`) and, when an operator set them,
provider credentials (`CLOUDFLARE_API_TOKEN`, `TAILSCALE_AUTH_KEY`,
`TAILSCALE_API_KEY`). Every tool the agent runs got them: incus, kubectl,
git, curl, the container clients, ollama, and the `systemd-run` client
that waits on a task's workload. Any of these can log them, pass them on
to its own children, or read them from `/proc` while it runs. A caller's
own command under `run_host_command` runs in a systemd unit, which starts
from the manager's environment, so it never saw them. That is the one
path the agent relied on, not a guarantee for the rest.

The failing-first test `hostobs::tests::a_child_never_sees_the_agents_secrets`
showed `CLOUDFLARE_API_TOKEN` in the output of `env` run through
`run_command` on rust-0.4.0-rc1's code.

## Inventory

Eight sites start a process. Each one passes the agent's environment
snapshot, except the two marked otherwise.

| Site | Children | What they need |
|---|---|---|
| `hostobs::run_command_raw` (every typed tool's command runner) | incus, kubectl, systemctl, systemd-run, sudo, container tools, ollama, git, curl, `run_host_command` | `PATH`, `HOME`, locale, proxies, `KUBECONFIG`, Incus and container client settings, the user bus for `--user` (added by the runner) |
| `hostexec::run_piped` (recipes, privileged packages, workload units) | systemd-run, sudo, apt, the recipe's commands | as above |
| `console::Consoles::start` | `setsid --ctty incus console` | `PATH`, `HOME`, `TERM`, Incus client settings |
| `servingops::Serving::run_ok` | systemctl | the user bus |
| `hostvirt::dpkg_architecture` | dpkg | `PATH` |
| `llmops::output` | ollama, systemctl, nvidia-smi | `PATH`, `OLLAMA_*` |
| `identity` (WSL machine id) | powershell.exe, reg.exe | `PATH`, `WSL_INTEROP`, `WSL_DISTRO_NAME`. Today it inherits the process environment |
| `providers::k3s::spawn_and_wait` (the K3s provider executable) | incus, sudo | `PATH`, `HOME`, Incus client settings. Today it inherits the provider's environment |

No child reads an `OPUTE_*` setting. Services the agent
installs (`opute-ollama`, llama-server, cloudflared) get their environment
from their unit files, never from the agent.

## What Changes

`Env::for_child` builds the environment of every child from the agent's
environment:

- Kept by exact name: `PATH`, `HOME`, `USER`, `LOGNAME`, `SHELL`, `TERM`,
  `TZ`, `TMPDIR`, `LANG`, `LANGUAGE`, `KUBECONFIG`, `SSL_CERT_FILE`,
  `SSL_CERT_DIR`, `DBUS_SESSION_BUS_ADDRESS`, `WSL_DISTRO_NAME`,
  `WSL_INTEROP`, `DOCKER_HOST`, `DOCKER_CONFIG`, `CONTAINER_HOST`,
  `REGISTRY_AUTH_FILE`.
- Kept by proxy name, in either case: `HTTP_PROXY`, `HTTPS_PROXY`,
  `NO_PROXY`, `ALL_PROXY`.
- Kept by prefix: `LC_`, `XDG_`, `INCUS_`, `OLLAMA_`.
- Never kept, even when a rule above matches: a name with a `_`-separated
  part `TOKEN`, `SECRET`, `PASSWORD`, `PASSWD`, `CREDENTIAL`,
  `CREDENTIALS`, `AUTH` or `KEY`.

Every site clears the inherited environment and passes that set. The user
bus is still added for `systemctl --user` and `systemd-run --user`. A
source test fails if a new site starts a process without it.

Go v0.2.4 passed the whole environment to every child. A caller sees no
change in a tool's result. The tools themselves no longer get a variable
outside the allowlist; none of them reads one.

## Options rejected

- **A denylist of known secret names.** A secret added later, or one an
  operator names freely, would leak. The allowlist fails closed.
- **A list per site.** Most of the eight sites run the same client tools.
  One list is easier to audit, and the source test holds every site to it.
- **Keeping Go's behaviour.** Every third-party tool would keep receiving
  the agent's bearer and the providers' credentials.

## Impact

`config.rs` (`Env::for_child`), the eight sites, and the tests
`config::tests::every_spawn_site_passes_the_child_environment` and
`hostobs::tests::a_child_never_sees_the_agents_secrets`, both failing first.
A product change: the next candidate is rust-0.4.0-rc2.
