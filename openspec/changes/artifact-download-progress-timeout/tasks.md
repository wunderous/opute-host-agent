## 1. Tests first

- [ ] 1.1 `hostartifact.rs` tests against a local server that sends the body slowly: a download that keeps making progress past the no-progress limit completes; a stall fails; one past the total limit fails. The first fails with the 0.5.3 single deadline.

## 2. Change

- [ ] 2.1 The download agent takes a no-progress limit (300 s) for connect, reads and writes, and a total limit (one hour).

## 3. Evidence

- [ ] 3.1 Journey `e2e8.local-llm` downloads the 1.5 GB Ollama archive through `ensure_host_artifact` on the dev VM, on a release candidate with this fix.
