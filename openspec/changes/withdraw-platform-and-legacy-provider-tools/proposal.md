# Withdraw four published tools

Status: **accepted (owner, 2026-11-11) as D37.** Goal m15.

## Why

The catalog publishes four tools that Go v0.2.4 never handled and Rust
answers with `not_implemented`. Each describes work the Host Agent does not
own:

- `register_kubernetes_cluster` persists a record "in the bridge database":
  registration is Platform state (AGENTS.md: Platform owns intent,
  orchestration and routing).
- `list_agents` lists agents "connected to the bridge server": Platform's
  view, not a host's.
- `install_provider_tools` and `uninstall_provider_tools` switch or drop the
  host's active provider; `opute.provider.install`, `opute.provider.teardown`
  and `opute.provider.status` already own the provider lifecycle.

## What Changes

The four tools are removed from the published catalog in every profile, and
their unported seam registrations go with them. A call by name gets the
MCP unknown-tool error, as any unpublished name does. This is a contract
change, so the catalog revision changes.

## Impact

`crates/host-agent/catalog/source.json` and the derived catalog, the seam
registrations and gaps, the parity manifest's catalog revision, the site
preview at publication (5.2).
