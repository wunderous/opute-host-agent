# Superseded by decision D18

This capture recorded `callback.child-task-unacknowledged-at-shutdown` as an
open divergence while the owner decision was pending. On 2026-10-12 the
owner decided D18: extend D12, so Rust keeps nothing durable for a delegated
child that never starts.

The scenario is now in the public provider-lifecycle suite. It isolates the
child's rows and restored handle, which the declared divergence
`D18.unacknowledged-callback-child` drops, and pins each side's exact values
with expectations. `tools/parity/open-divergences/unadmitted-callback-task.json`
is kept unchanged as this capture's source. The scenario generator no longer
writes it, so this capture's `verify.py` no longer reproduces at HEAD.
