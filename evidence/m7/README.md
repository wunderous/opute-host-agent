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

## Generation-scoped service mounts and cancellation-safe ownership

`provider/mounts.rs` now owns provider service values, immutable dependency
bindings, generation-specific lookups and the shared adapter's disposal. It
is private to the unactivated provider composition; no public provider
installation or dispatch is enabled. Dependency families are drawn only from
the mounting generation and the active generation of each provider, exactly
as the pinned Go `provider_service_plugin.go` does. Ambiguous or missing
families reject the mount. Earlier services from the failed attempt are
unmounted in reverse order; an earlier generation remains intact. The first
service owns closing the generation's adapter, so normal reverse teardown
closes that adapter last. Displacement and explicit generation retirement use
the same owned mounts; no second service map is cleared afterwards.

`provider-service-mounts-bound/summary.json` retains **385/385** exact
comparisons (77 cases, five iterations) against the actual owning Go mount,
lookup, dependency, displacement and Cordis disposal functions. The build-only
overlay adds an observation bridge while leaving the pinned tree unchanged.
It exercises nil and empty generations/services/adapters, early and late
dependency failures, duplicate mounts, ambiguity, replacement while the prior
generation is active, exclusion of draining generations, restored generations,
ordered partial disposal, repeated teardown, family displacement and opaque
keys containing whitespace, Unicode, slashes and `@`. Complete observations
compare mount order, service keys, offered identities, exact dependency keys
and each adapter's closed-state refusal. Every case ends with empty services
and mounts and an identical repeated-close observation. This is T0 evidence
with unconnected adapters, not a provider process or public activation gate.
The preceding `provider-service-mounts-initial/` capture retains the successful
pre-cancellation-hardening component result with its own historical source
bindings.

The new blocked-call test mounts two services sharing a real adapter HTTP
client, accepts a request on a loopback socket, cancels the teardown caller
while that request is held by an explicit barrier, then releases it. The
accepted request completes on its retained service/adapter; the owning cleanup
task closes the adapter and removes every mount, and later calls on the
retained handle fail closed. This matches Go's background disposal ownership
while avoiding an async cancellation leaving a native mount permanently
marked disposed. The cleanup task only owns disposal; it is not a workflow
executor. Broader timeout, task cancellation, callback and provider process
behavior still require the M7 composition and E2E matrix.

The changed oracle producer was rebuilt, and
`provider-service-mounts-registry-recheck/` passes **5,315/5,315** whole
registry comparisons with current source bindings. **180 workspace tests**
passed (seventeen dedicated driver fixtures ignored by the ordinary suite);
formatting, warnings-denied Clippy, locked release build and all nine OpenSpec
checks passed. Five repetitions of the owning Go mount, dependency, generation
swap and disposal tests also passed. The release remains byte-identical at
`ec63341311ec3ad41e5b30e6c5707a965bd95a7e42670f420e800ee0594323ab`;
the preceding 102-scenario runtime suite and sixteen-field secret sweep can
be reused only after checking their unchanged artifact, harness and raw
evidence bindings. M7 and M8 remain incomplete and the global cutover gate
still fails on inventory gaps and historical manifest provenance. Next is
private candidate capability composition with catalog publication, then the
fake-provider and all-five-provider process lifecycle matrix.

`provider-service-mounts-validation.json` independently verifies all current
mount/registry raw observations, source and raw-file indices, cleanup
invariants and reuse of the byte-identical public runtime and secret sweep.
The twelve selected owning Go tests passed all five repetitions (sixty test
passes). No runtime lease is needed for these in-memory/loopback provider
checks; no shared Incus, Kubernetes, tunnel or deployment state was mutated.

## Private candidate lookup and catalog ownership

The native candidate owner now retains generation-keyed manifests and adapter
entries separately. Snapshot lookup refuses blank generation IDs and requires
both entries; descriptor lookup uses only the manifest and matches the tool
name exactly. This preserves the owning Go distinction between an absent
adapter entry and an entry containing a null adapter. A null entry can project
a private snapshot, but supplies no executable adapter. Generation keys are
opaque, and removal revokes new lookups while retained immutable snapshots
remain usable. Public activation and invocation are still unconnected.

