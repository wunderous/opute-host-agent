import concurrent.futures
import http.client
import socket
import unittest

from parity.agent import HttpStub


class HttpStubTests(unittest.TestCase):
    def test_parallel_endpoints_keep_independent_scripts_and_release_listeners(self):
        with concurrent.futures.ThreadPoolExecutor(max_workers=16) as pool:
            stubs = list(pool.map(
                lambda i: HttpStub([{'status': 503, 'body': str(i)}],
                                   {'status': 200, 'body': 'ready'}), range(32)))
        ports = [stub.port for stub in stubs]
        try:
            self.assertEqual(len(set(ports)), len(stubs))

            def query(item):
                i, stub = item
                client = http.client.HTTPConnection('127.0.0.1', stub.port, timeout=5)
                try:
                    client.request('GET', '/')
                    response = client.getresponse()
                    self.assertEqual((response.status, response.read()), (503, str(i).encode()))
                    client.request('GET', '/')
                    response = client.getresponse()
                    self.assertEqual((response.status, response.read()), (200, b'ready'))
                    self.assertEqual(stub.hits, 2)
                finally:
                    client.close()

            with concurrent.futures.ThreadPoolExecutor(max_workers=16) as pool:
                list(pool.map(query, enumerate(stubs)))
        finally:
            with concurrent.futures.ThreadPoolExecutor(max_workers=16) as pool:
                list(pool.map(lambda stub: stub.close(), stubs))
        for port in ports:
            with socket.socket() as probe:
                self.assertNotEqual(probe.connect_ex(('127.0.0.1', port)), 0)
