"""MCP and HTTP requests as a client sends them; standard library only.

Used by the host side of the harness and, copied into the dev VM, by journey
steps that reach an agent listening inside the guest
(`python3 mcpwire.py < request.json`).
"""
from __future__ import annotations

import base64
import json
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from typing import Any

PROTOCOL_VERSION = "2026-07-28"
KEPT_HEADERS = ("content-type", "location", "cache-control", "www-authenticate", "mcp-session-id")


def decode_body(ctype: str, raw: bytes) -> Any:
    text = raw.decode(errors="replace")
    if ctype.startswith("text/event-stream"):
        events = [json.loads(line[5:]) for line in text.splitlines() if line.startswith("data:") and line[5:].strip()]
        return events[-1] if events else None
    try:
        return json.loads(text) if text.strip() else None
    except ValueError:
        return {"raw": text[:4000]}


def _send(request: urllib.request.Request, timeout: float) -> dict:
    started = time.monotonic()
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            raw, status, headers = response.read(), response.status, response.headers
    except urllib.error.HTTPError as error:
        raw, status, headers = error.read(), error.code, error.headers
    except OSError as error:
        return {"status": None, "error": type(getattr(error, "reason", error)).__name__,
                "ms": round((time.monotonic() - started) * 1000)}
    ctype = headers.get("Content-Type", "")
    return {"status": status, "contentType": ctype.split(";")[0].strip(), "body": decode_body(ctype, raw),
            "headers": {k: headers[k] for k in KEPT_HEADERS if headers.get(k) is not None},
            "ms": round((time.monotonic() - started) * 1000)}


class _NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *args: Any) -> None:
        return None


def mcp(port: int, token: str | None, method: str, params: dict | None = None, request_id: int = 1,
        client: str = "opute-e2e", timeout: float = 120, modern: bool = True, meta: dict | None = None) -> dict:
    """One JSON-RPC request. A modern request carries the 2026-07-28 metadata
    and headers; a legacy one (modern=False) is a bare JSON-RPC request."""
    params = dict(params or {})
    headers = {"Accept": "application/json, text/event-stream", "Content-Type": "application/json",
               "Origin": f"http://127.0.0.1:{port}"}
    if modern:
        params["_meta"] = {"io.modelcontextprotocol/protocolVersion": PROTOCOL_VERSION,
                           "io.modelcontextprotocol/clientCapabilities": {},
                           "io.modelcontextprotocol/clientInfo": {"name": client, "version": "1"}, **(meta or {})}
        headers.update({"MCP-Protocol-Version": PROTOCOL_VERSION, "Mcp-Method": method})
        if method == "tools/call":
            headers["Mcp-Name"] = params["name"]
        elif method.startswith("tasks/") and "taskId" in params:
            headers["Mcp-Name"] = params["taskId"]
    if token:
        headers["Authorization"] = f"Bearer {token}"
    body = {"jsonrpc": "2.0", "method": method, "id": request_id}
    if params:
        body["params"] = params
    request = urllib.request.Request(f"http://127.0.0.1:{port}/mcp", data=json.dumps(body).encode(),
                                     headers=headers, method="POST")
    return _send(request, timeout)


def http(port: int, method: str = "GET", path: str = "/", query: dict | None = None, form: dict | None = None,
         json_body: Any = None, basic: list | None = None, headers: dict | None = None, timeout: float = 60) -> dict:
    """One plain HTTP request; redirects are returned, not followed."""
    url = f"http://127.0.0.1:{port}{path}" + (f"?{urllib.parse.urlencode(query)}" if query else "")
    hdrs = dict(headers or {})
    data = None
    if form is not None:
        data = urllib.parse.urlencode(form).encode()
        hdrs["Content-Type"] = "application/x-www-form-urlencoded"
    elif json_body is not None:
        data = json.dumps(json_body).encode()
        hdrs["Content-Type"] = "application/json"
    if basic:
        hdrs["Authorization"] = "Basic " + base64.b64encode(f"{basic[0]}:{basic[1]}".encode()).decode()
    request = urllib.request.Request(url, data=data, headers=hdrs, method=method)
    opener = urllib.request.build_opener(_NoRedirect)
    started = time.monotonic()
    try:
        with opener.open(request, timeout=timeout) as response:
            raw, status, rh = response.read(), response.status, response.headers
    except urllib.error.HTTPError as error:
        raw, status, rh = error.read(), error.code, error.headers
    except OSError as error:
        return {"status": None, "error": type(getattr(error, "reason", error)).__name__,
                "ms": round((time.monotonic() - started) * 1000)}
    ctype = rh.get("Content-Type", "")
    return {"status": status, "contentType": ctype.split(";")[0].strip(), "body": decode_body(ctype, raw),
            "headers": {k: rh[k] for k in KEPT_HEADERS if rh.get(k) is not None},
            "ms": round((time.monotonic() - started) * 1000)}


def health(port: int, timeout: float = 5) -> dict:
    try:
        with urllib.request.urlopen(f"http://127.0.0.1:{port}/health", timeout=timeout) as response:
            return {"status": response.status, "body": json.loads(response.read())}
    except urllib.error.HTTPError as error:
        return {"status": error.code, "body": None}
    except (OSError, ValueError) as error:
        return {"status": None, "error": type(error).__name__}


if __name__ == "__main__":
    # In the guest: one request described on stdin, the response on stdout.
    spec = json.load(sys.stdin)
    kind = spec.pop("kind")
    out = {"mcp": mcp, "http": http, "health": health}[kind](**spec)
    json.dump(out, sys.stdout)
