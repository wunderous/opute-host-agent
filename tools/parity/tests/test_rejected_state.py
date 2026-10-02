"""Rejected-call state checks must catch writes that do not change counts."""

import shutil
import sqlite3
import unittest

from parity.agent import Sandbox


class DurableSnapshotTest(unittest.TestCase):
    def test_updates_and_new_tables_change_snapshot_but_select_does_not(self):
        sandbox = Sandbox("snapshot-test", "test", None)
        self.addCleanup(shutil.rmtree, sandbox.root)
        conn = sqlite3.connect(sandbox.root / "state" / "test.db")
        self.addCleanup(conn.close)
        conn.execute("CREATE TABLE audit(id INTEGER PRIMARY KEY, value TEXT)")
        conn.execute("INSERT INTO audit VALUES (1, 'before')")
        conn.commit()
        before = sandbox.durable_snapshot()
        self.assertEqual(before["count"], 1)
        conn.execute("SELECT * FROM audit").fetchall()
        self.assertEqual(sandbox.durable_snapshot(), before)
        conn.execute("UPDATE audit SET value='after' WHERE id=1")
        conn.commit()
        updated = sandbox.durable_snapshot()
        self.assertNotEqual(updated["digest"], before["digest"])
        conn.execute("CREATE TABLE extra(value TEXT)")
        conn.commit()
        self.assertNotEqual(sandbox.durable_snapshot()["digest"], updated["digest"])
        self.assertNotIn("before", str(before))
        self.assertNotIn("after", str(updated))


if __name__ == "__main__":
    unittest.main()
