"""Failing-first tests of the cutover readiness verifier (D28).

Each test builds a small evidence tree in a temporary root (a copy of one
committed e2e-0 receipt, a fake seam stage, a questions table) and checks
that missing, tampered or stale evidence fails its item.
"""
from __future__ import annotations

import json
import shutil
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))
import readiness  # noqa: E402

REPO = readiness.ROOT
RUN = "e2e-0-r4"
JOURNEY = "selfcheck.noop"


class Root:
    def __init__(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.path = Path(self.tmp.name)

    def write(self, rel: str, text: str) -> Path:
        path = self.path / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)
        return path

    def manifest(self, *items: dict) -> None:
        self.write(readiness.MANIFEST, json.dumps({"items": list(items)}))

    def gate(self, gates: dict | None = None) -> None:
        """One committed e2e-0 receipt and a gates.json that names it."""
        shutil.copytree(REPO / "evidence/e2e/runs" / RUN / JOURNEY, self.path / "evidence/e2e/runs" / RUN / JOURNEY)
        journeys = self.path / "evidence/e2e/journeys"
        journeys.mkdir(parents=True, exist_ok=True)
        shutil.copy(REPO / "evidence/e2e/journeys" / f"{JOURNEY}.json", journeys)
        self.write("evidence/e2e/gates.json", json.dumps(
            gates if gates is not None else {"e2e-0": {"run": RUN, "journeys": {JOURNEY: {"pass": True}}}}))

    def seam(self) -> dict:
        stage = self.path / "evidence/seam/s1"
        report = self.write("evidence/seam/s1/report.json", json.dumps({"gaps": [], "problems": [], "counts": {}}))
        log = self.write("evidence/seam/s1/verifier.log", "test seam::seam_verifier ... ok\n")
        self.write("evidence/seam/s1-validation.json", json.dumps({
            "reportSha256": readiness.sha256_file(report), "verifierLogSha256": readiness.sha256_file(log),
            "canary": {"red": True}}))
        self.write("crates/x/src/lib.rs", "fn main() {}\n")
        assert stage.exists()
        return {"id": "seam", "kind": "seam", "stage": "s1", "sources": ["crates"]}

    def status(self, item_id: str) -> str:
        return next(i for i in readiness.evaluate(self.path)["items"] if i["id"] == item_id)["status"]


class GateTest(unittest.TestCase):
    def test_a_verified_gate_passes(self):
        root = Root()
        root.gate()
        root.manifest({"id": "e2e-0", "kind": "e2e-gate", "gate": "e2e-0"})
        self.assertEqual(root.status("e2e-0"), "passed")

    def test_a_tampered_receipt_fails(self):
        root = Root()
        root.gate()
        results = root.path / "evidence/e2e/runs" / RUN / JOURNEY / "results.json"
        doc = json.loads(results.read_text())
        doc["pass"] = not doc["pass"]
        results.write_text(json.dumps(doc))
        root.manifest({"id": "e2e-0", "kind": "e2e-gate", "gate": "e2e-0"})
        self.assertEqual(root.status("e2e-0"), "failed")

    def test_a_missing_gate_fails(self):
        root = Root()
        root.gate()
        root.manifest({"id": "e2e-9", "kind": "e2e-gate", "gate": "e2e-9"})
        self.assertEqual(root.status("e2e-9"), "failed")

    def test_a_gate_whose_run_is_missing_fails(self):
        root = Root()
        root.gate({"e2e-0": {"run": "e2e-0-r99", "journeys": {JOURNEY: {"pass": True}}}})
        root.manifest({"id": "e2e-0", "kind": "e2e-gate", "gate": "e2e-0"})
        self.assertEqual(root.status("e2e-0"), "failed")

    def test_a_gate_expecting_questions_its_receipt_lacks_fails(self):
        root = Root()
        root.gate({"e2e-0": {"run": RUN, "journeys": {JOURNEY: {"pass": True, "unverified": ["Q99"]}}}})
        root.manifest({"id": "e2e-0", "kind": "e2e-gate", "gate": "e2e-0"})
        # verify.py itself refuses a receipt whose unverified set differs from gates.json.
        self.assertEqual(root.status("e2e-0"), "failed")

    def test_a_blocked_gate_is_blocked_with_its_owner(self):
        root = Root()
        root.write("evidence/e2e/gates.json", json.dumps({"e2e-5": {"run": None, "journeys": {}, "blocked": "no run"}}))
        root.manifest({"id": "e2e-5", "kind": "e2e-gate", "gate": "e2e-5", "owner": "Platform", "unblocker": "staging"})
        item = readiness.evaluate(root.path)["items"][0]
        self.assertEqual((item["status"], item["owner"], item["unblocker"]), ("blocked", "Platform", "staging"))


