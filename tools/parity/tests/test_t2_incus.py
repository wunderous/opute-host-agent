"""T2 nested-Incus observation: random device names and the Incus projection."""
import unittest

from parity import t2


def _snap(units: dict, incus=None) -> dict:
    return {"files": {}, "system": {"running": True, "unitFiles": {}, "units": units},
            "user": {"running": False}, "linger": [], "firewall": {}, "incus": incus}


VETH = {"active": "active", "load": "loaded", "sub": "plugged"}


class StableDeviceNames(unittest.TestCase):
    def test_veth_and_tap_names_are_replaced_in_names_and_descriptions(self) -> None:
        snap = _snap({"sys-devices-virtual-net-vethab12cd34.device": VETH,
                      "sys-subsystem-net-devices-tap0f2513a2.device": VETH, "x.service": VETH})
        snap["system"]["show"] = {"sys-devices-virtual-net-vethab12cd34.device":
                                  {"Description": "/sys/devices/virtual/net/vethab12cd34"}}
        out = t2._stable_device_names(snap)["system"]
        self.assertEqual(sorted(out["units"]), ["sys-devices-virtual-net-veth#1.device",
                                                "sys-subsystem-net-devices-tap#0.device", "x.service"])
        self.assertEqual(out["show"]["sys-devices-virtual-net-veth#1.device"]["Description"],
                         "/sys/devices/virtual/net/veth#1")

    def test_observation_compares_normalized_snapshots_on_both_sides(self) -> None:
        # The same container device under a different random name in each
        # snapshot is no change; a regression compared the raw current one.
        scenario = {"steps": [{"as": "base", "snapshot": {}}, {"as": "after", "snapshot": {"baseline": "base"}}]}
        raw = {"steps": {"base": {"snapshot": _snap({"sys-devices-virtual-net-veth11111111.device": VETH})},
                         "after": {"snapshot": _snap({"sys-devices-virtual-net-veth22222222.device": VETH})}}}
        effects = t2.observation(raw, scenario)["steps"]["after"]
        self.assertEqual(effects["system"], {})


class IncusDelta(unittest.TestCase):
    def test_incus_kinds_are_diffed_and_absent_incus_adds_nothing(self) -> None:
        box = {"name": "box1", "status": "Running"}
        before = _snap({}, {"instances": {}, "profiles": {}})
        after = _snap({}, {"instances": {"box1": box}, "profiles": {}})
        self.assertEqual(t2.effects(before, after)["incus"], {"instances": {"added": {"box1": box}}})
        self.assertEqual(t2.effects(before, before)["incus"], {})
        self.assertNotIn("incus", t2.effects(_snap({}), _snap({})))


if __name__ == "__main__":
    unittest.main()
