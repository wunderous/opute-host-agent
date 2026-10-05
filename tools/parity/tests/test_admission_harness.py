"""M4 harness mechanics: counted gates, shim exit evidence and X2 exemptions."""

import json
import signal
import subprocess
import tempfile
import time
import unittest
from unittest.mock import patch
from types import SimpleNamespace
from pathlib import Path

from parity import shims
from parity.runner import invariant_violations
from parity import runner


class ShimSandbox:
    """A shim directory whose only rule blocks on gate "list"."""

    def __init__(self, test: unittest.TestCase):
        tmp = tempfile.TemporaryDirectory()
        test.addCleanup(tmp.cleanup)
        self.root = Path(tmp.name)
        fixture = self.root / "fixture.json"
        fixture.write_text(json.dumps({"commands": {"incus": [
            {"argvPrefix": ["list"], "gate": "list", "stdout": "[]"}]}}))
        self.trace = self.root / "trace.jsonl"
        shims.install(self.root / "bin", fixture, self.trace)
        self.procs = []
        test.addCleanup(self._reap)

    def start(self, *argv):
        proc = subprocess.Popen([str(self.root / "bin" / "incus"), *argv],
                                stdin=subprocess.DEVNULL, stdout=subprocess.PIPE)
        self.procs.append(proc)
        return proc

    def _reap(self):
        for proc in self.procs:
            if proc.poll() is None:
                proc.kill()
            proc.wait()
            proc.stdout.close()


class CountedGateTest(unittest.TestCase):
    def test_one_release_wakes_every_counted_waiter(self):
        box = ShimSandbox(self)
        first, second = box.start("list"), box.start("list", "--all")
        self.assertTrue(shims.await_gate(box.root, "list", count=2))
        self.assertEqual(shims.waiting_count(box.root, "list"), 2)
        self.assertEqual(shims.release_gate(box.root, "list", count=2),
                         {"gate": "list", "released": True})
        self.assertEqual([first.wait(10), second.wait(10)], [0, 0])
        self.assertEqual(first.stdout.read(), b"[]\n")
        # Once released the gate stays open: later invocations pass straight through.
        late = box.start("list")
        self.assertEqual(late.wait(10), 0)
        self.assertEqual(shims.live_shims(box.root), [])

    def test_release_reports_too_few_waiters(self):
        box = ShimSandbox(self)
        box.start("list")
        self.assertFalse(shims.await_gate(box.root, "list", timeout=0.5, count=2))
        self.assertEqual(shims.release_gate(box.root, "list", timeout=0.2, count=2)["released"], False)


class ShimExitTest(unittest.TestCase):
    def _blocked(self, box):
        proc = box.start("list")
        self.assertTrue(shims.await_gate(box.root, "list"))
        return proc

    def test_catchable_signal_is_traced_and_cleans_up(self):
        box = ShimSandbox(self)
        proc = self._blocked(box)
        proc.send_signal(signal.SIGTERM)
        self.assertEqual(proc.wait(10), 128 + signal.SIGTERM)
        signalled = [e for e in shims.read_trace(box.trace) if "signal" in e]
        self.assertEqual(signalled, [{"cmd": "incus", "argv": ["list"], "signal": "SIGTERM"}])
        self.assertEqual(shims.unclean_shims(box.root), [])
        self.assertEqual(shims.live_shims(box.root), [])

    def test_sigkill_leaves_an_unclean_pid_file(self):
        box = ShimSandbox(self)
        proc = self._blocked(box)
        self.assertEqual(len(shims.live_shims(box.root)), 1)
        self.assertEqual(shims.unclean_shims(box.root), [])  # running is not unclean
        proc.kill()
        proc.wait(10)
        deadline = time.monotonic() + 5
        while shims.live_shims(box.root) and time.monotonic() < deadline:
            time.sleep(0.01)
        self.assertEqual(shims.unclean_shims(box.root), [{"cmd": "incus list"}])
        self.assertFalse(any("signal" in e for e in shims.read_trace(box.trace)))


