"""T2 guest driver: runs one scenario program as root inside a twin VM.

Reads {"agent": {...}, "steps": [...]} as JSON on stdin and writes the
observation as JSON on stdout. Steps:

  {"as": L, "serve": {"env": {...}, "unset": [K...], "user": "ubuntu"|"root",
                      "manager": "system"|"user", "unit": U}}
      start the agent under test as a transient unit (default
      parity-agent.service in the system manager, running as that user;
      with manager "user", a unit of ubuntu's own user manager, which must
      already be running), then wait for /mcp.
  {"as": L, "mcp": {"method": M, "params": {...}, "name": N, "meta": {...},
                    "capture": {VAR: [path...]}, "until": {...}, "timeout": s}}
      one JSON-RPC request; "${VAR}" in params/name expands earlier captures.
  {"as": L, "shell": {"argv": [...], "user": "root"|"ubuntu", "stdin": s}}
      a setup or inspection command; exit code and output are recorded.
  {"as": L, "write": {"path": p, "content": s | "base64": b, "mode": "0644", "owner": "u:g"}}
  {"as": L, "snapshot": {"baseline": B}}
      the effect snapshot; with "baseline", units new or changed since the
      snapshot B are also described with `systemctl show`.
  {"as": L, "stop": {}}
      stop the agent unit.

Nothing here talks to anything outside the VM. Snapshots are returned in
full; the host computes deltas.
"""
from __future__ import annotations

import base64
import http.client
import json
import os
import re
import socket
import subprocess
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import snapshot as snap  # noqa: E402

PROTOCOL_VERSION = "2026-07-28"
AGENT_ROOT = "/opt/parity-t2"
UNIT = "parity-agent.service"
SERVED: dict = {}


def expand(value, variables):
    if isinstance(value, str):
        for key, val in variables.items():
            value = value.replace("${" + key + "}", str(val))
        return value
    if isinstance(value, list):
        return [expand(v, variables) for v in value]
    if isinstance(value, dict):
        return {k: expand(v, variables) for k, v in value.items()}
    return value


def lookup(doc, path):
    for key in path:
        if isinstance(doc, dict):
            doc = doc.get(key)
        elif isinstance(doc, list) and isinstance(key, int) and key < len(doc):
            doc = doc[key]
        else:
            return None
    return doc


def mcp(port, token, method, params, name=None, timeout=3600, meta=None):
    params = dict(params or {})
    params.setdefault("_meta", {
        "io.modelcontextprotocol/protocolVersion": PROTOCOL_VERSION,
        "io.modelcontextprotocol/clientInfo": {"name": "opute-parity-t2", "version": "1"},
        "io.modelcontextprotocol/clientCapabilities": {"extensions": {"io.modelcontextprotocol/tasks": {}}},
    })
    if meta:
        params["_meta"] = {**params["_meta"], **meta}
    headers = {"Content-Type": "application/json", "Accept": "application/json, text/event-stream",
               "MCP-Protocol-Version": PROTOCOL_VERSION, "Mcp-Method": method,
               "Authorization": "Bearer " + token}
    if name:
        headers["Mcp-Name"] = name
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params})
    conn = http.client.HTTPConnection("127.0.0.1", port, timeout=timeout)
    conn.request("POST", "/mcp", body, headers)
    response = conn.getresponse()
    raw = response.read()
    conn.close()
    text = raw.decode("utf-8", "replace")
    if response.getheader("Content-Type", "").startswith("text/event-stream"):
        events = [json.loads(line[5:].strip()) for line in text.splitlines() if line.startswith("data:")]
        return {"status": response.status, "body": {"$sse": events}}
    try:
        return {"status": response.status, "body": json.loads(text)}
    except ValueError:
        return {"status": response.status, "body": text}


def run(argv, user="root", stdin=None, timeout=3600):
    if user != "root":
        argv = ["setpriv", "--reuid=" + user, "--regid=" + user, "--init-groups"] + argv
    p = subprocess.run(argv, input=stdin, capture_output=True, text=True, timeout=timeout)
    # systemd-run reports a per-start invocation id; it identifies the run, not its effect.
    stderr = re.sub(r"invocation ID: [0-9a-f]{32}", "invocation ID: <id>", p.stderr)
    return {"exit": p.returncode, "stdout": p.stdout, "stderr": stderr}


