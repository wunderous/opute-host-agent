#!/usr/bin/env python3
"""M10 clean-machine journey, run as root inside a disposable twin.

Reads a program (JSON) on stdin and prints one observation document. The twin
holds the release sets under /opt/releases/<name>/v<version>/ and Go's deploy
files under /opt/deploy/; nothing else of Opute is on the machine. Every step
does what an operator (or the Platform's generated installer) does with the
published files, so two twins that differ only in their release set differ
only where the artifacts do.

Steps:
  serve            serve /opt/releases on the guest's loopback
  userInstall      download, verify and unpack the release binary for a user,
                   write host-agent.env, install deploy/opute-host-agent.service
                   and enable it under the user's systemd manager
  swap             replace an installed binary from another release set,
                   keeping its env file and state (upgrade or downgrade)
  bootstrapInstall install deploy/opute-bootstrap-mcp.service and the sudoers
                   file for the opute user, and start the system unit
  start / stop     systemctl start or stop a unit (user or system scope)
  health           wait for GET /health and record it
  firstCall        authenticated read-only MCP first success
  unit             the unit's state
  tree             files, modes and owners under the given roots
  journal          the unit's journal lines, normalized
  state            the standalone state directory's SQLite tables and rows
  uninstall        remove what the install step installed
"""
import gzip
import hashlib
import json
import os
import pwd
import re
import sqlite3
import stat
import subprocess
import sys
import time
import urllib.error
import urllib.request

SERVE_PORT = 8765
USER_UNIT = "opute-host-agent.service"
SYSTEM_UNIT = "opute-bootstrap-mcp.service"


def sh(argv, check=True, **kw):
    done = subprocess.run(argv, capture_output=True, text=True, **kw)
    if check and done.returncode != 0:
        raise RuntimeError(f"{argv!r} exited {done.returncode}: {done.stderr.strip()[-800:]}")
    return done


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def ensure_user(name):
    try:
        entry = pwd.getpwnam(name)
    except KeyError:
        sh(["useradd", "-m", "-s", "/bin/bash", name])
        entry = pwd.getpwnam(name)
    sh(["loginctl", "enable-linger", name])
    uid = entry.pw_uid
    deadline = time.monotonic() + 60
    while time.monotonic() < deadline:
        # systemctl --user reaches the manager over its private socket; a
        # session bus exists only where dbus-user-session is installed.
        if sh(["systemctl", "is-active", "--quiet", f"user@{uid}.service"], check=False).returncode == 0 \
                and os.path.exists(f"/run/user/{uid}/systemd/private"):
            return entry
        time.sleep(0.5)
    raise RuntimeError(f"user manager for {name} did not start")


def as_user(entry, argv, check=True, input_=None):
    env = ["env", f"XDG_RUNTIME_DIR=/run/user/{entry.pw_uid}", f"HOME={entry.pw_dir}"]
    if os.path.exists(f"/run/user/{entry.pw_uid}/bus"):
        env.append(f"DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/{entry.pw_uid}/bus")
    return sh(["runuser", "-u", entry.pw_name, "--"] + env + argv, check=check, input=input_)


def fetch(url):
    with urllib.request.urlopen(url, timeout=30) as response:
        return response.read()


def download(release, version, artifact):
    """Download an artifact and verify it against the release's SHA256SUMS."""
    base = f"http://127.0.0.1:{SERVE_PORT}/{release}/v{version}"
    sums = fetch(f"{base}/SHA256SUMS").decode()
    expected = None
    for line in sums.splitlines():
        match = re.match(r"^([a-f0-9]{64})\s+[* ]?(.+)$", line.strip())
        if match and os.path.basename(match.group(2)) == artifact:
            expected = match.group(1)
    archive = fetch(f"{base}/{artifact}")
    verified = expected is not None and sha256(archive) == expected
    if not verified:
        raise RuntimeError(f"{artifact} from {release}: checksum not verified")
    binary = gzip.decompress(archive)
    return binary, {"artifact": artifact, "listed": expected is not None, "verified": verified,
                    "archiveSha256": sha256(archive), "binarySha256": sha256(binary)}


