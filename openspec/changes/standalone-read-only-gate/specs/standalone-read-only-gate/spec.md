## ADDED Requirements

### Requirement: Standalone without mutations runs read tools only

When Host Agent serves in standalone mode and
`OPUTE_STANDALONE_ALLOW_MUTATIONS` is not enabled, it SHALL run a
`tools/call` only when the tool's effect is `read`. For a tool the server
publishes in `tools/list`, the effect SHALL be the one in its published
descriptor. For a dispatchable tool the server does not publish, the effect
SHALL count as `read` only when the pinned contracts declare it; an effect
that is inferred because nothing was declared SHALL be treated as not
`read`. A tool on the Go standalone mutation list SHALL be refused
regardless of its effect.

#### Scenario: Published non-read tool
- **WHEN** a standalone client with mutations disabled calls any published tool whose descriptor effect is not `read`
- **THEN** the call returns the error result `standalone mutations are disabled; set OPUTE_STANDALONE_ALLOW_MUTATIONS=true`

#### Scenario: Published read tool
- **WHEN** a standalone client with mutations disabled calls a published tool whose descriptor effect is `read`
- **THEN** the gate does not refuse the call

#### Scenario: Unpublished tool without a declared read effect
- **WHEN** a standalone client with mutations disabled calls a dispatchable tool that is not published and whose effect is not declared `read`
- **THEN** the call is refused with the same error result

#### Scenario: Unpublished tool with a declared read effect
- **WHEN** a standalone client with mutations disabled calls a dispatchable, unpublished tool whose declared effect is `read`
- **THEN** the gate does not refuse the call

#### Scenario: Mutations allowed
- **WHEN** the agent serves in standalone mode with `OPUTE_STANDALONE_ALLOW_MUTATIONS=true`
- **THEN** the gate refuses no tool

#### Scenario: Platform mode
- **WHEN** the agent serves in platform mode
- **THEN** the standalone gate refuses no tool; platform authorization and admission apply as in Go

### Requirement: The gate refuses before any effect

A call refused by the standalone gate SHALL be refused after the catalog
revision check and argument decoding and before lifecycle routing, task
handling, resource binding, admission, and handler dispatch. It SHALL leave
no operation, plan run, invocation, reservation or task record and run no
host command.

#### Scenario: Refused call leaves no trace
- **WHEN** a standalone client with mutations disabled calls every published non-read tool
- **THEN** the state store has no operation, plan run or invocation rows afterwards

#### Scenario: Stale catalog revision still wins
- **WHEN** a refused tool is called with a stale catalog revision
- **THEN** the call returns `catalog_revision_stale`, as it does for every tool

### Requirement: The gate does not change the catalog

The standalone gate SHALL NOT change the published catalog: names,
descriptors, effects, schemas and the catalog revision stay identical to
the Go baseline in every mode.

#### Scenario: Catalog parity
- **WHEN** Go and Rust start in standalone mode with mutations disabled
- **THEN** their `tools/list` results and catalog revisions are identical
