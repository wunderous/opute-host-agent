# Open spec questions for the owner (D27)

Until D27, these behaviours were settled by "as Go does". The frozen v0.2.4
record either captures them only incidentally or does not decide them at all.
Each one needs an owner answer before a Rust change may depart from what Rust
does today. Until then, Rust keeps its current behaviour, which matched Go at
the last parity gate. Nothing here is guessed.

| # | Behaviour | Where Go decided it | Question |
|---|-----------|--------------------|----------|
| Q0 | The frozen record itself | `baseline/inventory`, `tools/parity/divergences.json` | Is the frozen v0.2.4 record normative, with every captured field a requirement, or only evidence that the spec should restate in prose? |
| Q1 | Auth refusal statuses and bodies | Go HTTP middleware | Which status codes and `WWW-Authenticate` and body shapes are contract, and which are incidental? |
| Q2 | The legacy handshake method set | Go ADR 0011 | Should the spec list the method set explicitly, so that the ADR is no longer the source? |
| Q3 | Typed admission error shapes | Go admission errors | Are error codes and messages contract, or only codes? |
| Q4 | Recipe canonical hashing and validation outcomes | Go recipe package | Is the canonical hash algorithm and its byte layout part of the contract, for example because the Platform stores the hashes? |
| Q5 | Plan interruption semantics | Go plan executor | What should resume, compensate or stop after a restart, for each node state? |
| Q6 | List order where Go ranged over maps | k3s roles and du paths, tailscale teardown | Is the order unspecified, or should the spec require a sorted order? |
| Q7 | Go defects ported as is | cloudflare null-args panic, k3s AES-192 unseal, proxy environment in 8 core clients | Should each be fixed to a specified behaviour? Each fix needs its own change. |
| Q8 | Exact error text | D20 (zstd messages) | Are error message strings contract? |
| Q9 | Bridged operation terminal status | Go keys the bridge on the task ID, so the status stays `working` | What is the specified terminal status of a bridged operation? |
| Q10 | Release artifact names and SHA256SUMS order | Go release scripts | Is the order of lines in SHA256SUMS contract, or only the names and the hashes? |
| Q11 | Incidental go-sdk wire details | go-sdk v1.7.0: session log lines, server info | Which of these does a client rely on? |
| Q12 | Catalog revision computation | Go catalog | Is the revision a contract value, meaning its algorithm, or only an opaque token that changes when the catalog changes? |
| Q13 | Cutover evidence | `rust-cutover-parity-gate.json` | Are the e2e-1 to e2e-5 journey gates, the contract suites and the conversion and rollback evidence the agreed M13 evidence set? |

The spec's Purpose paragraph still describes behaviour preserved "from the
pinned Go source". OpenSpec deltas cannot change a Purpose, so it is
rewritten when this change is archived (tasks 3.1).
