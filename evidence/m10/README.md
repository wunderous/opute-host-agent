# M10: packaging, installer, npm launcher, site (evidence)

M10 proves that the Rust release set is operated exactly like Go's. It
covers:

- the release artifacts;
- the published systemd units and sudoers file on clean machines;
- an upgrade and downgrade on one identity;
- the npm launcher;
- the generated site.

The public website, the npm package, the release artifacts and the install
scripts keep their current owners. Nothing was published and no tag was
pushed. Releases were served only from local release bases
(`OPUTE_HOST_AGENT_RELEASE_BASE_URL` for the launcher; a loopback server on
each twin).

- **Go authority:** `wunderous/host-agents` `ace7013d` (tree `ac17cfb2`),
  `@opute/host-agent@0.2.3`.
- **Rust candidate:** the x64 release artifact itself
  (`dist/host-agent-linux-x64`, `d1f091ed…`), built with `make artifacts`.
- **Rust release set:** pinned by its `SHA256SUMS` (`f565809a…`).
- **Go release set:** built by Go's own targets (`parity dist release-go`,
  `7f8d3a89…`).
- **Branch base:** `main` at `1cf05b50`, after
  wunderous/opute-host-agent#20.

**Result:**

- The m10 gate passes for the Linux targets, and gates m0–m6 and m8a–m8f
  still pass.
- The seam verifier reports no gap, and its dispatch canary is red.
- The cutover gate fails, as expected. It fails on 8 owned inventory gaps
  and on the four cutover evidence files of M12/M13.

## Rounds

