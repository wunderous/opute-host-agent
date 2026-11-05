"""Shared pieces of the E2E dev VM, the journey runner and the verifier.

Python 3.11 standard library only. Nothing here reads a credential other
than the dev VM's own test token, and nothing writes one to a log.
"""
from __future__ import annotations

import hashlib
import json
import os
import re
import subprocess
import time
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any

E2E_DIR = Path(__file__).resolve().parent
REPO_ROOT = E2E_DIR.parent.parent


def _parity_dir() -> Path:
    """The main checkout's .parity: the goldens, release sets and the dev VM live there."""
    if os.environ.get("OPUTE_E2E_PARITY_DIR"):
        return Path(os.environ["OPUTE_E2E_PARITY_DIR"])
    common = subprocess.run(["git", "-C", str(REPO_ROOT), "rev-parse", "--path-format=absolute", "--git-common-dir"],
                            capture_output=True, text=True, check=True).stdout.strip()
    return Path(common).parent / ".parity"


PARITY = _parity_dir()
T2 = PARITY / "t2"
DEV = PARITY / "e2e" / "devvm"
# The one lease the dev VM and the T2 twins share (tools/parity/parity/t2.py
# reads the same file): while it is held, no twin starts.
LEASE = T2 / "dev-vm.lease.json"
LEASE_MAX_HOURS = 12
PROTOCOL_VERSION = "2026-07-28"


def sha256_file(path: Path) -> str:
    with Path(path).open("rb") as f:
        return hashlib.file_digest(f, "sha256").hexdigest()


def sha256_json(obj: Any) -> str:
    return hashlib.sha256(json.dumps(obj, sort_keys=True, separators=(",", ":")).encode()).hexdigest()


def now() -> str:
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())


# --- the dev-vm lease ---------------------------------------------------------------


def lease_status() -> dict | None:
    """The held lease, or None when there is none or it expired."""
    try:
        lease = json.loads(LEASE.read_text())
    except (FileNotFoundError, ValueError):
        return None
    return lease if lease.get("expiresEpoch", 0) > time.time() else None


def require_lease() -> dict:
    lease = lease_status()
    if lease is None:
        raise SystemExit(f"refused: the dev-vm lease is not held ({LEASE}); "
                         "run `devvm.py lease acquire --holder NAME` first")
    return lease


def running_t2_twins() -> list[str]:
    """qemu processes booted with the T2 seed (every twin and golden build is)."""
    seed = f"file={T2}/seed.img,".encode()
    out = []
    for proc in Path("/proc").iterdir():
        if not proc.name.isdigit():
            continue
        try:
            argv = (proc / "cmdline").read_bytes().split(b"\0")
        except OSError:
            continue
        if argv and argv[0].endswith(b"qemu-system-x86_64") and any(a.startswith(seed) for a in argv):
            out.append(proc.name)
    return out


def acquire_lease(holder: str, hours: float) -> dict:
    if not holder or hours <= 0 or hours > LEASE_MAX_HOURS:
        raise SystemExit(f"refused: a lease needs a holder and 0 < hours <= {LEASE_MAX_HOURS}")
    held = lease_status()
    if held and held["holder"] != holder:
        raise SystemExit(f"refused: the dev-vm lease is held by {held['holder']} until {held['expires']}")
    twins = running_t2_twins()
    if twins:
        raise SystemExit(f"refused: T2 twins are running (pids {', '.join(twins)}); they share the runtime")
    T2.mkdir(parents=True, exist_ok=True)
    expires = time.time() + hours * 3600
    lease = {"holder": holder, "acquired": now(), "expiresEpoch": expires,
             "expires": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(expires)),
             "scope": "E2E dev VM: one qemu/KVM VM, 8 vCPU, 16 GiB, 100 GiB; T2 twins do not start while held"}
    tmp = LEASE.with_suffix(".tmp")
    tmp.write_text(json.dumps(lease, indent=1) + "\n")
    tmp.replace(LEASE)
    return lease


def release_lease(holder: str) -> None:
    held = lease_status()
    if held and held["holder"] != holder:
        raise SystemExit(f"refused: the dev-vm lease is held by {held['holder']}")
    LEASE.unlink(missing_ok=True)


