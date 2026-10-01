"""Generate the M2 wire corpus (scenarios/wire.json).

The corpus is generated, not hand-maintained, so every dimension is
enumerated in one place and its size is a property of the code:

    families                    what varies
    --------------------------  ------------------------------------------------
    wire.routing                path x HTTP method, no auth
    wire.transport              Content-Type x Accept, for an agent-handled and
                                an SDK-handled method
    wire.envelope               malformed / unusual JSON-RPC bodies and sizes
    wire.methods.legacy-{off,on} every MCP method x every header/_meta variant,
                                with the ADR 0011 flag off and on
    wire.auth.{standalone,platform}
                                Authorization x Host, against seeded tokens,
                                then revoke and reuse
    wire.origin.{standalone,platform,public-opt-in}
                                Origin x Host

Token *issuance* (/oauth/authorize, /oauth/token) is deliberately absent: it
is deferred pending an owner design decision, so the corpus seeds token rows
directly and exercises validation and revocation only.

`python -m parity corpus` rewrites scenarios/wire.json; the harness tests fail
when the committed file is stale.
"""

from __future__ import annotations

import base64
import hashlib
import json
from pathlib import Path
from typing import Any

from . import agent

VERSION = 1
OUT_FILE = Path(__file__).resolve().parent.parent / "scenarios" / "wire.json"

PROTOCOL = agent.PROTOCOL_VERSION
LEGACY_ENV = {"OPUTE_MCP_ALLOW_LEGACY_HANDSHAKE": "true"}
BEARER = "Bearer ${TOKEN}"
LOCAL_HOST = "127.0.0.1:${PORT}"

# Every method name the inventory knows, plus names a client might send.
METHODS = [
    "initialize", "notifications/initialized", "notifications/cancelled",
    "notifications/progress", "ping", "tools/list", "tools/call",
    "prompts/list", "prompts/get", "resources/list", "resources/read",
    "resources/templates/list", "resources/subscribe", "server/discover",
    "tasks/get", "tasks/list", "tasks/update", "tasks/cancel", "tasks/result",
    "completion/complete", "logging/setLevel", "sampling/createMessage",
    "roots/list", "elicitation/create", "subscriptions/listen",
    "no/such/method", "",
]

# Catalog content is M3's: a successful tools/list is compared by type only.
CATALOG_MASK_REASON = "catalog content is owned by the M3 catalog scenarios"
UNKNOWN_TOOL = "parity_no_such_tool"
# encodeCursor("abc") and encodeCursor("") from the Go SDK (gob, base64url).
VALID_CURSOR = "In8DAQEJcGFnZVRva2VuAf-AAAEBAQdMYXN0VUlEAQwAAAAI_4ABA2FiYwA="
EMPTY_CURSOR = "In8DAQEJcGFnZVRva2VuAf-AAAEBAQdMYXN0VUlEAQwAAAAD_4AA"
TASK_ID = "parity-no-such-task"


def _meta() -> dict:
    return agent.modern_meta()


def _params_for(method: str) -> dict:
    """Minimal well-formed params for a method (before _meta is added)."""
    if method == "tools/call":
        return {"name": UNKNOWN_TOOL, "arguments": {}}
    if method in ("tasks/get", "tasks/cancel", "tasks/result"):
        return {"taskId": TASK_ID}
    if method == "tasks/update":
        return {"taskId": TASK_ID, "inputResponses": {}}
    if method == "prompts/get":
        return {"name": "parity-no-such-prompt"}
    if method == "resources/read":
        return {"uri": "file:///parity"}
    if method == "initialize":
        return {"protocolVersion": "2025-06-18", "capabilities": {},
                "clientInfo": {"name": "parity", "version": "1"}}
    return {}


def _mcp_name(method: str) -> str | None:
    if method == "tools/call":
        return UNKNOWN_TOOL
    if method in ("tasks/get", "tasks/update", "tasks/cancel"):
        return TASK_ID
    return None


def _is_notification(method: str) -> bool:
    return method.startswith("notifications/")


def _envelope(method: str, params: Any, *, with_params: bool = True, rid: Any = 1) -> dict:
    env: dict[str, Any] = {"jsonrpc": "2.0", "method": method}
    if with_params:
        env["params"] = params
    if not _is_notification(method):
        env["id"] = rid
    return env


def _headers(method: str | None, *, modern: bool = True, auth: str | None = BEARER,
             name: str | None = None) -> dict:
    hdrs = {"Content-Type": "application/json", "Accept": "application/json, text/event-stream"}
    if modern and method is not None:
        hdrs["MCP-Protocol-Version"] = PROTOCOL
        hdrs["Mcp-Method"] = method
        if name:
            hdrs["Mcp-Name"] = name
    if auth:
        hdrs["Authorization"] = auth
    return hdrs


def _http(method: str, path: str, headers: dict | None = None, body: Any = None, **extra) -> dict:
    spec: dict[str, Any] = {"method": method, "path": path}
    if headers:
        spec["headers"] = headers
    if body is not None:
        spec["body"] = body if isinstance(body, str) else json.dumps(body, separators=(",", ":"))
    spec.update(extra)
    return {"http": spec}


def _scenario(sid: str, surface: str, owner: str, anchors: list[str], steps: list[dict],
              *, profile: str = "standalone", env: dict | None = None,
              argv: list[str] | None = None, masks: list[dict] | None = None,
              collect: list[str] | None = None, prelude: list[dict] | None = None) -> dict:
    serve: dict[str, Any] = {"argv": argv or (["serve", "--mode=platform"] if profile == "platform" else ["serve"]),
                             "profile": profile}
    if env:
        serve["env"] = env
    body = [*(prelude or []), {"as": "start", "serve": serve}, *steps, {"as": "stop", "stop": {}}]
    doc: dict[str, Any] = {"id": sid, "surface": surface, "owner": owner, "anchors": anchors,
                           "fixture": "incus-empty", "corpusVersion": VERSION, "steps": body}
    if collect:
        doc["collect"] = collect
    if masks:
        doc["compare"] = {"masks": masks}
    return doc


