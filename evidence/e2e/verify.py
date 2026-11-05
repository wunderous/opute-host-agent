#!/usr/bin/env python3
"""Verify an E2E gate from its committed, hash-pinned receipts.

  python3 evidence/e2e/verify.py --gate e2e-0 [--report FILE]

gates.json names, per gate, the run and the journeys it requires and the
verdict each must have (a self-check that must fail names the failure it
must fail with). For each receipt the verifier:
  - checks every recorded file against its sha256 in receipt.json;
  - checks journey.json is the committed journey (journeys/<id>.json);
  - re-derives results.json from the transcript and the state files with
    run.evaluate, and requires it to equal the recorded one;
  - requires the expected verdict;
  - refuses any file that carries a test token.
A missing, stale or altered receipt fails the gate. Exit 0 only on pass.
"""
from __future__ import annotations

import argparse
import gzip
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import e2elib  # noqa: E402
import run as runner  # noqa: E402

GATES = e2elib.E2E_DIR / "gates.json"


def verify_receipt(directory: Path, expect: dict) -> list[str]:
    problems = []
    receipt_path = directory / "receipt.json"
    if not receipt_path.exists():
        return [f"{directory.name}: no receipt"]
    receipt = json.loads(receipt_path.read_text())
    present = {p.name for p in directory.iterdir()} - {"receipt.json"}
    if present != set(receipt["files"]):
        problems.append(f"files {sorted(present)} != receipt {sorted(receipt['files'])}")
    for name, digest in receipt["files"].items():
        path = directory / name
        if not path.exists() or e2elib.sha256_file(path) != digest:
            problems.append(f"{name}: sha256 differs from the receipt")
    for path in directory.iterdir():
        data = gzip.decompress(path.read_bytes()) if path.suffix == ".gz" else path.read_bytes()
        if runner.TOKEN_PATTERN.search(data.decode(errors="replace")):
            problems.append(f"{path.name}: carries a test token")
    if problems:
        return problems
    journey = json.loads((directory / "journey.json").read_text())
    source = runner.JOURNEYS / f"{receipt['journey']}.json"
    if not source.exists() or json.loads(source.read_text()) != journey:
        problems.append("journey.json is not the committed journey")
    if e2elib.sha256_file(source) != receipt["journeySourceSha256"]:
        problems.append("the committed journey changed since the run")
    malformed = runner.journey_problems(journey)
    if malformed:
        problems.append("journey malformed: " + "; ".join(malformed))
        return problems
    transcript = [json.loads(line) for line in (directory / "transcript.jsonl").read_text().splitlines()]
    states = {name: json.loads(gzip.decompress((directory / f"state-{name}.json.gz").read_bytes()))
              for name in ("before", "after", "cleanup")}
    derived = runner.evaluate(journey, transcript, states)
    recorded = json.loads((directory / "results.json").read_text())
    if derived != recorded:
        problems.append("results.json does not re-derive from the transcript and states")
    if receipt["verdict"] != {"pass": derived["pass"], "failures": derived["failures"]}:
        problems.append("receipt verdict differs from the derived one")
    if derived["pass"] != expect["pass"]:
        problems.append(f"verdict pass={derived['pass']} ({derived['failures']}), expected pass={expect['pass']}")
    if "failures" in expect and derived["failures"] != expect["failures"]:
        problems.append(f"failures {derived['failures']} != expected {expect['failures']}")
    return problems


def verify_gate(gate: str) -> dict:
    spec = json.loads(GATES.read_text())[gate]
    results = {}
    for journey_id, expect in spec["journeys"].items():
        directory = runner.RUNS / spec["run"] / journey_id
        problems = verify_receipt(directory, expect) if directory.exists() else ["no receipt for this run"]
        results[journey_id] = {"ok": not problems, "problems": problems, "expect": expect}
    blocked = spec.get("blocked")
    passed = all(r["ok"] for r in results.values()) and not blocked
    return {"gate": gate, "run": spec["run"], "pass": passed, "blocked": blocked, "journeys": results}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--gate", required=True)
    parser.add_argument("--report", type=Path)
    args = parser.parse_args()
    report = verify_gate(args.gate)
    text = json.dumps(report, indent=1, sort_keys=True) + "\n"
    if args.report:
        args.report.write_text(text)
    print(text, end="")
    return 0 if report["pass"] else 1


if __name__ == "__main__":
    sys.exit(main())
