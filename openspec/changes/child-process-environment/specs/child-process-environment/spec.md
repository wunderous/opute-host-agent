## ADDED Requirements

### Requirement: A child process never receives the agent's secrets
Every process the Rust Host Agent or a Rust provider executable starts SHALL receive an environment built from an allowlist (owner decision D40): `PATH`, `HOME`, `USER`, `LOGNAME`, `SHELL`, `TERM`, `TZ`, `TMPDIR`, `LANG`, `LANGUAGE`, `KUBECONFIG`, `SSL_CERT_FILE`, `SSL_CERT_DIR`, `DBUS_SESSION_BUS_ADDRESS`, `WSL_DISTRO_NAME`, `WSL_INTEROP`, `DOCKER_HOST`, `DOCKER_CONFIG`, `CONTAINER_HOST`, `REGISTRY_AUTH_FILE`; `HTTP_PROXY`, `HTTPS_PROXY`, `NO_PROXY` and `ALL_PROXY` in either case; and names starting `LC_`, `XDG_`, `INCUS_` or `OLLAMA_`. A name with a `_`-separated part `TOKEN`, `SECRET`, `PASSWORD`, `PASSWD`, `CREDENTIAL`, `CREDENTIALS`, `AUTH` or `KEY` SHALL never be passed. The user bus variables added for a `--user` systemd command SHALL still be added.

#### Scenario: A command run through run_host_command
- **WHEN** the agent runs with `MCP_AUTH_TOKEN`, `OPUTE_OAUTH_CLIENTS` and `CLOUDFLARE_API_TOKEN` set, and a caller runs `env` through `run_host_command`
- **THEN** the result lists `PATH`, `HOME` and the proxy settings, and none of those three variables or any `OPUTE_` setting

#### Scenario: A running console
- **WHEN** a console session is open
- **THEN** the environment of its `incus console` process holds no variable the allowlist excludes