`provider-candidate-lookup/` passes **2,710/2,710** exact comparisons across
five repetitions, including platform and standalone catalogs, missing entries,
null adapter presence, whitespace and Unicode/slash/@ generation IDs, exact
names, duplicate operations, replacement edges, and descriptor metadata. The
build-only bridge calls the owning Go candidate snapshot and capability lookup
functions; the descriptor is taken from the returned capability's Definition.
No candidate operation is executed by this differential fixture. The driver
now removes duplicate queries and uses faster lossless raw-output compression.
The initial corpus passed 3,190 comparisons before the final test-only Clippy
repair and query deduplication; that diagnostic remains under `.parity/m7/`.

The rebuilt oracle and native test binary also pass the complete registry
**5,315/5,315** and service-mount **385/385** rechecks with current bindings.
**181 workspace tests** passed, with seventeen dedicated differential fixtures
ignored by the ordinary suite. Formatting, warnings-denied Clippy, the locked
release build and all nine OpenSpec checks passed. The two owning Go candidate
projection/recipe-dispatch tests passed five repetitions using local fake HTTP
providers; those are reference-side tests, not native execution validation.
The public release remains byte-identical at
`ec63341311ec3ad41e5b30e6c5707a965bd95a7e42670f420e800ee0594323ab`.

`provider-candidate-validation.json` independently checks every current raw
comparison, source hash, evidence index and binary binding. Build and check
logs are in `provider-candidate-checks/`. Raw captures were recompressed for
repository storage with every uncompressed byte hash verified before and
after; `provider-candidate-checks/archive.json` records those hashes. Cutover still fails on 76 inventory
gaps and fourteen historical evidence provenance problems. M7 and M8 remain
incomplete. The next boundary is executable candidate/active capability
binding with generation sessions and signed resource delegation through the
existing admission and task owners, followed by publication/rollback and the
fake-provider and five unchanged Go-provider process lifecycle matrix. The
owning activation function restores a prior manifest after publication failure
and explicitly rolls back failures after lifecycle activation; registry
replacement alone does not establish that composition behavior. No shared
Incus, Kubernetes, tunnel or deployment state was mutated.

## Executable generation/session binding

The private provider binding now projects immutable operation descriptors and
retains the exact accepted lifecycle session and adapter. Candidate calls open
a session for their explicit generation before resolving a candidate adapter.
Active calls open the current session, refuse an immutable descriptor whose
generation is superseded, then resolve the generation-owned service adapter.
Every disconnected, refused, completed or cancelled path releases its session.
The accepted HTTP call uses `CallSynchronousOnly` with unchanged arguments and
never resolves a replacement adapter after acceptance. Resource delegation
must be issued by the composition owner before this boundary is entered.
This module neither authorizes nor publishes a capability.

`provider-operation-binding/` passes **560/560** comparisons over five
repetitions. It includes all prior service-mount cases plus candidate/active
binding against each restored generation state, missing and cross-provider
identities, absent/null adapters, closed retained candidate connections,
superseded descriptors, and whitespace/Unicode/slash/@ generations. The
build-only oracle invokes the actual owning Go capability and compares exact
refusal text, tool-error versus call-error behavior, session counts and mount
observations. These calls use an unreserved background context, where owning
Go delegation is a no-op. Complete MCP error envelopes, signed delegation and
admission are not established by these comparisons. The independent verifier
checks every observed invocation leaves zero active sessions and each fixture
finishes with no mounted services.

Two native loopback HTTP tests use explicit request/release barriers. A blocked
accepted g1 operation remains on g1 while g2 activates; a stale g1 descriptor
refuses new work, and a fresh operation returns g2's result. Cancelling the
teardown caller while the g1 request is accepted does not abandon its cleanup
owner; after release, g1 completes, its session closes, and its mount disappears.
A separate cancellation test aborts an accepted HTTP call and verifies the
session is released, disposal completes, and retained adapter calls refuse.
Both tests verify arguments at the fake provider and join every provider
thread. These are private binding/adapter tests, not the full provider process
or public Host Agent dispatch E2E matrix.

