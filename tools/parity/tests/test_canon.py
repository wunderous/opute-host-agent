import unittest

from parity import canon


class CanonTest(unittest.TestCase):
    def test_key_order_is_ignored_array_order_is_not(self):
        self.assertEqual(canon.diff({"a": 1, "b": 2}, {"b": 2, "a": 1}), [])
        self.assertEqual(len(canon.diff([1, 2], [2, 1])), 2)

    def test_declared_set_ignores_order(self):
        spec = {"sets": [["xs"]]}
        left, _ = canon.normalize({"xs": [1, 2, 3]}, spec, {})
        right, _ = canon.normalize({"xs": [3, 1, 2]}, spec, {})
        self.assertEqual(canon.diff(left, right), [])

    def test_set_still_detects_membership_change(self):
        spec = {"sets": [["xs"]]}
        left, _ = canon.normalize({"xs": [1, 2]}, spec, {})
        right, _ = canon.normalize({"xs": [1, 3]}, spec, {})
        self.assertNotEqual(canon.diff(left, right), [])

    def test_typed_mask_hides_value_of_declared_type(self):
        spec = {"masks": [{"path": ["t"], "type": "rfc3339", "reason": "clock"}]}
        left, vl = canon.normalize({"t": "2026-09-30T02:00:00Z"}, spec, {})
        right, vr = canon.normalize({"t": "2026-09-30T03:00:00.5Z"}, spec, {})
        self.assertEqual((vl, vr), ([], []))
        self.assertEqual(canon.diff(left, right), [])

    def test_typed_mask_does_not_hide_wrong_type(self):
        spec = {"masks": [{"path": ["t"], "type": "rfc3339", "reason": "clock"}]}
        doc, violations = canon.normalize({"t": ""}, spec, {})
        self.assertEqual(len(violations), 1)
        self.assertIn("$maskViolation", doc["t"])

    def test_mask_on_missing_path_is_a_violation(self):
        spec = {"masks": [{"path": ["t"], "type": "rfc3339", "reason": "clock"}]}
        _, violations = canon.normalize({}, spec, {})
        self.assertEqual(violations[0]["reason"], "mask path not found")

    def test_mask_requires_reason_and_known_type(self):
        with self.assertRaises(canon.MaskError):
            canon.normalize({"t": 1}, {"masks": [{"path": ["t"], "type": "int"}]}, {})
        with self.assertRaises(canon.MaskError):
            canon.normalize({"t": 1}, {"masks": [{"path": ["t"], "type": "regex", "reason": "x"}]}, {})

    def test_substitution_is_exact_and_longest_first(self):
        doc = canon.substitute(
            {"p": "/tmp/sb/home", "id": "agent-a", "k": "agent-a-extra"},
            {"SANDBOX": "/tmp/sb", "AGENT_ID": "agent-a"},
        )
        self.assertEqual(doc, {"p": "${SANDBOX}/home", "id": "${AGENT_ID}", "k": "${AGENT_ID}-extra"})

    def test_embedded_json_is_parsed_and_masked(self):
        spec = {
            "parseJson": [["content", "*", "text"]],
            "masks": [{"path": ["content", 0, "text", "$json", "n"], "type": "int", "reason": "live"}],
        }
        left, _ = canon.normalize({"content": [{"text": '{"n": 1, "s": "x"}'}]}, spec, {})
        right, _ = canon.normalize({"content": [{"text": '{"s": "x", "n": 2}'}]}, spec, {})
        self.assertEqual(canon.diff(left, right), [])
        changed, _ = canon.normalize({"content": [{"text": '{"s": "y", "n": 2}'}]}, spec, {})
        self.assertNotEqual(canon.diff(left, changed), [])

    def test_bool_is_not_a_number(self):
        self.assertNotEqual(canon.diff({"x": True}, {"x": 1}), [])


if __name__ == "__main__":
    unittest.main()


