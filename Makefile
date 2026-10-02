.PHONY: spec-validate spec-list rust-check rust-build parity-test go-reference parity-capture parity-go-vs-go parity-go-vs-rust parity-canaries parity-oracles parity-manifest parity-verify parity-verify-m1 parity-verify-m2 parity-verify-cutover parity-m0 parity-m1 parity-m2 parity-corpus parity-ci parity-contracts parity-rust-canaries catalog-source

OPENSPEC := npx --yes @fission-ai/openspec@1.13.2

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

# Declared divergences (decision D8): the Rust contract suite, and patched
# Rust builds that must turn it red.
parity-contracts:
	rm -rf evidence/current/contracts/oauth-issuance
	$(PARITY) contract --impl rust=$(RUST_BIN) --suite oauth-issuance --out $(CURDIR)/evidence/current/contracts/oauth-issuance

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

# CI: verify the committed evidence, then re-prove M1 and M2 parity with a
# freshly built Rust binary (fails on any diff in those surfaces).
parity-ci: parity-test go-reference rust-build
	$(PARITY) catalog-source --go-src $(GO_SRC) --check
	$(PARITY) verify --gate m2
	$(PARITY) run --left go=$(GO_REF) --right rust=$(RUST_BIN) --suite ci-go-vs-rust --repeat 2 --surfaces cli,config,lifecycle,http,mcp-wire,auth --out $(CURDIR)/.parity/ci-go-vs-rust
	$(PARITY) oracle --go-src $(GO_SRC) --binary go=$(GO_REF) --binary rust=$(RUST_BIN) --out $(CURDIR)/.parity/ci-go-oracles.json
	$(PARITY) contract --impl rust=$(RUST_BIN) --suite oauth-issuance
