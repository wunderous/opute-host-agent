import copy
import importlib.util
import json
import unittest

from parity import soak, t2

_spec = importlib.util.spec_from_file_location("soak_guest", t2.GUEST_DIR / "soak.py")
guest = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(guest)

UUID = "0607bbc8-2421-448b-86fa-e67f06296ae3"
THRESHOLDS = {"latency": {"maxRatio": {"p50": 2.0, "p95": 2.0, "p99": 3.0}, "slackMs": 5},
              "growth": {"rssKb": {"slopeSlackPerHour": 1000}, "fds": {"slopeSlackPerHour": 10},
                         "threads": {"slopeSlackPerHour": 5},
                         "stateBytes": {"maxRatio": 2.0, "slopeSlackPerHour": 100000}},
              "downSampleSlack": 1}


def result(**changes):
    side = {"epochEndSlopePerHour": 0.0}
    growth = {"rssKb": side, "fds": side, "threads": side, "stateBytes": {"slopePerHour": 10.0}, "downSamples": 0}
    base = {"iterations": {"a": 10, "b": 10}, "classMismatches": {"count": 0},
            "classes": {"a": {"host_info": {"ok": 10}}, "b": {"host_info": {"ok": 10}}},
            "restarts": {"a": [{"kind": "sigterm", "started": True, "ready": True}],
                         "b": [{"kind": "sigterm", "started": True, "ready": True}]},
            "latency": {"a": {"host_info": {"call": {"p50": 10, "p95": 20, "p99": 30}}},
                        "b": {"host_info": {"call": {"p50": 12, "p95": 25, "p99": 40}}}},
            "growth": {"a": copy.deepcopy(growth), "b": copy.deepcopy(growth)},
            "journal": {"a": {"errors": 2}, "b": {"errors": 2}},
            "finalState": {"diffCount": 0, "violations": {"a": [], "b": []}}}
    base.update(changes)
    return base


class ScheduleTest(unittest.TestCase):
    def test_seeded_schedule_is_deterministic(self):
        self.assertEqual(guest.schedule(7, 200), guest.schedule(7, 200))
        self.assertNotEqual(guest.schedule(7, 200), guest.schedule(8, 200))
        self.assertEqual(set(guest.schedule(7, 2000)), {name for name, _ in guest.OPS})


class StatisticsTest(unittest.TestCase):
    def test_percentile_interpolates(self):
        self.assertEqual(soak.percentile([1, 2, 3, 4], 0.5), 2.5)
        self.assertIsNone(soak.percentile([], 0.5))

    def test_slope_per_hour(self):
        self.assertEqual(soak.slope_per_hour([(0, 0), (1800, 5), (3600, 10)]), 10.0)
        self.assertIsNone(soak.slope_per_hour([(0, 0), (1, 1)]))

    def test_growth_splits_epochs_at_restarts_and_counts_down_samples(self):
        samples = [{"t": t, "rssKb": 100 + t, "fds": 10, "threads": 4, "stateBytes": 1000} for t in range(0, 10)]
        samples.append({"t": 10, "down": True, "stateBytes": 1000})
        samples += [{"t": t, "rssKb": 100 + t, "fds": 10, "threads": 4, "stateBytes": 1000} for t in range(11, 20)]
        out = soak.growth(samples, [{"event": "restart", "t": 10}])
        self.assertEqual(out["epochs"], 2)
        self.assertEqual(out["downSamples"], 1)
        self.assertEqual(out["rssKb"]["max"], 119)


class FinalStateTest(unittest.TestCase):
    def test_align_rows_aliases_only_random_ids(self):
        databases = {"state/state.db": {"tables": {
            "capability_invocations": {UUID: {"_rowid": 2, "invocation_id": UUID},
                                       "ab" * 16: {"_rowid": 1, "invocation_id": "ab" * 16}},
            "active_capabilities": {"kubernetes": {"_rowid": 1, "capability": "kubernetes"}}}}}
        variables = {}
        soak.align_rows(databases, variables)
        tables = databases["state/state.db"]["tables"]
        self.assertEqual([r["invocation_id"] for r in tables["capability_invocations"]], ["ab" * 16, UUID])
        self.assertEqual(sorted(variables.values()), sorted([UUID, "ab" * 16]))
        self.assertNotIn("kubernetes", variables.values())
        self.assertNotIn("_rowid", tables["capability_invocations"][0])

    def test_live_readings_are_masked_in_host_info_rows_and_plans(self):
        system = {"cpuLoad1m": 0.1, "cpuCount": 2}
        doc = {"sqliteRows": {"state/state.db": {"tables": {
            "capability_invocations": [{"operation_id": "get_host_info",
                                        "observation_json": {"$json": {"structured": {"system": dict(system)}}}},
                                       {"operation_id": "inspect_host_file",
                                        "observation_json": {"$json": {"structured": {"system": dict(system)}}}}],
            "plan_runs": {"r": {"state_json": {"$json": {"outputs": {"host": {"system": dict(system)}},
                                                         "nodes": {"host": {"output": {"system": dict(system)}}}}}}}}}}}
        self.assertEqual(soak.mask_live_readings(doc), 3)
        rows = doc["sqliteRows"]["state/state.db"]["tables"]
        self.assertEqual(rows["capability_invocations"][0]["observation_json"]["$json"]["structured"]["system"],
                         soak.LIVE)
        self.assertEqual(rows["capability_invocations"][1]["observation_json"]["$json"]["structured"]["system"], system)
        state = rows["plan_runs"]["r"]["state_json"]["$json"]
        self.assertEqual(state["outputs"]["host"]["system"], soak.LIVE)
        self.assertEqual(state["nodes"]["host"]["output"]["system"], soak.LIVE)

    def test_reservation_masks_skip_control(self):
        rows = [{"binding_json": json.dumps({"reservationId": "control"})},
                {"binding_json": json.dumps({"reservationId": "host-reservation-17-1"})}]
        doc = {"sqliteRows": {"state/state.db": {"tables": {"capability_invocations": rows}}}}
        masks = soak.reservation_masks(doc)
        self.assertEqual([m["path"][4] for m in masks], [1])


