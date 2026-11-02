#!/usr/bin/env python3
"""Provider wire corpus: one provider executable's MCP surface, recorded.

Starts the executable on a loopback port (OPUTE_PROVIDER_PORT), sends a fixed
corpus of HTTP and JSON-RPC requests straight to it (both protocol eras,
every method the go-sdk server table knows, malformed params, the transport
guards) and records each response: status, the headers a client can act on,
and the body (parsed JSON when it is JSON). Two recordings compare equal when
the provider's wire contract is the same.

  python3 provider_wire.py record <executable> <out.json> [--env K=V ...] [--calls calls.json]
                                 [--fake cloudflare|tailscale]

`--fake <api>` serves a stateful stand-in for that API
(provider-wire/fake_<api>.py), points the provider at it, and records every
request the provider sent it. Random references (`mesh-`, `node-` and
`ingress-<hex>`) are
masked in the recording.

`calls.json` lists provider-specific tool calls, `[[tool, arguments], ...]`,
each sent in both eras; an optional third element is merged into `_meta`.
  python3 provider_wire.py compare <left.json> <right.json>
"""
import http.client
import json
import os
import re
import socket
import subprocess
import sys
import time

MODERN = "2026-07-28"
LEGACY = "2025-06-18"
KEPT_HEADERS = ["Allow", "Cache-Control", "Content-Type", "Mcp-Session-Id", "Www-Authenticate",
                "X-Content-Type-Options"]


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def meta() -> dict:
    return {"io.modelcontextprotocol/protocolVersion": MODERN,
            "io.modelcontextprotocol/clientInfo": {"name": "opute-parity", "version": "1"},
            "io.modelcontextprotocol/clientCapabilities": {}}


def rpc(method, params=None, *, modern=True, rid=1, name=None, headers=None, drop=None, raw=None):
    """A JSON-RPC POST to /mcp as the step's request."""
    if raw is None:
        p = dict(params or {})
        if modern and params is not False:
            p.setdefault("_meta", meta())
        env = {"jsonrpc": "2.0", "method": method}
        if params is not False:
            env["params"] = p
        if rid is not None:
            env["id"] = rid
        body = json.dumps(env).encode()
    else:
        body = raw if isinstance(raw, bytes) else raw.encode()
    h = {"Content-Type": "application/json", "Accept": "application/json, text/event-stream"}
    if modern:
        h["MCP-Protocol-Version"] = MODERN
        h["Mcp-Method"] = method
        if name:
            h["Mcp-Name"] = name
    else:
        h["MCP-Protocol-Version"] = LEGACY
    h.update(headers or {})
    for k in drop or []:
        h.pop(k, None)
    return {"method": "POST", "path": "/mcp", "headers": h, "body": body}


