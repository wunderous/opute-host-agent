"""Fail-closed verification of committed M12a rehearsal evidence.

The rehearsal round passes only when its receipts reproduce it: outcome.json
and results.json match the hashes its summary recorded; the summary's
provenance is the pinned Go tree, the manifest's Go reference and Rust
candidate, the current harness and rehearsal driver, at least the manifest's
repeat count, nothing changed during the capture, and no Incus binary on the
agents' PATH; and re-deriving every check from outcome.json
(parity/rehearsal.py's evaluate) gives the recorded results, all passing.
"""
from __future__ import annotations

import hashlib
import json
from pathlib import Path
from typing import Callable

from . import rehearsal, runner

PASS, FAIL, UNVERIFIED = "pass", "fail", "unverified"
CHECKS = ("snapshotUnchanged", "originInFlightAtStop", "forwardEqualsGoRestart", "restartStable",
          "rollbackKeepsTargetWork", "identityKept", "cleanStops")


def _sha(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def items(results: dict) -> dict[str, str]:
    """One pass/fail per rehearsal property and drill."""
    out: dict[str, str] = {}
    for target in ("go", "rust"):
        runs = results["checks"][target]
        for check in CHECKS:
            out[f"{target}:{check}"] = PASS if runs and all(r.get(check) is True for r in runs) else FAIL
    out["goControlEqualsRust"] = PASS if results["comparisons"] and not any(results["comparisons"]) else FAIL
    for drill, diff in sorted(results["drills"].items()):
        out[f"drill:{drill}"] = FAIL if diff else PASS
    return out


def check(root: Path, manifest: dict, lock: dict, problem: Callable[[str], None]) -> dict[str, str]:
    spec = manifest.get("rehearsalEvidence") or {}
    path = root / spec.get("summary", "")
    if not spec.get("summary") or not path.is_file():
        problem("rehearsal: summary missing")
        return {}
    summary = json.loads(path.read_text())
    bundle = path.parent
    provenance = summary.get("provenance") or {}
    stale = []
    if provenance.get("sourceLock") != lock:
        stale.append("not captured from the pinned Go tree")
    if provenance.get("goBinarySha256") != (manifest.get("goReference") or {}).get("binarySha256"):
        stale.append("Go binary is not the manifest's Go reference")
    if provenance.get("rustBinarySha256") != (manifest.get("rustCandidate") or {}).get("binarySha256"):
        stale.append("Rust binary is not the manifest's candidate")
    if provenance.get("harnessSha256") != runner.harness_sha256():
        stale.append("harness changed since the capture")
    if provenance.get("driverSha256") != runner.sha256_file(Path(rehearsal.__file__)):
        stale.append("rehearsal driver changed since the capture")
    if provenance.get("changedDuringRun") is not False:
        stale.append("a binary, the harness or the driver changed during the capture")
    if provenance.get("noIncusOnPath") is not True:
        stale.append("an Incus binary was reachable from the agents' PATH")
    if not isinstance(provenance.get("repeat"), int) or provenance["repeat"] < spec.get("minRepeat", 1):
        stale.append(f"fewer than {spec.get('minRepeat', 1)} rehearsals per side")
    for name, digest in (summary.get("files") or {}).items():
        if not (bundle / name).is_file() or _sha(bundle / name) != digest:
            stale.append(f"{name} missing or changed")
    if set(summary.get("files") or {}) != {"outcome.json", "results.json"}:
        stale.append("receipts are not outcome.json and results.json")
    if stale:
        for text in stale:
            problem(f"rehearsal: {text}")
        return {"rehearsal": UNVERIFIED}
    outcome = json.loads((bundle / "outcome.json").read_text())
    if any(len(outcome["rehearsals"][side]) != provenance["repeat"] for side in ("go", "rust")):
        problem("rehearsal: outcome does not hold the recorded number of rehearsals")
        return {"rehearsal": UNVERIFIED}
    try:
        results = rehearsal.evaluate(outcome)
    except (KeyError, TypeError, ValueError, IndexError) as error:
        problem(f"rehearsal: outcome does not re-derive ({type(error).__name__}: {error})")
        return {"rehearsal": FAIL}
    if json.loads((bundle / "results.json").read_text()) != results:
        problem("rehearsal: recorded results differ from the re-derived ones")
    if results["pass"] is not summary.get("pass"):
        problem("rehearsal: summary pass differs from the re-derived result")
    return items(results)
