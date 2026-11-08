## 1. Decision

- [x] 1.1 Owner answers Q7 (rust-spec-authority/questions.md): fix each ported defect separately.

## 2. Implementation

- [x] 2.1 Failing-first test: an envelope whose data key is AES-192 unseals (vector made with Python `cryptography`), and a tampered one fails authentication.
- [x] 2.2 Open AES-192 GCM with `aes-gcm`; AES-128 and AES-256 stay on ring.
