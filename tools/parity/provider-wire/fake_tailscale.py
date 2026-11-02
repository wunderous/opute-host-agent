"""A stateful stand-in for the Tailscale v2 API and the Host Agent, for
provider wire corpora.

Serves devices and auth keys of one tailnet from memory, and at /mcp a
scripted Host Agent whose run_instance_command answers the provider's guest
scripts the way a healthy guest would (an enrollment registers its device
with the tailnet). Every request to either is logged. A Funnel to port 81
answers with an error result; a ping from `peer-b` fails, so the reverse
probe falls back to `tailscale ping`.

The bearer key steers the API's response shape:
  ts-api       the tailnet
  ts-badtype   a device whose id is a number
  ts-500       HTTP 500 for everything
  ts-garbage   a body that is not JSON
  anything else  HTTP 401
"""
import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

API_KEY = "ts-api"


class State:
    def __init__(self):
        self.lock = threading.Lock()
        self.devices = {
            "dev-peer": {"id": "dev-peer", "name": "peer-b.tail1234.ts.net", "hostname": "peer-b",
                         "addresses": ["100.70.0.2", "fd7a:115c:a1e0::2"], "os": "linux"},
            "dev-other": {"id": "dev-other", "name": "other.tail1234.ts.net", "hostname": "Other",
                          "addresses": ["fd7a:115c:a1e0::3", "100.70.0.3/32"]},
        }
        self.next = 0
        self.log = []


def serve():
    state = State()

    class Handler(BaseHTTPRequestHandler):
        def _any(self):
            raw = self.rfile.read(int(self.headers.get("Content-Length") or 0))
            auth = self.headers.get("Authorization") or ""
            with state.lock:
                state.log.append({"method": self.command, "path": self.path, "body": raw.decode("utf-8", "replace"),
                                  "authorization": auth, "contentType": self.headers.get("Content-Type"),
                                  "mcpMethod": self.headers.get("Mcp-Method"), "mcpName": self.headers.get("Mcp-Name")})
                if self.path == "/mcp":
                    status, payload = self.host_agent(raw)
                else:
                    status, payload = self.route(auth.removeprefix("Bearer "))
            data = payload.encode() if isinstance(payload, str) else json.dumps(payload).encode()
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def host_agent(self, raw):
            request = json.loads(raw or b"{}")
            method = request.get("method")
            params = request.get("params") or {}
            if method == "server/discover":
                result = {"supportedVersions": ["2026-07-28"], "capabilities": {"tools": {}},
                          "serverInfo": {"name": "fake-host-agent", "version": "1"}}
            elif method == "tools/call":
                result = self.instance_command(params.get("name"), params.get("arguments") or {})
            else:
                return 200, {"jsonrpc": "2.0", "id": request.get("id"), "error": {"code": -32601, "message": "Method not found"}}
            return 200, {"jsonrpc": "2.0", "id": request.get("id"), "result": result}

        def instance_command(self, name, args):
            uri, command = args.get("uri", ""), args.get("command", "")
            script = " ".join(args.get("args") or [])
            if "127.0.0.1:81" in script:
                return {"content": [{"type": "text", "text": f" funnel refused on {uri} "}], "isError": True}
            out, code = "", 0
            if name != "run_instance_command":
                out = "ok"
            elif command == "ping":
                code = 1 if uri.endswith(":peer-b") else 0
                out = "1 packets transmitted, %d received" % (1 - code)
            elif command == "tailscale":
                out = "pong from peer-b (100.70.0.2) via DERP(fra) in 30ms"
            elif "tailscale up --authkey" in script:
                host = script.split("--hostname=")[1].split()[0].strip("'")
                state.next += 1
                ip = f"100.70.1.{state.next}"
                state.devices[f"dev-{host}"] = {"id": f"dev-{host}", "name": f"{host}.tail1234.ts.net",
                                                "hostname": host, "addresses": [ip]}
                out = f"mesh_ip={ip}\n5: tailscale0    inet {ip}/32 scope global tailscale0\n"
            elif "agent_ready=true" in script or "running=false" in script:
                out = "agent_ready=true\nagent_version=1.80.2\n  tailscale commit: abc\n"
            elif "tailscale funnel --bg --yes" in script:
                out = "dns_name=node-a.tail1234.ts.net.\nlocal_target=http://127.0.0.1:80\n"
            content = {"exitCode": code, "stdout": out, "stderr": ""}
            return {"content": [{"type": "text", "text": json.dumps(content)}], "structuredContent": content}

        def route(self, key):
            if key == "ts-500":
                return 500, "tailnet unavailable"
            if key == "ts-garbage":
                return 200, "<html>"
            if key not in (API_KEY, "ts-badtype"):
                return 401, {"message": "API token invalid"}
            parts = [p for p in self.path.split("?")[0].split("/") if p]
            if parts[:3] == ["api", "v2", "tailnet"] and parts[4:] == ["devices"] and self.command == "GET":
                devices = list(state.devices.values())
                if key == "ts-badtype":
                    devices = devices + [{"id": 7, "hostname": "bad"}]
                return 200, {"devices": devices}
            if parts[:3] == ["api", "v2", "tailnet"] and parts[4:] == ["keys"] and self.command == "POST":
                state.next += 1
                return 200, {"id": f"k{state.next}", "key": f"tskey-auth-fake-{state.next}"}
            if parts[:3] == ["api", "v2", "device"] and self.command == "DELETE":
                if state.devices.pop(parts[3], None) is None:
                    return 404, {"message": "not found"}
                return 200, ""
            return 404, {"message": "no route"}

        do_GET = do_POST = do_DELETE = _any

        def log_message(self, *args):
            pass

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return f"http://127.0.0.1:{server.server_address[1]}/", state, server


def env(base: str) -> list[str]:
    """The live backend against the fake (API and Host Agent), with no env or
    auth key file and
    any request that is not to loopback sent to a dead proxy."""
    return ["OPUTE_TAILSCALE_BACKEND=live", f"TAILSCALE_API_BASE={base}", f"OPUTE_HOST_AGENT_ENDPOINT={base}mcp",
            "OPUTE_HOST_AGENT_BEARER_TOKEN=host-token", f"TAILSCALE_API_KEY={API_KEY}",
            "TAILSCALE_TAILNET=ex ample/net", "OPUTE_TAILSCALE_ENV_FILE=/nonexistent/tailscale.env",
            "TAILSCALE_AUTH_KEY_FILE=/nonexistent/tailscale.authkey", "HTTPS_PROXY=http://127.0.0.1:9",
            "HTTP_PROXY=http://127.0.0.1:9", "NO_PROXY="]
