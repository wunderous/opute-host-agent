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

## Native HTTP cancellation and deadlines

`provider-client-cancellation/summary.json` records **35/35 exact T1 comparisons** (seven cases, five iterations) through the actual pinned Go MCP client and Rust client on exclusively bound native loopback sockets. Cases cover cancellation before dispatch, an already expired context, cleanup cancellation after expiration, cancellation/deadlines while waiting for headers, and cancellation/deadlines after an observed partial response-body read. Request bodies, header values, opaque delegation, error strings, body-read observations and socket EOF agree. Preflight refusals send no request. Go's transport wrapper and Rust's test-only progress markers observe native read stages without substituting response errors; body cases wait for the client's read marker before explicit cancellation. No error or request normalization is applied.

The matrix exposed two gaps: body-read cancellation errors must be returned directly after HTTP headers, while header-wait errors retain Go's `Post` URL wrapper; cleanup cancellation must preserve an existing deadline error. Rust now separates these boundaries and preserves that terminal cause. This proves the listed cancellation orderings only; parent/child cause ordering, task-wait cancellation, redirects, TLS/proxy behavior and connection failures remain open.

`provider-adapter-cancellation-recheck/summary.json` retains the existing **755/755** native owning-adapter comparisons through the revised body reader. Both drivers freeze binaries, verify the pinned Go checkout, reject source changes during capture, and hash raw evidence. Adapter provenance now includes the shared plan cancellation context. Validation: **179 workspace tests passed, eight dedicated fixtures ignored** (differential drivers invoke their fixtures explicitly); Clippy with warnings denied, formatting, nine structural OpenSpec validations and Go module verification passed. M7–M13 remain active; no M7 exit gate or runtime promotion is asserted.

## Native task waits and independent cleanup cancellation

`provider-task-cancellation/summary.json` records **70/70 exact T1 comparisons** (14 cases, five iterations) through the actual pinned Go `mcphttp.Client.CallTool` and Rust client. Cases cover pre-dispatch cancellation/expiration; cancellation and deadlines during `tasks/get` headers, observed partial status-body reads, and the interval between polls; and cleanup RPC success, typed refusal, stalled headers and stalled bodies. Each admitted journey makes exactly `tools/call` → `tasks/get` → `tasks/cancel`; preflight refusals make no request. The task ID, signed opaque delegation, request bodies, header values, final error strings and body-read observations agree. EOF observations from independent sockets are sorted by their unique method; no other result normalization is used.

These checks exercise Go's independent two-second `context.WithoutCancel` cleanup path: after the parent has ended, the cleanup still reaches the provider with its original delegation, and stalled cleanup closes its socket with the matching deadline error. The socket fixture bounds its close observation to four seconds; it does not claim an exact wall-clock latency comparison. Header and body failures retain their distinct error wrapping through the enclosing task failure. Poll-interval cases require the unwrapped context cause and exactly one status request, proving the cancellation occurred during the wait rather than masking a status-fetch failure. Progress markers are filtered to `tasks/get`, so the initial task-handle response cannot falsely establish a status read.

`provider-client-cancellation-task-recheck/` retains **35/35** direct-client cancellation comparisons through the expanded fixture and rebuilt binaries. The runtime client behavior required no further change in this capture; only test instrumentation and the actual Go oracle were extended. Source/binary hashes, pinned checkout checks and raw evidence indices bind both captures. Workspace **179 passed, eight dedicated fixtures ignored**; drivers explicitly invoked the cancellation fixture. Clippy with warnings denied, formatting, all nine structural OpenSpec validations and the pinned Go `internal/mcphttp` focused suite passed.

Generation admission/affinity, signed callback validation, candidate/private catalog projection, atomic activation, provider state recovery, real provider journeys, parent/child context-cause ordering, redirects and TLS/proxy behavior remain open. The null-input-request proposed divergence still requires owner review. This evidence does not close M7 or authorize public provider activation.

## Shared provider descriptors and private candidate catalogs

`provider-catalog-projection/summary.json` records **790/790 exact T0 comparisons** (158 cases, five iterations) against the owning pinned Go `providerOperationDescriptor`, `providerCapabilitiesForGeneration` and `providerCandidateSnapshot` functions. A checked-in build-only overlay bridge exposes those functions to a separate oracle without modifying the pinned source tree or copying their projection. The fixture seeds candidate manifest/adapter-presence maps with an inert adapter; it exercises catalog projection, not transport connection, readiness or registry admission. `python3 tools/parity/build-provider-catalog-oracle.py` builds the oracle with a read-only module lock.

The shared Rust descriptor now carries operation/capability identity, version, task support, validation/observation schemas and exact generation identity. Built-ins retain their preceding encoded catalog and revision. Provider operations use their declared effect, approval, resource cost/bindings, result types and readiness metadata without importing built-in host defaults. Trusted dynamic descriptors enter the same default-label, edge-generation and exact Go JSON hash path. Rebuilding clears prior generated edges and argument-producer metadata. Candidate projection filters only the matching provider from the active snapshot, adds its generation descriptors and returns a separate snapshot.

