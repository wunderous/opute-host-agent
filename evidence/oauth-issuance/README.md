# OAuth issuance evidence (decision D8)

This file records the Rust implementation of
[`secure-oauth-issuance`](../../openspec/changes/secure-oauth-issuance/proposal.md),
a declared divergence from the Go baseline approved as decision D8 in
[milestones.md](../../openspec/changes/reimplement-host-agent-in-rust/milestones.md).
The Go agent is not changed by this work.

The evidence is in [`evidence/current/`](../current/):
`contracts/oauth-issuance/` and `rust-canaries.json`. Reproduce it with
`make parity-contracts parity-rust-canaries`, or `make parity-m2` for the
whole gate.

## How a divergence is verified

```text
            Go-vs-Rust twin runs                 Rust-only contract suite
  ┌─────────────────────────────────────┐   ┌──────────────────────────────┐
  │ every M1/M2 scenario, ×5            │   │ 24 scenarios, one per spec   │
  │ D8-owned content dropped from BOTH  │   │ requirement/scenario         │
  │ sides by id (divergences.json)      │   │ status, headers, JSON, CLI,  │
  │ stale declaration  → fail           │   │ file modes, rows, log scans  │
  │ unapproved decision → fail          │   └──────────────┬───────────────┘
  └─────────────────────────────────────┘                  │ must go red under
                                                           ▼
                                             15 Rust canaries (one-line patches)
```

Parity proves that nothing *outside* D8 moved. The contract suite proves the
new behaviour is what the spec says. The canaries prove the contract suite
can fail.

## Results

| Check | Result |
| --- | --- |
| Contract suite `oauth-issuance` against the Rust candidate | **24/24 pass** |
| Rust canaries | **15/15 caught**, each by its named scenario (plus declared co-failures); unpatched build green |
| Go vs Rust, M1/M2 surfaces, ×5 | clean outside the declared D8 divergences; none stale |
| `make parity-verify-m2` | **PASS** (the gate now requires the contract suite and the Rust canaries) |

### Declared divergences

| Id | What Rust adds | Scenarios |
| --- | --- | --- |
| `D8.credential-files` | `state/credentials/` (0700) with 0600 secret files | lifecycle, `state.schema-after-start` |
| `D8.authz-tables` | `pending_authorizations`, `consents`, `schema_migrations` | lifecycle, `wire.auth.*`, `state.schema-after-start` |
| `D8.authz-row-counts` | the `host-agent-provider` client row and the new tables' rows | same as above |
| `D8.audit-server-log`, `D8.audit-step-stderr` | one `msg=oauth` audit line per issuance decision, provisioning and migration | lifecycle |

The seeded store in `wire.auth.*` and `authz.store-cross-read` records the
`oauth-issuance.v1` migration, so both binaries read the same seeded tokens
and the cross-read stays a strict parity check. The one-time revocation on
upgrade is checked by `migration.revokes-live-tokens` instead.

### Contract coverage

| Spec requirement | Scenarios |
| --- | --- |
| Every issued token is backed by a credential or an operator approval | `cc.no-secret`, `cc.unknown-client`, `cc.confidential-without-hash`, `cc.public-client`, `authorize.unapproved` |
| Provisioned credentials for built-in machine clients | `credentials.first-start`, `credentials.operator-secret`, `credentials.rotation` |
| Provider access paths | `provider.colocated-bearer`, `provider.public-without-credential` |
| Operator approval of interactive authorization requests | `approval.cli-approve`, `approval.deny`, `approval.expiry`, `approval.not-over-http`, `approval.remembered-consent`, `approval.once-and-disabled` |
| Issued tokens name only resources this agent serves | `audience.arbitrary-host` |
| Redirect, client metadata and abuse hardening | `redirect.exact-and-loopback`, `cimd.refuses-local-metadata`, `abuse.pending-flood`, `abuse.backoff-only-failures` |
| Issuance is audited without secrets | `audit.issuance-line`, plus log scans in `credentials.first-start` |
| Migration revokes pre-existing tokens | `migration.revokes-live-tokens`, `migration.idempotent` |

### Rust canaries

| Canary | Mutation | Caught by |
| --- | --- | --- |
| K1 | `client_credentials` accepts an empty secret | `cc.no-secret` |
| K2 | public clients may use `client_credentials` | `cc.public-client` |
| K3 | any requested resource is bound | `audience.arbitrary-host` |
| K4 | codes issued without an operator decision | `authorize.unapproved` |
| K5 | redirect URIs match on a scheme-and-host prefix | `redirect.exact-and-loopback` |
| K6 | credential files written world-readable | `credentials.first-start` |
| K7 | rotation leaves tokens valid | `credentials.rotation` |
| K8 | the migration revokes nothing | `migration.revokes-live-tokens` |
| K9 | codes redeemable more than once | `approval.cli-approve` |
| K10 | denied grants log the presented secret | `audit.issuance-line` |
| K11 | backoff refuses valid grants too | `abuse.backoff-only-failures` |
| K12 | approval page framable by the same origin | `approval.cli-approve` |
| K13 | one more pending request per client | `abuse.pending-flood` |
| K14 | pending requests outlive ten minutes | `approval.expiry` |
| K15 | an HTTP route approves a pending request | `approval.not-over-http` |

## What validation caught (and what changed)

1. **Restarts rotated the Platform secret.** The Go-compatible open
   re-registered the built-in clients on every start, wiping the stored hash,
   so provisioning saw a mismatch and rotated (revoking tokens). The
   migration step now owns the built-in client rows.
2. **Backoff locked out valid callers.** Failures were counted per remote
   address, and behind a tunnel every caller is `127.0.0.1`. Only failing
   requests are throttled now; secrets and codes carry 256 bits, so slowing
   a valid grant adds nothing. The spec gained a scenario for it (K11).
3. **CLI flag order.** `oauth approve CODE --state-dir DIR` ignored the
   trailing flag under Go flag rules; the new `oauth` command accepts flags
   anywhere.
4. **A contract check that could not fail.** K14 first survived because the
   expiry-window check ran after the test had aged the row. The check now
   runs on the fresh request.

## Open items

- **5.3:** the go-sdk real client completing an operator-approved
  authorization against Rust.
- **M3:** the `list_oauth_authorizations` / `approve_oauth_authorization`
  admin tools.
- **M7:** the provider environment gains `OPUTE_HOST_AGENT_CLIENT_ID` and
  `OPUTE_HOST_AGENT_CLIENT_SECRET_FILE`; public-URL activation evidence
  needs the provider-side `client_secret_basic` change and fails closed
  until then.
- **M8:** in-process self-probe tokens and public bindings as allowed
  audiences; their contract scenarios join the suite then.
