"""Capture the machine-readable parity inventory from the pinned Go tree.

Two sources feed the inventory:

* the pinned Go source checkout (static facts with file:line anchors), and
* the Go reference binary, run through the same scenarios the twin harness
  uses (observed facts).

Every inventory item carries an owner and the scenarios that cover it. An item
without a covering scenario is written as an explicit gap, never dropped.
Output is deterministic: rerunning against the same source and binary
reproduces the files byte for byte.
"""

from __future__ import annotations

import concurrent.futures
import hashlib
import json
import re
import subprocess
from pathlib import Path
from typing import Any

from . import agent, canon, runner

INVENTORY_DIR = runner.REPO_ROOT / "baseline" / "inventory"
# Runs per recipe validation (finding F-4): 16 once saw a single variant of
# a two-variant message (M10 recapture), so the sample is larger.
PLAN_RUNS = 64

# Owner of each surface; matches the ownership candidates in design.md.
SURFACE_OWNERS = {
    "cli": "cli",
    "config": "config",
    "http": "transport",
    "lifecycle": "server",
    "auth": "transport",
    "mcp-wire": "transport",
    "catalog": "contracts",
    "read-only-tools": "host",
    "admission": "core",
    "durable-state": "state",
    "effects": "host",
    "plans": "core",
    "tasks": "core",
    "recipe-corpus": "core",
    "host-read": "host",
    "providers": "core",
    "distribution": "release",
    "cutover": "release",
}

ENV_PATTERN = re.compile(r'"((?:OPUTE|HOST_MCP)_[A-Z0-9_]+|MCP_AUTH_TOKEN)"')
EXEC_PATTERN = re.compile(
    r'(?:exec\.Command(?:Context)?\((?:ctx, |[a-zA-Z]+, )?|LookPath\(|RunHost\(\[\]string\{|'
    r'RunPrivileged(?:Package)?\((?:ctx, )?)"([a-zA-Z0-9_.-]+)"'
)
ROUTE_PATTERN = re.compile(r'HandleFunc\(("(?P<lit>[^"]+)"|(?P<fn>[A-Za-z]+)\(\))')
TABLE_PATTERN = re.compile(r"CREATE TABLE IF NOT EXISTS (\w+)")
TOOLNAME_PATTERN = re.compile(r'^\s*[A-Z][A-Za-z0-9]*\s*=\s*"([a-z0-9_.]+)"', re.M)


def _go_files(src: Path, *roots: str) -> list[Path]:
    files: list[Path] = []
    for root in roots:
        files += [p for p in sorted((src / root).rglob("*.go")) if not p.name.endswith("_test.go")]
    return files


def _anchor(src: Path, path: Path, text: str, offset: int) -> str:
    line = text.count("\n", 0, offset) + 1
    return f"{path.relative_to(src)}:{line}"


def _scenario_refs(scenarios: list[dict]) -> dict[str, str]:
    """Scenario id -> canonical JSON text, for coverage lookups.

    A step's profile environment is part of what it runs, so it is included.
    The single-implementation contract suites are the owning evidence for
    surfaces a decision moved off Go-vs-Rust comparison (D8's OAuth
    issuance), so their scenarios count too, as `contract:<suite>/<id>`.
    """
    refs = {}
    for s in scenarios:
        profiles = [runner.PROFILES[step[kind].get("profile", "empty")]
                    for step in s.get("steps", []) for kind in ("serve", "cli") if kind in step]
        refs[s["id"]] = json.dumps([s, profiles], sort_keys=True)
    for path in sorted((runner.TOOLS_DIR / "contracts").glob("*.json")):
        for s in json.loads(path.read_text())["scenarios"]:
            refs[f"contract:{path.stem}/{s['id']}"] = json.dumps(s, sort_keys=True)
    return refs


def _covering(needle: str, refs: dict[str, str]) -> list[str]:
    quoted = json.dumps(needle)
    return sorted(sid for sid, text in refs.items() if quoted in text or needle in text)


def verify_source(src: Path, lock: dict) -> None:
    head = subprocess.run(["git", "-C", str(src), "rev-parse", "HEAD", "HEAD^{tree}"],
                          capture_output=True, text=True, check=True).stdout.split()
    if head != [lock["sourceCommit"], lock["sourceTree"]]:
        raise SystemExit(f"Go source at {src} is {head}, expected the source lock")
    dirty = subprocess.run(["git", "-C", str(src), "status", "--porcelain"],
                           capture_output=True, text=True, check=True).stdout
    if dirty.strip():
        raise SystemExit(f"Go source at {src} has local changes; capture refuses a dirty tree")


