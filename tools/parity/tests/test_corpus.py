import json
import unittest

from parity import corpus


class CorpusTest(unittest.TestCase):
    def test_committed_corpus_matches_generator(self):
        self.assertEqual(corpus.OUT_FILE.read_text(), corpus.render(),
                         "scenarios/wire.json is stale: run `python3 -m parity corpus`")

    def test_corpus_meets_the_m2_size_target(self):
        steps = sum(len(s["steps"]) for s in corpus.generate())
        self.assertGreaterEqual(steps, 500)

    def test_every_inventoried_method_runs_in_both_flag_states(self):
        for legacy in (False, True):
            labels = {s["as"] for s in corpus.methods(legacy)["steps"]}
            for method in corpus.METHODS:
                self.assertIn(f"{method or '<empty>'} valid", labels)
                self.assertIn(f"{method or '<empty>'} bare", labels)

    def test_token_issuance_endpoints_are_not_exercised(self):
        # Issuance is deferred pending an owner decision; see corpus.py.
        text = corpus.render()
        self.assertNotIn("/oauth/token", text)
        self.assertNotIn("/oauth/authorize", text)

    def test_tools_list_results_are_masked_by_type_only(self):
        doc = corpus.methods(False)
        masked = {tuple(m["path"]) for m in doc["compare"]["masks"]}
        for step in doc["steps"]:
            if step["as"].startswith("tools/list "):
                self.assertIn(("steps", step["as"], "body", "result", "tools"), masked)

    def test_labels_are_unique_per_scenario(self):
        for scenario in corpus.generate():
            labels = [s["as"] for s in scenario["steps"]]
            self.assertEqual(len(labels), len(set(labels)), scenario["id"])
            json.dumps(scenario)


if __name__ == "__main__":
    unittest.main()
