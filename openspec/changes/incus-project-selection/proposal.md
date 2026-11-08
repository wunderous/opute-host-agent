# Address one Incus project consistently

Status: decided (owner, 2026-11-07, D28). Owner question Q16
([rust-spec-authority/questions.md](../archive/2026-11-07-rust-spec-authority/questions.md)).
Implemented after the decision; see tasks.md.

## Why

e2e-3a found this while exploring for its first journey. The dev VM's
`opute` user had its `incus` client switched to the `opute-e2e` project,
and in that state `provision_vm` failed for a container it had just
created:

- the container was launched with plain `incus launch`, which uses the
  client's current project, so it landed in `opute-e2e` and got an address;
- the address wait uses `incus query /1.0/instances/<name>/state`. That
  query does not apply the client's current project and reads `default`,
  so after 3 minutes the call failed with `Instance not found` in project
  `"default"`.

Go has the same code (`awaitContainerIPv4` and `readIncusInstanceIPv4`).
The agent has no Incus project setting, so the only working configuration
is a client whose current project is `default`. Nothing in the spec, the
configuration reference or the docs says so. e2e-3a puts its guests in the
`default` project for this reason.

## What Changes

Decided (D28): the agent always addresses the `default` project. Every
Incus call the agent makes, through the core runner (`Incus::run_with`)
and the K3s provider (`run_program`), passes `--project default` (`incus query`, which refuses that flag, gets
`project=default` in its API path), so the client's current project never decides where an instance lands or is read.
No configuration is added.

## Impact

- `crates/host-agent/src/incus.rs` and `providers/k3s/mod.rs`; the k3s
  wire fake (`tools/parity/provider-wire/fake_k3s.py`) strips the prefix.
- A failing-first test reproduces the mismatch before the fix.
