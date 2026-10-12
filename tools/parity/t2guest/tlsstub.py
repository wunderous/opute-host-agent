"""T2 in-guest HTTPS artifact stub.

  python3 tlsstub.py init DIR          make a throwaway CA and a 127.0.0.1 leaf in DIR
  python3 tlsstub.py serve DIR PORT    serve DIR/artifact for every GET until killed

The CA exists only inside one disposable twin; the agent trusts it via
SSL_CERT_FILE, exactly as the T1 harness does with its host-side stub.
Nothing here leaves the VM.
"""
from __future__ import annotations

import http.server
import os
import ssl
import subprocess
import sys


def init(root: str) -> None:
    os.makedirs(root, exist_ok=True)

    def run(*argv: str) -> None:
        subprocess.run(argv, check=True, capture_output=True, cwd=root)

    run("openssl", "req", "-x509", "-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:prime256v1", "-nodes",
        "-days", "2", "-subj", "/CN=opute-parity-t2-ca", "-keyout", "ca.key", "-out", "ca.pem")
    run("openssl", "req", "-newkey", "ec", "-pkeyopt", "ec_paramgen_curve:prime256v1", "-nodes",
        "-subj", "/CN=127.0.0.1", "-keyout", "leaf.key", "-out", "leaf.csr")
    with open(os.path.join(root, "leaf.ext"), "w") as f:
        f.write("subjectAltName=IP:127.0.0.1\nbasicConstraints=CA:FALSE\n")
    run("openssl", "x509", "-req", "-in", "leaf.csr", "-CA", "ca.pem", "-CAkey", "ca.key", "-CAcreateserial",
        "-days", "2", "-extfile", "leaf.ext", "-out", "leaf.pem")
    for name in os.listdir(root):
        os.chmod(os.path.join(root, name), 0o644)


def serve(root: str, port: int) -> None:
    with open(os.path.join(root, "artifact"), "rb") as f:
        body = f.read()

    class Handler(http.server.BaseHTTPRequestHandler):
        def do_GET(self) -> None:  # noqa: N802
            self.send_response(200)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *args) -> None:
            pass

    server = http.server.ThreadingHTTPServer(("127.0.0.1", port), Handler)
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.load_cert_chain(os.path.join(root, "leaf.pem"), os.path.join(root, "leaf.key"))
    server.socket = context.wrap_socket(server.socket, server_side=True)
    server.serve_forever()


if __name__ == "__main__":
    if sys.argv[1] == "init":
        init(sys.argv[2])
    else:
        serve(sys.argv[2], int(sys.argv[3]))
