.PHONY: supply-chain fuzz fuzz-seeds reproducible-build spec-validate spec-list rust-check rust-build release-toolchain artifacts parity-test go-reference go-providers parity-capture parity-go-vs-go parity-go-vs-rust parity-canaries parity-oracles parity-manifest parity-verify parity-verify-m1 parity-verify-m2 parity-verify-m3 parity-verify-m4 parity-verify-cutover parity-m0 parity-m1 parity-m2 parity-m3 parity-m4 parity-m6 parity-corpus parity-ci rust-suites e2e-test cutover-readiness e2e-devvm-build e2e-verify parity-contracts parity-rust-canaries catalog-source

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

# --- Hardening (m16; evidence/hardening/README.md).
# cargo-deny policy (deny.toml); cargo-deny, cargo-fuzz and cargo-cyclonedx
# are installed under .parity/tools/cargo-m16 locally, by CI in CI.
FUZZ_SECONDS ?= 20

supply-chain:
	cargo deny check

# Every fuzz target for FUZZ_SECONDS each on the stable toolchain; the stable
# replay of seeds and regressions runs in `cargo test` (fuzzing::tests).
fuzz:
	fuzz/run.sh $(FUZZ_SECONDS)

fuzz-seeds:
	python3 fuzz/make_seeds.py

# Two clean, offline, locked builds of the release binary must be equal.
reproducible-build:
	tools/release/reproducible-build.sh

# --- Release set (M10): Go's names, gzip and SHA256SUMS; see tools/release.
VERSION ?= 0.2.4

release-toolchain:
	tools/release/setup-toolchain.sh

artifacts:
	VERSION=$(VERSION) DIST=$(CURDIR)/dist tools/release/build-artifacts.sh

# --- Parity harness (M0). Python 3.11 stdlib only; Go toolchain for the reference.
PARITY := cd tools/parity && python3 -m parity
GO_REF := $(CURDIR)/.parity/bin/go-reference
GO_SRC := $(CURDIR)/.parity/go-src

parity-test:
	cd tools/parity && python3 -m unittest discover -s tests

go-reference:
	PARITY_GO_WORKDIR=$(GO_SRC) PARITY_GO_OUT=$(GO_REF) tools/parity/build-go-reference.sh

# The five pinned Go provider executables (.parity/bin/providers), built
# reproducibly from the checkout go-reference pins.
go-providers: go-reference
	PARITY_GO_WORKDIR=$(GO_SRC) tools/parity/build-go-providers.sh

parity-capture:
	$(PARITY) capture --go $(GO_REF) --go-src $(GO_SRC)

parity-go-vs-go:
	rm -rf evidence/current/go-vs-go
	$(PARITY) run --left go=$(GO_REF) --right go=$(GO_REF) --suite go-vs-go --repeat 3 --out $(CURDIR)/evidence/current/go-vs-go

# Exit status is ignored: surfaces owned by later milestones are expected to
# differ. The verifier decides what each gate requires.
parity-go-vs-rust:
	rm -rf evidence/current/go-vs-rust
	-$(PARITY) run --left go=$(GO_REF) --right rust=$(RUST_BIN) --suite go-vs-rust --repeat 1 --out $(CURDIR)/evidence/current/go-vs-rust

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

# CI (D27): the Rust-only suites. The harness unit tests, then every contract
# suite against a freshly built Rust binary, then the e2e-0 gate re-derived
# from its committed receipts. The canaries
# (parity-rust-canaries) and the go-sdk OAuth client
# (tools/parity/oauth-sdk-client) are slower local suites.
RUST_SUITES := $(CONTRACT_SUITES) evidence-redaction

rust-suites: parity-test e2e-test rust-build
	for s in $(RUST_SUITES); do ($(PARITY) contract --impl rust=$(RUST_BIN) --suite $$s) || exit 1; done
	python3 evidence/e2e/verify.py --gate e2e-0 > /dev/null

# --- E2E dev VM and journeys (e2e-0; evidence/e2e/README.md).
E2E := python3 evidence/e2e
E2E_PARITY := $(shell git rev-parse --path-format=absolute --git-common-dir | xargs dirname)/.parity
E2E_RELEASE ?= $(E2E_PARITY)/e2e/release/rust
E2E_GATE ?= e2e-0

e2e-test:
	python3 -m unittest discover -s evidence/e2e/tests
	python3 -m unittest discover -s evidence/cutover/tests

# Cutover readiness from committed evidence only (D28): every item passed,
# unverified (with its question) or blocked (with its owner); a missing,
# stale or tampered item fails.
cutover-readiness:
	python3 evidence/cutover/readiness.py

# One command: the Rust release set, then the VM (needs the dev-vm lease).
e2e-devvm-build:
	SETUP=$(E2E_PARITY)/setup DIST=$(E2E_RELEASE) tools/release/build-artifacts.sh
	$(E2E)/devvm/devvm.py build --rust-release $(E2E_RELEASE)

e2e-verify:
	$(E2E)/verify.py --gate $(E2E_GATE)

# Historical (before D27): verify the committed evidence, then re-prove
# parity on every surface against the Go reference.
parity-ci: parity-test go-reference rust-build
	$(PARITY) catalog-source --go-src $(GO_SRC) --check
	$(PARITY) verify --gate m4
	$(PARITY) run --left go=$(GO_REF) --right rust=$(RUST_BIN) --suite ci-go-vs-rust --repeat 1 --out $(CURDIR)/.parity/ci-go-vs-rust
	$(PARITY) oracle --go-src $(GO_SRC) --binary go=$(GO_REF) --binary rust=$(RUST_BIN) --out $(CURDIR)/.parity/ci-go-oracles.json
	for s in $(CONTRACT_SUITES); do ($(PARITY) contract --impl rust=$(RUST_BIN) --suite $$s) || exit 1; done
