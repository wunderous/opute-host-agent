#!/usr/bin/env python3
"""Generate scenarios/provider-real.json: the five pinned Go providers, unchanged.

Each scenario launches one pinned provider executable (built by
build-go-providers.sh) on a loopback port with the sandbox's recording shims
first on PATH and every outbound HTTP(S) request sent to a dead loopback
proxy, then drives the public lifecycle: install with the provider's own
activation recipe, status, the published catalog (the provider's M3 catalog
cell), one read-only provider call, validation, teardown and the catalog
after it. The agent's own process tree is observed before it stops: the core
never spawns a provider, so it must have no descendants.

Run from tools/parity:
  python3 provider_real_scenarios.py scenarios/provider-real.json
"""
import json, sys

import yaml
from pathlib import Path

from provider_lifecycle_scenarios import RESTART, SERVE, MASKS, SETS, STATE, call, task_done, wait

GO_SRC = Path(__file__).resolve().parents[2] / ".parity/go-src/plugins"
HOST_TOOLS = 142
PROVIDERS = [
    # (plugin directory, provider id, the manifest's first recipe (the one Go
    # selects when no mode is given), published operation count, read-only
    # call and its arguments, extra validation arguments)
    ("platform/hostos", "com.opute.hostos", None, 2, "opute.capability.host-platform.detect", {}, {}),
    ("llm/ollama", "com.opute.ollama", "recipes/activate.yaml", 3, "opute.capability.llm-serving.get-context-size", {},
     {"endpoint": "http://127.0.0.1:9"}),
    ("kubernetes/k3s", "com.opute.k3s", "recipes/kubernetes.yaml", 28, "opute.capability.kubernetes.validate", {}, {}),
    ("tunneling/cloudflare", "com.opute.cloudflare", "recipes/activate-tunneling.yaml", 13,
     "opute.capability.tunneling.validate", {"bindings": [{"bindingId": "fixture", "localTarget": "http://127.0.0.1:3080"}], "placement": "host"}, {"targetUri": "vm:local:fixture-vm"}),
    # Every published Tailscale tool binds a resource URI first, so its
    # journey runs with the recording Incus inventory and lists it (below).
    ("tunneling/tailscale", "com.opute.tailscale", "recipes/activate.yaml", 13,
     "opute.capability.mesh-runtime.status", {"targetUri": "container:local:worker-01", "hostAgentId": "${AGENT_ID}"}, {"targetUri": "vm:local:fixture-vm"}),
]


def host_info_masks():
    """M3's typed get_host_info masks (host-read.host-info.inventory-mix),
    re-rooted at the activation node's output in the run and task results."""
    source = json.load(open(Path(__file__).resolve().parent / "scenarios/host-read.json"))
    [base] = [s for s in source if s["id"] == "host-read.host-info.inventory-mix"]
    out = []
    for mask in base["compare"]["masks"]:
        path = mask["path"]
        if "structuredContent" not in path or "text" in path:
            continue
        tail = path[path.index("structuredContent") + 1:]
        for root in HOST_INFO_ROOTS:
            out.append(dict(mask, path=root + tail, optional=True))
    return out


def host_info_omitempty():
    """M3's omitempty rules for the same output (PSI averages are omitempty
    in Go: a zero sample is omitted, so absent and 0 are one value),
    re-rooted like host_info_masks."""
    source = json.load(open(Path(__file__).resolve().parent / "scenarios/host-read.json"))
    [base] = [s for s in source if s["id"] == "host-read.host-info.inventory-mix"]
    out = []
    for rule in base["compare"].get("omitempty", []):
        path = rule["path"]
        if "structuredContent" not in path or "text" in path:
            continue
        tail = path[path.index("structuredContent") + 1:]
        for root in HOST_INFO_ROOTS:
            out.append(dict(rule, path=root + tail))
    return out


# Where the activation node's get_host_info output appears: in the run (and
# its task result) per node and again under the run's outputs.
HOST_INFO_ROOTS = [prefix + inner
                   for prefix in (["steps", "*", "body", "result", "structuredContent"],
                                  ["steps", "*", "body", "result", "result", "structuredContent"])
                   for inner in (["nodes", "*", "output"], ["outputs", "*"], ["state", "nodes", "*", "output"], ["state", "outputs", "*"])]
TOOL_SETS = [root + ["supportedTools"] for root in HOST_INFO_ROOTS]


def descriptor(plugin):
    """The plugin's own descriptor, with the endpoint moved to the scenario's port."""
    doc = yaml.safe_load((GO_SRC / plugin / "plugin.yaml").read_text())
    doc["server"] = {"transport": doc["server"]["transport"], "endpoint": "http://127.0.0.1:${PROVIDER_PORT}/mcp"}
    return doc


