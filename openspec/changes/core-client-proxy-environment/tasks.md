## 1. Decision

- [x] 1.1 Owner answers Q7 (rust-spec-authority/questions.md): fix each ported defect separately.

## 2. Implementation

- [x] 2.1 Map each core client to its Go client and transport in the pinned tree.
- [x] 2.2 Failing-first test: the HTTP probe reaches a non-loopback endpoint through `HTTP_PROXY`.
- [x] 2.3 Apply the environment proxy to the six clients whose Go counterpart uses the default transport.
