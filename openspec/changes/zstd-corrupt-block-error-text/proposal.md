# Decoder-specific text for a corrupt zstd block

## Why

`extract_host_archive` decodes `tar.zst` archives. The pinned Go agent uses
`klauspost/compress` v1.18.0, and Rust uses `ruzstd`. Both decode every
well-formed stream to the same bytes, and Rust reproduces Go's error text for
every frame-level fault:
- a bad magic number;
- truncation anywhere in the stream;
- a skippable or truncated skippable frame;
- concatenated frames and trailing bytes;
- every tar-level fault.

When a compressed block is internally corrupt, though, each decoder reports
its own internal finding, often at a different check. Go says
`corrupt input: last value not power of two`, where Rust says
`zstd: Failed to parse block header: DecompressLiteralsError(...)`.
Matching Go's text would mean porting klauspost's decoder.

**Decided (owner, 2026-10-12), recorded as D20.** Declare the difference
narrowly, instead of porting the decoder. Recipe archives are
sha256-verified before extraction, so a corrupt block does not occur in
normal use.

## What Changes

For a corrupt compressed block, Rust's error is
`extract host archive: zstd: <ruzstd's description>`. The call still fails
the same way, with the same `extract host archive: ` prefix and the same
extracted tree. No other error, result or effect changes. Go is unchanged.

## Capabilities

### New Capabilities

- `zstd-corrupt-block-error-text`: the declared, decoder-specific error text
  for a corrupt zstd block.

### Modified Capabilities

None.

## Impact

`D20.corrupt-zstd-block` in `tools/parity/divergences.json` keeps only the
`Error: extract host archive: ` prefix of the step text. It applies only in
`host-domain.archives.corrupt-blocks`, and only between Go and Rust.
Per-side expectations pin each side's exact text. Every other archive case,
including the frame-level faults, stays compared byte for byte.
