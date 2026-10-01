"""Negative tests for the fail-closed verifier (milestones.md §2.5.3).

Each test builds a self-consistent synthetic evidence tree in a temp dir,
checks the gate passes, then damages one thing and checks the gate fails.
No Go or Rust binary is needed.
"""

import copy
import gzip
import hashlib
import json
import shutil
import tempfile
import unittest
from pathlib import Path

from parity import canaries, canon, contract, oracle, runner, verify

GO_SHA = "a" * 64
RUST_SHA = "b" * 64


def _sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


class Tree:
    def __init__(self) -> None:
        self.root = Path(tempfile.mkdtemp(prefix="verify-test-"))
        self.lock = json.loads((runner.REPO_ROOT / "baseline/source-lock.json").read_text())
        (self.root / "baseline/inventory").mkdir(parents=True)
        lock_bytes = (runner.REPO_ROOT / "baseline/source-lock.json").read_bytes()
        (self.root / "baseline/source-lock.json").write_bytes(lock_bytes)
        self.scenarios = runner.load_scenarios()
        self.manifest = {
            "schemaVersion": 1,
            "sourceLock": "baseline/source-lock.json",
            "sourceLockSha256": _sha(lock_bytes),
            "goReference": {"binarySha256": GO_SHA},
            "decisions": {"D8": {"status": "approved"}},
            "rustCandidate": {"binarySha256": RUST_SHA},
            "evidence": {
                "go-vs-go": {"summary": "evidence/gg/summary.json", "right": "go", "minRepeat": 2},
                "go-vs-rust": {"summary": None, "right": "rust", "minRepeat": 1},
            },
            "canaries": {"results": "evidence/canaries.json", "expected": "canaries.json"},
            "oracles": {"results": "evidence/oracles.json"},
            "gates": {
                "m0": {"requirePass": ["go-vs-go"], "requireCanaries": True, "requireInventory": True},
                "cutover": {"requirePass": ["go-vs-go", "go-vs-rust"], "requireCanaries": True,
                            "requireInventory": True},
                "m1": {"requirePass": ["go-vs-go", {"suite": "go-vs-rust", "surfaces": ["cli", "config", "lifecycle"]}],
                       "requireCanaries": True, "requireInventory": True, "requireOracles": True},
                "issuance": {"requireContracts": ["oauth-issuance"]},
            },
            "contracts": {"oauth-issuance": {"results": "evidence/contracts/oauth-issuance/summary.json"}},
            "rustCanaries": {"results": "evidence/rust-canaries.json"},
            "items": [{"id": s["id"], "scenario": s["id"], "surface": s["surface"], "owner": s["owner"]}
                      for s in self.scenarios],
        }
        self.write_bundle("evidence/gg", "go", GO_SHA, repeat=2)
        self.write_inventory()
        self.write_canaries()
        self.write_oracles()
        self.write_contracts()
        self.save()

    def save(self) -> None:
        (self.root / "parity-manifest.json").write_text(json.dumps(self.manifest))

    def write_bundle(self, rel: str, right_label: str, right_sha: str, repeat: int,
                     dirty: set[str] = frozenset(), divergences: dict[str, list] | None = None) -> None:
        """`divergences` overrides the recorded divergences per scenario id;
        by default Go-vs-Rust records each declared one, not stale."""
        bundle = self.root / rel
        registry = canon.load_divergences(runner.DIVERGENCE_FILE)
        items = {}
        for s in self.scenarios:
            sdir = bundle / "scenarios" / s["id"]
            sdir.mkdir(parents=True, exist_ok=True)
            bad = s["id"] in dirty
            recorded = [] if right_label != "rust" else [
                {"id": d, "decision": registry[d]["decision"], "stale": False}
                for d in s.get("compare", {}).get("divergences", [])]
            if divergences and s["id"] in divergences:
                recorded = divergences[s["id"]]
            files = {
                "a.json.gz": gzip.compress(b"{}", mtime=0),
                "b.json.gz": gzip.compress(b"{}", mtime=0),
                "diff.json": json.dumps({"diff": [{"path": ["x"], "left": 1, "right": 2}] if bad else [],
                                         "maskViolations": {"a": [], "b": []},
                                         "divergences": recorded}).encode(),
                "iterations.json": json.dumps([{"iteration": i, "diffCount": int(bad), "maskViolations": 0}
                                               for i in range(repeat)]).encode(),
            }
            for name, data in files.items():
                (sdir / name).write_bytes(data)
            items[s["id"]] = {"scenarioSha256": s["_sha256"], "files": {n: _sha(d) for n, d in files.items()},
                              "status": "pass"}  # status deliberately always "pass": must not be trusted
        summary = {"provenance": {"left": {"label": "go", "binarySha256": GO_SHA},
                                  "right": {"label": right_label, "binarySha256": right_sha},
                                  "harnessSha256": runner.harness_sha256(),
                                  "sourceCommit": self.lock["sourceCommit"],
                                  "sourceTree": self.lock["sourceTree"], "repeat": repeat},
                   "items": items}
        (bundle / "summary.json").write_text(json.dumps(summary))

    def write_inventory(self) -> None:
        inv = self.root / "baseline/inventory"
        gaps = json.dumps({"count": 0, "unownedCount": 0, "items": []}).encode()
        (inv / "gaps.json").write_bytes(gaps)
        index = {"sourceCommit": self.lock["sourceCommit"], "sourceTree": self.lock["sourceTree"],
                 "goBinarySha256": GO_SHA, "files": {"gaps.json": _sha(gaps)}}
        (inv / "index.json").write_text(json.dumps(index))

    def write_canaries(self, drop: str | None = None, uncaught: str | None = None) -> None:
        expected = json.loads(canaries.CANARY_FILE.read_text())
        results = []
        for c in expected:
            if c["id"] == drop:
                continue
            failed = [] if c["id"] == uncaught else [c["scenario"]]
            results.append({"id": c["id"], "patchApplied": True, "failedScenarios": failed})
        doc = {"sourceCommit": self.lock["sourceCommit"], "goBinarySha256": GO_SHA,
               "harnessSha256": runner.harness_sha256(),
               "canaryFileSha256": runner.sha256_file(canaries.CANARY_FILE), "results": results}
        (self.root / "evidence").mkdir(exist_ok=True)
        (self.root / "evidence/canaries.json").write_text(json.dumps(doc))

    def write_oracles(self, control_exit: int = 1, rust_sha: str = RUST_SHA, rust_exit: int = 0) -> None:
        suites = json.loads(oracle.ORACLE_FILE.read_text())
        results = []
        for suite in suites:
            for label, sha, code in (("go", GO_SHA, 0), ("rust", rust_sha, rust_exit),
                                     (oracle.NEGATIVE_CONTROL[0], "c" * 64, control_exit)):
                results.append({"suite": suite["id"], "binary": label, "binarySha256": sha, "exit": code,
                                "passed": suite["tests"] if code == 0 else [], "failed": [],
                                "expectedTests": suite["tests"]})
        doc = {"sourceCommit": self.lock["sourceCommit"],
               "oracleFileSha256": runner.sha256_file(oracle.ORACLE_FILE), "results": results}
        (self.root / "evidence/oracles.json").write_text(json.dumps(doc))

    def write_contracts(self, failing: str | None = None, missing: str | None = None,
                        uncaught: str | None = None, baseline_red: bool = False,
                        contract_sha: str | None = None) -> None:
        doc = contract.load("oauth-issuance")
        bundle = self.root / "evidence/contracts/oauth-issuance"
        items = {}
        for sc in doc["scenarios"]:
            if sc["id"] == missing:
                continue
            data = gzip.compress(b"{}", mtime=0)
            (bundle / "scenarios" / sc["id"]).mkdir(parents=True, exist_ok=True)
            (bundle / "scenarios" / sc["id"] / "observation.json.gz").write_bytes(data)
            failures = ["x: status = 200, want 401"] if sc["id"] == failing else []
            items[sc["id"]] = {"status": "pass", "failures": failures,
                               "files": {"observation.json.gz": _sha(data)}}
        sha = contract_sha or contract.contract_sha256("oauth-issuance")
        summary = {"suite": "oauth-issuance", "items": items,
                   "provenance": {"impl": {"label": "rust", "binarySha256": RUST_SHA}, "contractSha256": sha}}
        (bundle / "summary.json").write_text(json.dumps(summary))
        expected = json.loads(contract.RUST_CANARY_FILE.read_text())
        results = [{"id": c["id"], "patchApplied": True,
                    "failedScenarios": [] if c["id"] == uncaught else [c["scenario"]]} for c in expected]
        doc = {"rustBinarySha256": RUST_SHA, "canaryFileSha256": runner.sha256_file(contract.RUST_CANARY_FILE),
               "contractSha256": {"oauth-issuance": contract.contract_sha256("oauth-issuance")},
               "baselineFailures": {"oauth-issuance": ["cc.no-secret"] if baseline_red else []},
               "results": results}
        (self.root / "evidence/rust-canaries.json").write_text(json.dumps(doc))

    def rust_evidence(self, dirty: set[str] = frozenset(),
                      divergences: dict[str, list] | None = None) -> None:
        self.write_bundle("evidence/gr", "rust", RUST_SHA, repeat=1, dirty=dirty, divergences=divergences)
        self.manifest["evidence"]["go-vs-rust"]["summary"] = "evidence/gr/summary.json"
        self.save()

    def run(self, gate: str = "m0") -> dict:
        return verify.verify(self.root / "parity-manifest.json", gate)

    def cleanup(self) -> None:
        shutil.rmtree(self.root, ignore_errors=True)