The rebuilt producer also passes **5,315/5,315** registry and **2,710/2,710**
private candidate rechecks with current binary/source bindings. The workspace
passes **183 tests** with seventeen dedicated differential fixtures ignored;
formatting, warnings-denied Clippy, locked release build and all nine OpenSpec
checks pass. Eight selected owning Go retirement, catalog refresh, candidate
dispatch and callback delegation/task-owner tests pass five repetitions
(forty reference-side passes). The public release remains byte-identical at
`ec63341311ec3ad41e5b30e6c5707a965bd95a7e42670f420e800ee0594323ab`.

`provider-binding-validation.json` independently verifies full raw equality,
source/binary hashes and indices; `provider-binding-checks/` retains check logs
and lossless archival byte hashes. Cutover still fails on 76 inventory gaps
and fourteen historical provenance problems. M7/M8 remain incomplete. Next is
signed callback delegation and the provider capability output-schema/
observation hook, then ordinary dispatch composition, publication/rollback,
and the fake-provider plus five unchanged Go-provider lifecycle E2E matrix.
No shared Incus, Kubernetes, tunnel or deployment state was mutated.

## Provider output validation and shared Go regexp execution

The private provider operation now exposes the owning `ValidateResult` hook.
It refuses nil results through the outer capability guard, validates successful
structured results against the declared output schema, and encodes the exact
pass-through observation. Error results bypass output validation and retain
`status=error` and `retryability=capability`. Content, resource links and
manifest generation identity do not synthesize resources, facts, catalog
revisions or semantic completion. The hook consumes SDK-projected JSON results;
programmatic Go values that cannot occur on the JSON provider wire are outside
this fixture. Public provider composition and signed delegation remain open.

That hook exposed an existing mismatch between the two native value validators.
The plan runner now delegates to the same Go-compatible validator used by the
ordinary capability boundary. This removes divergent type/const/property/minimum
error formatting and equality behavior. The initial owning-hook corpus found
ninety integer refusal mismatches: Go shadows the original value with the
`number(value)` conversion, so even a non-number reports `got float64`. The
native validator now preserves that behavior. The full historical failure
capture is retained in `provider-observation-integer-diagnostic/`; its original
validator source is `provider-observation-integer-before.rs`. Historical source
bindings must not be interpreted as current evidence. The intermediate smaller
corpus in `provider-observation-bound/` passed 11,805 comparisons before seeded
pattern expansion and the final layout repair; current evidence is the final
expanded bundle.

Boolean pattern execution now begins with the pinned native Go parser and
Unicode 15 tables. Its owned AST is emitted iteratively into a normalized
expression: literal folding and class ranges come from the pinned data, word
boundaries use Go's ASCII definition, anchors preserve Go's line/text semantics,
and capture names and greediness are omitted only because the caller requests a
boolean match. Surrogate-only ranges cannot match valid input scalars. Original
Go patterns never pass through Rust syntax or Rust Unicode class/folding rules.
Rust's regex engine executes the normalized language with compilation size and
nesting settings that avoid adding its default admission limits; the owning
Go parser retains its original resource checks. Catalog compilation still uses
syntax admission alone. This is a pattern engine boundary, not a plan executor.

`provider-observation-final/` passes **67,335/67,335** exact owning Go
`Capability.ValidateResult` comparisons over five repetitions. It covers
primitive/union types, enum/const, required and nested properties, arrays,
byte lengths and numeric minima, nil and error results, opaque observations,
ASCII boundaries, quoting, named captures, Unicode categories/scripts and
simple folding, seeded expression combinations and large/deep/repeated patterns.
The existing syntax corpus remains green at **138,920/138,920**, and all
**447** resource-limit cases pass. Registry **5,315/5,315**, generation binding
and mounts **560/560**, and private candidates **2,710/2,710** pass with current
source/binary bindings. The workspace passes **183 tests** with eighteen
explicit differential fixtures ignored by the ordinary suite; formatting,
warnings-denied Clippy, locked release build and all nine OpenSpec checks pass.

