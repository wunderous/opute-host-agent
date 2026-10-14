#!/usr/bin/env python3
"""Inventory the Go incus domain for M8b: baseline/inventory/incus-domain.json.

Run from the repository root: python3 tools/parity/incus_domain.py

The same projection host_domain.py makes for svc.Host(), applied to every
tool whose Go handler calls svc.Incus(): registration (effect, admission
class, task mode, anchor), the internal/domain/incus functions it reaches,
the commands and environment they name, the focused Go tests, catalog
publication per captured cell, the Service Definition it is ported as, its
D19 neutral name, and whether Rust serves it. The incus domain runs the
incus binary through its provider command runner, so commandsStatic names
incus wherever a reached function calls that runner; the shim traces (T1)
and the nested-Incus twins (T2) record the argv.
"""
import json
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import host_domain as hd  # noqa: E402

OUT = hd.ROOT / "baseline/inventory/incus-domain.json"
INCUS = hd.GO / "internal/domain/incus"

# Service Definitions the incus domain is ported as (D14); the reads were
# ported in M3 as instance-inventory.
FAMILY = {
    "get_vm_info": "instance-inventory", "list_vms": "instance-inventory",
    "provision_container": "system-containers", "probe_gpu_container": "system-containers",
    "create_vm": "instance-lifecycle", "provision_vm": "instance-lifecycle", "start_vm": "instance-lifecycle",
    "stop_vm": "instance-lifecycle", "restart_vm": "instance-lifecycle", "update_vm_resources": "instance-lifecycle",
    "delete_vm": "instance-lifecycle",
    "check_local_prerequisites": "local-status", "get_local_status": "local-status",
    "reset_incus_stack": "virtualization-stack",
}
# The provider command runner (commandRunner and the RunVMExec family)
# runs the incus binary with the argv it is given.
RUN_INCUS = re.compile(r"\b(?:commandRunner|CommandRunner(?:Context)?|RunProviderContext|RunVMExec\w*)\(")


def main() -> None:
    functions = hd.go_functions(INCUS)
    cells = hd.catalog_cells()
    omitted = hd.excluded()
    tools = []
    for reg in hd.registrations("Incus"):
        reached = hd.reach(functions, reg["entry"])
        commands, env, anchors = set(), set(), []
        for name in reached:
            fn = functions[name]
            anchors.append(f'{fn["file"]}:{fn["line"]}:{name}')
            commands.update(c for c in hd.EXEC.findall(fn["body"]) if c in hd.COMMANDS)
            if RUN_INCUS.search(fn["body"]):
                commands.add("incus")
            env.update(hd.ENV.findall(fn["body"]))
        tool = reg["tool"]
        tools.append({
            **{k: reg[k] for k in ("tool", "effect", "admissionClass", "taskMode", "anchor")},
            "neutralName": hd.NEUTRAL.get(tool, tool),
            "family": FAMILY.get(tool, "unassigned"),
            "domainFunctions": anchors,
            "commandsStatic": sorted(commands),
            "envReads": sorted(env),
            "publishedIn": sorted(c for c, names in cells.items() if tool in names),
            "catalogExclusion": omitted.get(tool),
            "goTests": hd.tests_for(tool, reg["entry"]),
            "rustServes": hd.rust_serves(tool),
        })
    unassigned = [t["tool"] for t in tools if t["family"] == "unassigned"]
    if unassigned:
        sys.exit(f"tools without a service family: {unassigned}")
    doc = {
        "surface": "incus-domain",
        "owner": "incus",
        "milestone": "M8b",
        "source": json.loads((hd.ROOT / "baseline/source-lock.json").read_text())["sourceCommit"],
        "note": ("Static inventory of the Go incus domain. commandsStatic is a lower bound from literal "
                 "command names and the provider command runner seam; effects.json records the commands "
                 "each tool is observed running. publishedIn lists the captured catalog cells that publish "
                 "the tool."),
        "families": sorted({t["family"] for t in tools}),
        "tools": sorted(tools, key=lambda t: (t["family"], t["tool"])),
    }
    OUT.write_text(json.dumps(doc, indent=1, sort_keys=True) + "\n")
    print(f"wrote {OUT.relative_to(hd.ROOT)}: {len(tools)} tools in {len(doc['families'])} families")


if __name__ == "__main__":
    main()
