# Spec Delta

## Purpose

Require every Rust Host Agent capability, built-in or provided by an external
provider, to follow one architecture with a Service Definition, a Service
Provider and a Consumer, verified by evidence and without changing any
behaviour a client observes.

## ADDED Requirements

### Requirement: Every capability has three verified roles
Every capability the Rust Host Agent implements SHALL have a Service
Definition, a Service Provider and at least one Consumer. The Service
Definition SHALL use the provider manifest model and SHALL come from the one
descriptor source or, for an external provider, its validated manifest. The
Service Provider SHALL be mounted in the one service context. The Consumer
SHALL be public catalog dispatch, a plan or recipe node, or another service
that declares the capability as a dependency. A seam verifier SHALL report
every capability's roles and SHALL fail when an implemented capability is
reached outside the service context and is not listed as a seam gap.

#### Scenario: Built-in host tool
- **WHEN** the seam verifier inspects a published built-in tool such as `get_host_info`
- **THEN** it reports the tool's Service Definition, the built-in provider's mounted service that offers it, and public dispatch as a Consumer

#### Scenario: Definition without a provider
- **WHEN** the descriptor source defines a capability that no mounted service offers
- **THEN** the capability stays published exactly where the pinned Go catalog publishes it, every call fails closed with the typed `not_implemented` capability error, and the seam verifier reports it as unported, never as compliant

#### Scenario: Provider without a definition
- **WHEN** a built-in or external provider offers an operation that has no Service Definition
- **THEN** the mount is refused, what was already mounted is disposed, and the catalog and its revision are unchanged

#### Scenario: Dispatch outside the service context
- **WHEN** an implementation is selected by tool name instead of through the service context
- **THEN** the seam verifier fails

### Requirement: Built-in and external capabilities follow one architecture
Built-in and external capabilities SHALL share the Service Definition model,
registration in the one service context, dependency and family rules,
resolution, the invocation interface and the admission pipeline. They SHALL
differ only in distribution, trust, and the Go-observable per-capability
contracts that parity preserves. Distribution: the built-in provider is
compiled in, is mounted by the composition root before the listener accepts
requests, has no install, recipe, durable generation record or provider
status entry, and is disposed by the composition root after accepted work
drains. Trust: it runs in process and carries no delegation token. Each
preserved Go contract SHALL be declared in the design with its owner.

#### Scenario: A built-in mount fails
- **WHEN** the built-in provider fails to mount during startup
- **THEN** the services already mounted are disposed in reverse order and the agent exposes no usable MCP listener

#### Scenario: Shutdown with work in flight
- **WHEN** the agent shuts down while an accepted plan still dispatches built-in tools
- **THEN** provider generations are disposed when serving stops, the built-in services stay available until accepted work drains, and then they are disposed exactly once in reverse mount order

### Requirement: Provider families cannot reach built-in families
Built-in capability families and the built-in provider identity SHALL use
identifiers that a provider manifest cannot express. The family, dependency
and displacement rules SHALL be the same for every service, so no provider
can depend on, publish into or displace a built-in family, and provider
behaviour SHALL match the pinned Go agent.

#### Scenario: A provider depends on a capability family
- **WHEN** a provider generation declares a dependency on a capability family that only built-in services implement
- **THEN** activation fails with the same result and error as the pinned Go agent

#### Scenario: A provider publishes into a family
- **WHEN** a provider generation publishes operations in a capability family
- **THEN** no built-in tool is displaced, and the catalog changes exactly as it does in the pinned Go agent

### Requirement: The seam changes no observable contract
Mounting a capability on the seam SHALL NOT change any published name,
descriptor, effect, approval, idempotency, resource binding, error, catalog
revision, admission order, task behaviour, durable record or default. After
each change, the parity captures of the milestone that delivered the
capability SHALL be rerun against the pinned Go agent and SHALL pass with no
new normalization.

#### Scenario: Read-only tools move onto built-in services
- **WHEN** the M3 read-only host tools are revised onto built-in services
- **THEN** the M3 catalog matrix, including the catalog revision, and the M3 read-only scenarios match the pinned Go agent as before

#### Scenario: A capture fails after a move
- **WHEN** a rerun parity capture differs from the pinned Go agent after a capability moves onto the seam
- **THEN** the move is not accepted, and the failing capture is kept and not relabeled

### Requirement: The rule applies from decision D14, with delivered capabilities revised before M8
The rule SHALL apply to every capability implemented or changed after
2026-10-11. Each capability delivered in M1–M7 that does not yet meet it
SHALL be listed by name as a seam gap in the seam verifier's report, and
SHALL NOT be reported as compliant. M8 SHALL NOT start until the seam
verifier reports no gap. Unported capabilities are M8 work and SHALL arrive
on the seam.

#### Scenario: An unrevised M3 tool
- **WHEN** the seam verifier runs before the M3 tools are revised
- **THEN** it lists each of them as a named seam gap and does not report them as compliant

#### Scenario: Gate before M8
- **WHEN** the seam verifier still reports any gap
- **THEN** the M8 entry gate is closed

#### Scenario: A new M8 domain operation
- **WHEN** an M8 sub-milestone ports a domain operation
- **THEN** the operation is offered by a mounted service, and the seam verifier reports neither a gap nor an unported capability for it
