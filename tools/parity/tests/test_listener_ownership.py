import socket
import os
import select
import subprocess
import sys
import unittest
import tempfile
from pathlib import Path
from unittest.mock import patch
from types import SimpleNamespace

from parity import agent, runner


class ListenerOwnership(unittest.TestCase):
    def test_allocated_unbound_port_is_leased_across_processes(self):
        port = agent.free_port()
        code = ("from parity import agent\n"
                f"agent._PORT_RANGE = range({port}, {port + 1})\n"
                "agent._PORT_CURSOR[0] = 0\nprint(agent.free_port())\n")
        try:
            blocked = subprocess.run([sys.executable, "-c", code], capture_output=True,
                                     timeout=5, env={**os.environ, "PYTHONPATH": str(runner.TOOLS_DIR)})
            self.assertNotEqual(blocked.returncode, 0)
            self.assertIn(b"no free leased parity ports", blocked.stderr)
            agent.release_port(port)
            available = subprocess.run([sys.executable, "-c", code], capture_output=True,
                                       timeout=5, env={**os.environ, "PYTHONPATH": str(runner.TOOLS_DIR)})
            self.assertEqual(available.returncode, 0, available.stderr)
            self.assertEqual(int(available.stdout), port)
        finally:
            agent.release_port(port)

    def test_fixed_port_lease_blocks_another_process_until_release(self):
        code = ("from parity import runner\nfrom pathlib import Path\nfrom unittest.mock import patch\n"
                "import sys\nprint('started', flush=True)\n"
                "with patch('parity.runner.Path.home', return_value=Path(sys.argv[1])):\n"
                " with runner.exclusive_ports({'exclusive': True}):\n  print('acquired', flush=True)\n")
        child = None
        try:
            # Test the same flock protocol without competing with live
            # matrix runs for their actual default-port lease.
            with tempfile.TemporaryDirectory() as home, patch("parity.runner.Path.home", return_value=Path(home)):
                with runner.exclusive_ports({"exclusive": True}):
                    child = subprocess.Popen([sys.executable, "-c", code, home], stdout=subprocess.PIPE,
                                             env={**os.environ, "PYTHONPATH": str(runner.TOOLS_DIR)})
                    self.assertTrue(select.select([child.stdout], [], [], 5)[0])
                    self.assertEqual(child.stdout.readline(), b"started\n")
                    self.assertFalse(select.select([child.stdout], [], [], 0.1)[0])
                output, _ = child.communicate(timeout=5)
                self.assertEqual(output, b"acquired\n")
                self.assertEqual(child.returncode, 0)
        finally:
            if child is not None and child.poll() is None:
                child.terminate()
                child.communicate(timeout=5)

    def test_foreign_listener_cannot_mark_unstarted_process_ready(self):
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            listener.listen()
            port = listener.getsockname()[1]
            child = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(10)"])
            try:
                server = agent.Server.__new__(agent.Server)
                server.proc = child
                server.sandbox = SimpleNamespace(port=port)
                self.assertEqual(server.wait_ready(timeout=0.05), {"ready": False, "timedOut": True})
            finally:
                child.terminate()
                child.wait(timeout=5)
