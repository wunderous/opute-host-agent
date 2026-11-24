## ADDED Requirements

### Requirement: A plan node never starts a plan run of its own
The Rust Host Agent SHALL refuse, before any side effect, a host plan or recipe in which a node's action, compensation or recovery names a tool that starts a plan run: `run_host_plan`, `run_host_local_recipe`, `run_runtime_recipe`, `run_tunnel_recipe`, `opute.provider.install`, `opute.provider.reload` or `opute.provider.teardown` (owner decision D44). A recipe SHALL also refuse one as a node's validation. The refusal for `run_host_plan` SHALL keep the text `node "<id>" cannot recursively run a host plan`. A plan with no such node SHALL be validated and run as before.

#### Scenario: A recipe node runs another recipe
- **WHEN** a client validates or runs a host recipe whose node calls `run_host_local_recipe`
- **THEN** the agent refuses it with `recipe node "<id>" cannot recursively run run_host_local_recipe`, and no run starts

#### Scenario: A plan installs a provider
- **WHEN** a client validates a host plan whose node, compensation or recovery calls `opute.provider.install`
- **THEN** the agent refuses it with `node "<id>" cannot recursively run opute.provider.install`

#### Scenario: A plan uses a provider's tools
- **WHEN** a provider was installed by a typed call before the run, and a node calls one of its tools
- **THEN** the plan validates and runs as before
