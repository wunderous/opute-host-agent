"""Fail-closed parity verifier.

The verifier never trusts a status field it did not derive. It:

1. checks the manifest is keyed to the current baseline/source-lock.json;
2. checks every evidence bundle's provenance (Go commit/tree, Go binary hash,
   harness hash, scenario hashes) against the manifest and the working tree;
3. re-hashes every evidence file and recomputes each item's status from the
   diff and iteration files themselves;
4. evaluates the requested gate.

Anything missing, stale, malformed or blocked is `unverified` or `fail`,
never `pass`.
"""

from __future__ import annotations

import hashlib
import json
from pathlib import Path
from typing import Any

from . import runner

PASS, FAIL, UNVERIFIED = "pass", "fail", "unverified"


def _sha(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def _load(path: Path) -> tuple[Any, str | None]:
    try:
        return json.loads(path.read_text()), None
    except FileNotFoundError:
        return None, f"missing: {path}"
    except (ValueError, OSError) as exc:
        return None, f"malformed: {path}: {exc}"


class Report:
    """Problems are scoped so a gate fails only on what it depends on.

    Scope "global" (manifest, source lock, scenario ownership) always counts.
    """

    def __init__(self) -> None:
        self.problems: list[tuple[str, str]] = []
        self.items: dict[str, dict] = {}

    def problem(self, text: str, scope: str = "global") -> None:
        self.problems.append((scope, text))


def _check_suite(root: Path, manifest: dict, suite_name: str, suite: dict, lock: dict,
                 scenarios: dict[str, dict], report: Report) -> dict[str, str]:
    """Return scenario id -> derived status for one evidence suite."""
    statuses: dict[str, str] = {}
    if suite.get("blocked"):
        report.problem(f"{suite_name}: blocked ({suite['blocked']})", suite_name)
        return statuses
    path = suite.get("summary")
    if not path:
        return statuses
    summary, err = _load(root / path)
    if err:
        report.problem(f"{suite_name}: {err}", suite_name)
        return statuses
    try:
        prov = summary["provenance"]
        items = summary["items"]
    except (KeyError, TypeError):
        report.problem(f"{suite_name}: malformed summary", suite_name)
        return statuses
    stale = []
    if prov.get("sourceCommit") != lock["sourceCommit"] or prov.get("sourceTree") != lock["sourceTree"]:
        stale.append("Go source revision differs from source lock")
    go_sha = manifest["goReference"]["binarySha256"]
    if prov.get("left", {}).get("label") != "go" or prov["left"].get("binarySha256") != go_sha:
        stale.append("left side is not the pinned Go reference binary")
    expected_right = suite.get("right")
    right = prov.get("right", {})
    if expected_right == "go" and (right.get("label") != "go" or right.get("binarySha256") != go_sha):
        stale.append("right side is not the pinned Go reference binary")
    if expected_right == "rust":
        rust_sha = manifest.get("rustCandidate", {}).get("binarySha256")
        if not rust_sha or right.get("label") != "rust" or right.get("binarySha256") != rust_sha:
            stale.append("right side is not the recorded Rust candidate")
    if prov.get("harnessSha256") != runner.harness_sha256():
        stale.append("harness changed since evidence was produced")
    min_repeat = suite.get("minRepeat", 1)
    if int(prov.get("repeat", 0)) < min_repeat:
        stale.append(f"repeat {prov.get('repeat')} < required {min_repeat}")
    if stale:
        for s in stale:
            report.problem(f"{suite_name}: stale: {s}", suite_name)
        return statuses

    bundle = (root / path).parent
    for sid, item in items.items():
        scenario = scenarios.get(sid)
        if scenario is None:
            report.problem(f"{suite_name}/{sid}: scenario no longer exists", suite_name)
            continue
        if item.get("scenarioSha256") != scenario["_sha256"]:
            report.problem(f"{suite_name}/{sid}: stale: scenario changed since evidence was produced", suite_name)
            continue
        sdir = bundle / "scenarios" / sid
        ok = True
        for name, digest in (item.get("files") or {}).items():
            fpath = sdir / name
            if not fpath.exists() or _sha(fpath) != digest:
                report.problem(f"{suite_name}/{sid}: evidence file {name} missing or modified", suite_name)
                ok = False
        required = {"a.json.gz", "b.json.gz", "diff.json", "iterations.json"}
        if not required <= set(item.get("files") or {}):
            report.problem(f"{suite_name}/{sid}: evidence incomplete", suite_name)
            ok = False
        if not ok:
            continue
        diff, err = _load(sdir / "diff.json")
        iters, err2 = _load(sdir / "iterations.json")
        if err or err2 or not isinstance(iters, list):
            report.problem(f"{suite_name}/{sid}: malformed diff or iterations", suite_name)
            continue
        clean = (not diff.get("diff")
                 and not any(diff.get("maskViolations", {}).values())
                 and len(iters) >= min_repeat
                 and all(i.get("diffCount") == 0 and i.get("maskViolations") == 0
                         and not i.get("staleDivergences") for i in iters))
        if not _check_divergences(manifest, suite_name, sid, expected_right, scenario,
                                  diff.get("divergences") or [], report):
            clean = False
        statuses[sid] = PASS if clean else FAIL
    return statuses


def _check_divergences(manifest: dict, suite_name: str, sid: str, right: str | None,
                       scenario: dict, recorded: list, report: Report) -> bool:
    """Declared divergences: Go-vs-Rust only, every declaration applied, each
    citing an approved decision, none stale."""
    ok = True
    declared = set(scenario.get("compare", {}).get("divergences", []))
    applied = {d.get("id") for d in recorded}
    if right != "rust":
        if applied:
            report.problem(f"{suite_name}/{sid}: divergences applied outside Go-vs-Rust", suite_name)
            return False
        return True
    if applied != declared:
        report.problem(f"{suite_name}/{sid}: recorded divergences {sorted(applied)} "
                       f"differ from declared {sorted(declared)}", suite_name)
        ok = False
    decisions = manifest.get("decisions", {})
    for d in recorded:
        if (decisions.get(d.get("decision")) or {}).get("status") != "approved":
            report.problem(f"{suite_name}/{sid}: divergence {d.get('id')} cites "
                           f"unapproved decision {d.get('decision')!r}", suite_name)
            ok = False
        if d.get("stale"):
            report.problem(f"{suite_name}/{sid}: divergence {d.get('id')} is stale "
                           "(both sides agree; remove the declaration)", suite_name)
            ok = False
    return ok


def _check_canaries(root: Path, manifest: dict, lock: dict, report: Report,
                     waived: dict[str, str] | None = None) -> dict[str, str]:
    """A canary passes (for the harness) only when its expected scenario failed."""
    out: dict[str, str] = {}
    spec = dict(manifest.get("canaries", {}))
    if spec.get("expected") == "canaries.json":
        from . import canaries as canary_mod
        spec["expected"] = json.loads(canary_mod.CANARY_FILE.read_text())
    doc, err = _load(root / spec["results"]) if spec.get("results") else (None, "no canary results")
    if err:
        report.problem(f"canaries: {err}", "canaries")
        return {c["id"]: UNVERIFIED for c in spec.get("expected", [])}
    from . import canaries as canary_mod
    if (doc.get("sourceCommit") != lock["sourceCommit"]
            or doc.get("harnessSha256") != runner.harness_sha256()
            or doc.get("goBinarySha256") != manifest["goReference"]["binarySha256"]
            or doc.get("canaryFileSha256") != runner.sha256_file(canary_mod.CANARY_FILE)):
        report.problem("canaries: stale results (source, Go binary, harness or canary list changed)", "canaries")
        return {c["id"]: UNVERIFIED for c in spec.get("expected", [])}
    results = {r["id"]: r for r in doc.get("results", [])}
    for canary in spec.get("expected", []):
        result = results.get(canary["id"])
        if result is None:
            out[canary["id"]] = UNVERIFIED
            continue
        caught = canary["scenario"] in result.get("failedScenarios", [])
        others_clean = set(result.get("failedScenarios", [])) <= set(canary.get("mayAlsoFail", [])) | {canary["scenario"]}
        out[canary["id"]] = PASS if caught and others_clean and result.get("patchApplied") else FAIL
        if not caught:
            reason = (waived or {}).get(canary["id"])
            scope = "waivedCanaries" if reason else "canaries"
            msg = f"canary {canary['id']}: scenario {canary['scenario']} stayed green (harness gap)"
            if reason:
                msg += f" -- waived: {reason}"
            report.problem(msg, scope)
    return out


def _check_oracles(root: Path, manifest: dict, lock: dict, report: Report) -> str:
    """Go black-box tests: the Go reference and the recorded Rust candidate
    pass every expected test, and the negative control fails."""
    from . import oracle as oracle_mod
    spec = manifest.get("oracles") or {}
    doc, err = _load(root / spec["results"]) if spec.get("results") else (None, "no oracle results")
    if err:
        report.problem(f"oracles: {err}", "oracles")
        return UNVERIFIED
    if (doc.get("sourceCommit") != lock["sourceCommit"]
            or doc.get("oracleFileSha256") != runner.sha256_file(oracle_mod.ORACLE_FILE)):
        report.problem("oracles: stale results (source or oracle list changed)", "oracles")
        return UNVERIFIED
    expected_suites = {s["id"] for s in json.loads(oracle_mod.ORACLE_FILE.read_text())}
    want = {"go": manifest["goReference"]["binarySha256"],
            "rust": (manifest.get("rustCandidate") or {}).get("binarySha256")}
    ok = True
    for suite in sorted(expected_suites):
        rows = {r["binary"]: r for r in doc.get("results", []) if r.get("suite") == suite}
        for label, sha in want.items():
            row = rows.get(label)
            if row is None or not sha or row.get("binarySha256") != sha:
                report.problem(f"oracles/{suite}: no result for the recorded {label} binary", "oracles")
                ok = False
            elif row.get("exit") != 0 or not set(row.get("expectedTests", [])) <= set(row.get("passed", [])):
                report.problem(f"oracles/{suite}: {label} failed {row.get('failed')}", "oracles")
                ok = False
        control = rows.get(oracle_mod.NEGATIVE_CONTROL[0])
        if control is None or control.get("exit") == 0:
            report.problem(f"oracles/{suite}: negative control did not fail (overlay may not run the binary)", "oracles")
            ok = False
    return PASS if ok else FAIL


def _check_contracts(root: Path, manifest: dict, names: list[str], report: Report) -> str:
    """Single-implementation contract suites for declared divergences, and the
    Rust canaries that prove each suite can fail."""
    from . import contract as contract_mod
    rust_sha = (manifest.get("rustCandidate") or {}).get("binarySha256")
    decisions = manifest.get("decisions", {})
    ok = True
    for name in names:
        spec = (manifest.get("contracts") or {}).get(name)
        if not spec:
            report.problem(f"contracts/{name}: not declared in the manifest", "contracts")
            ok = False
            continue
        summary, err = _load(root / spec["results"])
        if err:
            report.problem(f"contracts/{name}: {err}", "contracts")
            ok = False
            continue
        try:
            doc = contract_mod.load(name)
        except (OSError, ValueError) as exc:
            report.problem(f"contracts/{name}: {exc}", "contracts")
            ok = False
            continue
        if (decisions.get(doc.get("decision")) or {}).get("status") != "approved":
            report.problem(f"contracts/{name}: decision {doc.get('decision')!r} is not approved", "contracts")
            ok = False
        prov = summary.get("provenance", {})
        if prov.get("impl", {}).get("label") != "rust" or not rust_sha or prov["impl"].get("binarySha256") != rust_sha:
            report.problem(f"contracts/{name}: stale: not run against the recorded Rust candidate", "contracts")
            ok = False
            continue
        if prov.get("contractSha256") != contract_mod.contract_sha256(name):
            report.problem(f"contracts/{name}: stale: contract or harness changed since the run", "contracts")
            ok = False
            continue
        bundle = (root / spec["results"]).parent
        items = summary.get("items", {})
        for scenario in doc["scenarios"]:
            item = items.get(scenario["id"])
            if item is None:
                report.problem(f"contracts/{name}/{scenario['id']}: no result", "contracts")
                ok = False
                continue
            for fname, digest in (item.get("files") or {}).items():
                fpath = bundle / "scenarios" / scenario["id"] / fname
                if not fpath.exists() or _sha(fpath) != digest:
                    report.problem(f"contracts/{name}/{scenario['id']}: evidence file {fname} missing or modified", "contracts")
                    ok = False
            if not item.get("files") or item.get("failures") != []:
                report.problem(f"contracts/{name}/{scenario['id']}: fails {item.get('failures')}", "contracts")
                ok = False
    canaries, err = _load(root / manifest["rustCanaries"]["results"]) if manifest.get("rustCanaries") else (None, "no Rust canary results")
    if err:
        report.problem(f"rust-canaries: {err}", "contracts")
        return FAIL
    expected = json.loads(contract_mod.RUST_CANARY_FILE.read_text())
    if (canaries.get("rustBinarySha256") != rust_sha
            or canaries.get("canaryFileSha256") != runner.sha256_file(contract_mod.RUST_CANARY_FILE)
            or any(canaries.get("contractSha256", {}).get(c["contract"]) != contract_mod.contract_sha256(c["contract"])
                   for c in expected)):
        report.problem("rust-canaries: stale results (Rust candidate, canary list or contract changed)", "contracts")
        return FAIL
    if any(canaries.get("baselineFailures", {}).values()):
        report.problem("rust-canaries: the unpatched Rust candidate failed its contract", "contracts")
        ok = False
    results = {r["id"]: r for r in canaries.get("results", [])}
    for canary in expected:
        result = results.get(canary["id"])
        failed = set((result or {}).get("failedScenarios", []))
        if (result is None or not result.get("patchApplied") or canary["scenario"] not in failed
                or not failed <= set(canary.get("mayAlsoFail", [])) | {canary["scenario"]}):
            report.problem(f"rust-canary {canary['id']}: not caught cleanly by {canary['scenario']}", "contracts")
            ok = False
    return PASS if ok else FAIL


def _check_inventory(root: Path, lock: dict, manifest: dict, report: Report) -> str:
    index, err = _load(root / "baseline/inventory/index.json")
    if err:
        report.problem(f"inventory: {err}", "inventory")
        return UNVERIFIED
    ok = True
    if index.get("sourceCommit") != lock["sourceCommit"] or index.get("sourceTree") != lock["sourceTree"]:
        report.problem("inventory: captured from a different Go revision", "inventory")
        ok = False
    if index.get("goBinarySha256") != manifest["goReference"]["binarySha256"]:
        report.problem("inventory: captured from a different Go binary", "inventory")
        ok = False
    for name, digest in index.get("files", {}).items():
        path = root / "baseline/inventory" / name
        if not path.exists() or _sha(path) != digest:
            report.problem(f"inventory: {name} missing or modified", "inventory")
            ok = False
    gaps, err = _load(root / "baseline/inventory/gaps.json")
    if err or gaps.get("unownedCount", 1) != 0:
        report.problem("inventory: unowned gaps or gaps file missing", "inventory")
        ok = False
    return PASS if ok else FAIL


def verify(manifest_path: Path, gate: str) -> dict:
    root = manifest_path.resolve().parent
    report = Report()
    manifest, err = _load(manifest_path)
    if err:
        return {"gate": {"name": gate, "pass": False}, "problems": [err], "items": {}}
    lock_path = root / manifest.get("sourceLock", "baseline/source-lock.json")
    lock, err = _load(lock_path)
    if err:
        return {"gate": {"name": gate, "pass": False}, "problems": [err], "items": {}}
    if manifest.get("sourceLockSha256") != _sha(lock_path):
        report.problem("manifest is not keyed to the current source lock (rebase required)")
    if gate not in manifest.get("gates", {}):
        return {"gate": {"name": gate, "pass": False}, "problems": [f"unknown gate {gate}"], "items": {}}
    rules = manifest["gates"][gate]

    scenarios = {s["id"]: s for s in runner.load_scenarios()}
    suites = {name: _check_suite(root, manifest, name, suite, lock, scenarios, report)
              for name, suite in manifest.get("evidence", {}).items()}
    canaries = _check_canaries(root, manifest, lock, report, rules.get("waivedCanaries", {}))
    inventory = _check_inventory(root, lock, manifest, report)

    for item in manifest.get("items", []):
        sid = item["scenario"]
        if sid not in scenarios:
            report.problem(f"item {item['id']}: scenario {sid} does not exist")
        row = {"surface": item.get("surface"), "owner": item.get("owner")}
        for suite in manifest.get("evidence", {}):
            row[suite] = suites[suite].get(sid, UNVERIFIED)
        report.items[item["id"]] = row
    manifest_ids = {i["scenario"] for i in manifest.get("items", [])}
    for sid in scenarios:
        if sid not in manifest_ids:
            report.problem(f"scenario {sid} has no manifest item (unowned surface)")
    for item in manifest.get("items", []):
        if not item.get("owner"):
            report.problem(f"item {item['id']} has no owner")

    failures: list[str] = []
    for req in rules.get("requirePass", []):
        suite = req if isinstance(req, str) else req["suite"]
        surfaces = None if isinstance(req, str) else set(req.get("surfaces", []))
        scoped = {i: row for i, row in report.items.items()
                  if surfaces is None or row.get("surface") in surfaces}
        if surfaces is not None and not scoped:
            failures.append(f"{suite}: no items for surfaces {sorted(surfaces)}")
        bad = [i for i, row in scoped.items() if row.get(suite) != PASS]
        if bad:
            failures.append(f"{suite}: {len(bad)} item(s) not pass: {', '.join(sorted(bad)[:8])}"
                            + (" ..." if len(bad) > 8 else ""))
    for suite in rules.get("requireNotFail", []):
        bad = [i for i, row in report.items.items() if row.get(suite) == FAIL]
        if bad:
            failures.append(f"{suite}: {len(bad)} item(s) fail")
    if rules.get("requireCanaries"):
        waived = rules.get("waivedCanaries", {})
        bad = [c for c, s in canaries.items() if s != PASS and c not in waived]
        if bad or not canaries:
            failures.append(f"canaries not all caught: {bad or 'none recorded'}")
    if rules.get("requireInventory") and inventory != PASS:
        failures.append(f"inventory {inventory}")
    if rules.get("requireNoGaps"):
        # Owned gaps are allowed while milestones are in flight, never at
        # cutover: passing scenarios only prove the surfaces they cover.
        gaps, err = _load(root / "baseline/inventory/gaps.json")
        count = None if err else gaps.get("count")
        if not isinstance(count, int) or count != 0:
            failures.append(f"inventory gaps remain: {count if isinstance(count, int) else 'unknown'}")
    oracles = _check_oracles(root, manifest, lock, report) if rules.get("requireOracles") else None
    if rules.get("requireOracles") and oracles != PASS:
        failures.append(f"oracles {oracles}")
    contracts = None
    if rules.get("requireContracts"):
        contracts = _check_contracts(root, manifest, rules["requireContracts"], report)
        if contracts != PASS:
            failures.append(f"contracts {contracts}")
    required_suites = {r if isinstance(r, str) else r["suite"] for r in rules.get("requirePass", [])}
    scopes = {"global"} | required_suites | set(rules.get("requireNotFail", []))
    if rules.get("requireOracles"):
        scopes.add("oracles")
    if rules.get("requireCanaries"):
        scopes.add("canaries")
    if rules.get("requireInventory"):
        scopes.add("inventory")
    if rules.get("requireContracts"):
        scopes.add("contracts")
    gating = [text for scope, text in report.problems if scope in scopes]
    if gating:
        failures.append(f"{len(gating)} provenance/evidence problem(s)")
    return {
        "gate": {"name": gate, "pass": not failures, "failures": failures},
        "problems": [f"[{scope}] {text}" for scope, text in report.problems],
        "items": report.items,
        "canaries": canaries,
        "inventory": inventory,
        "oracles": oracles,
        "contracts": contracts,
    }


def render(report: dict) -> str:
    lines = []
    gate = report["gate"]
    counts: dict[str, dict[str, int]] = {}
    for row in report.get("items", {}).values():
        for key, value in row.items():
            if value in (PASS, FAIL, UNVERIFIED):
                counts.setdefault(key, {PASS: 0, FAIL: 0, UNVERIFIED: 0})[value] += 1
    for suite, c in sorted(counts.items()):
        lines.append(f"  {suite:12} pass={c[PASS]} fail={c[FAIL]} unverified={c[UNVERIFIED]}")
    if report.get("canaries") is not None:
        c = report["canaries"]
        lines.append(f"  canaries     caught={sum(v == PASS for v in c.values())}/{len(c)}")
    if report.get("inventory"):
        lines.append(f"  inventory    {report['inventory']}")
    if report.get("oracles"):
        lines.append(f"  go oracles   {report['oracles']}")
    if report.get("contracts"):
        lines.append(f"  contracts    {report['contracts']}")
    for p in report.get("problems", [])[:20]:
        lines.append(f"  problem: {p}")
    for f in gate.get("failures", []):
        lines.append(f"  gate failure: {f}")
    lines.insert(0, f"gate {gate['name']}: {'PASS' if gate['pass'] else 'FAIL'}")
    return "\n".join(lines)
