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

from parity import canaries, canon, runner, verify

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
            "rustCandidate": {"binarySha256": None},
            "evidence": {
                "go-vs-go": {"summary": "evidence/gg/summary.json", "right": "go", "minRepeat": 2},
                "go-vs-rust": {"summary": None, "right": "rust", "minRepeat": 1},
            },
            "canaries": {"results": "evidence/canaries.json", "expected": "canaries.json"},
            "gates": {
                "m0": {"requirePass": ["go-vs-go"], "requireCanaries": True, "requireInventory": True},
                "cutover": {"requirePass": ["go-vs-go", "go-vs-rust"], "requireCanaries": True,
                            "requireInventory": True},
            },
            "items": [{"id": s["id"], "scenario": s["id"], "surface": s["surface"], "owner": s["owner"]}
                      for s in self.scenarios],
        }
        self.write_bundle("evidence/gg", "go", GO_SHA, repeat=2)
        self.write_inventory()
        self.write_canaries()
        self.save()

    def save(self) -> None:
        (self.root / "parity-manifest.json").write_text(json.dumps(self.manifest))

    def write_bundle(self, rel: str, right_label: str, right_sha: str, repeat: int,
                     dirty: set[str] = frozenset()) -> None:
        bundle = self.root / rel
        items = {}
        for s in self.scenarios:
            sdir = bundle / "scenarios" / s["id"]
            sdir.mkdir(parents=True, exist_ok=True)
            bad = s["id"] in dirty
            files = {
                "a.json.gz": gzip.compress(b"{}", mtime=0),
                "b.json.gz": gzip.compress(b"{}", mtime=0),
                "diff.json": json.dumps({"diff": [{"path": ["x"], "left": 1, "right": 2}] if bad else [],
                                         "maskViolations": {"a": [], "b": []}}).encode(),
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

    def test_malformed_manifest_fails(self):
        (self.tree.root / "parity-manifest.json").write_text("{not json")
        self.assertFalse(self.tree.run()["gate"]["pass"])


if __name__ == "__main__":
    unittest.main()