def corpus(tools: list[str], calls: list) -> list[tuple[str, dict]]:
    steps = []
    add = lambda name, req: steps.append((name, req))
    # HTTP surface and guards.
    for m in ["GET", "HEAD", "PUT", "DELETE", "PATCH", "OPTIONS"]:
        add(f"http {m} /mcp", {"method": m, "path": "/mcp", "headers": {}, "body": b""})
    add("http GET /", {"method": "GET", "path": "/", "headers": {}, "body": b""})
    add("http GET /health", {"method": "GET", "path": "/health", "headers": {}, "body": b""})
    add("post / (any path serves)", dict(rpc("ping"), path="/"))
    add("post /other", dict(rpc("ping"), path="/other"))
    add("bad content type", rpc("ping", headers={"Content-Type": "text/plain"}))
    add("no content type", rpc("ping", drop=["Content-Type"]))
    add("accept json only", rpc("ping", headers={"Accept": "application/json"}))
    add("no accept", rpc("ping", drop=["Accept"]))
    add("accept star", rpc("ping", headers={"Accept": "*/*"}))
    add("unsupported version header", rpc("ping", modern=False, headers={"MCP-Protocol-Version": "2024-01-01"}))
    add("future version header", rpc("ping", headers={"MCP-Protocol-Version": "2099-01-01"}))
    add("non-loopback host", rpc("ping", headers={"Host": "example.com"}))
    add("empty body", rpc("ping", raw=b""))
    add("invalid json", rpc("ping", raw=b"{"))
    add("json array", rpc("ping", raw=b"[]"))
    add("batch", rpc("ping", raw=json.dumps([{"jsonrpc": "2.0", "id": 1, "method": "ping"}])))
    add("no jsonrpc", rpc("ping", raw=json.dumps({"id": 1, "method": "ping"})))
    add("bool id", rpc("ping", raw=json.dumps({"jsonrpc": "2.0", "id": True, "method": "ping"})))
    add("string id", rpc("ping", rid="abc"))
    for i, body in enumerate(['{', 'x', '{"a"', '{}x', 'nul', '"str"', '1', '  ', '[1]',
                              '{"jsonrpc":"2.0","id":1,"method":"ping"} {}', '[{"jsonrpc":"2.0","id":1,"method":"ping"}',
                              '{"jsonrpc":"2.0","id":1,"method":"ping",}', '{"jsonrpc":"2.0","id":1,"method":5}',
                              '{"jsonrpc":"2.0","id":1.5,"method":"tools/list"}', '{"jsonrpc":"2.0","id":-3,"method":"tools/list"}',
                              '{"jsonrpc":"2.0","id":1e3,"method":"tools/list"}', '{"jsonrpc":"2.0","id":null,"method":"tools/list"}',
                              '{"jsonrpc":"2.0","id":{},"method":"tools/list"}', '{"jsonrpc":"2.0","id":1,"method":"tools/list","params":5}',
                              '{"jsonrpc":"2.0","id":1,"result":{}}', '{"jsonrpc":"2.0","method":""}']):
        add(f"malformed {i} {body[:40]}", rpc("ping", modern=False, raw=body))
    for version in ("2025-03-26", "2024-11-05"):
        one = json.dumps([{"jsonrpc": "2.0", "id": 1, "method": "tools/list"}])
        add(f"batch of one {version}", rpc("ping", modern=False, raw=one, headers={"MCP-Protocol-Version": version}))
    add("oversize", rpc("ping", raw=b'{"jsonrpc":"2.0","id":1,"method":"ping","params":{"x":"' + b"a" * (4 << 20) + b'"}}'))
    # Every server method, legacy and modern.
    methods = ["initialize", "ping", "tools/list", "prompts/list", "prompts/get", "resources/list",
               "resources/templates/list", "resources/read", "resources/subscribe", "resources/unsubscribe",
               "completion/complete", "logging/setLevel", "server/discover", "subscriptions/listen",
               "tasks/get", "tasks/list", "tasks/cancel", "tasks/result", "sampling/createMessage",
               "roots/list", "elicitation/create", "no/such/method"]
    for modern in (False, True):
        era = "modern" if modern else "legacy"
        for m in methods:
            params = {}
            if m == "initialize":
                params = {"protocolVersion": LEGACY, "capabilities": {}, "clientInfo": {"name": "p", "version": "1"}}
            add(f"{era} {m}", rpc(m, params, modern=modern))
            add(f"{era} {m} no params", rpc(m, False, modern=modern))
            add(f"{era} {m} array params", rpc(m, modern=modern, raw=json.dumps(
                {"jsonrpc": "2.0", "id": 1, "method": m, "params": []}),
                headers={"Mcp-Method": m} if modern else None))
        add(f"{era} prompts/get named", rpc("prompts/get", {"name": "x"}, modern=modern))
        add(f"{era} resources/read uri", rpc("resources/read", {"uri": "file:///x"}, modern=modern))
        add(f"{era} completion ref", rpc("completion/complete", {"ref": {"type": "ref/prompt", "name": "x"},
                                                                 "argument": {"name": "a", "value": "b"}}, modern=modern))
        add(f"{era} setLevel debug", rpc("logging/setLevel", {"level": "debug"}, modern=modern))
        add(f"{era} setLevel bogus", rpc("logging/setLevel", {"level": "bogus"}, modern=modern))
        add(f"{era} tools/list cursor bad", rpc("tools/list", {"cursor": "bogus"}, modern=modern))
        add(f"{era} notification initialized", rpc("notifications/initialized", {}, modern=modern, rid=None))
        add(f"{era} notification cancelled", rpc("notifications/cancelled", {"requestId": 1}, modern=modern, rid=None))
        add(f"{era} notification unknown", rpc("notifications/bogus", {}, modern=modern, rid=None))
        add(f"{era} ping without id", rpc("ping", {}, modern=modern, rid=None))
        # Tools.
        add(f"{era} call unknown tool", rpc("tools/call", {"name": "no.such.tool", "arguments": {}}, modern=modern,
                                            name="no.such.tool"))
        add(f"{era} call no name", rpc("tools/call", {"arguments": {}}, modern=modern))
        for tool in tools:
            add(f"{era} call {tool}", rpc("tools/call", {"name": tool, "arguments": {}}, modern=modern, name=tool))
            add(f"{era} call {tool} no args", rpc("tools/call", {"name": tool}, modern=modern, name=tool))
            add(f"{era} call {tool} null args", rpc("tools/call", {"name": tool, "arguments": None}, modern=modern, name=tool))
            add(f"{era} call {tool} array args", rpc("tools/call", {"name": tool, "arguments": []}, modern=modern, name=tool))
            add(f"{era} call {tool} string args", rpc("tools/call", {"name": tool, "arguments": "x"}, modern=modern, name=tool))
    for name, args, *extra in calls:
        # A third element is merged into the call's _meta (a delegation).
        extra_meta = extra[0] if extra else {}
        add(f"call {name} {json.dumps(args)[:60]}",
            rpc("tools/call", {"name": name, "arguments": args, "_meta": {**meta(), **extra_meta}}, name=name))
        add(f"legacy call {name} {json.dumps(args)[:60]}",
            rpc("tools/call", {"name": name, "arguments": args, **({"_meta": extra_meta} if extra_meta else {})}, modern=False))
    add("modern resources/read named", rpc("resources/read", {"uri": "file:///x"}, name="file:///x"))
    add("modern resources/read misnamed", rpc("resources/read", {"uri": "file:///x"}, name="file:///y"))
    # Modern header checks.
    add("modern missing Mcp-Method", rpc("ping", drop=["Mcp-Method"]))
    add("modern wrong Mcp-Method", rpc("ping", headers={"Mcp-Method": "tools/list"}))
    add("modern meta version mismatch", rpc("ping", {"_meta": dict(meta(), **{"io.modelcontextprotocol/protocolVersion": LEGACY})}))
    add("modern no meta", rpc("ping", {"_meta": {}}))
    add("modern call missing Mcp-Name", rpc("tools/call", {"name": tools[0], "arguments": {}}))
    add("modern call wrong Mcp-Name", rpc("tools/call", {"name": tools[0], "arguments": {}}, name="other"))
    add("no version header", rpc("ping", modern=False, drop=["MCP-Protocol-Version"]))
    return steps