def write_file(path, data, mode, owner=None):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    temporary = path + ".partial"
    with open(temporary, "wb") as f:
        f.write(data)
    os.chmod(temporary, mode)
    if owner:
        os.chown(temporary, owner.pw_uid, owner.pw_gid)
    os.replace(temporary, path)


def mkdirs(path, owner, mode=0o755):
    parts = []
    while not os.path.isdir(path):
        parts.append(path)
        path = os.path.dirname(path)
    for directory in reversed(parts):
        os.mkdir(directory, mode)
        os.chown(directory, owner.pw_uid, owner.pw_gid)


def env_file(values):
    return "".join(f"{key}={value}\n" for key, value in values.items()).encode()


def user_paths(entry):
    home = entry.pw_dir
    return {"binary": f"{home}/.local/share/opute/opute-host-agent",
            "env": f"{home}/.config/opute/host-agent.env",
            "unit": f"{home}/.config/systemd/user/{USER_UNIT}"}


def step_user_install(spec, ctx):
    entry = ensure_user(spec.get("user", "opute"))
    paths = user_paths(entry)
    binary, receipt = download(spec["release"], spec["version"], spec.get("artifact", "host-agent-linux-x64.gz"))
    for directory in (os.path.dirname(paths["binary"]), os.path.dirname(paths["env"]),
                      os.path.dirname(paths["unit"])):
        mkdirs(directory, entry)
    write_file(paths["binary"], binary, 0o755, entry)
    write_file(paths["env"], env_file(spec["env"]), 0o600, entry)
    with open("/opt/deploy/opute-host-agent.service", "rb") as f:
        write_file(paths["unit"], f.read(), 0o644, entry)
    as_user(entry, ["systemctl", "--user", "daemon-reload"])
    enabled = as_user(entry, ["systemctl", "--user", "enable", "--now", USER_UNIT], check=False)
    ctx["users"][entry.pw_name] = paths
    return {**receipt, "enable": {"exit": enabled.returncode, "stderr": normalize(enabled.stderr)}}


def step_swap(spec, ctx):
    entry = pwd.getpwnam(spec.get("user", "opute"))
    paths = user_paths(entry) if spec.get("scope", "user") == "user" else {
        "binary": f"{entry.pw_dir}/.local/share/opute/opute-host-agent"}
    binary, receipt = download(spec["release"], spec["version"], spec.get("artifact", "host-agent-linux-x64.gz"))
    write_file(paths["binary"], binary, 0o755, entry)
    return receipt


def step_bootstrap_install(spec, ctx):
    entry = ensure_user("opute")
    binary_path = f"{entry.pw_dir}/.local/share/opute/opute-host-agent"
    binary, receipt = download(spec["release"], spec["version"], spec.get("artifact", "host-agent-linux-x64.gz"))
    mkdirs(os.path.dirname(binary_path), entry)
    write_file(binary_path, binary, 0o755, entry)
    with open("/opt/deploy/opute-bootstrap-mcp.service", "rb") as f:
        write_file(f"/etc/systemd/system/{SYSTEM_UNIT}", f.read(), 0o644)
    with open("/opt/deploy/opute-host-agent-sudoers", "rb") as f:
        write_file("/etc/sudoers.d/opute-host-agent", f.read(), 0o440)
    if spec.get("dropIn"):
        write_file(f"/etc/systemd/system/{SYSTEM_UNIT}.d/identity.conf",
                   ("[Service]\n" + "".join(f"Environment={k}={v}\n" for k, v in spec["dropIn"].items())).encode(),
                   0o644)
    visudo = sh(["visudo", "-cf", "/etc/sudoers.d/opute-host-agent"], check=False)
    sh(["systemctl", "daemon-reload"])
    enabled = sh(["systemctl", "enable", "--now", SYSTEM_UNIT], check=False)
    return {**receipt, "visudo": visudo.returncode,
            "enable": {"exit": enabled.returncode, "stderr": normalize(enabled.stderr)}}