The matrix covers both serving modes; missing/zero/negative/maximal versions; description/observation defaults and whitespace; case-sensitive effects; generic-map float64 precision; schema default labels; result selectors; nil/empty arrays and maps; costs and argument bindings; task support; duplicate names/services; built-in name collisions; linked producer/consumer operations; replacement of the same provider; preservation of other providers; removal of old edges; and opaque/replaced generation IDs. Raw descriptor strings, complete active/candidate catalogs, field order, edges and revision bytes are compared without normalization. Full raw outputs are retained as deterministic, lossless `.json.gz` files and covered by the evidence index. Invalid manifest examples here test projection only; manifest validation remains a separate owning boundary.

Validation: **179 workspace tests passed, nine dedicated fixtures ignored** (the driver explicitly invokes the new catalog fixture); Clippy with warnings denied, formatting and a locked release build passed. All nine OpenSpec structural validations, the actual Go descriptor task-support test, focused Go catalog tests and oracle module verification passed. Source/binary hashes and raw evidence indices were verified after capture.

The candidate helper is not yet wired to the public server. Connection presence checks, readiness, registry validation, atomic catalog publication, generation-bound invocation, signed callback admission, durable restoration and all real provider journeys still require runtime integration and E2E evidence. Public provider activation remains closed. This projection subset does not close M7 or promote a runtime seam.

The catalog change invalidated the old M5 secret-projection source binding. `provider-catalog-secret-recheck/` retains a fresh sweep against the **current locked release binary** and current projection/storage fixtures: **16 marked fields, both modes, zero failures**, with durable sink scans and recovered MCP task responses rechecked from the retained raw data. This supplements the historical M1–M6 evidence; it does not replace the manifest's earlier binary provenance or claim the whole candidate is validated.

`provider-catalog-cutover.log` records the required whole-agent verifier invocation. It **fails closed** on **76 inventory gaps** and the historical M5 secret projection's stale runtime-source binding. The new sweep proves the affected current check, but a complete manifest/binary/evidence refresh is still required before any runtime seam promotion. No failed or stale gate was converted into a pass.

## Executable registry ownership and atomic replacement

`provider-registry/summary.json` records **1,015/1,015 exact T0 comparisons** (203 scenarios, five iterations) with the actual pinned Go executable catalog registry. The internal Rust registry owns immutable executable references, provider authorization, installed implementation identity, registration normalization, metadata checks, revisioned resolution, upsert/downgrade rules, generation/provider replacement, family displacement and unregister. Replacement validates the complete proposed overlay and its conflicts before removing existing entries. Every observed step retains its full encoded snapshot and revision, including steps that fail.

The matrix covers metadata-only/nil executable refusals; registration identity, name/version, provider and implementation authorization; base/overlay conflicts; cost declarations and argument paths; known resource kinds; typed array/nested bindings; result selectors and cardinality; task support; duplicate, nil, mismatched-generation and mixed-provider replacements; stale/missing revisions; replacement/removal of overlays; exclusive family displacement; and invocation through retained old versus newly resolved executable handles. Those marker invocations prove registry reference ownership only: lifecycle session admission and in-flight generation affinity still require integrated runtime evidence. No result or error normalization is applied. Ambiguous first-error selection when several cost dimensions are negative remains outside this exact subset.

**Schema compilation remains a required unresolved dependency.** Registry construction requires an explicit schema checker; production has no default success checker. The test-only fixture asserts a declared well-formed, reference-free schema subset and aborts for unsupported keywords. Go uses its real schema compiler for the same fixtures. These comparisons establish registry transactions and metadata rules on that subset; they do not establish native Go JSON Schema decoding/resolution parity, malformed-schema behavior or callable public activation. The schema callback is a concrete dependency boundary corresponding to Go's schema compiler, not a product endpoint or configurable policy.

Descriptor cloning now also matches Go's JSON round-trip omission of empty optional maps before rebuilding a snapshot. `provider-catalog-registry-recheck/` retains **790/790** complete descriptor/candidate-catalog comparisons through the new build. `provider-registry-secret-recheck/` retains a fresh **16-field, two-mode secret sweep with zero failures** against the current locked release and current projection/storage fixtures; retained sink scans were independently rechecked.

Validation: **179 workspace tests passed, ten dedicated fixtures ignored** (drivers invoke their fixtures explicitly); Clippy with warnings denied, formatting, locked release build, all nine structural OpenSpec validations and the owning Go `internal/catalog` suite passed. Source/binary provenance and all raw evidence indices were verified. `provider-registry-cutover.log` retains the required whole-agent check, which fails closed on **76 inventory gaps** and the historical M5 projection source binding. The new sweep supplements historical evidence; complete manifest refresh is still required before promotion.

Native schema compilation, provider connection/readiness, activation publication, generation-bound admitted invocation, signed callbacks, durable recovery and real provider E2E remain open. Public provider activation remains closed, and M7–M13 remains active.