class ReceiptAndSeamTest(unittest.TestCase):
    def receipt(self, root: Root, passed: bool = True) -> dict:
        root.write("evidence/r.json", json.dumps({"pass": passed}))
        root.write("crates/x/src/lib.rs", "fn main() {}\n")
        item = {"id": "r", "kind": "receipt", "receipt": "evidence/r.json", "sources": ["crates"]}
        root.manifest(item)
        readiness.record(root.path, "r")
        return item

    def test_a_recorded_receipt_passes(self):
        root = Root()
        self.receipt(root)
        self.assertEqual(root.status("r"), "passed")

    def test_a_tampered_receipt_fails(self):
        root = Root()
        self.receipt(root)
        root.write("evidence/r.json", json.dumps({"pass": True, "x": 1}))
        self.assertEqual(root.status("r"), "failed")

    def test_a_stale_receipt_fails(self):
        root = Root()
        self.receipt(root)
        root.write("crates/x/src/lib.rs", "fn main() { changed(); }\n")
        self.assertEqual(root.status("r"), "failed")

    def test_a_failed_run_fails(self):
        root = Root()
        self.receipt(root, passed=False)
        self.assertEqual(root.status("r"), "failed")

    def test_a_missing_receipt_fails(self):
        root = Root()
        self.receipt(root)
        (root.path / "evidence/r.json").unlink()
        self.assertEqual(root.status("r"), "failed")

    def test_a_current_seam_report_passes(self):
        root = Root()
        root.manifest(root.seam())
        readiness.record(root.path, "seam")
        self.assertEqual(root.status("seam"), "passed")

    def test_a_stale_seam_report_fails(self):
        root = Root()
        root.manifest(root.seam())
        readiness.record(root.path, "seam")
        root.write("crates/x/src/lib.rs", "fn main() { changed(); }\n")
        self.assertEqual(root.status("seam"), "failed")

    def test_a_tampered_seam_report_fails(self):
        root = Root()
        root.manifest(root.seam())
        readiness.record(root.path, "seam")
        root.write("evidence/seam/s1/report.json", json.dumps({"gaps": [], "problems": [], "counts": {"x": 1}}))
        self.assertEqual(root.status("seam"), "failed")


class QuestionsTest(unittest.TestCase):
    TABLE = ("| Q | Topic | Answer |\n| --- | --- | --- |\n"
             "| Q1 | a | Answered (D28): spec. Spec. |\n"
             "| Q2 | b [fix-two](../fix-two/proposal.md) | Answered (D28): fix. Its proposal. |\n"
             "| Q3 | c | Open: the owner confirms. |\n"
             "| Q4 | d | Deferred (D28): not a blocker. |\n")

    def statuses(self, tasks: str) -> dict:
        root = Root()
        root.write("q.md", self.TABLE)
        root.write("openspec/changes/fix-two/tasks.md", tasks)
        root.manifest({"id": "questions", "kind": "questions", "path": "q.md"})
        return {i["id"]: i["status"] for i in readiness.evaluate(root.path)["items"]}

    def test_answered_open_and_deferred_rows(self):
        self.assertEqual(self.statuses("- [x] 1.1 done\n"),
                         {"Q1": "passed", "Q2": "passed", "Q3": "unverified", "Q4": "unverified"})

    def test_an_answered_question_whose_fix_is_unfinished_fails(self):
        self.assertEqual(self.statuses("- [x] 1.1 done\n- [ ] 2.3 rerun\n")["Q2"], "failed")


if __name__ == "__main__":
    unittest.main()
