"""Contract suite files stay in sync with the pinned catalog source."""

import json
import unittest

from parity import contract, runner

SOURCE = runner.REPO_ROOT / "crates" / "host-agent" / "catalog" / "source.json"


class StandaloneGateContractTest(unittest.TestCase):
    def test_unpublished_names_are_the_unpublished_internal_definitions(self):
        """D10: the suite must name every dispatchable tool the standalone
        catalog does not publish, so a new internal tool cannot go untested."""
        src = json.loads(SOURCE.read_text())
        published = {d["name"] for key in ("hostDefinitions", "standaloneDefinitions", "standaloneFromAll")
                     for d in src[key]}
        unpublished = sorted({d["name"] for d in src["internalDefinitions"]} - published)
        doc = contract.load("standalone-read-only-gate")
        named = set()
        for scenario in doc["scenarios"]:
            for step in scenario["steps"]:
                named.update(step.get("sweep", {}).get("names", []))
        self.assertEqual(sorted(named), unpublished)

    def test_suites_load(self):
        for path in sorted(contract.CONTRACT_DIR.glob("*.json")):
            doc = contract.load(path.stem)
            self.assertTrue(doc.get("decision"), path.name)


if __name__ == "__main__":
    unittest.main()
