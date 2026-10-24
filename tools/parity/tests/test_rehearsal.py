import unittest

from parity import rehearsal, rehearsal_verify

CANCELLED = {"status": "cancelled", "statusMessage": "The Host Agent shut down before the task completed.",
             "isError": None, "code": None}
DRAINED = {"shimAliveAtSignal": True, "aliveWhileInFlight": True, "newConnectionAccepted": False,
           "gate": {"shimAliveAtRelease": True, "released": True}, "exit": 0}


def drill(after):
    return {"taskHandle": True, "gateReached": True, "beforeStop": "working", "stop": DRAINED,
            "afterRestart": after, "stop2": {"exit": 0, "killed": False, "seconds": 0.0}}


class LongTaskTest(unittest.TestCase):
    def test_admitted_task_must_match(self):
        sides = {"go": drill(CANCELLED), "rust": drill(CANCELLED)}
        self.assertEqual(rehearsal._long_task_diff("admitted", sides), [])
        sides["rust"] = drill({**CANCELLED, "status": "failed"})
        self.assertNotEqual(rehearsal._long_task_diff("admitted", sides), [])

    def test_binding_task_is_held_to_its_d12_expectations(self):
        unknown = {"status": {"error": -32603}, "statusMessage": None, "isError": None, "code": None}
        sides = {"go": drill(CANCELLED), "rust": drill(unknown)}
        self.assertEqual(rehearsal._long_task_diff("binding", sides), [])
        # Rust keeping the task, or Go losing it, misses the expectation.
        self.assertNotEqual(rehearsal._long_task_diff("binding", {"go": drill(CANCELLED), "rust": drill(CANCELLED)}),
                            [])
        self.assertNotEqual(rehearsal._long_task_diff("binding", {"go": drill(unknown), "rust": drill(unknown)}), [])
        # Everything outside the declared projection is still compared.
        refused = {**drill(unknown), "stop": {**DRAINED, "aliveWhileInFlight": False}}
        self.assertNotEqual(rehearsal._long_task_diff("binding", {"go": drill(CANCELLED), "rust": refused}), [])


class RolloverTest(unittest.TestCase):
    def test_statuses_compare_status_and_message_only(self):
        def answer(status, message, extra=None):
            return {"status": 200, "body": {"result": {"status": status, "statusMessage": message, **(extra or {})}}}
        before = {"A": answer("cancelled", "x", {"inputRequests": {"r": {}}})}
        after = {"A": answer("cancelled", "x")}
        self.assertEqual(rehearsal._statuses(before), rehearsal._statuses(after))
        self.assertNotEqual(rehearsal._statuses(before), rehearsal._statuses({"A": answer("failed", "x")}))
        missing = {"A": {"status": 200, "body": {"error": {"code": -32603, "message": "task not found"}}}}
        self.assertNotEqual(rehearsal._statuses(before), rehearsal._statuses(missing))

    def test_origin_must_really_be_in_flight(self):
        working = {"status": 200, "body": {"result": {"status": "working"}}}
        pre_stop = {"gateReached": True, "answers": {"E": working}, "stop": DRAINED}
        self.assertTrue(rehearsal._origin_in_flight(pre_stop))
        done = {"status": 200, "body": {"result": {"status": "completed"}}}
        self.assertFalse(rehearsal._origin_in_flight({**pre_stop, "answers": {"E": done}}))
        self.assertFalse(rehearsal._origin_in_flight({**pre_stop, "stop": {**DRAINED, "shimAliveAtSignal": False}}))

    def test_normalized_masks_ids_and_times(self):
        doc = {"taskId": "abc-123", "createdAt": "2026-10-24T05:21:04Z", "lastUpdatedAt": "yesterday"}
        norm, violations = rehearsal._normalized(doc, {"TASK_A": "abc-123"})
        self.assertEqual(norm["taskId"], "${TASK_A}")
        self.assertEqual(norm["createdAt"], "<rfc3339>")
        self.assertEqual(violations, ["/lastUpdatedAt"])


class VerifierTest(unittest.TestCase):
    def test_missing_summary_is_not_a_pass(self):
        problems = []
        got = rehearsal_verify.check(rehearsal.runner.REPO_ROOT, {"rehearsalEvidence": {"summary": "nope.json"}},
                                     {}, problems.append)
        self.assertEqual(got, {})
        self.assertEqual(problems, ["rehearsal: summary missing"])

    def test_items_fail_on_any_drill_difference(self):
        results = {"checks": {"go": [{c: True for c in rehearsal_verify.CHECKS}],
                              "rust": [{c: True for c in rehearsal_verify.CHECKS}]},
                   "comparisons": [[]], "drills": {"drain": [], "twoWriters": [{"path": ["x"]}]}}
        got = rehearsal_verify.items(results)
        self.assertEqual(got["drill:twoWriters"], "fail")
        self.assertEqual(got["drill:drain"], "pass")
        self.assertEqual(got["goControlEqualsRust"], "pass")


if __name__ == "__main__":
    unittest.main()