## Owning schema compiler baseline and native integer decoding

`provider-schema-compiler/summary.json` captures **1,520 compiler cases, five
iterations**, by calling the actual pinned Go registry's `validateJSONSchema`
through a build-only overlay. The pinned checkout remains unchanged. Complete
inputs and exact Go results are retained; all observed results agreed across
these five runs. This is a Go behavior capture, not 7,600 Rust parity passes.
The bridge and frozen oracle binary are bound to the source lock and the
google/jsonschema-go v0.4.3 module sources.

The corpus covers every JSON-visible schema field's typed decoding; null and
Boolean subschemas; nested error paths and escaped property keys; aliases;
integer bounds and fractional values; malformed-error precedence; empty versus
nil definitions and vocabulary; unknown keywords and type names; draft-07 and
2020-12 IDs, local/remote references, anchors, dynamic references, JSON Pointers
and recursive references; and Go regexp acceptance and error strings. This
capture does not establish that every possible schema or first-error order is
covered. In particular, multiple invalid entries in a Go map can still have
nondeterministic first-error selection.

The native compiler's first component preserves Go's custom nullable int32
decoding for all eight integer keywords. **1,040/1,040 exact comparisons** verify
decoded values, nullable pointers and malformed error strings. These cases also
exercise the owning boundary's float64-map marshal before decoding: the
original manifest number spelling is not the decoder's input. Negative bounds
are accepted by Go; replacing this component with a stricter meta-schema check
would change the migration contract. The native component is private and is
not wired as a complete registry checker. Typed schema-tree decoding, Go regexp
compilation, URI/reference resolution and their full differential validation
remain required.

`provider-schema-rechecks.json` binds the rebuilt registry/catalog regression
reports to their existing complete raw evidence. The retained raw input/output
hashes are checked against the new captures before reuse. No result is
normalized. `provider-schema-secret-recheck/` records a fresh sweep against the
current locked release and projection fixtures: **16 marked fields, both
modes, zero failures**.

Validation: **179 workspace tests passed, eleven dedicated fixtures ignored**;
the driver explicitly invokes the new integer fixture. Formatting, Clippy with
warnings denied, locked release build, all nine structural OpenSpec checks and
the owning Go catalog tests passed. Raw evidence indices and source provenance
were independently verified. `provider-schema-cutover.log` retains the required
whole-agent verifier result: it fails closed on **76 inventory gaps** and the
historical M5 projection source binding. This work does not close M7, promote
provider activation or authorize cutover.

## Native typed schema-tree decoding

`provider-schema-decoder/summary.json` records **18,185/18,185 T0 comparisons**
(3,637 cases, five iterations) with the actual pinned
`jsonschema.Schema.UnmarshalJSON`. The oracle marshals the same generic map
as the owning registry, decodes through the real library and mechanically
reflects every exported field. It preserves nil versus empty collections,
raw/const presence, exact raw default bytes and exact float64 bits. The
comparison treats numeric JSON spellings by value while keeping Booleans
distinct; no malformed error text is normalized. This private typed-tree
projection does not claim product wire serialization or `Schema.Resolve`
parity. The pinned checkout remains unchanged.

The native decoder now owns typed child pointers, maps and slices, separated
`type`, `items` and dependency unions, metadata, generic extras, raw defaults
and nullable integer/float values. It follows Go's case-alias merging and
slice backing reuse, including the fresh allocation for an empty array.
Custom integer and child-schema errors stop decoding immediately; ordinary
standard JSON type errors are remembered while decoding continues. Special
union fields are decoded after the typed struct phase, and integer values
are assigned after those unions, matching Go's error precedence and reuse.
Explicit null defaults and constants remain distinguishable from absence.
Float64 storage retains negative zero, and the raw default bytes remain
available to the forthcoming typed JSON Pointer resolver. Compiler errors
are owned within the catalog module; compilation has no MCP dependency.

The matrix includes the preceding 1,520 compiler-capture inputs (evaluated
here for decoding only), explicit scalar/child/map/slice aliases and backing
reuse, nested custom-error precedence, union fields accumulated across child
decodes, unknown extras, null/Boolean children, escaping, negative zero and
2,048 seeded composite cases. All observed Go outcomes agree across five
runs. Multiple malformed dependency entries with nondeterministic Go map
first-error selection and deeper input limits still require dedicated
evidence. The originating library's MIT notice is retained in
`crates/host-agent/src/catalog/schema-LICENSE.txt`.

Validation: **179 workspace tests passed, twelve dedicated fixtures ignored**;
the driver explicitly invoked the new decoder fixture. Formatting, Clippy
with warnings denied, locked release build, all nine structural OpenSpec
checks and the owning Go catalog suite passed. Complete raw evidence indices
and source/library/binary provenance were independently verified.
`provider-schema-decoder-secret-reuse.json` records reuse of the preceding
16-field, two-mode secret sweep only after verifying the new locked release
is byte-for-byte identical, its projection runtime sources are unchanged and
the retained outcome hashes agree. The native decoder remains unwired, so no
public behavior changed.