# --- MCP over Streamable HTTP ---------------------------------------------------------


def mcp(port: int, token: str | None, method: str, params: dict | None = None, request_id: int = 1,
        client: str = "opute-e2e", timeout: float = 120) -> dict:
    """One 2026-07-28 JSON-RPC request; returns status, content type and the decoded body."""
    body = {"jsonrpc": "2.0", "method": method, "id": request_id, "params": {
        **(params or {}), "_meta": {
            "io.modelcontextprotocol/protocolVersion": PROTOCOL_VERSION,
            "io.modelcontextprotocol/clientCapabilities": {},
            "io.modelcontextprotocol/clientInfo": {"name": client, "version": "1"}}}}
    headers = {"Accept": "application/json, text/event-stream", "Content-Type": "application/json",
               "MCP-Protocol-Version": PROTOCOL_VERSION, "Mcp-Method": method,
               "Origin": f"http://127.0.0.1:{port}"}
    if token:
        headers["Authorization"] = f"Bearer {token}"
    if method == "tools/call":
        headers["Mcp-Name"] = (params or {})["name"]
    request = urllib.request.Request(f"http://127.0.0.1:{port}/mcp", data=json.dumps(body).encode(),
                                     headers=headers, method="POST")
    started = time.monotonic()
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            raw, status, ctype = response.read(), response.status, response.headers.get("Content-Type", "")
    except urllib.error.HTTPError as error:
        raw, status, ctype = error.read(), error.code, error.headers.get("Content-Type", "")
    except OSError as error:
        return {"status": None, "error": type(getattr(error, "reason", error)).__name__,
                "ms": round((time.monotonic() - started) * 1000)}
    return {"status": status, "contentType": ctype.split(";")[0].strip(), "body": decode_body(ctype, raw),
            "ms": round((time.monotonic() - started) * 1000)}


def decode_body(ctype: str, raw: bytes) -> Any:
    text = raw.decode(errors="replace")
    if ctype.startswith("text/event-stream"):
        events = [json.loads(line[5:]) for line in text.splitlines() if line.startswith("data:") and line[5:].strip()]
        return events[-1] if events else None
    try:
        return json.loads(text) if text.strip() else None
    except ValueError:
        return {"raw": text[:400]}


def health(port: int, timeout: float = 5) -> dict:
    try:
        with urllib.request.urlopen(f"http://127.0.0.1:{port}/health", timeout=timeout) as response:
            return {"status": response.status, "body": json.loads(response.read())}
    except urllib.error.HTTPError as error:
        return {"status": error.code, "body": None}
    except (OSError, ValueError) as error:
        return {"status": None, "error": type(error).__name__}


# --- JSON Schema (the subset the catalog uses; anything else fails closed) ------------

_ANNOTATIONS = {"description", "default", "format", "writeOnly", "readOnly", "title", "examples", "$schema",
                "$id", "$comment", "deprecated"}
_TYPES = {"object": dict, "array": list, "string": str, "boolean": bool, "null": type(None)}


def _is_type(value: Any, name: str) -> bool:
    if name == "integer":
        return isinstance(value, int) and not isinstance(value, bool) or (
            isinstance(value, float) and value.is_integer())
    if name == "number":
        return isinstance(value, (int, float)) and not isinstance(value, bool)
    return isinstance(value, _TYPES[name]) if name in _TYPES else False


