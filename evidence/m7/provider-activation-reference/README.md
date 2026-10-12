# Owning Go activation reference

Eight reference checks capture the pinned Go activation helper through the actual server constructor, isolated temporary durable state, and a fiber-owned event/admission listener. Both standalone and platform projections are included. This bundle contains no Rust comparison and does not close M7, provider invocation, transport, process supervision, or full recipe cleanup gates.

The checks cover a metadata no-op, activation with no adapter followed by an already-active retry, admission denial, and a closed-state-store failure during readiness persistence. Raw inputs and observations are retained with source and binary hashes in `summary.json` and `index.json`. The production Go checkout remains clean; the helper is a build-only overlay.

The no-adapter case records an active lifecycle state and durable row. That observation does not establish that the generation can execute a provider operation. Admission denial emits ready and admission events, changes the in-memory generation to failed, and leaves the durable row ready inside this helper; the outer recipe's candidate cleanup is outside this fixture. Failure to persist readiness prevents both event emission and admission.

Next integration must retain this ordering, publish before lifecycle activation, persist draining and active rows before mounting, preserve the predecessor until replacement success, and execute the enclosing cleanup and rollback paths. It must then validate actual adapters and all five unchanged Go provider processes through public MCP.
