"""Fail-closed verification of committed M10 distribution evidence.

The distribution round passes only when its receipts reproduce it: every
receipt matches the hash its summary recorded, the summary's provenance
matches the manifest (the pinned Go tree, both release sets by their
SHA256SUMS, the clean goldens) and the current harness files, nothing
changed during the capture, and re-deriving every place's result from the
receipts (parity/dist.py's evaluators) passes. The preview-site difference
must be exactly the reviewed manifest list.
"""
from __future__ import annotations

import hashlib
import json
from pathlib import Path
from typing import Callable

from . import dist

PASS, FAIL, UNVERIFIED = "pass", "fail", "unverified"


def _sha(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def check(root: Path, manifest: dict, lock: dict, problem: Callable[[str], None]) -> dict[str, str]:
    """Place -> pass/fail for the committed round; problems make it unverified."""
    spec = manifest.get("distEvidence") or {}
    path = root / spec.get("summary", "")
    if not spec.get("summary") or not path.is_file():
        problem("distribution: summary missing")
        return {}
    summary = json.loads(path.read_text())
    bundle = path.parent
    stale = []
    if summary.get("tier") != "dist":
        stale.append("not a distribution summary")
    if summary.get("changedDuringRun") is not False:
        stale.append("harness or a release set changed during the capture")
    if summary.get("goSourceCommit") != lock.get("sourceCommit"):
        stale.append("not captured from the pinned Go tree")
    if summary.get("version") != dist.VERSION or lock.get("publishedPackage", "").rsplit("@", 1)[-1] != dist.VERSION:
        stale.append("release version is not the pinned package version")
    for side, pinned in (spec.get("releases") or {}).items():
        if summary.get("releases", {}).get(side) != pinned.get("sha256sums"):
            stale.append(f"{side} release set is not the pinned one")
    if set(summary.get("releases", {})) != {"go", "rust"}:
        stale.append("release sets are not go and rust")
    distros = spec.get("distros") or []
    for distro in distros:
        if summary.get("goldens", {}).get(distro) != (spec.get("goldens") or {}).get(distro):
            stale.append(f"{distro} golden is not the pinned one")
    if summary.get("harness") != dist.harness_hashes():
        stale.append("harness files changed since the capture")
    receipts = {}
    for name, digest in (summary.get("receipts") or {}).items():
        receipt = bundle / f"{name}.json"
        if not receipt.is_file() or _sha(receipt) != digest:
            stale.append(f"receipt {name} missing or changed")
            continue
        receipts[name] = json.loads(receipt.read_text())
    required = {"releases", "artifacts", "logs", "npm", "site"} | {f"machines-{d}" for d in distros}
    if not required <= set(receipts):
        stale.append(f"receipts missing: {sorted(required - set(receipts))}")
    if stale:
        for text in stale:
            problem(f"distribution: {text}")
        return {name: UNVERIFIED for name in sorted(required)}
    if any(hashlib.sha256(receipts["releases"][side]["text"].encode()).hexdigest() != summary["releases"][side]
           for side in ("go", "rust")):
        problem("distribution: recorded SHA256SUMS text does not hash to the summary's release set")
        return {name: UNVERIFIED for name in sorted(required)}
    reviewed = [entry["file"] for entry in (spec.get("reviewedSiteDiff") or [])]
    try:
        results = dist.evaluate_round(receipts, reviewed, distros)
    except (KeyError, TypeError, ValueError, IndexError) as error:
        problem(f"distribution: receipts do not re-derive ({type(error).__name__}: {error})")
        return {name: FAIL for name in sorted(required)}
    recorded = json.loads((bundle / "results.json").read_text()) if (bundle / "results.json").is_file() else None
    if recorded is not None and recorded != results:
        problem("distribution: recorded results differ from the re-derived ones")
    return {name: PASS if result.get("pass") else FAIL for name, result in sorted(results.items())}