`provider-schema-decoder-cutover.log` records the required whole-agent gate,
which still fails closed on **76 inventory gaps** and the historical M5
projection source binding. Structural/local schema checks, Go regexp
compilation, URI/anchor/reference resolution and registry wiring remain open.
M7 is not complete and public provider activation remains closed.

## Typed JSON Pointers and structural indexing

`provider-schema-pointers/summary.json` records **25,215/25,215 T0 comparisons**
(5,043 cases, five iterations) with the actual private Go
`dereferenceJSONPointer` and `checkStructure` functions. The native resolver
traverses the decoded typed tree, including the `type`, `items` and dependency
unions. It preserves case-sensitive field lookup, one-pass `~0`/`~1`
unescaping, signed array indices, leading-zero rejection, nil schema targets,
pointer-to-scalar diagnostics, generic interface diagnostics and raw default
byte indexing. Pointer refusals, nil results and complete reflected target
schemas agree; float64 bits and raw default bytes remain in the projections.
These are resolver-component checks, not full `Schema.Resolve` or provider
activation checks.

Structural indexing follows the owner's schema-field order, visits only typed
child schemas and reports escaped paths for nil array/map entries. Owned JSON
decoding constructs a tree, so shared or cyclic child pointers cannot arise at
this boundary. Go deliberately leaves map iteration order unspecified. Ten
multi-fault cases have **two actual Go refusal outcomes each**, captured across
128 independent owning-helper runs. Native map traversal also leaves the
first invalid map child unspecified. Each native result must equal a complete
observed Go outcome; no diagnostic text is rewritten. A negative canary
rejects an invented refusal. Successful structural paths are sorted by the
test-only Go bridge because its internal index is a map; this does not change
product output.

The matrix covers absent and populated schema fields, typed and generic
collections, every JSON-visible keyword, nil/Boolean schemas, map-key escaping,
malformed pointers, index overflow and signed indices, special unions,
accumulated child unions, malformed schema precedence, raw UTF-8/escaped
defaults and 522 finite numeric diagnostic cases including seeded float64
bit patterns. The native fixture reads generic map numbers through the owning
Go-compatible JSON codec rather than an unrelated fixture decoder.

Go disallows overlays inside its module cache. The build therefore verifies
module checksums, takes a byte-identical private snapshot of the pinned
google/jsonschema-go v0.4.3 dependency and uses an alternate build-only module
file to add a virtual bridge. Full source/snapshot file inventories, module
locks, source lock and frozen binaries are bound to the evidence and checked
before and after capture. Neither the module cache nor the pinned Go checkout
is modified; the bridge calls the actual private implementation.

Validation: **179 workspace tests passed, thirteen dedicated fixtures ignored**;
the driver explicitly invoked the resolver fixture. Formatting, Clippy with
warnings denied, locked release build, all nine OpenSpec checks, module
checksum verification and the owning Go catalog suite passed. Source,
snapshot and complete raw evidence indices were independently verified.
`provider-schema-pointers-secret-reuse.json` verifies that the locked release
and projection runtime sources are still byte-identical before reusing the
preceding 16-field, two-mode secret sweep and its retained outcome hashes.

`provider-schema-pointers-cutover.log` retains the required whole-agent gate.
It still fails closed on **76 inventory gaps** and the historical M5 projection
source binding. Local/basic schema checks, Go regexp compilation,
URI/anchor/reference resolution and registry wiring remain open. Public
provider activation remains closed and M7–M13 remains active.

## Native local checks and URI components

`provider-schema-local/summary.json` records **3,985/3,985 T0 comparisons**
(797 cases, five iterations) against the actual private Go `Schema.check`.
The native checks preserve structural-error precedence, Type/Types,
Defs/Definitions and Items/ItemsArray conflicts, dependency schema/string
conflicts, vocabulary restrictions, schema description labels, sorted child
traversal and joined local errors. Five cases with multiple faulty map entries
accept only a complete observed Go outcome from 128 independent reference
runs; an invented refusal is rejected. The fixture supplies an explicit
regexp callback that accepts only an empty pattern and panics if any nonempty
pattern reaches it. Nonempty patterns are covered only when the owning basic
check short-circuits regexp compilation at that same node. This does not
establish regexp parity or provide a production success callback.

`provider-schema-uri/summary.json` records **106,055/106,055 T0 comparisons**
(21,211 cases, five iterations) against actual Go 1.25.4 `net/url.Parse` and
`URL.ResolveReference`. The private native implementation preserves schemes,
opaque URLs, authority and user/password presence, host/path/fragment bytes,
raw escape hints, query flags, escaped component strings and `URL.String`.
Percent decoding can produce invalid UTF-8, so decoded bytes are projected as
hex in both fixtures rather than replaced or canonicalized. Complete error
strings are compared unchanged, including `netip.ParseAddr` IPv6 and embedded
IPv4 diagnostics. The matrix includes all ASCII characters, all 256 percent
bytes in both hex cases, malformed escapes and hosts, IPv6 zones and ports,
RFC reference examples, Unicode, 12,000 seeded parser inputs with sampled
reference composition, and 2,000 generated IP-literal boundaries.

