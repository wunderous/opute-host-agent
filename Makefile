.PHONY: spec-validate spec-list parity-test go-reference parity-capture parity-go-vs-go parity-canaries parity-manifest parity-verify parity-verify-cutover parity-m0

OPENSPEC := npx --yes @fission-ai/openspec@1.13.2

spec-validate:
	$(OPENSPEC) validate --all --strict

spec-list:
	$(OPENSPEC) list --specs
	$(OPENSPEC) list

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
	rm -rf evidence/m0/go-vs-go
	$(PARITY) run --left go=$(GO_REF) --right go=$(GO_REF) --suite go-vs-go --repeat 20 --out $(CURDIR)/evidence/m0/go-vs-go

parity-canaries:
	$(PARITY) canaries --go $(GO_REF) --go-src $(GO_SRC) --out $(CURDIR)/evidence/m0/canaries/results.json

parity-manifest:
	$(PARITY) manifest --go $(GO_REF)

parity-verify:
	$(PARITY) verify --gate m0 --report $(CURDIR)/evidence/m0/verify-report.json

parity-verify-cutover:
	$(PARITY) verify --gate cutover

parity-m0: parity-test go-reference parity-manifest parity-capture parity-go-vs-go parity-canaries parity-verify