The changed release artifact is
`f7c537fed070c1b242506110dad90bbb82548839602e816313f73f1698d876b8`.
Fresh public-runtime evidence passes **102/102 scenarios over five iterations**,
and a fresh sixteen-field secret sweep across standalone/platform modes has
**zero failures**. These establish regression coverage for the existing public
runtime; they do not establish the still-unconnected provider lifecycle E2E.
`provider-observation-validation.json` independently verifies every component
raw comparison, source and binary hash, compressed-file index, runtime raw
normalization/invariants/declared divergences, iteration records, and secret
outcomes. Check logs and lossless archival byte hashes are retained in
`provider-observation-checks/`. Global cutover still fails on 76 inventory gaps
and fourteen historical provenance problems; the old global manifest was not
spliced with fresh component evidence. The full M7–M13 goal remains open.
Next is signed resource delegation and callback/task-owner composition, followed
by publication/rollback and the fake-provider/five-Go-provider process matrix.
No shared Incus, Kubernetes, tunnel or deployment state was mutated.

## Signed delegation verification and permit lifetime

The private provider delegation owner preserves the pinned
`hostmcp/resource_delegation.go` HMAC-SHA256 verification order, process-local
active-token membership, claim completeness/expiry, and exact Go claim JSON
encoding. It uses the already locked ring 0.17.14 cryptographic implementation
for signing and constant-time MAC verification. Go raw URL base64 decoding
permits CR/LF and noncanonical trailing bits; the native decoder preserves those
rules while active membership still requires the exact issued token string.
Go JSON duplicate fields, null handling, case folding, integer decoding and
invalid UTF-8 replacement are retained through the existing native Go decoder.
A non-cloneable permit revokes the token when its operation releases ownership,
including when an accepted asynchronous operation is aborted.

`provider-delegation-bound/` compares **811 cases over five iterations** with
the actual pinned Go server verifier, using a build-only bridge and fixture-owned
keys/maps. All **4,055 comparisons** pass with exact errors and encoded claim
bytes. This covers malformed framing/signatures/payloads, incomplete and expired
claims, inactive or replaced tokens, key changes, duplicate/case-folded/null
fields, Unicode, raw JSON errors, signed alternate base64 representations,
CR/LF, arbitrary key lengths and seeded claim mutations. The pinned checkout
remains unchanged. Dedicated lifetime tests prove drop/abort revocation and
refusal after process-key replacement; the owning Go callback/delegation tests
also pass **25 executions**.

This checkpoint implements the cryptographic owner only. It does not claim
validated reservation/task issuance, callback tool scope, child task inheritance,
generation-bound admission, public provider composition, or full M7–M13 E2E.
Those remain required before promotion. No shared runtime was mutated.

The workspace passes **185 tests**, with nineteen dedicated differential
fixtures ignored by the ordinary suite; formatting, warnings-denied Clippy,
locked release build and all nine OpenSpec checks pass. The changed public
release is `9ef6471ac9cfe6bc923ecc0e55544481bce859f3a73180930ddb4bd1ae018098`.
Fresh runtime regression passes **102/102 scenarios over five iterations** and
the sixteen-field secret sweep has **zero failures** in standalone/platform
modes. `provider-delegation-validation.json`, reproduced by
`provider-delegation-checks/verify.py`, independently checks every component raw
array, source/binary/file-index hash, public raw comparison and iteration record,
and secret outcome/provenance. Global cutover still fails on 76 inventory gaps
and fourteen historical provenance problems; its old manifest is preserved.
Next is typed reservation/task ownership for delegation issuance and callback
scope/child execution, followed by provider publication and process E2E.

## Typed delegation issuance, callback ownership and bound invocation

The native delegation owner now issues a token only after the independent
context task/operation matches the reservation, the reservation belongs to this
agent, and the durable task registry reports active work. Empty/control/unmanaged
reservations retain the original context without issuance. Opaque provider,
generation and operation strings are preserved; only the pinned context helpers'
identity and metadata trimming is applied. `RunCtx` carries these typed values
through cancellation children, and reservation handles retain their creation and
expiry times without changing ledger serialization or admission defaults.

