## ADDED Requirements

### Requirement: Sealed join material opens under every AES key size
The K3s provider SHALL unseal join material whose GCM data key is 16, 24 or
32 bytes, and SHALL refuse any other key size with
`crypto/aes: invalid key size <n>`. A ciphertext that fails authentication
SHALL be refused with `cipher: message authentication failed`.

#### Scenario: A sealer used an AES-192 data key
- **WHEN** a destination unseals join material whose data key is 24 bytes
- **THEN** the material's fields are returned

#### Scenario: A tampered AES-192 envelope
- **WHEN** the ciphertext of such an envelope is altered
- **THEN** unsealing fails with `decrypt sealed join material: cipher: message authentication failed`