# Variables pinned Go reads that are not M1-M7 surfaces. Each stays in the
# inventory with its owner; an inert one has no observable effect in Go.
ENV_DOMAIN = {
    "OPUTE_DOGFOOD_GATEWAY_VM": "internal/domain/cluster (cluster agent bridge relay)",
    "OPUTE_K3S_CONTAINER": "internal/domain/cluster (cluster agent bridge relay)",
    "OPUTE_K3S_VM": "internal/domain/cluster (cluster agent bridge relay)",
    "OPUTE_PLATFORM_GUEST_HOST": "internal/domain/cluster (cluster agent)",
    "OPUTE_PLATFORM_PUBLIC_URL": "internal/domain/cluster (cluster agent)",
    "OPUTE_PLATFORM_URL": "internal/domain/cluster (cluster agent)",
    "OPUTE_HOST_AGENT_ELEVATED": "internal/domain/host (firewall)",
    "OPUTE_HOST_AGENT_RELAY_DIR": "internal/domain/postgres (relay configuration directory)",
    "OPUTE_HOST_AGENT_SQLITE_ROOT": "internal/domain/postgres (local SQLite databases)",
    "OPUTE_SHARED_HOST_OWNER_INSTANCE": "internal/domain/incus (shared incus stack ownership)",
    "OPUTE_LLAMA_SERVER_ARTIFACT_PATH": "internal/domain/llm (llama server)",
    "OPUTE_LLAMA_SERVER_BASE_MODEL": "internal/domain/llm (llama server)",
    "OPUTE_LLAMA_SERVER_BINARY": "internal/domain/llm (llama server)",
    "OPUTE_LLAMA_SERVER_BUILD": "internal/domain/llm (llama server)",
    "OPUTE_LLAMA_SERVER_MODEL": "internal/domain/llm (llama server)",
    "OPUTE_LLAMA_SERVER_PORT": "internal/domain/llm (llama server)",
    "OPUTE_LLAMA_SERVER_VERSION": "internal/domain/llm (llama server)",
    "OPUTE_OLLAMA_BINARY": "internal/domain/llm (host ollama runtime)",
    "OPUTE_OLLAMA_CONTEXT_SIZE": "internal/domain/llm (host ollama runtime)",
    "OPUTE_OLLAMA_MODEL": "internal/domain/llm (host ollama runtime)",
    "OPUTE_OLLAMA_PORT": "internal/domain/llm (host ollama runtime)",
}
ENV_INERT = {
    "OPUTE_TEST": "sets Config.TestMode, which nothing in pinned Go reads",
    "OPUTE_MCP_ROUTE_HOST": "read only by mcphttp.ApplyMcpRouteHost, which nothing in pinned Go calls",
    "OPUTE_HOST_MAX_QUEUED_OPERATIONS": "bounds Coordinator.AcquireClass's queue, which nothing in pinned Go "
                                        "calls; a non-positive or non-numeric value falls back to 16, so "
                                        "Config.Validate never sees a negative one",
}


def capture_env(src: Path, refs: dict[str, str]) -> dict:
    found: dict[str, list[str]] = {}
    for path in _go_files(src, "internal", "pkg", "cmd"):
        text = path.read_text()
        for m in ENV_PATTERN.finditer(text):
            found.setdefault(m.group(1), []).append(_anchor(src, path, text, m.start()))
    items = []
    for name in sorted(found):
        covered = _covering(name, refs)
        item = {
            "name": name,
            "owner": "config",
            "anchors": sorted(set(found[name])),
            "scenarios": covered,
            "status": "covered" if covered else "gap",
        }
        if not covered and name in ENV_INERT:
            item.update(status="inert", inertReason=ENV_INERT[name])
        elif not covered and name in ENV_DOMAIN:
            item.update(milestone="M8", gapReason=f"read only by {ENV_DOMAIN[name]}; domain operations are M8")
        items.append(item)
    return {"surface": "config", "owner": "config", "items": items}


def capture_routes(src: Path, refs: dict[str, str]) -> dict:
    resolved = {"AuthorizationServerMetadataPath": "/.well-known/oauth-authorization-server"}
    items = []
    for path in _go_files(src, "internal"):
        text = path.read_text()
        for m in ROUTE_PATTERN.finditer(text):
            route = m.group("lit") or resolved.get(m.group("fn"), f"<unresolved:{m.group('fn')}>")
            covered = _covering(route, refs)
            items.append({
                "route": route,
                "owner": "transport",
                "anchors": [_anchor(src, path, text, m.start())],
                "scenarios": covered,
                "status": "covered" if covered else "gap",
            })
    return {"surface": "http", "owner": "transport", "items": sorted(items, key=lambda i: i["route"])}


# Commands whose effects M8a (host) does not own: they stay owned gaps of
# the sub-milestone whose domain runs them (milestones.md, M8 table).
EFFECT_OWNERS = {
    "buildah": ("M8d", "oci", "run only by internal/domain/oci (image build); OCI effects are M8d"),
    "podman": ("M8d", "oci", "run only by internal/domain/oci (builder and container runtime); OCI effects are M8d"),
    "ollama": ("M8f", "llm", "run only by internal/domain/llm (Ollama runtime); LLM effects are M8f"),
    "helm": ("M8c", "kubernetes", "host-domain code only looks helm up (ensure_host_tool) and installs it "
                                  "through the remote installer script, observed as bash in T1; helm itself "
                                  "runs in internal/domain/kubernetes (install_helm_chart), M8c"),
    "kill": ("M8f", "serving", "run only by internal/domain/serving (assignment liveness probe, kill -0); "
                               "serving effects are M8f"),
}
# Observed commands whose real effect T2 deliberately does not execute,
# with the reason. T1 still proves their argv.
T2_EXCLUDED = {
    "bash": "the only host-domain bash runs are ensure_host_tool's bun and helm installers, which pipe a "
            "third-party script fetched from the internet into bash; T2 never executes remote code, so "
            "these stay T1 shim-observed",
    "helm": "ensure_host_tool only looks helm up and installs it through the remote installer script (see bash)",
}
T2_SCENARIO_DIR = runner.REPO_ROOT / "tools" / "parity" / "scenarios-t2"


