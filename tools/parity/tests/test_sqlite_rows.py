"""M5 shape parity: full typed row content, for structural diff against the
other side (masks and `canon.substitute` handle timestamps and per-side
random identity -- this test only checks the dump primitive itself)."""

import shutil
import sqlite3
import unittest

from parity.agent import Sandbox


class SqliteRowsTest(unittest.TestCase):
    def test_dumps_typed_values_not_repr_strings(self):
        sandbox = Sandbox("sqlite-rows-test", "test", None)
        self.addCleanup(shutil.rmtree, sandbox.root)
        conn = sqlite3.connect(sandbox.root / "state" / "test.db")
        self.addCleanup(conn.close)
        conn.execute(
            "CREATE TABLE operations("
            "operation_id TEXT PRIMARY KEY, status TEXT, result_json TEXT)"
        )
        conn.execute(
            "INSERT INTO operations VALUES ('op-1', 'completed', '{\"ok\":true}')"
        )
        conn.execute("INSERT INTO operations VALUES ('op-2', 'failed', NULL)")
        conn.commit()

        dumped = sandbox.sqlite_rows()
        self.assertIn("state/test.db", dumped)
        rows = dumped["state/test.db"]["tables"]["operations"]
        self.assertEqual(set(rows), {"op-1", "op-2"})
        self.assertEqual(rows["op-1"]["status"], "completed")
        self.assertEqual(rows["op-1"]["result_json"], '{"ok":true}')
        self.assertIsNone(rows["op-2"]["result_json"])

    def test_tables_with_no_single_column_primary_key_stay_a_list(self):
        sandbox = Sandbox("sqlite-rows-no-pk-test", "test", None)
        self.addCleanup(shutil.rmtree, sandbox.root)
        conn = sqlite3.connect(sandbox.root / "state" / "test.db")
        self.addCleanup(conn.close)
        conn.execute(
            "CREATE TABLE plan_runs(plan_id TEXT, generation INTEGER, "
            "PRIMARY KEY (plan_id, generation))"
        )
        conn.execute("INSERT INTO plan_runs VALUES ('p-1', 0)")
        conn.commit()

        dumped = sandbox.sqlite_rows()
        self.assertEqual(
            dumped["state/test.db"]["tables"]["plan_runs"],
            [{"plan_id": "p-1", "generation": 0}],
        )

    def test_wal_and_shm_files_are_not_mistaken_for_databases(self):
        sandbox = Sandbox("sqlite-rows-wal-test", "test", None)
        self.addCleanup(shutil.rmtree, sandbox.root)
        (sandbox.root / "state" / "test.db-wal").write_bytes(b"not a database")
        (sandbox.root / "state" / "test.db-shm").write_bytes(b"not a database")
        conn = sqlite3.connect(sandbox.root / "state" / "test.db")
        self.addCleanup(conn.close)
        conn.execute("CREATE TABLE t(id INTEGER PRIMARY KEY)")
        conn.commit()

        dumped = sandbox.sqlite_rows()
        self.assertEqual(list(dumped), ["state/test.db"])


if __name__ == "__main__":
    unittest.main()
