# Read container runtime reports from stdout alone (proposed)

Status: proposed. Owner question Q26
([rust-spec-authority/questions.md](../archive/2026-11-07-rust-spec-authority/questions.md)).
Nothing here is implemented.

## Why

e2e-4a journey `e2e2.oci` calls `inspect_container_storage` on the dev VM
right after `ensure_oci_builder` installed Podman. The agent's user has no
user session (lingering is off), so `podman info --format json` writes
warnings to stderr:

    time="…" level=warning msg="The cgroupv2 manager is set to systemd but there is no systemd user session available"

The agent runs the command with combined output and parses the whole stream
as JSON, so the call fails:

    Error: parse podman runtime information: invalid character 'i' in literal true (expecting 'r')

`configure_oci_storage` and `cleanup_container_storage` fail the same way.
Go v0.2.4 behaves the same: `runContainerCommand` uses `hostexec.Run`,
which returns `CombinedOutput`. With lingering enabled (what
`ensure_host_service_supervisor` sets up) Podman prints no warning and all
three tools succeed. No spec says whether these tools need a user session,
or whether a runtime's stderr belongs in its machine-readable report.

## What Changes

If the owner chooses it, the container storage tools parse a runtime's
stdout only and keep stderr for error messages, so a warning does not fail
a report. The alternative is to state that they need the user service
supervisor and fail with an error that says so. Until the owner decides,
Rust keeps Go's behaviour.

## Impact

- `crates/host-agent/src/ociops.rs` (runtime command output, `normalize_report`).
- `e2e2.oci` then asserts a report on a host without a user session.
