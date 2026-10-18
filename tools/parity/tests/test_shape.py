import copy
import json
import unittest
from unittest.mock import patch

from parity import canon, m5_verify, runner, shape


class FullDatabaseShape(unittest.TestCase):
    def test_exemptions_keep_hash_coverage_without_requiring_uncomparable_rows(self):
        scenarios = [{"id": "bounded", "_sha256": "a", "steps": [{"as": "ran"}]},
                     {"id": "timed", "_sha256": "b", "steps": [],
                      "shapeExempt": "retry count depends on elapsed time"}]
        summary = {"repeats": 5, "provenance": {"scenarioHashes": {"bounded": "a", "timed": "b"}}}
        result = {"raw": {side: {"observation": {"steps": {"ran": {}}, "sqlite": {}, "sqliteRows": {}},
                                  "variables": {}} for side in ["a", "b"]},
                  "diff": [], "maskViolations": {}}
        outcomes = {f"bounded.{i}": copy.deepcopy(result) for i in range(5)}
        with patch.object(runner, "load_scenarios", return_value=scenarios):
            self.assertEqual(m5_verify.shape_failures(summary, outcomes), [])
            incomplete = copy.deepcopy(summary)
            del incomplete["provenance"]["scenarioHashes"]["timed"]
            self.assertTrue(m5_verify.shape_failures(incomplete, outcomes))
            del outcomes["bounded.0"]
            self.assertTrue(m5_verify.shape_failures(summary, outcomes))

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


