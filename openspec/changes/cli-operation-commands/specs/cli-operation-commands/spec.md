## ADDED Requirements

### Requirement: CLI operation commands call the published tools

The `public-mcp`, `recipe` and `provider` commands SHALL each perform the
operation by calling the agent's published tool in-process, through the same
admission, mutation authority and plan execution as any MCP client. Each SHALL
print the tool's structured result as indented JSON on stdout. A command that
starts a run SHALL wait for it to end, and SHALL exit with an error naming
the status unless the run completed. A command SHALL NOT report success for an
operation it did not perform. The operator holds the bootstrap bearer's
authority: a tool whose effect is not `read` SHALL run only when the command
is given `--allow-mutations`.

#### Scenario: Recipe apply waits for the run
- **WHEN** `opute-host-agent recipe apply --allow-mutations --kind runtime --source FILE` is run
- **THEN** it calls `run_runtime_recipe`, waits for the run, and prints the completed run's structured result, or exits non-zero with the run's status and error

#### Scenario: A mutation without the grant
- **WHEN** `opute-host-agent recipe apply --source FILE` is run without `--allow-mutations`
- **THEN** it exits non-zero with the `mutation_refused` text, prints no JSON, and the recipe's plan does not run

#### Scenario: Provider status
- **WHEN** `opute-host-agent provider status --provider ID` is run
- **THEN** it prints `opute.provider.status`'s structured result for that provider

#### Scenario: A refused operation
- **WHEN** a command's tool call returns an error result
- **THEN** the command exits non-zero, naming the operation and the tool's error text, and prints no JSON

#### Scenario: A running agent holds the state
- **WHEN** a command is run while an agent holds the same instance state
- **THEN** it exits non-zero with the instance-lock refusal and calls no tool
