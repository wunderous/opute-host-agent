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

    def test_match_mask_covers_only_matching_values(self):
        spec = {"masks": [{"path": ["argv", "*"], "type": "string", "match": r".*/tmp-\d+",
                           "reason": "random temp name"}]}
        left, vl = canon.normalize({"argv": ["-m", "/x/tmp-1"]}, spec, {})
        right, vr = canon.normalize({"argv": ["-m", "/x/tmp-2"]}, spec, {})
        self.assertEqual((vl, vr, canon.diff(left, right)), ([], [], []))
        other, _ = canon.normalize({"argv": ["-n", "/x/tmp-2"]}, spec, {})
        self.assertNotEqual(canon.diff(left, other), [])

    def test_mask_on_missing_path_is_a_violation(self):
        spec = {"masks": [{"path": ["t"], "type": "rfc3339", "reason": "clock"}]}
        _, violations = canon.normalize({}, spec, {})
        self.assertEqual(violations[0]["reason"], "mask path not found")

    def test_mask_requires_reason_and_known_type(self):
        with self.assertRaises(canon.MaskError):
            canon.normalize({"t": 1}, {"masks": [{"path": ["t"], "type": "int"}]}, {})
        with self.assertRaises(canon.MaskError):
            canon.normalize({"t": 1}, {"masks": [{"path": ["t"], "type": "regex", "reason": "x"}]}, {})

    def test_one_of_mask_accepts_only_declared_values(self):
        spec = {"masks": [{"path": ["m"], "type": "one-of", "values": ["/", "${SANDBOX}/home"], "reason": "tie"}]}
        left, lv = canon.normalize({"m": "/"}, spec, {})
        right, rv = canon.normalize({"m": "/tmp/sb/home"}, spec, {"SANDBOX": "/tmp/sb"})
        self.assertEqual((left, lv, rv), (right, [], []))
        _, violations = canon.normalize({"m": "/var"}, spec, {})
        self.assertEqual(violations[0]["value"], "/var")
        with self.assertRaises(canon.MaskError):
            canon.normalize({"m": "/"}, {"masks": [{"path": ["m"], "type": "one-of", "values": ["/"], "reason": "x"}]}, {})

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
        "redacted": {"id": "redacted", "decision": "D13", "reason": "r", "path": ["result"],
                     "dropValueDeep": "[redacted]"},
    }

    def test_drop_value_deep_removes_matching_leaves_at_any_depth(self):
        a = {"result": {"hostName": "h", "capacity": {"totalVmCount": 2, "totalContainerCount": 1}}}
        b = {"result": {"hostName": "h", "capacity": {"totalVmCount": "[redacted]", "totalContainerCount": "[redacted]"}}}
        a2, b2, rec = canon.apply_divergences(a, b, ["redacted"], self.REGISTRY)
        self.assertEqual(canon.diff(a2, b2), [])
        self.assertFalse(rec[0]["stale"])

    def test_drop_value_deep_is_stale_when_nothing_matches(self):
        a = {"result": {"hostName": "h"}}
        _, _, rec = canon.apply_divergences(a, dict(a), ["redacted"], self.REGISTRY)
        self.assertTrue(rec[0]["stale"])

    def test_drop_value_deep_leaves_a_real_difference_undeclared(self):
        a = {"result": {"hostName": "a"}}
        b = {"result": {"hostName": "b"}}
        a2, b2, _ = canon.apply_divergences(a, b, ["redacted"], self.REGISTRY)
        self.assertNotEqual(canon.diff(a2, b2), [])

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

    def test_port_inside_a_hex_hash_is_kept(self):
        # A hex digest's digit run can coincidentally equal the port string,
        # flanked by a-f letters rather than digits -- the digit-only
        # boundary check alone would still match and corrupt the hash.
        text = "d1b3b144e3b77a18c87b3a4354cfb20331e00759e222643251ca159f01550c98"
        self.assertEqual(canon.substitute(text, {"PORT": "20331"}), text)


class KeepPrefixTest(unittest.TestCase):
    """D20: only the text after a declared prefix is the declared difference."""

    REGISTRY = {"tail": {"id": "tail", "decision": "D20", "reason": "r",
                         "path": ["steps", "*", "text"], "keepPrefix": "Error: x: "}}

    def test_text_after_the_prefix_is_dropped(self):
        a = {"steps": {"s": {"text": "Error: x: go reason"}}}
        b = {"steps": {"s": {"text": "Error: x: rust reason"}}}
        a2, b2, rec = canon.apply_divergences(a, b, ["tail"], self.REGISTRY)
        self.assertEqual(canon.diff(a2, b2), [])
        self.assertEqual(rec, [{"id": "tail", "decision": "D20", "stale": False}])

    def test_text_without_the_prefix_stays_a_diff(self):
        a = {"steps": {"s": {"text": "Error: x: reason"}}}
        b = {"steps": {"s": {"text": "Error: y: reason"}}}
        a2, b2, _ = canon.apply_divergences(a, b, ["tail"], self.REGISTRY)
        self.assertNotEqual(canon.diff(a2, b2), [])

    def test_identical_tails_are_stale(self):
        a = {"steps": {"s": {"text": "Error: x: same"}}}
        _, _, rec = canon.apply_divergences(a, {"steps": {"s": {"text": "Error: x: same"}}}, ["tail"], self.REGISTRY)
        self.assertTrue(rec[0]["stale"])


