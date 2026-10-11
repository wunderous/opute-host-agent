# Spec Delta

## Purpose

Require every Rust Host Agent capability, built-in or provided by an external
provider, to have a Service Definition, a Service Provider and a Consumer,
verified by evidence and without changing any behaviour a client observes.

## ADDED Requirements

### Requirement: Every capability has three verified roles
Every capability the Rust Host Agent publishes or dispatches SHALL have a
Service Definition, a Service Provider and at least one Consumer. The Service
Definition SHALL be a versioned, provider-neutral entry in the one descriptor
source. The Service Provider SHALL be an implementation registered for that
definition in the provider-neutral service context, either built in or
supplied by an external provider generation. The Consumer SHALL be public
catalog dispatch, a plan or recipe node, or another service that declares
the capability as a dependency. A seam verifier SHALL report every
capability's roles, and SHALL fail when a capability lacks a role and is not
listed as a seam gap.

#### Scenario: Built-in host tool
- **WHEN** the seam verifier inspects a published built-in tool such as `get_host_info`
- **THEN** it reports the tool's Service Definition, a registered built-in Service Provider and public dispatch as a Consumer

#### Scenario: Definition without a provider
- **WHEN** the descriptor source defines a capability that no provider registers
- **THEN** the capability is neither published nor dispatched, and the seam verifier fails

#### Scenario: Provider without a definition
- **WHEN** a built-in or external provider registers a service that has no Service Definition
- **THEN** the registration is refused, and the catalog and its revision are unchanged

#### Scenario: Dispatch outside the service context
- **WHEN** a capability is reached by a tool-name match that does not resolve a registered service
- **THEN** the seam verifier fails

### Requirement: Built-in and external providers share one lifecycle
Built-in providers SHALL be registered in the same service context and
reached through the same admission pipeline as external provider
generations. They SHALL be mounted in dependency order before the MCP
listener accepts requests, and disposed in reverse order at shutdown. A
built-in provider that fails to mount SHALL abort startup with the same
outcome as any other startup failure.

#### Scenario: A built-in provider fails to mount
- **WHEN** a built-in provider fails to mount during startup
- **THEN** the providers already mounted are disposed in reverse order and the agent exposes no usable MCP listener

#### Scenario: Shutdown
- **WHEN** the agent shuts down with built-in and external providers mounted
- **THEN** every provider is disposed exactly once, in reverse mount order

### Requirement: Built-in services stay outside provider families
Built-in services SHALL NOT be candidates for provider dependency injection.
They SHALL NOT take part in provider per-family publication or displacement,
so no built-in tool can be displaced by a provider generation and no provider
dependency can become ambiguous because of a built-in service. A change to
this isolation SHALL require its own OpenSpec change.

#### Scenario: A provider declares a dependency on a capability family
- **WHEN** a provider generation declares a dependency on a capability family that a built-in service also implements
- **THEN** dependency resolution considers only provider services, with the same result and error as the pinned Go agent

#### Scenario: A provider publishes into a family
- **WHEN** a provider generation publishes operations in a capability family
- **THEN** no built-in tool is displaced, and the catalog changes exactly as it does in the pinned Go agent

### Requirement: The seam changes no observable contract
Registering a capability on the seam SHALL NOT change any published name,
descriptor, effect, approval, idempotency, resource binding, error, catalog
revision, admission order, task behaviour, durable record or default. After
each change, the parity captures of the milestone that delivered the
capability SHALL be rerun against the pinned Go agent and SHALL pass with no
new normalization.

#### Scenario: Read-only tools move onto built-in providers
- **WHEN** the M3 read-only host tools are revised onto built-in providers
- **THEN** the M3 catalog matrix, including the catalog revision, and the M3 read-only scenarios match the pinned Go agent as before

#### Scenario: A capture fails after a move
- **WHEN** a rerun parity capture differs from the pinned Go agent after a capability moves onto the seam
- **THEN** the move is not accepted, and the failing capture is kept and not relabeled

### Requirement: The rule applies from decision D14, with delivered capabilities revised before M8
The rule SHALL apply to every capability implemented or changed after
2026-10-11. Each capability delivered in M1–M7 that does not yet meet it
SHALL be listed by name as a seam gap in the seam verifier's report, and
SHALL NOT be reported as compliant. M8 SHALL NOT start until the seam
verifier reports no gap.

#### Scenario: An unrevised M3 tool
- **WHEN** the seam verifier runs before the M3 tools are revised
- **THEN** it lists each of them as a named seam gap and does not report them as compliant

#### Scenario: Gate before M8
- **WHEN** the seam verifier still reports any gap
- **THEN** the M8 entry gate is closed

#### Scenario: A new M8 domain operation
- **WHEN** an M8 sub-milestone ports a domain operation
- **THEN** the operation is a built-in provider of a three-role service and the seam verifier reports no gap for that domain
