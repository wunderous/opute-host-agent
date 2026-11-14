#!/usr/bin/env python3
"""The local distribution checks on the release candidate (m13b, tasks 5.1/5.2).

  python3 evidence/cutover/distribution.py --go-src DIR --go-release DIR --tarball FILE \\
      --release DIR --work DIR

Runs M10's own observers (tools/parity/parity/dist.py) against the candidate
recorded in evidence/cutover/candidate.json, local only:

  canary   Go's published-package readonly canary
           (published-readonly-canary.test.js) against the locally packed
           launcher tarball, starting the candidate's x64 binary
  site     Go's check-site on the committed site from Go v0.2.4 and from the
           candidate, a preview site (no stable release catalog) from each,
           and the candidate's preview with the declared D19-D24 renames
           reversed, which must differ from Go's only in the catalog files
           and only in the tools whose change D28 decided (DECLARED)

--go-src is a git checkout of the pinned Go tree (baseline/source-lock.json
sourceCommit); --go-release is the published Go v0.2.4 release set.
Clean-machine install, upgrade from Go v0.2.4, rollback and the npm launch
path run as dev-VM journeys (e2e1.*, e2e4.cutover) and the arm64 receipt.

Writes evidence/cutover/distribution.json. Nothing is published.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
sys.path.insert(0, str(ROOT / "tools/parity"))
from parity import dist  # noqa: E402

RECEIPT = HERE / "distribution.json"
# Catalog changes decided after M10's D19-D24 rename rule (D28, D37, D39, and
# platform-neutral-host-agent's descriptions without Platform names): the only tools
# the candidate's preview may still publish differently from Go's.
DECLARED = {"configure_service_domain": "D28 (Q28): bind-standalone-service-domain",
            "remove_service_domain": "D28 (Q28): bind-standalone-service-domain",
            "restart_host_service": "D28 (Q23): privileged-host-service-restart",
            "provision_container": "D28 (Q21): declare-read-inputs",
            "run_host_local_recipe": "platform-neutral-host-agent 3.5 (owner, 2026-11-08): no Platform name in a description",
            "send_console_input": "D39: vm-console-session (output and closed)",
            "register_kubernetes_cluster": "D37: withdraw-platform-and-legacy-provider-tools",
            "list_agents": "D37: withdraw-platform-and-legacy-provider-tools",
            "install_provider_tools": "D37: withdraw-platform-and-legacy-provider-tools",
            "uninstall_provider_tools": "D37: withdraw-platform-and-legacy-provider-tools"}
# The generated files that render the catalog; any other difference fails.
CATALOG_FILES = {"context/release-catalog.json", "public/search-index.json",
                 "public/docs/previews/v0.2.4/capabilities/catalog.json",
                 "public/docs/previews/v0.2.4/capabilities/index.html"}


def sha256(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--go-src", type=Path, required=True)
    parser.add_argument("--go-release", type=Path, required=True)
    parser.add_argument("--tarball", type=Path, required=True)
    parser.add_argument("--release", type=Path, required=True, help="the candidate's release set")
    parser.add_argument("--work", type=Path, required=True, help="a scratch directory under .parity/")
    args = parser.parse_args()
    candidate = json.loads((HERE / "candidate.json").read_text())
    binary = args.release / "host-agent-linux-x64"
    if sha256(binary) != candidate["artifacts"]["host-agent-linux-x64"]:
        raise SystemExit(f"refused: {binary} is not the candidate's x64 binary")
    lock = json.loads((ROOT / "baseline/source-lock.json").read_text())
    head = subprocess.run(["git", "-C", str(args.go_src), "rev-parse", "HEAD"], capture_output=True, text=True,
                          check=True).stdout.strip()
    if head != lock["sourceCommit"]:
        raise SystemExit(f"refused: {args.go_src} is at {head}, not the pinned {lock['sourceCommit']}")
    go_binary = args.go_release / "host-agent-linux-x64"
    canary = dist.npm_canary(args.go_src, args.tarball, binary)
    site = dist.observe_site(args.go_src, go_binary, binary, args.work)
    go_catalog = json.loads((args.work / "preview-go" / dist.SITE_CATALOG).read_text())
    as_go = json.loads((args.work / "preview-rust-as-go" / dist.SITE_CATALOG).read_text())
    tools = lambda catalog: {t["name"]: t for t in catalog["tools"]}
    go_tools, as_go_tools = tools(go_catalog), tools(as_go)
    differing = sorted(n for n in set(go_tools) | set(as_go_tools) if go_tools.get(n) != as_go_tools.get(n))
    steps = lambda side: {k: v.get("exit") for k, v in site[f"committed-{side}"]["steps"].items()}
    receipt = {
        "schemaVersion": "opute-cutover-distribution.v1",
        "candidate": {"name": candidate["name"], "binarySha256": candidate["artifacts"]["host-agent-linux-x64"]},
        "goSource": head, "goBinarySha256": sha256(go_binary),
        "tarballSha256": sha256(args.tarball),
        "canary": canary,
        "site": {"committedGo": {"steps": steps("go"), "status": site["committed-go"]["status"]},
                 "committedCandidate": {"steps": steps("rust"), "status": site["committed-rust"]["status"]},
                 "previewCapture": {s: site[f"preview-{s}"]["capture"].get("exit") for s in ("go", "rust")},
                 "previewGenerate": {s: site[f"preview-{s}"]["generate"].get("exit") for s in ("go", "rust")},
                 "previewDiff": site["previewDiff"], "previewAsGoDiff": site["previewAsGoDiff"],
                 "catalogAsGoEqual": site["catalogAsGoEqual"],
                 "toolsDifferingAfterRenames": {n: DECLARED.get(n, "undeclared") for n in differing}, "propertiesSorted": site["propertiesSorted"],
                 "renameRule": site["renameRule"]},
    }
    receipt["pass"] = {
        "canary": canary["exit"] == 0 and canary["counts"].get("fail", 1) == 0 and canary["counts"].get("pass", 0) > 0,
        "previewEqualsGoAfterDeclared": set(differing) == set(DECLARED)
        and set(site["previewAsGoDiff"]) <= CATALOG_FILES,
        "committedSiteFromGo": all(v == 0 for v in steps("go").values()),
    }
    RECEIPT.write_text(json.dumps(receipt, indent=1) + "\n")
    print(json.dumps({"pass": receipt["pass"], "committedCandidate": receipt["site"]["committedCandidate"]["steps"],
                      "previewDiff": site["previewDiff"]}))
    return 0 if all(receipt["pass"].values()) else 1


if __name__ == "__main__":
    sys.exit(main())
