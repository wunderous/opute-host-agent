"""Offline tests of the E2E journey harness: no VM, no agent.

  python3 -m unittest discover -s evidence/e2e/tests
"""
import json
import os
import sys
import tempfile
import time
import unittest
from pathlib import Path

E2E = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(E2E))
sys.path.insert(0, str(E2E.parent.parent / "tools/parity"))

import e2elib  # noqa: E402
import run as runner  # noqa: E402

SPEC = "openspec/specs/host-agent-contract/spec.md"


def journey(**over):
    base = {"id": "t", "gate": "e2e-0", "description": "d", "inputs": {}, "maxMinutes": 5,
            "requirements": [{"spec": SPEC, "requirement": "R", "pass": [
                {"step": "h", "path": "response.status", "equals": 200}]}],
            "effects": {"allow": [], "persist": []}, "steps": [{"id": "h", "health": True}], "cleanup": []}
    base.update(over)
    return base


def health(status=200):
    return {"step": "h", "phase": "step", "kind": "health", "response": {"status": status}, "ms": 5}


STATE = {"files": {"/etc/a": {"sha256": "1"}}}


class Evaluate(unittest.TestCase):
    def test_noop_passes(self):
        out = runner.evaluate(journey(), [health()], {"before": STATE, "after": STATE, "cleanup": STATE})
        self.assertTrue(out["pass"], out)

    def test_broken_pass_condition_fails(self):
        out = runner.evaluate(journey(), [health(418)], {"before": STATE, "after": STATE, "cleanup": STATE})
        self.assertEqual(out["failures"], ["pass-condition"])

    def test_missing_step_record_fails(self):
        out = runner.evaluate(journey(), [], {"before": STATE, "after": STATE, "cleanup": STATE})
        self.assertEqual(out["failures"], ["pass-condition"])

    def test_undeclared_effect_fails_and_declared_passes(self):
        after = {"files": {"/etc/a": {"sha256": "1"}, "/etc/b": {"sha256": "2"}}}
        states = {"before": STATE, "after": after, "cleanup": STATE}
        self.assertEqual(runner.evaluate(journey(), [health()], states)["failures"], ["undeclared-effect"])
        declared = journey(effects={"allow": ["files//etc/b"], "persist": []})
        self.assertTrue(runner.evaluate(declared, [health()], states)["pass"])

    def test_cleanup_residue_fails(self):
        after = {"files": {"/etc/a": {"sha256": "1"}, "/etc/b": {"sha256": "2"}}}
        declared = journey(effects={"allow": ["files//etc/b"], "persist": []})
        out = runner.evaluate(declared, [health()], {"before": STATE, "after": after, "cleanup": after})
        self.assertEqual(out["failures"], ["cleanup-residue"])

    def test_schema_violation_fails(self):
        tool = {"name": "p", "inputSchema": {"type": "object"},
                "outputSchema": {"type": "object", "required": ["n"], "properties": {"n": {"type": "integer"}}}}
        listed = {"step": "l", "phase": "step", "kind": "mcp", "method": "tools/list", "requestId": 1,
                  "response": {"status": 200, "body": {"jsonrpc": "2.0", "id": 1, "result": {"tools": [tool]}}}}
        called = {"step": "c", "phase": "step", "kind": "mcp", "method": "tools/call", "requestId": 2, "tool": "p",
                  "response": {"status": 200, "body": {"jsonrpc": "2.0", "id": 2, "result": {
                      "content": [], "structuredContent": {"n": "x"}}}}}
        j = journey(steps=[{"id": "l", "mcp": "tools/list"}, {"id": "c", "mcp": "tools/call", "tool": "p"}],
                    requirements=[{"spec": SPEC, "requirement": "R", "pass": [
                        {"step": "c", "path": "response.status", "equals": 200}]}])
        out = runner.evaluate(j, [listed, called], {"before": STATE, "after": STATE, "cleanup": STATE})
        self.assertEqual(out["failures"], ["schema"])
        called["response"]["body"]["result"]["structuredContent"] = {"n": 3}
        self.assertTrue(runner.evaluate(j, [listed, called], {"before": STATE, "after": STATE, "cleanup": STATE})["pass"])

    def test_harness_error_fails(self):
        record = {**health(), "harnessError": "TimeoutExpired"}
        out = runner.evaluate(journey(), [record], {"before": STATE, "after": STATE, "cleanup": STATE})
        self.assertIn("harness-error", out["failures"])

    def test_time_limit(self):
        record = {**health(), "ms": 6 * 60 * 1000}
        out = runner.evaluate(journey(), [record], {"before": STATE, "after": STATE, "cleanup": STATE})
        self.assertEqual(out["failures"], ["time-limit"])

    def test_absent_sentinel(self):
        a = {"step": "h", "path": "response.body.isError", "in": [False, "<absent>"]}
        self.assertTrue(runner.check(a, health())["ok"])
        self.assertFalse(runner.check({**a, "in": [False]}, health())["ok"])


