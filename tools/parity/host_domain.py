#!/usr/bin/env python3
"""Inventory the Go host domain for M8a: baseline/inventory/host-domain.json.

Run from the repository root: python3 tools/parity/host_domain.py

Every tool whose Go handler calls svc.Host() gets its registration (effect,
admission class, task mode, anchor), the domain functions it reaches
(transitively, inside internal/domain/host), the commands and environment
those functions name, the focused Go tests, its catalog publication per
captured cell, its service family, its D19 neutral name, and whether Rust
serves it. The static command scan is a lower bound; runtime shim traces
confirm each command (effects.json).
"""
import json, re, sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
GO = ROOT / ".parity/go-src"
OUT = ROOT / "baseline/inventory/host-domain.json"
HOST = GO / "internal/domain/host"

# D19 (openspec/changes/neutral-host-tool-names): Go name -> published name.
NEUTRAL = {
    "install_incus_stack": "install_virtualization_stack",
    "uninstall_incus_stack": "uninstall_virtualization_stack",
    "probe_incus_gpu": "probe_virtualization_gpu",
    "terminate_wsl_distribution": "terminate_host_vm_distribution",
    "shutdown_wsl": "shutdown_host_vm_subsystem",
    "compact_wsl_disk": "compact_host_vm_disk",
    "ensure_docker": "ensure_container_engine",
    "ensure_k3d": "ensure_local_cluster_runtime",
}

# Service Definitions the host domain is ported as (D14). A family is one
# built-in service; dependencies follow the Go call graph. Reads ported in M3
# keep their M3 services (host-observation, host-info).
FAMILY = {
    "ensure_host_file": "host-files", "inspect_host_file": "host-observation", "remove_host_file": "host-files",
    "extract_host_archive": "host-archives",
    "ensure_host_artifact": "host-artifacts", "prepare_host_agent_artifacts": "host-artifacts",
    "ensure_host_tool": "host-tools",
    "exec_command": "host-exec", "run_host_command": "host-exec", "agent_shell": "host-exec",
    "run_instance_command": "instance-exec",
    "inspect_host_service": "host-observation", "list_host_services": "host-observation",
    "restart_host_service": "host-services", "set_host_service_state": "host-services",
    "ensure_host_service_supervisor": "host-services", "inspect_host_service_supervisor": "host-services",
    "probe_http_endpoint": "host-observation",
    "ensure_host_firewall_rule": "host-firewall",
    "configure_agent_connection": "agent-connection",
    "ensure_public_mcp_tunnel": "public-mcp", "ensure_public_mcp_quick_tunnel": "public-mcp",
    "remove_public_mcp_quick_tunnel": "public-mcp",
    "terminate_wsl_distribution": "host-vm-subsystem", "shutdown_wsl": "host-vm-subsystem",
    "compact_wsl_disk": "host-vm-subsystem",
    "install_incus_stack": "virtualization-stack", "uninstall_incus_stack": "virtualization-stack",
    "probe_incus_gpu": "virtualization-stack",
    "ensure_docker": "container-engine", "ensure_k3d": "local-cluster-runtime",
    "get_host_info": "host-info", "detect_host_platform": "host-info",
    "diagnose_bridge": "bridge", "recover_bridge": "bridge",
}

# Tools whose handler reaches the host domain through a seam other than
# svc.Host(): the resource coordinator's policy backend is wired to
# Host().ReconcileHostResourcePolicy in internal/app/runtime.go.
ROUTED = {"reconcile_host_resource_policy": ["ReconcileHostResourcePolicy"]}
FAMILY["reconcile_host_resource_policy"] = "host-resources"

COMMANDS = {"apt-cache", "apt-get", "bash", "curl", "dpkg", "dpkg-query", "install", "kill", "rm",
            "sudo", "systemctl", "usermod", "incus", "wsl.exe", "powershell.exe", "nft", "iptables",
            "ufw", "cloudflared", "docker", "k3d", "journalctl", "loginctl", "ss", "tar", "sh",
            "netsh.exe", "cmd.exe", "tailscale", "ip"}
EXEC = re.compile(r'(?:exec\.Command(?:Context)?\((?:ctx, |[a-zA-Z]+, )?|LookPath\(|RunHost\(\[\]string\{|'
                  r'RunPrivileged(?:Package)?\((?:ctx, )?|\[\]string\{)"([a-zA-Z0-9_.-]+)"')
ENV = re.compile(r'(?:os\.Getenv|os\.LookupEnv)\("([A-Z0-9_]+)"\)')
FUNC = re.compile(r'^func (?:\([a-z]+ \*?([A-Za-z]+)\) )?([A-Za-z0-9_]+)\(', re.M)


def go_functions(directory: Path) -> dict[str, dict]:
    """name -> {file, line, body} for non-test functions (methods keyed by name)."""
    out = {}
    for path in sorted(directory.glob("*.go")):
        if path.name.endswith("_test.go"):
            continue
        text = path.read_text()
        starts = [m for m in FUNC.finditer(text)]
        for i, m in enumerate(starts):
            end = starts[i + 1].start() if i + 1 < len(starts) else len(text)
            out.setdefault(m.group(2), {"file": str(path.relative_to(GO)),
                                        "line": text.count("\n", 0, m.start()) + 1,
                                        "body": text[m.start():end]})
    return out


def reach(functions: dict, roots: list[str]) -> list[str]:
    seen, todo = [], list(roots)
    while todo:
        name = todo.pop()
        if name in seen or name not in functions:
            continue
        seen.append(name)
        body = functions[name]["body"]
        for called in re.findall(r'\b([A-Za-z_][A-Za-z0-9_]*)\(', body):
            if called in functions and called not in seen:
                todo.append(called)
    return seen