def _elevated(entry: dict) -> str | None:
    """The command a `sudo -n CMD ...` trace entry runs, by base name."""
    if entry.get("cmd") != "sudo":
        return None
    for arg in entry.get("argv", []):
        if not arg.startswith("-"):
            return arg.rsplit("/", 1)[-1]
    return None


def _t2_commands() -> dict[str, list[str]]:
    """T2 scenarios that execute a command's real effect, keyed by command."""
    out: dict[str, list[str]] = {}
    for path in sorted(T2_SCENARIO_DIR.glob("*.json")):
        for scenario in json.loads(path.read_text()):
            for cmd in scenario.get("commands", []):
                out.setdefault(cmd, []).append(scenario["id"])
    return out


def capture_effects(src: Path, observed: list[dict]) -> dict:
    found: dict[str, list[str]] = {}
    for path in _go_files(src, "internal", "pkg") + _go_files(src, "plugins"):
        text = path.read_text()
        for m in EXEC_PATTERN.finditer(text):
            found.setdefault(m.group(1), []).append(_anchor(src, path, text, m.start()))
    observed_cmds: dict[str, set[str]] = {}
    for entry in observed:
        observed_cmds.setdefault(entry["cmd"], set()).add(" ".join(entry["argv"][:2]))
        inner = _elevated(entry)
        if inner:
            # Recorded under the elevated command too, with its sudo argv.
            observed_cmds.setdefault(inner, set()).add("sudo " + " ".join(entry["argv"][:3]))
    t2 = _t2_commands()
    items = []
    for name in sorted(set(found) | set(observed_cmds)):
        item = {
            "command": name,
            "owner": "host",
            "anchors": sorted(set(found.get(name, []))),
            "observedInvocations": sorted(observed_cmds.get(name, [])),
            "t2Scenarios": sorted(t2.get(name, [])),
        }
        if name in observed_cmds:
            # T1 proves argv through the PATH (or sudo) shim; a T2 scenario
            # additionally executes the real effect in the disposable sandbox.
            item.update(status="covered", tier="T2-sandbox" if name in t2 else "T1-shim-observed")
            if name not in t2 and name in T2_EXCLUDED:
                item["t2Excluded"] = T2_EXCLUDED[name]
        elif name in EFFECT_OWNERS:
            milestone, owner, reason = EFFECT_OWNERS[name]
            item.update(status="gap", tier="unclassified", owner=owner, milestone=milestone, gapReason=reason)
        else:
            item.update(status="gap", tier="unclassified", milestone="M8",
                        gapReason="host effect tiering of domain operations is M8")
        items.append(item)
    return {
        "surface": "effects",
        "owner": "host",
        "note": "Static scan finds literal command names only. Commands built from variables "
                "(for example hostexec wrappers) are found through shim traces as scenarios grow; "
                "a command run through `sudo -n` is recorded under its own name too. Tiers: "
                "T1-shim-observed (argv proved by shim trace parity), T2-sandbox (also executed for real "
                "in the twin T2 sandboxes, scenarios listed). A command never observed stays a gap.",
        "items": items,
    }


# Mutating host tools no T2 scenario can execute here, with owner,
# milestone and the reason. T1 shim parity still covers their argv.
T2_UNVERIFIED = {
    "compact_wsl_disk": ("M8a", "needs a WSL2 runner: the effect is a VHDX compact on the Windows host through "
                                "wsl.exe and PowerShell; the Linux T2 sandbox has no WSL"),
    "shutdown_wsl": ("M8a", "needs a WSL2 runner: wsl.exe --shutdown acts on the Windows host's WSL VM"),
    "terminate_wsl_distribution": ("M8a", "needs a WSL2 runner: wsl.exe --terminate acts on a Windows-hosted "
                                          "distribution"),
}
# Mutating host tools that cannot be called at all.
T2_INERT = {
    "ensure_k3d": "omitted from every Go catalog (IncusOmittedToolNames) and unregistered in Rust; no call reaches it",
    "configure_service_domain": "Go takes the cluster VM only from the call's resource binding "
                                "(vmNameFromBinding), and its catalog entry declares no resource argument, so every "
                                "call refuses with 'vmName, namespace, ingressName, hostname, serviceName, and "
                                "positive servicePort are required' before reaching the provider; the refusal is "
                                "compared in T1 (cluster-domain.exposure) and in T2 (t2.cluster-domain.resources.lifecycle, "
                                "whose snapshot shows no Ingress)",
    "remove_service_domain": "as configure_service_domain: no resource binding, so every call refuses with "
                             "'vmName, namespace, and ingressName are required' before reaching the provider",
    "uninstall_helm_chart": "registered but never published in any Go catalog (dispatch registry: unreachable); "
                            "every call is an unknown tool on both sides (cluster-domain.unreachable)",
    "install_postgresql": "Go takes the cluster VM only from the call's resource binding (vmNameFromBinding), "
                          "and its catalog entry declares no resource argument, so every call refuses with "
                          "'vmName is required' before reaching the provider; compared in T1 "
                          "(postgres-domain.standalone) and in T2 (t2.postgres-domain.admission.rejected-no-effect)",
    "delete_postgresql": "as install_postgresql: no resource binding, so every call refuses with 'kubernetes "
                         "provider instance name is required' before reaching the provider",
    "run_sql": "as install_postgresql: no resource binding, so every call refuses with 'vmName and sql are "
               "required' before reaching the provider",
    "ensure_local_llm_k3s_proxy": "Go takes the cluster VM only from the call's resource binding "
                                  "(vmNameFromBinding), and its catalog entry declares no resource argument, so "
                                  "every call refuses with 'vmName is invalid' before rendering or applying "
                                  "anything; compared in T1 (llm-domain.cluster-proxy) and in T2 "
                                  "(t2.llm-domain.admission.rejected-no-effect)",
    "remove_local_llm_k3s_proxy": "as ensure_local_llm_k3s_proxy: no resource binding, so every call refuses "
                                  "with 'vmName is invalid' before deleting anything",
    "exec_command": "catalog-excluded (CatalogExcludedToolNames), so Go resolves it no resource binding and every "
                    "call refuses with 'name is required' before running anything; t2.incus-domain."
                    "container.lifecycle calls it by name and by URI against a live owned container and checks "
                    "inside the guest that nothing ran",
}