def send(port: int, req: dict) -> dict:
    conn = http.client.HTTPConnection("127.0.0.1", port, timeout=30)
    headers = dict(req["headers"])
    try:
        conn.putrequest(req["method"], req["path"], skip_host="Host" in headers, skip_accept_encoding=True)
        for k, v in headers.items():
            conn.putheader(k, v)
        body = req["body"]
        if body or req["method"] in ("POST", "PUT", "PATCH"):
            conn.putheader("Content-Length", str(len(body)))
        conn.endheaders(body or None)
        resp = conn.getresponse()
        data = resp.read()
    except (OSError, http.client.HTTPException) as e:
        return {"error": type(e).__name__}
    finally:
        conn.close()
    out = {"status": resp.status,
           "headers": {k: resp.getheader(k) for k in KEPT_HEADERS if resp.getheader(k) is not None}}
    text = data.decode("utf-8", "replace")
    try:
        out["json"] = json.loads(text)
    except ValueError:
        out["text"] = text
    return out


def process_lines(stderr: bytes, port: int) -> list[str]:
    """The process's log lines without the `log` timestamp; a panic keeps its
    message and drops the goroutine dump, which is runtime detail."""
    lines, dump = [], False
    for line in stderr.decode().splitlines():
        if line.startswith("goroutine ") and line.endswith(":"):
            dump = True
        if dump:
            continue
        lines.append((line.split(" ", 2)[2] if line[:4].isdigit() else line).replace(str(port), "${PORT}"))
    while lines and lines[-1] == "":
        lines.pop()
    return lines