Callback verification checks active signed claims before tool scope, then task
liveness. Scope uses the owning host tool definitions and capability descriptor
implementation, preserving the Go refusal of lifecycle, provider and unknown
callback targets. The task-owner bridge validates owner agreement and expiry;
a durable child copies only that typed resource owner into its independent
context. Its registry identity, cancellation and callback metadata stay its own.
`Operation::invoke` composes issuance before generation acceptance and retains
both the delegation permit and immutable session throughout the provider call.
Admission/authentication remain the caller's responsibility. Refusals retain a
separate error currency from provider transport/protocol failures, matching Go's
error tool result versus invocation-error distinction.

Current, provenance-bound T0 comparisons pass **3,570 issuance cases**,
**4,445 callback/task-owner cases**, **4,055 verifier cases**, and **560 generation
binding/mount cases** (all totals include five iterations). Issuance random UUIDs
and real-clock expiry are normalized only after independent UUID-v4, one-hour
TTL and HMAC checks; every other field/error/context outcome is exact. Native
issued tokens also pass the actual Go verifier. Callback comparison is exact,
including reserved metadata typing/trimming, implementation scope, task states,
owner matching, parent/child cancellation isolation and extreme signed expiry.

The first callback matrix found five repeat mismatches at `int64::MAX`: Go's
year-1 epoch shift wraps and `Time.Before` treats the reservation as expired.
`provider-callback-expiry-diagnostic/` and
`provider-callback-expiry-before.rs` retain the failed raw capture and its
causative native source. Native comparison now preserves that behavior, with
additional cases immediately around the wrapping boundary. Those historical
failures are diagnostic evidence, not current parity evidence.

A real loopback provider test proves reserved wire metadata, unchanged arguments,
live generation-bound claims, callback ownership, independent child task identity,
parent-task completion refusal, and token/session release on normal completion
and abort. Failed generation acceptance also releases the token issued before
it. The workspace passes **186 tests** with twenty-one dedicated fixtures ignored
by the ordinary suite; formatting, warnings-denied Clippy, locked release build,
all nine OpenSpec checks and 25 owning Go test executions pass.

The current release is
`668a2c92b28a179e6eface6b9a8874881e16189c87f3791649821de57665a2e7`.
Fresh existing-public-runtime regression passes **102/102 scenarios over five
iterations**, and the sixteen-field secret sweep across standalone/platform has
**zero failures**. `provider-owner-validation.json`, reproduced by
`provider-owner-checks/verify.py`, independently checks all raw component arrays,
UUID/TTL/signature invariants, cross-verification captures, source/binary/index
hashes, public raw comparisons and iteration records, and secret outcomes.
The final private invocation error-type refinement leaves this public release
byte-identical; source-bound component comparisons were rerun for the final code.

The full M7–M13 goal remains open. Public authenticated callback dispatch and
admission composition, provider publication/rollback, real process/provider
matrix and later domain E2E are still required. Global cutover remains failed on
76 inventory gaps and fourteen historical provenance problems; the global
manifest was not spliced with this component evidence. No shared Incus,
Kubernetes, tunnel or deployment runtime was mutated. Next is the provider
publication/rollback transaction and durable runtime wiring.

## Provider publication ownership and activation reference

`provider::publication::Publication` now composes native schema compilation,
generation-bound executable descriptor construction, family displacement,
publication, retirement, and restoration through the active generation.
The composition owner supplies trusted executable and MCP refresh factories.
Retained capability handles and catalog snapshots remain immutable across
replacement and retirement. The public native provider runtime is still closed.

The owning Go `registerProviderServicesForGeneration`,
`retireProviderCapabilities`, and `restoreProviderManifest` functions are
captured alongside registry state, published catalogs, executable descriptors,
remembered manifests, and actual SDK `tools/list`. **242 cases over five
iterations pass, totaling 1,210 comparisons.** The raw batches are in
`provider-publication-parallel/`. Comparison uses parsed JSON structural equality
without value normalization; it does not require JSON object member order or
literal transport byte identity. Cases cover both modes, malformed schemas,
competing families, generation/provider identity, replacement, empty services,
restore refusal, exact retirement, retained snapshots/handles, and seeded
multi-step sequences. Invocation is outside this metadata fixture.

