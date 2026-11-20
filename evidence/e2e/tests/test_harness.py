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
                        {"step": "c", "path": "response.body.result.structuredContent", "exists": True}]}])
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


def mcp(step, result=None, error=None, method="tools/call"):
    body = {"jsonrpc": "2.0", "id": 1, **({"error": error} if error else {"result": result})}
    return {"step": step, "phase": "step", "kind": "mcp", "method": method, "requestId": 1,
            "response": {"status": 200, "body": body}, **({"tool": "p"} if method == "tools/call" else {})}


def outcome_journey(steps, *assertions, **over):
    return journey(steps=steps, requirements=[{"spec": SPEC, "requirement": "R", "pass": list(assertions)}], **over)


CALL = {"id": "c", "mcp": "tools/call", "tool": "p"}
LISTED = {"step": "l", "phase": "step", "kind": "mcp", "method": "tools/list", "requestId": 1,
          "response": {"status": 200, "body": {"jsonrpc": "2.0", "id": 1, "result": {"tools": [
              {"name": "p", "inputSchema": {"type": "object"}}]}}}}
FAILED = {"content": [{"type": "text", "text": "Error: refused"}], "isError": True}


class Outcomes(unittest.TestCase):
    """m22: a tools/call or tasks/get step whose outcome no assertion decides fails the journey."""

    def judge(self, j, *records):
        j = {**j, "steps": [{"id": "l", "mcp": "tools/list"}] + j["steps"]}
        return runner.evaluate(j, [LISTED, *records], {"before": STATE, "after": STATE, "cleanup": STATE})

    def test_status_only_is_unchecked(self):
        j = outcome_journey([CALL], {"step": "c", "path": "response.status", "equals": 200})
        self.assertEqual(runner.unchecked_outcome_steps(j), ["c"])
        out = self.judge(j, mcp("c", {"content": []}))
        self.assertEqual(out["failures"], ["unchecked-outcome"])
        self.assertEqual(out["uncheckedOutcomes"], ["c"])

    def test_asserted_success_passes(self):
        j = outcome_journey([CALL], {"step": "c", "path": "response.body.result.isError", "in": [False, "<absent>"]})
        self.assertEqual(runner.unchecked_outcome_steps(j), [])
        self.assertTrue(self.judge(j, mcp("c", {"content": []}))["pass"])

    def test_unexpected_error_fails_though_its_content_is_asserted(self):
        j = outcome_journey([CALL], {"step": "c", "path": "response.body.result.content[0].text", "matches": "^Error"})
        self.assertEqual(runner.unchecked_outcome_steps(j), [])
        self.assertEqual(self.judge(j, mcp("c", FAILED))["failures"], ["unchecked-outcome"])

    def test_expected_refusal_passes(self):
        j = outcome_journey([CALL], {"step": "c", "path": "response.body.result.isError", "equals": True},
                            {"step": "c", "path": "response.body.result.content[0].text", "equals": "Error: refused"})
        self.assertTrue(self.judge(j, mcp("c", FAILED))["pass"])

    def test_rpc_error_needs_its_assertion(self):
        poll = {"id": "t", "mcp": "tasks/get", "params": {"taskId": "x"}}
        error = {"code": -32602, "message": "task not found: x"}
        j = outcome_journey([poll], {"step": "t", "path": "response.body.result.result.content", "exists": False})
        self.assertEqual(self.judge(j, mcp("t", error=error, method="tasks/get"))["failures"], ["unchecked-outcome"])
        j = outcome_journey([poll], {"step": "t", "path": "response.body.error.message", "matches": "^task not found"})
        self.assertTrue(self.judge(j, mcp("t", error=error, method="tasks/get"))["pass"])

    def test_completed_task_needs_its_result(self):
        poll = {"id": "t", "mcp": "tasks/get", "params": {"taskId": "x"}}
        done = {"status": "completed", "result": FAILED}
        status = {"step": "t", "path": "response.body.result.status", "equals": "completed"}
        self.assertEqual(runner.unchecked_outcome_steps(outcome_journey([poll], status)), ["t"])
        result = {"step": "t", "path": "response.body.result.result.structuredContent", "exists": False}
        j = outcome_journey([poll], status, result)
        self.assertEqual(runner.unchecked_outcome_steps(j), [])
        self.assertEqual(self.judge(j, mcp("t", done, method="tasks/get"))["failures"], ["unchecked-outcome"])
        j = outcome_journey([poll], status, {"step": "t", "path": "response.body.result.result.isError",
                                             "equals": False})
        self.assertEqual(self.judge(j, mcp("t", done, method="tasks/get"))["failures"],
                         ["pass-condition", "unchecked-outcome"])

    def test_launch_is_judged_at_its_poll(self):
        launch = {**CALL, "capture": {"TID": "response.body.result.taskId"}}
        poll = {"id": "t", "mcp": "tasks/get", "params": {"taskId": "${TID}"}}
        j = outcome_journey([launch, poll], {"step": "t", "path": "response.body.result.result.isError",
                                             "equals": False})
        self.assertEqual(runner.unchecked_outcome_steps(j), [])
        ok = {"status": "completed", "result": {"content": [], "isError": False}}
        self.assertTrue(self.judge(j, mcp("c", {"taskId": "1", "status": "working"}),
                                   mcp("t", ok, method="tasks/get"))["pass"])
        # a launch refused outright is still a failure no assertion expects
        self.assertEqual(self.judge(j, mcp("c", FAILED), mcp("t", ok, method="tasks/get"))["failures"],
                         ["unchecked-outcome"])

    def test_opt_out_needs_its_reason(self):
        j = outcome_journey([{**CALL, "expectAnyOutcome": "the bus may not be up yet"},
                             {"id": "h", "health": True}], {"step": "h", "path": "response.status", "equals": 200})
        self.assertEqual(runner.unchecked_outcome_steps(j), [])
        self.assertTrue(self.judge(j, mcp("c", FAILED), health())["pass"])
        self.assertTrue(runner.journey_problems(
            outcome_journey([{**CALL, "expectAnyOutcome": " "}, {"id": "h", "health": True}],
                            {"step": "h", "path": "response.status", "equals": 200})))

    def test_committed_journeys_check_every_outcome(self):
        for path in sorted((E2E / "journeys").glob("*.json")):
            j = json.loads(path.read_text())
            self.assertEqual(runner.unchecked_outcome_steps(j), [], path.name)