def _label(steps: list[dict], label: str, step: dict) -> None:
    if any(s["as"] == label for s in steps):
        raise ValueError(f"duplicate corpus label {label}")
    steps.append({"as": label, **step})


# --- wire.routing -------------------------------------------------------------

ROUTING_PATHS = [
    "/", "/health", "/health/", "/healthz", "/mcp", "/mcp/", "/MCP", "//mcp",
    "/./mcp", "/x/../mcp", "/mcp?x=1", "/%6Dcp", "/mcp%2F", "/health?verbose=1",
    "/.well-known/oauth-protected-resource",
    "/.well-known/oauth-protected-resource/mcp",
    "/.well-known/oauth-protected-resource/other",
    "/.well-known/oauth-authorization-server",
    "/.well-known/openid-configuration", "/oauth/revoke", "/oauth/", "/nope",
]
ROUTING_METHODS = ["GET", "HEAD", "POST", "PUT", "DELETE", "PATCH", "OPTIONS"]


def routing() -> dict:
    steps: list[dict] = []
    for path in ROUTING_PATHS:
        for method in ROUTING_METHODS:
            _label(steps, f"{method} {path}", _http(method, path))
    # Framing cases http.client will not produce.
    raw = {
        "noHost": "GET /health HTTP/1.1\r\nConnection: close\r\n\r\n",
        "http10NoHost": "GET /health HTTP/1.0\r\n\r\n",
        "absoluteUri": "GET http://127.0.0.1:${PORT}/health HTTP/1.1\r\nHost: 127.0.0.1:${PORT}\r\nConnection: close\r\n\r\n",
        "doubleHost": "GET /health HTTP/1.1\r\nHost: a\r\nHost: b\r\nConnection: close\r\n\r\n",
        "badVersion": "GET /health HTTP/9.9\r\nHost: x\r\nConnection: close\r\n\r\n",
        "asteriskOptions": "OPTIONS * HTTP/1.1\r\nHost: 127.0.0.1:${PORT}\r\nConnection: close\r\n\r\n",
        "lowercaseMethod": "get /health HTTP/1.1\r\nHost: 127.0.0.1:${PORT}\r\nConnection: close\r\n\r\n",
        "unknownMethod": "BREW /health HTTP/1.1\r\nHost: 127.0.0.1:${PORT}\r\nConnection: close\r\n\r\n",
    }
    for key, data in raw.items():
        _label(steps, f"raw {key}", {"raw": {"data": data}})
    return _scenario("wire.routing", "http", "transport",
                     ["internal/transport/http.go:NewHTTPServer mux", "net/http ServeMux"], steps)


# --- wire.transport -----------------------------------------------------------

CONTENT_TYPES = [None, "", "text/plain", "application/json", "application/json; charset=utf-8",
                 "Application/JSON", "application/json-seq", "application/x-www-form-urlencoded",
                 "application/json;charset", "application/json, text/plain"]
ACCEPTS = [None, "*/*", "application/json", "text/event-stream",
           "application/json, text/event-stream", "text/event-stream, application/json",
           "application/json;q=0.9, text/event-stream;q=0.1", "application/*, text/*",
           "APPLICATION/JSON,TEXT/EVENT-STREAM", "application/jsonx, text/event-streamx"]


def transport() -> dict:
    steps: list[dict] = []
    for method in ("server/discover", "prompts/list"):
        body = _envelope(method, {"_meta": _meta()})
        for ci, ctype in enumerate(CONTENT_TYPES):
            for ai, accept in enumerate(ACCEPTS):
                hdrs = _headers(method)
                hdrs.pop("Content-Type")
                hdrs.pop("Accept")
                if ctype is not None:
                    hdrs["Content-Type"] = ctype
                if accept is not None:
                    hdrs["Accept"] = accept
                _label(steps, f"{method} ct{ci} accept{ai}", _http("POST", "/mcp", hdrs, body))
    # Repeated Accept header lines are joined by the SDK, so send them raw.
    payload = json.dumps(_envelope("prompts/list", {"_meta": _meta()}), separators=(",", ":"))
    for key, accepts in {"split": ["application/json", "text/event-stream"],
                         "splitStar": ["text/html", "*/*"]}.items():
        lines = "".join(f"Accept: {a}\r\n" for a in accepts)
        data = (f"POST /mcp HTTP/1.1\r\nHost: {LOCAL_HOST}\r\nConnection: close\r\n"
                f"Content-Type: application/json\r\n{lines}"
                f"MCP-Protocol-Version: {PROTOCOL}\r\nMcp-Method: prompts/list\r\n"
                f"Authorization: {BEARER}\r\nContent-Length: {len(payload)}\r\n\r\n{payload}")
        _label(steps, f"raw accept {key}", {"raw": {"data": data}})
    return _scenario("wire.transport", "mcp-wire", "transport",
                     ["go-sdk mcp/streamable.go:serveStateless", "internal/transport/http.go:handleMCP"],
                     steps)


# --- wire.envelope ------------------------------------------------------------