class NeutralNamesTest(unittest.TestCase):
    """D19.neutral-host-tool-names: the Rust side is rewritten to Go's names
    through the one name map and the pinned revision pairs, nothing else."""

    @classmethod
    def setUpClass(cls):
        import json
        from parity import runner
        cls.registry = canon.load_divergences(runner.DIVERGENCE_FILE)
        manifest = json.loads((canon._REPO / "parity-manifest.json").read_text())
        cls.pair = manifest["catalogRevisions"]["platform"]

    def apply(self, go, rust):
        return canon.apply_divergences(go, rust, ["D19.neutral-host-tool-names"], self.registry)

    def catalog(self, revision, install, install_field, compact_text):
        return {"steps": {"list": {"revision": revision, "tools": sorted([
            {"name": install, "inputSchema": {"properties": {install_field: {"type": "string"}},
                                               "required": [install_field]}},
            {"name": "get_host_info", "inputSchema": {"properties": {"packageName": {}}}},
            {"name": "install_k3s"},
            {"name": compact_text},
        ], key=lambda t: t["name"]), "text": "compact needs " + compact_text}}}

    def test_field_rename_on_a_tool_that_keeps_its_name(self):
        # D21: provision_container keeps its name but publishes hostGpuLibs.
        def tool(field):
            return {"steps": {"list": {"tools": [{"name": "provision_container", "inputSchema": {
                "properties": {field: {"type": "boolean"}, "gpu": {"type": "boolean"}}}}]}}}
        _, b, records = self.apply(tool("wslGpuLibs"), tool("hostGpuLibs"))
        self.assertEqual(b, tool("wslGpuLibs"))
        self.assertFalse(records[0]["stale"])
        # Outside that tool's input schema the field name is left alone.
        other = {"steps": {"x": {"hostGpuLibs": True}}}
        _, b, records = self.apply(other, other)
        self.assertEqual(b, other)
        self.assertTrue(records[0]["stale"])

    def test_renamed_catalog_matches_go(self):
        go = self.catalog(self.pair["go"], "install_incus_stack", "incusPackage", "shutdown_wsl")
        rust = self.catalog(self.pair["rust"], "install_virtualization_stack", "packageName",
                            "shutdown_host_vm_subsystem")
        self.assertNotEqual([t["name"] for t in go["steps"]["list"]["tools"]].index("install_k3s"),
                            [t["name"] for t in rust["steps"]["list"]["tools"]].index("install_k3s"))
        a, b, rec = self.apply(go, rust)
        self.assertEqual(canon.diff(a, b), [])
        self.assertEqual(rec, [{"id": "D19.neutral-host-tool-names", "decision": "D19", "stale": False}])
        self.assertEqual(a, go)

    def test_unrelated_field_and_partial_words_are_kept(self):
        rust = {"steps": {"s": {"name": "get_host_info", "inputSchema": {"properties": {"packageName": 1}},
                                "text": "xshutdown_host_vm_subsystem"}}}
        _, b, rec = self.apply({}, rust)
        self.assertEqual(b, rust)
        self.assertTrue(rec[0]["stale"])

    def test_canary_unmapped_rename_and_unpinned_revision_stay_diffs(self):
        go = self.catalog(self.pair["go"], "install_incus_stack", "incusPackage", "shutdown_wsl")
        unmapped = self.catalog(self.pair["rust"], "install_virt_stack", "packageName",
                                "shutdown_host_vm_subsystem")
        a, b, _ = self.apply(go, unmapped)
        self.assertNotEqual(canon.diff(a, b), [])
        unpinned = self.catalog("sha256:" + "0" * 64, "install_virtualization_stack", "packageName",
                                "shutdown_host_vm_subsystem")
        a, b, _ = self.apply(go, unpinned)
        self.assertEqual([d["path"] for d in canon.diff(a, b)], [["steps", "list", "revision"]])

    def test_only_the_rust_side_is_rewritten(self):
        rust = {"steps": {"s": "shutdown_host_vm_subsystem"}}
        a, b, _ = canon.apply_divergences(rust, {"steps": {"s": "x"}}, ["D19.neutral-host-tool-names"],
                                          self.registry, rust_side="b")
        self.assertEqual(a, rust)


class EmbeddedNumberTest(unittest.TestCase):
    def test_substituted_port_parses_back_as_that_sides_number(self):
        doc = canon.substitute({"row": '{"port":43131,"url":"http://127.0.0.1:43131/"}'}, {"STUB_PORT": "43131"})
        parsed = canon.parse_embedded(doc, [["row"]], {"STUB_PORT": "43131"})
        self.assertEqual(parsed, {"row": {"$json": {"port": 43131, "url": "http://127.0.0.1:${STUB_PORT}/"}}})

    def test_unknown_placeholder_stays_text(self):
        doc = {"row": '{"port":${OTHER}}'}
        self.assertEqual(canon.parse_embedded(doc, [["row"]], {"STUB_PORT": "1"}), doc)
