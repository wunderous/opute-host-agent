## ADDED Requirements

### Requirement: A secret never travels on a command line
A secret the Rust Host Agent or a Rust provider executable holds or passes on SHALL reach the process that uses it through a file, its standard input or its environment, never through the arguments of any process, including processes systemd starts for a unit the agent installs (owner decision D46).

#### Scenario: An agent's tools run with planted secrets
- **WHEN** an agent holding its bearer, an OAuth client secret and three provider credentials issues an OAuth token, starts an LLM relay, installs a host cluster agent and a public MCP connector
- **THEN** no process on the host carries any of them, or the tool inputs' tokens, raw or base64-encoded, in its arguments

### Requirement: A file holding a secret is private to the agent's user
A file the Rust Host Agent or a Rust provider executable writes or installs that holds a secret SHALL have mode 0600 or stricter and be owned by the agent's user, or by root in a guest (owner decision D46).

#### Scenario: Files after the agent's tools ran
- **WHEN** the agent's tools have written their files
- **THEN** every file under `/etc`, `/opt`, `/tmp`, `/var/tmp`, `/home`, `/root`, `/run`, `/usr/local` and `/srv` that holds a secret, raw or base64-encoded, has mode 0600 or stricter and the agent's user as owner

### Requirement: Logs, the store and responses carry a secret only redacted
The journal, the agent's state store and every MCP response SHALL hold no secret; an input that is a credential SHALL be published `writeOnly`, so the D13 storage projection redacts it (owner decision D46).

#### Scenario: The journal, the store and the responses after the agent's tools ran
- **WHEN** the agent's tools have run with planted secrets
- **THEN** the journal, every file in the agent's state directory other than its 0600 credential files, and every MCP response hold none of them

#### Scenario: Every declared credential input is writeOnly
- **WHEN** an input property in the served catalog is a string whose name ends in token, secret, password, passwd, credential, authkey, apikey, bearerkey or privatekey
- **THEN** it is published `writeOnly`
