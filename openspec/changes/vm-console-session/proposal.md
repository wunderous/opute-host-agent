# A console session an MCP client can read

Status: **accepted (owner, 2026-11-11) as D38 and D39.** Goal m15.

## Why

The catalog publishes `stream_vm_console` ("Stream VM serial console output
(lxc console). Direct TTY access for boot observability"),
`send_console_input` and `resize_console`. Rust answers `not_implemented`.
The frozen v0.2.4 record (D27) shows two problems with simply porting Go:

- Go runs `incus exec <vm> -- bash -il`, a root shell in the guest, not the
  serial console the description promises.
- Over MCP, Go starts the PTY with no output reader: a client can type and
  resize but never sees a byte. Only the Platform worker protocol read the
  output.

## What Changes

- **D38:** a session runs `incus console <instance>`, the instance's
  console, as the description says. It is not a shell.
- **D39:** output reaches the client by pull. The session keeps the newest
  64 KiB of output; `send_console_input` returns, in a new `output` field,
  what arrived since the previous read, after writing `data` (an empty
  `data` only reads), and `closed: true` once the session has ended. Both
  fields are added to its output schema.
- Otherwise as recorded: one session per instance (a new stream replaces
  the old one), sessions keyed by `operationId`, `stream_vm_console`
  crosses admission with its declared cost and releases it when the call
  returns, `resize_console` sets the PTY size, and every session ends at
  agent shutdown.

## Impact

`crates/host-agent/src` (a console service on the D14 seam), the catalog's
`send_console_input` output schema (the catalog revision changes), the
fuzz inventory (console output is untrusted bytes), a console journey on a
dev-VM guest.