# The sub-milestone that owns a domain's T2 coverage (milestones.md, M8 table).
T2_MILESTONE = {"host": "M8a", "incus": "M8b", "kubernetes": "M8c", "oci": "M8d", "postgres": "M8e",
                "llm": "M8f", "serving": "M8f"}


def capture_t2() -> dict:
    """T2 coverage of every mutating host-, incus-, cluster-, oci-, postgres-, llm- and serving-domain tool
    (M8 validations 2-4)."""
    domains = [json.loads((INVENTORY_DIR / f"{d}-domain.json").read_text())
               for d in ("host", "incus", "cluster", "oci", "postgres", "llm", "serving")]
    calls: dict[str, set[str]] = {}
    refused: dict[str, set[str]] = {}
    for path in sorted(T2_SCENARIO_DIR.glob("*.json")):
        for scenario in json.loads(path.read_text()):
            # A rejectedOnly scenario proves calls change nothing; it executes no tool.
            target = refused if scenario.get("rejectedOnly") else calls
            for step in scenario["steps"]:
                mcp = step.get("mcp") or {}
                if mcp.get("method") == "tools/call":
                    target.setdefault(mcp["params"]["name"], set()).add(scenario["id"])
    items = []
    tools = [(doc["owner"], tool) for doc in domains for tool in doc["tools"]]
    for owner, tool in sorted(tools, key=lambda t: t[1]["tool"]):
        if tool["effect"] == "EffectRead":
            continue
        name = tool["tool"]
        covered = sorted(calls.get(name, set()) | calls.get(tool.get("neutralName") or name, set()))
        item = {"tool": name, "neutralName": tool.get("neutralName"), "owner": owner, "effect": tool["effect"],
                "t2Scenarios": covered, "t2RejectedScenarios": sorted(refused.get(name, set()))}
        if name in T2_INERT:
            item.update(status="inert", inertReason=T2_INERT[name])
        elif name in T2_UNVERIFIED:
            milestone, reason = T2_UNVERIFIED[name]
            item.update(status="gap", milestone=milestone, gapReason="T2 unverified: " + reason)
        elif covered:
            item["status"] = "covered"
        else:
            item.update(status="gap", milestone=T2_MILESTONE.get(owner, "M8b"),
                        gapReason="no T2 scenario executes this tool")
        items.append(item)
    return {"surface": "t2-effects", "owner": "host",
            "note": "Each mutating host- and incus-domain tool and the T2 scenarios that execute it in the twin "
                    "sandboxes (incus-domain scenarios boot the nested-Incus golden). "
                    "Coverage is proven by passing T2 evidence (verify gate requireT2), not by this listing.",
            "items": items}


def capture_toolnames(src: Path) -> list[str]:
    text = (src / "internal/contract/toolname/names.go").read_text()
    return sorted(set(TOOLNAME_PATTERN.findall(text)))


def capture_state_static(src: Path) -> list[dict]:
    items = []
    for path in _go_files(src, "internal"):
        text = path.read_text()
        for m in TABLE_PATTERN.finditer(text):
            items.append({"table": m.group(1), "anchor": _anchor(src, path, text, m.start())})
    return sorted(items, key=lambda i: (i["table"], i["anchor"]))


def _descriptor_row(tool: dict) -> dict:
    meta = tool.get("_meta", {}).get("capability", {})
    return {
        "name": tool.get("name"),
        "title": tool.get("title"),
        "effect": meta.get("effect"),
        "privilege": meta.get("privilege"),
        "requiresApproval": meta.get("requiresApproval"),
        "idempotency": meta.get("idempotency"),
        "provider": meta.get("provider"),
        "resourceCost": tool.get("_meta", {}).get("resourceCost") or meta.get("resourceCost"),
        "taskSupport": (tool.get("execution") or {}).get("taskSupport"),
        "descriptorSha256": hashlib.sha256(canon.canonical_json(tool).encode()).hexdigest(),
    }


def capture_catalog_cell(observation: dict, variables: dict[str, str], cell: dict) -> dict:
    doc = canon.substitute(observation, variables)
    result = doc["steps"][cell.get("step", "list")]["body"]["result"]
    tools = sorted(result["tools"], key=lambda t: t["name"])
    return {
        "surface": "catalog",
        "owner": "contracts",
        "cell": cell,
        "toolCount": len(tools),
        "resultMeta": {k: v for k, v in result.items() if k != "tools"},
        "tools": [_descriptor_row(t) for t in tools],
        "descriptors": tools,
    }


