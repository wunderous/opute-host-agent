#!/usr/bin/env python3
"""Cutover readiness from committed evidence (D28, Q13).

  python3 evidence/cutover/readiness.py [--report FILE]
  python3 evidence/cutover/readiness.py --record ITEM     # after a capture

readiness.json lists every item the cutover needs. Each item is reported
`passed`, `unverified` (with its question), `blocked` (with its owner and
what unblocks it) or `failed`:

  e2e-gate     the gate's receipts verify (evidence/e2e/verify.py); a gate
               whose journeys pass with open questions is unverified.
  requirement  one requirement of one verified gate journey passed.
  receipt      a committed receipt: its sha256 is the recorded one (else
               tampered), its sources' digest is the recorded one (else
               stale), and its own verdict passed.
  seam         a seam stage: report and verifier log hash to its validation
               receipt, its canary was red, and its sources are current.
  candidate    one release set (candidate.json) is the build every listed
               gate ran, the contract suites ran and the arm64 install ran;
               journey inputs taken from a release set are its artifacts
               (or its recorded upgrade target); and the sources are still
               the ones it was built from.
  spec-text    the normative spec states the decided value.
  questions    every row of the questions table: answered rows need each
               carrier change's tasks done; open and deferred rows are
               unverified.
  blocked      recorded with its owner and unblocker.

Missing, stale, malformed or tampered evidence fails its item. Exit 0 only
when no item failed; unverified and blocked items are listed, never hidden.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
MANIFEST = "evidence/cutover/readiness.json"
STATUSES = ("passed", "unverified", "blocked", "failed")


def sha256_file(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def source_digest(root: Path, paths: list[str]) -> str:
    """One digest over every file under `paths` (sorted, by relative path),
    skipping build output."""
    h = hashlib.sha256()
    for entry in sorted(paths):
        base = root / entry
        files = sorted(p for p in base.rglob("*") if p.is_file()) if base.is_dir() else [base]
        for path in files:
            rel = path.relative_to(root).as_posix()
            if "/target/" in f"/{rel}" or "__pycache__" in rel:
                continue
            h.update(rel.encode() + b"\0" + (sha256_file(path).encode() if path.exists() else b"missing") + b"\n")
    return h.hexdigest()


def e2e_verifier(root: Path):
    """evidence/e2e/verify.py bound to `root` (tests verify copies)."""
    sys.path.insert(0, str(ROOT / "evidence/e2e"))
    import run as runner  # noqa: E402
    import verify  # noqa: E402
    runner.RUNS = root / "evidence/e2e/runs"
    runner.JOURNEYS = root / "evidence/e2e/journeys"
    verify.GATES = root / "evidence/e2e/gates.json"
    return verify


def check_gate(root: Path, item: dict) -> dict:
    gates_path = root / "evidence/e2e/gates.json"
    gates = json.loads(gates_path.read_text()) if gates_path.exists() else {}
    if item["gate"] not in gates:
        return {"status": "failed", "detail": f"gate {item['gate']} is not in gates.json"}
    spec = gates[item["gate"]]
    if spec.get("blocked"):
        return {"status": "blocked", "owner": item.get("owner", ""), "unblocker": item.get("unblocker", ""),
                "detail": spec["blocked"]}
    report = e2e_verifier(root).verify_gate(item["gate"])
    if not report["pass"]:
        problems = {j: r["problems"] for j, r in report["journeys"].items() if not r["ok"]}
        return {"status": "failed", "detail": f"run {report['run']}: {problems}"}
    open_questions = sorted({q for j in spec["journeys"].values() for q in j.get("unverified", [])})
    if open_questions:
        return {"status": "unverified", "questions": open_questions, "detail": f"run {report['run']}"}
    return {"status": "passed", "detail": f"run {report['run']}, {len(spec['journeys'])} journeys"}


def check_requirement(root: Path, item: dict) -> dict:
    gate = check_gate(root, {"gate": item["gate"]})
    if gate["status"] == "failed":
        return gate
    run = json.loads((root / "evidence/e2e/gates.json").read_text())[item["gate"]]["run"]
    results = root / "evidence/e2e/runs" / run / item["journey"] / "results.json"
    found = [r for r in json.loads(results.read_text())["requirements"] if r["requirement"] == item["requirement"]]
    if not found:
        return {"status": "failed", "detail": f"{item['journey']} has no requirement {item['requirement']!r}"}
    outcomes = sorted({r["outcome"] for r in found})
    if outcomes != ["pass"]:
        return {"status": "failed", "detail": f"{item['journey']}: {item['requirement']!r} is {outcomes}"}
    return {"status": "passed", "detail": f"{item['journey']} in run {run}"}


def check_receipt(root: Path, item: dict) -> dict:
    path = root / item["receipt"]
    if not path.exists():
        return {"status": "failed", "detail": f"{item['receipt']} is missing"}
    if sha256_file(path) != item.get("sha256"):
        return {"status": "failed", "detail": f"{item['receipt']} does not hash to the recorded sha256 (tampered)"}
    if item.get("sources") and source_digest(root, item["sources"]) != item.get("sourceDigest"):
        return {"status": "failed", "detail": f"{item['receipt']} is stale: its sources changed since the capture"}
    try:
        receipt = json.loads(path.read_text())
    except ValueError:
        return {"status": "failed", "detail": f"{item['receipt']} is malformed"}
    if receipt.get("pass") is not True:
        return {"status": "failed", "detail": f"{item['receipt']} records a failed run"}
    return {"status": "passed", "detail": item.get("summary", item["receipt"])}


def check_seam(root: Path, item: dict) -> dict:
    stage = root / "evidence/seam" / item["stage"]
    validation = root / "evidence/seam" / f"{item['stage']}-validation.json"
    if not (stage / "report.json").exists() or not validation.exists():
        return {"status": "failed", "detail": f"seam stage {item['stage']} or its validation is missing"}
    receipt = json.loads(validation.read_text())
    if sha256_file(stage / "report.json") != receipt.get("reportSha256") or \
            sha256_file(stage / "verifier.log") != receipt.get("verifierLogSha256"):
        return {"status": "failed", "detail": f"seam stage {item['stage']} does not hash to its validation (tampered)"}
    report = json.loads((stage / "report.json").read_text())
    if report.get("gaps") or report.get("problems"):
        return {"status": "failed", "detail": f"seam report has gaps {report.get('gaps')} or problems"}
    if not receipt.get("canary", {}).get("red"):
        return {"status": "failed", "detail": "the seam canary did not turn the verifier red"}
    if source_digest(root, item["sources"]) != item.get("sourceDigest"):
        return {"status": "failed", "detail": f"seam stage {item['stage']} is stale: its sources changed"}
    return {"status": "passed", "detail": f"stage {item['stage']}: {report.get('counts')}"}


def check_candidate(root: Path, item: dict) -> dict:
    """Every gate and receipt names the one release set in the candidate record."""
    path = root / item["record"]
    if not path.exists():
        return {"status": "failed", "detail": f"{item['record']} is missing"}
    candidate = json.loads(path.read_text())
    artifacts = candidate["artifacts"]
    allowed = set(artifacts.values()) | set(candidate.get("upgradeTarget", {}).get("artifacts", {}).values()) \
        | set(candidate.get("rollbackTarget", {}).get("artifacts", {}).values())
    problems = []
    gates = json.loads((root / "evidence/e2e/gates.json").read_text())
    for gate in item["gates"]:
        spec = gates.get(gate) or {}
        if not spec.get("run") or not spec.get("journeys"):
            problems.append(f"gate {gate} has no run")
            continue
        for journey in spec["journeys"]:
            directory = root / "evidence/e2e/runs" / spec["run"] / journey
            ran = json.loads((directory / "receipt.json").read_text())["devvm"]["rustRelease"]
            other = sorted(name for name, digest in ran.items() if artifacts.get(name) != digest)
            if other:
                problems.append(f"gate {gate} ({spec['run']}, {journey}) ran another build of {other}")
            files = json.loads((directory / "journey.json").read_text())["inputs"].get("files", {})
            foreign = sorted(name for name, f in files.items()
                             if "/e2e/release/" in f.get("path", "") and f.get("sha256") not in allowed)
            if foreign:
                problems.append(f"gate {gate} ({journey}) took {foreign} from another release set")
    contracts = json.loads((root / item["contracts"]).read_text())
    if contracts.get("binarySha256") != artifacts["host-agent-linux-x64"]:
        problems.append("the contract suites ran another binary")
    arm64 = json.loads((root / item["arm64"]).read_text())["release"]
    if arm64.get("sha256") != artifacts["host-agent-linux-arm64.gz"] or \
            arm64.get("sha256sums") != candidate.get("sha256sums"):
        problems.append("the arm64 install ran another release set")
    if source_digest(root, item["sources"]) != candidate.get("sourceDigest"):
        problems.append("the sources changed since the candidate was built")
    if problems:
        return {"status": "failed", "detail": "; ".join(problems[:6]) +
                (f"; and {len(problems) - 6} more" if len(problems) > 6 else "")}
    return {"status": "passed", "detail": f"{candidate['name']} ({candidate['version']}, commit "
                                          f"{candidate['commit'][:9]}): {len(item['gates'])} gates and the receipts"}


def check_spec_text(root: Path, item: dict) -> dict:
    path = root / item["path"]
    if not path.exists():
        return {"status": "failed", "detail": f"{item['path']} is missing"}
    if item["contains"] not in path.read_text():
        return {"status": "failed", "detail": f"{item['path']} does not state {item['contains']!r}"}
    return {"status": "passed", "detail": f"{item['path']} states {item['contains']!r}"}


LINK = re.compile(r"\]\(\.\./([a-z0-9-]+)/proposal\.md\)")


def change_dir(root: Path, name: str) -> Path | None:
    active = root / "openspec/changes" / name
    if active.is_dir():
        return active
    archived = sorted((root / "openspec/changes/archive").glob(f"*-{name}"))
    return archived[-1] if archived else None


def check_questions(root: Path, item: dict) -> list[dict]:
    path = root / item["path"]
    if not path.exists():
        return [{"id": "questions", "status": "failed", "detail": f"{item['path']} is missing"}]
    out = []
    for line in path.read_text().splitlines():
        m = re.match(r"\| (Q\d+) \|", line)
        if not m:
            continue
        q = m.group(1)
        cells = [c.strip() for c in line.strip().strip("|").split("|")]
        answer = cells[-1]
        entry = {"id": q}
        if answer.startswith("Open"):
            entry.update(status="unverified", questions=[q], detail=answer)
        elif answer.startswith("Deferred"):
            entry.update(status="unverified", questions=[q], detail=answer)
        elif answer.startswith("Answered"):
            carriers = LINK.findall(answer) or (LINK.findall(line) if "Its proposal" in answer else [])
            problems = []
            for name in carriers:
                directory = change_dir(root, name)
                if directory is None:
                    problems.append(f"{name}: no such change")
                    continue
                tasks = (directory / "tasks.md").read_text() if (directory / "tasks.md").exists() else ""
                open_tasks = re.findall(r"^- \[ \] (\S+)", tasks, re.M)
                if open_tasks:
                    problems.append(f"{name}: tasks {open_tasks} open")
            if problems:
                entry.update(status="failed", detail="; ".join(problems))
            else:
                entry.update(status="passed", detail=f"{answer[:80]}" + (f" ({', '.join(carriers)})" if carriers else ""))
        else:
            entry.update(status="failed", detail=f"unrecognised answer: {answer[:80]}")
        out.append(entry)
    return out


CHECKS = {"e2e-gate": check_gate, "requirement": check_requirement, "receipt": check_receipt,
          "seam": check_seam, "candidate": check_candidate, "spec-text": check_spec_text}


def evaluate(root: Path = ROOT) -> dict:
    manifest_path = root / MANIFEST
    manifest = json.loads(manifest_path.read_text())
    items = []
    for item in manifest["items"]:
        kind = item["kind"]
        if kind == "questions":
            items.extend({**entry, "kind": kind} for entry in check_questions(root, item))
            continue
        if kind == "blocked":
            result = {"status": "blocked", "owner": item["owner"], "unblocker": item["unblocker"],
                      "detail": item.get("detail", "")}
        elif kind in CHECKS:
            try:
                result = CHECKS[kind](root, item)
            except (OSError, ValueError, KeyError) as error:
                result = {"status": "failed", "detail": f"unreadable evidence: {error!r}"}
            if result["status"] == "blocked":
                result.setdefault("owner", item.get("owner", ""))
                result.setdefault("unblocker", item.get("unblocker", ""))
        else:
            result = {"status": "failed", "detail": f"unknown kind {kind}"}
        items.append({"id": item["id"], "kind": kind, **result})
    counts = {s: sum(1 for i in items if i["status"] == s) for s in STATUSES}
    not_passed = [{"id": i["id"], "status": i["status"], **({"questions": i["questions"]} if "questions" in i else {}),
                   **({"owner": i["owner"], "unblocker": i["unblocker"]} if i["status"] == "blocked" else {})}
                  for i in items if i["status"] != "passed"]
    return {"schemaVersion": "opute-cutover-readiness-report.v1", "pass": counts["failed"] == 0,
            "counts": counts, "notPassed": not_passed, "items": items}


def record(root: Path, item_id: str) -> None:
    """After a capture: pin an item's receipt hash and sources digest."""
    manifest_path = root / MANIFEST
    manifest = json.loads(manifest_path.read_text())
    for item in manifest["items"]:
        if item["id"] == item_id:
            if "receipt" in item:
                item["sha256"] = sha256_file(root / item["receipt"])
            if "sources" in item:
                item["sourceDigest"] = source_digest(root, item["sources"])
            break
    else:
        raise SystemExit(f"no item {item_id}")
    manifest_path.write_text(json.dumps(manifest, indent=1) + "\n")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--report", type=Path)
    parser.add_argument("--record", metavar="ITEM")
    args = parser.parse_args()
    if args.record:
        record(ROOT, args.record)
        return 0
    report = evaluate()
    if args.report:
        args.report.write_text(json.dumps(report, indent=1) + "\n")
    for item in report["items"]:
        extra = item.get("questions") or ([item["owner"], item["unblocker"]] if item["status"] == "blocked" else "")
        print(f"{item['status']:<10} {item['id']:<28} {extra or ''} {item.get('detail', '')[:140]}")
    print(json.dumps({"pass": report["pass"], "counts": report["counts"],
                      "notPassed": [i["id"] for i in report["notPassed"]]}))
    return 0 if report["pass"] else 1


if __name__ == "__main__":
    sys.exit(main())