class X2InvariantTest(unittest.TestCase):
    SCENARIO = {
        "invariants": ["X2"],
        "x2Reads": [{"cmd": "incus", "argvPrefix": ["query"], "argPrefix": "/1.0/"},
                    {"cmd": "incus", "argvPrefix": ["list"]}],
        "x2GoGaps": {"operations": "D12"},
    }

    def violations(self, trace=(), rows=None, label="rust"):
        obs = {"trace": list(trace), "sqlite": {"state.db": {"rowCounts": rows or {}}}}
        return invariant_violations(self.SCENARIO, obs, label)

    def test_declared_reads_are_not_effects(self):
        self.assertEqual(self.violations([
            {"cmd": "incus", "argv": ["list", "--format", "json"]},
            {"cmd": "incus", "argv": ["query", "/1.0/instances"]},
        ]), [])

    def test_arg_prefix_rejects_a_write_through_a_read_verb(self):
        found = self.violations([{"cmd": "incus", "argv": ["query", "-X", "PUT", "/1.0/x"]},
                                 {"cmd": "incus", "argv": ["query"]},
                                 {"cmd": "incus", "argv": ["start", "vm"]}])
        self.assertEqual(len(found), 1)
        self.assertEqual(len(found[0]["value"]), 3)

    def test_go_gaps_exempt_only_the_go_side_and_only_their_table(self):
        rows = {"operations": 1, "capability_invocations": 0}
        self.assertEqual(self.violations(rows=rows, label="go-left"), [])
        self.assertEqual([v["reason"] for v in self.violations(rows=rows, label="rust")],
                         ["state.db operations has 1 rows"])
        self.assertEqual(len(self.violations(rows={"plan_runs": 2}, label="go-left")), 1)


class BoundaryExpectationTest(unittest.TestCase):
    def test_guarded_followup_permits_synchronous_refusal_without_task_handle(self):
        response = {"body": {"error": {"code": -32603}}}
        sandbox = SimpleNamespace(captured={})
        with patch.object(runner.agent, "mcp_call", return_value=response):
            self.assertEqual(runner._mcp_step(sandbox, {
                "method": "tools/call", "capture": {"TASK_ID": ["body", "result", "taskId"]}},
                {"TASK_ID"}), response)
        self.assertEqual(sandbox.captured, {})

    def test_equal_nonterminal_responses_cannot_pass_a_terminal_poll(self):
        response = {"body": {"result": {"status": "working"}}}
        spec = {"method": "tasks/get", "timeout": 0,
                "until": {"path": ["body", "result", "status"], "in": ["completed"]}}
        with patch.object(runner.agent, "mcp_call", return_value=response):
            with self.assertRaisesRegex(RuntimeError, "polling timed out"):
                runner._mcp_step(SimpleNamespace(captured={}), spec)

    def test_missing_task_handle_cannot_turn_followup_calls_into_equal_errors(self):
        with patch.object(runner.agent, "mcp_call", return_value={"body": {"error": {"code": -32603}}}):
            with self.assertRaisesRegex(RuntimeError, "required capture TASK_ID"):
                runner._mcp_step(SimpleNamespace(captured={}), {
                    "method": "tools/call", "capture": {"TASK_ID": ["body", "result", "taskId"]}})

    def test_equal_early_refusals_do_not_prove_saturation(self):
        scenario = {"expectations": [{"path": ["steps", "held", "reached"], "equals": True}]}
        self.assertTrue(invariant_violations(scenario, {"steps": {"held": {"reached": False}}}))
        self.assertTrue(invariant_violations(scenario, {}))
        self.assertTrue(invariant_violations(scenario, {"steps": {"held": {"reached": 1}}}))
        self.assertEqual(invariant_violations(scenario, {"steps": {"held": {"reached": True}}}), [])

    def test_late_snapshot_overwrite_fails_even_when_live_poll_was_cancelled(self):
        scenario = {"expectations": [
            {"path": ["taskSnapshotStatuses"], "equals": ["cancelled"], "implementations": ["go"]},
            {"path": ["taskSnapshotStatuses"], "equals": [], "implementations": ["rust"]},
        ]}
        self.assertTrue(invariant_violations(scenario, {"taskSnapshotStatuses": ["completed"]}, "canary"))
        self.assertTrue(invariant_violations(scenario, {"taskSnapshotStatuses": ["failed"]}, "go"))
        self.assertEqual(invariant_violations(scenario, {"taskSnapshotStatuses": ["cancelled"]}, "go"), [])
        self.assertEqual(invariant_violations(scenario, {"taskSnapshotStatuses": []}, "rust"), [])
        self.assertTrue(invariant_violations(scenario, {"taskSnapshotStatuses": []}, "go"))
        self.assertTrue(invariant_violations(scenario, {"taskSnapshotStatuses": ["cancelled"]}, "rust"))


if __name__ == "__main__":
    unittest.main()
