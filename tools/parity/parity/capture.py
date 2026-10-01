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
PLAN_RUNS = 16

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
    "providers": "core",
    "distribution": "release",
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
    """Scenario id -> canonical JSON text, for coverage lookups."""
    return {s["id"]: json.dumps(s, sort_keys=True) for s in scenarios}


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


def capture_env(src: Path, refs: dict[str, str]) -> dict:
    found: dict[str, list[str]] = {}
    for path in _go_files(src, "internal", "pkg", "cmd"):
        text = path.read_text()
        for m in ENV_PATTERN.finditer(text):
            found.setdefault(m.group(1), []).append(_anchor(src, path, text, m.start()))
    items = []
    for name in sorted(found):
        covered = _covering(name, refs)
        items.append({
            "name": name,
            "owner": "config",
            "anchors": sorted(set(found[name])),
            "scenarios": covered,
            "status": "covered" if covered else "gap",
        })
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


def capture_effects(src: Path, observed: list[dict]) -> dict:
    found: dict[str, list[str]] = {}
    for path in _go_files(src, "internal", "pkg") + _go_files(src, "plugins"):
        text = path.read_text()
        for m in EXEC_PATTERN.finditer(text):
            found.setdefault(m.group(1), []).append(_anchor(src, path, text, m.start()))
    observed_cmds: dict[str, set[str]] = {}
    for entry in observed:
        observed_cmds.setdefault(entry["cmd"], set()).add(" ".join(entry["argv"][:2]))
    items = []
    for name in sorted(set(found) | set(observed_cmds)):
        items.append({
            "command": name,
            "owner": "host",
            "anchors": sorted(set(found.get(name, []))),
            "observedInvocations": sorted(observed_cmds.get(name, [])),
            # Shimmable when the harness can intercept it through PATH or an
            # explicit *_BINARY_PATH override. The effect tier (T1 shim vs T2
            # sandbox) is confirmed per command in M8; until then it is a gap.
            "tier": "T1-shim-observed" if name in observed_cmds else "unclassified",
            "status": "covered" if name in observed_cmds else "gap",
        })
    return {
        "surface": "effects",
        "owner": "host",
        "note": "Static scan finds literal command names only. Commands built from variables "
                "(for example hostexec wrappers) are found through shim traces as scenarios grow; "
                "a command never observed stays a gap.",
        "items": items,
    }


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
    result = doc["steps"]["list"]["body"]["result"]
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
        observed[s["id"]] = runner.execute(impl, s, "a", "capture")

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
        registry.append({
            "name": tool,
            "owner": "contracts",
            "publishedIn": sorted(c for c, cat in catalogs.items()
                                  if tool in {t["name"].replace("${TOOL_PREFIX}_", "") for t in cat["descriptors"]}),
            "catalogExcluded": excluded,
            "status": "covered" if in_catalog else ("excluded-by-design" if excluded else "gap"),
        })

    state_obs = canon.substitute(*observed["state.schema-after-start"])
    inventory: dict[str, Any] = {
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
    inventory["plans.json"] = capture_plans(impl, src)
    inventory["findings.json"]["items"] += _plan_findings(inventory["plans.json"])
    inventory["findings.json"]["items"] += _registry_findings(registry)
    inventory["distribution.json"] = capture_distribution(src)
    inventory["gaps.json"] = collect_gaps(inventory, catalogs)
    inventory["index.json"] = {
        "sourceCommit": lock["sourceCommit"],
        "sourceTree": lock["sourceTree"],
        "goBinarySha256": impl.sha256(),
        "harnessSha256": runner.harness_sha256(),
        "files": {},  # filled by write()
    }
    return inventory


def capture_plans(impl: agent.Impl, src: Path) -> dict:
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
        items.append({
            "recipe": rel,
            "owner": "core",
            "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
            "cliValidate": observed[0] if len(observed) == 1 else {"nondeterministic": True,
                                                                   "variants": observed},
            "status": "gap",
            "gapReason": "plan execution parity (node states, retry, compensation) is M6",
        })
    return {"surface": "plans", "owner": "core", "items": items}


def capture_distribution(src: Path) -> dict:
    makefile = (src / "Makefile").read_text()
    artifacts = sorted(set(re.findall(r"\$\(DIST\)/((?:host-agent|opute-provider)-[a-z0-9_.-]+)", makefile)))
    package = json.loads((src / "npm/local-host-agent/package.json").read_text())
    deploy = {
        str(p.relative_to(src)): hashlib.sha256(p.read_bytes()).hexdigest()
        for p in sorted((src / "deploy").iterdir()) if p.is_file()
    }
    return {
        "surface": "distribution", "owner": "release",
        "artifacts": artifacts,
        "npm": {k: package.get(k) for k in ("name", "version", "bin", "files", "engines")},
        "deployFiles": deploy,
        "status": "gap",
        "gapReason": "packaging parity is M10",
    }


def collect_gaps(inventory: dict, catalogs: dict) -> dict:
    gaps = []
    for fname, doc in sorted(inventory.items()):
        if not isinstance(doc, dict):
            continue
        for item in doc.get("items", []):
            if item.get("status") == "gap":
                key = item.get("name") or item.get("route") or item.get("command") or item.get("recipe")
                gaps.append({"file": fname, "item": key, "owner": item.get("owner"),
                             "reason": item.get("gapReason", "no covering scenario yet")})
        if doc.get("status") == "gap":
            gaps.append({"file": fname, "item": "*", "owner": doc.get("owner"), "reason": doc.get("gapReason")})
    gaps.append({"file": "catalog/*", "item": "provider cells", "owner": "contracts",
                 "reason": "catalog cells with active providers (k3s, cloudflare, tailscale, ollama, hostos) "
                           "require provider installation fixtures; captured in M7"})
    unowned = [g for g in gaps if not g.get("owner")]
    return {"count": len(gaps), "unownedCount": len(unowned), "items": gaps}


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
