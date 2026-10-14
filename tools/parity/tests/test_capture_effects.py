"""Effect tiers and T2 coverage in the inventory capture (parity/capture.py)."""

import unittest
from pathlib import Path

from parity import capture


class EffectTierTest(unittest.TestCase):
    def test_sudo_runs_are_recorded_under_the_elevated_command(self) -> None:
        self.assertEqual(capture._elevated({"cmd": "sudo", "argv": ["-n", "/usr/bin/systemctl", "start", "x"]}),
                         "systemctl")
        self.assertEqual(capture._elevated({"cmd": "sudo", "argv": ["-n", "apt-get", "update"]}), "apt-get")
        self.assertIsNone(capture._elevated({"cmd": "incus", "argv": ["list"]}))

    def test_tiers(self) -> None:
        src = Path(self.id())  # no Go files: only observed commands are inventoried
        capture._go_files, saved = (lambda *_a: []), capture._go_files
        try:
            doc = capture.capture_effects(src, [
                {"cmd": "sudo", "argv": ["-n", "apt-get", "update"]},
                {"cmd": "curl", "argv": ["-fsSL", "https://example.invalid/key.asc"]},
            ])
        finally:
            capture._go_files = saved
        items = {i["command"]: i for i in doc["items"]}
        # apt-get runs for real in a T2 scenario; curl too (virt stack install).
        self.assertEqual(items["apt-get"]["status"], "covered")
        self.assertEqual(items["apt-get"]["tier"], "T2-sandbox")
        self.assertTrue(items["apt-get"]["t2Scenarios"])
        self.assertEqual(items["sudo"]["tier"], "T2-sandbox")
        self.assertIn("sudo -n apt-get update", items["apt-get"]["observedInvocations"])

    def test_t2_coverage_counts_executing_scenarios_only(self) -> None:
        doc = capture.capture_t2()
        items = {i["tool"]: i for i in doc["items"]}
        self.assertNotIn("get_host_info", items)  # reads are not effects
        for name, item in items.items():
            for sid in item["t2Scenarios"]:
                self.assertNotIn("admission", sid, name)
        self.assertEqual(items["compact_wsl_disk"]["status"], "gap")
        self.assertEqual(items["compact_wsl_disk"]["owner"], "host")
        self.assertEqual(items["ensure_k3d"]["status"], "inert")
        self.assertEqual(items["install_incus_stack"]["status"], "covered")
        self.assertTrue(items["install_incus_stack"]["t2RejectedScenarios"])


if __name__ == "__main__":
    unittest.main()
