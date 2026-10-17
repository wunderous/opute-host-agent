#!/usr/bin/env python3
"""Inventory the Go kubernetes and cluster domains for M8c:
baseline/inventory/cluster-domain.json.

Run from the repository root: python3 tools/parity/cluster_domain.py

The same projection host_domain.py makes for svc.Host(), applied to every
tool whose Go handler calls svc.Kubernetes() or svc.Cluster(). The
kubernetes domain never runs kubectl itself: every cluster access is a
synchronous call to the active provider that declares the kubernetes
capability (hostmcp/kubernetes_provider.go). So providerOperations lists the
opute.capability.kubernetes.* operations a tool reaches, commandsStatic
lists the host commands (helm, through the host's ensure_host_tool), and
guestArgv records the cluster domain's RunVMExec guest commands, which run
through incus exec. The shim traces (T1) and the k3s twins (T2) record the
observed calls.
"""
import json
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import host_domain as hd  # noqa: E402

OUT = hd.ROOT / "baseline/inventory/cluster-domain.json"
DIRS = {"Kubernetes": hd.GO / "internal/domain/kubernetes", "Cluster": hd.GO / "internal/domain/cluster"}
CAPABILITY = hd.GO / "contracts/capability/capability.go"

# Service Definitions the two domains are ported as (D14).
FAMILY = {
    "list_kubernetes_clusters": "cluster-discovery", "list_clusters": "cluster-discovery",
    "get_cluster_details": "cluster-discovery", "get_cluster_runtime_details": "cluster-discovery",
    "apply_manifest": "cluster-resources", "get_k8s_resource": "cluster-resources",
    "delete_k8s_resource": "cluster-resources", "get_k8s_resource_status": "cluster-resources",
    "list_k8s_events": "cluster-resources", "put_k8s_secret": "cluster-resources",
    "exec_kubernetes_command": "cluster-resources",
    "install_helm_chart": "cluster-packages", "uninstall_helm_chart": "cluster-packages",
    "render_helm_template": "cluster-packages",
    "configure_service_domain": "service-exposure", "remove_service_domain": "service-exposure",
    "inspect_guest_storage": "guest-storage", "trim_guest_storage": "guest-storage",
    "prune_unused_cluster_images": "guest-storage", "garbage_collect_cluster_registry": "guest-storage",
    "install_cluster_agent": "cluster-agent",
    **{t: "cluster-listings" for t in ("list_namespaces", "list_storage_classes", "list_ingress_classes",
                                       "list_certificate_issuers", "list_pods", "list_services",
                                       "list_deployments")},
}
# RunKubectl* reach the provider's exec-command operation.
KUBECTL = re.compile(r"\b(?:RunKubectl\w*|runProviderCommand|runKubernetesKubectl)\(")
GUEST = re.compile(r'RunVMExec\(\w+, \[\]string\{"([a-zA-Z0-9_.-]+)"')
# Keys read in a loop over a literal list (os.Getenv(key)) or through
# hostruntime.EnvOr; a systemd unit's Environment= line is not a read.
ENV_LITERAL = re.compile(r'(?:\[\]string\{|, )"(OPUTE_[A-Z0-9_]+)"|EnvOr\("([A-Z][A-Z0-9_]+)"')
HOST_TOOL = re.compile(r'(?:EnsureHostTool|lookupHostTool)\("([a-z0-9_.-]+)"')


def operations() -> dict[str, str]:
    """Go constant name -> provider operation id."""
    text = CAPABILITY.read_text()
    return dict(re.findall(r'(Kubernetes\w+Operation)\s*=\s*"([a-z0-9.\-]+)"', text))


def main() -> None:
    ops = operations()
    functions = {}
    for directory in DIRS.values():
        for name, fn in hd.go_functions(directory).items():
            functions.setdefault(name, fn)
    cells = hd.catalog_cells()
    omitted = hd.excluded()
    registry = {e["name"]: e for e in json.loads((hd.ROOT / "baseline/inventory/dispatch-registry.json")
                                                 .read_text())["items"]}
    tools = []
    for domain in DIRS:
        for reg in hd.registrations(domain):
            reached = hd.reach(functions, reg["entry"])
            commands, env, guest, provider, anchors = set(), set(), set(), set(), []
            for name in reached:
                fn = functions[name]
                body = fn["body"]
                anchors.append(f'{fn["file"]}:{fn["line"]}:{name}')
                commands.update(c for c in hd.EXEC.findall(body) if c in hd.COMMANDS)
                commands.update(HOST_TOOL.findall(body))
                guest.update(GUEST.findall(body))
                provider.update(ops[c] for c in re.findall(r"\b(Kubernetes\w+Operation)\b", body) if c in ops)
                if KUBECTL.search(body):
                    provider.add(ops["KubernetesExecCommandOperation"])
                env.update(hd.ENV.findall(body))
                env.update(k for pair in ENV_LITERAL.findall(body) for k in pair if k)
            if guest:
                commands.add("incus")
            tool = reg["tool"]
            tools.append({
                **{k: reg[k] for k in ("tool", "effect", "admissionClass", "taskMode", "anchor")},
                "domain": domain.lower(),
                "neutralName": hd.NEUTRAL.get(tool, tool),
                "family": FAMILY.get(tool, "unassigned"),
                "domainFunctions": anchors,
                "commandsStatic": sorted(commands),
                "guestArgv": sorted(guest),
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
        "surface": "cluster-domain",
        "owner": "kubernetes",
        "milestone": "M8c",
        "source": json.loads((hd.ROOT / "baseline/source-lock.json").read_text())["sourceCommit"],
        "note": ("Static inventory of the Go kubernetes and cluster domains. providerOperations are the "
                 "Kubernetes provider operations a tool reaches (kubectl runs inside the provider); "
                 "commandsStatic is a lower bound of host commands; guestArgv names the commands the "
                 "cluster domain runs in a guest through incus exec; effects.json records what each tool "
                 "is observed running. reachability is the dispatch registry's status: an unreachable "
                 "tool is registered but never published, and Rust refuses it as Go does."),
        "families": sorted({t["family"] for t in tools}),
        "tools": sorted(tools, key=lambda t: (t["family"], t["tool"])),
    }
    OUT.write_text(json.dumps(doc, indent=1, sort_keys=True) + "\n")
    print(f"wrote {OUT.relative_to(hd.ROOT)}: {len(tools)} tools in {len(doc['families'])} families")


if __name__ == "__main__":
    main()
