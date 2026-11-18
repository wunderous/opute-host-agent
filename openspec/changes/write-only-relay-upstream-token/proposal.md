# The LLM relay's upstream token is writeOnly

Status: **accepted (Host Agent owner, delegated 2026-11-11) as D51**, under D46 ([`secret-placement`](../secret-placement/proposal.md)). Goal m19, finding S5. Applies to 0.3.0 from `rust-0.3.0-rc6` and to 0.4.0 from `rust-0.4.0-rc4`.

## Why

`ensure_local_llm_relay` declares `upstreamToken` without `writeOnly`, so
D13's storage projection keeps it verbatim in the audit row. Its
`incomingToken` is not declared at all, so the fail-closed rule already
redacts it. `e2e6.secret-sweep` found the upstream token in the state store
on rust-0.3.0-rc5 and rust-0.4.0-rc3 (finding S5). A catalog scan found no
other declared credential input without `writeOnly`.

## What Changes

`upstreamToken` joins D24's list (`crates/host-agent/catalog/write-only.json`),
so it is published `writeOnly` and the store records it as
`[redacted]`. The catalog revision changes; the input it accepts does not.

## Options rejected

- **Declaring `incomingToken` too.** It is already redacted; declaring it
  would change the published schema for no gain.

## Impact

`catalog/write-only.json`, the catalog revisions in `parity-manifest.json`,
and the test `catalog::tests::every_declared_credential_input_is_write_only`,
which failed on rc5's and rc3's code.