def registrations() -> list[dict]:
    names = {}
    for path in (GO / "internal/contract/toolname").glob("*.go"):
        for m in re.finditer(r'(\w+)\s*=\s*"([a-z0-9_.]+)"', path.read_text()):
            names[m.group(1)] = m.group(2)
    rows = []
    for path in sorted((GO / "internal/tools").glob("dispatch*.go")):
        if path.name.endswith("_test.go"):
            continue
        text = path.read_text()
        found = list(re.finditer(r'register\(toolname\.(\w+),\s*(\w+),\s*resource\.(\w+),\s*(\w+),', text))
        for i, m in enumerate(found):
            end = min(x for x in (text.find("\nfunc ", m.end()), found[i + 1].start() if i + 1 < len(found) else -1,
                                  len(text)) if x > 0)
            body = text[m.start():end]
            handler = re.match(r'\s*([A-Za-z_]\w*)\)', text[m.end():])
            if handler:
                # A shared handler variable: its definition is earlier in
                # the same init block.
                start = text.rfind("\nfunc init()", 0, m.start())
                body = text[start:end]
            calls = sorted(set(re.findall(r'svc\.Host\(\)\.(\w+)', body)))
            calls = calls or ROUTED.get(names[m.group(1)], [])
            if calls:
                rows.append({"tool": names[m.group(1)], "effect": m.group(2), "admissionClass": m.group(3),
                             "taskMode": m.group(4), "anchor": f"{path.relative_to(GO)}:{text.count(chr(10), 0, m.start()) + 1}",
                             "entry": calls})
    return rows


def catalog_cells() -> dict[str, set[str]]:
    cells = {}
    for path in sorted((ROOT / "baseline/inventory/catalog").glob("*.json")):
        cells[path.stem] = {d["name"] for d in json.loads(path.read_text())["descriptors"]}
    return cells


def excluded() -> dict[str, str]:
    text = (GO / "internal/tools/catalog.go").read_text()
    out = {}
    for group in ("CatalogExcludedToolNames", "IncusOmittedToolNames"):
        block = text[text.find(f"var {group}"):]
        block = block[:block.find("\n}")]
        for name in re.findall(r'"([a-z0-9_]+)":\s*true', block):
            out[name] = group
    return out


def tests_for(tool: str, functions: list[str]) -> list[str]:
    found = set()
    for path in sorted((GO / "internal").rglob("*_test.go")):
        text = path.read_text()
        for m in re.finditer(r'^func (Test[A-Za-z0-9_]+)\(', text, re.M):
            end = text.find("\nfunc ", m.end())
            body = text[m.start(): end if end > 0 else len(text)]
            if f'"{tool}"' in body or any(re.search(rf'\.{f}\(', body) for f in functions[:1]):
                found.add(f"{path.relative_to(GO)}:{m.group(1)}")
    return sorted(found)


def rust_serves(tool: str) -> bool:
    """A built-in service operation registered under the tool's name."""
    text = (ROOT / "crates/host-agent/src/builtin.rs").read_text()
    return any(re.search(rf'"{re.escape(name)}",\s*op\(', text) for name in (tool, NEUTRAL.get(tool, tool)))


def main() -> None:
    functions = go_functions(HOST)
    cells = catalog_cells()
    omitted = excluded()
    tools = []
    for reg in registrations():
        reached = reach(functions, reg["entry"])
        commands, env, anchors = set(), set(), []
        for name in reached:
            fn = functions[name]
            anchors.append(f'{fn["file"]}:{fn["line"]}:{name}')
            commands.update(c for c in EXEC.findall(fn["body"]) if c in COMMANDS)
            commands.update(c for c in re.findall(r'"([a-z][a-z0-9.-]*)"', fn["body"])
                            if c in COMMANDS and c not in {"install", "rm", "kill", "ip", "sh", "ss", "tar"})
            env.update(ENV.findall(fn["body"]))
        tool = reg["tool"]
        tools.append({
            **{k: reg[k] for k in ("tool", "effect", "admissionClass", "taskMode", "anchor")},
            "neutralName": NEUTRAL.get(tool, tool),
            "family": FAMILY.get(tool, "unassigned"),
            "domainFunctions": anchors,
            "commandsStatic": sorted(commands),
            "envReads": sorted(env),
            "publishedIn": sorted(c for c, names in cells.items() if tool in names),
            "catalogExclusion": omitted.get(tool),
            "goTests": tests_for(tool, reg["entry"]),
            "rustServes": rust_serves(tool),
        })
    unassigned = [t["tool"] for t in tools if t["family"] == "unassigned"]
    if unassigned:
        sys.exit(f"tools without a service family: {unassigned}")
    doc = {
        "surface": "host-domain",
        "owner": "host",
        "milestone": "M8a",
        "source": json.loads((ROOT / "baseline/source-lock.json").read_text())["sourceCommit"],
        "note": ("Static inventory of the Go host domain. commandsStatic is a lower bound from literal "
                 "command names; effects.json records the commands each tool is observed running. "
                 "publishedIn lists the captured catalog cells that publish the tool; a tool in no cell "
                 "is reachable only as catalogExclusion says, which runtime evidence must confirm."),
        "families": sorted({t["family"] for t in tools}),
        "tools": sorted(tools, key=lambda t: (t["family"], t["tool"])),
    }
    OUT.write_text(json.dumps(doc, indent=1, sort_keys=True) + "\n")
    print(f"wrote {OUT.relative_to(ROOT)}: {len(tools)} tools in {len(doc['families'])} families")


if __name__ == "__main__":
    main()