class RevisionPairTest(unittest.TestCase):
    G1, R1, G2, R2 = ("sha256:" + c * 64 for c in "1234")

    def test_unpinned_pairs_must_be_one_to_one(self):
        a = {"rows": [{"catalog_revision": self.G1}, {"catalog_revision": self.G1}, {"catalog_revision": self.G2}]}
        b = {"rows": [{"catalog_revision": self.R1}, {"catalog_revision": self.R1}, {"catalog_revision": self.R2}]}
        pairs, problems = soak.pair_reload_revisions(a, b)
        self.assertEqual(problems, [])
        self.assertEqual(len(pairs), 2)
        self.assertEqual(a, b)
        b = {"rows": [{"catalog_revision": self.R1}, {"catalog_revision": self.R2}]}
        _, problems = soak.pair_reload_revisions({"rows": [{"catalog_revision": self.G1}] * 2}, b)
        self.assertTrue(problems)

    def test_pinned_go_revision_is_never_paired(self):
        manifest = json.loads((t2.TOOLS_DIR.parent.parent / "parity-manifest.json").read_text())
        pinned = manifest["catalogRevisions"]["standalone"]["go"]
        _, problems = soak.pair_reload_revisions({"catalogRevision": pinned}, {"catalogRevision": self.R1})
        self.assertTrue(problems)


class ChecksTest(unittest.TestCase):
    def test_equal_rounds_pass(self):
        self.assertTrue(all(c["pass"] for c in soak.checks(result(), THRESHOLDS).values()))

    def test_each_check_fails_closed(self):
        failing = {
            "classesEqual": {"classMismatches": {"count": 1}},
            "sameIterations": {"iterations": {"a": 10, "b": 9}},
            "restartsEqual": {"restarts": {"a": [{"kind": "sigterm", "started": True, "ready": True}],
                                           "b": [{"kind": "sigterm", "started": True, "ready": False}]}},
            "latencyWithinBounds": {"latency": {"a": {"x": {"call": {"p50": 10}}}, "b": {"x": {"call": {"p50": 26}}}}},
            "journalErrors": {"journal": {"a": {"errors": 0}, "b": {"errors": 1}}},
            "finalStateParity": {"finalState": {"diffCount": 1, "violations": {"a": [], "b": []}}},
        }
        for name, change in failing.items():
            with self.subTest(name):
                self.assertFalse(soak.checks(result(**change), THRESHOLDS)[name]["pass"])

    def test_growth_beyond_slack_fails(self):
        r = result()
        r["growth"]["b"]["fds"] = {"epochEndSlopePerHour": 50.0}
        self.assertFalse(soak.checks(r, THRESHOLDS)["growthWithinBounds"]["pass"])
        r = result()
        r["growth"]["b"]["downSamples"] = 2
        self.assertFalse(soak.checks(r, THRESHOLDS)["noDownSamplesOutsideRestarts"]["pass"])


class DeriveTest(unittest.TestCase):
    def test_thresholds_are_twice_the_go_go_spread_with_floors(self):
        derived = soak.derive_thresholds(result(), "evidence/m12b/shakedown-go-go")
        self.assertEqual(derived["latency"]["maxRatio"], {"p50": 2.4, "p95": 2.5, "p99": 3.0})
        self.assertEqual(derived["latency"]["slackMs"], 5.0)
        self.assertEqual(derived["growth"]["fds"]["slopeSlackPerHour"], 8)
        self.assertEqual(derived["downSampleSlack"], 1)
        self.assertTrue(all(c["pass"] for c in soak.checks(result(), derived).values()))


if __name__ == "__main__":
    unittest.main()