def _findings(catalogs: dict[str, dict]) -> list[dict]:
    findings = []
    # G-1 (legacy-inventory.md): confirm against the live catalog.
    legacy_desc = "LEGACY compatibility wrapper"
    for cell, cat in sorted(catalogs.items()):
        flagged = sorted(t["name"] for t in cat["descriptors"]
                         if legacy_desc in (t.get("description") or ""))
        if flagged:
            findings.append({
                "id": f"G-1/{cell}",
                "kind": "apparent-go-defect",
                "summary": "local-LLM tools are published with the generic LEGACY wrapper description",
                "evidence": {"cell": cell, "tools": flagged},
                "decision": "open: reproduce exactly in Rust, or fix in both implementations",
            })
    findings.append({
        "id": "F-6",
        "kind": "descriptor-authority",
        "summary": "the published apply_manifest description comes from embedded schemas/incus-tools.json; "
                   "the same text in internal/tools/catalog.go and schemas/all-tools.json, and a different "
                   "text in internal/tools/standalone.go, never reach the wire",
        "evidence": {"canary": "C1-descriptor-text", "method": "patch each source in turn; only "
                     "incus-tools.json changes tools/list"},
        "decision": "supports legacy-inventory I-1/I-2: Rust generates descriptors from one source; M0 must "
                    "extend this per-tool authority check to every descriptor before M3",
    })
    findings.append({
        "id": "F-7",
        "kind": "inherited-surface",
        "summary": "the Go agent inherits go-sdk MCPGODEBUG compatibility switches from its environment "
                   "(disablelocalhostprotection, allowsessionsinstateless, disablecontenttypecheck, "
                   "noprotocolerrorbody, nowrapinvalidparams, ...); one of them disables the SDK's "
                   "DNS-rebinding guard",
        "evidence": {"source": "go-sdk v1.7.0 internal/mcpgodebug, mcp/streamable.go"},
        "decision": "D9: Rust implements the default behaviour (Go with MCPGODEBUG unset) and does not "
                    "reproduce the switches; owner to confirm",
    })
    findings.append({
        "id": "F-2",
        "kind": "nondeterminism",
        "summary": "get_host_info.supportedTools order varies between runs (Go map iteration); "
                   "compared as a set",
        "evidence": {"scenario": "mcp.get-host-info"},
        "decision": "membership is contract, order is not; Rust may emit any order",
    })
    findings.append({
        "id": "F-3",
        "kind": "message-quirk",
        "summary": "OPUTE_TRANSPORT=stdio is rejected with the text 'invalid --transport \"stdio\"' "
                   "even when the value came from the environment",
        "evidence": {"scenario": "startup.fail-closed", "step": "transportEnvStdio"},
        "decision": "reproduce exactly (error text is observable)",
    })
    return findings


def _plan_findings(plans: dict) -> list[dict]:
    flaky = [i["recipe"] for i in plans["items"] if i["cliValidate"].get("nondeterministic")]
    if not flaky:
        return []
    return [{
        "id": "F-4",
        "kind": "nondeterminism",
        "summary": "recipe validate names a different missing required input between runs "
                   "(map iteration order)",
        "evidence": {"recipes": flaky},
        "decision": "open: Rust must report one of the same variants; a deterministic order "
                    "would be an evidenced Go defect fixed in both implementations",
    }]


def _registry_findings(registry: list[dict]) -> list[dict]:
    hidden = [r["name"] for r in registry if r["status"] == "gap"]
    if not hidden:
        return []
    return [{
        "id": "F-5",
        "kind": "unpublished-dispatch",
        "summary": "dispatch-registered tool names that appear in no captured catalog and are not "
                   "in CatalogExcludedToolNames",
        "evidence": {"tools": hidden},
        "decision": "open: determine whether tools/call can reach them (hidden surface) or they are "
                    "provider-conditional; add a scenario before M3",
    }]


