## ADDED Requirements

### Requirement: A corrupt zstd block fails like Go, with the decoder's own text
When `extract_host_archive` meets a corrupt compressed zstd block, the Rust Host Agent SHALL fail the call as the pinned Go agent does: an error result whose text starts with `extract host archive: `, and the same extracted tree (owner decision D20). The text after that prefix MAY be the Rust decoder's own description. Every frame-level fault SHALL keep Go's exact text: a bad magic number, truncation, a skippable frame, concatenated frames, trailing bytes, and every tar fault.

#### Scenario: Corrupt literals in a compressed block
- **WHEN** an archive whose first compressed block has a corrupt Huffman table is extracted
- **THEN** both agents fail with `extract host archive: `, Go continues with `corrupt input: last value not power of two`, Rust continues with its decoder's description, and the extracted trees are identical

#### Scenario: Frame-level faults keep Go's text
- **WHEN** the archive is truncated, has a bad magic number or has a truncated skippable frame
- **THEN** Rust's error text equals Go's exactly

### Requirement: The difference is declared, isolated and pinned
The parity suite SHALL declare `D20.corrupt-zstd-block`, which keeps only the `Error: extract host archive: ` prefix of a step's text. It SHALL apply only in `host-domain.archives.corrupt-blocks` and only between Go and Rust. Per-side expectations SHALL pin each side's exact text, and a text without the prefix SHALL stay compared whole.

#### Scenario: Corrupt-block evidence
- **WHEN** the corrupt-block case runs Go against Rust
- **THEN** the divergence is applied and not stale, each side matches its pinned text, and every other step and the tree match
