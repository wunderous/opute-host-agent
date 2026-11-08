## 1. Decision

- [x] 1.1 Owner answers Q19. Decided 2026-11-08: an optional, off-by-default
  endpoint (this change), resolved on the Incus bridge DNS, enabled by
  `OPUTE_K3S_CLUSTER_ENDPOINT=enabled`, CA by record reference.

## 2. Implementation

- [ ] 2.1 Failing-first tests: not configured means not published and
  refused before any effect.
- [ ] 2.2 Failing-first tests: not opted in means no TLS names or resolution
  changes on build, join or retire.
- [ ] 2.3 Failing-first tests: opt-in and opt-out update TLS names and
  resolution together; a repeat is unchanged; a server that does not return
  stops the rollout with `rollout_incomplete`.
- [ ] 2.4 Failing-first tests: the CA is only ever a reference.
- [ ] 2.5 The implementation in the K3s provider (`endpoint.rs`, membership
  hooks, manifest).

## 3. Journeys

- [ ] 3.1 The existing e2e-3a journeys rerun unchanged against the rebuilt
  provider (the endpoint is optional).
- [ ] 3.2 A new journey `e2e3a.endpoint` opts in, loses a server, retires it
  and opts out.