class ProviderDatabaseShape(unittest.TestCase):
    """The M7 provider tables and callbacks in the shape matrix."""

    TABLES = ["sqliteRows", "state/state.db", "tables"]

    def raw(self, tables, steps=None):
        observation = {"sqlite": {"state/state.db": {"rowCounts": {t: len(r) for t, r in tables.items()}}},
                       "sqliteRows": {"state/state.db": {"tables": tables}}, "steps": steps or {}}
        return {side: {"observation": copy.deepcopy(observation), "variables": {}} for side in ["a", "b"]}

    def rows(self, raw, side):
        return raw[side]["observation"]["sqliteRows"]["state/state.db"]["tables"]

    def test_wall_clock_accepts_only_rfc3339_or_sqlite_datetime(self):
        check = canon.MASK_TYPES["wall-clock"]
        self.assertTrue(check("2026-10-11T19:45:10.1Z") and check("2026-10-11 19:45:10"))
        for invalid in ["2026-10-11", "19:45:10", "garbage", None, 0]:
            self.assertFalse(check(invalid))

    def test_generation_activation_time_is_masked_only_when_set(self):
        generation = {"generation_id": "g-1", "created_at": "2026-10-11T00:00:00Z", "active_at": None}
        raw = self.raw({"provider_generations": {"g-1": generation}})
        result = shape.evaluate({}, raw)
        self.assertFalse(result["diff"] or any(result["maskViolations"].values()))
        self.rows(raw, "a")["provider_generations"]["g-1"]["active_at"] = "2026-10-11T00:00:01Z"
        self.rows(raw, "b")["provider_generations"]["g-1"]["active_at"] = "2026-10-11T00:00:02Z"
        result = shape.evaluate({}, raw)
        self.assertFalse(result["diff"] or any(result["maskViolations"].values()))
        # A generation activated on one side only is a difference.
        self.rows(raw, "b")["provider_generations"]["g-1"]["active_at"] = None
        self.assertTrue(shape.evaluate({}, raw)["diff"])

    def test_capability_activation_time_is_masked(self):
        raw = self.raw({"active_capabilities": {"llm": {"capability": "llm", "activated_at": "2026-10-11T00:00:01Z"}}})
        self.rows(raw, "b")["active_capabilities"]["llm"]["activated_at"] = "2026-10-11T00:00:09Z"
        self.assertFalse(shape.evaluate({}, raw)["diff"])
        self.rows(raw, "b")["active_capabilities"]["llm"]["activated_at"] = "soon"
        self.assertTrue(shape.evaluate({}, raw)["maskViolations"]["b"])

    def invocation(self, operation, structured):
        return {"invocation_id": "00000000-0000-4000-8000-00000000000" + str(len(operation) % 10),
                "operation_id": operation, "created_at": "2026-10-11T00:00:00Z", "binding_json": "{}",
                "result_json": json.dumps({"structured": structured})}

    def test_supported_tools_compare_as_a_set(self):
        row = self.invocation("get_host_info", {"supportedTools": ["a", "b", "c"]})
        raw = self.raw({"capability_invocations": {row["invocation_id"]: row}})
        reordered = self.rows(raw, "b")["capability_invocations"][row["invocation_id"]]
        reordered["result_json"] = json.dumps({"structured": {"supportedTools": ["c", "a", "b"]}})
        self.assertFalse(shape.evaluate({}, raw)["diff"])
        reordered["result_json"] = json.dumps({"structured": {"supportedTools": ["c", "a"]}})
        self.assertTrue(shape.evaluate({}, raw)["diff"])

    def test_only_tools_called_solely_by_until_polls_lose_their_audit_rows(self):
        poll = {"mcp": {"method": "tools/call", "name": "get_host_capacity", "until": {}}}
        call = {"mcp": {"method": "tools/call", "name": "get_host_capacity"}}
        self.assertEqual(shape.polled_tools({"steps": [poll]}), {"get_host_capacity"})
        self.assertEqual(shape.polled_tools({"steps": [poll, call]}), set())
        first = self.invocation("get_host_capacity", {"reservations": {"count": 1}})
        raw = self.raw({"capability_invocations": {first["invocation_id"]: first}})
        self.rows(raw, "b")["capability_invocations"] = {}
        self.assertFalse(shape.evaluate({"steps": [poll]}, raw)["diff"])
        self.assertTrue(shape.evaluate({"steps": [call]}, raw)["diff"])
        # Other rows of a polling scenario still compare.
        other = self.invocation("get_host_info", {"hostname": "a"})
        self.rows(raw, "a")["capability_invocations"][other["invocation_id"]] = other
        self.assertTrue(shape.evaluate({"steps": [poll]}, raw)["diff"])

    def callback(self, child):
        return {"bridged": {"body": {"result": {"result": {"structuredContent": {
            "callback": {"body": {"result": {"taskId": child}}}}}}}}}

    def test_d12_drops_only_a_refused_callback_child(self):
        child = "11111111-1111-4111-8111-111111111111"
        refused = {"operation_id": child, "status": "working", "created_at": "2026-10-11T00:00:00Z",
                   "updated_at": "2026-10-11T00:00:00Z",
                   "result_json": json.dumps({"isError": True, "structuredContent": {
                       "owner": "admission", "code": "resource_binding"}})}
        raw = self.raw({"operations": {child: refused}}, self.callback(child))
        self.rows(raw, "b")["operations"] = {}
        raw["b"]["observation"]["sqlite"]["state/state.db"]["rowCounts"]["operations"] = 0
        scenario = {"compare": {"divergences": ["D12.refused-task-operation-count"]}}
        self.assertFalse(shape.evaluate(scenario, raw)["diff"])
        # The same row without the typed admission refusal stays compared.
        self.rows(raw, "a")["operations"][child]["result_json"] = json.dumps({"isError": False})
        self.assertTrue(shape.evaluate(scenario, raw)["diff"])
        # A refusal whose handle no callback reported stays compared.
        self.rows(raw, "a")["operations"][child] = refused
        raw["a"]["observation"]["steps"] = {}
        self.assertTrue(shape.evaluate(scenario, raw)["diff"])

    def test_gate_accepts_only_a_typed_refusal_of_a_reported_callback_child(self):
        child = "22222222-2222-4222-8222-222222222222"
        row = {"operation_id": child, "result_json": json.dumps({"isError": True, "structuredContent": {
            "owner": "admission", "code": "resource_binding"}})}
        scenario = {"compare": {"divergences": ["D12.refused-callback-child-operation"]}}
        raw = self.raw({"operations": {child: row}}, self.callback(child))
        self.rows(raw, "b")["operations"] = {}
        self.assertEqual(m5_verify.refused_row_failures(scenario, raw), [])
        raw["a"]["observation"]["steps"] = {}
        self.assertTrue(m5_verify.refused_row_failures(scenario, raw))
        raw = self.raw({"operations": {child: dict(row, result_json=json.dumps({"isError": False}))}}, self.callback(child))
        self.rows(raw, "b")["operations"] = {}
        self.assertTrue(m5_verify.refused_row_failures(scenario, raw))