def listed_with_effects(**effects):
    tools = [{"name": n, "inputSchema": {"type": "object"}, "_meta": {"capability": {"effect": e}}}
             for n, e in effects.items()]
    return {"step": "l", "phase": "step", "kind": "mcp", "method": "tools/list", "requestId": 1,
            "response": {"status": 200, "body": {"jsonrpc": "2.0", "id": 1, "result": {"tools": tools}}}}


def guest(step, stdout="", exit_=0):
    return {"step": step, "phase": "step", "kind": "guest", "exit": exit_, "stdout": stdout, "ms": 5}


class Effects(unittest.TestCase):
    """m23: a mutation the journey expects to succeed needs a later independent observation."""

    WRITE = {"id": "w", "mcp": "tools/call", "tool": "write"}
    OK = {"step": "w", "path": "response.body.result.isError", "in": [False, "<absent>"]}
    EFFECTS = {"write": "mutation", "read": "read"}

    def judge(self, steps, *assertions, records=()):
        j = outcome_journey([{"id": "l", "mcp": "tools/list"}] + steps, *assertions)
        lst = listed_with_effects(**self.EFFECTS)
        return j, runner.evaluate(j, [lst, *records], {"before": STATE, "after": STATE, "cleanup": STATE})

    def call(self, step, tool):
        return {**mcp(step, {"content": []}), "tool": tool}

    def test_unconfirmed_mutation_fails(self):
        j, out = self.judge([self.WRITE], self.OK, records=[self.call("w", "write")])
        self.assertEqual(runner.unconfirmed_effect_steps(j, self.EFFECTS), ["w"])
        self.assertEqual(out["failures"], ["unconfirmed-effect"])
        self.assertEqual(out["unconfirmedEffects"], ["w"])

    def test_guest_confirmation_passes(self):
        look = {"id": "g", "guest": "cat /f", "confirms": ["w"]}
        _, out = self.judge([self.WRITE, look], self.OK, {"step": "g", "path": "stdout", "equals": "x\n"},
                            records=[self.call("w", "write"), guest("g", "x\n")])
        self.assertTrue(out["pass"], out)

    def test_confirmation_that_finds_nothing_fails(self):
        look = {"id": "g", "guest": "cat /f", "confirms": ["w"]}
        _, out = self.judge([self.WRITE, look], self.OK, {"step": "g", "path": "stdout", "equals": "x\n"},
                            records=[self.call("w", "write"), guest("g", "")])
        self.assertEqual(out["failures"], ["pass-condition"])

    def test_read_tool_confirms_and_mutating_tool_does_not(self):
        read = {"id": "r", "mcp": "tools/call", "tool": "read", "confirms": ["w"]}
        again = {"id": "r", "mcp": "tools/call", "tool": "write", "confirms": ["w"]}
        ra = {"step": "r", "path": "response.body.result.structuredContent", "exists": False}
        j, out = self.judge([self.WRITE, read], self.OK, ra, records=[self.call("w", "write"), self.call("r", "read")])
        self.assertEqual(runner.unconfirmed_effect_steps(j, self.EFFECTS), [])
        j = outcome_journey([self.WRITE, again], self.OK, ra)
        self.assertEqual(runner.unconfirmed_effect_steps(j, self.EFFECTS), ["w", "r"])

    def test_confirmation_needs_its_own_assertion_and_must_come_later(self):
        look = {"id": "g", "guest": "cat /f", "confirms": ["w"]}
        self.assertEqual(runner.unconfirmed_effect_steps(outcome_journey([self.WRITE, look], self.OK), self.EFFECTS),
                         ["w"])
        early = outcome_journey([{"id": "g", "guest": "true", "confirms": ["w"]}, self.WRITE], self.OK,
                                {"step": "g", "path": "exit", "equals": 0})
        self.assertTrue(runner.journey_problems(early))

    def test_expected_failure_and_opt_out_need_no_confirmation(self):
        refused = {"step": "w", "path": "response.body.result.isError", "equals": True}
        self.assertEqual(runner.unconfirmed_effect_steps(outcome_journey([self.WRITE], refused), self.EFFECTS), [])
        out = outcome_journey([{**self.WRITE, "effectUnobservable": "its only effect is its output"}], self.OK)
        self.assertEqual(runner.unconfirmed_effect_steps(out, self.EFFECTS), [])
        self.assertTrue(runner.journey_problems(outcome_journey([{**self.WRITE, "effectUnobservable": ""}], self.OK)))

    def test_task_effect_is_its_polls(self):
        launch = {**self.WRITE, "capture": {"TID": "response.body.result.taskId"}}
        poll = {"id": "t", "mcp": "tasks/get", "params": {"taskId": "${TID}"}}
        reread = {"id": "t2", "mcp": "tasks/get", "params": {"taskId": "${TID}"}}
        res = {"step": "t", "path": "response.body.result.result.isError", "equals": False}
        j = outcome_journey([launch, poll, reread], res, {"step": "t2", "path": "response.body.result.status",
                                                          "equals": "completed"})
        self.assertEqual(runner.unconfirmed_effect_steps(j, self.EFFECTS), ["t"])
        cancelled = outcome_journey([launch, poll], {"step": "t", "path": "response.body.result.status",
                                                     "equals": "cancelled"})
        self.assertEqual(runner.unconfirmed_effect_steps(cancelled, self.EFFECTS), [])

    def test_committed_journeys_confirm_every_effect(self):
        import outcomes
        effects = outcomes.committed_effects()
        for path in sorted((E2E / "journeys").glob("*.json")):
            if path.stem in outcomes.negative_controls("unconfirmed-effect"):
                continue
            j = json.loads(path.read_text())
            self.assertEqual(runner.unconfirmed_effect_steps(j, effects), [], path.name)


