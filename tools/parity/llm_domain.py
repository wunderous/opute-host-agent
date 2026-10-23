#!/usr/bin/env python3
"""Inventory the Go llm and serving domains for M8f:
baseline/inventory/llm-domain.json and baseline/inventory/serving-domain.json.

Run from the repository root: python3 tools/parity/llm_domain.py

The same projection host_domain.py makes for svc.Host(), applied to every
tool whose Go handler calls svc.Llm() (dispatch_llm.go, and the host tool
probe_openai_compatible_server) or svc.Serving() (dispatch_serving.go).

The llm domain runs its host commands three ways: the shared host command
runner (systemctl, bash, the ollama and llama-server binaries,
nvidia-smi), the host workload runner (the llama-server build), and
exec.Command directly (systemctlUser, nvidia-smi in the llama probe). Its
cluster proxy reaches the kubernetes domain (Deps), which applies and
deletes through the active Kubernetes provider. The serving domain runs
exec.Command("systemctl"/"kill") directly and the rest through its Deps:
the host domain's agent shell and service state, the incus domain's VM
info, and kubectl through the provider. effects.json records what each
tool is observed running.
"""
import json
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import cluster_domain as cd  # noqa: E402
import host_domain as hd  # noqa: E402

DOMAINS = {
    "llm": {
        "out": hd.ROOT / "baseline/inventory/llm-domain.json",
        "dir": hd.GO / "internal/domain/llm",
        "accessor": "Llm",
        "family": {
            "check_local_llm_prerequisites": "local-llm-runtime",
            "list_local_llm_models": "local-llm-runtime", "probe_local_llm": "local-llm-runtime",
            "install_local_llm_model": "local-llm-runtime", "configure_local_llm_model": "local-llm-runtime",
            "start_local_llm_runtime": "local-llm-runtime", "stop_local_llm_runtime": "local-llm-runtime",
            "ensure_local_llm_server_binary": "local-llm-server-binary",
            "ensure_local_llm_relay": "local-llm-relay", "remove_local_llm_relay": "local-llm-relay",
            "ensure_local_llm_k3s_proxy": "local-llm-cluster-proxy",
            "remove_local_llm_k3s_proxy": "local-llm-cluster-proxy",
            "probe_openai_compatible_server": "openai-compatible-probe",
        },
        # Cross-domain seams (Deps) and the provider operation each reaches.
        "deps": {"KubernetesTargetURI": None, "ApplyManifest": "KubernetesApplyManifestOperation",
                 "DeleteK8sResource": "KubernetesDeleteResourceOperation"},
        "note": ("Static inventory of the Go llm domain. commandsStatic names the host commands the domain "
                 "runs (through the shared host command and workload runners, and exec.Command directly); "
                 "dependencies are its kubernetes-domain seams and providerOperations the Kubernetes "
                 "provider operations they reach; envReads are the domain's own configuration reads; "
                 "effects.json records what each tool is observed running."),
    },
    "serving": {
        "out": hd.ROOT / "baseline/inventory/serving-domain.json",
        "dir": hd.GO / "internal/domain/serving",
        "accessor": "Serving",
        "family": {"reconcile_serving_assignment": "serving-assignment",
                   "discover_service_ingress": "service-ingress"},
        # IngressLoadBalancer is runKubernetesKubectl (exec-command).
        "deps": {"RunAgentShell": None, "SetHostServiceState": None, "BridgeIP": None,
                 "IngressLoadBalancer": "KubernetesExecCommandOperation"},
        "note": ("Static inventory of the Go serving domain. commandsStatic names the host commands the "
                 "domain runs itself (exec.Command); dependencies are its Deps seams into the host, incus "
                 "and kubernetes domains, and providerOperations the Kubernetes provider operations they "
                 "reach; effects.json records what each tool is observed running."),
    },
}

COMMANDS = {"systemctl", "bash", "kill", "nvidia-smi", "ollama", "ldd", "strings", "cmake", "ninja", "nvcc",
            "ss", "tar", "install", "systemd-run"}
LITERAL = re.compile(r'"([a-z][a-z0-9-]*)"')
# Commands whose name is not a literal at the call: the configured binaries.
NAMED = {"cfg.BinaryPath, \"pull\"": "ollama", "outputPath, \"--version\"": "llama-server",
         "nvidiaSmiCommand()": "nvidia-smi"}


def main() -> None:
    ops = cd.operations()
    cells = hd.catalog_cells()
    omitted = hd.excluded()
    registry = {e["name"]: e for e in json.loads((hd.ROOT / "baseline/inventory/dispatch-registry.json")
                                                 .read_text())["items"]}
    values = json.loads((hd.ROOT / "crates/host-agent/catalog/neutral-names.json").read_text())["values"]
    source = json.loads((hd.ROOT / "baseline/source-lock.json").read_text())["sourceCommit"]
    for domain, spec in DOMAINS.items():
        functions = hd.go_functions(spec["dir"])
        tools = []
        for reg in hd.registrations(spec["accessor"]):
            reached = hd.reach(functions, reg["entry"])
            commands, env, provider, deps, anchors = set(), set(), set(), set(), []
            for name in reached:
                fn = functions[name]
                body = fn["body"]
                anchors.append(f'{fn["file"]}:{fn["line"]}:{name}')
                runs = ("exec.Command" in body or "HostCommandRunner" in body
                        or "HostWorkloadRunner" in body or "systemctlUser(" in body
                        or "runSystemctlUser(" in body)
                if runs:
                    commands.update(c for c in LITERAL.findall(body) if c in COMMANDS)
                    commands.update(c for needle, c in NAMED.items() if needle in body)
                    if "systemctlUser(" in body or "runSystemctlUser(" in body:
                        commands.add("systemctl")
                for dep, op in spec["deps"].items():
                    if f"s.deps.{dep}(" in body:
                        deps.add(dep)
                        if op:
                            provider.add(ops[op])
                env.update(hd.ENV.findall(body))
                if "os.Getenv(name)" in body:
                    # A loop over a literal list of names (ollamaContextSizeFromEnvironment).
                    env.update(re.findall(r'"((?:OPUTE|OLLAMA)_[A-Z0-9_]+)"', body))
            tool = reg["tool"]
            tools.append({
                **{k: reg[k] for k in ("tool", "effect", "admissionClass", "taskMode", "anchor")},
                "domain": domain,
                "neutralName": hd.NEUTRAL.get(tool, tool),
                "neutralValues": {field: values["map"] for field in values["tools"].get(tool, [])},
                "family": spec["family"].get(tool, "unassigned"),
                "domainFunctions": anchors,
                "commandsStatic": sorted(commands),
                "dependencies": sorted(deps),
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
            sys.exit(f"{domain}: tools without a service family: {unassigned}")
        missing = sorted(set(spec["family"]) - {t["tool"] for t in tools})
        if missing:
            sys.exit(f"{domain}: families name tools Go does not register: {missing}")
        doc = {
            "surface": f"{domain}-domain",
            "owner": domain,
            "milestone": "M8f",
            "source": source,
            "note": spec["note"],
            "families": sorted({t["family"] for t in tools}),
            "tools": sorted(tools, key=lambda t: (t["family"], t["tool"])),
        }
        spec["out"].write_text(json.dumps(doc, indent=1, sort_keys=True) + "\n")
        print(f"wrote {spec['out'].relative_to(hd.ROOT)}: {len(tools)} tools in {len(doc['families'])} families")


if __name__ == "__main__":
    main()
