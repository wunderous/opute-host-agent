"""A stateful stand-in for the Cloudflare v4 API, for provider wire corpora.

Serves the endpoints the Cloudflare provider calls (tunnels, their ingress
and tokens, DNS records, WARP connectors, Zero Trust connectivity settings)
from memory, with deterministic ids, and logs every request it receives so
two provider executables can be compared on what they sent as well as on
what they answered. Seeded with the Platform's own tunnel and hostnames, so
the provider's refusals are exercised against the real shapes.

Hostnames that steer the fake into failure shapes:
  http500.*     DNS listing answers HTTP 500
  apifail.*     DNS listing answers 200 with success=false and an error
  garbage.*     DNS listing answers a body that is not JSON
  badtype.*     DNS listing answers a record whose id is a number
"""
import json
import threading
import urllib.parse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ACCOUNT, ZONE, TOKEN = "acct", "zone", "cf-test-token"


class State:
    def __init__(self):
        self.lock = threading.Lock()
        self.next = 0
        self.tunnels = {"platformtun": {"id": "platformtun", "name": "opute-platform-opute-io"}}
        self.ingress = {"platformtun": [{"hostname": "platform.opute.io", "service": "http://localhost:9090"},
                                        {"service": "http_status:404"}]}
        self.records = {
            "rec-taken": {"id": "rec-taken", "type": "CNAME", "name": "taken.example.test",
                          "content": "platformtun.cfargotunnel.com"},
            "rec-a": {"id": "rec-a", "type": "A", "name": "conflict.example.test", "content": "192.0.2.1"},
        }
        self.connectors = {}
        self.settings = {"offramp_warp_enabled": False, "icmp_proxy_enabled": True}
        self.log = []

    def new_id(self, prefix):
        self.next += 1
        return f"{prefix}{self.next:04d}"


def ok(result):
    return 200, {"success": True, "errors": [], "messages": [], "result": result}


def handle(state, method, path, query, body):
    parts = [p for p in path.split("/") if p][2:]  # drop client/v4
    if parts[:2] == ["accounts", ACCOUNT]:
        rest = parts[2:]
        if rest == ["cfd_tunnel"] and method == "GET":
            name = query.get("name", [""])[0]
            return ok([t for t in state.tunnels.values() if t["name"] == name])
        if rest == ["cfd_tunnel"] and method == "POST":
            tid = state.new_id("tun")
            state.tunnels[tid] = {"id": tid, "name": body.get("name", "")}
            return ok(state.tunnels[tid])
        if len(rest) >= 2 and rest[0] == "cfd_tunnel":
            tid = rest[1]
            if tid not in state.tunnels:
                return 404, {"success": False, "errors": [{"code": 1003, "message": "not found"}]}
            if len(rest) == 2 and method == "GET":
                return ok(state.tunnels[tid])
            if len(rest) == 2 and method == "DELETE":
                del state.tunnels[tid]
                state.ingress.pop(tid, None)
                return ok({"id": tid})
            if rest[2:] == ["configurations"] and method == "GET":
                return ok({"config": {"ingress": state.ingress.get(tid, [])}} if tid in state.ingress else None)
            if rest[2:] == ["configurations"] and method == "PUT":
                state.ingress[tid] = body["config"]["ingress"]
                return ok({"config": body["config"]})
            if rest[2:] == ["token"]:
                return ok(f"run-token-{tid}")
        if rest == ["warp_connector"] and method == "GET":
            return ok(list(state.connectors.values()) or None)
        if rest == ["warp_connector"] and method == "POST":
            cid = state.new_id("warp")
            state.connectors[cid] = {"id": cid, "name": body.get("name", ""), "status": "inactive"}
            return ok(state.connectors[cid])
        if len(rest) >= 2 and rest[0] == "warp_connector":
            cid = rest[1]
            if cid not in state.connectors:
                return 404, {"success": False, "errors": [{"code": 1003, "message": "not found"}]}
            if rest[2:] == ["token"]:
                return ok(f"warp-token-{cid}")
            if method == "DELETE":
                del state.connectors[cid]
                return ok(None)
        if rest == ["zerotrust", "connectivity_settings"]:
            if method == "PATCH":
                state.settings.update(body)
            return ok(dict(state.settings))
    if parts[:2] == ["zones", ZONE] and parts[2:3] == ["dns_records"]:
        rest = parts[3:]
        if not rest and method == "GET":
            name = query.get("name", [""])[0]
            if name.startswith("http500."):
                return 500, "upstream exploded"
            if name.startswith("apifail."):
                return 200, {"success": False, "errors": [{"code": 9109, "message": "Invalid access token"}]}
            if name.startswith("garbage."):
                return 200, "<html>not json"
            if name.startswith("badtype."):
                return ok([{"id": 7, "type": "CNAME", "name": name, "content": "x"}])
            return ok([r for r in state.records.values() if r["name"] == name])
        if not rest and method == "POST":
            rid = state.new_id("rec")
            state.records[rid] = {"id": rid, "type": body.get("type"), "name": body.get("name"),
                                  "content": body.get("content"), "proxied": body.get("proxied")}
            return ok(state.records[rid])
        if len(rest) == 1 and method == "DELETE":
            if state.records.pop(rest[0], None) is None:
                return 404, {"success": False, "errors": [{"code": 81044, "message": "Record does not exist."}]}
            return ok({"id": rest[0]})
    return 404, {"success": False, "errors": [{"code": 7003, "message": "No route for that URI"}]}


def serve():
    """Start the fake on a loopback port; returns (base_url, state, server)."""
    state = State()

    class Handler(BaseHTTPRequestHandler):
        def _any(self):
            url = urllib.parse.urlsplit(self.path)
            raw = self.rfile.read(int(self.headers.get("Content-Length") or 0))
            with state.lock:
                state.log.append({"method": self.command, "path": self.path, "body": raw.decode("utf-8", "replace"),
                                  "authorization": self.headers.get("Authorization"),
                                  "contentType": self.headers.get("Content-Type")})
                if self.headers.get("Authorization") != f"Bearer {TOKEN}":
                    status, payload = 403, {"success": False, "errors": [{"code": 10000, "message": "Authentication error"}]}
                else:
                    try:
                        body = json.loads(raw) if raw else {}
                    except ValueError:
                        body = {}
                    status, payload = handle(state, self.command, url.path, urllib.parse.parse_qs(url.query), body)
            data = payload.encode() if isinstance(payload, str) else json.dumps(payload).encode()
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        do_GET = do_POST = do_PUT = do_PATCH = do_DELETE = _any

        def log_message(self, *args):
            pass

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return f"http://127.0.0.1:{server.server_address[1]}/client/v4", state, server


def env(base: str) -> list[str]:
    """The provider's Cloudflare credentials against the fake. Any request
    that is not to loopback goes to a dead proxy, so no run reaches the real
    Cloudflare API (cleanupExternalResources hardcodes its host)."""
    return [f"CLOUDFLARE_API_BASE={base}", f"CLOUDFLARE_ACCOUNT_ID={ACCOUNT}", f"CLOUDFLARE_ZONE_ID={ZONE}",
            f"CLOUDFLARE_API_TOKEN={TOKEN}", "HTTPS_PROXY=http://127.0.0.1:9", "HTTP_PROXY=http://127.0.0.1:9",
            "NO_PROXY="]
