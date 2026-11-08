## 1. Review

- [x] 1.1 Obtain the owner's decision on refusing null arguments instead of exiting.

## 2. Evidence

- [x] 2.1 Reproduce the Go panic and exit status 2 through the provider wire corpus, with a restart after each exit; the Rust port matches it.
- [x] 2.2 After an approval, implement the refusal in the Rust provider and record the step as a named divergence.
