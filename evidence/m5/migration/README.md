# M5 older-state migration

Six differential runs cover two fixtures, each repeated three times:

- `released-v0.2.2`: schema captured by starting the previous release's
  isolated Go binary, then populated with non-secret representative records.
- `pre-column-additions`: that schema with the additive migration columns
  removed, and the newer active-capability/resource tables absent. This is
  an explicit synthetic fixture for the older-column contract exercised by
  Go's focused state tests, not a claim that v0.2.2 lacked those columns.

The release source is commit `9f2a18993b0f268f393483a5e098124a276fa25c`,
tree `0f002df5625c56b2b095bb8e77f40ee2e13bf6e4`, built with Go 1.25.4.
The checked-in fixture in `tools/parity/fixtures/m5/v0.2.2-state-schema.json`
records the binary and state-source hashes. `tools/parity/capture-legacy-state.py`
reproduces capture from a clean `.parity/legacy-v0.2.2` checkout at that
revision and `.parity/bin/go-v0.2.2` built using:

```sh
GOTOOLCHAIN=go1.25.4 CGO_ENABLED=0 go build -trimpath -buildvcs=false \
  -ldflags='-s -w -buildid=' -o ../bin/go-v0.2.2 ./cmd/opute-host-agent
```

Each current binary upgrades its own identical database. The harness compares
the complete SQL schema and all seven state-table row sets. Independent seed
checks detect equal data loss, changed identity, altered terminal records,
and invented success. Only `updated_at` for the two interrupted records is
masked: both are changed to `unknown` by startup's `datetime('now')` update.
Created timestamps, hashes, revisions, bindings, provider metadata, and
terminal statuses compare exactly. No provider is activated and no tool is
called against shared infrastructure.

Reproduce from the repository root with the two current binaries available:

```sh
cd tools/parity
python3 -m parity migration --go ../../.parity/bin/go-reference \
  --rust ../../target/release/opute-host-agent --repeats 3 \
  --out ../../evidence/m5/migration
```

On this host `/tmp` is a full tmpfs; set `TMPDIR` to the repository-owned
`.parity/m5-tmp` directory on the workspace filesystem before running.

`summary.json` binds the source lock, binary hashes, driver, harness, fixture,
and `outcomes.json` checksum. The M5 verifier rechecks these and recomputes
the preservation assertions and differences from the raw observations. This
proves this migration matrix, not production conversion or rollback (M12).
