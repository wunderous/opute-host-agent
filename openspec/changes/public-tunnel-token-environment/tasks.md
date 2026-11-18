## 1. Decision

- [x] 1.1 Decide S1 (delegated by the owner, 2026-11-11): D47.

## 2. Implementation and evidence

- [x] 2.1 The fix on main, merged into rust-0.4.0.
- [x] 2.2 Failing-first test `hostpublic::tests::the_tunnel_token_never_reaches_cloudflareds_arguments`.
- [ ] 2.3 Gate e2e-6 on rust-0.3.0-rc6 and rust-0.4.0-rc4: the journey sees no tunnel token on any argv, and cloudflared still reads it (it rejects the test token) (`e2e6.secret-sweep`).
