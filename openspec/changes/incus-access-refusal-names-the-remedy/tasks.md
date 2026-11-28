## 1. Tests first

- [x] 1.1 `incus::tests::a_refused_incus_socket_says_how_to_get_access`: a fake `incus` that refuses the socket. On 0.5.4's code the error carries no remedy.

## 2. Change

- [x] 2.1 The Incus command runner appends the remedy to that refusal, from the group database and the process's groups.
- [x] 2.2 README step 4: after the first Incus install, log in again and restart the agent.

## 3. Evidence

- [x] 3.1 Journey `e2e9.readme`: the newcomer's first `install_virtualization_stack` names the remedy; after a new login and a restart, `create_vm` makes the sandbox VM. Passes on `rust-0.5.5-rc1` (run `m33-r3`, gate `e2e-readme`); on 0.5.4 only the remedy is missing (`m33-before-r3`).
