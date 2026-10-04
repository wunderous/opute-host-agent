"""Isolated agent processes and a raw MCP/HTTP client.

Every side of a comparison gets its own Sandbox: a fresh directory holding
HOME, XDG config, standalone state, instance root, shims and the shim trace,
plus its own opaque agent ID, port and bearer token. Nothing is shared
between sides.
"""

from __future__ import annotations

import http.client
import json
import os
import random
import signal
import socket
import sqlite3
import stat
import subprocess
import tempfile
import threading
import time
import uuid
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

from . import shims

# Response headers that are part of the observable contract. Anything not
# listed (Date, Content-Length, ...) is transport noise.
HEADER_ALLOWLIST = (
    "allow",
    "cache-control",
    "content-type",
    "location",
    "mcp-protocol-version",
    "mcp-session-id",
    "www-authenticate",
    "x-content-type-options",
)

PROTOCOL_VERSION = "2026-07-28"


# ADR 0012: the MCP wire prefix is the first 8 hex digits of
# UUIDv5(UUIDv5(DNS, "opute.host-agent.mcp-tool-prefix"), agentID). The
# harness derives it independently so that a wrong derivation shows up as a
# diff instead of being hidden by a mask.
_PREFIX_NAMESPACE = uuid.uuid5(uuid.NAMESPACE_DNS, "opute.host-agent.mcp-tool-prefix")


_HOST_IP: list[str] = []


def host_ip() -> str:
    """A non-loopback address of this machine, or "" when there is none.

    The agent treats its own interface addresses as local hosts, which is the
    one case where the SDK's DNS-rebinding guard can fire; steps that need
    such an address declare `"requires": ["HOST_IP"]`.
    """
    if not _HOST_IP:
        found = ""
        probe = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        try:
            probe.connect(("10.255.255.255", 1))
            address = probe.getsockname()[0]
            if not address.startswith("127."):
                found = address
        except OSError:
            pass
        finally:
            probe.close()
        _HOST_IP.append(found)
    return _HOST_IP[0]


def tool_name_prefix(agent_id: str) -> str:
    agent_id = agent_id.strip()
    if not agent_id:
        return ""
    return uuid.uuid5(_PREFIX_NAMESPACE, agent_id).hex[:8]


# Sandbox ports come from a fixed range below Linux's ephemeral range
# (32768+), and each is handed out once per harness process. Asking the
# kernel for port 0 is racy under parallel load: the number can be reused by
# another sandbox or by an outgoing client connection before the agent binds
# it, which shows up as a spurious diff (listener seen, bind failed).
_PORT_RANGE = range(20000, 30000)
_PORT_LOCK = threading.Lock()
_PORTS_HANDED_OUT: set[int] = set()
_PORT_CURSOR = [random.randrange(len(_PORT_RANGE))]


def free_port() -> int:
    with _PORT_LOCK:
        for _ in range(len(_PORT_RANGE)):
            _PORT_CURSOR[0] = (_PORT_CURSOR[0] + 1) % len(_PORT_RANGE)
            port = _PORT_RANGE[_PORT_CURSOR[0]]
            if port in _PORTS_HANDED_OUT:
                continue
            with socket.socket() as probe:
                try:
                    probe.bind(("0.0.0.0", port))
                except OSError:
                    continue
            _PORTS_HANDED_OUT.add(port)
            return port
        _PORTS_HANDED_OUT.clear()
    return free_port()


def port_open(port: int) -> bool:
    with socket.socket() as s:
        s.settimeout(0.05)
        return s.connect_ex(("127.0.0.1", port)) == 0


@dataclass
class Impl:
    """One implementation under test (the Go reference or the Rust candidate)."""

    label: str  # "go" or "rust"
    binary: Path

    def sha256(self) -> str:
        import hashlib

        return hashlib.sha256(Path(self.binary).read_bytes()).hexdigest()


