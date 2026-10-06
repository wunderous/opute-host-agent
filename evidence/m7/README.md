# M7 provider lifecycle — in progress

The public provider boundary remains closed. This evidence validates the internal generation core and typed provider definitions; it does not satisfy the M7 wire, provider, restart, or live E2E gate.

## Authority

The reference is the unchanged pinned Go source at `ace7013df17528fee1bed13a1d70a132d6c5eb9b` (tree `ac17cfb298f789095e7a3af2219837e3490f798a`). The oracle imports its actual `internal/cordis` generation manager through a separate module without modifying that checkout.

## Implemented and checked

The Rust generation manager preserves candidate/ready/active/draining/stopped/failed transitions, immutable session affinity, replacement and rollback rules, restore sequence recovery, original timestamps, signed session-count overflow, disposal refusal while sessions remain owned, and cancellation-aware draining. The manager has not been connected to public provider tools or catalog promotion.

`provider-core-current/summary.json` records **235/235 differential traces** (47 cases, five iterations). Each trace retains the input and both implementations' structured outputs. Only newly generated wall-clock timestamps are normalized; separate assertions require creation and activation timestamps to retain generation affinity. Restored explicit timestamps remain exact. Binary, source, driver, and baseline hashes are recorded; `index.json` hashes the evidence files.

The preceding `provider-core/` capture records the earlier 200/200 case set and its own source hashes. `provider-core-final/` records 235/235 before definition validation was added. The current capture retains invalid restoration and signed session-count edge cases against the rebuilt test binary.

Validation: `cargo test --workspace` passed 176 tests with four dedicated evidence fixtures ignored; the generation fixture was exercised by the differential driver. `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --all -- --check` passed.

## Typed provider definitions

`provider-definition/summary.json` records **1,290/1,290 exact validation comparisons** (258 cases, five iterations), with no normalization. The typed descriptor and manifest definitions preserve the Go validation ordering for identity, recipes, services, dependency shape, operation schemas/effects, result selectors, task support, resource costs and input bindings, and teardown. URL validation follows the pinned Go toolchain's `net/url` rules rather than WHATWG URL rules. That capture compared typed inputs only. The subsequent wire capture below exercises decoding and encoding; the transport and adapter remain unpromoted.

All five pinned Go provider modules built with `-mod=readonly`. `provider-builds.json` records their module, descriptor, and binary hashes. This is build-only provenance; these providers have not yet been installed, activated, or called through Rust.

## Typed JSON decoding and encoded manifest identity

`provider-wire/summary.json` records **2,515/2,515 exact comparisons** (503 cases, five iterations) against pinned Go `encoding/json`. Each successful comparison includes both the decoded projection and the exact `json.Marshal` string; errors match exactly. No normalization is used. The Rust decoder accepts an already parsed JSON node and preserves case-insensitive field names, duplicate-field order, scalar null behavior, nested struct and map merging, pointer reset, nil versus empty slices, reused slice slots/backing storage, integer and floating-point overflow errors, omission rules, field order, HTML/Unicode escaping, and generic-map float64 decoding.

The fixture includes duplicate slices that shrink and then grow, recipe inputs that are empty or null, large generic numbers, malformed typed values, and root null values. It does not exercise syntactically invalid JSON, descriptor YAML loading, outbound HTTP, callbacks, or public provider tooling. Those boundaries remain required for M7. `provider-definition-wire-recheck/` retains 1,290/1,290 validator comparisons against this source and binary, and `provider-core-wire-recheck/` retains 235/235 generation comparisons against the rebuilt binary.

Validation after this implementation: the workspace suite again passed 176 tests with four dedicated fixtures ignored; the provider fixtures were invoked explicitly by the drivers. Clippy with warnings denied and formatting checks passed.

## Outbound MCP discovery — T1 subset

`provider-adapter-discovery/summary.json` records **130/130 comparisons** (26 cases, five iterations) using an exclusively bound, OS-assigned loopback HTTP fixture. The Go oracle calls the actual pinned `internal/cordis/mcp.Connect`; Rust calls the new outbound adapter. Captures retain response outcomes, exact request JSON bytes and header values, modern protocol metadata, default/explicit client identity, bearer tokens, HTTP/RPC refusals, unsupported versions, malformed or duplicate tool declarations, and a missing manifest tool. HTTP header names are compared case-insensitively; tool names are projected as a sorted set because Go map iteration is unspecified. No endpoint, identity, body, or header-value rewriting is applied.

The HTTP edge uses locked reqwest 0.12.28 with native certificate roots, HTTP/2 support, gzip and proxy support. Provider calls are still unconnected to public runtime dispatch. A separate Rust test starts a real waiting HTTP connection and proves cancellation closes it; this is not a Go/Rust cancellation parity gate. The workspace suite passed 177 tests with five dedicated fixtures ignored; the adapter fixture was explicitly exercised. Clippy with warnings denied and formatting passed. Existing definition, wire, and core captures were refreshed against this rebuilt binary.

Still required at this edge: tool-result typed projection, manifest calls, synchronous/task bridging, signed callback delegation, cancellation/deadline differential traces, malformed JSON/response type errors, DNS/connection failures, redirects, proxy environment parity, TLS verification/handshake behavior, and all platform transport defaults. Passing discovery does not prove those cases or the M7 gate.

## Remaining M7 work

Descriptor source/YAML loading and transport JSON error handling, outbound provider MCP adapters, candidate validation and atomic catalog promotion, typed callback/task bridging, durable restoration and reverse dependency cleanup, and all five real Go providers' installation/activation/call/teardown checks remain required. Live checks must use owning typed capabilities and independently leased disposable runtimes. No milestone completion or runtime seam promotion is claimed by this core evidence.