def envelope() -> dict:
    steps: list[dict] = []
    meta = _meta()
    for method in ("server/discover", "prompts/list"):
        hdrs = _headers(method)
        good = {"_meta": meta}
        bodies: dict[str, Any] = {
            "empty": "", "space": " ", "null": "null", "true": "true", "number": "1",
            "string": '"x"', "emptyArray": "[]", "emptyObject": "{}",
            "batch1": [_envelope(method, good)],
            "batch2": [_envelope(method, good), _envelope(method, good, rid=2)],
            "noJsonrpc": {"id": 1, "method": method, "params": good},
            "jsonrpc1": {"jsonrpc": "1.0", "id": 1, "method": method, "params": good},
            "jsonrpcNumber": {"jsonrpc": 2, "id": 1, "method": method, "params": good},
            "methodNumber": {"jsonrpc": "2.0", "id": 1, "method": 7, "params": good},
            "methodNull": {"jsonrpc": "2.0", "id": 1, "method": None, "params": good},
            "noMethod": {"jsonrpc": "2.0", "id": 1, "params": good},
            "paramsArray": {"jsonrpc": "2.0", "id": 1, "method": method, "params": [good]},
            "paramsString": {"jsonrpc": "2.0", "id": 1, "method": method, "params": "x"},
            "extraField": {"jsonrpc": "2.0", "id": 1, "method": method, "params": good, "extra": True},
            "resultField": {"jsonrpc": "2.0", "id": 1, "result": {}},
            "errorField": {"jsonrpc": "2.0", "id": 1, "error": {"code": 1, "message": "x"}},
            "trailingGarbage": json.dumps(_envelope(method, good)) + " x",
            "twoObjects": json.dumps(_envelope(method, good)) + json.dumps(_envelope(method, good)),
            "bom": "﻿" + json.dumps(_envelope(method, good)),
            "escapedMethod": json.dumps(_envelope(method, good)).replace(
                '"method": "' + method + '"', '"method": "' + method.replace("/", "\\u002f") + '"'),
            "duplicateMethod": '{"jsonrpc":"2.0","id":1,"method":"no/such","method":"' + method
                               + '","params":' + json.dumps(good) + "}",
            "unterminated": json.dumps(_envelope(method, good))[:-1],
            "deepNesting": '{"jsonrpc":"2.0","id":1,"method":"' + method + '","params":{"_meta":'
                           + json.dumps(meta) + ',"x":' + "[" * 2000 + "]" * 2000 + "}}",
        }
        for rid_key, rid in {"id0": 0, "idNeg": -1, "idFloat": 1.5, "idStr": "a", "idEmptyStr": "",
                             "idNull": None, "idTrue": True, "idObj": {}, "idArr": [],
                             "idBig": 9007199254740993, "idExp": 1e3}.items():
            bodies[rid_key] = {"jsonrpc": "2.0", "id": rid, "method": method, "params": good}
        bodies["noId"] = {"jsonrpc": "2.0", "method": method, "params": good}
        for key, body in bodies.items():
            _label(steps, f"{method} {key}", _http("POST", "/mcp", hdrs, body))
        # Sizes around the SDK's 4 MiB cap. The filler lives inside params.
        template = ('{"jsonrpc":"2.0","id":1,"method":"' + method + '","params":{"_meta":'
                    + json.dumps(meta, separators=(",", ":")) + ',"pad":"${PAD}"}}')
        for size_key, size in {"4MiB-1": (4 << 20) - 1, "4MiB": 4 << 20, "4MiB+1": (4 << 20) + 1,
                               "8MiB": 8 << 20}.items():
            _label(steps, f"{method} size {size_key}",
                   _http("POST", "/mcp", hdrs, template, padTo=size))
    return _scenario("wire.envelope", "mcp-wire", "transport",
                     ["internal/transport/http.go:handleMCP envelope",
                      "go-sdk internal/jsonrpc2/wire.go"], steps)


# --- wire.methods.* -----------------------------------------------------------

def _rfc2047_b(value: str) -> str:
    return "=?UTF-8?B?" + base64.b64encode(value.encode()).decode() + "?="


def _rfc2047_q(value: str) -> str:
    return "=?utf-8?q?" + value.replace("=", "=3D").replace("/", "=2F").replace(" ", "_") + "?="


