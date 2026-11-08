# Refuse null arguments to the Cloudflare host-tunnel operations

## Why

The pinned Go provider `opute-provider-cloudflare` (v0.2.4) decodes a
`tools/call` whose `arguments` is JSON `null` into a nil `map[string]any`
(`requestArguments`, `main.go:927`). The host-tunnel operations
(`opute.capability.tunneling.ensure-host-tunnel` and its legacy aliases
`ensure_cloudflared_tunnel` and `create_cloudflare_tunnel`) then write the
derived `connector` into that map (`ensureTunnel`, `main.go:282`), which
panics with `assignment to entry in nil map`. go-sdk does not recover a
tool handler's panic, so the provider process exits with status 2 and every
other call in flight on it is lost. The Host Agent never sends null
arguments; any other caller that reaches the provider's port can end it.

The M9 port (`port-provider-executables`) preserves Go's behaviour: the Rust
executable panics with the same message and exits with status 2. The
provider wire corpus (`tools/parity/provider-wire/cloudflare.json`, recorded
with a restart after each exit) shows Go and Rust equal on this step.

## What Changes

Decided (owner, 2026-11-07, D28, Q7): the host-tunnel operations treat
null arguments as an empty argument map, so the call is refused with the
existing `bindingId and localTarget are required` error and the process
keeps serving. No other operation, argument shape or error text changes.
The wire corpus's null-argument steps for these three tools are now a named
difference from Go (Go exits with status 2, Rust refuses); under the
2026-11-04 decision to stop Go parity checks the corpus is not rerun.

## Capabilities

### New Capabilities

- `cloudflare-null-arguments`: refusal of null arguments by the Cloudflare
  host-tunnel operations.

### Modified Capabilities

None.

## Impact

`crates/host-agent/src/providers/support.rs` (`Args::set` on a nil map) and
the cloudflare wire corpus. A Go fix belongs to the Go tree's owner; Go
remains the authority for every other step of the corpus.
