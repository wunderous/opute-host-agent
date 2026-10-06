# M7 provider lifecycle — in progress

The public provider activation/catalog boundary remains closed. This evidence validates internal generation, definitions, and portions of the outbound adapter; it does not satisfy the complete M7 provider, restart, or live E2E gate. **The old `provider-sdk-content/` and `provider-sdk-types/` T0 captures are invalidated:** their oracle serialized the SDK result by value and skipped its pointer-only marshaler. Use `provider-sdk-requests/` for the corrected projection evidence.

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

## Provider calls and task polling — T1 core flows

`provider-adapter-calls/summary.json` records **250/250 comparisons** (50 cases, five iterations) against actual Go adapter methods, covering the preceding discovery cases plus registered calls, unknown-operation refusal, idempotent close/refusal, synchronous-only guards, typed install manifest retrieval, invalid manifest identity/services, completed/failed/cancelled/input-required child tasks, polling failures, and cancellation RPC failures. Captures compare result/error projections, exact request bodies and header values. Task polls and cancellation RPCs carry the actual child task identifier in `Mcp-Name` and retain opaque delegation metadata; the fixture delegation is a synthetic string, not a verified host-issued lease or admission signature.

The adapter holds its read guard for accepted work; closing waits for calls rather than redirecting them. The client polls flat task handles before returning and gives cancellation RPCs an independent two-second context after interrupted waits. Manifest retrieval uses the typed decoder and trusted descriptor identity validator. The current tool-result projection is raw JSON: SDK content union decoding, input-request/result-type projection, malformed response handling, cancellation/deadline differential traces, and real signed callback admission remain explicit gaps before this edge can serve public provider work. The successful cases here use valid SDK tool-result projections and do not prove those omitted cases.

The workspace suite passed 178 tests with five dedicated fixtures ignored; the adapter fixture was invoked by the differential driver. A second real-connection test proves close waits for an accepted call, then refuses new work. Clippy with warnings denied and formatting checks passed. The driver uses immutable binary snapshots and refuses evidence if source provenance changes during a run. M7 remains open.

## SDK content projection — T0 subset

`provider-sdk-content/summary.json` records **400/400 comparisons** (80 cases, five iterations) against actual pinned Go SDK `CallToolResult` decoding and re-serialization. The new projection covers all seven content variants, required/default fields, omitted empty metadata, annotations and icons, nested tool-result restrictions, resource blob nil/empty behavior, base64 decoding and exact corruption offsets, and generic-map float64 precision including values above 2^53. The fixture emits numbers with the existing Go JSON encoder so large floating-point values retain the same delivered decimal representation. Structured projections are compared with no normalization.

**Correction:** that oracle serialized the SDK result by value. The real adapter returns a pointer, whose JSON marshaler preserves nonempty `resultType` and non-nil empty `inputRequests`. The old claim that Go drops `resultType` was incorrect; this historical capture is invalidated. At that capture the projection was deliberately unwired and typed malformed-field validation, case-sensitive SDK field decoding, input-request variants, and adapter integration remained required. The next capture below closes the content-field typing subset. These cases do not establish full SDK decoding or the M7 gate.

The driver uses immutable binary snapshots, checks the pinned Go checkout, and rejects changing source provenance. `go mod verify` reported all modules verified. The workspace suite passed 178 tests with six dedicated fixtures ignored; the SDK fixture was explicitly invoked. Clippy with warnings denied and formatting checks passed.

## Remaining M7 work

Descriptor source/YAML loading and transport JSON error handling, outbound provider MCP adapters, candidate validation and atomic catalog promotion, typed callback/task bridging, durable restoration and reverse dependency cleanup, and all five real Go providers' installation/activation/call/teardown checks remain required. Live checks must use owning typed capabilities and independently leased disposable runtimes. No milestone completion or runtime seam promotion is claimed by this core evidence.

## SDK typed content decoding — T0 subset

`provider-sdk-types/summary.json` records **1,800/1,800 exact comparisons** (360 cases, five iterations). The oracle follows the actual client's generic-result-map → JSON marshal → pinned SDK `CallToolResult` decode → JSON marshal path. Rust now validates SDK field names case-sensitively and preserves malformed scalar, map, pointer, slice, annotation, icon, embedded-resource, and content fields, including fields unused by the selected content variant. The comparisons retain exact error text, anonymous Go struct names, nested field paths, offending-input prefixes, numeric overflow, and base64 error priority. Null entries in typed string slices become empty strings. Generic-map float64 rounding occurs before typed integer validation, matching the client boundary rather than treating large input integers as lossless.

The content projection remains unwired from the adapter. Input-request variants, broader transport errors, signed callbacks, public activation/catalog wiring, restart, and isolated provider journeys remain required. This capture does not close M7 or promote a runtime seam. `provider-wire-sdk-types-recheck/` retains **2,515/2,515** definition-wire comparisons against the rebuilt binary; `provider-adapter-sdk-types-recheck/` retains **250/250** adapter comparisons against the same rebuilt binary.