An initial IPv4 trailing-dot error mismatch was corrected before retained
capture. Bracket ordering also follows Go's port-validation precedence. No
parser refusal was waived. This URI component is not a schema ID, anchor,
`$ref` or `$dynamicRef` graph resolver, and does not make the registry's full
schema compiler available.

`provider-schema-local/pointer-regression/summary.json` retains a new
**25,215/25,215** owning pointer/structure run after the shared traversal
refactor. Every matrix retains inputs, full raw Go and Rust outcomes, exact
source and binary provenance, and checked evidence indices. The new driver
also binds the Go URI, netip and quoting source files. Module-cache and
private library snapshot inventories remain unchanged; no Go source checkout
or dependency cache is modified by the build-only bridge.

Validation: **179 workspace tests passed, fourteen dedicated fixtures ignored**;
the differential drivers explicitly invoked their fixtures. Formatting,
Clippy with warnings denied, locked release build, all nine OpenSpec checks,
module checksum verification and the owning Go catalog suite passed.
`provider-schema-local-uri-validation.json` records the checks and hashes.
`provider-schema-local-uri-secret-reuse.json` verifies the exact unchanged
release binary, projection runtime sources and retained secret outcomes before
reusing the preceding 16-field, two-mode sweep. These private components are
unwired and the release binary is byte-identical.

`provider-schema-local-uri-cutover.log` retains the whole-agent verifier:
**FAIL**, with 76 inventory gaps and the historical M5 secret projection's
stale source binding. A full manifest refresh remains required. Go regexp
compilation, schema ID/anchor/reference graph resolution and registry wiring
remain open. Public provider activation stays closed; M7 is incomplete and
the M7–M13 goal remains active.

## Native schema identity and reference graph

`provider-schema-graph/summary.json` records **35,775/35,775 T0 comparisons**
(7,155 cases, five iterations). The build-only bridge calls the actual private
Go `resolveURIs` for URI-only cases and actual public `Schema.Resolve` for
reference cases. It projects the resulting typed graph: draft, URI-to-schema
registrations, every schema's base and optional URI, anchors and their dynamic
flags, lexical and dynamic reference targets, and required-property sets.
Targets are identified by the owning typed tree's paths, not reconstructed
from generic JSON. Complete errors and graph projections compare unchanged.

The native graph preserves root draft selection, relative ID composition,
original-base aliases, scoped anchors, first-registration anchor collisions,
last-registration URI collisions, draft-7 fragment IDs and `$ref` ID ignoring,
lexical and dynamic references, nil-success pointer targets and the owning
compiler's default remote-loader refusal. Nested schemas do not override the
root's draft selection. Local reference cycles link identities without
recursively expanding the schema. The registry calls `Resolve(nil)` in Go, so
no remote loader or default-validation mode is added in Rust.

Percent decoding may produce invalid UTF-8 in a reference fragment. Typed
pointer traversal now processes segment bytes directly: invalid bytes cannot
match a JSON schema field or map key and remain exact in the quoted refusal.
All 256 byte values are exercised across anchor, field, array, string, map,
raw default and scalar pointer positions. The matrix also covers URI/error
precedence, required-set construction, scoped and duplicate anchors/IDs,
canonical/original base lookup, missing remote documents, malformed pointers,
1,000 seeded coherent trees, the JSON-representable upstream `TestResolveURIs`
fixture and local reference cycles. Three multi-fault structural-map cases
accept only a complete owning Go refusal captured in 128 independent runs;
an invented refusal is rejected.

These fixtures remain explicitly regexp-free. A nonempty regexp reaching the
native fixture callback panics rather than returning an invented acceptance.
The actual Go resolver runs its full local checks. Full Go-compatible regexp
compilation and registry compiler wiring are still open, so this is not a
complete provider admission or activation result.

`provider-schema-graph-regressions/` retains **25,215/25,215** fresh pointer and
structural comparisons and **3,985/3,985** fresh local comparisons after byte
traversal changed. Earlier component captures remain historical: their source
and test-oracle hashes are not relabeled as current. Inputs, full raw twin
outcomes, source/library inventories, frozen binary hashes and evidence indices
are retained and checked before and after capture.

Validation: **179 workspace tests passed, fourteen dedicated fixtures ignored**;
the differential drivers explicitly invoked their fixture. Formatting, Clippy
with warnings denied, locked release build, all nine OpenSpec validations,
pinned Go catalog tests and seven focused upstream resolver tests passed.
Source, snapshot and raw evidence bindings were independently verified.
`provider-schema-graph-validation.json` records the checks and retained log
hashes. `provider-schema-graph-secret-reuse.json` verifies the unchanged release,
projection runtime sources and preceding 16-field, two-mode secret outcomes.