def record(executable: str, out: str, env_pairs: list[str], calls: list, fake: str | None = None) -> int:
    port = free_port()
    fake_state = None
    if fake:
        sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "provider-wire"))
        module = __import__(f"fake_{fake}")
        base, fake_state, _ = module.serve()
        env_pairs = module.env(base) + env_pairs
    env = {"PATH": os.environ.get("PATH", "/usr/bin:/bin"), "HOME": os.environ.get("HOME", "/tmp"),
           "OPUTE_PROVIDER_PORT": str(port)}
    env.update(pair.split("=", 1) for pair in env_pairs)

    def start():
        proc = subprocess.Popen([executable, "serve"], env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        for _ in range(100):
            try:
                socket.create_connection(("127.0.0.1", port), 0.2).close()
                break
            except OSError:
                time.sleep(0.05)
        return proc

    # A step that ends the process (a Go panic) is recorded with its exit
    # status and log, and the executable is started again for the next step.
    proc = start()
    stdout, stderr = b"", []
    try:
        tools = [t["name"] for t in send(port, rpc("tools/list"))["json"]["result"]["tools"]]
        steps = []
        for name, req in corpus(tools, calls):
            seen = len(fake_state.log) if fake_state is not None else 0
            step = {"as": name, "response": send(port, req)}
            if "error" in step["response"]:
                try:
                    proc.wait(timeout=2)
                except subprocess.TimeoutExpired:
                    pass
            if proc.poll() is not None:
                out_bytes, err_bytes = proc.communicate()
                stdout += out_bytes
                step["exit"] = {"code": proc.returncode, "log": process_lines(err_bytes, port)}
                proc = start()
            if fake_state is not None:
                # The requests this step caused, in the order they arrived.
                step["fakeRequests"] = fake_state.log[seen:]
            steps.append(step)
    finally:
        proc.terminate()
        out_bytes, err_bytes = proc.communicate(timeout=10)
    stdout += out_bytes
    doc = {"tools": tools, "steps": steps, "stdout": stdout.decode(), "stderr": process_lines(err_bytes, port)}
    text = json.dumps(doc, indent=1, sort_keys=True)
    text = re.sub(r"mesh-[0-9a-f]{36}", "mesh-${REF}", text)
    text = re.sub(r"\b(mesh|node|ingress)-[0-9a-f]{24}\b", r"\1-${REF}", text)
    with open(out, "w") as f:
        f.write(text + "\n")
    return 0


def compare(left: str, right: str) -> int:
    a, b = json.load(open(left)), json.load(open(right))
    diffs = 0
    if a["tools"] != b["tools"]:
        print("tools differ"); diffs += 1
    if a["stderr"] != b["stderr"] or a["stdout"] != b["stdout"]:
        print("process output differs:", a["stderr"], b["stderr"]); diffs += 1
    order_only = 0
    for x, y in zip(a["steps"], b["steps"], strict=True):
        fx, fy = x.pop("fakeRequests", None), y.pop("fakeRequests", None)
        if fx != fy:
            canon = lambda reqs: sorted(json.dumps(r, sort_keys=True) for r in reqs or [])
            if canon(fx) == canon(fy):
                # Go ranges over maps in random order; within one step the
                # same requests in another order are reported, not failed.
                order_only += 1
                print(f"~~ {x['as']}: the same {len(fx)} fake requests in another order")
            else:
                diffs += 1
                for i, (rx, ry) in enumerate(zip(fx or [], fy or [])):
                    if rx != ry:
                        print(f"== {x['as']} fake request {i}\n  L {json.dumps(rx, sort_keys=True)[:600]}\n  R {json.dumps(ry, sort_keys=True)[:600]}")
                        break
                if len(fx or []) != len(fy or []):
                    print(f"== {x['as']}: fake request counts differ: {len(fx or [])} vs {len(fy or [])}")
        if x != y:
            diffs += 1
            print(f"== {x['as']}\n  L {json.dumps(x['response'], sort_keys=True)[:600]}\n  R {json.dumps(y['response'], sort_keys=True)[:600]}")
    print(f"{len(a['steps'])} steps, {diffs} differ" + (f", {order_only} in another order only" if order_only else ""))
    return 1 if diffs else 0


if __name__ == "__main__":
    if sys.argv[1] == "record":
        opts = list(zip(sys.argv[4::2], sys.argv[5::2]))
        envs = [v for k, v in opts if k == "--env"]
        calls = [c for k, v in opts if k == "--calls" for c in json.load(open(v))]
        fake = next((v for k, v in opts if k == "--fake"), None)
        sys.exit(record(sys.argv[2], sys.argv[3], envs, calls, fake))
    sys.exit(compare(sys.argv[2], sys.argv[3]))
