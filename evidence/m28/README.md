# m28: the 0.5.0 release (D55)

The owner signed off on `rust-0.5.0-rc1` on 2026-11-24 (packet 9.6). PR #74
merged the implementation and its evidence into main. rc1's files went out
unchanged through the m26 release path. The publication is recorded in
packet section 9.7.

- `before-d55.log`, `after-d55.log`: the D55 tests, failing on 0.4.0's code
  and passing on rc1.
- `fuzz-050.txt`: the 60 s pass over all 18 fuzz targets before the build.
- Rust release: [`wunderous/opute-host-agent` v0.5.0](https://github.com/wunderous/opute-host-agent/releases/tag/v0.5.0),
  tagged on `ddefaa2b0`. It carries the 9 files of rc1's set. They were
  downloaded again after upload and match `SHA256SUMS` `c3129bae…`.
- host-agents #35: the pin `release/v0.5.0.json`, the 0.5.0 unit
  (`--allow-mutations`, none of the old settings), the launcher (forwards
  the flag and strips the old settings), the READMEs and site docs, and
  launcher 0.5.0. `make check-site` passed locally, including the npm
  read-only canary on the pinned binary. So did the launcher tests (13/13)
  and the three decision checks.
- Public release: [`wunderous/host-agents` v0.5.0](https://github.com/wunderous/host-agents/releases/tag/v0.5.0)
  (Latest), from Publish run 45450198950 on `c8e52c0e`. It was downloaded
  again, and all 9 files equal the pin.
- npm: `@opute/host-agent@0.5.0` is `latest`, with `gitHead` `c8e52c0e`.
  The published read-only canary passed all seven checks, and the site
  catalog is promoted from it (host-agents #36).
- minimal.opute.io serves from the rc1 VM (recipe run `10f5041e…`, lease
  `minimal-opute-io`). The first restore attempt failed because the m20
  prerequisite helper still named the 0.4.0 VM's release directory. Once
  pointed at `v0.5.0`, the second attempt passed.
- `per-client-mutation-authority` is not archived. Its delta modifies
  `platform-neutral-host-agent`, which is not archived either, so
  `openspec archive` refuses it until that change is archived first.