def systemctl(spec, verb):
    if spec.get("scope", "user") == "user":
        entry = pwd.getpwnam(spec.get("user", "opute"))
        done = as_user(entry, ["systemctl", "--user", verb, USER_UNIT], check=False)
    else:
        done = sh(["systemctl", verb, SYSTEM_UNIT], check=False)
    return {"exit": done.returncode, "stderr": normalize(done.stderr)}


def unit_state(spec):
    props = ["LoadState", "ActiveState", "SubState", "UnitFileState", "Result", "ExecMainCode",
             "ExecMainStatus", "NRestarts", "Type", "Restart", "FragmentPath", "User"]
    argv = ["systemctl", "show", "-p", ",".join(props + ["MainPID"])]
    if spec.get("scope", "user") == "user":
        entry = pwd.getpwnam(spec.get("user", "opute"))
        done = as_user(entry, ["systemctl", "--user", "show", "-p", ",".join(props + ["MainPID"]), USER_UNIT])
    else:
        done = sh(argv + [SYSTEM_UNIT])
    values = dict(line.split("=", 1) for line in done.stdout.splitlines() if "=" in line)
    values["MainPIDRunning"] = values.pop("MainPID", "0") != "0"
    return values


def http_get(url, timeout=3):
    try:
        with urllib.request.urlopen(url, timeout=timeout) as response:
            return response.status, response.read()
    except urllib.error.HTTPError as error:
        return error.code, error.read()


def step_health(spec, ctx):
    url = f"http://127.0.0.1:{spec.get('port', 3014)}/health"
    deadline = time.monotonic() + spec.get("timeout", 60)
    last = None
    while time.monotonic() < deadline:
        try:
            status, body = http_get(url)
            if status == 200:
                payload = json.loads(body)
                if payload.get("ok") is True:
                    return {"status": status, "body": payload}
            last = status
        except (OSError, ValueError) as error:
            last = type(error).__name__
        time.sleep(0.5)
    return {"status": None, "last": str(last)}


def mcp(port, token, method, params=None, request_id=1):
    body = {"jsonrpc": "2.0", "method": method, "id": request_id, "params": {
        **(params or {}), "_meta": {
            "io.modelcontextprotocol/protocolVersion": "2026-07-28",
            "io.modelcontextprotocol/clientCapabilities": {},
            "io.modelcontextprotocol/clientInfo": {"name": "opute-parity-m10", "version": "1"}}}}
    url = f"http://127.0.0.1:{port}/mcp"
    headers = {"Accept": "application/json, text/event-stream", "Content-Type": "application/json",
               "MCP-Protocol-Version": "2026-07-28", "Mcp-Method": method,
               "Origin": f"http://127.0.0.1:{port}"}
    if token:
        headers["Authorization"] = f"Bearer {token}"
    if method == "tools/call":
        headers["Mcp-Name"] = params["name"]
    request = urllib.request.Request(url, data=json.dumps(body).encode(), headers=headers, method="POST")
    try:
        with urllib.request.urlopen(request, timeout=60) as response:
            raw = response.read()
            status = response.status
    except urllib.error.HTTPError as error:
        raw, status = error.read(), error.code
    except OSError as error:
        return None, {"connection": type(getattr(error, "reason", error)).__name__}
    try:
        return status, json.loads(raw)
    except ValueError:
        return status, {"raw": raw.decode(errors="replace")[:400]}