@dataclass
class Sandbox:
    side: str
    run_id: str
    fixture: Path | None
    root: Path = field(init=False)
    agent_id: str = field(init=False)
    token: str = field(init=False)
    port: int = field(init=False)

    def __post_init__(self) -> None:
        self.root = Path(tempfile.mkdtemp(prefix=f"parity-{self.side}-"))
        # Distinct explicit identities per side; never derived from the machine.
        self.agent_id = f"parity-{self.side}-{self.run_id}"
        self.token = f"parity-token-{self.side}-{self.run_id}"
        self.port = free_port()
        for sub in ("home", "xdg", "state", "instance", "shims"):
            (self.root / sub).mkdir()
        self.trace_path = self.root / "trace.jsonl"
        self.captured: dict[str, str] = {}
        self.shim_names: list[str] = []
        if self.fixture:
            self.shim_names = shims.install(self.root / "shims", self.fixture, self.trace_path)

    @property
    def variables(self) -> dict[str, str]:
        variables = {
            "SANDBOX": str(self.root),
            "AGENT_ID": self.agent_id,
            "TOKEN": self.token,
            "PORT": str(self.port),
            "TOOL_PREFIX": tool_name_prefix(self.agent_id),
        }
        if host_ip():
            variables["HOST_IP"] = host_ip()
        # Values a scenario captured from a response (a task ID, say). They
        # are per-side literals like the agent ID: normalization replaces
        # them with ${NAME}, and later steps may expand them.
        variables.update(self.captured)
        return variables

    def base_env(self) -> dict[str, str]:
        env = {
            "PATH": f"{self.root / 'shims'}:/usr/bin:/bin",
            "HOME": str(self.root / "home"),
            "XDG_CONFIG_HOME": str(self.root / "xdg"),
            "LANG": "C",
            "TZ": "UTC",
        }
        if "incus" in self.shim_names:
            env["OPUTE_INCUS_BINARY_PATH"] = str(self.root / "shims" / "incus")
        return env

    def expand(self, value: str) -> str:
        for name, literal in self.variables.items():
            value = value.replace("${" + name + "}", literal)
        return value

    def env_for(self, overrides: dict[str, Any]) -> dict[str, str]:
        env = self.base_env()
        for key, value in overrides.items():
            if value is None:
                env.pop(key, None)
            else:
                env[key] = self.expand(str(value))
        return env

    def trace(self) -> list[dict]:
        return shims.read_trace(self.trace_path)

    def files(self) -> list[dict]:
        """Files the agent created, with type and permission bits."""
        out = []
        skip = {self.root / "shims", self.root / "gates", self.root / "pids", self.trace_path}
        for path in sorted(self.root.rglob("*")):
            if any(path == s or s in path.parents for s in skip):
                continue
            st = path.lstat()
            kind = "dir" if stat.S_ISDIR(st.st_mode) else "file"
            out.append(
                {
                    "path": str(path.relative_to(self.root)),
                    "type": kind,
                    "mode": oct(stat.S_IMODE(st.st_mode)),
                }
            )
        return out

    def sqlite_schemas(self) -> dict[str, Any]:
        """Schema of every SQLite database the agent created (read-only)."""
        out: dict[str, Any] = {}
        for path in sorted(self.root.rglob("*")):
            if not path.is_file() or path.suffix in (".db-wal", ".db-shm") or path.name.endswith(("-wal", "-shm")):
                continue
            with open(path, "rb") as fh:
                if fh.read(16) != b"SQLite format 3\x00":
                    continue
            conn = sqlite3.connect(f"file:{path}?mode=ro", uri=True)
            try:
                rows = conn.execute(
                    "SELECT type, name, tbl_name, sql FROM sqlite_master ORDER BY type, name"
                ).fetchall()
                version = conn.execute("PRAGMA user_version").fetchone()[0]
                counts = {}
                for kind, name, _tbl, _sql in rows:
                    if kind == "table" and not name.startswith("sqlite_"):
                        counts[name] = conn.execute(f'SELECT COUNT(*) FROM "{name}"').fetchone()[0]
            finally:
                conn.close()
            out[str(path.relative_to(self.root))] = {
                "userVersion": version,
                "objects": [
                    {"type": t, "name": n, "table": tb, "sql": s} for t, n, tb, s in rows
                ],
                "rowCounts": counts,
            }
        return out

    def durable_snapshot(self) -> dict[str, Any]:
        """Fingerprint every SQLite schema and row, without exposing values.

        Logical contents are compared: WAL/checkpoint bytes are not durable
        semantics. Include table names and row contents, so an UPDATE with
        unchanged row counts cannot pass the rejected-call contract.
        """
        import hashlib

        databases = {}
        for relative, schema in self.sqlite_schemas().items():
            conn = sqlite3.connect(f"file:{self.root / relative}?mode=ro", uri=True)
            try:
                contents = {}
                for obj in schema["objects"]:
                    if obj["type"] != "table":
                        continue
                    name = obj["name"].replace('"', '""')
                    contents[obj["name"]] = sorted(
                        repr(row) for row in conn.execute(f'SELECT * FROM "{name}"'))
                payload = json.dumps({"schema": schema, "rows": contents}, sort_keys=True)
                databases[relative] = hashlib.sha256(payload.encode()).hexdigest()
            finally:
                conn.close()
        payload = json.dumps(databases, sort_keys=True)
        return {"digest": hashlib.sha256(payload.encode()).hexdigest(),
                "count": len(databases), "databases": databases}


