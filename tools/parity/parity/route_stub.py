"""A scripted HTTP endpoint that answers by route, for runtime APIs the agent
probes and drives (Ollama's /api/*, an OpenAI-compatible /v1/*).

Unlike HttpStub, which answers hits in order, a route stub picks the first
rule that matches the request, so the order of an agent's calls may vary
while each call still gets the same answer on both sides. A rule:

    {"method": "POST", "path": "/api/show", "bodyContains": "\"opute/context-",
     "ifExists": "run/ollama-up", "unlessExists": "...",
     "status": 200, "json": {...} | "body": "text", "headers": {...},
     "chunks": ["data: ...\\n\\n", ...]}

"ifExists" / "unlessExists" are sandbox-relative markers, as in the shims,
so a recorded `systemctl --user enable --now` can bring the API up, and a
rule's "creates" touches markers (a model a generate call made resident).
"chunks" answers with chunked transfer encoding, one chunk per entry. An
unmatched request answers 404 {"error": "not found"}.

Every request is recorded: method, path, the body (parsed when it is JSON)
and the headers a relay or client is expected to set or strip.
"""

from __future__ import annotations

import http.server
import json
import threading
from pathlib import Path
from typing import Any

RECORDED_HEADERS = ("authorization", "content-type", "accept-encoding", "user-agent", "x-forwarded-for",
                    "connection", "x-relay-token", "proxy-authorization")


class RouteStub:
    def __init__(self, routes: list[dict], root: Path):
        self.records: list[dict] = []
        self._lock = threading.Lock()
        stub = self

        class Handler(http.server.BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def _body(self) -> bytes:
                if self.headers.get("Transfer-Encoding", "").lower() == "chunked":
                    data = b""
                    while True:
                        size = int(self.rfile.readline().strip() or b"0", 16)
                        if size == 0:
                            self.rfile.readline()
                            return data
                        data += self.rfile.read(size)
                        self.rfile.readline()
                length = int(self.headers.get("Content-Length") or 0)
                return self.rfile.read(length) if length else b""

            def _handle(self) -> None:
                raw = self._body()
                text = raw.decode(errors="replace")
                try:
                    body: Any = json.loads(text) if text else None
                except ValueError:
                    body = text
                headers = {k: self.headers[k] for k in RECORDED_HEADERS if self.headers.get(k) is not None}
                with stub._lock:
                    stub.records.append({"method": self.command, "path": self.path, "body": body,
                                         "headers": headers})
                rule = next((r for r in routes if _matches(r, self.command, self.path, text, root)), None)
                if rule is None:
                    rule = {"status": 404, "json": {"error": "not found"}}
                for relative in rule.get("creates", []):
                    # The runtime's own state change (a model made resident).
                    (root / relative).parent.mkdir(parents=True, exist_ok=True)
                    (root / relative).touch()
                self.send_response(rule.get("status", 200))
                for name, value in rule.get("headers", {}).items():
                    self.send_header(name, value)
                if "chunks" in rule:
                    self.send_header("Transfer-Encoding", "chunked")
                    self.end_headers()
                    for chunk in rule["chunks"]:
                        data = chunk.encode()
                        self.wfile.write(b"%x\r\n%s\r\n" % (len(data), data))
                        self.wfile.flush()
                    self.wfile.write(b"0\r\n\r\n")
                    return
                payload = (json.dumps(rule["json"]) if "json" in rule else rule.get("body", "")).encode()
                if "json" in rule and "content-type" not in {k.lower() for k in rule.get("headers", {})}:
                    self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(payload)))
                self.end_headers()
                self.wfile.write(payload)

            do_GET = do_POST = do_PUT = do_DELETE = do_HEAD = _handle  # noqa: N815 (stdlib names)

            def log_message(self, *_args: Any) -> None:
                pass

        self._server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self._server.daemon_threads = True
        # A TCP readiness probe connects and closes without a request.
        self._server.handle_error = lambda *_args: None
        self.port = self._server.server_address[1]
        self._thread = threading.Thread(target=self._server.serve_forever, daemon=True)
        self._thread.start()

    def requests(self, clear: bool = False) -> list[dict]:
        with self._lock:
            out = list(self.records)
            if clear:
                self.records.clear()
        return out

    def close(self) -> None:
        self._server.shutdown()
        self._server.server_close()
        self._thread.join(timeout=5)


def _matches(rule: dict, method: str, path: str, body: str, root: Path) -> bool:
    if rule.get("method", method) != method:
        return False
    if "path" in rule and rule["path"] != path.split("?", 1)[0]:
        return False
    if "bodyContains" in rule and rule["bodyContains"] not in body:
        return False
    if "ifExists" in rule and not (root / rule["ifExists"]).exists():
        return False
    if "unlessExists" in rule and (root / rule["unlessExists"]).exists():
        return False
    return True