| Round | Head | Result | Notes |
| --- | --- | --- | --- |
| 1 | see [checks/head](checks/head) | All 34 steps exited 0. The tree was clean before and after, and the binaries and both release sets were unchanged during the round. | The m10 gate first failed on provenance: the manifest pinned a stale `SHA256SUMS` for the Rust set, from an in-place rebuild before the round that differs only in gzip mtimes ([NOTE.md](NOTE.md#the-rust-release-set-pin)). The pin was corrected in its own commit, and the gate then passes. No capture was retired. |

Dry runs of `parity dist run` during development were not evidence. They
found three harness defects, each fixed before round 1:

- systemd's memory amounts in journal lines;
- systemd's accounting line;
- `npm exec`'s own install race.

## Results

| Place | Go/Go | Go/Rust | What was compared |
| --- | --- | --- | --- |
| Artifacts | — | pass (6/6) | Release names and order in `SHA256SUMS`; checksums verify; gzip header (`-9`, name, mtime set) and archive = binary; ELF machine and linkage (x64 dynamic, arm64 static); glibc floor (Rust 2.34 ≤ Go 2.34); `--version`/`-version` = `0.2.3` |
| Reproducibility | binaries rebuilt byte for byte | binaries rebuilt byte for byte (cold target dir) | `host-agent-linux-x64`, `host-agent-linux-arm64` and the three providers; archives differ only by gzip mtime |
| Log lines | 19/19 | 19/19 | Startup lines and 18 request classes (answered, refused by auth, transport or session, removed in the new protocol, notifications, per-request log level) |
| Clean machine, Ubuntu 24.04 | 0 diffs | 0 diffs | User unit install from the local base; health; read-only first call (142 tools, wrong token 401, `get_host_info`, catalog); unit state up and down; tree (paths, modes, owners, hashes); journal; state tables; uninstall. Published bootstrap unit refusing without an identity; bootstrap unit with an identity drop-in and the sudoers file (`visudo` clean) |
| Clean machine, Debian 12 | 0 diffs | 0 diffs | Same journey |
| Upgrade / downgrade (both distros) | Go→Go→Go | Go→Rust→Go, 0 diffs against Go→Go→Go | Explicit `OPUTE_REMOTE_AGENT_ID`, instance id, fingerprint and token kept in every phase; env file unchanged; no new identity; state read back by Go after Rust |
| npm launcher | identical | identical (21/21 checks) | `index.test.js` 13 pass, 1 skipped (native-Windows only, covered by the `win32` case). Readonly canary per side (Go's `local-npm-readonly-canary`). Download, verify and cache from the local base, and cache reuse. Checksum mismatch rejected, nothing cached. Concurrent download lock (one archive download, lock seen and released). Unsupported `darwin/arm64`, `linux/ia32` and `win32` messages |
| Site | Go's `check-site` on the committed site: 11/11 steps, tree clean | Rust preview equals Go's byte for byte after reversing the declared renames | Raw preview difference: exactly the 4 reviewed files (`distEvidence.reviewedSiteDiff`, D19/D21–D24). Go's release policy refuses the Rust catalog for the stable `0.2.3` package ([NOTE.md](NOTE.md#findings), finding 1) |
| T1 differential | 327/327 ×20 | 327/327 ×5 | All suites rerun against the release artifact |
| T2 twins | 34/34 ×20 | 34/34 ×5 | All suites rerun against the release artifact |

The declared normalizations are listed in [NOTE.md](NOTE.md#declared-normalizations).
`dist_verify` re-derives every result above from the committed receipts
([dist/](dist/)).

## Owned gaps (not closed here)

| Gap | Owner | Why it is open |
| --- | --- | --- |
| `host-agent-windows-x64.gz`, `.exe`, `.exe.gz` | release | Needs a Windows build. Go's own build lacks `modernc.org/libc`'s Windows files in the pinned offline module cache, and Rust's Windows target is not built here. |
| WSL2 install | release | Needs a WSL2 runner |
| `compact_wsl_disk`, `shutdown_wsl`, `terminate_wsl_distribution` (T2) | host | Need a WSL2 runner: the effects act on the Windows host |
| linux-arm64 clean-machine install | release | No `qemu-system-aarch64` or user-mode emulation on the runner. The arm64 binary is checked as an artifact only. |
| Cutover evidence: `packageFirstSuccess`, `externalEffects`, `rollbackRehearsal`, `ownerReview` | M12/M13 | Belong to the cutover rehearsal and decision |
| Published-package canary against the registry | 5.2 / cutover | Nothing is published. Publishing a Rust build under `0.2.3` is refused by Go's release policy (finding 1). |

`parity-verify-cutover` fails, as expected, on these items.

CI's parity job on `main` fails only because provider binaries are not built
in CI. That is unchanged by M10.

## Receipts

| What | Receipt |
| --- | --- |
| Validation receipt, recomputed from the raw captures by [`verify.py`](verify.py) | [validation.json](validation.json) |
| Round script and its raw logs and exit codes | [run.sh](run.sh), [checks/](checks/) |
| Distribution round: releases, artifacts, logs, npm, site, machines, summary, results | [dist/](dist/) |
| T1 suites | [go-vs-go](go-vs-go/summary.json), [go-vs-rust](go-vs-rust/summary.json) |
| T2 suites and goldens | [t2-go-vs-go](t2-go-vs-go/summary.json), [t2-go-vs-rust](t2-go-vs-rust/summary.json), [base](t2-golden.json), [incus](t2-golden-incus.json), [k3s](t2-golden-k3s.json), [oci](t2-golden-oci.json), [postgres](t2-golden-postgres.json), [debian](t2-golden-debian.json) |
| Gate reports, rerun byte for byte by `verify.py` | [m0](verify-m0.json) [m1](verify-m1.json) [m2](verify-m2.json) [m3](verify-m3.json) [m4](verify-m4.json) [m5](verify-m5.json) [m6](verify-m6.json) [m8a](verify-m8a.json) [m8b](verify-m8b.json) [m8c](verify-m8c.json) [m8d](verify-m8d.json) [m8e](verify-m8e.json) [m8f](verify-m8f.json) [m10](verify-m10.json) [cutover](verify-cutover.json) |
| Contracts, canaries, oracles, M5 | [contracts/](contracts/), [canaries](canaries.json), [Rust canaries](rust-canaries.json), [oracles](oracles.json), [oracles-t2](oracles-t2.json), [m5/](m5/) |
| D14 seam stage | [report](../seam/m10/report.json) |
| Go's surfaces, Rust changes, findings, masks | [NOTE.md](NOTE.md) |
| Inventory | [distribution](../../baseline/inventory/distribution.json), [gaps](../../baseline/inventory/gaps.json) |