def validate(value: Any, schema: Any, path: str = "$") -> list[str]:
    """Errors of value against schema; an unsupported keyword is an error."""
    if schema is True or schema == {}:
        return []
    if schema is False:
        return [f"{path}: no value allowed"]
    if not isinstance(schema, dict):
        return [f"{path}: schema is not an object"]
    errors: list[str] = []
    for key in schema:
        if key not in _ANNOTATIONS and not key.startswith("x-") and key not in (
                "type", "properties", "required", "additionalProperties", "items", "enum", "const", "minimum",
                "maximum", "minLength", "maxLength", "pattern", "minItems", "maxItems", "nullable"):
            errors.append(f"{path}: unsupported schema keyword {key!r}")
    if value is None and schema.get("nullable"):
        return errors
    types = schema.get("type")
    if types is not None:
        names = types if isinstance(types, list) else [types]
        if not any(_is_type(value, n) for n in names):
            return errors + [f"{path}: expected {'/'.join(names)}, got {type(value).__name__}"]
    if "enum" in schema and value not in schema["enum"]:
        errors.append(f"{path}: {value!r} not in enum")
    if "const" in schema and value != schema["const"]:
        errors.append(f"{path}: {value!r} != const")
    if isinstance(value, (int, float)) and not isinstance(value, bool):
        if "minimum" in schema and value < schema["minimum"]:
            errors.append(f"{path}: {value} < minimum {schema['minimum']}")
        if "maximum" in schema and value > schema["maximum"]:
            errors.append(f"{path}: {value} > maximum {schema['maximum']}")
    if isinstance(value, str):
        if "minLength" in schema and len(value) < schema["minLength"]:
            errors.append(f"{path}: shorter than {schema['minLength']}")
        if "maxLength" in schema and len(value) > schema["maxLength"]:
            errors.append(f"{path}: longer than {schema['maxLength']}")
        if "pattern" in schema and not re.search(schema["pattern"], value):
            errors.append(f"{path}: does not match {schema['pattern']!r}")
    if isinstance(value, list):
        if "minItems" in schema and len(value) < schema["minItems"]:
            errors.append(f"{path}: fewer than {schema['minItems']} items")
        if "maxItems" in schema and len(value) > schema["maxItems"]:
            errors.append(f"{path}: more than {schema['maxItems']} items")
        if "items" in schema:
            for i, item in enumerate(value):
                errors += validate(item, schema["items"], f"{path}[{i}]")
    if isinstance(value, dict):
        props = schema.get("properties") or {}
        for name in schema.get("required") or []:
            if name not in value:
                errors.append(f"{path}: missing required {name!r}")
        for name, item in value.items():
            if name in props:
                errors += validate(item, props[name], f"{path}.{name}")
            elif "additionalProperties" in schema:
                errors += validate(item, schema["additionalProperties"], f"{path}.{name}")
    return errors


def validate_response(method: str, request_id: int, response: dict, tools: dict[str, dict]) -> list[str]:
    """Schema errors of one MCP exchange: the JSON-RPC envelope, then the result's own schema.

    A tools/call result's structuredContent is checked against the outputSchema
    the same session's tools/list published for that tool.
    """
    body = response.get("body")
    if response.get("status") is None:
        return [f"no HTTP response ({response.get('error')})"]
    if response["status"] != 200:
        return []  # an HTTP refusal has no JSON-RPC body to validate; the journey judges its status
    if not isinstance(body, dict):
        return ["body is not a JSON-RPC object"]
    errors = []
    if body.get("jsonrpc") != "2.0":
        errors.append("jsonrpc != 2.0")
    if body.get("id") != request_id:
        errors.append(f"id {body.get('id')!r} != {request_id}")
    if ("result" in body) == ("error" in body):
        return errors + ["exactly one of result and error is required"]
    if "error" in body:
        return errors + validate(body["error"], {"type": "object", "required": ["code", "message"], "properties": {
            "code": {"type": "integer"}, "message": {"type": "string"}}}, "$.error")
    result = body["result"]
    if method == "tools/list":
        errors += validate(result, {"type": "object", "required": ["tools"], "properties": {"tools": {
            "type": "array", "items": {"type": "object", "required": ["name", "inputSchema"], "properties": {
                "name": {"type": "string", "minLength": 1}, "inputSchema": {"type": "object"},
                "outputSchema": {"type": "object"}}}}}}, "$.result")
    elif method == "tools/call":
        errors += validate(result, {"type": "object", "properties": {
            "content": {"type": "array", "items": {"type": "object", "required": ["type"]}},
            "isError": {"type": "boolean"}, "structuredContent": {"type": "object"}}}, "$.result")
        name = response.get("tool")
        schema = (tools.get(name) or {}).get("outputSchema")
        if name not in tools and result.get("resultType") != "task":
            errors.append(f"tool {name!r} was not in this session's tools/list")
        if schema and not result.get("isError") and "structuredContent" in result:
            errors += validate(result["structuredContent"], schema, "$.result.structuredContent")
    else:
        errors += validate(result, {"type": "object"}, "$.result")
    return errors
