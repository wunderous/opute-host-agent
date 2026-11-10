## 1. Decision

- [x] 1.1 Owner answers Q7 (rust-spec-authority/questions.md): fix each ported defect separately.

## 2. Implementation

- [x] 2.1 Map each core client to its Go client and transport in the pinned tree.
- [x] 2.2 Failing-first test: the HTTP probe reaches a non-loopback endpoint through `HTTP_PROXY`.
- [x] 2.3 Apply the environment proxy to the six clients whose Go counterpart uses the default transport.
- [x] 2.4 `NO_PROXY` selection follows Go's `httpproxy` rules (found
  2026-11-09 comparing `proxy_for` with the pinned toolchain's source; it
  had no test). Three deviations fixed, with a table test that was red
  first:
  - `.name` and `*.name` excluded the bare name too;
  - a name entry could match an IP host, and IP entries compared as text;
  - any run of leading `*` was a wildcard.
  Not ported: Go's refusal of `HTTP_PROXY` in a CGI environment
  (`REQUEST_METHOD` set), which a Host Agent never runs in.

