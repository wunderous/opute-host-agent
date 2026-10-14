"""Fail-closed verification of committed T2 (twin sandbox) evidence.

A T2 suite passes only when its receipts reproduce it: every receipt file
matches the hash its summary recorded, every raw snapshot it names is in
the content-addressed store with matching content, and re-deriving the
observation, the normalized diff, the declared divergences and the
no-unauthorized-effect check from those raw bytes gives exactly the
recorded, clean result for every iteration. Provenance must match the
manifest (pinned Go reference, recorded Rust candidate, pinned golden
image) and the current harness, guest scripts and scenario files, so
editing any of them after a capture invalidates it.
"""
from __future__ import annotations

import gzip
import hashlib
import json
from pathlib import Path
from typing import Any, Callable

from . import t2

PASS, FAIL, UNVERIFIED = "pass", "fail", "unverified"


def _sha(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def _restore(record: dict, store: Path) -> dict:
    """The raw driver output with each stored snapshot put back."""
    raw = json.loads(json.dumps(record["raw"]))
    for result in (raw.get("steps") or {}).values():
        if isinstance(result, dict) and isinstance(result.get("snapshot"), dict) \
                and set(result["snapshot"]) == {"sha256"}:
            result["snapshot"] = t2.load_snapshot(store, result["snapshot"]["sha256"])
    return raw


def check_suite(root: Path, manifest: dict, name: str, suite: dict, scenarios: dict[str, dict],
                problem: Callable[[str], None]) -> dict[str, str]:
    """Scenario id -> pass/fail for one T2 suite; problems make it unverified."""
    path = root / suite.get("summary", "")
    if not suite.get("summary") or not path.is_file():
        problem(f"{name}: summary missing")
        return {}
    summary = json.loads(path.read_text())
    bundle = path.parent
    stale = []
    go_sha = manifest["goReference"]["binarySha256"]
    if summary.get("tier") != "T2":
        stale.append("not a T2 summary")
    if summary.get("changedDuringRun") is not False:
        stale.append("binary, harness, guest scripts or scenarios changed during the capture")
    if summary.get("left", {}).get("sha256") != go_sha:
        stale.append("left side is not the pinned Go reference binary")
    right = summary.get("right", {})
    if suite.get("right") == "go" and right.get("sha256") != go_sha:
        stale.append("right side is not the pinned Go reference binary")
    if suite.get("right") == "rust":
        rust_sha = manifest.get("rustCandidate", {}).get("binarySha256")
        if not rust_sha or right.get("sha256") != rust_sha or not right.get("label", "").startswith("rust"):
            stale.append("right side is not the recorded Rust candidate")
    pinned = manifest.get("t2Evidence", {}).get("golden", {})
    if not pinned or any(summary.get("golden", {}).get(k) != v for k, v in pinned.items()):
        stale.append("golden image differs from the pinned golden")
    # Scenarios booted from the nested-Incus golden pin it too, over the base.
    if any(s.get("golden") == "incus" for s in scenarios.values()):
        pinned = manifest.get("t2Evidence", {}).get("incusGolden", {})
        if not pinned or any(summary.get("incusGolden", {}).get(k) != v for k, v in pinned.items()):
            stale.append("nested-Incus golden image differs from the pinned one")
    for guest, digest in (summary.get("guestSha256") or {}).items():
        if _sha(t2.GUEST_DIR / guest) != digest:
            stale.append(f"guest script {guest} changed since the capture")
    if set(summary.get("guestSha256") or {}) != set(t2.GUEST_FILES):
        stale.append("guest script set differs")
    if summary.get("harnessSha256") != _sha(Path(t2.__file__)):
        stale.append("T2 harness changed since the capture")
    current = {str(p.relative_to(t2.REPO_ROOT)): _sha(p) for p in sorted(t2.SCENARIO_DIR.glob("*.json"))}
    if summary.get("scenarioSha256") != current:
        stale.append("T2 scenarios changed since the capture")
    repeat = int(summary.get("repeat", 0))
    if repeat < suite.get("minRepeat", 1):
        stale.append(f"repeat {repeat} < required {suite.get('minRepeat')}")
    if stale:
        for text in stale:
            problem(f"{name}: stale: {text}")
        return {}

    statuses: dict[str, str] = {}
    store = bundle / "snapshots"
    results = summary.get("scenarios") or {}
    for sid, scenario in scenarios.items():
        result = results.get(sid)
        if result is None:
            problem(f"{name}/{sid}: no evidence")
            continue
        sdir = bundle / "scenarios" / sid
        ok = result.get("status") == "pass" and len(result.get("iterations", [])) == repeat
        for it in result.get("iterations", []):
            i = it["iteration"]
            receipts = it.get("receipts") or {}
            if set(receipts) != {f"a{i}.json.gz", f"b{i}.json.gz", f"diff{i}.json"}:
                problem(f"{name}/{sid}#{i}: receipts incomplete")
                ok = False
                continue
            if any(not (sdir / f).is_file() or _sha(sdir / f) != d for f, d in receipts.items()):
                problem(f"{name}/{sid}#{i}: receipt missing or modified")
                ok = False
                continue
            a = json.loads(gzip.decompress((sdir / f"a{i}.json.gz").read_bytes()))
            b = json.loads(gzip.decompress((sdir / f"b{i}.json.gz").read_bytes()))
            try:
                out = {side: {"raw": _restore(rec, store), "error": rec.get("error")}
                       for side, rec in (("a", a), ("b", b))}
            except (OSError, ValueError) as e:
                problem(f"{name}/{sid}#{i}: snapshot store: {e}")
                ok = False
                continue
            outcome = t2.judge(scenario, out["a"], out["b"], a.get("label", ""), b.get("label", ""))
            recorded = json.loads((sdir / f"diff{i}.json").read_text())
            if outcome["observation"] != {"a": a.get("observation"), "b": b.get("observation")}:
                problem(f"{name}/{sid}#{i}: recorded observation does not reproduce from the raw receipts")
                ok = False
            for key in ("diff", "maskViolations", "unexpectedEffects", "harnessErrors", "divergences"):
                if json.loads(json.dumps(outcome[key], sort_keys=True)) != recorded.get(key):
                    problem(f"{name}/{sid}#{i}: recorded {key} does not reproduce")
                    ok = False
            if t2.failed(outcome, {"a": a.get("error"), "b": b.get("error")}) or not it.get("pass"):
                ok = False
        statuses[sid] = PASS if ok else FAIL
    return statuses


def check(root: Path, manifest: dict, problem: Callable[[str], None]) -> dict[str, Any]:
    """Every suite in manifest.t2Evidence, against the current T2 scenarios."""
    spec = manifest.get("t2Evidence") or {}
    scenarios = {s["id"]: s for s in t2.load_scenarios()}
    suites = {name: check_suite(root, manifest, name, suite, scenarios, problem)
              for name, suite in (spec.get("suites") or {}).items()}
    return {sid: {name: suites[name].get(sid, UNVERIFIED) for name in suites} for sid in scenarios}
