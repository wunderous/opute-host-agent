# M8b entry: blocked on two owner confirmations

Checked 2026-10-14 on `main` at `aca4fb32` (PR wunderous/opute-host-agent#14,
M8a, merged).

## Met

- M8a is merged.
- On `main`, `evidence/m8a/verify.py` reproduces `validation.json`, and every
  gate report matches byte for byte. Gates m0–m6 and m8a pass. Cutover fails
  as expected.
- The locked release build reproduces the pinned Rust candidate `b190b597…`.

## Needed from the owner

### 1. D19 on durable rows and the secret sweep

M8a applied the existing `D19.neutral-host-tool-names` rule in two places
beyond the wire output:

- the durable rows compared by M5 shape (`catalog_revision`,
  `supportedTools`);
- the projected catalog in the secret sweep.

Those rows record the same renamed tools and the same Rust catalog revisions
that the wire comparison already maps. The rule changes only the Rust side,
and only for pinned names and revisions. It is recorded only when it rewrites
something. M8a also pinned one more provider-fixture revision pair: the rival
catalog after a family replacement, observed one-to-one in 5/5 iterations.
See `evidence/m8a/README.md`, "Normalizations and divergences".

Please confirm this scope (or reject it, in which case the M5 shape and
secret-sweep comparisons need a different treatment).

### 2. The M8b rename map

These are the Go incus-domain tools in scope, from
`internal/tools/dispatch_incus.go`, `dispatch_unassigned.go` and
`internal/contract/toolname`:

| Go tool | Effect / class | Provider-named? | Proposal |
| --- | --- | --- | --- |
| `reset_incus_stack` | destructive / heavy | **tool name** | rename to `reset_virtualization_stack`, matching D19's `install_`/`uninstall_virtualization_stack`; keep the Go name as a hidden alias |
| `provision_container` | mutation / heavy | **field `wslGpuLibs`** | either keep the field (a boolean: mount the WSL host's GPU library directory into the container when it exists) or rename it to `hostGpuLibs` with a hidden alias. Owner's choice; I lean to keeping it, since the field is about WSL specifically, not the instance provider |
| `create_vm`, `provision_vm` | mutation / heavy | no | keep |
| `start_vm`, `stop_vm`, `restart_vm` | mutation / normal | no | keep |
| `update_vm_resources` | mutation / normal | no | keep |
| `delete_vm` | destructive / normal | no | keep |
| `probe_gpu_container` | mutation / normal | no | keep |
| `exec_command`, `run_instance_command` | ported in M8a; T2 needs nested Incus | no | keep |

Tool descriptions keep Go's text, including "Incus" where it names the
product, as D19 did for M8a. Only renamed tool names inside descriptions are
rewritten.

If you approve, the rename goes into one OpenSpec change extending the D19 map
(`crates/host-agent/catalog/neutral-names.json`). The catalog revisions are
then re-pinned with their Go pairs.

## Next once confirmed

1. Write the OpenSpec change for the rename.
2. Inventory the 12 tools (schema, argv, effect tier, env reads, admission
   class, Go tests).
3. Add nested Incus to the T2 golden.
4. Port by service family.
