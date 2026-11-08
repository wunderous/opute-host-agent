## ADDED Requirements

### Requirement: Null arguments to a host-tunnel operation are refused without ending the provider
The Cloudflare provider SHALL treat a `tools/call` of
`opute.capability.tunneling.ensure-host-tunnel`, `ensure_cloudflared_tunnel`
or `create_cloudflare_tunnel` whose `arguments` is JSON `null` as a call with
no arguments, and SHALL refuse it with `bindingId and localTarget are
required`. The provider process SHALL keep serving. This requirement is a
deliberate deviation from the pinned Go provider's panic, approved by the
owner on 2026-11-07 (D28).

#### Scenario: A caller sends null arguments to the legacy alias
- **WHEN** a client calls `ensure_cloudflared_tunnel` with `"arguments": null`
- **THEN** the result is the error `bindingId and localTarget are required` and the next call on the same process is served

### Requirement: The difference stays explicit in evidence
The evidence SHALL name the refusal of null host-tunnel arguments as a
difference from the pinned Go provider and SHALL NOT report parity for that
step.

#### Scenario: The wire corpus covers null arguments
- **WHEN** the cloudflare wire corpus sends null arguments to a host-tunnel operation
- **THEN** Go's exit and Rust's refusal are recorded as a named difference, not as parity