The first matrix found four empty-service mismatches. Go keeps the old registry
entries on empty generation replacement and recreates their missing executable
bindings through its legacy MCP composition hook. Native refresh now uses
`Publication::ensure_legacy` for that same ownership boundary. The failed batch
and causative native source remain in
`provider-publication-diagnostic/empty-services/`; they are historical diagnostics.
The final matrix includes all four corrected cases.

The matrix also confirms a pinned Go failure: competing-family displacement
occurs before replacement schema validation. On failure, registry state loses
the old provider while the published catalog and MCP still retain it. A later
successful unrelated publication removes the old tool from those projections.
`provider-publication-diagnostic/family-displacement/` retains the raw sequence
and an independent verifier. Rust currently preserves the pinned ordering.
The separate `atomic-provider-family-publication` proposal is pending owner
review; this checkpoint does not claim failure-preserving activation or waive
the M7 exit gate.

Fresh source-bound ownership regressions pass **3,570 issuance**, **4,445
callback/task-owner**, **4,055 cryptographic verification**, and **560
binding/mount** comparisons. Issuance retains independent UUID, real-clock TTL,
HMAC, and actual Go cross-verification checks. The workspace passes **186 tests**
with twenty-two dedicated differential fixtures ignored by the ordinary suite.
Formatting, warnings-denied Clippy, locked release build, **20** owning Go test
executions, and **10** OpenSpec checks pass. The release remains byte-identical
to `668a2c92b28a179e6eface6b9a8874881e16189c87f3791649821de57665a2e7`.
The previous checkpoint's **102 public scenarios over five iterations** and
**sixteen-field secret sweep** are reused only after independent verification
of unchanged binary/harness hashes, raw comparisons, iteration records, and
secret outcomes. They do not validate public provider dispatch.

`provider-publication-checks/verify.py` reproduces
`provider-publication-validation.json`, checks every current raw batch and its
source/binary/file index, rechecks the refreshed owner captures and public
regressions, and verifies the clean pinned Go tree. Earlier component receipts
remain evidence for their own commits; this receipt binds the current checkpoint.
The original global manifest is preserved and cutover remains failed.

A separate build-only activation oracle uses the actual pinned Go server
constructor, independent temporary durable state, and a fiber-owned event and
admission listener. Its **eight reference checks** cover both modes, no-op and
already-active metadata, admission denial, and readiness persistence failure.
`provider-activation-reference/` preserves inputs, observations, source/binary
hashes, and the explicit scope. This is Go reference evidence, with no native
parity, adapter execution, outer recipe cleanup, or process E2E claim.

The M7–M13 goal remains active. Next is activation/rollback and durable
composition, followed by public admission/callback routing and the unchanged
provider process matrix. M8 domain E2E remains open. No shared Incus, Kubernetes,
tunnel, or deployment state was mutated by this checkpoint.

## Owned activation and durable rollback composition

The private `provider::activation::Activation` owner now follows Go's
`activateProviderGeneration` ordering: readiness and its durable row precede
admission; publication precedes lifecycle activation; draining and active rows
precede mounting; the predecessor is disposed last. Already-active retries
reaffirm publication and mounting without failing the live generation. The full
declaration is remembered separately from the recipe-filtered publication.
Rollback disposes the replacement, restores the predecessor's lifecycle and
durable row, then restores publication, preserving Go's combined error currency.
Candidate cleanup removes its connected ownership and persists the failed
observation. These are explicit lifecycle transitions, using the existing plan
runner rather than introducing another workflow executor.

Activation and cleanup tasks retain their owner if an awaiting caller is dropped.
A focused test uses the existing single-thread Tokio runtime, blocks the ready
announcement, aborts the caller, and verifies that the owner completes admission,
publication, durable activation and candidate removal. It establishes cancellation
ownership for this private composition; its provider has no adapter.

The source-bound owning Go/native metadata matrix passes **170 cases over five
iterations, totaling 850 comparisons**, in standalone and platform projections.
It covers lifecycle states, metadata typing, service filtering and precedence,
replacement, admission denial, schema/publication failure, rollback refusal,
live-session refusal and closed-store rollback. Both sides use independent
temporary durable state. Only fresh event and activation times are normalized,
after clock bounds and generation/durable consistency checks; all other parsed
JSON values are compared directly. Raw inputs, outputs, clocks, source/binary
hashes and file indexes are in `provider-activation/`.

