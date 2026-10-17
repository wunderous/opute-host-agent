# Cluster formatting oracle

`main.go` copies, verbatim, the pure formatting functions of the pinned Go
kubernetes and cluster domains (ace7013d): `renderHelmChartManifest`,
`HelmValuesYAML`, `clusterAgentConfig`'s marshalling,
`renderClusterAgentInstallScript`, `wrapHostPrivilegedShellBody` and
`k8sAge` (with `now` injected). `expected.json` is its output; the Rust crate
test `clusterops::tests::formatting_matches_go` compares against it.

Regenerate with the pinned toolchain:

    cd tools/parity/cluster-format-oracle
    GOTOOLCHAIN=local ../../../.parity/setup/go/bin/go run main.go > expected.json
