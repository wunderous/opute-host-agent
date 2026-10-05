.PHONY: spec-validate spec-list rust-check rust-build parity-test go-reference parity-capture parity-go-vs-go parity-go-vs-rust parity-canaries parity-oracles parity-manifest parity-verify parity-verify-m1 parity-verify-m2 parity-verify-m3 parity-verify-m4 parity-verify-cutover parity-m0 parity-m1 parity-m2 parity-m3 parity-m4 parity-m6 parity-corpus parity-ci parity-contracts parity-rust-canaries catalog-source

OPENSPEC := npx --yes @fission-ai/openspec@1.13.2
.PHONY: parity-verify-m5 parity-verify-m6
# Keep reference builds, patched canaries and client oracles on the same
# selected Go standard-library contract, even on hosts with a newer Go.
export GOTOOLCHAIN := $(shell python3 -c 'import json; print(json.load(open("baseline/source-lock.json"))["goToolchain"])')

spec-validate:
	$(OPENSPEC) validate --all --strict

spec-list:
	$(OPENSPEC) list --specs
	$(OPENSPEC) list

# --- Rust workspace (M1+).
RUST_BIN := $(CURDIR)/target/release/opute-host-agent

rust-check:
	cargo fmt --all -- --check
	cargo clippy --locked --all-targets -- -D warnings
	cargo test --locked

rust-build:
	cargo build --release --locked

# --- Parity harness (M0). Python 3.11 stdlib only; Go toolchain for the reference.
PARITY := cd tools/parity && python3 -m parity
GO_REF := $(CURDIR)/.parity/bin/go-reference
GO_SRC := $(CURDIR)/.parity/go-src

parity-test:
	cd tools/parity && python3 -m unittest discover -s tests

go-reference:
	PARITY_GO_WORKDIR=$(GO_SRC) PARITY_GO_OUT=$(GO_REF) tools/parity/build-go-reference.sh

parity-capture:
	$(PARITY) capture --go $(GO_REF) --go-src $(GO_SRC)

parity-go-vs-go:
	rm -rf evidence/current/go-vs-go
	$(PARITY) run --left go=$(GO_REF) --right go=$(GO_REF) --suite go-vs-go --repeat 20 --out $(CURDIR)/evidence/current/go-vs-go

# Exit status is ignored: surfaces owned by later milestones are expected to
# differ. The verifier decides what each gate requires.
parity-go-vs-rust:
	rm -rf evidence/current/go-vs-rust
	-$(PARITY) run --left go=$(GO_REF) --right rust=$(RUST_BIN) --suite go-vs-rust --repeat 5 --out $(CURDIR)/evidence/current/go-vs-rust

parity-canaries:
	$(PARITY) canaries --go $(GO_REF) --go-src $(GO_SRC) --out $(CURDIR)/evidence/current/canaries.json

parity-oracles:
	$(PARITY) oracle --go-src $(GO_SRC) --binary go=$(GO_REF) --binary rust=$(RUST_BIN) --out $(CURDIR)/evidence/current/go-oracles.json

# Declared divergences (decisions D8, D10, D11, D12): the Rust contract suites, and
# patched Rust builds that must turn them red.
CONTRACT_SUITES := oauth-issuance standalone-read-only-gate reject-without-audit-writes refuse-before-operation-record

parity-contracts:
	for s in $(CONTRACT_SUITES); do \
		(rm -rf evidence/current/contracts/$$s && \
		$(PARITY) contract --impl rust=$(RUST_BIN) --suite $$s --out $(CURDIR)/evidence/current/contracts/$$s) || exit 1; \
	done

parity-rust-canaries:
	$(PARITY) rust-canaries --rust $(RUST_BIN) --out $(CURDIR)/evidence/current/rust-canaries.json

parity-manifest:
	$(PARITY) manifest --go $(GO_REF) $(if $(wildcard $(RUST_BIN)),--rust $(RUST_BIN))

parity-verify:
	$(PARITY) verify --gate m0

parity-verify-m1:
	$(PARITY) verify --gate m1 --report $(CURDIR)/evidence/current/verify-m1.json

parity-verify-m2:
	$(PARITY) verify --gate m2 --report $(CURDIR)/evidence/current/verify-m2.json

parity-verify-m3:
	$(PARITY) verify --gate m3 --report $(CURDIR)/evidence/current/verify-m3.json

parity-verify-m4:
	$(PARITY) verify --gate m4 --report $(CURDIR)/evidence/current/verify-m4.json

parity-verify-m5:
	$(PARITY) verify --gate m5 --report $(CURDIR)/evidence/current/verify-m5.json

parity-verify-m6:
	$(PARITY) verify --gate m6 --report $(CURDIR)/evidence/current/verify-m6.json

parity-verify-cutover:
	$(PARITY) verify --gate cutover

# Regenerate the Rust catalog source from the pinned Go tree (M3).
catalog-source:
	$(PARITY) catalog-source --go-src $(GO_SRC)

# Regenerate scenarios/wire.json from parity/corpus.py (the harness tests fail
# when the committed corpus is stale).
parity-corpus:
	$(PARITY) corpus

parity-m0: parity-test go-reference parity-manifest parity-capture parity-go-vs-go parity-canaries parity-verify

parity-m1: rust-check rust-build parity-m0 parity-go-vs-rust parity-oracles parity-verify-m1

parity-m2: parity-m1 parity-contracts parity-rust-canaries parity-verify-m2

parity-m3: parity-m2 parity-verify-m3

parity-m4: parity-m3 parity-verify-m4

parity-m6: rust-check rust-build parity-m4 parity-go-vs-rust parity-verify-m6

# CI: verify the committed evidence, then re-prove parity on every surface
# with a freshly built Rust binary (fails on any diff).
parity-ci: parity-test go-reference rust-build
	$(PARITY) catalog-source --go-src $(GO_SRC) --check
	$(PARITY) verify --gate m4
	$(PARITY) run --left go=$(GO_REF) --right rust=$(RUST_BIN) --suite ci-go-vs-rust --repeat 2 --out $(CURDIR)/.parity/ci-go-vs-rust
	$(PARITY) oracle --go-src $(GO_SRC) --binary go=$(GO_REF) --binary rust=$(RUST_BIN) --out $(CURDIR)/.parity/ci-go-oracles.json
	for s in $(CONTRACT_SUITES); do ($(PARITY) contract --impl rust=$(RUST_BIN) --suite $$s) || exit 1; done
