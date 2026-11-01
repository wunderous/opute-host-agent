"""Fail-closed verification of committed M12 soak evidence (validation 5).

The soak round passes only when its receipts reproduce it: every gzipped
receipt matches the hash its summary recorded; the provenance is the pinned
Go tree, the manifest's Go reference on side a and Rust candidate on side b,
the pinned T2 golden, the current harness and soak driver, the declared
thresholds (pinned in the manifest and in force when the round started), at
least the manifest's minimum duration, no crash and nothing changed during
the capture; and re-deriving every check from the raw samples
(parity/soak.py's evaluate) gives the recorded evaluation, all passing.
"""
from __future__ import annotations

import hashlib
import json
from pathlib import Path
from typing import Callable

from . import runner, soak, t2

PASS, FAIL, UNVERIFIED = "pass", "fail", "unverified"
RECEIPTS = {f"{side}-{name}.gz" for side in ("a", "b")
            for name in ("ops.jsonl", "samples.jsonl", "events.jsonl", "final.json")}


def _sha(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def items(evaluation: dict) -> dict[str, str]:
    """One pass/fail per declared soak check."""
    return {f"soak:{name}": PASS if check.get("pass") is True else FAIL
            for name, check in sorted(evaluation["checks"].items())}


def check(root: Path, manifest: dict, lock: dict, problem: Callable[[str], None]) -> dict[str, str]:
    spec = manifest.get("soakEvidence") or {}
    path = root / spec.get("summary", "")
    if not spec.get("summary") or not path.is_file():
        problem("soak: summary missing")
        return {}
    summary = json.loads(path.read_text())
    bundle = path.parent
    provenance = summary.get("provenance") or {}
    thresholds_path = root / spec.get("thresholds", "")
    stale = []
    if provenance.get("sourceLock") != lock:
        stale.append("not captured from the pinned Go tree")
    if (provenance.get("left") or {}).get("binarySha256") != (manifest.get("goReference") or {}).get("binarySha256"):
        stale.append("side a is not the manifest's Go reference")
    right = provenance.get("right") or {}
    if right.get("label") != "rust" or right.get("binarySha256") != (manifest.get("rustCandidate") or {}).get(
            "binarySha256"):
        stale.append("side b is not the manifest's Rust candidate")
    if provenance.get("golden") != t2.golden_record().get("goldenSha256"):
        stale.append("twins did not boot from the pinned golden")
    if provenance.get("harnessSha256") != runner.harness_sha256():
        stale.append("harness changed since the capture")
    if provenance.get("driverSha256") != soak.driver_sha256():
        stale.append("soak driver changed since the capture")
    if provenance.get("changedDuringRun") is not False:
        stale.append("a binary, the harness or the driver changed during the capture")
    if summary.get("crashed") is not False:
        stale.append("a twin crashed during the round")
    if not spec.get("thresholds") or not thresholds_path.is_file():
        stale.append("declared thresholds missing")
    elif not (provenance.get("thresholdsSha256") == spec.get("thresholdsSha256") == _sha(thresholds_path)):
        stale.append("thresholds are not the declared ones in force when the round started")
    minutes = provenance.get("minutes")
    if not isinstance(minutes, (int, float)) or minutes < spec.get("minMinutes", 15):
        stale.append(f"shorter than {spec.get('minMinutes', 15)} minutes")
    files = summary.get("files") or {}
    if set(files) != RECEIPTS:
        stale.append("receipts are not both sides' ops, samples, events and final state")
    for name, digest in files.items():
        if not (bundle / name).is_file() or _sha(bundle / name) != digest:
            stale.append(f"{name} missing or changed")
    if not (bundle / "evaluation.json").is_file():
        stale.append("evaluation.json missing")
    if stale:
        for text in stale:
            problem(f"soak: {text}")
        return {"soak": UNVERIFIED}
    try:
        evaluation = soak.evaluate(bundle, json.loads(thresholds_path.read_text()))
    except (KeyError, TypeError, ValueError, IndexError, OSError) as error:
        problem(f"soak: receipts do not re-derive ({type(error).__name__}: {error})")
        return {"soak": FAIL}
    if json.loads((bundle / "evaluation.json").read_text()) != json.loads(json.dumps(evaluation)):
        problem("soak: recorded evaluation differs from the re-derived one")
    expected = int(minutes * 60 / provenance["config"]["intervalSec"])
    if evaluation["iterations"] != {"a": expected, "b": expected}:
        problem("soak: a side did not run every scheduled iteration")
    return items(evaluation)
