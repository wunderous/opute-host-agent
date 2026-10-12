"""A scripted provider MCP server for lifecycle scenarios.

The stub speaks the provider side of MCP 2026-07-28 over Streamable HTTP,
as the pinned Go adapter expects it: `server/discover`, `tools/list`,
`tools/call` and the task methods, one JSON response per request. It serves
`opute.provider.get_install_manifest` from the scenario's manifest and every
manifest operation (and teardown operation) as a tool whose results the
scenario scripts in order. It records each request so both sides can be
compared on what the host agent asked of the provider, not only on what it
reported.
"""
from __future__ import annotations

import copy
import http.client
import http.server
import json
import socket
import threading
import time
from typing import Any

MANIFEST_TOOL = "opute.provider.get_install_manifest"
PROTOCOL_VERSION = "2026-07-28"


class _Server(http.server.ThreadingHTTPServer):
    daemon_threads = True

    def __init__(self, *args, **kwargs):
        self.connections: set = set()
        self.connections_lock = threading.Lock()
        super().__init__(*args, **kwargs)

    def process_request(self, request, client_address):
        with self.connections_lock:
            self.connections.add(request)
        super().process_request(request, client_address)

    def shutdown_request(self, request):
        with self.connections_lock:
            self.connections.discard(request)
        super().shutdown_request(request)

    def handle_error(self, request, client_address):
        # A held reply written after the host dropped the connection (the
        # host shut down mid-call) is expected; nothing to report.
        import sys
        if isinstance(sys.exc_info()[1], (BrokenPipeError, ConnectionResetError)):
            return
        super().handle_error(request, client_address)

    def sever(self) -> None:
        """Drop every live keep-alive connection, as a crashed process would."""
        with self.connections_lock:
            live = list(self.connections)
        for conn in live:
            try:
                conn.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass


class ProviderStub:
    """Serve `spec` on a kernel-allocated loopback port.

    spec:
      manifest: install manifest returned by get_install_manifest
      tools: {name: {"responses": [reply...], "default": reply}}
      discover: optional server/discover result
      listTools: optional tool name list overriding the manifest's
      tasks: {taskId: [tasks/get reply...]}
    A reply is {"result": value} | {"error": {...}} |
    {"status": int, "body": str} | {"hang": seconds, ...reply} |
    {"gate": name, ...reply} (held until a providerGate step releases it) |
    {"callback": {...}, ...reply}: before answering, call back into the host
    with the delegation this request carried (see _callback).
    host: {"url": ..., "token": ...} for callbacks.
    """

    def __init__(self, spec: dict[str, Any]):
        self.spec = copy.deepcopy(spec)
        self.records: list[dict[str, Any]] = []
        self._closed = False
        self._gates: dict[str, threading.Event] = {}
        self._last_delegation = ""
        self._reached: dict[str, threading.Event] = {}
        self._counts: dict[str, int] = {}
        self._lock = threading.Lock()
        stub = self

        class Handler(http.server.BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"
            disable_nagle_algorithm = True

            def log_message(self, *args):
                pass

            def do_POST(self):
                length = int(self.headers.get("Content-Length") or 0)
                body = self.rfile.read(length)
                status, reply = stub._answer(body, self.headers)
                self.send_response(status)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(reply)))
                self.end_headers()
                self.wfile.write(reply)

            def do_GET(self):
                # Streamable HTTP lets a client open a server-sent stream;
                # this provider offers none.
                self.send_response(405)
                self.send_header("Content-Length", "0")
                self.end_headers()

            def do_DELETE(self):
                self.send_response(405)
                self.send_header("Content-Length", "0")
                self.end_headers()

        self._server = _Server(("127.0.0.1", 0), Handler)
        self.port = self._server.server_address[1]
        self._thread = threading.Thread(
            target=lambda: self._server.serve_forever(poll_interval=0.01), daemon=True)
        self._thread.start()

    # -- scripting -----------------------------------------------------
    def _next(self, key: str, script: dict[str, Any] | None) -> dict[str, Any]:
        script = script or {}
        with self._lock:
            index = self._counts.get(key, 0)
            self._counts[key] = index + 1
        responses = script.get("responses", [])
        if index < len(responses):
            return responses[index]
        return script.get("default", {"result": {"content": [], "structuredContent": {}}})

    def _tool_names(self) -> list[str]:
        if "listTools" in self.spec:
            return list(self.spec["listTools"])
        manifest = self.spec.get("manifest") or {}
        names = [MANIFEST_TOOL]
        for service in manifest.get("services") or []:
            for operation in service.get("operations") or []:
                names.append(operation["id"])
        teardown = manifest.get("teardown")
        if teardown:
            names.append(teardown["id"])
        return names

    def _tool_descriptor(self, name: str) -> dict[str, Any]:
        manifest = self.spec.get("manifest") or {}
        operations = [op for service in manifest.get("services") or []
                      for op in service.get("operations") or []]
        if manifest.get("teardown"):
            operations.append(manifest["teardown"])
        for operation in operations:
            if operation.get("id") == name:
                return {"name": name, "inputSchema": operation.get("inputSchema") or {"type": "object"},
                        "outputSchema": operation.get("outputSchema") or {"type": "object"}}
        return {"name": name, "inputSchema": {"type": "object"}}

    def _answer(self, body: bytes, headers) -> tuple[int, bytes]:
        try:
            payload = json.loads(body)
        except ValueError:
            payload = None
        if not isinstance(payload, dict):
            return 400, b'{"error":"bad request"}'
        method = payload.get("method")
        params = payload.get("params") if isinstance(payload.get("params"), dict) else {}
        record: dict[str, Any] = {"method": method}
        if method == "tools/call":
            record["name"] = params.get("name")
            record["arguments"] = params.get("arguments")
        elif method in ("tasks/get", "tasks/cancel", "tasks/result"):
            record["taskId"] = params.get("taskId")
        meta = params.get("_meta")
        if isinstance(meta, dict):
            record["metaKeys"] = sorted(meta)
        record["authorization"] = headers.get("Authorization") is not None
        with self._lock:
            self.records.append(record)
        if "id" not in payload:
            return 202, b""
        if method == "server/discover":
            reply = {"result": self.spec.get("discover", {"supportedVersions": [PROTOCOL_VERSION]})}
        elif method == "tools/list":
            reply = {"result": {"tools": [self._tool_descriptor(n) for n in self._tool_names()]}}
        elif method == "tools/call":
            name = params.get("name")
            script = (self.spec.get("tools") or {}).get(name)
            if name == MANIFEST_TOOL and script is None:
                reply = {"result": {"content": [], "structuredContent": self.spec.get("manifest")}}
            elif script is None and name not in self._tool_names():
                reply = {"error": {"code": -32602, "message": f"unknown tool {name}"}}
            else:
                reply = self._next("call:" + str(name), script)
        elif method == "tasks/get":
            task_id = str(params.get("taskId"))
            reply = self._next("task:" + task_id, {"responses": (self.spec.get("tasks") or {}).get(task_id, []),
                                                   "default": {"error": {"code": -32602, "message": "unknown task"}}})
        elif method == "tasks/cancel":
            reply = {"result": {}}
        else:
            reply = {"error": {"code": -32601, "message": f"method not found: {method}"}}
        if "callback" in reply:
            outcome = self._callback(reply["callback"], meta if isinstance(meta, dict) else {})
            with self._lock:
                self.records.append({"callback": outcome})
            if reply.get("echoCallback"):
                reply = {"result": {"content": [], "structuredContent": {"callback": outcome}}}
        if "gate" in reply:
            # Block this reply until the scenario releases the gate, so a
            # call can be held in flight deterministically.
            self._reached_event(reply["gate"]).set()
            self._gate_event(reply["gate"]).wait(60)
        if "hang" in reply:
            threading.Event().wait(float(reply["hang"]))
        if "status" in reply:
            return int(reply["status"]), str(reply.get("body", "")).encode()
        envelope = {"jsonrpc": "2.0", "id": payload["id"]}
        if "error" in reply:
            envelope["error"] = reply["error"]
        else:
            envelope["result"] = reply.get("result")
        return 200, json.dumps(envelope).encode()

    def _callback(self, spec: dict[str, Any], meta: dict[str, Any]) -> dict[str, Any]:
        """One provider callback into the host's MCP endpoint.

        "token": "received" (default) forwards this request's delegation,
        "previous" replays the one an earlier call carried, "tampered" flips
        its signature, "malformed" sends a non-token, "none" omits it.
        """
        received = meta.get("io.opute/host-resource-delegation", "")
        mode = spec.get("token", "received")
        token = {"received": received, "previous": self._last_delegation,
                 "malformed": "not-a-delegation", "none": ""}.get(mode, received)
        if mode == "tampered" and isinstance(received, str) and "." in received:
            payload, signature = received.rsplit(".", 1)
            token = payload + "." + ("A" if signature[:1] != "A" else "B") + signature[1:]
        if isinstance(received, str) and received:
            self._last_delegation = received
        name = spec["tool"]
        outcome = self._host_call("tools/call", name, {"name": name, "arguments": spec.get("arguments", {})}, token)
        outcome["token"] = mode
        task_id = outcome["body"].get("result", {}).get("taskId") if isinstance(outcome["body"], dict) else None
        if spec.get("ack") and isinstance(task_id, str):
            # Acknowledge the callback task by polling it, as Go requires
            # before a delegated child starts; record its terminal state.
            deadline = time.monotonic() + 15
            while True:
                polled = self._host_call("tasks/get", task_id, {"taskId": task_id}, "")
                status = polled["body"].get("result", {}).get("status") if isinstance(polled["body"], dict) else None
                if status not in ("working", None) or time.monotonic() > deadline:
                    break
                time.sleep(0.05)
            outcome["acknowledged"] = polled
        return outcome

    def _host_call(self, method: str, name: str, params: dict[str, Any], token: str) -> dict[str, Any]:
        call_meta = {
            "io.modelcontextprotocol/protocolVersion": PROTOCOL_VERSION,
            "io.modelcontextprotocol/clientInfo": {"name": "parity-provider", "version": "1"},
            "io.modelcontextprotocol/clientCapabilities": {"extensions": {"io.modelcontextprotocol/tasks": {}}},
        }
        if token:
            call_meta["io.opute/host-resource-delegation"] = token
        body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method,
                           "params": {**params, "_meta": call_meta}}).encode()
        host = self.spec["host"]
        port = int(host["url"].split(":")[2].split("/")[0])
        conn = http.client.HTTPConnection("127.0.0.1", port, timeout=60)
        try:
            conn.request("POST", "/mcp", body, {
                "Content-Type": "application/json", "Accept": "application/json, text/event-stream",
                "MCP-Protocol-Version": PROTOCOL_VERSION, "Mcp-Method": method, "Mcp-Name": name,
                "Authorization": "Bearer " + host["token"]})
            response = conn.getresponse()
            raw = response.read()
            try:
                decoded = json.loads(raw)
            except ValueError:
                decoded = raw.decode(errors="replace")
            outcome = {"status": response.status, "body": decoded}
        finally:
            conn.close()
        return outcome

    def _gate_event(self, name: str) -> threading.Event:
        with self._lock:
            return self._gates.setdefault(name, threading.Event())

    def _reached_event(self, name: str) -> threading.Event:
        with self._lock:
            return self._reached.setdefault(name, threading.Event())

    def await_gate(self, name: str, timeout: float = 15) -> bool:
        return self._reached_event(name).wait(timeout)

    def release_gate(self, name: str) -> None:
        self._gate_event(name).set()

    # -- observation ---------------------------------------------------
    def requests(self, clear: bool = False) -> list[dict[str, Any]]:
        with self._lock:
            out = copy.deepcopy(self.records)
            if clear:
                self.records.clear()
        return out

    def close(self) -> None:
        if self._closed:
            return
        self._closed = True
        for gate in list(self._gates.values()):
            gate.set()
        self._server.shutdown()
        self._server.server_close()
        self._server.sever()
        self._thread.join(timeout=5)