`provider-schema-graph-cutover.log` records **FAIL** on 76 inventory gaps and
the historical M5 secret projection source binding. The fresh secret sweep
binds the current M7 release; the selected whole-agent manifest still binds
the earlier M1–M6 artifact. Splicing the new sweep into that manifest would
fail binary provenance. A complete manifest refresh is required before seam
promotion. Public provider activation remains closed, M7 remains incomplete
and the M7–M13 goal remains active.

## Native regexp grammar; resource accounting remains incomplete

`provider-regexp-grammar/summary.json` retains **138,920/138,920 T0 comparisons**
(27,784 cases, five iterations) against actual pinned Go `regexp.Compile`.
The private native parser preserves grammar acceptance and complete errors for
captures, inline flag directives and their scope, empty branches, character
classes and ranges, POSIX classes, Perl and Unicode class escapes, numeric and
C escapes, quoted literals, repeat syntax/counts, nested repetition and capture
nesting in this corpus. This is a scoped grammar result, not a complete native
regexp compiler or a registry admission pass.

Unicode name facts come from Go 1.25.4 / Unicode 15.0.0 categories, scripts and
category aliases, filtered through actual Go `regexp.Compile`. The generator
is reproducible with `python3 tools/parity/generate_regexp_names.py`; the driver
regenerates the facts and rejects a changed native table. All raw name
candidates and case/underscore/hyphen/space variations are exercised. The
pinned parser's `canonicalName` does not successfully look up every raw script
key: for example `Old_Italic` and `SignWriting` are refused. These owning
refusals are preserved rather than inferred from another regexp library's
Unicode property support. Runtime parsing uses static native facts, with no Go
compiler subprocess or alternate product policy.

The grammar matrix also retains all printable ASCII escape positions,
Unicode/control characters, repeat formatting and bound errors, named capture
errors, POSIX negation, unsupported extensions, quoted literal termination,
20,000 seeded patterns and selected deep capture/noncapture boundaries. Full
raw input and twin error/acceptance outcomes are retained. No diagnostic text
is normalized or replaced.

`provider-regexp-resource-gaps/summary.json` is explicitly **FAIL: 98/114**,
with **16 unwaived differences**. Seven are incorrect nesting refusals caused
by missing owning AST optimizations (literal/class concatenation, alternation
factoring/class merging and empty-branch collapse). Nine are missing size
refusals from Unicode class expansion and compiled-program size accounting.
The retained 114-case probe checks near-1,000 nesting heights, Unicode rune
budgets and repeated-program sizes. Inputs and complete raw Go/native outcomes
are retained. This is actionable failed evidence, never an accepted exception
or a milestone pass. The next implementation step is the full owned AST and
resource accounting; the current parser remains unwired.

Validation: **179 workspace tests passed, fifteen dedicated fixtures ignored**;
the differential driver explicitly invoked its fixture. Formatting, Clippy
with warnings denied, locked release build, all nine OpenSpec validations and
the complete pinned Go `regexp/syntax` suite passed. Generated-name, source and
raw evidence hashes were independently verified.
`provider-regexp-validation.json` records both the passing grammar scope and
the failed resource probe. `provider-regexp-secret-reuse.json` checks the exact
unchanged release, projection runtime sources and preceding 16-field, two-mode
secret outcomes before reuse.

`provider-regexp-cutover.log` remains **FAIL** on 76 inventory gaps and the
historical M5 secret projection source binding. Earlier graph and grammar
captures keep their original source hashes; no historical capture is relabeled
current. Regexp resource equivalence, full checker assembly and registry wiring
remain open. Public provider activation stays closed; M7–M13 remains active.

## Native regexp AST and private assembled compiler

The subsequent owned arena adds literal concatenation, the pinned four
alternation-factoring rounds, singleton/fold-pair rewrites, simple-class unions,
empty-branch collapse, capture/repeat structure and true tree-height/program-size
estimates. Unicode 15.0.0 class ranges, POSIX ranges and SimpleFold cycles are
reproduced from actual Go 1.25.4; both generated names and range/fold data must
match regeneration before the drivers run. The arena uses indices and flat
ownership to avoid recursive Rust destruction. This is schema admission syntax
work, not a regexp matcher or a new public capability.

`provider-regexp-ast-grammar/summary.json` retains **138,920/138,920** exact
comparisons. `provider-regexp-ast-resource-gaps/summary.json` retains
**446/447**, explicitly **FAIL**. The first 114 rows are the original resource
probe, now **114/114**: all sixteen original differences are fixed. The expanded
probe covers grouped Unicode budgets, factored nesting heights, repeated
program sizes, upstream invalid patterns and seeded near-height inputs. Its
single remaining difference is `(?:abcd|abce|abcf){1000}` repeated 1,000 times:
Go accepts it while the native true-size check reports “expression too large.”
The larger debug-native probe receives a 300-second process deadline; extending
that harness deadline neither changes parser limits nor waives an outcome.
Historical failed captures remain intact under their original names and hashes.