def step_first_call(spec, ctx):
    port, token = spec.get("port", 3014), spec.get("token")
    out = {}
    if token:
        status, _ = mcp(port, "wrong-token-for-parity-m10", "server/discover")
        out["wrongToken"] = status
    status, discover = mcp(port, token, "server/discover", request_id=2)
    out["discover"] = {"status": status, "resultType": (discover.get("result") or {}).get("resultType"),
                       "error": discover.get("error")}
    status, listed = mcp(port, token, "tools/list", request_id=3)
    tools = (listed.get("result") or {}).get("tools") or []
    out["toolsList"] = {"status": status, "count": len(tools), "error": listed.get("error"),
                        "hasHostInfo": any(t.get("name") == "get_host_info" for t in tools)}
    status, called = mcp(port, token, "tools/call", {"name": "get_host_info", "arguments": {}}, request_id=4)
    result = called.get("result") or {}
    content = result.get("structuredContent")
    out["getHostInfo"] = {"status": status, "isError": result.get("isError", False), "error": called.get("error"),
                          "keys": sorted(content) if isinstance(content, dict) else None}
    status, catalog = mcp(port, token, "tools/call", {"name": "get_capability_catalog", "arguments": {}},
                          request_id=5)
    structured = (catalog.get("result") or {}).get("structuredContent") or {}
    out["catalog"] = {"status": status, "revision": structured.get("catalogRevision"),
                      "tools": len(structured.get("tools") or [])}
    return out


def walk(root):
    """Every path under root, dot-directories included (glob skips them)."""
    out = [root]
    for directory, dirs, files in os.walk(root):
        dirs.sort()
        out += [os.path.join(directory, name) for name in sorted(dirs + files)]
    return sorted(set(out))


def step_tree(spec, ctx):
    files = {}
    for root in spec["roots"]:
        for path in walk(root):
            if not os.path.lexists(path):
                continue
            st = os.lstat(path)
            entry = {"mode": oct(stat.S_IMODE(st.st_mode)),
                     "type": "dir" if stat.S_ISDIR(st.st_mode) else "link" if stat.S_ISLNK(st.st_mode) else "file",
                     "owner": pwd.getpwuid(st.st_uid).pw_name}
            if entry["type"] == "file" and path not in spec.get("noHash", []) \
                    and not any(re.search(p, path) for p in spec.get("noHashPatterns", [])):
                with open(path, "rb") as f:
                    entry["sha256"] = sha256(f.read())
            files[path] = entry
    return files


# Run-local values in journal lines: times, durations, process ids, ports
# the kernel picked, random identifiers and memory amounts.
_JOURNAL_RULES = [
    (re.compile(r"\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(?:\.\d+)?(?:Z|[+-]\d\d:\d\d)"), "<time>"),
    (re.compile(r"\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b"), "<uuid>"),
    (re.compile(r"\b\d+(?:\.\d+)?(?:ns|µs|us|ms|s)\b"), "<duration>"),
    (re.compile(r"\bpid[= ]\d+\b"), "pid=<pid>"),
    (re.compile(r"\(\d+\)"), "(<n>)"),
    # systemd's per-unit accounting line ("Consumed ... CPU time, 20.4M
    # memory peak, 0B memory swap peak"): the amounts are the run's own.
    (re.compile(r"\b\d+(?:\.\d+)?[BKMGT] memory (swap )?peak"), r"<bytes> memory \1peak"),
]


def normalize(text):
    for pattern, replacement in _JOURNAL_RULES:
        text = pattern.sub(replacement, text)
    return text


def step_journal(spec, ctx):
    if spec.get("scope", "user") == "user":
        entry = pwd.getpwnam(spec.get("user", "opute"))
        argv = ["journalctl", f"_UID={entry.pw_uid}", f"_SYSTEMD_USER_UNIT={USER_UNIT}", "-o", "cat",
                "--no-pager"]
    else:
        argv = ["journalctl", "-u", SYSTEM_UNIT, "-o", "cat", "--no-pager"]
    sh(["journalctl", "--sync"], check=False)
    lines = sh(argv, check=False).stdout.splitlines()
    # Only the lines since this scope's last journal step.
    seen = ctx.setdefault("journalSeen", {})
    key = spec.get("scope", "user")
    fresh, seen[key] = lines[seen.get(key, 0):], len(lines)
    return [normalize(line) for line in fresh]


