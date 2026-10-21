# M8d entry: naming (D23)

## Owner decision (2026-10-17)

The nine Go OCI tools in scope (`internal/tools/dispatch_oci.go`) keep their
names: `oci`, `container_storage` and `build_context` name open formats, not
providers. Provider names appear only as selector values.

Asked "keep as Go" or "neutral values", the owner chose **neutral values**,
then approved this map as proposed. It is recorded as D23 (OpenSpec change
`neutral-oci-selector-values`), extending the one D19 map
(`crates/host-agent/catalog/neutral-names.json`) with a value-level section.

| Field | Tools | Go value | Published value |
| --- | --- | --- | --- |
| `builder` | `ensure_oci_builder`, `build_and_push_oci_image` | `auto` | `auto` |
| | | `podman` | `container-engine` |
| | | `buildah` | `image-builder` |
| | | `buildkit` | `build-daemon` |
| `runtime` | `configure_oci_storage`, `inspect_container_storage`, `cleanup_container_storage` | `auto` | `auto` |
| | | `podman` | `container-engine` |

- Go values remain hidden aliases: an input with a Go value is accepted as
  the published value.
- Results that echo the chosen builder or runtime (for example
  `ensure_oci_builder`'s `builder`) publish the neutral value.
- Descriptions keep Go's text, as D19, D21 and D22 do.
- The parity divergence `D19.neutral-host-tool-names` rewrites Rust's side
  back to Go's values. Scenarios call with Go's values.

## Entry met (2026-10-17)

M8c round 4 passed, and `evidence/m8c/verify.py` reproduces it. M8b and M8c
were merged into `main` (PR wunderous/opute-host-agent#16 and #17). Gates
m0–m6, m8a, m8b and m8c pass on `main`.
