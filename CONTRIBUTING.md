# Contributing

Thanks for helping. Bug reports, documentation fixes and pull requests are all
welcome.

## Before you start

- **Bugs:** open an issue with the agent version (`npx @opute/host-agent
  status` or the release tag), the host OS, the MCP client, and the smallest
  request that shows the problem. Remove tokens, hostnames and other private
  values first.
- **New tools or behavior changes:** open an issue first. A change a client can
  observe, such as a tool, schema, default or permission, starts as an
  [OpenSpec](https://openspec.dev) change under `openspec/changes/`, so the
  contract can be reviewed before the code. See
  [How this project is built](docs/engineering.md).
- **Security issues:** don't open a public issue. Follow
  [SECURITY.md](SECURITY.md).

## Pull requests

```bash
make rust-check     # rustfmt, clippy -D warnings, unit tests
make rust-suites    # contract suites
make spec-validate  # when you touch openspec/ (needs Node.js 20.19+)
```

- Keep changes small and focused. Explain what a client will see differently
  and how you tested it.
- Add or update tests next to the code you change. A schema or wire change
  needs a contract test, not only a unit test.
- Never put credentials, tunnel tokens, hostnames or real infrastructure state
  in fixtures, logs or evidence.
- The agent never infers an operation from prose. Tools take explicit, typed
  arguments, and new tools follow the same rule.

By contributing you agree that your contribution is licensed under the
[MIT License](LICENSE).
