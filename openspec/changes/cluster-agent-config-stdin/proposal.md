# The cluster agent's config reaches the target on stdin, 0600

Status: **accepted (Host Agent owner, delegated 2026-11-11) as D50**, under D46 ([`secret-placement`](../secret-placement/proposal.md)). Goal m19, finding S4. Applies to 0.3.0 from `rust-0.3.0-rc6` and to 0.4.0 from `rust-0.4.0-rc4`.

## Why

`install_cluster_agent` rendered the cluster agent's config, bridge token
included, base64-encoded into its install script. The script runs as the
argument of `bash -lc` and, for a host install, of `sudo -n bash -lc`, or of
`incus exec <vm> -- bash -lc`, so the token was on argv; sudo logs its
command line, so it was in the journal; and the file was written with the
shell's umask, 0644 (finding S4, measured by `e2e6.secret-sweep` on
rust-0.3.0-rc5 and rust-0.4.0-rc3).

## What Changes

The script no longer contains the config. It is sent on the install
command's stdin (through `sudo` and `incus exec`), and the script's first
write, before any command that could read stdin, is `(umask 077 && cat >
/etc/opute/cluster-agent.json) && chmod 600 /etc/opute/cluster-agent.json`.
The rest of the script is Go's.

## Options rejected

- **Writing the config with `install_host_file`-style tool calls.** The
  VM path runs inside a guest the host's file tools do not reach.
- **Keeping base64 and only fixing the mode.** The token would stay on argv
  and in sudo's log.

## Impact

`clusterops.rs` (`agent_install`, `agent_install_script`,
`host_shell_input`); the test `the_bridge_token_never_travels_on_argv`
failed on rc5's and rc3's code; the Go-golden script test now expects
Go's script without the embedded config.
