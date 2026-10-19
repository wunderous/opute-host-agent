#!/usr/bin/env python3
"""Inventory the Go postgres domain for M8e: baseline/inventory/postgres-domain.json.

Run from the repository root: python3 tools/parity/postgres_domain.py

The same projection host_domain.py makes for svc.Host(), applied to every
tool whose Go handler calls svc.Postgres(). The domain never runs kubectl
itself: every kubectl call goes through the kubernetes domain's deps
(RunKubectlContext, RunKubectlTimed, RunKubectlWithStdinContext), which
reach the active Kubernetes provider's exec-command operation, so
providerOperations lists that operation. The relay's proxy device is the
one host command (incus, through the shared CommandRunner). The shim traces
(T1) and the twins (T2) record the observed calls.
"""
import json
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import cluster_domain as cd  # noqa: E402
import host_domain as hd  # noqa: E402

OUT = hd.ROOT / "baseline/inventory/postgres-domain.json"
DIR = hd.GO / "internal/domain/postgres"

# Service Definitions the domain is ported as (D14).
FAMILY = {
    "ensure_sqlite_database": "sqlite-database", "get_sqlite_database_status": "sqlite-database",
    "remove_sqlite_database": "sqlite-database",
    "reconcile_postgresql_service": "postgresql-service", "get_postgresql_service_status": "postgresql-service",
    "remove_postgresql_service": "postgresql-service",
    "release_postgresql_service_relay": "postgresql-relay",
    "ensure_sql_connector": "sql-connector", "get_sql_connector_status": "sql-connector",
    "release_sql_connector": "sql-connector",
    "install_postgresql": "standalone-postgresql", "get_postgresql_status": "standalone-postgresql",
    "delete_postgresql": "standalone-postgresql", "run_sql": "standalone-postgresql",
}
# The shared CommandRunner runs incus (the relay's proxy device).
RUNNER = "s.shared.CommandRunner("
# Each kubernetes dependency the domain calls, and the provider operation
# the kubernetes domain reaches for it (runProviderCommand).
DEPS = {"RunKubectlContext": "KubernetesExecCommandOperation",
        "RunKubectlTimed": "KubernetesExecCommandOperation",
        "RunKubectlWithStdinContext": "KubernetesExecCommandOperation"}


def main() -> None:
    ops = cd.operations()
    functions = hd.go_functions(DIR)
    cells = hd.catalog_cells()
    omitted = hd.excluded()
    registry = {e["name"]: e for e in json.loads((hd.ROOT / "baseline/inventory/dispatch-registry.json")
                                                 .read_text())["items"]}
    values = json.loads((hd.ROOT / "crates/host-agent/catalog/neutral-names.json").read_text())["values"]
    tools = []
    for reg in hd.registrations("Postgres"):
        reached = hd.reach(functions, reg["entry"])
        commands, env, provider, deps, anchors = set(), set(), set(), set(), []
        for name in reached:
            fn = functions[name]
            body = fn["body"]
            anchors.append(f'{fn["file"]}:{fn["line"]}:{name}')
            if RUNNER in body:
                commands.add("incus")
            for dep, op in DEPS.items():
                if f"s.deps.{dep}(" in body:
                    deps.add(dep)
                    if op:
                        provider.add(ops[op])
            env.update(hd.ENV.findall(body))
        tool = reg["tool"]
        tools.append({
            **{k: reg[k] for k in ("tool", "effect", "admissionClass", "taskMode", "anchor")},
            "domain": "postgres",
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
        "surface": "postgres-domain",
        "owner": "postgres",
        "milestone": "M8e",
        "source": json.loads((hd.ROOT / "baseline/source-lock.json").read_text())["sourceCommit"],
        "note": ("Static inventory of the Go postgres domain. commandsStatic names the host commands the "
                 "domain runs (incus, for the relay's proxy device); providerOperations are the Kubernetes "
                 "provider operations every kubectl call reaches through the kubernetes domain; envReads are "
                 "the domain's own configuration reads; effects.json records what each tool is observed "
                 "running."),
        "families": sorted({t["family"] for t in tools}),
        "tools": sorted(tools, key=lambda t: (t["family"], t["tool"])),
    }
    OUT.write_text(json.dumps(doc, indent=1, sort_keys=True) + "\n")
    print(f"wrote {OUT.relative_to(hd.ROOT)}: {len(tools)} tools in {len(doc['families'])} families")


if __name__ == "__main__":
    main()
