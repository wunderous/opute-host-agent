#!/usr/bin/env python3
"""Run the Rust-only contract suites once and write their receipt (D28, Q13).

  python3 evidence/cutover/contracts.py --binary target/release/opute-host-agent SUITE...

Runs `parity contract --impl rust=BINARY --suite SUITE` for each suite and
writes evidence/cutover/contract-suites.json: each suite's passed and total
scenario counts, the binary's sha256, and the verdict. Afterwards
`readiness.py --record contract-suites` pins the receipt and its sources.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import re
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[1]
SUMMARY = re.compile(r"^(\d+)/(\d+) contract scenarios pass$", re.M)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("suites", nargs="+")
    args = parser.parse_args()
    binary = args.binary.resolve()
    suites = {}
    for suite in args.suites:
        done = subprocess.run([sys.executable, "-m", "parity", "contract", "--impl", f"rust={binary}", "--suite", suite],
                              cwd=REPO / "tools/parity", capture_output=True, text=True)
        found = SUMMARY.findall(done.stdout)
        passed, total = (int(found[-1][0]), int(found[-1][1])) if found else (0, 0)
        suites[suite] = {"exit": done.returncode, "passed": passed, "total": total,
                         "scenarios": sorted(line.split(None, 1)[1] for line in done.stdout.splitlines()
                                             if line.startswith(("pass ", "FAIL ", "fail ")))}
    receipt = {"schemaVersion": "opute-contract-suites.v1",
               "binarySha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
               "suites": suites,
               "pass": all(s["exit"] == 0 and s["total"] > 0 and s["passed"] == s["total"] for s in suites.values())}
    (HERE / "contract-suites.json").write_text(json.dumps(receipt, indent=1) + "\n")
    print(json.dumps({"pass": receipt["pass"], "suites": {k: f"{v['passed']}/{v['total']}" for k, v in suites.items()}}))
    return 0 if receipt["pass"] else 1


if __name__ == "__main__":
    sys.exit(main())