class DivergenceTest(unittest.TestCase):
    REGISTRY = {
        "files": {"id": "files", "decision": "D8", "reason": "r", "path": ["files"],
                  "drop": {"field": "path", "prefix": "state/credentials"}},
        "rows": {"id": "rows", "decision": "D8", "reason": "r", "path": ["rows"], "dropKeys": ["new"]},
        "log": {"id": "log", "decision": "D8", "reason": "r", "path": ["steps", "*", "stderr"],
                "dropLines": {"contains": "msg=oauth "}},
    }

    def test_drops_only_declared_content_from_both_sides(self):
        a = {"files": [{"path": "state/state.db"}], "rows": {"clients": 2},
             "steps": {"s": {"stderr": "x\nlisten"}}}
        b = {"files": [{"path": "state/credentials"}, {"path": "state/state.db"}],
             "rows": {"clients": 2, "new": 0}, "steps": {"s": {"stderr": "x\nlevel=INFO msg=oauth event=migrate\nlisten"}}}
        a2, b2, rec = canon.apply_divergences(a, b, ["files", "rows", "log"], self.REGISTRY)
        self.assertEqual(canon.diff(a2, b2), [])
        self.assertEqual([r["stale"] for r in rec], [False, False, False])

    def test_undeclared_difference_survives(self):
        a = {"files": [{"path": "state/state.db", "mode": "0o644"}]}
        b = {"files": [{"path": "state/state.db", "mode": "0o600"}, {"path": "state/credentials/x"}]}
        a2, b2, _ = canon.apply_divergences(a, b, ["files"], self.REGISTRY)
        self.assertNotEqual(canon.diff(a2, b2), [])

    def test_identical_removal_is_stale(self):
        a = {"rows": {"clients": 2}}
        _, _, rec = canon.apply_divergences(a, dict(a), ["rows"], self.REGISTRY)
        self.assertTrue(rec[0]["stale"])

    def test_undefined_divergence_is_an_error(self):
        with self.assertRaises(canon.DivergenceError):
            canon.apply_divergences({}, {}, ["nope"], self.REGISTRY)

    def test_registry_file_is_valid(self):
        from parity import runner
        rules = canon.load_divergences(runner.DIVERGENCE_FILE)
        for scenario in runner.load_scenarios():
            for did in scenario.get("compare", {}).get("divergences", []):
                self.assertIn(did, rules, scenario["id"])


class OmitemptyTest(unittest.TestCase):
    RULE = {"path": ["psi", "*"], "keys": ["someAvg10"], "zero": 0, "reason": "omitempty"}

    def test_absent_equals_zero(self):
        a = canon.apply_omitempty({"psi": {"io": {}}}, [self.RULE])
        b = canon.apply_omitempty({"psi": {"io": {"someAvg10": 0}}}, [self.RULE])
        self.assertEqual(canon.diff(a, b), [])

    def test_nonzero_still_differs(self):
        a = canon.apply_omitempty({"psi": {"io": {}}}, [self.RULE])
        b = canon.apply_omitempty({"psi": {"io": {"someAvg10": 0.5}}}, [self.RULE])
        self.assertNotEqual(canon.diff(a, b), [])

    def test_rule_needs_reason(self):
        with self.assertRaises(canon.MaskError):
            canon.apply_omitempty({}, [{**self.RULE, "reason": ""}])


class SubstituteTest(unittest.TestCase):
    def test_port_inside_a_longer_number_is_kept(self):
        text = '{"totalBytes":270553174016,"endpoint":"http://127.0.0.1:27055/mcp"}'
        out = canon.substitute(text, {"PORT": "27055"})
        self.assertEqual(out, '{"totalBytes":270553174016,"endpoint":"http://127.0.0.1:${PORT}/mcp"}')

    def test_longer_literals_first(self):
        self.assertEqual(canon.substitute("/tmp/a/b", {"SANDBOX": "/tmp/a", "X": "/tmp"}), "${SANDBOX}/b")