def serve(spec, agent):
    env = dict(agent["env"])
    env.update(spec.get("env", {}))
    for key in spec.get("unset", []):
        env.pop(key, None)
    user = spec.get("user", "ubuntu")
    manager = spec.get("manager", "system")
    unit = spec.get("unit", UNIT)
    home = "/root" if user == "root" else "/home/" + user
    uid = 0 if user == "root" else snap.USER_UID
    env.setdefault("HOME", home)
    env.setdefault("USER", user)
    env.setdefault("PATH", "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin")
    env.setdefault("XDG_RUNTIME_DIR", f"/run/user/{uid}")
    argv = ["systemd-run", "--unit=" + unit, "--property=Type=simple", "--property=KillMode=control-group",
            "--collect"]
    if manager == "user":
        argv.insert(1, "--user")
    else:
        argv += [f"--uid={user}", f"--gid={user}", "--working-directory=" + home]
    for key, value in sorted(env.items()):
        argv.append(f"--setenv={key}={value}")
    argv += [AGENT_ROOT + "/agent"] + spec.get("argv", ["serve", "--mode=standalone"])
    if manager == "user":
        started = run(["env", f"XDG_RUNTIME_DIR=/run/user/{uid}",
                       f"DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/{uid}/bus"] + argv, user)
    else:
        started = run(argv)
    SERVED.update(unit=unit, manager=manager, user=user, uid=uid)
    port = int(env["HOST_MCP_PORT"])
    deadline = time.monotonic() + 30
    ready = False
    while time.monotonic() < deadline:
        try:
            with socket.create_connection(("127.0.0.1", port), timeout=1):
                ready = True
                break
        except OSError:
            time.sleep(0.2)
    return {"started": started["exit"] == 0, "ready": ready, "stderr": started["stderr"]}


def stop():
    if SERVED.get("manager") == "user":
        uid = SERVED["uid"]
        return run(["env", f"XDG_RUNTIME_DIR=/run/user/{uid}", f"DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/{uid}/bus",
                    "systemctl", "--user", "stop", SERVED["unit"]], SERVED["user"])
    return run(["systemctl", "stop", SERVED.get("unit", UNIT)])


def main():
    program = json.load(sys.stdin)
    agent = program["agent"]
    port = int(agent["env"]["HOST_MCP_PORT"])
    token = agent["env"]["MCP_AUTH_TOKEN"]
    variables = {}
    snapshots = {}
    steps = {}
    for index, step in enumerate(program["steps"]):
        label = step.get("as", f"step{index}")
        try:
            if "serve" in step:
                steps[label] = serve(step["serve"], agent)
            elif "mcp" in step:
                spec = expand(step["mcp"], variables)
                unbound = sorted(set(re.findall(r"\$\{([A-Z0-9_]+)\}", json.dumps(spec))))
                if unbound:
                    # The step that should have captured it answered without one
                    # (for example a synchronous refusal); that answer is recorded.
                    steps[label] = {"unbound": unbound}
                    continue
                deadline = time.monotonic() + spec.get("timeout", 30)
                while True:
                    result = mcp(port, token, spec["method"], spec.get("params"), spec.get("name"),
                                 meta=spec.get("meta"))
                    until = spec.get("until")
                    if not until or lookup(result, until["path"]) in until["in"] or time.monotonic() > deadline:
                        break
                    if isinstance(result.get("body"), dict) and "error" in result["body"]:
                        break  # a JSON-RPC error never turns into a terminal task status
                    time.sleep(0.25)
                steps[label] = result
                for var, path in spec.get("capture", {}).items():
                    value = lookup(result, path)
                    if value is not None:
                        variables[var] = value
            elif "shell" in step:
                spec = expand(step["shell"], variables)
                steps[label] = run(spec["argv"], spec.get("user", "root"), spec.get("stdin"))
            elif "write" in step:
                spec = step["write"]
                os.makedirs(os.path.dirname(spec["path"]), exist_ok=True)
                data = base64.b64decode(spec["base64"]) if "base64" in spec else spec.get("content", "").encode()
                with open(spec["path"], "wb") as f:
                    f.write(data)
                os.chmod(spec["path"], int(spec.get("mode", "0644"), 8))
                if spec.get("owner"):
                    subprocess.run(["chown", "-h", spec["owner"], spec["path"]], check=True)
                steps[label] = {"wrote": spec["path"]}
            elif "snapshot" in step:
                current = snap.snapshot()
                base = snapshots.get(step["snapshot"].get("baseline", ""))
                for scope in ("system", "user"):
                    if not current[scope].get("running"):
                        continue
                    before = (base or {}).get(scope, {})
                    names = set()
                    for kind in ("unitFiles", "units"):
                        old = before.get(kind, {}) if before.get("running") else {}
                        for unit, value in current[scope][kind].items():
                            if unit != UNIT and (unit not in old or old[unit] != value):
                                names.add(unit)
                    current[scope]["show"] = snap.show(sorted(names), scope == "user")
                snapshots[label] = current
                steps[label] = {"snapshot": current}
            elif "stop" in step:
                steps[label] = stop()
            else:
                steps[label] = {"error": "unknown step"}
        except Exception as e:  # recorded, never fatal: later steps still run
            steps[label] = {"exception": f"{type(e).__name__}: {e}"}
    json.dump({"steps": steps, "variables": variables}, sys.stdout)


if __name__ == "__main__":
    main()
