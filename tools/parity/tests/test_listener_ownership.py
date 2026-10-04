import socket
import os
import select
import subprocess
import sys
import unittest
from types import SimpleNamespace

from parity import agent, runner


class ListenerOwnership(unittest.TestCase):
    def test_fixed_port_lease_blocks_another_process_until_release(self):
        code = ("from parity import runner\nprint('started', flush=True)\n"
                "with runner.exclusive_ports({'exclusive': True}):\n print('acquired', flush=True)\n")
        child = None
        try:
            with runner.exclusive_ports({"exclusive": True}):
                child = subprocess.Popen([sys.executable, "-c", code], stdout=subprocess.PIPE,
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
                child.wait(timeout=5)

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
