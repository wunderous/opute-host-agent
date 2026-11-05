#!/usr/bin/env python3
"""Dev VM state, taken as root inside the E2E dev VM.

Reuses the T2 effect snapshot (snapshot.py, copied next to this file) with
the dev VM's roots: the production-style agent lives under /home/opute and
/etc/opute, and E2E guests live in the Incus project opute-e2e. The agent's
own state directory and caches are excluded: they are the subject's process,
judged through its MCP results, not an external effect. Prints one JSON
document.
"""
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import snapshot  # noqa: E402

PROJECT = "opute-e2e"
snapshot.ROOTS = snapshot.ROOTS + ["/home/opute", "/etc/opute"]
snapshot.EXCLUDE_PREFIXES = snapshot.EXCLUDE_PREFIXES + [
    "/opt/opute-e2e/guest",
    "/home/opute/.local/state",
    "/home/opute/.cache",
    # The default instance root (state database, instance lock) and the
    # host resource coordinator's reservations (e2e-0-r1, retired).
    "/home/opute/.config/opute/instances",
    "/home/opute/.config/opute/host-resource-coordinator",
]


def project() -> dict:
    """The E2E project's own objects, keyed as snapshot.incus() keys them."""
    out = {}
    for kind, argv in (("instances", ["list"]), ("profiles", ["profile", "list"])):
        items = snapshot._incus_json(argv + ["--project", PROJECT]) or []
        out[kind] = {item["name"]: snapshot._stable(item, snapshot.INCUS_FIELDS[kind]) for item in items}
    _, text = snapshot._run(["incus", "project", "show", PROJECT])
    out["project"] = text
    return out


def nft_blocks(lines: list[str] | None) -> list[str] | None:
    """The nft ruleset with each table's chains and sets in name order.

    Incus creates one chain set per managed network as it brings networks
    up, in no fixed order across boots (e2e-0-r2, retired); the rules in
    each chain are kept exactly as listed.
    """
    if lines is None:
        return None
    out, blocks, current, in_table = [], [], None, False
    for line in lines:
        if line.startswith("table "):
            in_table = True
            out.append(line)
        elif in_table and line == "}":
            out += [row for block in sorted(blocks) for row in block]
            out.append(line)
            blocks, in_table = [], False
        elif in_table and line.startswith("\t") and not line.startswith("\t\t") and line.endswith("{"):
            current = [line]
        elif in_table and current is not None:
            current.append(line)
            if line == "\t}":
                blocks.append(current)
                current = None
        elif not (in_table and line == ""):
            out.append(line)
    return out


if __name__ == "__main__":
    state = snapshot.snapshot()
    state["firewall"]["nft"] = nft_blocks(state["firewall"].get("nft"))
    state["e2eProject"] = project() if state.get("incus") is not None else None
    json.dump(state, sys.stdout, sort_keys=True)