def _method_variants(method: str) -> dict[str, dict]:
    """Header/_meta variants for one method. Each value is an http step."""
    name = _mcp_name(method)
    base = _params_for(method)
    meta = _meta()

    def call(*, params: Any = "default", with_params: bool = True, headers: dict | None = None,
             drop: tuple[str, ...] = (), modern: bool = True) -> dict:
        p = {**base, "_meta": meta} if params == "default" else params
        hdrs = _headers(method, modern=modern, name=name)
        for key in drop:
            hdrs.pop(key, None)
        hdrs.update(headers or {})
        return _http("POST", "/mcp", hdrs, _envelope(method, p, with_params=with_params))

    wrong = "ping" if method != "ping" else "tools/list"
    out = {
        "valid": call(),
        "bare": call(params=dict(base), modern=False),
        "bareVersionHeader": call(params=dict(base), modern=False,
                                  headers={"MCP-Protocol-Version": "2025-06-18"}),
        "bareNoParams": call(with_params=False, modern=False),
        "missingMcpMethod": call(drop=("Mcp-Method",)),
        "wrongMcpMethod": call(headers={"Mcp-Method": wrong}),
        "paddedMcpMethod": call(headers={"Mcp-Method": "  " + method + "  "}),
        "rfc2047B": call(headers={"Mcp-Method": _rfc2047_b(method)}),
        "rfc2047Q": call(headers={"Mcp-Method": _rfc2047_q(method)}),
        "rfc2047BadCharset": call(headers={"Mcp-Method": "=?iso-8859-1?B?"
                                           + base64.b64encode(method.encode()).decode() + "?="}),
        "missingVersionHeader": call(drop=("MCP-Protocol-Version",)),
        "oldVersionHeader": call(headers={"MCP-Protocol-Version": "2025-06-18"}),
        "futureVersionHeader": call(headers={"MCP-Protocol-Version": "2099-01-01"}),
        "garbageVersionHeader": call(headers={"MCP-Protocol-Version": "garbage"}),
        "metaMissing": call(params=dict(base)),
        "metaNull": call(params={**base, "_meta": None}),
        "metaEmpty": call(params={**base, "_meta": {}}),
        "metaOldVersion": call(params={**base, "_meta": {**meta, "io.modelcontextprotocol/protocolVersion": "2025-06-18"}}),
        "metaVersionNumber": call(params={**base, "_meta": {**meta, "io.modelcontextprotocol/protocolVersion": 20260728}}),
        "metaNoClientInfo": call(params={**base, "_meta": {k: v for k, v in meta.items()
                                                            if k != "io.modelcontextprotocol/clientInfo"}}),
        "metaNoCapabilities": call(params={**base, "_meta": {k: v for k, v in meta.items()
                                                              if k != "io.modelcontextprotocol/clientCapabilities"}}),
        "metaNoTasksExtension": call(params={**base, "_meta": {**meta, "io.modelcontextprotocol/clientCapabilities": {}}}),
        "paramsNull": call(params=None),
        "paramsAbsent": call(with_params=False),
    }
    if name:
        out["missingMcpName"] = call(drop=("Mcp-Name",))
        out["wrongMcpName"] = call(headers={"Mcp-Name": name + "-other"})
        out["rfc2047McpName"] = call(headers={"Mcp-Name": _rfc2047_b(name)})
    if method in ("tasks/get", "tasks/update", "tasks/cancel"):
        out["emptyTaskId"] = call(params={**base, "taskId": " ", "_meta": meta})
        out["numericTaskId"] = call(params={**base, "taskId": 7, "_meta": meta})
    return out


def methods(legacy: bool) -> dict:
    steps: list[dict] = []
    masks: list[dict] = []
    for method in METHODS:
        for variant, step in _method_variants(method).items():
            label = f"{method or '<empty>'} {variant}"
            _label(steps, label, step)
            if method == "tools/list":
                masks.append({"path": ["steps", label, "body", "result", "tools"], "type": "array",
                              "reason": CATALOG_MASK_REASON, "optional": True})
    suffix = "on" if legacy else "off"
    return _scenario(f"wire.methods.legacy-{suffix}", "mcp-wire", "transport",
                     ["internal/transport/discover.go", "docs/adr/0011 legacy handshake bound",
                      "internal/hostmcp/server.go:HandleExtensionMethod"],
                     steps, env=LEGACY_ENV if legacy else None, masks=masks)


# --- wire.auth.* --------------------------------------------------------------

def _hash(token: str) -> str:
    return hashlib.sha256(token.encode()).hexdigest()


FAR = 4102444800  # 2100-01-01
SEEDED = {
    # token: (resource, scope, expires_at, revoked)
    "oha_parity_valid": ("http://127.0.0.1:${PORT}/mcp", "mcp", FAR, 0),
    "oha_parity_localhost": ("http://localhost:${PORT}/mcp", "mcp", FAR, 0),
    "oha_parity_wrong_resource": ("http://127.0.0.1:1/mcp", "mcp", FAR, 0),
    "oha_parity_wrong_scope": ("http://127.0.0.1:${PORT}/mcp", "admin", FAR, 0),
    "oha_parity_empty_scope": ("http://127.0.0.1:${PORT}/mcp", "", FAR, 0),
    "oha_parity_expired": ("http://127.0.0.1:${PORT}/mcp", "mcp", 1, 0),
    "oha_parity_revoked": ("http://127.0.0.1:${PORT}/mcp", "mcp", FAR, 1),
    "oha_parity_public": ("http://example.com/mcp", "mcp", FAR, 0),
    "oha_parity_public_port": ("http://example.com:${PORT}/mcp", "mcp", FAR, 0),
    "oha_parity_public_https": ("https://example.com/mcp", "mcp", FAR, 0),
}


def _seed_prelude(profile: str) -> list[dict]:
    """Start once so the agent creates authz.sqlite, then seed token rows."""
    argv = ["serve", "--mode=platform"] if profile == "platform" else ["serve"]
    rows = [
        "INSERT INTO tokens(token_hash, client_id, resource, scope, expires_at, revoked, created_at) "
        f"VALUES('{_hash(tok)}','host-agent-bootstrap','{res}','{scope}',{exp},{rev},1)"
        for tok, (res, scope, exp, rev) in SEEDED.items()
    ]
    return [
        {"as": "init", "serve": {"argv": argv, "profile": profile}},
        {"as": "initStop", "stop": {}},
        {"as": "seed", "sql": {"db": "${SANDBOX}/state/authz.sqlite", "statements": rows}},
    ]


AUTH_HEADERS = {
    "none": None, "bearerOnly": "Bearer", "bearerSpace": "Bearer ", "lowercase": "bearer ${TOKEN}",
    "uppercase": "BEARER ${TOKEN}", "basic": "Basic Zm9vOmJhcg==", "bootstrap": BEARER,
    "bootstrapPadded": "Bearer   ${TOKEN}   ", "bootstrapSuffix": "Bearer ${TOKEN}x",
    "wrong": "Bearer oha_parity_unknown", "tab": "Bearer\t${TOKEN}",
    **{f"seed:{tok}": f"Bearer {tok}" for tok in SEEDED},
}
AUTH_HOSTS = {"default": None, "localhost": "localhost:${PORT}", "ipv6": "[::1]:${PORT}",
              "public": "example.com", "publicPort": "example.com:${PORT}", "noPort": "127.0.0.1"}