def step_state(spec, ctx):
    out = {}
    for path in walk(spec["root"]):
        if not os.path.isfile(path) or os.path.islink(path):
            continue
        try:
            with open(path, "rb") as f:
                head = f.read(16)
        except OSError:
            continue
        if head != b"SQLite format 3\x00":
            continue
        db = sqlite3.connect(f"file:{path}?mode=ro", uri=True)
        try:
            tables = [r[0] for r in db.execute("select name from sqlite_master where type='table' order by name")]
            out[path] = {t: db.execute(f'select count(*) from "{t}"').fetchone()[0] for t in tables}
        finally:
            db.close()
    return out


def step_uninstall(spec, ctx):
    if spec.get("scope", "user") == "user":
        entry = pwd.getpwnam(spec.get("user", "opute"))
        paths = user_paths(entry)
        disabled = as_user(entry, ["systemctl", "--user", "disable", "--now", USER_UNIT], check=False)
        for key in ("unit", "binary", "env"):
            if os.path.exists(paths[key]):
                os.unlink(paths[key])
        as_user(entry, ["systemctl", "--user", "daemon-reload"])
    else:
        disabled = sh(["systemctl", "disable", "--now", SYSTEM_UNIT], check=False)
        for path in (f"/etc/systemd/system/{SYSTEM_UNIT}", f"/etc/systemd/system/{SYSTEM_UNIT}.d/identity.conf",
                     "/etc/sudoers.d/opute-host-agent", "/home/opute/.local/share/opute/opute-host-agent"):
            if os.path.exists(path):
                os.unlink(path)
        if os.path.isdir(f"/etc/systemd/system/{SYSTEM_UNIT}.d"):
            os.rmdir(f"/etc/systemd/system/{SYSTEM_UNIT}.d")
        sh(["systemctl", "daemon-reload"])
    return {"exit": disabled.returncode, "stderr": normalize(disabled.stderr)}


def step_serve(spec, ctx):
    ctx["server"] = subprocess.Popen(
        ["python3", "-m", "http.server", "--bind", "127.0.0.1", str(SERVE_PORT), "--directory", "/opt/releases"],
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    deadline = time.monotonic() + 20
    while time.monotonic() < deadline:
        try:
            http_get(f"http://127.0.0.1:{SERVE_PORT}/")
            return {"port": SERVE_PORT}
        except OSError:
            time.sleep(0.2)
    raise RuntimeError("release server did not start")


STEPS = {
    "serve": step_serve, "userInstall": step_user_install, "swap": step_swap,
    "bootstrapInstall": step_bootstrap_install,
    "start": lambda spec, ctx: systemctl(spec, "start"), "stop": lambda spec, ctx: systemctl(spec, "stop"),
    "health": step_health, "firstCall": step_first_call, "unit": lambda spec, ctx: unit_state(spec),
    "tree": step_tree, "journal": step_journal, "state": step_state, "uninstall": step_uninstall,
    "sleep": lambda spec, ctx: time.sleep(spec["seconds"]) or {},
}


def main():
    program = json.load(sys.stdin)
    ctx = {"users": {}}
    steps = {}
    try:
        for step in program["steps"]:
            (kind, spec), = [(k, v) for k, v in step.items() if k != "as"]
            steps[step["as"]] = STEPS[kind](spec, ctx)
    except Exception as error:  # the observation records where it stopped
        steps["_error"] = f"{type(error).__name__}: {error}"
    finally:
        if ctx.get("server"):
            ctx["server"].terminate()
    json.dump({"steps": steps}, sys.stdout, indent=1, sort_keys=True)


if __name__ == "__main__":
    main()
