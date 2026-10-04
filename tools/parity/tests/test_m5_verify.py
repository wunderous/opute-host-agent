import copy
import json
import tempfile
import unittest
from pathlib import Path

from parity import cross_read, m5_verify, migration, runner
from parity import verify


class CrossReadEvidence(unittest.TestCase):
    def setUp(self):
        self.outcomes = {}
        for a, b in [("go", "rust"), ("rust", "go")]:
            for case, (live, restored) in cross_read.CASES.items():
                for repeat in range(3):
                    key = f"{a}-writes-{b}-reads.{case}.{repeat}"
                    observation = {"taskId": "task", "tasksGet": {"body": {"result": {"status": restored}}},
                                   "tasksList": {"body": {"error": {"code": -32601}}},
                                   "stateRows": {"tables": {"operations": {"task": {
                                       "updated_at": "2026-10-04 00:00:00", "task_snapshot_json": "{}"}}}},
                                   "variables": {}}
                    self.outcomes[key] = {"diff": [], "maskViolations": {}, "raw": {
                        "written": {"taskId": "task", "persisted": {"task": {}},
                                    "live": {"body": {"result": {"status": live}}}},
                        "reference": copy.deepcopy(observation), "readBack": copy.deepcopy(observation)}}
        self.summary = {"directions": list(self.outcomes)}

    def test_complete_matrix_passes(self):
        self.assertEqual(m5_verify.cross_read_failures(self.summary, self.outcomes), [])

    def test_missing_direction_does_not_pass(self):
        self.outcomes.pop(next(iter(self.outcomes)))
        self.assertTrue(m5_verify.cross_read_failures(self.summary, self.outcomes))

    def test_matching_missing_list_queries_do_not_pass(self):
        raw = self.outcomes[next(iter(self.outcomes))]["raw"]
        for side in ["reference", "readBack"]:
            raw[side].pop("tasksList")
        self.assertTrue(m5_verify.cross_read_failures(self.summary, self.outcomes))

    def test_claimed_green_diff_cannot_hide_changed_raw_row(self):
        raw = self.outcomes[next(iter(self.outcomes))]["raw"]
        raw["readBack"]["stateRows"]["tables"]["operations"]["task"]["task_snapshot_json"] = '{"changed":true}'
        self.assertTrue(m5_verify.cross_read_failures(self.summary, self.outcomes))

    def test_equal_readers_cannot_hide_wrong_restored_state(self):
        raw = self.outcomes[next(iter(self.outcomes))]["raw"]
        for side in ["reference", "readBack"]:
            raw[side]["tasksGet"]["body"]["result"]["status"] = "completed"
        self.assertTrue(m5_verify.cross_read_failures(self.summary, self.outcomes))

    def test_vacuous_missing_writer_state_does_not_pass(self):
        self.outcomes[next(iter(self.outcomes))]["raw"]["written"]["persisted"] = {}
        self.assertTrue(m5_verify.cross_read_failures(self.summary, self.outcomes))

    def test_malformed_observation_does_not_pass(self):
        self.outcomes[next(iter(self.outcomes))] = {"status": "pass"}
        self.assertTrue(m5_verify.cross_read_failures(self.summary, self.outcomes))

    def _artifact_check(self, change=None):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            outcomes = root / "directions.json"
            outcomes.write_text(json.dumps(self.outcomes))
            summary = copy.deepcopy(self.summary)
            summary["files"] = {"directions.json": runner.sha256_file(outcomes)}
            summary["provenance"] = {
                "sourceLock": {}, "goBinarySha256": "a" * 64, "rustBinarySha256": "b" * 64,
                "driverSha256": runner.sha256_file(Path(cross_read.__file__)),
                "harnessSha256": runner.harness_sha256()}
            if change:
                change(root, summary)
            (root / "summary.json").write_text(json.dumps(summary))
            manifest = {"m5Evidence": {"cross-read": {"summary": "summary.json"}},
                        "goReference": {"binarySha256": "a" * 64}, "rustCandidate": {"binarySha256": "b" * 64}}
            return m5_verify.check(root, manifest, {}, verify.Report(), verify._load, verify._sha)

    def test_missing_requirements_remain_unverified_with_green_cross_read(self):
        statuses = self._artifact_check()
        self.assertEqual(statuses["cross-read"], "pass")
        self.assertEqual({s for key, s in statuses.items() if key != "cross-read"}, {"unverified"})

    def test_changed_raw_file_invalidates_evidence(self):
        statuses = self._artifact_check(lambda root, _: (root / "directions.json").write_text("{}"))
        self.assertEqual(statuses["cross-read"], "unverified")

    def test_stale_binary_and_driver_invalidate_evidence(self):
        for key in ["goBinarySha256", "rustBinarySha256", "driverSha256", "harnessSha256"]:
            with self.subTest(key=key):
                statuses = self._artifact_check(lambda _, doc: doc["provenance"].update({key: "c" * 64}))
                self.assertEqual(statuses["cross-read"], "unverified")


