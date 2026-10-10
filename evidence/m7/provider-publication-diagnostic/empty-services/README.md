# Empty declarations retain catalog entries through Go's legacy composition

This historical twelve-case batch contains the four `empty-services-None` and `empty-services-[]` cases in standalone and platform projections. The failed native fixture and its publication source are retained alongside the Go observations. They are diagnostics, not a current parity receipt.

After an existing provider publishes an operation, replacing its generation with an empty service declaration keeps the old registry entries under the pinned `ReplaceGeneration` behavior. Publication removes the old executable binding, then Go's MCP refresh recreates a missing executable from the catalog through `ensureLegacyCapabilityForDescriptor`. The initial Rust composition fixture omitted that hook, so its executable metadata map differed while catalog and wire projections agreed.

`Publication::ensure_legacy` now exposes the corresponding trusted composition boundary. The publication fixture invokes it during refresh. The full current matrix rechecks all four cases; metadata equality does not establish that a legacy wrapper and a generation-bound provider invocation execute identically. Public dispatch remains an M7 gate.
