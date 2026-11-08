## 1. Decision

- [x] 1.1 Owner answers Q16 (rust-spec-authority/questions.md).

## 2. Implementation (only after 1.1)

- [x] 2.1 Failing-first test: every call through the core runner passes `--project default`; `incus query`, which refuses the flag, carries `project=default` in its API path.
- [x] 2.2 Address the chosen project in every Incus call, including `incus query` (ed3bfe558).
- [ ] 2.3 On the dev VM, with the agent's client switched to another project, provision a container and observe it in `default`.