class DatabaseCompanions(unittest.TestCase):
    """A wire divergence's database companions (D16, D18) apply only where
    the scenario declares that wire divergence, and hide nothing else."""

    def raw(self, child=True):
        go = {"operations": {"${CHILD}": {"operation_id": "${CHILD}", "status": "working"},
                             "kept": {"operation_id": "kept", "status": "completed"}}}
        rust = {"operations": {"kept": {"operation_id": "kept", "status": "completed"}}}
        if not child:
            go["operations"].pop("${CHILD}")
        observation = lambda tables: {
            "sqlite": {"state/state.db": {"rowCounts": {t: len(r) for t, r in tables.items()}}},
            "sqliteRows": {"state/state.db": {"tables": tables}}, "steps": {}}
        return {"a": {"observation": observation(go), "variables": {}},
                "b": {"observation": observation(rust), "variables": {}}}

    def test_d18_child_row_needs_the_declared_wire_divergence(self):
        self.assertTrue(shape.evaluate({}, self.raw())["diff"])
        declared = {"compare": {"divergences": ["D18.unacknowledged-callback-child"]}}
        result = shape.evaluate(declared, self.raw())
        self.assertFalse(result["diff"] or runner.stale_divergences(result))
        changed = self.raw()
        changed["b"]["observation"]["sqliteRows"]["state/state.db"]["tables"]["operations"]["kept"]["status"] = "failed"
        self.assertTrue(shape.evaluate(declared, changed)["diff"])

    def test_d18_companion_is_stale_without_the_child_difference(self):
        declared = {"compare": {"divergences": ["D18.unacknowledged-callback-child"]}}
        self.assertTrue(runner.stale_divergences(shape.evaluate(declared, self.raw(child=False))))

    def test_d16_drops_only_named_revisions(self):
        def tables(revision, other):
            return {"plan_runs": {"${OTHER_RUN}": {"catalog_revision": revision, "status": "completed"}},
                    "provider_generations": {"com.opute.fixture.other-3": {"catalog_revision": revision, "status": other}},
                    "capability_invocations": {}}
        raw = {}
        for side, revision in [("a", "sha256:go"), ("b", "sha256:rust")]:
            t = tables(revision, "active")
            t["capability_invocations"] = {f"{side}-{i}": {} for i in range(1 if side == "a" else 2)}
            raw[side] = {"observation": {"sqlite": {"state/state.db": {"rowCounts": {"capability_invocations": len(t["capability_invocations"])}}},
                                         "sqliteRows": {"state/state.db": {"tables": t}}, "steps": {}}, "variables": {}}
        declared = {"compare": {"divergences": ["D16.rejected-candidate-keeps-owner"]}}
        self.assertTrue(shape.evaluate({}, copy.deepcopy(raw))["diff"])
        result = shape.evaluate(declared, copy.deepcopy(raw))
        self.assertFalse(result["diff"] or runner.stale_divergences(result))
        raw["b"]["observation"]["sqliteRows"]["state/state.db"]["tables"]["provider_generations"]["com.opute.fixture.other-3"]["status"] = "failed"
        self.assertTrue(shape.evaluate(declared, raw)["diff"])


class PlanRunSupportedTools(unittest.TestCase):
    """F-2: a plan run's persisted node output compares supportedTools as a set."""

    def raw(self, a, b):
        def side(tools):
            state = json.dumps({"outputs": {"n1": {"supportedTools": tools}}})
            tables = {"plan_runs": {"run": {"state_json": state}}}
            return {"observation": {"sqlite": {"state/state.db": {"rowCounts": {"plan_runs": 1}}},
                                    "sqliteRows": {"state/state.db": {"tables": tables}}, "steps": {}},
                    "variables": {}}
        return {"a": side(a), "b": side(b)}

    def test_order_is_not_a_difference_but_membership_is(self):
        self.assertFalse(shape.evaluate({}, self.raw(["x", "y"], ["y", "x"]))["diff"])
        self.assertTrue(shape.evaluate({}, self.raw(["x", "y"], ["y", "z"]))["diff"])


