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
