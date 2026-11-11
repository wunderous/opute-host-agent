# A resource id holds no Unicode whitespace

## Why

m16's property tests (finding F1) found that `Uri::new` refused an id with a
space, tab, CR or LF, but accepted one with other Unicode whitespace
(U+000B, U+000C, U+0085, U+2003, ...). `Uri::parse` trims surrounding
whitespace, so the printed URI of such an id parsed back to a different
identity: `vm:t:a\u{2003}` became `vm:t:a`. The contract spec says the agent
resolves canonical URIs without rewriting IDs.

**Decided (owner, 2026-11-10), recorded as D30.** Refuse such ids.

## What Changes

`Uri::new` and `Uri::parse` refuse a resource id that contains any character
Rust's `char::is_whitespace` matches (Unicode `White_Space`, the same set
`trim` removes), with the existing message `invalid resource URI: resource id
is empty or contains whitespace`. `Uri::parse` still trims whitespace around
the whole value, as before. Every id accepted before that held no such
character is accepted unchanged. Go is unchanged.

## Capabilities

### New Capabilities

- `resource-id-unicode-whitespace`: the whitespace rule for resource ids.

### Modified Capabilities

None.

## Impact

`crates/host-agent/src/resource.rs`; the property tests
`accepted_resource_uris_round_trip` and `parsed_resource_uris_are_canonical`,
and the failing-first test `resource_ids_with_unicode_whitespace_are_refused`.
A product change: the next candidate is rust-0.3.0-rc2.
