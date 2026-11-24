# Security Policy

## Supported versions

Security fixes go into the latest release. The current release is listed on
the [releases page](https://github.com/wunderous/opute-host-agent/releases).

## Reporting a vulnerability

Please don't open a public issue for a suspected vulnerability. Report it
privately through this repository's
[security advisories](https://github.com/wunderous/opute-host-agent/security/advisories/new)
page. Include:

- the affected version;
- the operating system and the MCP client;
- any provider configuration;
- reproduction steps;
- the impact.

Remove credentials, tunnel tokens, hostnames and other sensitive values first.

We'll acknowledge a report within five business days, then coordinate the fix,
disclosure and credit with you.

## Scope

The agent runs infrastructure commands on the host where it runs. Treat these
as security-sensitive inputs:

- MCP client prompts and tool arguments;
- OAuth grants;
- environment variables and state files;
- provider plugins;
- release artifacts.

Ways to get past the mutation gate, the OAuth scopes, the redaction of secrets
or the loopback-only default are in scope.