def run_cli(impl: Impl, sandbox: Sandbox, argv: list[str], env: dict[str, Any],
            timeout: float = 30.0, probe_port: bool = False) -> dict:
    """Run a one-shot CLI invocation and observe exit, output, and listening."""
    proc = subprocess.Popen(
        [str(impl.binary)] + [sandbox.expand(a) for a in argv],
        env=sandbox.env_for(env),
        cwd=sandbox.root,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        stdin=subprocess.DEVNULL,
    )
    listened = False
    deadline = time.monotonic() + timeout
    if probe_port:
        while proc.poll() is None and time.monotonic() < deadline:
            if port_open(sandbox.port):
                listened = True
                break
            time.sleep(0.005)
    try:
        out, err = proc.communicate(timeout=max(0.1, deadline - time.monotonic()))
        timed_out = False
    except subprocess.TimeoutExpired:
        proc.kill()
        out, err = proc.communicate()
        timed_out = True
    result = {
        "exit": proc.returncode,
        "timedOut": timed_out,
        "stdout": out.decode(errors="replace"),
        "stderr": _strip_log_prefix(err.decode(errors="replace")),
    }
    if probe_port:
        result["listenedBeforeExit"] = listened
    return result


def _strip_log_prefix(text: str) -> str:
    """Drop the Go slog `time=...` attribute; the timestamp is not contract."""
    lines = []
    for line in text.splitlines():
        if line.startswith("time="):
            _, _, rest = line.partition(" ")
            line = rest
        lines.append(line)
    return "\n".join(lines)


class Server:
    def __init__(self, impl: Impl, sandbox: Sandbox, argv: list[str], env: dict[str, Any]):
        self.impl = impl
        self.sandbox = sandbox
        self.log_path = sandbox.root / "server.log"
        self._log = open(self.log_path, "wb")
        self.proc = subprocess.Popen(
            [str(impl.binary)] + [sandbox.expand(a) for a in argv],
            env=sandbox.env_for(env),
            cwd=sandbox.root,
            stdout=self._log,
            stderr=subprocess.STDOUT,
            stdin=subprocess.DEVNULL,
        )

    def wait_ready(self, timeout: float = 30.0, port: int | None = None) -> dict:
        deadline = time.monotonic() + timeout
        port = port or self.sandbox.port
        while time.monotonic() < deadline:
            if self.proc.poll() is not None:
                return {"ready": False, "exit": self.proc.returncode}
            if port_open(port):
                return {"ready": True}
            time.sleep(0.02)
        return {"ready": False, "timedOut": True}

    def stop(self, sig: int = signal.SIGINT, timeout: float = 10.0) -> dict:
        if self.proc.poll() is None:
            self.proc.send_signal(sig)
            try:
                self.proc.wait(timeout=timeout)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                self.proc.wait()
                self._log.close()
                return {"exit": self.proc.returncode, "killed": True}
        self._log.close()
        return {"exit": self.proc.returncode, "killed": False}


