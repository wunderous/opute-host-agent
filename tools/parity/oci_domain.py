#!/usr/bin/env python3
"""Inventory the Go oci domain for M8d: baseline/inventory/oci-domain.json.

Run from the repository root: python3 tools/parity/oci_domain.py

The same projection host_domain.py makes for svc.Host(), applied to every
tool whose Go handler calls svc.Oci(). The domain resolves its image tools
by name (ContainerLookPath) and runs them by the resolved path, so
commandsStatic lists those names, and sudo and apt-get for the builder's
package install (RunPrivilegedPackage). The registry tools never run
kubectl: they reach the active Kubernetes provider through the kubernetes
domain, so providerOperations lists the operations they reach. The shim
traces (T1) and the twins (T2) record the observed calls.
"""
import json
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import cluster_domain as cd  # noqa: E402
import host_domain as hd  # noqa: E402

OUT = hd.ROOT / "baseline/inventory/oci-domain.json"
DIR = hd.GO / "internal/domain/oci"

# Service Definitions the domain is ported as (D14).
FAMILY = {
    "install_oci_registry": "oci-registry", "get_oci_registry_status": "oci-registry",
    "delete_oci_registry": "oci-registry",
    "ensure_oci_builder": "oci-builder",
    "stage_build_context": "build-context",
    "build_and_push_oci_image": "image-build",
    "configure_oci_storage": "container-storage", "inspect_container_storage": "container-storage",
    "cleanup_container_storage": "container-storage",
}
LOOKUP = re.compile(r'ContainerLookPath\("([a-z0-9_.-]+)"\)')
# Candidates looked up in a loop: `for _, c := range []string{"a", "b"}`.
CANDIDATES = re.compile(r'range \[\]string\{([^}]*)\}')
PRIVILEGED = re.compile(r'RunPrivilegedPackage\(\w+, "([a-z0-9_.-]+)"')
# Each kubernetes dependency the domain calls, and the provider operation
# the kubernetes domain reaches for it.
DEPS = {"ApplyManifest": "KubernetesApplyManifestOperation",
        "GetK8sResource": "KubernetesGetResourceOperation",
        "DeleteK8sResource": "KubernetesDeleteResourceOperation",
        "KubernetesTargetURI": None}


def main() -> None:
    ops = cd.operations()
    functions = hd.go_functions(DIR)
    cells = hd.catalog_cells()
    omitted = hd.excluded()
    registry = {e["name"]: e for e in json.loads((hd.ROOT / "baseline/inventory/dispatch-registry.json")
                                                 .read_text())["items"]}
    values = json.loads((hd.ROOT / "crates/host-agent/catalog/neutral-names.json").read_text())["values"]
    tools = []
    for reg in hd.registrations("Oci"):
        reached = hd.reach(functions, reg["entry"])
        commands, env, provider, deps, anchors = set(), set(), set(), set(), []
        for name in reached:
            fn = functions[name]
            body = fn["body"]
            anchors.append(f'{fn["file"]}:{fn["line"]}:{name}')
            commands.update(LOOKUP.findall(body))
            if "ContainerLookPath(candidate)" in body:
                for group in CANDIDATES.findall(body):
                    commands.update(re.findall(r'"([a-z0-9_.-]+)"', group))
            for command in PRIVILEGED.findall(body):
                commands.update(("sudo", command))
            for dep, op in DEPS.items():
                if f"s.deps.{dep}(" in body:
                    deps.add(dep)
                    if op:
                        provider.add(ops[op])
            env.update(hd.ENV.findall(body))
        tool = reg["tool"]
        tools.append({
            **{k: reg[k] for k in ("tool", "effect", "admissionClass", "taskMode", "anchor")},
            "domain": "oci",
            "neutralName": hd.NEUTRAL.get(tool, tool),
            "neutralValues": {field: values["map"] for field in values["tools"].get(tool, [])},
            "family": FAMILY.get(tool, "unassigned"),
            "domainFunctions": anchors,
            "commandsStatic": sorted(commands),
            "kubernetesDependencies": sorted(deps),
            "providerOperations": sorted(provider),
            "envReads": sorted(env),
            "publishedIn": sorted(c for c, names in cells.items() if tool in names),
            "reachability": registry.get(tool, {}).get("status"),
            "catalogExclusion": omitted.get(tool),
            "goTests": hd.tests_for(tool, reg["entry"]),
            "rustServes": hd.rust_serves(tool),
        })
    unassigned = [t["tool"] for t in tools if t["family"] == "unassigned"]
    if unassigned:
        sys.exit(f"tools without a service family: {unassigned}")
    doc = {
        "surface": "oci-domain",
        "owner": "oci",
        "milestone": "M8d",
        "source": json.loads((hd.ROOT / "baseline/source-lock.json").read_text())["sourceCommit"],
        "note": ("Static inventory of the Go oci domain. commandsStatic names the image tools the domain "
                 "resolves by name and runs by the resolved path, and sudo and apt-get for a builder's "
                 "package install; providerOperations are the Kubernetes provider operations the registry "
                 "tools reach through the kubernetes domain; neutralValues is the D23 selector value map; "
                 "effects.json records what each tool is observed running."),
        "families": sorted({t["family"] for t in tools}),
        "tools": sorted(tools, key=lambda t: (t["family"], t["tool"])),
    }
    OUT.write_text(json.dumps(doc, indent=1, sort_keys=True) + "\n")
    print(f"wrote {OUT.relative_to(hd.ROOT)}: {len(tools)} tools in {len(doc['families'])} families")


if __name__ == "__main__":
    main()