def capture(go_binary: Path, src: Path) -> dict[str, Any]:
    lock = json.loads((runner.REPO_ROOT / "baseline" / "source-lock.json").read_text())
    verify_source(src, lock)
    impl = agent.Impl(label="go", binary=go_binary)
    scenarios = runner.load_scenarios()
    refs = _scenario_refs(scenarios)
    by_id = {s["id"]: s for s in scenarios}

    observed: dict[str, tuple[dict, dict]] = {}
    for s in scenarios:
        # Inventory must preserve a failing reference boundary as an owned
        # gap, rather than discard all other observations. Milestone runs
        # still fail on the same execution error; capture never blesses it.
        observed[s["id"]] = runner._execute_safe(impl, s, "a", "capture")

    trace: list[dict] = []
    for obs, _ in observed.values():
        trace += obs.get("trace", [])

    cells = {
        "standalone": {"mode": "standalone", "providers": [], "mutations": False, "prefixedNames": False,
                       "scenario": "catalog.standalone"},
        "standalone-mutations": {"mode": "standalone", "providers": [], "mutations": True,
                                 "prefixedNames": False, "scenario": "catalog.standalone-mutations-enabled"},
        "standalone-prefixed": {"mode": "standalone", "providers": [], "mutations": False,
                                "prefixedNames": True, "scenario": "catalog.standalone-prefixed-names"},
        "platform": {"mode": "platform", "providers": [], "mutations": False, "prefixedNames": False,
                     "scenario": "catalog.platform"},
    }
    # Provider cells: the catalog each real provider journey publishes once
    # its install has run (hostos: Go's install refuses it, and the cell is
    # that unchanged catalog). The ollama provider is out of the Rust
    # migration (owner decision D15), so its cell is not inventoried.
    for provider in ["k3s", "cloudflare", "tailscale", "hostos"]:
        cells[f"provider-{provider}"] = {"mode": "standalone", "providers": [provider], "mutations": True,
                                         "prefixedNames": False, "scenario": f"provider-real.{provider}",
                                         "step": "catalog"}
    catalogs = {}
    for name, cell in cells.items():
        obs, variables = observed[cell["scenario"]]
        catalogs[name] = capture_catalog_cell(obs, variables, cell)

    # Cross-check: every dispatch-registry name must be in some captured
    # catalog or be an explicit gap with a reason.
    dispatch = capture_toolnames(src)
    published = set()
    for cat in catalogs.values():
        published |= {canon.substitute(t["name"], {}) for t in cat["descriptors"]}
    published_plain = {n.split("_", 1)[1] if n.startswith("${TOOL_PREFIX}_") else n for n in published}
    excluded_text = (src / "internal/tools/catalog.go").read_text()
    registry = []
    for tool in dispatch:
        in_catalog = tool in published_plain
        excluded = f'"{tool}":' in excluded_text.split("CatalogExcludedToolNames", 1)[-1].split("}\n", 1)[0]
        refused = _refused_as_unknown(tool, observed, by_id)
        registry.append({
            "name": tool,
            "owner": "contracts",
            "publishedIn": sorted(c for c, cat in catalogs.items()
                                  if tool in {t["name"].replace("${TOOL_PREFIX}_", "") for t in cat["descriptors"]}),
            "catalogExcluded": excluded,
            **({"unreachableIn": refused} if refused and not in_catalog else {}),
            "status": "covered" if in_catalog else ("excluded-by-design" if excluded
                                                    else "unreachable" if refused else "gap"),
        })

    state_obs = canon.substitute(*observed["state.schema-after-start"])
    inventory: dict[str, Any] = {
        "runtime-gaps.json": {
            "surface": "runtime-observations",
            "items": [
                {"name": sid, "owner": SURFACE_OWNERS[by_id[sid]["surface"]],
                 "status": "gap", "gapReason": obs["_executionError"]}
                for sid, (obs, _) in sorted(observed.items()) if "_executionError" in obs
            ],
        },
        "cli.json": {
            "surface": "cli", "owner": "cli",
            "items": [
                {"scenario": sid, "owner": SURFACE_OWNERS[by_id[sid]["surface"]],
                 "observed": canon.substitute(*observed[sid])["steps"]}
                for sid in sorted(by_id) if by_id[sid]["surface"] in ("cli", "config")
            ],
        },
        "env.json": capture_env(src, refs),
        "http.json": capture_routes(src, refs),
        "effects.json": capture_effects(src, trace),
        "state.json": {
            "surface": "durable-state", "owner": "state",
            "static": capture_state_static(src),
            "observedAfterStart": state_obs.get("sqlite", {}),
            "files": state_obs.get("files", []),
        },
        "dispatch-registry.json": {"surface": "catalog", "owner": "contracts", "items": registry},
        "findings.json": {"items": _findings(catalogs)},
    }
    for name, cat in catalogs.items():
        inventory[f"catalog/{name}.json"] = cat
    inventory["plans.json"] = capture_plans(impl, src, scenarios)
    inventory["findings.json"]["items"] += _plan_findings(inventory["plans.json"])
    inventory["findings.json"]["items"] += _registry_findings(registry)
    inventory["distribution.json"] = capture_distribution(src)
    inventory["t2.json"] = capture_t2()
    inventory["cutover.json"] = capture_cutover()
    inventory["gaps.json"] = collect_gaps(inventory, catalogs)
    inventory["index.json"] = {
        "sourceCommit": lock["sourceCommit"],
        "sourceTree": lock["sourceTree"],
        "goBinarySha256": impl.sha256(),
        "harnessSha256": runner.harness_sha256(),
        "files": {},  # filled by write()
    }
    return inventory


def _refused_as_unknown(tool: str, observed: dict, by_id: dict) -> list[str]:
    """Modes in which a direct tools/call of `tool` is refused as unknown."""
    modes = set()
    for sid, (obs, _) in observed.items():
        mode = None
        for step in by_id[sid].get("steps", []):
            if "serve" in step:
                mode = next((a.split("=", 1)[1] for a in step["serve"].get("argv", [])
                             if a.startswith("--mode=")), mode)
            call = step.get("mcp") or {}
            if call.get("method") != "tools/call" or call.get("name") != tool:
                continue
            body = (obs.get("steps", {}).get(step["as"]) or {}).get("body") or {}
            if (body.get("error") or {}).get("message") == f'unknown tool "{tool}"' and mode:
                modes.add(mode)
    # A dispatch name counts as unreachable only when every serving mode refuses it.
    return sorted(modes) if modes >= {"standalone", "platform"} else []


# Recipes of the ollama provider, which owner decision D15 takes out of the
# Rust migration; they leave the M6 and M7 inventory.
PLANS_OUT_OF_SCOPE = {
    "plugins/llm/ollama/recipes/activate.yaml",
    "plugins/llm/ollama/recipes/ollama-external.yaml",
    "plugins/llm/ollama/recipes/ollama.yaml",
    "test/fixtures/recipes/ollama.yaml",
}


