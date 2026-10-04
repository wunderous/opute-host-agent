import copy
import unittest

from parity import canon, m5_verify, runner, shape


class FullDatabaseShape(unittest.TestCase):
    def raw(self):
        row = {"operation_id": "task-a", "status": "working", "created_at": "2026-10-04T00:00:00Z",
               "updated_at": "2026-10-04T00:00:00Z", "result_json": '{"answer":1}',
               "task_snapshot_json": '{"taskId":"task-a","status":"input_required","createdAt":"2026-10-04T00:00:00Z"}'}
        observation = {"sqlite": {}, "sqliteRows": {"state/state.db": {"tables": {"operations": {"task-a": row}}}},
                       "steps": {"create": {"body": {"result": {"taskId": "task-a"}}}}}
        raw = {side: {"observation": copy.deepcopy(observation), "variables": {}} for side in ["a", "b"]}
        raw["b"]["observation"] = self.replace_task(raw["b"]["observation"])
        return raw

    def replace_task(self, value):
        if isinstance(value, str):
            return value.replace("task-a", "task-b")
        if isinstance(value, dict):
            return {self.replace_task(k): self.replace_task(v) for k, v in value.items()}
        return value

    def test_known_task_identity_and_typed_times_compare(self):
        raw = self.raw()
        raw["b"]["observation"]["sqliteRows"]["state/state.db"]["tables"]["operations"]["task-b"]["created_at"] = "2026-10-04T01:00:00Z"
        result = shape.evaluate({}, raw)
        self.assertFalse(result["diff"])
        self.assertFalse(any(result["maskViolations"].values()))

    def test_changed_result_is_not_hidden(self):
        raw = self.raw()
        raw["b"]["observation"]["sqliteRows"]["state/state.db"]["tables"]["operations"]["task-b"]["result_json"] = '{"answer":2}'
        self.assertTrue(shape.evaluate({}, raw)["diff"])

    def test_missing_row_is_not_hidden(self):
        raw = self.raw()
        raw["b"]["observation"]["sqliteRows"]["state/state.db"]["tables"]["operations"] = {}
        self.assertTrue(shape.evaluate({}, raw)["diff"])

    def test_invalid_timestamp_is_not_hidden(self):
        raw = self.raw()
        raw["b"]["observation"]["sqliteRows"]["state/state.db"]["tables"]["operations"]["task-b"]["created_at"] = "garbage"
        self.assertTrue(shape.evaluate({}, raw)["maskViolations"]["b"])

    def test_equal_execution_failures_are_not_parity(self):
        raw = self.raw()
        for side in raw.values():
            side["observation"]["_executionError"] = "no space"
        self.assertTrue(all(shape.evaluate({}, raw)["maskViolations"].values()))

    def test_reservation_type_preserves_control_and_rejects_invalid_ids(self):
        self.assertTrue(canon.MASK_TYPES["host-reservation-id"]("host-reservation-1234-2"))
        for invalid in ["control", "host-reservation-1234-0", "random", "host-reservation--1234-2"]:
            self.assertFalse(canon.MASK_TYPES["host-reservation-id"](invalid))

    def test_shape_gate_requires_all_scenarios_and_five_repeats(self):
        self.assertTrue(m5_verify.shape_failures({"repeats": 1}, {}))
        hashes = {s["id"]: s["_sha256"] for s in runner.load_scenarios()}
        self.assertTrue(m5_verify.shape_failures({"repeats": 5, "provenance": {"scenarioHashes": hashes}}, {}))

    def test_claimed_green_shape_cannot_hide_missing_execution(self):
        scenarios = runner.load_scenarios()
        summary = {"repeats": 5, "provenance": {"scenarioHashes": {s["id"]: s["_sha256"] for s in scenarios}}}
        blank = {"raw": {side: {"observation": {"steps": {}, "sqlite": {}, "sqliteRows": {}}, "variables": {}}
                         for side in ["a", "b"]}, "diff": [], "maskViolations": {}}
        outcomes = {f"{s['id']}.{i}": copy.deepcopy(blank) for s in scenarios for i in range(5)}
        self.assertTrue(m5_verify.shape_failures(summary, outcomes))

    def test_d12_cannot_hide_admitted_operation(self):
        scenario = {"compare": {"divergences": ["D12.refused-task-operation-row"]}}
        raw = self.raw()
        raw["b"]["observation"]["sqliteRows"]["state/state.db"]["tables"]["operations"] = {}
        self.assertTrue(m5_verify.refused_row_failures(scenario, raw))
        row = raw["a"]["observation"]["sqliteRows"]["state/state.db"]["tables"]["operations"]["task-a"]
        row["result_json"] = '{"isError":true,"structuredContent":{"owner":"admission","code":"resource_binding"}}'
        self.assertEqual(m5_verify.refused_row_failures(scenario, raw), [])