def _proc_listen_inodes() -> dict[str, str]:
    """inode -> "addr:port" for every LISTEN TCP socket in this netns."""
    out: dict[str, str] = {}
    for table, v6 in (("/proc/net/tcp", False), ("/proc/net/tcp6", True)):
        try:
            lines = Path(table).read_text().splitlines()[1:]
        except OSError:
            continue
        for line in lines:
            parts = line.split()
            if len(parts) < 10 or parts[3] != "0A":
                continue
            host_hex, port_hex = parts[1].split(":")
            raw = bytes.fromhex(host_hex)
            if v6:
                words = [raw[i:i + 4][::-1] for i in range(0, 16, 4)]
                import ipaddress
                addr = "[" + str(ipaddress.IPv6Address(b"".join(words))) + "]"
            else:
                addr = ".".join(str(b) for b in raw[::-1])
            out[parts[9]] = f"{addr}:{int(port_hex, 16)}"
    return out


def listeners_of(pid: int) -> list[str]:
    """Sorted LISTEN addresses held by a process (Go and Rust alike)."""
    inodes = _proc_listen_inodes()
    found = set()
    try:
        for fd in Path(f"/proc/{pid}/fd").iterdir():
            try:
                target = os.readlink(fd)
            except OSError:
                continue
            if target.startswith("socket:["):
                inode = target[8:-1]
                if inode in inodes:
                    found.add(inodes[inode])
    except OSError:
        pass
    return sorted(found)


def modern_meta(client_name: str = "opute-parity", client_version: str = "1") -> dict:
    return {
        "io.modelcontextprotocol/protocolVersion": PROTOCOL_VERSION,
        "io.modelcontextprotocol/clientInfo": {"name": client_name, "version": client_version},
        "io.modelcontextprotocol/clientCapabilities": {
            "extensions": {"io.modelcontextprotocol/tasks": {}}
        },
    }


def http_request(sandbox: Sandbox, method: str, path: str, headers: dict[str, str] | None = None,
                 body: bytes | None = None, timeout: float = 300.0) -> dict:
    conn = http.client.HTTPConnection("127.0.0.1", sandbox.port, timeout=timeout)
    try:
        conn.request(method, path, body=body, headers=headers or {})
        resp = conn.getresponse()
        raw = resp.read()
        hdrs = {k.lower(): v for k, v in resp.getheaders() if k.lower() in HEADER_ALLOWLIST}
        return {"status": resp.status, "headers": hdrs, "body": _decode_body(hdrs, raw)}
    finally:
        conn.close()


