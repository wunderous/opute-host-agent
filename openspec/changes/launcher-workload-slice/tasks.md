## 1. Decision

- [x] 1.1 Decide (owner, 2026-11-24): D56, the launcher creates the workload slice on `--allow-mutations`; target 0.5.1, launcher only.

## 2. Tests first

- [x] 2.1 Launcher tests that fail against 0.5.0's launcher, with a fake `systemctl` on `PATH` and a temporary `XDG_CONFIG_HOME`: slice created with the agent's text, existing slice kept, no user session refused, read-only start untouched, stdout unchanged. Three of the four fail before the change (`evidence/d56/before-d56.log`; the read-only test passes on 0.5.0 by design).

## 3. Launcher

- [x] 3.1 `npm/local-host-agent/index.js`: ensure the slice before the foreground and background starts when `--allow-mutations` is given.
- [x] 3.2 The text matches `hostpolicy.rs`'s rendered workload slice for `opute-host-resource-policy.v2` (byte-compared against the Rust format string).

## 4. Evidence and release

- [x] 4.1 A fresh Ubuntu 24.04 user, using the packed 0.5.1 launcher and the 0.5.0 binary: `start --background --allow-mutations` with no manual step; the first `ensure_host_file` succeeds; a second start keeps the slice; a start with no user session is refused. Also: a read-only start writes nothing. `evidence/d56/fresh-user.txt`.
- [x] 4.2 README step 4 (both repositories) and the npm README drop the manual block.
- [x] 4.3 Release 0.5.1: the host-agents pin `release/v0.5.1.json` names opute-host-agent `v0.5.0`'s files unchanged; tag, Publish, read-only canary, npm `latest`. Published 2026-11-24: host-agents#38 (merge `e3881f2`), tag `v0.5.1`, Publish run 45532389044 (read-only canary passed), 9 files equal the pin, npm `latest` 0.5.1; the published package passes the fresh-user check (`evidence/d56/fresh-user.txt`).
