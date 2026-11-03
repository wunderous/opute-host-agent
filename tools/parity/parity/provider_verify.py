"""Fail-closed verification of committed M9 provider evidence (D26).

A ported provider passes only when its committed receipts reproduce it:

- wire: every corpus recording is the declared Go executable and the
  declared Rust executable, and re-comparing them (provider_wire.py's
  differences) finds no step that differs. A step whose fake requests differ
  only in order passes only where the manifest declares that Go ranges over
  a map there; the Go/Go recording shows no difference either.
- swap: the provider-real scenarios ran under one Rust core (the manifest's
  candidate) on both sides, the pinned Go executable on the left and the
  declared Rust executable on the right, every iteration passing with no
  divergence.
- t2 (where declared): the T2 provider swap passed every scenario with the
  declared Rust executable on the right.
- release (for a shipped provider): the declared Rust executable is the one
  in the committed Rust release set.

Receipts are pinned by hash in the manifest; absent or changed fails closed.
"""
from __future__ import annotations

import gzip
import hashlib
import importlib.util
import json
from pathlib import Path
from typing import Callable

PASS, FAIL, UNVERIFIED = "pass", "fail", "unverified"
_HERE = Path(__file__).resolve().parent


def _sha(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def _wire_module():
    spec = importlib.util.spec_from_file_location("provider_wire", _HERE.parent / "provider_wire.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _load(root: Path, entry: dict, problem: Callable[[str], None], what: str):
    path = root / entry.get("path", "")
    if not entry.get("path") or not path.is_file():
        problem(f"{what}: {entry.get('path') or 'no path'} missing")
        return None
    if _sha(path) != entry.get("sha256"):
        problem(f"{what}: {entry['path']} is not the reviewed file")
        return None
    data = path.read_bytes()
    if path.suffix == ".gz":
        data = gzip.decompress(data)
    return json.loads(data)


def _wire(root: Path, name: str, spec: dict, go_sha: str, rust_sha: str, problem) -> str:
    wire = _wire_module()
    corpora = spec.get("wire") or []
    if not corpora:
        problem(f"{name}: no wire corpus")
        return UNVERIFIED
    status = PASS
    for corpus in corpora:
        label = f"{name} wire {corpus.get('calls', '?')}"
        docs = {side: _load(root, corpus.get(side) or {}, problem, f"{label} {side}") for side in ("go", "goAgain", "rust")}
        if any(doc is None for doc in docs.values()):
            status = UNVERIFIED
            continue
        for side, want in (("go", go_sha), ("goAgain", go_sha), ("rust", rust_sha)):
            if docs[side].get("executableSha256") != want:
                problem(f"{label}: the {side} recording is not the declared executable")
                status = FAIL
        declared = corpus.get("orderOnly") or {}
        for pair in (("go", "goAgain"), ("go", "rust")):
            differ, order_only = wire.differences(docs[pair[0]], docs[pair[1]])
            if differ:
                problem(f"{label}: {pair[0]}/{pair[1]}: {len(differ)} step(s) differ: {', '.join(differ[:5])}")
                status = FAIL
            undeclared = [step for step in order_only if not any(key in step for key in declared)]
            if undeclared:
                problem(f"{label}: {pair[0]}/{pair[1]}: request order differs in undeclared steps: "
                        f"{', '.join(undeclared[:5])}")
                status = FAIL
    return status


def _swap(root: Path, name: str, spec: dict, manifest: dict, rust_sha: str, problem) -> str:
    entry = spec.get("swap") or {}
    summary = _load(root, entry, problem, f"{name} swap")
    if summary is None:
        return UNVERIFIED
    status = PASS
    provenance = summary.get("provenance") or {}
    left, right = provenance.get("left") or {}, provenance.get("right") or {}
    core = (manifest.get("rustCandidate") or {}).get("binarySha256")
    if left.get("binarySha256") != core or right.get("binarySha256") != core:
        problem(f"{name} swap: both sides must run the manifest's Rust candidate core")
        status = FAIL
    if left.get("providerSha256s"):
        problem(f"{name} swap: the left side must run the pinned Go providers")
        status = FAIL
    if (right.get("providerSha256s") or {}).get(f"opute-provider-{name}") != rust_sha:
        problem(f"{name} swap: the right side did not run the declared Rust executable")
        status = FAIL
    if provenance.get("changedDuringRun"):
        problem(f"{name} swap: something changed during the run")
        status = FAIL
    items = summary.get("items") or {}
    wanted = entry.get("scenarios") or []
    if not wanted or set(wanted) - set(items):
        problem(f"{name} swap: declared scenarios missing from the summary")
        status = FAIL
    for sid in wanted:
        item = items.get(sid) or {}
        if item.get("divergences") or item.get("failingIterations") or not item.get("iterations"):
            problem(f"{name} swap: {sid} did not pass")
            status = FAIL
    return status


def _t2(root: Path, name: str, spec: dict, rust_sha: str, problem) -> str:
    entries = spec.get("t2") or []
    status = PASS
    for entry in entries:
        summary = _load(root, entry, problem, f"{name} t2 {entry.get('path', '')}")
        if summary is None:
            status = UNVERIFIED
            continue
        if summary.get("changedDuringRun"):
            problem(f"{name} t2: something changed during the run")
            status = FAIL
        if (summary.get("left") or {}).get("providerSha256s"):
            problem(f"{name} t2: the left side must run the pinned Go provider")
            status = FAIL
        if ((summary.get("right") or {}).get("providerSha256s") or {}).get(f"opute-provider-{name}") != rust_sha:
            problem(f"{name} t2: the right side did not run the declared Rust executable")
            status = FAIL
        scenarios = summary.get("scenarios") or {}
        bad = sorted(sid for sid, row in scenarios.items() if row.get("status") != "pass")
        if not scenarios or bad:
            problem(f"{name} t2: {', '.join(bad[:5]) if bad else 'no scenarios'} not pass")
            status = FAIL
    return status


def _release(root: Path, name: str, spec: dict, evidence: dict, rust_sha: str, problem) -> str:
    artifact = spec.get("artifact")
    if not artifact:
        return PASS  # not in the release set (hostos)
    releases = _load(root, evidence.get("releases") or {}, problem, "release set")
    if releases is None:
        return UNVERIFIED
    listed = {}
    for line in ((releases.get("rust") or {}).get("text") or "").splitlines():
        digest, _, file = line.partition("  ")
        listed[file] = digest
    if listed.get(artifact) != rust_sha:
        problem(f"{name}: {artifact} in the Rust release set is not the declared executable")
        return FAIL
    return PASS


def check(root: Path, manifest: dict, providers: list[str], problem: Callable[[str], None]) -> dict[str, str]:
    """One status per provider and part (`<provider>:<part>`)."""
    evidence = manifest.get("providerEvidence") or {}
    declared = evidence.get("providers") or {}
    out: dict[str, str] = {}
    for name in providers:
        spec = declared.get(name)
        if not spec:
            problem(f"{name}: no provider evidence")
            out[f"{name}:evidence"] = UNVERIFIED
            continue
        go_sha, rust_sha = spec.get("goSha256"), spec.get("rustSha256")
        out[f"{name}:wire"] = _wire(root, name, spec, go_sha, rust_sha, problem)
        out[f"{name}:swap"] = _swap(root, name, spec, manifest, rust_sha, problem)
        if spec.get("t2"):
            out[f"{name}:t2"] = _t2(root, name, spec, rust_sha, problem)
        out[f"{name}:release"] = _release(root, name, spec, evidence, rust_sha, problem)
    return out
