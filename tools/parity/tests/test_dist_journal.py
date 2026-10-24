import unittest

from parity import dist

UNIT = "opute-bootstrap-mcp.service"
AGENT = "OPUTE_REMOTE_AGENT_ID is required; the host agent must be onboarded with one canonical id"
STARTED = f"Started {UNIT} - Opute standalone bootstrap MCP (temporary)."
EXITED = f"{UNIT}: Main process exited, code=exited, status=1/FAILURE"
FAILED = f"{UNIT}: Failed with result 'exit-code'."
RESTART = f"{UNIT}: Scheduled restart job, restart counter is at 1."


class CausalJournalTest(unittest.TestCase):
    def test_agent_line_recorded_after_exit_moves_before_it(self):
        recorded = [STARTED, EXITED, FAILED, AGENT, RESTART, STARTED, AGENT, EXITED]
        self.assertEqual(dist._causal_journal(recorded),
                         [STARTED, AGENT, EXITED, FAILED, RESTART, STARTED, AGENT, EXITED])

    def test_causal_order_is_unchanged(self):
        ordered = [STARTED, AGENT, EXITED, FAILED, RESTART]
        self.assertEqual(dist._causal_journal(ordered), ordered)

    def test_lines_never_cross_a_start(self):
        recorded = [STARTED, EXITED, STARTED, AGENT]
        self.assertEqual(dist._causal_journal(recorded), recorded)

    def test_a_missing_or_extra_line_still_differs(self):
        go = dist._causal_journal([STARTED, AGENT, EXITED])
        self.assertNotEqual(go, dist._causal_journal([STARTED, EXITED]))
        self.assertNotEqual(go, dist._causal_journal([STARTED, AGENT, AGENT, EXITED]))


if __name__ == "__main__":
    unittest.main()