`provider-regexp-cache-diagnostic.json` records an independent actual Go public
AST/compile diagnostic. Its first repeated child is `(?:abc[d-f]){1000}`, with
an uncached estimate of 4,000 instructions; the 1,000-child concatenation has
an uncached pessimistic estimate of **4,000,000**, above the parser limit of
**3,355,443**, yet `regexp.Compile` succeeds. The diagnostic uses the pinned
`parse.go` size equations without changing the owning parser, and its source,
binary, toolchain and standard-library hashes are retained in
`provider-regexp-cache-diagnostic-provenance.json`. Go's lazy size map and
allocation reuse remain the next implementation boundary; this diagnostic does
not prove that native cache/reuse equivalence has been implemented.

`provider-schema-assembled/summary.json` retains **317,015/317,015** scoped
T0 comparisons (63,403 cases, five iterations) of the private native assembled
compiler against actual owning `validateJSONSchema`. It combines all 3,637
decoder cases, 4,198 default-base graph cases, and 27,784 grammar inputs in each
of `pattern` and `patternProperties`. Typed decoding, local checks, URI/anchor
registration, lexical/dynamic references and regexp syntax now run together.
Three flagged multi-fault map cases must match an entire actual Go outcome from
128 retained samples. No error substring replacement, synthetic acceptance or
resource waiver is applied. This compiler corpus excludes the large resource
probe; its green result cannot erase the separate failed resource evidence.
Registry construction still requires an explicit checker, and the assembled
checker remains private and unwired.

Validation: **179 workspace tests passed, sixteen dedicated fixtures ignored**;
the differential drivers explicitly invoked their fixtures. Formatting, Clippy
with warnings denied, locked release build, all nine OpenSpec validations and
the pinned Go `regexp/syntax` suite passed. Sources, input/output comparisons
and evidence indices were independently verified. The release artifact changed
to `e2e6e5c21a758219df0e7f25536c079ac91f36d521003d0733ce5b9578579ced`;
`provider-schema-assembled-secret-recheck/summary.json` therefore retains a
fresh **16-field, two-mode** secret sweep with **zero failures**, bound to this
artifact and the actual projection runtime sources. Earlier secret receipts
were not reused for this binary.

`provider-schema-assembled-validation.json` binds these results.
`provider-schema-assembled-cutover.log` remains **FAIL** on 76 inventory gaps
and the historical M5 secret source binding. The full manifest still selects
an earlier M1–M6 artifact, so the new sweep cannot be spliced into it; a complete
manifest refresh remains required before promotion. The unwaived regexp cache
mismatch, registry wiring and the remainder of M7–M13 stay open. Public provider
activation remains closed; no milestone exit is asserted by these T0 results.

## Native allocation reuse and lazy regexp resource caches

The private parser now uses one flat parse stack with explicit parenthesis and
alternation markers, an index-based LIFO allocation pool, and the owning lazy
size/height caches. Pool reuse deletes the height entry and retains the size
entry, matching actual Go. Cache activation uses the number of distinct
allocations and the global repeat product; checked roots are recalculated while
cached descendants keep Go's values. Factoring mutates existing allocations,
checks the owning suffix boundaries and recycles removed nodes in Go order.
Height and size walks use an explicit work stack instead of recursive Rust
ownership. These are compatibility mechanics, not new public parser policy.

`provider-regexp-cache-resources/summary.json` retains **447/447** exact
comparisons. It includes all 114 original resource cases and the subsequent
333-case expansion. The last retained mismatch, the accepted
`(?:abcd|abce|abcf){1000}` expression repeated 1,000 times, now receives Go's
acceptance through the generic allocator/cache mechanics. There is no
pattern-specific success branch or relaxed limit. Historical 98/114 and
446/447 failed captures remain intact with their original source hashes.

`provider-regexp-cache-controls/summary.json` retains **925/925** comparisons
(185 cases, five iterations). It varies factored prefix lengths, branch counts,
captures, fold flags, singleton classes, duplicate/empty branches, surrounding
literals and near-size repetition counts. It also compares literal-only
expressions around 3,355,443 runes: Go can leave its size cache inactive when
the allocation count and repeat product are small. Near-1,000 nesting cases
exercise the height cache and pool reuse. Outcomes are complete exact errors or
empty success; no diagnostic substitution or resource waiver is used.

The Unicode probe exposed repeated copying of a growing class vector in the
native merge. Class unions now append in place like Go, and preserve the
owning raw-range accounting before final cleanup. A superseded scratch producer
was explicitly stopped after identifying that copy behavior; its partial
outputs are not retained as a passing result. The final 447-case producer
completed successfully. Dedicated large debug-fixture processes receive a
600-second harness deadline; this does not change any product parser limit.

`provider-regexp-cache-grammar/summary.json` retains **138,920/138,920** exact
comparisons. `provider-schema-cache-assembled/summary.json` retains
**317,015/317,015** assembled compiler comparisons (63,403 cases, five
iterations), with the resource/caching corpora verified separately. These are
T0 component results. The schema checker remains private; the registry fixture
still uses its explicitly declared well-formed subset callback until the next
integration step. No runtime registry publication or provider activation is
claimed by these captures.