def scenario(plugin, provider, recipe_uri, operations, operation, arguments, validation):
    name = plugin.split("/")[1]
    recipe = (GO_SRC / plugin / recipe_uri).read_text() if recipe_uri else None
    install_args = {"descriptor": descriptor(plugin), "activate": True}
    steps = [SERVE,
             {"as": "provider", "providerProcess": {"binary": "opute-provider-" + name, "portVar": "PROVIDER_PORT"}},
             {"as": "catalogBefore", "mcp": {"method": "tools/list", "params": {}}}]
    if recipe is not None:
        steps.append({"as": "recipe", "write": {"path": "${SANDBOX}/recipes/activate.yaml", "content": recipe}})
        install_args["recipeSource"] = "${SANDBOX}/recipes/activate.yaml"
    steps += [call("install", "opute.provider.install", install_args,
                   capture={"RUN_ID": ["body", "result", "structuredContent", "runId"]}),
              wait("installed"), task_done("installTask", "RUN_ID"),
              call("status", "opute.provider.status", {"provider": provider}),
              {"as": "catalog", "mcp": {"method": "tools/list", "params": {}}},
              *([call("inventory", "list_vms", {"fast": True})] if name == "tailscale" else []),
              call("call", operation, arguments),
              call("validate", "opute.provider.validate", {"provider": provider, **validation}),
              call("teardown", "opute.provider.teardown", {"provider": provider, "confirm": True, "inputs": {"hostService": "none"}},
                   capture={"TEARDOWN_RUN": ["body", "result", "structuredContent", "runId"]}),
              call("tornDown", "get_host_plan_run", {"runId": "${TEARDOWN_RUN}"},
                   until={"path": ["body", "result", "structuredContent", "status"], "in": ["completed", "failed", "unknown", "cancelled"]}),
              task_done("teardownTask", "TEARDOWN_RUN"),
              call("statusAfter", "opute.provider.status", {"provider": provider}),
              {"as": "catalogAfter", "mcp": {"method": "tools/list", "params": {},
                                             "until": {"path": ["body", "result", "tools", "$len"], "in": [HOST_TOOLS, HOST_TOOLS + operations]}, "timeout": 15}},
              STATE,
              {"as": "agentProcesses", "agentProcesses": {}},
              {"as": "stop", "stop": {}},
              {"as": "providerStopped", "providerProcessStop": {"process": "provider"}}]
    for step in steps:
        if step.get("as") == "tornDown":
            step["requires"] = ["TEARDOWN_RUN"]
    return {"id": "provider-real." + name, "surface": "provider-real", "owner": "provider",
            "anchors": ["internal/hostmcp/provider_install.go:handleProviderInstallContext", "plugins/" + plugin],
            "fixture": "incus-two-containers" if name == "tailscale" else None, "collect": ["orphans"], "steps": steps,
            "compare": {"sets": SETS + TOOL_SETS, "masks": MASKS + host_info_masks(),
                        "omitempty": host_info_omitempty(),
                        "notes": "One pinned Go provider executable, unchanged, launched by the harness on loopback with recording shims and a dead egress proxy; the core never spawns providers."}}


def restart_scenario():
    """k3s across an agent restart: the provider process keeps running (the
    core never spawns or relaunches one), the restarted agent restores the
    generation and its catalog, reconnects, serves calls and tears down."""
    base = scenario(*PROVIDERS[2])
    steps = base["steps"]
    cut = [s.get("as") for s in steps].index("validate")
    restart = [{"as": "stop1", "stop": {}}, RESTART,
               call("statusRestored", "opute.provider.status", {"provider": "com.opute.k3s"}),
               {"as": "catalogRestored", "mcp": {"method": "tools/list", "params": {},
                                                 "until": {"path": ["body", "result", "tools", "$len"], "in": [HOST_TOOLS + 28]}, "timeout": 15}},
               call("callRestored", "opute.capability.kubernetes.validate", {})]
    base["steps"] = steps[:cut] + restart + steps[cut:]
    base["id"] = "provider-real.k3s.restart"
    return base


def with_d19(case):
    """D19 (neutral-host-tool-names): every case observes the host catalog
    or a catalog revision, which Rust publishes under the neutral names."""
    case["compare"]["divergences"] = case["compare"].get("divergences", []) + ["D19.neutral-host-tool-names"]
    return case


if __name__ == "__main__":
    json.dump([with_d19(scenario(*p)) for p in PROVIDERS] + [with_d19(restart_scenario())],
              open(sys.argv[1], "w"), indent=1)