class VerifyTest(unittest.TestCase):
    def setUp(self) -> None:
        self.tree = Tree()

    def tearDown(self) -> None:
        self.tree.cleanup()

    def assertGate(self, passed: bool, gate: str = "m0") -> dict:
        report = self.tree.run(gate)
        self.assertEqual(report["gate"]["pass"], passed, verify.render(report))
        return report

    def test_consistent_tree_passes_m0(self):
        report = self.assertGate(True)
        self.assertTrue(all(r["go-vs-rust"] == "unverified" for r in report["items"].values()))

    def test_cutover_fails_without_rust_evidence(self):
        self.assertGate(False, "cutover")

    def test_missing_summary_fails(self):
        (self.tree.root / "evidence/gg/summary.json").unlink()
        self.assertGate(False)

    def test_modified_evidence_file_fails(self):
        sid = self.tree.scenarios[0]["id"]
        (self.tree.root / f"evidence/gg/scenarios/{sid}/diff.json").write_text('{"diff": []}')
        self.assertGate(False)

    def test_summary_status_is_not_trusted(self):
        tree = self.tree
        sid = tree.scenarios[0]["id"]
        tree.write_bundle("evidence/gg", "go", GO_SHA, repeat=2, dirty={sid})
        report = self.assertGate(False)
        self.assertEqual(report["items"][sid]["go-vs-go"], "fail")

    def test_too_few_repeats_is_stale(self):
        self.tree.write_bundle("evidence/gg", "go", GO_SHA, repeat=1)
        self.assertGate(False)

    def test_wrong_go_binary_is_stale(self):
        self.tree.manifest["goReference"]["binarySha256"] = "c" * 64
        self.tree.save()
        self.assertGate(False)

    def test_changed_source_lock_is_stale(self):
        lock = self.tree.root / "baseline/source-lock.json"
        data = json.loads(lock.read_text())
        data["sourceCommit"] = "0" * 40
        lock.write_text(json.dumps(data))
        self.assertGate(False)

    def test_changed_scenario_invalidates_its_evidence(self):
        sid = self.tree.scenarios[0]["id"]
        summary_path = self.tree.root / "evidence/gg/summary.json"
        summary = json.loads(summary_path.read_text())
        summary["items"][sid]["scenarioSha256"] = "d" * 64
        summary_path.write_text(json.dumps(summary))
        self.assertGate(False)

    def test_harness_change_invalidates_evidence(self):
        summary_path = self.tree.root / "evidence/gg/summary.json"
        summary = json.loads(summary_path.read_text())
        summary["provenance"]["harnessSha256"] = "e" * 64
        summary_path.write_text(json.dumps(summary))
        self.assertGate(False)

    def test_blocked_required_suite_fails(self):
        self.tree.manifest["evidence"]["go-vs-go"]["blocked"] = "no runner"
        self.tree.save()
        self.assertGate(False)

    def test_blocked_unrequired_suite_does_not_fail_m0(self):
        self.tree.manifest["evidence"]["sandbox-t2"] = {"blocked": "no disposable sandbox pool (D4)"}
        self.tree.save()
        report = self.assertGate(True)
        self.assertTrue(any("blocked" in p for p in report["problems"]))

    def test_uncaught_canary_fails(self):
        self.tree.write_canaries(uncaught="C2-auth-status")
        self.assertGate(False)

    def test_missing_canary_result_fails(self):
        self.tree.write_canaries(drop="C4-legacy-bypass-widened")
        self.assertGate(False)

    def test_modified_inventory_fails(self):
        (self.tree.root / "baseline/inventory/gaps.json").write_text('{"unownedCount": 0}')
        self.assertGate(False)

    def test_unowned_scenario_fails(self):
        self.tree.manifest["items"] = self.tree.manifest["items"][1:]
        self.tree.save()
        self.assertGate(False)

    def test_item_without_owner_fails(self):
        self.tree.manifest["items"][0]["owner"] = ""
        self.tree.save()
        self.assertGate(False)

    def test_unknown_gate_fails(self):
        self.assertFalse(self.tree.run("nonexistent")["gate"]["pass"])

    def test_scoped_gate_ignores_items_outside_its_surfaces(self):
        later = next(s["id"] for s in self.tree.scenarios if s["surface"] not in ("cli", "config", "lifecycle"))
        self.tree.rust_evidence(dirty={later})
        report = self.assertGate(True, "m1")
        self.assertEqual(report["items"][later]["go-vs-rust"], "fail")
        self.assertGate(False, "cutover")

    def test_scoped_gate_fails_on_its_own_surface(self):
        own = next(s["id"] for s in self.tree.scenarios if s["surface"] == "lifecycle")
        self.tree.rust_evidence(dirty={own})
        self.assertGate(False, "m1")

    # --- declared divergences (decision D8) ---

    def _diverging(self) -> dict:
        return next(s for s in self.tree.scenarios
                    if s["surface"] == "lifecycle" and s.get("compare", {}).get("divergences"))

    def test_declared_divergences_pass(self):
        self.tree.rust_evidence()
        self.assertGate(True, "m1")

    def test_stale_divergence_fails(self):
        s = self._diverging()
        recorded = [{"id": d, "decision": "D8", "stale": True} for d in s["compare"]["divergences"]]
        self.tree.rust_evidence(divergences={s["id"]: recorded})
        self.assertGate(False, "m1")

    def test_unapproved_decision_fails(self):
        self.tree.manifest["decisions"]["D8"]["status"] = "proposed"
        self.tree.rust_evidence()
        self.assertGate(False, "m1")

    def test_dropped_declaration_fails(self):
        s = self._diverging()
        self.tree.rust_evidence(divergences={s["id"]: []})
        self.assertGate(False, "m1")

    def test_divergence_never_applies_go_vs_go(self):
        s = self._diverging()
        self.tree.write_bundle("evidence/gg", "go", GO_SHA, repeat=2, divergences={
            s["id"]: [{"id": d, "decision": "D8", "stale": False} for d in s["compare"]["divergences"]]})
        self.assertGate(False, "m0")

    def test_stale_iteration_fails(self):
        s = self._diverging()
        self.tree.rust_evidence()
        sdir = self.tree.root / "evidence/gr/scenarios" / s["id"]
        data = json.dumps([{"iteration": 0, "diffCount": 0, "maskViolations": 0,
                            "staleDivergences": 1}]).encode()
        (sdir / "iterations.json").write_bytes(data)
        summary = json.loads((self.tree.root / "evidence/gr/summary.json").read_text())
        summary["items"][s["id"]]["files"]["iterations.json"] = _sha(data)
        (self.tree.root / "evidence/gr/summary.json").write_text(json.dumps(summary))
        self.assertGate(False, "m1")

    # --- contract suites and Rust canaries ---

    def test_contracts_pass(self):
        self.assertGate(True, "issuance")

    def test_failing_contract_scenario_fails(self):
        self.tree.write_contracts(failing="cc.no-secret")
        self.assertGate(False, "issuance")

    def test_missing_contract_scenario_fails(self):
        self.tree.write_contracts(missing="approval.deny")
        self.assertGate(False, "issuance")

    def test_stale_contract_fails(self):
        self.tree.write_contracts(contract_sha="0" * 64)
        self.assertGate(False, "issuance")

    def test_contract_for_other_binary_fails(self):
        self.tree.manifest["rustCandidate"]["binarySha256"] = "d" * 64
        self.tree.save()
        self.assertGate(False, "issuance")

    def test_uncaught_rust_canary_fails(self):
        self.tree.write_contracts(uncaught="K1-empty-secret")
        self.assertGate(False, "issuance")

    def test_red_unpatched_baseline_fails(self):
        self.tree.write_contracts(baseline_red=True)
        self.assertGate(False, "issuance")

    def test_contract_decision_must_be_approved(self):
        del self.tree.manifest["decisions"]["D8"]
        self.tree.save()
        self.assertGate(False, "issuance")

    def test_scoped_gate_requires_rust_evidence(self):
        self.assertGate(False, "m1")

    def test_oracle_negative_control_must_fail(self):
        self.tree.rust_evidence()
        self.tree.write_oracles(control_exit=0)
        self.assertGate(False, "m1")

    def test_oracle_must_cover_the_recorded_rust_binary(self):
        self.tree.rust_evidence()
        self.tree.write_oracles(rust_sha="d" * 64)
        self.assertGate(False, "m1")

    def test_oracle_failure_fails_gate(self):
        self.tree.rust_evidence()
        self.tree.write_oracles(rust_exit=1)
        self.assertGate(False, "m1")

    def test_malformed_manifest_fails(self):
        (self.tree.root / "parity-manifest.json").write_text("{not json")
        self.assertFalse(self.tree.run()["gate"]["pass"])


if __name__ == "__main__":
    unittest.main()