class FullRowRejectionInvariant(unittest.TestCase):
    def test_equal_execution_failures_never_establish_parity(self):
        observation = {"steps": {}, "_executionError": "No space left on device"}
        self.assertTrue(runner.invariant_violations({}, observation, "go"))
        self.assertTrue(runner.invariant_violations({}, observation, "rust"))

    def test_d12_cannot_exempt_rust_row(self):
        scenario = {"invariants": ["X2"], "x2GoGaps": {"operations": "D12"}}
        observation = {"trace": [], "sqliteRows": {"state/state.db": {
            "tables": {"operations": {"refused": {"status": "working"}}}}}}
        self.assertEqual(runner.invariant_violations(scenario, observation, "go"), [])
        self.assertTrue(runner.invariant_violations(scenario, observation, "rust"))


class MigrationEvidence(unittest.TestCase):
    def setUp(self):
        keys = {"operations": "operation_id", "plan_runs": "run_id", "provider_generations": "generation_id",
                "capability_invocations": "invocation_id", "resource_registry": "uri",
                "active_runtimes": "capability", "active_capabilities": "capability"}
        self.outcomes = {}
        for case in migration.CASES:
            rows = copy.deepcopy(migration.SEEDS)
            for table in ["operations", "plan_runs"]:
                for row in rows[table]:
                    if row["status"] in ["working", "running"]:
                        row["status"] = "unknown"
                        row["updated_at"] = "2026-10-04 00:01:00"
            if case == "pre-column-additions":
                rows["resource_registry"] = []
                for table, columns in migration.REMOVED.items():
                    for row in rows[table]:
                        for column in columns:
                            row[column] = "{}" if column in ["input_bindings_json", "observation_json"] else ""
                rows["active_capabilities"] = [dict(rows["active_runtimes"][0])]
                rows["active_capabilities"][0]["provider"] = rows["active_capabilities"][0].pop("runtime")
            tables = {table: {row[keys[table]]: row for row in data} for table, data in rows.items()}
            observation = {"fixture": {"case": case, "releaseFixtureSha256": runner.sha256_file(migration.RELEASE_FIXTURE)},
                           "schema": {}, "rows": {"tables": tables}}
            for repeat in range(3):
                self.outcomes[f"{case}.{repeat}"] = {"diff": [], "failures": [],
                    "raw": {"go": copy.deepcopy(observation), "rust": copy.deepcopy(observation)}}
        self.summary = {"repeats": 3, "cases": list(migration.CASES)}

    def test_preserved_matrix_passes(self):
        self.assertEqual(m5_verify.migration_failures(self.summary, self.outcomes), [])

    def test_equal_data_loss_on_both_sides_fails(self):
        raw = self.outcomes["released-v0.2.2.0"]["raw"]
        for side in ["go", "rust"]:
            del raw[side]["rows"]["tables"]["capability_invocations"]["i1"]
        self.assertTrue(m5_verify.migration_failures(self.summary, self.outcomes))

    def test_schema_difference_cannot_hide_behind_green_diff(self):
        self.outcomes["released-v0.2.2.0"]["raw"]["rust"]["schema"] = {"unexpected": True}
        self.assertTrue(m5_verify.migration_failures(self.summary, self.outcomes))

    def test_lost_identity_and_invented_success_fail(self):
        for column, value in [("operation_id", "different"), ("status", "completed")]:
            with self.subTest(column=column):
                outcome = copy.deepcopy(self.outcomes)
                for side in ["go", "rust"]:
                    outcome["released-v0.2.2.0"]["raw"][side]["rows"]["tables"]["operations"]["op-working"][column] = value
                self.assertTrue(m5_verify.migration_failures(self.summary, outcome))

    def test_missing_case_fails(self):
        self.outcomes.pop("pre-column-additions.2")
        self.assertTrue(m5_verify.migration_failures(self.summary, self.outcomes))


if __name__ == "__main__":
    unittest.main()
