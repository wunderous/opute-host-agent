## 1. Decision

- [x] 1.1 Owner answers Q19. Decided 2026-11-08: an optional, off-by-default
  endpoint (this change), resolved on the Incus bridge DNS, enabled by
  `OPUTE_K3S_CLUSTER_ENDPOINT=enabled`, CA by record reference.

## 2. Implementation

The tests were written alongside the code; each was shown red by mutating
the behaviour it guards (noted per task), not by history.

- [x] 2.1 Failing-first tests: not configured means not published and
  refused before any effect (`an_unconfigured_endpoint_is_not_published_and_is_refused`, `a_configured_endpoint_is_published`; red with the dispatch guard or the publication gate removed).
- [x] 2.2 Failing-first tests: not opted in means no TLS names or resolution
  changes on build, join or retire (`without_a_record_membership_hooks_change_nothing`; red when a missing record is treated as present).
- [x] 2.3 Failing-first tests: opt-in and opt-out update TLS names and
  resolution together; a repeat is unchanged; a server that does not return
  stops the rollout with `rollout_incomplete` (`opting_in_sets_tls_names_and_resolution_together`, `a_repeat_with_unchanged_membership_restarts_nothing`, `opting_out_removes_tls_names_resolution_and_the_record`, `a_server_that_does_not_return_stops_the_rollout`, `an_endpoint_that_resolves_keeps_resolving_only_to_ready_servers`, `a_retired_server_leaves_the_endpoint`, `a_joined_server_takes_the_tls_name_then_resolves`; red with the DNS update removed).
- [x] 2.4 Failing-first tests: the CA is only ever a reference (`the_ca_is_only_ever_a_reference`; red when the result carries the CA).
- [x] 2.5 The implementation in the K3s provider (`endpoint.rs`, membership
  hooks, manifest).

## 3. Journeys

- [ ] 3.1 The existing e2e-3a journeys rerun unchanged against the rebuilt
  provider (the endpoint is optional).
- [ ] 3.2 A new journey `e2e3a.endpoint` opts in, loses a server, retires it
  and opts out.