def auth(profile: str) -> dict:
    steps: list[dict] = []
    body = _envelope("server/discover", {"_meta": _meta()})
    for akey, value in AUTH_HEADERS.items():
        for hkey, host in AUTH_HOSTS.items():
            hdrs = _headers("server/discover", auth=value)
            if host:
                hdrs["Host"] = host
            _label(steps, f"{akey} @{hkey}", _http("POST", "/mcp", hdrs, body))
    # Forwarded scheme decides the canonical resource.
    for tok in ("oha_parity_public", "oha_parity_public_https"):
        hdrs = _headers("server/discover", auth=f"Bearer {tok}")
        hdrs.update({"Host": "example.com", "X-Forwarded-Proto": "https"})
        _label(steps, f"forwarded-https seed:{tok}", _http("POST", "/mcp", hdrs, body))
    # Revocation, then reuse. Revoke reads the form body or the query string.
    form = {"Content-Type": "application/x-www-form-urlencoded"}
    _label(steps, "revoke GET", _http("GET", "/oauth/revoke?token=oha_parity_valid"))
    _label(steps, "revoke empty", _http("POST", "/oauth/revoke", form, ""))
    _label(steps, "revoke unknown", _http("POST", "/oauth/revoke", form, "token=oha_parity_unknown"))
    _label(steps, "revoke valid", _http("POST", "/oauth/revoke", form, "token=oha_parity_valid"))
    _label(steps, "reuse revoked", _http("POST", "/mcp", _headers("server/discover",
                                                                  auth="Bearer oha_parity_valid"), body))
    _label(steps, "revoke via query", _http("POST", "/oauth/revoke?token=oha_parity_localhost", form, ""))
    hdrs = _headers("server/discover", auth="Bearer oha_parity_localhost")
    hdrs["Host"] = "localhost:${PORT}"
    _label(steps, "reuse query-revoked", _http("POST", "/mcp", hdrs, body))
    _label(steps, "revoke bootstrap", _http("POST", "/oauth/revoke", form, "token=${TOKEN}"))
    _label(steps, "reuse bootstrap", _http("POST", "/mcp", _headers("server/discover"), body))
    # Metadata documents follow the Host the caller used.
    for hkey, host in AUTH_HOSTS.items():
        extra = {"Host": host} if host else {}
        _label(steps, f"prm @{hkey}", _http("GET", "/.well-known/oauth-protected-resource/mcp", extra))
        _label(steps, f"asm @{hkey}", _http("GET", "/.well-known/oauth-authorization-server", extra))
    _label(steps, "asm forwarded-https", _http("GET", "/.well-known/oauth-authorization-server",
                                               {"Host": "example.com", "X-Forwarded-Proto": "HTTPS"}))
    return _scenario(f"wire.auth.{profile}", "auth", "authz",
                     ["internal/authz/service.go:Authorize", "internal/authz/service.go:handleRevoke",
                      "internal/authz/resource.go:CanonicalMCPResource"],
                     steps, profile=profile, prelude=_seed_prelude(profile), collect=["sqlite"])


# --- wire.origin.* ------------------------------------------------------------

ORIGINS = {
    "none": None, "empty": "", "loopback": "http://127.0.0.1", "loopbackPort": "http://127.0.0.1:${PORT}",
    "loopbackSlash": "http://127.0.0.1/", "localhostHttps": "https://localhost:1",
    "ipv6": "http://[::1]:8", "localhostSuffix": "http://localhost.evil.example",
    "loopbackSuffix": "http://127.0.0.1.evil.example", "null": "null",
    "public": "http://example.com", "publicHttps": "https://example.com",
    "publicPort": "http://example.com:${PORT}", "foreign": "http://evil.example",
}
ORIGIN_HOSTS = {
    "local": (None, BEARER), "localhost": ("localhost:${PORT}", BEARER),
    "public": ("example.com", "Bearer oha_parity_public"),
    "publicPort": ("example.com:${PORT}", "Bearer oha_parity_public_port"),
}


def origin(profile: str, *, public_opt_in: bool = False) -> dict:
    steps: list[dict] = []
    for method in ("server/discover", "prompts/list"):
        body = _envelope(method, {"_meta": _meta()})
        for okey, value in ORIGINS.items():
            for hkey, (host, token) in ORIGIN_HOSTS.items():
                hdrs = _headers(method, auth=token)
                if host:
                    hdrs["Host"] = host
                if value is not None:
                    hdrs["Origin"] = value
                _label(steps, f"{method} {okey} @{hkey}", _http("POST", "/mcp", hdrs, body))
    # An address of this machine that is not loopback: local to the agent's
    # authorizer, but not to the SDK's DNS-rebinding guard.
    for method in ("server/discover", "prompts/list"):
        body = _envelope(method, {"_meta": _meta()})
        for okey in ("none", "loopback", "hostIp"):
            hdrs = _headers(method)
            hdrs["Host"] = "${HOST_IP}:${PORT}"
            if okey == "loopback":
                hdrs["Origin"] = "http://127.0.0.1"
            if okey == "hostIp":
                hdrs["Origin"] = "http://${HOST_IP}:${PORT}"
            _label(steps, f"{method} {okey} @hostIp", {**_http("POST", "/mcp", hdrs, body),
                                                       "requires": ["HOST_IP"]})
    sid = f"wire.origin.{profile}" + (".public-opt-in" if public_opt_in else "")
    env = {"OPUTE_MCP_DISABLE_LOCALHOST_PROTECTION": "true"} if public_opt_in else None
    return _scenario(sid, "mcp-wire", "transport",
                     ["internal/authz/resource.go:OriginAllowed",
                      "internal/transport/http.go:mcpHandlerForRequest",
                      "go-sdk mcp/streamable.go DNS rebinding protection"],
                     steps, profile=profile, env=env, prelude=_seed_prelude(profile))


