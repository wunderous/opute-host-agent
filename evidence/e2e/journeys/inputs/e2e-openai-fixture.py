#!/usr/bin/env python3
"""A test fixture, not a model: an OpenAI-compatible endpoint with one fixed model.

  python3 e2e-openai-fixture.py PORT

GET /v1/models lists `e2e-fixture`; POST /v1/chat/completions answers with a
fixed completion (streamed as server-sent events when "stream" is true). The
e2e-3 serving journey runs it as a serving assignment's process, because a real
runtime and weights need downloads beyond test fixtures.
"""
import http.server
import json
import sys
import time

MODEL = "e2e-fixture"
TEXT = "e2e fixture completion"


class Handler(http.server.BaseHTTPRequestHandler):
    def _json(self, code, body):
        data = json.dumps(body).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        if self.path.rstrip("/") in ("/v1/models", "/models"):
            return self._json(200, {"object": "list", "data": [{"id": MODEL, "object": "model", "owned_by": "e2e"}]})
        if self.path in ("/", "/health"):
            return self._json(200, {"status": "ok"})
        return self._json(404, {"error": "not found"})

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers.get("Content-Length") or 0)) or b"{}")
        if self.path.rstrip("/") not in ("/v1/chat/completions", "/chat/completions"):
            return self._json(404, {"error": "not found"})
        created = int(time.time())
        if body.get("stream"):
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.end_headers()
            chunk = {"id": "e2e-1", "object": "chat.completion.chunk", "created": created, "model": MODEL,
                     "choices": [{"index": 0, "delta": {"role": "assistant", "content": TEXT}, "finish_reason": None}]}
            done = {**chunk, "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]}
            for item in (chunk, done):
                self.wfile.write(f"data: {json.dumps(item)}\n\n".encode())
            self.wfile.write(b"data: [DONE]\n\n")
            return
        return self._json(200, {"id": "e2e-1", "object": "chat.completion", "created": created, "model": MODEL,
                                "choices": [{"index": 0, "message": {"role": "assistant", "content": TEXT},
                                             "finish_reason": "stop"}],
                                "usage": {"prompt_tokens": 1, "completion_tokens": 3, "total_tokens": 4}})

    def log_message(self, *args):
        pass


http.server.ThreadingHTTPServer(("127.0.0.1", int(sys.argv[1])), Handler).serve_forever()