Validation: workspace **178 passed, six dedicated fixtures ignored** (drivers explicitly exercise their fixtures); Clippy with warnings denied, formatting, and all eight structural OpenSpec validations passed. The SDK and adapter drivers freeze binary snapshots and reject changed source provenance. Each evidence index hashes its raw inputs and Go/Rust outputs.

## SDK input requests and internal adapter integration

`provider-sdk-requests/summary.json` records **3,650/3,650 exact comparisons** (730 cases, five iterations) against the corrected Go client-map → SDK pointer result path. `serializer-canary.json` deliberately compares pointer and by-value marshaling and requires the broken by-value oracle to be detected. The corrected projection preserves `resultType` and a non-nil empty input-request map. The raw historical captures remain available but are explicitly invalidated in `invalidated-sdk-captures.json`.

Input requests preserve standard Go JSON decoding for raw request envelopes, elicitation parameters and mode inference, roots metadata, sampling parameters, model hints/preferences, tool choice, tools/annotations/icons, and interface-valued schemas/metadata. Sampling-message content retains the SDK's case-sensitive union decoder, allowed-content restrictions, array-to-single-object encoding, and error-context behavior. Case-variant repeated fields preserve scalar null semantics, map/pointer merging, pointer resets, and reused slice slots including shrink/grow. The driver also checks error ordering when malformed typed fields and custom content errors coexist.

The provider client now applies this typed projection after direct results and completed child-task results. `provider-adapter-sdk/summary.json` records **440/440 T1 comparisons** (88 cases, five iterations) against actual pinned Go adapter methods, including content defaults, malformed content/refusals, input requests, synchronous guards, manifest calls, and task results. The exclusively bound HTTP fixture uses TCP_NODELAY, matching Go net/http server defaults; latency is not normalized or claimed. Request bytes, header values, task IDs and opaque delegation remain exact.

A separate finding remains an **unapproved proposed divergence**: a null input-request map entry panics in the pinned SDK. `null-input-request/current/` retains five owning-adapter Go panic/Rust refusal observations, equal request traces, binary/source provenance and a regression proving later valid decoder input still succeeds. The proposed `nil InputRequest entry` refusal is in `openspec/changes/reject-null-provider-input-requests/`; owner review is pending. Those observations are not reported as Go parity. Public provider activation remains closed, and this proposal does not waive any M7 gate.

Validation: workspace **179 passed, six dedicated fixtures ignored** (the drivers invoke their fixtures explicitly), Clippy with warnings denied, formatting, and nine OpenSpec structural validations passed. Complete HTTP error/cancellation parity, signed callback admission, public lifecycle/catalog wiring, persistence/restart, all provider journeys, and later milestones remain outstanding.

## Typed response envelopes and Go syntax diagnostics

`provider-response-envelopes/summary.json` records **40,495/40,495 exact T0 comparisons** (8,099 cases, five iterations). The oracle calls the actual pinned `internal/mcphttp.Client.Call` with a fixed-response HTTP transport. Rust now decodes its response envelope with Go's typed rules: result-map merging/reset, error-pointer merging/reset, case folding, duplicate order, scalar null semantics, integer/float64 failures, and first typed-error precedence. A byte-state validator preserves the pinned Go JSON syntax diagnostics before typed decoding. Every byte value is exercised at scanner-state prefixes, alongside truncated literals, strings/escapes, invalid UTF-8, malformed typed envelopes, and overflow nested in generic result values. These are response-codec checks; the fixture does not establish DNS, TLS, proxy, cancellation, or deep-runtime processing behavior.

`provider-adapter-envelopes/summary.json` records **755/755 T1 comparisons** (151 cases, five iterations), retaining exact request traces and final result/error projections through the actual owning adapters on exclusive loopback ports. It includes the existing discovery, calls, manifest, SDK and task cases plus empty/truncated/syntactically invalid bodies, malformed root/result/error fields, case variants and duplicate fields, generic-number overflow/precision, and invalid UTF-8 or unpaired Unicode escapes.

The byte matrix found a shared formatter gap: Rust's previous `goerr::quote` treated nonbreaking space and soft hyphen as printable. It now uses generated **pinned Go 1.25.4 `strconv.IsPrint` data**, whose source hash is retained in the generated module. The deterministic generator and Go BSD notice are checked in. `provider-sdk-unicode/summary.json` records **20,325/20,325** comparisons, including the full preceding SDK case set and actual SDK refusal diagnostics at every pinned printable-range and exception boundary. This checks the shared formatter through its consumer, rather than testing Rust against a copy of itself.

`null-input-request/envelope-recheck/` again records five Go panic/Rust proposed-refusal observations through the rebuilt adapter, plus the subsequent-valid-input regression. The owner decision remains pending, and these are explicitly unverified divergence observations.

Validation: **179 workspace tests passed, seven dedicated fixtures ignored** (the drivers invoke them explicitly); Clippy with warnings denied, formatting, and nine structural OpenSpec validations passed. The Go module cache verified successfully. Source/binary provenance and evidence indices were checked after capture. Cancellation/deadline parity before headers versus during body reads, connection failures, redirects, proxy/TLS behavior, deep-runtime responses, signed callbacks, lifecycle/catalog promotion, persistent generation recovery and isolated provider journeys remain required. No M7 exit gate or runtime promotion is asserted.
