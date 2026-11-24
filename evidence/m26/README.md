# m26: the 0.3.0 release

Goal m26 publishes `rust-0.3.0-rc9`'s verified files as Host Agent 0.3.0
through the existing release surfaces; Go v0.2.4 stays the rollback. The
owners signed off on 2026-11-23 (packet section 6); the publication is packet
section 8.

- [`surfaces.json`](surfaces.json): every release consumer, what it fetches
  and whether it pins a version. No consumer follows `latest` unpinned, so
  host-agents `v0.3.0` is marked Latest.
- Rust release: [`wunderous/opute-host-agent` v0.3.0](https://github.com/wunderous/opute-host-agent/releases/tag/v0.3.0),
  tag on `0883e47fa`, the 9 files of rc9's set, checked after upload.
- Public release: [`wunderous/host-agents` v0.3.0](https://github.com/wunderous/host-agents/releases/tag/v0.3.0),
  published by Publish run 45328650629 from the pin `release/v0.3.0.json`
  (host-agents #30, #31). Downloaded again: all 9 files equal the pin; the
  launcher URL for `host-agent-linux-x64.gz` answers 200.
- npm: `@opute/host-agent@0.3.0`, dist-tag `latest`, published by the npm
  job of run 45328650629 attempt 3 after the owner renewed `NPM_TOKEN`
  (attempt 1 E404, expired; attempt 2 E403, no 2FA bypass). The published
  read-only canary passed; the site catalog is promoted from its evidence
  (host-agents #32).
- Not in 0.3.0: a Windows binary (D3), and a build-provenance attestation
  (the host-agents workflow re-hosts files it did not build; provenance is
  the pinned commit and `make reproducible-build`).
