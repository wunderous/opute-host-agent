"""Acceptance checker negative controls against retained, real observations."""
import base64
import copy
import gzip
import json
import unittest

from parity import m5_verify, runner, secret_sweep, unknown_projection


class SecretSinkChecks(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.corpus = json.loads(gzip.decompress((runner.REPO_ROOT / "evidence/m5/secrets/outcomes.json.gz").read_bytes()))

    def setUp(self):
        self.data = copy.deepcopy(self.corpus)

    def test_complete_real_sink_corpus_passes(self):
        self.assertEqual(secret_sweep.failures(self.data), [])

    def test_unsupported_nested_markings_fail_closed(self):
        schemas = [{"type": "array", "items": {"writeOnly": True}},
                   {"type": "object", "additionalProperties": {"writeOnly": True}},
                   {"oneOf": [{"properties": {"secret": {"writeOnly": True}}}]}]
        for schema in schemas:
            with self.subTest(schema=schema), self.assertRaises(ValueError):
                secret_sweep.field_cases({"tools": [{"name": "probe", "inputSchema": {"properties": {"nested": schema}}}]}, "platform")

    def test_green_hit_list_cannot_hide_raw_wal_leak(self):
        file = self.data["outcomes"]["go.platform"]["scannedFiles"]["live"][1]
        raw = base64.b64decode(file["dataBase64"]) + self.data["cases"]["platform"][0]["canary"].encode()
        file.update(dataBase64=base64.b64encode(raw).decode(), bytes=len(raw), sha256=runner.sha256_bytes(raw))
        self.assertTrue(secret_sweep.failures(self.data))

    def test_missing_live_journal_does_not_pass(self):
        files = self.data["outcomes"]["rust.platform"]["scannedFiles"]["live"]
        files[:] = [f for f in files if f["path"] != "state.db-wal"]
        self.assertTrue(secret_sweep.failures(self.data))

    def test_missing_catalog_field_does_not_pass(self):
        self.data["cases"]["platform"].pop()
        self.assertTrue(secret_sweep.failures(self.data))

    def test_matching_missing_writes_do_not_pass(self):
        for side in ["go", "rust"]:
            self.data["outcomes"][side + ".standalone"]["liveRows"]["tables"]["capability_invocations"] = {}
        self.assertTrue(secret_sweep.failures(self.data))

    def test_matching_secret_context_retention_does_not_pass(self):
        case = self.data["cases"]["platform"][0]
        for side in ["go", "rust"]:
            self.data["outcomes"][side + ".platform"]["projected"][case["id"]]["state"]["context"]["secret"]["value"] = "arbitrary"
        self.assertTrue(secret_sweep.failures(self.data))

    def test_modified_retained_bytes_do_not_pass(self):
        self.data["outcomes"]["rust.standalone"]["scannedFiles"]["recovered"][0]["dataBase64"] = "AA=="
        self.assertTrue(secret_sweep.failures(self.data))


class CrashSinkChecks(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        root = runner.REPO_ROOT / "evidence/m5/crash"
        cls.summary = json.loads((root / "summary.json").read_text())
        cls.corpus = json.loads(gzip.decompress((root / "outcomes.json.gz").read_bytes()))

    def setUp(self):
        self.data = copy.deepcopy(self.corpus)

    def test_complete_real_crash_corpus_passes(self):
        self.assertEqual(m5_verify.crash_failures(self.summary, self.data), [])

    def test_missing_seed_does_not_pass(self):
        self.data.pop("199")
        self.assertTrue(m5_verify.crash_failures(self.summary, self.data))

    def test_matching_torn_state_does_not_pass(self):
        for side in ["go", "rust"]:
            self.data["0"]["raw"][side]["afterKill"]["tables"]["plan_runs"] = {}
        self.assertTrue(m5_verify.crash_failures(self.summary, self.data))

    def test_matching_unreached_checkpoint_does_not_pass(self):
        for side in ["go", "rust"]:
            self.data["0"]["raw"][side]["checkpoint"]["walBytes"] = 0
        self.assertTrue(m5_verify.crash_failures(self.summary, self.data))

    def test_matching_graceful_exit_does_not_pass(self):
        for side in ["go", "rust"]:
            self.data["0"]["raw"][side]["checkpoint"]["exit"] = 0
        self.assertTrue(m5_verify.crash_failures(self.summary, self.data))


class UnknownProjectionChecks(unittest.TestCase):
    """D13 (`redact-unmarked-projections`): Rust now diverges from Go's
    pinned, unchanged verbatim persistence. These are negative controls
    against the real, current observations, proving the checker still
    catches a Rust regression and a stale (no-longer-real) divergence,
    rather than just recording today's green result."""

    @classmethod
    def setUpClass(cls):
        cls.corpus = json.loads((runner.REPO_ROOT / "evidence/m5/unknown-projection/outcomes.json").read_text())

    def setUp(self):
        self.data = copy.deepcopy(self.corpus)

    def test_current_outcomes_show_only_the_declared_divergence(self):
        self.assertEqual(unknown_projection.failures(self.data), [])

    def test_rust_regression_is_caught(self):
        canary = unknown_projection.CANARY
        self.data["rust.input-unmarked"]["stateRows"]["capability_invocations"] = {
            "x": {"arguments_json": json.dumps({"unmarked": canary})}
        }
        failures = unknown_projection.failures(self.data)
        self.assertTrue(any("D13 regression" in f for f in failures))

    def test_stale_divergence_is_caught(self):
        for table in self.data["go.input-unmarked"]["stateRows"].values():
            for row in table.values():
                for column in row:
                    if isinstance(row[column], str) and unknown_projection.CANARY in row[column]:
                        row[column] = row[column].replace(unknown_projection.CANARY, "")
        failures = unknown_projection.failures(self.data)
        self.assertTrue(any("went missing" in f for f in failures))

    def test_missing_projection_coverage_does_not_pass(self):
        self.assertTrue(unknown_projection.failures({}))
