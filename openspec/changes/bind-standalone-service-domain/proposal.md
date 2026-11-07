# Bind the standalone service domain tools to their VM (proposed)

Status: proposed. Owner question Q28
([rust-spec-authority/questions.md](../rust-spec-authority/questions.md)).
Nothing here is implemented.

## Why

e2e-4a journey `e2e3.site` maps the deployed site to a test hostname with
`configure_service_domain`, as `tools/list` publishes it in standalone
mode: `vmName`, `namespace`, `ingressName`, `hostname`, `serviceName` and
`servicePort`, all required. With every field given, the call fails:

    Error: vmName, namespace, ingressName, hostname, serviceName, and positive servicePort are required

The handler reads the VM from the call's resource binding
(`vmNameFromBinding`), never from the `vmName` argument. The standalone
descriptor declares no resource requirement, so admission builds no
binding and the VM name is empty. `remove_service_domain` fails the same
way. The full catalog's descriptor takes a cluster `uri` and declares
the binding, but standalone mode lists the `vmName` form.

Go v0.2.4 behaves the same (`tools/standalone.go` lists the `vmName`
schema; `tools/dispatch_kubernetes.go` reads the binding). As published,
neither tool can succeed in standalone mode. The journey applies the same
Ingress through `apply_manifest` instead.

## What Changes

If the owner chooses it, one of:
- standalone mode lists the descriptor that declares the cluster `uri`
  binding, so the schema and the handler agree; or
- the standalone handler resolves the published `vmName` to its instance.

Until the owner decides, Rust keeps Go's behaviour.

## Impact

- `crates/host-agent/catalog/source.json` (standalone definitions) or
  `crates/host-agent/src/builtin.rs` (`service_exposure`).
- `e2e3.site` then maps the hostname through `configure_service_domain`.
