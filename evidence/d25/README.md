# D25: the parity rebase onto Go v0.2.4 and the writer lock (evidence)

D25 (option A, owners 2026-11-01) puts one writer on each instance state
directory, in Go and in Rust. Go released the lock in v0.2.4. This round
rebases parity onto that release (task 3.1), checks Rust's lock (3.2) and
records the refusal matrix (4.1–4.3). Every suite the gates re-derive was
rerun against the v0.2.4 Go reference and the 0.2.4 Rust release artifact.

Nothing was published, no tag was pushed, and no shared Incus, Kubernetes,
registry, tunnel or deployment state was touched. The twins are disposable
qemu overlays of the pinned goldens, with no real credentials and no
production identity.

- **Go authority:** `wunderous/host-agents` v0.2.4, `c9f9e775` (tree
  `3bea852b`), `@opute/host-agent@0.2.4`.
- **Go reference:** `.parity/bin/go-reference`, `2e2b5486…`.
- **Rust candidate:** the x64 release artifact (`dist/host-agent-linux-x64`),
  `5eb911c8…`.
- **Script:** [run.sh](run.sh), in phases: `round`, `t2-resplit`, `dist`,
  `soak` and `gates`.

Owner rules (2026-11-01) apply: every suite runs once, Go/Go
self-validation runs three times, and no step runs over 30 minutes. A step
that hits the cutoff exits 124 and is retired, never rerun under a longer
limit.

**Result:**

- Gates m0–m6, m8a–m8f, m10, m12a, m12b and d25 pass.
- The d25 gate is the m12b gate plus three requirements:
  - the four writer-lock drills (`requireRehearsalDrills`);
  - the writer-lock inventory item covered (`requireCovered`);
  - the user unit refused, on both clean machines, while the bootstrap unit
    serves the instance.
- The soak (15 minutes, seed 1204) passes every check against the
  thresholds committed in M12b, which are unchanged.
- The seam verifier reports no gap.
- The cutover gate fails, as expected, on 13 owned inventory gaps (M11, M13,
  the 0.2.3 publish decision and the M10 platform gaps) and on the four M13
  cutover evidence files.

## Rounds

| Round | Head | Result | Notes |
| --- | --- | --- | --- |
| 1 | `307ef1d0` | Retired (`.parity/retired/d25-round-1-*`) | `build-artifacts.sh` still defaulted VERSION to 0.2.3, so the rebuild could not match `dist/`; fixed in `e1ea2d18` |
| 2 | `e1ea2d18` | Retired (`d25-round-2`) | The audit-write contract still expected server version 0.2.3; fixed in `53693a18` |
| 3 | `cef3a159` | Retired (`d25-round-3`) | `set -u` stopped the T2 loop on an unset shard name; fixed in `1db83b19` |
| 4 | `1db83b19` | Stands, except three captures | Go/Go T2 incus and cluster shards (x3) hit the 30-minute cutoff (exit 124); dist failed on three harness faults. All three retired (`d25-round-4`) |
| t2-resplit | `90848e3c` | All seven Go/Go shards pass | incus, serving-llm, cluster-a, cluster-b, oci-a, oci-b, postgres |
| dist 1 | `90848e3c` | Retired (`d25-round-5-dist`) | npm `concurrentLock`: the pinned launcher strips `OPUTE_*`, so both launchers shared one instance and the lock refused the second. Fixed in `6ed2322a` (a config directory per launcher) |
| dist 2 | `6ed2322a` | Passes | Both clean machines, the user/bootstrap refusal, npm and the site |
| soak | `0469584e` | Every check passes | 15 minutes, seed 1204 |
| gates | after `0469584e` | m0–m12b and d25 pass; cutover fails as expected | Inventory recaptured with the writer lock covered |

Go/Rust T2 (host, incus, cluster with oci and postgres, x1) fit inside the
limit in round 4 and stands.
