# m27: the 0.4.0 release

The owner signed off on 0.4.0 on 2026-11-24 (packet 7.6) and asked for the
install surfaces to ship with it. PR #50 merged `rust-0.4.0` into main.
`rust-0.4.0-rc6`'s files went out unchanged through the release path of
m26; the publication is packet 8.1.

- Rust release: [`wunderous/opute-host-agent` v0.4.0](https://github.com/wunderous/opute-host-agent/releases/tag/v0.4.0),
  tag on `dd38feade`, the 9 files of rc6's set, checked after upload.
- host-agents #33: the pin `release/v0.4.0.json`; the proposed 0.4.0 unit,
  launcher patch and site-capture patch (`evidence/cutover/proposed-0.4.0`);
  the site and npm README document `serve --transport http` and
  `OPUTE_HOST_ALLOW_MUTATIONS`; launcher 0.4.0. Locally the launcher tests
  (13/13), the npm read-only canary on rc6 (1/1) and the capture tests passed.
- Public release: [`wunderous/host-agents` v0.4.0](https://github.com/wunderous/host-agents/releases/tag/v0.4.0),
  Publish run 45337632821; downloaded again, all 9 files equal the pin.
- npm: `@opute/host-agent@0.4.0`, `latest`, `gitHead` `29be23dc`; the
  published read-only canary passed. The site catalog is promoted from it
  (host-agents #34).
- rc6's shipped source is the source its gates ran on (run `m25-r4`); only
  `fuzz/Cargo.lock` changed on the branch since.