Validation: **179 workspace tests passed, sixteen dedicated fixtures ignored**;
the differential drivers explicitly invoked their fixtures. Formatting, Clippy
with warnings denied, locked release build, the complete pinned Go
`regexp/syntax` suite and all nine OpenSpec checks passed. Full source and raw
input/output indices were independently verified. Logs and receipts are bound
by `provider-regexp-cache-validation.json`.

The release remains byte-identical at
`e2e6e5c21a758219df0e7f25536c079ac91f36d521003d0733ce5b9578579ced`.
`provider-regexp-cache-secret-reuse.json` verifies that exact artifact, the
unchanged projection runtime sources and the complete retained raw outcomes
before reusing the preceding 16-field, two-mode secret sweep. The whole-agent
cutover verifier remains **FAIL** on 76 inventory gaps and the historical M5
source binding; the manifest still requires a complete refresh before
promotion. Registry integration, the fake/real-provider lifecycle matrix,
restart and in-flight replacement evidence, and M8–M13 remain open. Public
provider activation stays closed and the goal remains active.

## Native schema checker integrated into registry transactions

The registry differential fixture now passes the assembled native compiler
to its explicit schema-check dependency. The former reference-free subset
callback has been removed. Public provider activation is still closed.

`provider-registry-schema-enum-fixed/summary.json` retains **5,315/5,315**
exact comparisons (1,063 cases over five iterations). Malformed decoder
inputs, local and URI references, anchors, JSON pointers, regexp failures and
valid patterns exercise registration, explicit registration, upsert, generation
replacement, provider replacement and base validation. Replacement cases put
the candidate before and after another valid operation to check atomicity.
In addition to whole-output comparison, **3,690** rejection checks across both
implementations independently assert that the catalog and resolved descriptor
remain unchanged and that both old and newly resolved handles still invoke g1.
All prior handles are checked after accepted replacement as well. Raw inputs,
complete Go/Rust observations, source provenance and indices are retained.

The wider corpus exposed two invocation discrepancies. The registry fixture
now applies the existing plan validator to its legacy-adapter arguments,
matching the owning Go adapter. The native plan validator now uses Go's
string-only enum extraction and Go value formatting for enum errors. Numeric
and mixed-type enum arrays are ignored by the pinned legacy validator; this
is separate from schema compilation and is preserved for migration parity.
Empty and string enum arrays remain enforced. Regression cases cover each.

The initial diagnostic is retained in
`provider-registry-schema-initial-diagnostic/`; its final provenance assertion
failed after a comment changed during the run, so it makes no source-bound
pass claim. `provider-registry-schema-enum-gap/` retains the subsequent
**4,985/5,015** failing result and its original source bindings and raw errors.
Neither failure has been relabeled as passing evidence.

This remains T0 registry/compiler integration evidence. It does not establish
M7 provider process lifecycle or any M8 external effects. Next, connect
generation-owned service mounts and private candidate capabilities to catalog
publication, preserving one adapter owner, reverse disposal and the owning
Go dependency injection scope. Then validate fake-provider failures, in-flight
replacement, restart and all five unchanged Go providers before advancing to
M8 domain live checks in leased disposable runtimes.

The first full runtime regression retained **101/102** passing scenarios:
iteration two of `host-plan-fault-scripts.permanent-failure-with-compensation`
failed before Rust startup with `OSError: [Errno 98] Address already in use`.
The fixture probed and released a port before binding its HTTP stub. Stubs
now bind port zero and retain the kernel-allocated listener atomically;
external-agent sandbox port leasing is unchanged. The new parallel endpoint
test checks 32 independent scripted endpoints and released listeners. All
**141 harness tests** passed, and the affected scenario passed **20** fresh
Go/Rust repetitions. The failed runtime bundle remains unchanged in
`provider-registry-schema-runtime-regression/`.

The current release artifact is
`ec63341311ec3ad41e5b30e6c5707a965bd95a7e42670f420e800ee0594323ab`.
Fresh `provider-registry-schema-secret-fixed/` evidence covers all sixteen
marked fields in both modes with no failures, and uses the revised harness.
The earlier fresh secret sweep is retained as historical evidence. The global
cutover verifier still fails on 76 inventory gaps and stale historical
manifest provenance, including the prior harness fingerprint. This private
registry integration does not refresh or promote the global manifest.

The full rerun in `provider-registry-schema-runtime-fixed/` passes
**102/102 scenarios over five iterations** with no producer change during
the run. Formatting, warnings-denied Clippy, locked release build,
**179 workspace tests** (sixteen explicitly driven fixture tests ignored by
the ordinary suite), and all **nine** OpenSpec checks passed.
`provider-registry-schema-validation.json` binds the current source hashes,
release artifact, logs and independently verified registry, runtime,
compensation and secret captures. M7 and M8 remain incomplete.