def capture_plans(impl: agent.Impl, src: Path, scenarios: list[dict] | None = None) -> dict:
    items = []
    recipes = sorted(p for p in src.rglob("*.yaml")
                     if "recipes" in p.parts and "manifests" not in p.parts)
    for path in recipes:
        rel = str(path.relative_to(src))
        # Go reports the first missing input found while iterating a map, so
        # the message can vary between runs (finding F-4). Record every
        # variant observed over PLAN_RUNS runs instead of one sample.
        def once(_: int, path: Path = path) -> str:
            sandbox = agent.Sandbox(side="a", run_id="plans", fixture=None)
            result = agent.run_cli(impl, sandbox, ["recipe", "validate", "--source", str(path)],
                                   dict(runner.PROFILES["standalone"]))
            return canon.canonical_json(canon.substitute(result, {**sandbox.variables, "GO_SRC": str(src)}))

        with concurrent.futures.ThreadPoolExecutor(max_workers=8) as pool:
            variants = set(pool.map(once, range(PLAN_RUNS)))
        observed = [json.loads(v) for v in sorted(variants)]
        item = {
            "recipe": rel,
            "owner": "core",
            "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
            "cliValidate": observed[0] if len(observed) == 1 else {"nondeterministic": True,
                                                                   "variants": observed},
        }
        # Covered when a scenario validates this file through the public
        # recipe tool, or installs and runs its exact text (a real provider
        # journey writes the provider's own recipe). Node states, retry and
        # compensation are proven by the M6 fault scripts; the domain actions
        # a recipe invokes are M8.
        text = path.read_text()
        covering = sorted(s["id"] for s in scenarios or []
                          if rel in json.dumps(s) or any(
                              (step.get("write") or {}).get("content") == text for step in s.get("steps", [])))
        if rel in PLANS_OUT_OF_SCOPE:
            item.update(status="out-of-scope", decision="D15")
        elif covering:
            item.update(status="covered", scenarios=covering)
        else:
            item.update(status="gap", milestone="M6",
                        gapReason="no scenario validates or runs this recipe")
        items.append(item)
    return {"surface": "plans", "owner": "core", "items": items}


# M10 coverage of each distribution surface (evidence/m10/dist, parity dist
# run): what the distribution round proves, or why it cannot here.
_DIST_COVERED = {
    "host-agent-linux-x64": "release name, gzip, SHA256SUMS, ELF linkage, glibc floor and version output; "
                            "installed, started under systemd, called and removed on clean Ubuntu 24.04 and "
                            "Debian 12 twins; Go -> Rust -> Go on one identity",
    "host-agent-linux-arm64": "release name, gzip, SHA256SUMS, static ELF linkage for aarch64",
    "opute-provider-cloudflare-linux-x64": "built unchanged from the pinned Go tree with Go's target (D2, M9)",
    "opute-provider-k3s-linux-x64": "built unchanged from the pinned Go tree with Go's target (D2, M9)",
    "opute-provider-tailscale-linux-x64": "built unchanged from the pinned Go tree with Go's target (D2, M9)",
}
_WINDOWS_GAP = ("needs a Windows build: Go's own Windows build needs modernc.org/libc's Windows files, absent "
                "from the pinned offline module cache, and the Rust Windows target is not built or run here")


def capture_distribution(src: Path) -> dict:
    makefile = (src / "Makefile").read_text()
    artifacts = sorted(set(re.findall(r"\$\(DIST\)/((?:host-agent|opute-provider)-[a-z0-9_.-]+)", makefile)))
    package = json.loads((src / "npm/local-host-agent/package.json").read_text())
    deploy = {
        str(p.relative_to(src)): hashlib.sha256(p.read_bytes()).hexdigest()
        for p in sorted((src / "deploy").iterdir()) if p.is_file()
    }
    items = []
    for name in artifacts:
        if name in _DIST_COVERED:
            items.append({"name": name, "kind": "artifact", "owner": "release", "status": "covered",
                          "coverage": _DIST_COVERED[name]})
        else:
            items.append({"name": name, "kind": "artifact", "owner": "release", "status": "gap",
                          "milestone": "M10", "gapReason": _WINDOWS_GAP})
    for path in deploy:
        items.append({"name": path, "kind": "deploy", "owner": "release", "status": "covered",
                      "coverage": "installed as published on clean Ubuntu 24.04 and Debian 12 twins; unit "
                                  "status, paths, modes and journal lines match Go"})
    items.append({"name": package["name"], "kind": "npm", "owner": "release", "status": "covered",
                  "coverage": "packed locally; index.test.js, the readonly canary, download and verify from a "
                              "local release base, checksum mismatch, the cache lock and unsupported platforms"})
    items.append({"name": "site", "kind": "site", "owner": "release", "status": "covered",
                  "coverage": "Go's check-site on the committed site; the Rust preview site equals Go's once the "
                              "declared D19-D24 renames are reversed; the remaining files are reviewed"})
    items.append({"name": "linux-arm64 clean-machine install", "kind": "install", "owner": "release",
                  "status": "gap", "milestone": "M10",
                  "gapReason": "needs an arm64 runner: the runner has no qemu-system-aarch64 or user-mode "
                               "emulation, so the arm64 binary is checked as an artifact only; the owner kept "
                               "this gap open at M12a (2026-10-24) rather than install an emulator"})
    items.append({"name": "WSL2 install", "kind": "install", "owner": "release", "status": "gap",
                  "milestone": "M10",
                  "gapReason": "needs a WSL2 runner: the Linux binary inside a Windows-hosted WSL2 distribution"})
    return {
        "surface": "distribution", "owner": "release",
        "artifacts": artifacts,
        "npm": {k: package.get(k) for k in ("name", "version", "bin", "files", "engines")},
        "deployFiles": deploy,
        "items": items,
    }