def raw_request(sandbox: Sandbox, data: bytes, timeout: float = 10.0) -> dict:
    """Send exact bytes and parse every HTTP/1.x response until the server
    closes (pipelining, 100 Continue, keep-alive). Framing is part of the
    contract here, so every header except Date is recorded, in order."""
    with socket.create_connection(("127.0.0.1", sandbox.port), timeout=timeout) as conn:
        # No half-close: Go treats a client EOF as a cancelled request.
        # Every raw case ends with "Connection: close" or a request the
        # server rejects, so the server closes first.
        conn.sendall(data)
        chunks = []
        try:
            while True:
                chunk = conn.recv(65536)
                if not chunk:
                    break
                chunks.append(chunk)
        except (socket.timeout, ConnectionResetError):
            chunks.append(b"<timeout>")
    stream = b"".join(chunks)
    responses = []
    while stream:
        head, sep, rest = stream.partition(b"\r\n\r\n")
        lines = head.decode(errors="replace").split("\r\n")
        parts = lines[0].split(" ", 2) if lines and lines[0] else []
        if not sep or len(parts) < 2 or not parts[1].isdigit():
            responses.append({"unparsed": stream.decode(errors="replace")})
            break
        headers = []
        length = None
        chunked = False
        for line in lines[1:]:
            key, _, value = line.partition(":")
            value = value.strip()
            if key.lower() == "date":
                continue
            headers.append([key, value])
            if key.lower() == "content-length" and value.isdigit():
                length = int(value)
            if key.lower() == "transfer-encoding" and "chunked" in value.lower():
                chunked = True
        status = int(parts[1])
        no_body = status in (204, 304) or 100 <= status < 200
        if no_body:
            body, stream = b"", rest
        elif chunked:
            body, stream = _dechunk_prefix(rest)
        elif length is not None:
            body, stream = rest[:length], rest[length:]
        else:
            body, stream = rest, b""
        hdrs = {k.lower(): v for k, v in headers}
        responses.append({"statusLine": lines[0], "headers": headers,
                          "body": _decode_body(hdrs, body)})
    if len(responses) == 1:
        return responses[0]
    return {"responses": responses}


def _dechunk_prefix(data: bytes) -> tuple[bytes, bytes]:
    out = bytearray()
    while data:
        size_line, _, rest = data.partition(b"\r\n")
        try:
            size = int(size_line.split(b";")[0], 16)
        except ValueError:
            return bytes(out), b""
        if size == 0:
            _, _, after = rest.partition(b"\r\n")
            return bytes(out), after
        out += rest[:size]
        data = rest[size + 2:]
    return bytes(out), b""


def run_sql(path: Path, statements: list[str]) -> dict:
    """Seed a SQLite file the agent owns (only while the agent is stopped)."""
    conn = sqlite3.connect(path)
    try:
        for statement in statements:
            conn.execute(statement)
        conn.commit()
    finally:
        conn.close()
    return {"sql": len(statements)}


def _decode_body(headers: dict[str, str], raw: bytes) -> Any:
    text = raw.decode(errors="replace")
    ctype = headers.get("content-type", "")
    if ctype.startswith("text/event-stream"):
        events = []
        for line in text.splitlines():
            if line.startswith("data:"):
                payload = line[5:].strip()
                try:
                    events.append(json.loads(payload))
                except ValueError:
                    events.append(payload)
        return {"$sse": events}
    try:
        return json.loads(text)
    except ValueError:
        return text


def mcp_call(sandbox: Sandbox, method: str, params: dict | None = None, *,
             token: str | None = "${TOKEN}", name: str | None = None,
             modern: bool = True, headers: dict[str, str] | None = None,
             omit_headers: list[str] | None = None, request_id: Any = 1,
             meta: dict | None = None) -> dict:
    """One JSON-RPC request over Streamable HTTP, as a 2026-07-28 client sends it.

    `meta` adds keys (for example catalogRevision) to the modern _meta."""
    params = dict(params or {})
    if modern:
        params.setdefault("_meta", modern_meta())
    if meta:
        params["_meta"] = {**params.get("_meta", {}), **meta}
    envelope = {"jsonrpc": "2.0", "method": method, "params": params}
    if request_id is not None:
        envelope["id"] = request_id
    hdrs = {
        "Content-Type": "application/json",
        "Accept": "application/json, text/event-stream",
    }
    if modern:
        hdrs["MCP-Protocol-Version"] = PROTOCOL_VERSION
        hdrs["Mcp-Method"] = method
        if name:
            hdrs["Mcp-Name"] = name
    if token:
        hdrs["Authorization"] = "Bearer " + sandbox.expand(token)
    for key, value in (headers or {}).items():
        hdrs[key] = sandbox.expand(value)
    for key in omit_headers or []:
        hdrs.pop(key, None)
    return http_request(sandbox, "POST", "/mcp", hdrs, json.dumps(envelope).encode())
