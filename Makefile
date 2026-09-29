.PHONY: spec-validate spec-list

OPENSPEC := npx --yes @fission-ai/openspec@1.13.2

spec-validate:
	$(OPENSPEC) validate --all --strict

spec-list:
	$(OPENSPEC) list --specs
	$(OPENSPEC) list