class Allowances(unittest.TestCase):
    """m24: every allowance matches what the run changed, and none is broader than its effect."""

    AFTER = {"files": {"/etc/a": {"sha256": "1"}, "/etc/b": {"sha256": "2"}}}

    def judge(self, effects, cleanup=None):
        states = {"before": STATE, "after": self.AFTER, "cleanup": cleanup or STATE}
        return runner.evaluate(journey(effects=effects), [health()], states)

    def test_used_allowance_passes(self):
        self.assertTrue(self.judge({"allow": ["files//etc/b"], "persist": []})["pass"])

    def test_unused_allow_fails(self):
        out = self.judge({"allow": ["files//etc/b", "files//etc/never"], "persist": []})
        self.assertEqual(out["failures"], ["unused-allowance"])
        self.assertEqual(out["allowances"]["unusedAllow"], ["files//etc/never"])

    def test_unused_persist_fails(self):
        out = self.judge({"allow": ["files//etc/b"], "persist": ["files//etc/b"]})
        self.assertEqual(out["failures"], ["unused-allowance"])
        self.assertEqual(out["allowances"]["unusedPersist"], ["files//etc/b"])
        self.assertTrue(self.judge({"allow": ["files//etc/b"], "persist": ["files//etc/b"]}, self.AFTER)["pass"])

    def test_optional_allowance_needs_no_match(self):
        effects = {"allow": ["files//etc/b", "files//etc/c"], "persist": [],
                   "optional": {"files//etc/c": "written only when the mirror is stale"}}
        self.assertTrue(self.judge(effects)["pass"])

    def test_optional_needs_a_reason_and_an_allow_pattern(self):
        empty = journey(effects={"allow": ["files//etc/c"], "persist": [], "optional": {"files//etc/c": " "}})
        self.assertIn("effects.optional 'files//etc/c' needs its reason", runner.journey_problems(empty))
        stray = journey(effects={"allow": [], "persist": [], "optional": {"files//etc/c": "why"}})
        self.assertIn("effects.optional 'files//etc/c' is not an allow pattern", runner.journey_problems(stray))

    def test_broad_allowance_fails_even_when_used(self):
        out = self.judge({"allow": ["files//etc*"], "persist": []})
        self.assertEqual(out["failures"], ["broad-allowance"])
        self.assertEqual(out["allowances"]["broad"], ["files//etc*"])

    def test_what_is_broad(self):
        for pattern, broad in (("incus/*", True), ("incus/volumes/*", True), ("system/units/*", True),
                               ("user/*", True), ("linger*", True), ("files//etc*", True), ("files//*", True),
                               ("linger", False), ("system/units/e2e-*", False), ("incus/instances/e2e-k*", False),
                               ("files//etc/systemd/system/*", True), ("incus/volumes/default/image/*", True),
                               ("files//var/log*", False), ("files//etc/opute-e2e-probe", False),
                               ("system/units/tmp-e2e*agents-*.mount", False)):
            self.assertEqual(runner.broad_pattern(pattern), broad, pattern)

    def test_committed_journeys_name_their_effects(self):
        import outcomes
        controls = outcomes.negative_controls("broad-allowance")
        for path in sorted((E2E / "journeys").glob("*.json")):
            if path.stem in controls:
                continue
            j = json.loads(path.read_text())
            optional = j["effects"].get("optional") or {}
            broad = [p for p in j["effects"]["allow"] if p not in optional and runner.broad_pattern(p)]
            self.assertEqual(broad, [], path.name)


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

    def test_e2e1_journeys_are_the_generator_output(self):
        sys.path.insert(0, str(E2E / "journeys"))
        import make_e2e1
        if not Path(make_e2e1.CANDIDATE.replace("${PARITY}", str(make_e2e1.PARITY))).exists():
            self.skipTest("the journeys' pinned inputs live under .parity/ (absent on CI)")
        for name, text in make_e2e1.render().items():
            self.assertEqual((E2E / "journeys" / name).read_text(), text, f"{name}: rerun make_e2e1.py")

    def test_e2e3a_journeys_are_the_generator_output(self):
        sys.path.insert(0, str(E2E / "journeys"))
        import make_e2e1
        if not Path(make_e2e1.CANDIDATE.replace("${PARITY}", str(make_e2e1.PARITY))).exists():
            self.skipTest("the journeys' pinned inputs live under .parity/ (absent on CI)")
        import make_e2e3a  # hashes its inputs on import
        for name, text in make_e2e3a.render().items():
            self.assertEqual((E2E / "journeys" / name).read_text(), text, f"{name}: rerun make_e2e3a.py")

    def test_e2e4a_journeys_are_the_generator_output(self):
        sys.path.insert(0, str(E2E / "journeys"))
        import make_e2e1
        if not Path(make_e2e1.CANDIDATE_NEXT.replace("${PARITY}", str(make_e2e1.PARITY))).exists():
            self.skipTest("the journeys' pinned inputs live under .parity/ (absent on CI)")
        import make_e2e4a  # hashes its inputs on import
        for name, text in make_e2e4a.render().items():
            self.assertEqual((E2E / "journeys" / name).read_text(), text, f"{name}: rerun make_e2e4a.py")

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