# M12 without Platform (evidence/m12a, parity rehearsal): what the rehearsal
# round proves on isolated state copies, and what stays open. M12 formally
# depends on M11, which needs staging Platform.
_M11_GAP = ("M11 dependency: needs staging Platform and a dedicated Rust validation identity; M12 depends on M11, "
            "so only the M12 items that need no Platform were run")


def capture_cutover() -> dict:
    covered = [
        ("state conversion", "no conversion: Rust starts on Go's stopped state unchanged; its answers equal Go's "
                             "own restart on a copy, and the snapshot's checksums are unchanged (M5 cross-read, "
                             "M12a)"),
        ("forward rehearsal", "stop Go with a task in flight, snapshot with checksums, start Rust on a copy, query "
                              "over MCP, restart, query again: equal to Go's restart and to the Go control; exact "
                              "agent ID kept"),
        ("rollback rehearsal", "Go starts on the copy Rust wrote and reports every task, Rust-era work included, "
                               "with the status and message Rust last reported; the Go control matches"),
        ("in-flight drill: synchronous call", "SIGINT with a call mid-shim: both agents stop accepting, finish "
                                              "the call (200, Connection: close) and exit 0"),
        ("in-flight drill: admitted task", "SIGINT with an admitted task mid-shim: both persist it cancelled "
                                           "(\"The Host Agent shut down before the task completed.\"), wait for "
                                           "the shim and exit 0; a restart reports it cancelled"),
        ("in-flight drill: task in binding", "SIGINT with a task still in resource binding: both drain the same "
                                             "way; after a restart Go reports it cancelled and Rust does not know "
                                             "it, the declared D12 difference"),
        ("two agents on one state directory", "Go takes no state-directory lock: on both sides the second agent "
                                              "starts and serves, neither sees the other's in-memory tasks, and a "
                                              "restart sees both agents' tasks"),
    ]
    items = [{"name": name, "kind": "rehearsal", "owner": "release", "status": "covered", "coverage": text}
             for name, text in covered]
    gaps = [
        ("state-directory writer lock", "core", "M12",
         "M12 and task 4.4 ask that a second writer on one state directory be refused, but the pinned Go agent "
         "takes no lock and both implementations let it start (evidence/m12a); a refusal is new policy that "
         "needs its own OpenSpec change and owner decision"),
        ("72-hour soak", "release", "M12", "needs a 72-hour twin-sandbox run under a mixed workload; not run"),
        ("M11 staging validation identity", "platform", "M11", _M11_GAP),
        ("M11 Platform-driven journey", "platform", "M11", _M11_GAP),
        ("M11 client first-success", "platform", "M11", _M11_GAP + "; codex over WSL2 also needs a WSL2 runner"),
        ("M13 cutover evidence", "release", "M13",
         "packageFirstSuccess, externalEffects, rollbackRehearsal and ownerReview are reviewed at the cutover "
         "decision"),
        ("stable 0.2.3 publish decision", "release", "M10",
         "task 5.2: Go's release policy refuses the Rust catalog under the stable 0.2.3 package; a tested Rust "
         "package needs a new version and catalog decision"),
    ]
    items += [{"name": name, "kind": "cutover", "owner": owner, "status": "gap", "milestone": milestone,
               "gapReason": reason} for name, owner, milestone, reason in gaps]
    return {"surface": "cutover", "owner": "release", "items": items}


def collect_gaps(inventory: dict, catalogs: dict) -> dict:
    gaps = []
    for fname, doc in sorted(inventory.items()):
        if not isinstance(doc, dict):
            continue
        for item in doc.get("items", []):
            if item.get("status") == "gap":
                key = (item.get("name") or item.get("route") or item.get("command") or item.get("recipe")
                       or item.get("tool"))
                gaps.append({"file": fname, "item": key, "owner": item.get("owner"),
                             "milestone": item.get("milestone"),
                             "reason": item.get("gapReason", "no covering scenario yet")})
        if doc.get("status") == "gap":
            gaps.append({"file": fname, "item": "*", "owner": doc.get("owner"), "milestone": doc.get("milestone"),
                         "reason": doc.get("gapReason")})
    unowned = [g for g in gaps if not g.get("owner")]
    by_milestone: dict[str, int] = {}
    for g in gaps:
        by_milestone[g["milestone"] or "unassigned"] = by_milestone.get(g["milestone"] or "unassigned", 0) + 1
    return {"count": len(gaps), "unownedCount": len(unowned), "byMilestone": by_milestone, "items": gaps}


def write(inventory: dict[str, Any]) -> dict:
    INVENTORY_DIR.mkdir(parents=True, exist_ok=True)
    index = inventory.pop("index.json")
    for name, doc in sorted(inventory.items()):
        path = INVENTORY_DIR / name
        path.parent.mkdir(parents=True, exist_ok=True)
        data = (canon.canonical_json(doc) + "\n").encode()
        path.write_bytes(data)
        index["files"][name] = hashlib.sha256(data).hexdigest()
    (INVENTORY_DIR / "index.json").write_text(canon.canonical_json(index) + "\n")
    return index
