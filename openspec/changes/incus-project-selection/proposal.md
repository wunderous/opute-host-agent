# Address one Incus project consistently (proposed)

Status: proposed. Owner question Q16
([rust-spec-authority/questions.md](../archive/2026-11-07-rust-spec-authority/questions.md)).
Nothing here is implemented. Current behaviour matches Go v0.2.4.

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

If the owner accepts Q16, one of these:

- the agent addresses `default` explicitly in every Incus call, and
  documents it; or
- the agent takes one configured project and passes it to every call,
  including `incus query` (`?project=`).

## Impact

- `crates/host-agent/src/incus.rs` and `incusops.rs`; the configuration
  reference.
- A failing-first test reproduces the mismatch before the fix.