The closed-store reference exposed a native provider-generation persistence
error-text gap. Its save/list methods now return Go's `sql: database is closed`.
The initial broader verifier also assumed a zero activation time had the same
JSON and durable representations. Go intentionally serializes zero time in a
generation but stores an empty activation-time string. The corrected verifier
retains that conversion and still checks nonzero time consistency. The historical
verifier and raw observation are preserved in
`provider-activation-diagnostic/zero-time/`; they do not establish a runtime
mismatch. The final 850 comparisons bind the final native source and test binary.

`provider-activation-checks/verify.py` reproduces
`provider-activation-validation.json`, independently checks time normalization,
recomputes each raw pair, and verifies source/binary/index hashes and the clean
pinned Go tree. The workspace passes **187 tests** with twenty-three dedicated
fixtures ignored by the ordinary suite. Formatting, warnings-denied Clippy,
locked release build, **30** owning Go test executions and **10** OpenSpec checks
pass. The public release remains byte-identical to
`668a2c92b28a179e6eface6b9a8874881e16189c87f3791649821de57665a2e7`.
The existing 102 public scenarios over five iterations and sixteen-field secret
sweep are reused only after rechecking exact current binary/harness identity,
raw observations, iteration records and secret outcomes. They do not establish
public provider activation.

This differential corpus contains **no adapters**. Actual adapter invocation,
public authenticated admission/callback composition, enclosing recipe cleanup,
process supervision, restart, all five unchanged provider processes and full
M7–M13 E2E remain open. The competing-family atomic-publication proposal is still
pending owner review; the pinned failure ordering remains preserved. Global
cutover is failed and its original manifest is untouched. No shared Incus,
Kubernetes, tunnel or deployment state was mutated. Next is actual adapter/mount
failure and public lifecycle composition, then the process/provider matrix.

## Connected activation and mount ownership

The activation differential fixture now connects actual Go and native adapters
to exclusively owned loopback MCP endpoints. The connected corpus covers
**24 cases × 5 iterations = 120 passing comparisons**, including dependency
chains, failure before and after the first mount, successful and failed
replacement, active-generation reaffirmation, admission/store/filter failures,
completion before activation and explicit candidate cleanup. The complete
observations include ordered service mounts, resolved dependency keys and
candidate adapter ownership. Synchronous connection probes establish that a
failed replacement keeps the prior adapter usable and a successful replacement
closes the predecessor. They do not establish public admitted dispatch.

The independent verifier in `provider-activation-connected-checks/verify.py`
checks raw comparisons, actual request payloads/protocol headers, typed loopback
endpoint substitution, fresh timestamp bounds, durable consistency and explicit
mount/rollback outcomes. It produces `provider-activation-connected-validation.json`.
The workspace again passes **187 tests**, warnings-denied Clippy, formatting,
locked release build, focused pinned Go mount/swap/idempotence tests over five
iterations and **10** OpenSpec checks. The unchanged public release bytes permit
rechecking the prior 102 public scenarios and sixteen-field secret sweep.
Historical private receipts remain bound to their recorded source versions.

No production runtime source changed in this checkpoint: native changes are
inside the existing ignored differential fixture. Public lifecycle composition,
enclosing recipe cleanup, process supervision, restart and all five unchanged
provider executables still require validation. The whole-agent gate remains
failed with 76 inventory gaps and 14 historical provenance/evidence problems;
its original manifest is preserved. No shared runtime was mutated.

## Shared public catalog publication

The composed server now owns an `Arc<Publication>` instead of a static catalog
and startup-only tool vector. Admission, binding, plan/recipe validation,
audit/redaction and tool-name readers take immutable snapshots from that owner.
MCP tool listing and registered-name lookup use the separate last-completed
publication snapshot. This preserves the pinned distinction between catalog
state and the SDK listing after failed displacement followed by no-op retirement.
The initial wiring draft's wire-only mismatch is preserved in
`public-catalog-diagnostic/noop-retire`, including a reconstructible source patch.

