"""Negative tests for the T2 receipt verifier (parity/t2_verify.py).

A synthetic capture is written through the same receipt writer the T2
runner uses (snapshot store, receipt hashes), verified clean, then damaged
one way at a time. No VM or agent binary is needed.
"""

import gzip
import hashlib
import json
import shutil
import tempfile
import unittest
from pathlib import Path

from parity import t2, t2_verify

GO_SHA = "a" * 64
RUST_SHA = "b" * 64
GOLDEN = {"goldenSha256": "c" * 64}


def _sha(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def _snapshot(extra: dict | None = None) -> dict:
    files = {"/etc/hosts": {"type": "file", "mode": "0o644", "owner": "root:root", "sha256": "0" * 64}}
    files.update(extra or {})
    manager = {"running": True, "unitFiles": {}, "units": {}}
    return {"files": files, "system": dict(manager), "user": {"running": False}, "linger": [], "firewall": {}}


SCENARIO = {
    "id": "t2.synthetic", "tier": "T2", "owner": "host", "commands": [],
    "steps": [
        {"as": "baseline", "snapshot": {}},
        {"as": "call", "mcp": {"method": "tools/call", "name": "x", "params": {}}},
        {"as": "after", "snapshot": {"baseline": "baseline"}},
        {"as": "empty", "snapshot": {"baseline": "baseline", "expectEmpty": True}},
    ],
    "compare": {"masks": []},
}


def _raw(changed: bool) -> dict:
    after = _snapshot({"/home/ubuntu/x": {"type": "file", "mode": "0o600", "owner": "ubuntu:ubuntu",
                                          "sha256": "1" * 64}} if changed else None)
    return {"steps": {"baseline": {"snapshot": _snapshot()},
                      "call": {"status": 200, "body": {"result": {"ok": True}}},
                      "after": {"snapshot": after}, "empty": {"snapshot": _snapshot()}},
            "variables": {}}


class T2VerifyTest(unittest.TestCase):
    def setUp(self) -> None:
        self.root = Path(tempfile.mkdtemp(prefix="t2-verify-test-"))
        self.addCleanup(shutil.rmtree, self.root)
        bundle = self.root / "evidence" / "t2"
        sdir = bundle / "scenarios" / SCENARIO["id"]
        sdir.mkdir(parents=True)
        out = {side: {"raw": _raw(changed=True), "error": None, "twin": {}} for side in "ab"}
        outcome = t2.judge(SCENARIO, out["a"], out["b"], "go", "rust")
        for side, label in (("a", "go"), ("b", "rust")):
            (sdir / f"{side}0.json.gz").write_bytes(t2._gz(t2._store_snapshots(
                {"label": label, **out[side], "observation": outcome["observation"][side]}, bundle / "snapshots")))
        (sdir / "diff0.json").write_text(json.dumps({
            "diff": outcome["diff"], "maskViolations": outcome["maskViolations"],
            "unexpectedEffects": outcome["unexpectedEffects"], "harnessErrors": outcome["harnessErrors"],
            "divergences": outcome["divergences"],
            "errors": {"a": None, "b": None}}, indent=1, sort_keys=True) + "\n")
        receipts = {n: _sha(sdir / n) for n in ("a0.json.gz", "b0.json.gz", "diff0.json")}
        self.summary = {
            "tier": "T2", "left": {"label": "go", "sha256": GO_SHA}, "right": {"label": "rust", "sha256": RUST_SHA},
            "changedDuringRun": False,
            "golden": GOLDEN, "guestSha256": {g: _sha(t2.GUEST_DIR / g) for g in t2.GUEST_FILES},
            "harnessSha256": _sha(Path(t2.__file__)),
            "scenarioSha256": {str(p.relative_to(t2.REPO_ROOT)): _sha(p) for p in sorted(t2.SCENARIO_DIR.glob("*.json"))},
            "repeat": 1,
            "scenarios": {SCENARIO["id"]: {"status": "pass", "iterations": [
                {"iteration": 0, "pass": True, "diffCount": 0, "receipts": receipts}]}},
        }
        self.summary_path = bundle / "summary.json"
        self.manifest = {"goReference": {"binarySha256": GO_SHA}, "rustCandidate": {"binarySha256": RUST_SHA},
                         "t2Evidence": {"golden": GOLDEN, "suites": {}}}
        self.sdir = sdir
        self.bundle = bundle

    def verify(self) -> tuple[dict, list[str]]:
        self.summary_path.write_text(json.dumps(self.summary))
        problems: list[str] = []
        suite = {"summary": "evidence/t2/summary.json", "minRepeat": 1, "right": "rust"}
        statuses = t2_verify.check_suite(self.root, self.manifest, "go-vs-rust", suite,
                                         {SCENARIO["id"]: SCENARIO}, problems.append)
        return statuses, problems

    def test_clean_capture_passes(self) -> None:
        statuses, problems = self.verify()
        self.assertEqual(problems, [])
        self.assertEqual(statuses, {SCENARIO["id"]: "pass"})

    def test_modified_receipt_fails(self) -> None:
        with (self.sdir / "diff0.json").open("a") as f:
            f.write(" ")
        statuses, problems = self.verify()
        self.assertNotEqual(statuses.get(SCENARIO["id"]), "pass")
        self.assertTrue(any("receipt missing or modified" in p for p in problems))

    def test_tampered_snapshot_fails(self) -> None:
        store = self.bundle / "snapshots"
        victim = sorted(store.iterdir())[0]
        data = json.loads(gzip.decompress(victim.read_bytes()))
        data["linger"] = ["ubuntu"]
        victim.write_bytes(gzip.compress(json.dumps(data).encode()))
        statuses, problems = self.verify()
        self.assertEqual(statuses.get(SCENARIO["id"]), "fail")
        self.assertTrue(any("snapshot store" in p for p in problems))

    def test_effect_under_expect_empty_fails(self) -> None:
        # Both twins change the host identically under an expectEmpty
        # snapshot and every receipt is self-consistent: equal twins are
        # still not enough.
        records = {}
        for side in "ab":
            rec = json.loads(gzip.decompress((self.sdir / f"{side}0.json.gz").read_bytes()))
            rec["raw"]["steps"]["empty"] = rec["raw"]["steps"]["after"]
            records[side] = rec
        out = {side: {"raw": t2_verify._restore(rec, self.bundle / "snapshots"), "error": None}
               for side, rec in records.items()}
        outcome = t2.judge(SCENARIO, out["a"], out["b"], "go", "rust")
        self.assertTrue(outcome["unexpectedEffects"]["a"])
        self.assertEqual(outcome["diff"], [])
        for side, rec in records.items():
            rec["observation"] = outcome["observation"][side]
            (self.sdir / f"{side}0.json.gz").write_bytes(t2._gz(rec))
        (self.sdir / "diff0.json").write_text(json.dumps({
            "diff": outcome["diff"], "maskViolations": outcome["maskViolations"],
            "unexpectedEffects": outcome["unexpectedEffects"], "harnessErrors": outcome["harnessErrors"],
            "divergences": outcome["divergences"],
            "errors": {"a": None, "b": None}}, indent=1, sort_keys=True) + "\n")
        it = self.summary["scenarios"][SCENARIO["id"]]["iterations"][0]
        it["receipts"] = {n: _sha(self.sdir / n) for n in it["receipts"]}
        statuses, problems = self.verify()
        self.assertEqual(problems, [])
        self.assertEqual(statuses.get(SCENARIO["id"]), "fail")

    def test_unknown_tool_is_not_evidence(self) -> None:
        obs = {"steps": {"call": {"status": 400, "body": {"error": {"code": -32602,
                                                                    "message": "unknown tool \"x\""}}},
                         "poll": {"unbound": ["T_CALL"]}}}
        self.assertEqual(set(t2.harness_errors(obs)), {"call", "poll"})

    def test_wrong_binary_is_stale(self) -> None:
        self.summary["right"]["sha256"] = "d" * 64
        statuses, problems = self.verify()
        self.assertEqual(statuses, {})
        self.assertTrue(any("Rust candidate" in p for p in problems))

    def test_golden_mismatch_is_stale(self) -> None:
        self.summary["golden"] = {"goldenSha256": "e" * 64}
        statuses, problems = self.verify()
        self.assertEqual(statuses, {})
        self.assertTrue(any("golden" in p for p in problems))

    def test_too_few_iterations_fail(self) -> None:
        self.summary["repeat"] = 2
        problems: list[str] = []
        self.summary_path.write_text(json.dumps(self.summary))
        statuses = t2_verify.check_suite(self.root, self.manifest, "go-vs-rust",
                                         {"summary": "evidence/t2/summary.json", "minRepeat": 1, "right": "rust"},
                                         {SCENARIO["id"]: SCENARIO}, problems.append)
        self.assertEqual(statuses.get(SCENARIO["id"]), "fail")


if __name__ == "__main__":
    unittest.main()
