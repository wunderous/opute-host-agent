"""The incus shim matches a default-project call against fixtures written
without the project (D28, Q16) and records the project it was given."""
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from parity.shims import _project  # noqa: E402


class ProjectTest(unittest.TestCase):
    def test_a_project_flag_is_stripped_and_recorded(self):
        self.assertEqual(_project("incus", ["--project", "default", "list", "--format", "json"]),
                         (["list", "--format", "json"], "default"))

    def test_a_query_path_project_is_stripped_and_recorded(self):
        self.assertEqual(_project("incus", ["query", "/1.0/instances/a?project=default"]),
                         (["query", "/1.0/instances/a"], "default"))
        self.assertEqual(_project("incus", ["query", "/1.0/instances?recursion=1&project=default"]),
                         (["query", "/1.0/instances?recursion=1"], "default"))

    def test_a_call_without_a_project_is_unchanged(self):
        self.assertEqual(_project("incus", ["list"]), (["list"], None))
        self.assertEqual(_project("kubectl", ["--project", "default"]), (["--project", "default"], None))


if __name__ == "__main__":
    unittest.main()