`public-catalog-publication` compares **32 cases × 5 iterations = 160 passes**.
Every native transition crosses the authenticated HTTP `tools/list` route and
is compared against actual Go SDK listing; observations and errors remain exact
parsed JSON. The corpus includes replacement, retained handles and snapshots,
family displacement/schema failures, empty services, restore, retirement and
explicit stale-list regression checks. Public provider invocation is outside
this corpus.

The changed public release (`2b110669e98d55ed53f048924ed76a670cd2809ea811bedd02f8282cdfcb43b6`)
has fresh captures: **102/102 public scenarios over five iterations**, and
**16 marked secret fields with zero failures**. The independent verifier in
`public-catalog-checks/verify.py` reproduces `public-catalog-validation.json`,
checks all current source/binary/capture hashes, recomputes the raw comparisons
and confirms the stale-list behavior. The workspace passes **187 tests**,
formatting, warnings-denied Clippy, locked release build, **15** focused owning
Go test executions and **10** OpenSpec checks.

Public provider lifecycle commands, activation/recipe composition, generation-
bound provider dispatch, supervised processes, restart and full M7–M13 E2E remain
open. No production provider seam or cutover is claimed. The whole-agent gate
still fails with 76 inventory gaps and 14 historical provenance problems; its
original manifest is preserved. No shared runtime was mutated.

## Server-owned provider runtime, status and validation

`provider::runtime::Runtime` is now composed into the server beside the shared
publication owner. It owns the lifecycle, candidates, mounts, activation and
delegation owners for one serving process, binds once to the existing serving
Tokio runtime, and gives activated operations immutable capability factories
that invoke through the retained generation session and adapter. Ordinary
admission and the single plan runner keep execution ownership; no provider
workflow executor is introduced. Shutdown disposes mounts, then candidate
adapters, inside an owned cleanup task while the serving runtime still drives
accepted provider I/O. A focused test holds an accepted factory call on a real
loopback socket, shows that shutdown waits for it, and then proves later calls
on the retained handle fail closed with zero sessions remaining.

`opute.provider.status` and `opute.provider.validate` now dispatch to this
owner. `provider-runtime/summary.json` records **120/120 exact comparisons**
(24 cases × 5 iterations, both modes) against the actual pinned Go server
constructor and SDK lifecycle admission, through native authenticated HTTP.
Cases cover absent, blank, padded and case-variant provider IDs, unconnected
and connected generations, expected-endpoint matching, remembered and explicit
validation operations, missing validation declarations, retained handles
across a failed predecessor and replacement, and failed mounted generations.
Provider request payloads and protocol headers are compared exactly per batch;
no result normalization is applied. Trusted setup restores and activates the
generations; public installation and ordinary provider capability dispatch
are not exercised by this corpus.

`provider-runtime-diagnostic/header-name/` retains an earlier fixture error:
its client passed a blank `tools/call` name and native correctly refused the
header mismatch before dispatch. The fixture was corrected; production header
validation did not change. It is neither a runtime divergence nor a pass.

The changed public release (`cf583927c7fe869d961578d99b53d87c0829cd1482645a3b41b2af7dd9f656d6`)
has fresh captures: **102/102 public scenarios over five iterations** in
`provider-runtime-public/` and a **16-field, two-mode secret sweep with zero
failures** in `provider-runtime-secret/`. `provider-runtime-checks/verify.py`
reproduces `provider-runtime-validation.json`: it rechecks source and binary
provenance, every raw batch and request trace, the public scenario diffs and
secret outcomes, and the clean pinned Go tree. The workspace passes
**188 tests** (24 dedicated fixtures ignored), with formatting,
warnings-denied Clippy, locked release build, **25** owning Go test executions
and **10** OpenSpec checks.

Public provider lifecycle commands, generation-bound ordinary dispatch,
callback routing, process supervision, restart and the provider matrix remain
open. The whole-agent gate still fails with 76 inventory gaps and 14 historical
provenance problems; its original manifest is preserved. No shared runtime
was mutated.
