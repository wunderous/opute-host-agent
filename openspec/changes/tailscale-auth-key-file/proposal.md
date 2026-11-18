# The Tailscale join reads its auth key from stdin

Status: **accepted (Host Agent owner, delegated 2026-11-11) as D49**, under D46 ([`secret-placement`](../secret-placement/proposal.md)). Goal m19, finding S3. Applies to 0.3.0 from `rust-0.3.0-rc6` and to 0.4.0 from `rust-0.4.0-rc4`.

## Why

The Tailscale provider sends the auth key to the guest on stdin, but the
join script then ran `tailscale up --authkey="$auth_key"`, putting it on
argv for any user in the guest to read for the up to 120 s the join takes
(finding S3, a unit test; the dev VM has no tailnet).

## What Changes

The script pipes the key into `tailscale up --auth-key=file:/dev/stdin`
(tailscale reads a `file:` auth key from that path) and unsets it. Nothing
else changes.

## Options rejected

- **A temporary file in the guest.** It would have to be created 0600 and
  removed on every exit path; stdin needs neither.
- **Keeping Go's `--authkey="$auth_key"`.** The key would stay on argv for
  the whole join.

## Impact

`providers/tailscale/live.rs` (`enroll_script`); the test
`the_auth_key_never_reaches_tailscales_arguments` runs the script with stub
tools and failed on rc5's and rc3's code.