# --- wire.framing -------------------------------------------------------------

def _raw(method: str, target: str, headers: list[str], body: str = "", *, proto: str = "HTTP/1.1",
         close: bool = True, host: bool = True) -> str:
    lines = [f"{method} {target} {proto}"]
    if host:
        lines.append(f"Host: {LOCAL_HOST}")
    lines += headers
    if close:
        lines.append("Connection: close")
    return "\r\n".join(lines) + "\r\n\r\n" + body


def _mcp_headers(method: str = "server/discover") -> list[str]:
    return ["Content-Type: application/json", "Accept: application/json, text/event-stream",
            f"MCP-Protocol-Version: {PROTOCOL}", f"Mcp-Method: {method}", f"Authorization: {BEARER}"]


def _chunk(body: str, *parts: int) -> str:
    out, i = [], 0
    for n in parts:
        out.append(f"{n:x}\r\n{body[i:i + n]}\r\n")
        i += n
    if i < len(body):
        out.append(f"{len(body) - i:x}\r\n{body[i:]}\r\n")
    return "".join(out) + "0\r\n\r\n"


def framing() -> dict:
    steps: list[dict] = []
    body = json.dumps(_envelope("server/discover", {"_meta": _meta()}), separators=(",", ":"))
    cl = f"Content-Length: {len(body)}"
    mh = _mcp_headers()
    cases: dict[str, Any] = {
        # body framing
        "cl": _raw("POST", "/mcp", mh + [cl], body),
        "chunked": _raw("POST", "/mcp", mh + ["Transfer-Encoding: chunked"], _chunk(body, len(body))),
        "chunkedSplit": _raw("POST", "/mcp", mh + ["Transfer-Encoding: chunked"], _chunk(body, 1, 7, 50)),
        "chunkedExtension": _raw("POST", "/mcp", mh + ["Transfer-Encoding: chunked"],
                                 f"{len(body):x};name=value\r\n{body}\r\n0\r\n\r\n"),
        "chunkedTrailer": _raw("POST", "/mcp", mh + ["Transfer-Encoding: chunked", "Trailer: X-T"],
                               f"{len(body):x}\r\n{body}\r\n0\r\nX-T: 1\r\n\r\n"),
        "chunkedUpperHex": _raw("POST", "/mcp", mh + ["Transfer-Encoding: CHUNKED"],
                                f"{len(body):X}\r\n{body}\r\n0\r\n\r\n"),
        "chunkedBadSize": _raw("POST", "/mcp", mh + ["Transfer-Encoding: chunked"], f"zz\r\n{body}\r\n0\r\n\r\n"),
        "chunkedMissingCrlf": _raw("POST", "/mcp", mh + ["Transfer-Encoding: chunked"],
                                   f"{len(body):x}\r\n{body}XX0\r\n\r\n"),
        "clAndChunked": _raw("POST", "/mcp", mh + ["Content-Length: 3", "Transfer-Encoding: chunked"],
                             _chunk(body, len(body))),
        "clTwiceSame": _raw("POST", "/mcp", mh + [cl, cl], body),
        "clTwiceDiffer": _raw("POST", "/mcp", mh + [cl, "Content-Length: 3"], body),
        "clEmpty": _raw("POST", "/mcp", mh + ["Content-Length:"], body),
        "clPlus": _raw("POST", "/mcp", mh + [f"Content-Length: +{len(body)}"], body),
        "clNegative": _raw("POST", "/mcp", mh + ["Content-Length: -1"], body),
        "clHex": _raw("POST", "/mcp", mh + ["Content-Length: 0x10"], body),
        "clInnerSpace": _raw("POST", "/mcp", mh + ["Content-Length: 1 2"], body),
        "teGzip": _raw("POST", "/mcp", mh + ["Transfer-Encoding: gzip"], body),
        "teChunkedGzip": _raw("POST", "/mcp", mh + ["Transfer-Encoding: chunked, gzip"], body),
        "teTwice": _raw("POST", "/mcp", mh + ["Transfer-Encoding: chunked", "Transfer-Encoding: chunked"], body),
        "teIgnoredOnHttp10": _raw("POST", "/mcp", mh + ["Transfer-Encoding: chunked", cl], body, proto="HTTP/1.0"),
        "expectContinue": _raw("POST", "/mcp", mh + [cl, "Expect: 100-continue"], body),
        "expectContinueUnread": _raw("POST", "/health", [cl, "Expect: 100-continue"], body),
        "expectContinueNoBody": _raw("GET", "/health", ["Expect: 100-continue"]),
        "expectOther": _raw("GET", "/health", ["Expect: something"]),
        # header syntax
        "obsFold": _raw("GET", "/health", ["X-Folded: a", " b"]),
        "foldFirstLine": "GET /health HTTP/1.1\r\n X-Bad: a\r\nHost: x\r\nConnection: close\r\n\r\n",
        "noColon": _raw("GET", "/health", ["X-NoColon"]),
        "spaceBeforeColon": _raw("GET", "/health", ["X-Space : a"]),
        "ctlInValue": _raw("GET", "/health", ["X-Ctl: a\x01b"]),
        "delInValue": _raw("GET", "/health", ["X-Del: a\x7fb"]),
        "obsTextValue": _raw("GET", "/health", ["X-Obs: caf\xe9"]),
        "emptyKey": _raw("GET", "/health", [": a"]),
        "badKeyChar": _raw("GET", "/health", ["X(Bad): a"]),
        "hostWithSpace": _raw("GET", "/health", ["Host: a b"], host=False),
        "hostEmpty": _raw("GET", "/health", ["Host:"], host=False),
        "hostIpv6": _raw("GET", "/health", ["Host: [::1]:${PORT}"], host=False),
        # request line
        "noProto": "GET /health\r\n\r\n",
        "doubleSpace": _raw("GET", " /health", []),
        "relativeUri": _raw("GET", "health", []),
        "badEscape": _raw("GET", "/%zz", []),
        "protoTrailing": _raw("GET", "/health", [], proto="HTTP/1.1 x"),
        "badMethodChar": _raw("G(T", "/health", []),
        "http20": _raw("GET", "/health", [], proto="HTTP/2.0"),
        "http09": _raw("GET", "/health", [], proto="HTTP/0.9"),
        "priStar": _raw("PRI", "*", [], proto="HTTP/2.0"),
        "lfOnly": "GET /health HTTP/1.1\nHost: 127.0.0.1:${PORT}\nConnection: close\n\n",
        "uriWithFragment": _raw("GET", "/health#frag", []),
        "absoluteUriOtherHost": _raw("GET", "http://example.com/health", []),
        "absoluteUriNoPath": _raw("GET", "http://127.0.0.1:${PORT}", []),
        "connectAuthority": _raw("CONNECT", "127.0.0.1:${PORT}", []),
        "connectPath": _raw("CONNECT", "/mcp", []),
        # connection management
        "pipelined": _raw("GET", "/health", [], close=False) + _raw("GET", "/nope", []),
        "http10KeepAlive": _raw("GET", "/health", ["Connection: keep-alive"], proto="HTTP/1.0", close=False)
                           + _raw("GET", "/nope", [], proto="HTTP/1.0", close=False),
        "http10Plain": _raw("GET", "/nope", [], proto="HTTP/1.0", close=False),
        "postThenCrlf": _raw("POST", "/oauth/revoke", ["Content-Length: 0"], close=False)
                        + "\r\n" + _raw("GET", "/nope", []),
        "connectionCloseUpper": _raw("GET", "/nope", ["Connection: CLOSE"], close=False),
        # response framing
        "longRedirect": _raw("GET", "//" + "a" * 2100, []),
        "longRedirectHttp10": _raw("GET", "//" + "a" * 2100, [], proto="HTTP/1.0", close=False),
        "redirectQuery": _raw("GET", "//mcp?a=1&b=%20", []),
        "redirectEscaped": _raw("GET", "//%6Dcp", []),
        "redirectNonAscii": _raw("GET", "//caf%C3%A9", []),
        "redirectSpecialChars": _raw("GET", "//a%22b%3Cc", []),
    }
    for key, data in cases.items():
        _label(steps, f"raw {key}", {"raw": {"data": data}})
    _label(steps, "raw headerTooLarge", {"raw": {"data": _raw("GET", "/health", ["X-Big: ${PAD}"]),
                                                  "padTo": (1 << 20) + 8192}})
    _label(steps, "raw headerJustUnderLimit", {"raw": {"data": _raw("GET", "/nope", ["X-Big: ${PAD}"]),
                                                        "padTo": (1 << 20)}})
    return _scenario("wire.framing", "http", "transport",
                     ["net/http server.go:readRequest", "net/http transfer.go:readTransfer",
                      "net/http server.go:chunkWriter.writeHeader"], steps)