class RedactedRowAlignment(unittest.TestCase):
    path = ["sqliteRows", "state/state.db", "tables", "capability_invocations"]

    def doc(self, rows):
        return {"sqliteRows": {"state/state.db": {"tables": {"capability_invocations": rows}}}}

    def test_rust_rows_are_placed_opposite_the_go_rows_they_redact(self):
        go = self.doc([{"args": {"name": "a.service", "state": "start"}},
                       {"args": {"name": "b.service", "state": "stop"}}])
        rust = self.doc([{"args": {"name": "[redacted]", "state": "stop"}},
                         {"args": {"name": "[redacted]", "state": "start"}}])
        aligned = shape.align_redacted_rows(go, rust)
        self.assertEqual([r["args"]["state"] for r in runner._lookup(aligned, self.path)], ["start", "stop"])

    def test_a_real_difference_is_not_paired_away(self):
        go = self.doc([{"args": {"state": "start"}}])
        rust = self.doc([{"args": {"state": "stop"}}])
        aligned = shape.align_redacted_rows(go, rust)
        self.assertEqual(runner._lookup(aligned, self.path), [{"args": {"state": "stop"}}])
        self.assertTrue(canon.diff(go, aligned))


class LiveHostRows(unittest.TestCase):
    DECLARED = {"liveHostRows": {"table": "resource_registry", "resourceType": "host-service",
                                 "reason": "a live listing of the developer host's units"}}

    def row(self, name, scope="system"):
        uri = f"host-service:local:{scope}/{name}"
        return uri, {"uri": uri, "resource_type": "host-service", "tenant_id": "local",
                     "resource_id": f"{scope}/{name}", "status": "active",
                     "coordinates_json": {"$json": {"scope": scope, "serviceName": name}}}

    def doc(self, *rows, other=True):
        table = dict(rows)
        if other:
            table["incus:local:c1"] = {"uri": "incus:local:c1", "resource_type": "incus"}
        return {"sqlite": {"state/state.db": {"rowCounts": {"resource_registry": len(table)}}},
                "sqliteRows": {"state/state.db": {"tables": {"resource_registry": table}}}}

    def test_a_unit_listed_by_one_side_only_is_the_host(self):
        violations = {"a": [], "b": []}
        a, b = shape.drop_live_host_rows(self.DECLARED, self.doc(self.row("dbus")),
                                         self.doc(self.row("dbus"), self.row("NetworkManager-dispatcher")),
                                         violations)
        self.assertEqual(canon.diff(a, b), [])
        self.assertEqual(violations, {"a": [], "b": []})
        self.assertEqual(a["sqlite"]["state/state.db"]["rowCounts"]["resource_registry"], 1)

    def test_other_rows_are_still_compared(self):
        violations = {"a": [], "b": []}
        a, b = shape.drop_live_host_rows(self.DECLARED, self.doc(self.row("dbus")),
                                         self.doc(self.row("dbus"), other=False), violations)
        self.assertTrue(canon.diff(a, b))

    def test_undeclared_scenarios_compare_every_row(self):
        go, rust = self.doc(self.row("dbus")), self.doc(self.row("dbus"), self.row("extra"))
        a, b = shape.drop_live_host_rows({}, go, rust, {"a": [], "b": []})
        self.assertTrue(canon.diff(a, b))

    def test_a_malformed_registration_or_a_missing_listing_is_a_violation(self):
        uri, bad = self.row("dbus")
        bad = dict(bad, resource_id="system/other")
        violations = {"a": [], "b": []}
        shape.drop_live_host_rows(self.DECLARED, self.doc((uri, bad)), self.doc(self.row("dbus")), violations)
        self.assertTrue(violations["a"])
        violations = {"a": [], "b": []}
        shape.drop_live_host_rows(self.DECLARED, self.doc(), self.doc(self.row("dbus")), violations)
        self.assertTrue(violations["a"] and violations["b"])