class Extensions(unittest.TestCase):
    def ctx(self):
        return {"token": "e2e-test-token-" + "a" * 48, "vars": {"RUN": "r-1"}, "secrets": {}}

    def test_expand_and_scrub(self):
        ctx = self.ctx()
        out = runner.expand({"cmd": "x ${TOKEN} ${RUN} ${SECRET:s}"}, ctx)
        secret = ctx["secrets"]["s"]
        self.assertEqual(out["cmd"], f"x {ctx['token']} r-1 {secret}")
        self.assertEqual(runner.scrub(out, ctx)["cmd"], "x <test-token> r-1 <secret:s>")
        self.assertEqual(runner.expand("${SECRET:s}", ctx), secret, "a secret is stable within a run")
        with self.assertRaises(KeyError):
            runner.expand("${NEVER}", ctx)

    def test_secret_capture_is_scrubbed(self):
        ctx = self.ctx()
        runner.capture({"capture": {"T": {"path": "json.access_token", "secret": True}}},
                       {"json": {"access_token": "issued-abc"}}, ctx)
        self.assertEqual(ctx["vars"]["T"], "issued-abc")
        self.assertEqual(runner.scrub({"x": "Bearer issued-abc"}, ctx), {"x": "Bearer <secret:T>"})
        with self.assertRaises(KeyError):
            runner.capture({"capture": {"U": "json.missing"}}, {"json": {}}, ctx)

    def test_open_question_is_unverified_not_pass_or_fail(self):
        j = journey(requirements=[
            {"spec": SPEC, "requirement": "R", "pass": [{"step": "h", "path": "response.status", "equals": 200}]},
            {"spec": SPEC, "requirement": "Q", "openQuestion": "Q5",
             "pass": [{"step": "h", "path": "response.status", "equals": 500}]}])
        out = runner.evaluate(j, [health()], {"before": STATE, "after": STATE, "cleanup": STATE})
        self.assertTrue(out["pass"])
        self.assertEqual(out["unverified"], ["Q5"])
        self.assertEqual(out["requirements"][1]["outcome"], "unverified")
        self.assertFalse(out["requirements"][1]["observedAsAsserted"])
        self.assertTrue(runner.journey_problems(journey(requirements=[
            {"spec": SPEC, "requirement": "Q", "openQuestion": "five", "pass": [
                {"step": "h", "path": "response.status", "equals": 200}]}])))

    def test_equals_step(self):
        a = {"step": "a", "phase": "step", "json": {"n": 3}}
        b = {"step": "b", "phase": "step", "json": {"n": 3}}
        records = {"a": a, "b": b}
        ok = runner.check({"step": "a", "path": "json.n", "equalsStep": {"step": "b", "path": "json.n"}}, a, records)
        self.assertTrue(ok["ok"])
        b["json"]["n"] = 4
        self.assertFalse(runner.check({"step": "a", "path": "json.n", "equalsStep": {"step": "b", "path": "json.n"}},
                                      a, records)["ok"])

    def test_excludes(self):
        rec = {"step": "a", "phase": "step", "response": {"body": {"result": {"text": "a <secret:x> b"}}}}
        self.assertFalse(runner.check({"step": "a", "path": "response.body", "excludes": "<secret:"}, rec)["ok"])
        rec["response"]["body"]["result"]["text"] = "clean"
        self.assertTrue(runner.check({"step": "a", "path": "response.body", "excludes": "<secret:"}, rec)["ok"])
        # an absent path never passes as excluding
        self.assertFalse(runner.check({"step": "a", "path": "response.nothing", "excludes": "<secret:"}, rec)["ok"])

    def test_client_reported_results_are_schema_checked(self):
        tool = {"name": "p", "inputSchema": {"type": "object"},
                "outputSchema": {"type": "object", "required": ["n"]}}
        listed = {"step": "l", "phase": "step", "kind": "mcp", "target": "agent", "method": "tools/list",
                  "requestId": 1, "response": {"status": 200, "body": {"jsonrpc": "2.0", "id": 1,
                                                                       "result": {"tools": [tool]}}}}
        client = {"step": "c", "phase": "step", "kind": "guest", "exit": 0, "schemaResults": "json.results",
                  "json": {"results": [{"tool": "p", "structuredContent": {}}]}}
        j = journey(steps=[{"id": "l", "mcp": "tools/list"}, {"id": "c", "guest": "client"}],
                    requirements=[{"spec": SPEC, "requirement": "R", "pass": [
                        {"step": "c", "path": "exit", "equals": 0}]}])
        states = {"before": STATE, "after": STATE, "cleanup": STATE}
        self.assertEqual(runner.evaluate(j, [listed, client], states)["failures"], ["schema"])
        client["json"]["results"][0]["structuredContent"] = {"n": 1}
        self.assertTrue(runner.evaluate(j, [listed, client], states)["pass"])
        client["json"]["results"] = []
        self.assertEqual(runner.evaluate(j, [listed, client], states)["failures"], ["schema"])

    def test_copy_needs_a_pinned_input(self):
        j = journey(steps=[{"id": "h", "health": True}, {"id": "c", "copy": "bin", "to": "/x"}])
        self.assertTrue(runner.journey_problems(j))
        j["inputs"] = {"files": {"bin": {"path": "${PARITY}/x", "sha256": "ab"}}}
        self.assertEqual(runner.journey_problems(j), [])


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
