## ADDED Requirements

### Requirement: A child process never receives the agent's secrets
Every process the Rust Host Agent or a Rust provider executable starts SHALL receive an environment built from an allowlist (owner decision D40): `PATH`, `HOME`, `USER`, `LOGNAME`, `SHELL`, `TERM`, `TZ`, `TMPDIR`, `LANG`, `LANGUAGE`, `KUBECONFIG`, `SSL_CERT_FILE`, `SSL_CERT_DIR`, `DBUS_SESSION_BUS_ADDRESS`, `WSL_DISTRO_NAME`, `WSL_INTEROP`, `DOCKER_HOST`, `DOCKER_CONFIG`, `CONTAINER_HOST`, `REGISTRY_AUTH_FILE`; `HTTP_PROXY`, `HTTPS_PROXY`, `NO_PROXY` and `ALL_PROXY` in either case; and names starting `LC_`, `XDG_`, `INCUS_` or `OLLAMA_`. A name with a `_`-separated part `TOKEN`, `SECRET`, `PASSWORD`, `PASSWD`, `CREDENTIAL`, `CREDENTIALS`, `AUTH` or `KEY` SHALL never be passed. The user bus variables added for a `--user` systemd command SHALL still be added.

#### Scenario: A running console
- **WHEN** the agent runs with `MCP_AUTH_TOKEN`, `CLOUDFLARE_API_TOKEN` and an `OPUTE_` setting, and a console session is open
- **THEN** the environment of its `incus console` process holds `PATH` and none of those variables

#### Scenario: A recipe step's command
- **WHEN** a recipe step's `run_host_command` workload is running
- **THEN** the `systemd-run` process the agent started for it holds `PATH` and no variable the allowlist excludes
