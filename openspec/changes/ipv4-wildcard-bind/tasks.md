## 1. Decision

- [x] 1.1 Owner answers Q14 (rust-spec-authority/questions.md): `0.0.0.0` binds IPv4 only.

## 2. Implementation

- [x] 2.1 Failing-first test: listening on `0.0.0.0:0` yields an IPv4 socket.
- [x] 2.2 Bind `AF_INET` for an explicit `0.0.0.0`.
- [ ] 2.3 Rerun e2e1.system-unit with the IPv4 listener as a pass condition.
