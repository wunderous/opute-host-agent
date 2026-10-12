# Failed competing-family publication

This historical capture calls the pinned Go publication owner and observes actual SDK `tools/list`. It is a failure diagnostic, not current Rust parity or full provider activation evidence. The entire twelve-case batch is retained; `receipt.json` identifies the failing sequence and binds captures, producer sources, source lock, and binary. Run `python3 evidence/m7/provider-publication-diagnostic/family-displacement/verify.py` to inspect the failure independently.

| Step | Registry | Published catalog and actual MCP listing |
| --- | --- | --- |
| Alpha publishes `test.probe` in `test.family` | Alpha present | Alpha present |
| Beta competes for `test.family` with an invalid input schema | Alpha absent; Beta rejected | Alpha still present |
| Observe after rejection | Alpha absent | Alpha still present |
| Gamma publishes `third.probe` in `other.family` | Gamma present; Alpha absent | Gamma present; Alpha absent |

The error is `capability "other.probe" input schema must declare type object`. Go displaces the other family's registry owner before validating the candidate. A later successful refresh exposes that earlier displacement. The candidate's own predecessor restoration cannot restore a different provider's lost registry entries.

When this was captured, Rust preserved this ordering. Owner decision D16 (2026-10-12) has since made publication atomic in Rust, and `openspec/changes/atomic-provider-family-publication/` records the change. The `rust.json.gz` capture here is the pre-D16 Rust and is kept as it was. Current Go/Rust evidence for the decided behaviour comes from the public `provider-lifecycle.family.*` scenarios.