class Format(unittest.TestCase):
    def test_vacuous_requirement_refused(self):
        j = journey(requirements=[{"spec": SPEC, "requirement": "R", "pass": []}])
        self.assertTrue(runner.journey_problems(j))

    def test_no_requirement_refused(self):
        self.assertTrue(runner.journey_problems(journey(requirements=[])))

    def test_over_thirty_minutes_refused(self):
        self.assertTrue(runner.journey_problems(journey(maxMinutes=31)))

    def test_persist_must_be_allowed(self):
        self.assertTrue(runner.journey_problems(journey(effects={"allow": [], "persist": ["files//x"]})))

    def test_committed_journeys_are_well_formed(self):
        for path in sorted((E2E / "journeys").glob("*.json")):
            j = json.loads(path.read_text())
            self.assertEqual(runner.journey_problems(j), [], path.name)
            self.assertEqual(j["id"], path.stem)

    def test_gates_name_committed_journeys(self):
        gates = json.loads((E2E / "gates.json").read_text())
        for gate, spec in gates.items():
            for journey_id in spec["journeys"]:
                self.assertTrue((E2E / "journeys" / f"{journey_id}.json").exists(), (gate, journey_id))
            if not spec["journeys"]:
                self.assertTrue(spec.get("blocked"), f"{gate} with no journeys must be blocked")


class Schema(unittest.TestCase):
    def test_unsupported_keyword_fails_closed(self):
        self.assertTrue(e2elib.validate({}, {"type": "object", "oneOf": []}))

    def test_subset(self):
        schema = {"type": "object", "required": ["a"], "additionalProperties": False, "properties": {
            "a": {"type": "string", "minLength": 2, "pattern": "^x"}, "b": {"type": ["integer", "null"]}}}
        self.assertEqual(e2elib.validate({"a": "xy", "b": None}, schema), [])
        self.assertTrue(e2elib.validate({"a": "x"}, schema))
        self.assertTrue(e2elib.validate({"a": "xy", "c": 1}, schema))
        self.assertTrue(e2elib.validate({"a": "xy", "b": True}, schema))

    def test_envelope(self):
        bad = {"status": 200, "body": {"jsonrpc": "2.0", "id": 9, "result": {}}}
        self.assertTrue(e2elib.validate_response("server/discover", 1, bad, {}))


class Lease(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.saved = (e2elib.T2, e2elib.LEASE)
        e2elib.T2 = Path(self.tmp.name)
        e2elib.LEASE = e2elib.T2 / "dev-vm.lease.json"

    def tearDown(self):
        e2elib.T2, e2elib.LEASE = self.saved
        self.tmp.cleanup()

    def test_require_without_lease_refuses(self):
        with self.assertRaises(SystemExit):
            e2elib.require_lease()

    def test_other_holder_refused_and_expiry(self):
        e2elib.acquire_lease("a", 1)
        self.assertEqual(e2elib.require_lease()["holder"], "a")
        with self.assertRaises(SystemExit):
            e2elib.acquire_lease("b", 1)
        lease = json.loads(e2elib.LEASE.read_text())
        lease["expiresEpoch"] = time.time() - 1
        e2elib.LEASE.write_text(json.dumps(lease))
        with self.assertRaises(SystemExit):
            e2elib.require_lease()

    def test_t2_twins_refuse_while_held(self):
        from parity import t2
        saved = t2.WORK
        t2.WORK = e2elib.T2
        try:
            t2.refuse_during_dev_vm_lease()
            e2elib.acquire_lease("a", 1)
            with self.assertRaises(RuntimeError):
                t2.refuse_during_dev_vm_lease()
        finally:
            t2.WORK = saved


class GuestState(unittest.TestCase):
    def test_nft_chain_order_is_normalized(self):
        import importlib.util
        sys.path.insert(0, str(E2E.parent.parent / "tools/parity/t2guest"))
        spec = importlib.util.spec_from_file_location("e2e_state", E2E / "devvm/guest/state.py")
        state = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(state)
        a = ["table inet incus {", "\tchain b {", "\t\trule b", "\t}", "", "\tchain a {", "\t\trule a", "\t}", "}"]
        b = ["table inet incus {", "\tchain a {", "\t\trule a", "\t}", "", "\tchain b {", "\t\trule b", "\t}", "}"]
        self.assertEqual(state.nft_blocks(a), state.nft_blocks(b))
        c = ["table inet incus {", "\tchain a {", "\t\trule changed", "\t}", "\tchain b {", "\t\trule b", "\t}", "}"]
        self.assertNotEqual(state.nft_blocks(a), state.nft_blocks(c))


if __name__ == "__main__":
    unittest.main()