# --- wire.sdk-edges --------------------------------------------------------------

def sdk_edges(legacy: bool) -> dict:
    steps: list[dict] = []
    meta = _meta()
    ci, caps = "io.modelcontextprotocol/clientInfo", "io.modelcontextprotocol/clientCapabilities"

    def call(method: str, params: Any, *, name: str | None = None, modern: bool = True,
             rid: Any = 1, headers: dict | None = None, with_params: bool = True) -> dict:
        hdrs = _headers(method, modern=modern, name=name)
        hdrs.update(headers or {})
        env = {"jsonrpc": "2.0", "method": method}
        if with_params:
            env["params"] = params
        if rid is not None:
            env["id"] = rid
        return _http("POST", "/mcp", hdrs, env)

    cases = {
        "clientInfoNumber": call("prompts/list", {"_meta": {**meta, ci: 7}}),
        "clientInfoString": call("prompts/list", {"_meta": {**meta, ci: "x"}}),
        "clientInfoNameNumber": call("prompts/list", {"_meta": {**meta, ci: {"name": 1, "version": "1"}}}),
        "clientInfoNull": call("prompts/list", {"_meta": {**meta, ci: None}}),
        "clientInfoExtraFields": call("prompts/list", {"_meta": {**meta, ci: {"name": "a", "x": [1]}}}),
        "capabilitiesNull": call("prompts/list", {"_meta": {**meta, caps: None}}),
        "capabilitiesString": call("prompts/list", {"_meta": {**meta, caps: "x"}}),
        "capabilitiesArray": call("prompts/list", {"_meta": {**meta, caps: []}}),
        "capabilitiesExtensionsArray": call("prompts/list", {"_meta": {**meta, caps: {"extensions": []}}}),
        "logLevelMeta": call("prompts/list", {"_meta": {**meta, "io.modelcontextprotocol/logLevel": "debug"}}),
        "toolsCallNameNumber": call("tools/call", {"name": 7, "_meta": meta}, name="7"),
        "toolsCallArgsArray": call("tools/call", {"name": UNKNOWN_TOOL, "arguments": [], "_meta": meta},
                                   name=UNKNOWN_TOOL),
        "toolsCallArgsNull": call("tools/call", {"name": UNKNOWN_TOOL, "arguments": None, "_meta": meta},
                                  name=UNKNOWN_TOOL),
        "toolsCallNoArgs": call("tools/call", {"name": UNKNOWN_TOOL, "_meta": meta}, name=UNKNOWN_TOOL),
        "toolsCallPaddedName": call("tools/call", {"name": UNKNOWN_TOOL, "_meta": meta},
                                    name="  " + UNKNOWN_TOOL + "  "),
        "promptsGetNamed": call("prompts/get", {"name": "p", "_meta": meta}, name="p"),
        "promptsGetNameNumber": call("prompts/get", {"name": 3, "_meta": meta}, name="3"),
        "promptsGetArgsArray": call("prompts/get", {"name": "p", "arguments": [], "_meta": meta}, name="p"),
        "completionWithRef": call("completion/complete", {"ref": {"type": "ref/prompt", "name": "p"},
                                                          "argument": {"name": "a", "value": ""}, "_meta": meta}),
        "completionRefNull": call("completion/complete", {"ref": None, "_meta": meta}),
        "toolsListCursor": call("tools/list", {"cursor": "bogus", "_meta": meta}),
        "toolsListValidCursor": call("tools/list", {"cursor": VALID_CURSOR, "_meta": meta}),
        "promptsListValidCursor": call("prompts/list", {"cursor": VALID_CURSOR, "_meta": meta}),
        "promptsListEmptyCursor": call("prompts/list", {"cursor": EMPTY_CURSOR, "_meta": meta}),
        "promptsListTruncatedCursor": call("prompts/list", {"cursor": VALID_CURSOR[:-4], "_meta": meta}),
        "promptsListCursor": call("prompts/list", {"cursor": "bogus", "_meta": meta}),
        "notificationWithId": call("notifications/cancelled", {"requestId": 1, "_meta": meta}, rid=5),
        "progressNoParams": call("notifications/progress", None, with_params=False),
        "rootsListChanged": call("notifications/roots/list_changed", {"_meta": meta}, rid=None),
        "rootsListChangedCall": call("notifications/roots/list_changed", {"_meta": meta}),
        "resourcesUnsubscribe": call("resources/unsubscribe", {"uri": "x", "_meta": meta}),
        "discoverParamsExtra": call("server/discover", {"_meta": meta, "extra": True}),
        "discoverIdString": call("server/discover", {"_meta": meta}, rid="abc"),
    }
    if legacy:
        bare = lambda method, params, **kw: call(method, params, modern=False, **kw)
        cases.update({
            "initializeV20241105": bare("initialize", {**_params_for("initialize"), "protocolVersion": "2024-11-05"}),
            "initializeV20260728": bare("initialize", {**_params_for("initialize"), "protocolVersion": "2026-07-28"}),
            "initializeUnknownVersion": bare("initialize", {**_params_for("initialize"), "protocolVersion": "1999-01-01"}),
            "initializeVersionNumber": bare("initialize", {**_params_for("initialize"), "protocolVersion": 7}),
            "initializeNoVersion": bare("initialize", {"capabilities": {}, "clientInfo": {"name": "x", "version": "1"}}),
            "initializeEmpty": bare("initialize", {}),
            "initializeParamsNull": bare("initialize", None),
            "initializeParamsArray": bare("initialize", []),
            "initializeClientInfoNumber": bare("initialize", {**_params_for("initialize"), "clientInfo": 1}),
            "initializedWithId": bare("notifications/initialized", {}, rid=3),
            "pingParamsNull": bare("ping", None),
            "pingParamsArray": bare("ping", []),
            "pingParamsString": bare("ping", "x"),
            "toolsListParamsArray": bare("tools/list", []),
            "toolsListCursorLegacy": bare("tools/list", {"cursor": "bogus"}),
            "toolsCallArgsArrayLegacy": bare("tools/call", {"name": UNKNOWN_TOOL, "arguments": []}),
            "toolsCallParamsArrayLegacy": bare("tools/call", []),
            "promptsGetNoName": bare("prompts/get", {}),
            "promptsListOldMeta": bare("prompts/list", {"_meta": {"progressToken": 1}}),
            "promptsListVersionHeaderOnly": bare("prompts/list", {}, headers={"MCP-Protocol-Version": PROTOCOL}),
            "promptsListOldHeader": bare("prompts/list", {}, headers={"MCP-Protocol-Version": "2024-11-05"}),
            "promptsListFutureHeader": bare("prompts/list", {}, headers={"MCP-Protocol-Version": "2099-01-01"}),
            "cancelledNoParams": bare("notifications/cancelled", None, with_params=False, rid=None),
        })
    for key, step in cases.items():
        _label(steps, key, step)
    masks = [{"path": ["steps", k, "body", "result", "tools"], "type": "array",
              "reason": CATALOG_MASK_REASON, "optional": True} for k in cases if "toolsList" in k]
    suffix = "legacy-on" if legacy else "legacy-off"
    return _scenario(f"wire.sdk-edges.{suffix}", "mcp-wire", "transport",
                     ["go-sdk mcp/shared.go:validateRequestMeta", "go-sdk mcp/server.go:ServerSession.handle",
                      "go-sdk mcp/streamable.go:servePOST"],
                     steps, env=LEGACY_ENV if legacy else None, masks=masks)


def generate() -> list[dict]:
    return [
        routing(), transport(), envelope(), methods(legacy=False), methods(legacy=True),
        framing(), sdk_edges(False), sdk_edges(True),
        auth("standalone"), auth("platform"),
        origin("standalone"), origin("platform"), origin("standalone", public_opt_in=True),
    ]


def render() -> str:
    return json.dumps(generate(), indent=2, ensure_ascii=False) + "\n"


def write() -> int:
    OUT_FILE.write_text(render())
    return sum(len(s["steps"]) for s in generate())
