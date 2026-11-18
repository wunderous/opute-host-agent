#!/usr/bin/env python3
"""Where the agent's secrets go (m19), run as root inside the dev VM.

  secretsweep.py plant --env FILE --markers FILE [--set K=V]... [--setting K=V]... [--marker NAME=V]...
      the agent's env file (0600 root) holding --set and --setting lines, and the markers
      file (0600 root): every secret by class name, the --set values under
      their keys' names and the --marker values under theirs
  secretsweep.py agent start|stop NAME --port P --env FILE
      a transient system unit e2e-sweep-NAME running the installed release
      binary as root with EnvironmentFile=FILE (no secret on its command line
      or in its unit), its instance root, state and HOME under
      /tmp/e2e-agents/NAME; waits for /health
  secretsweep.py watch start|stop --unit UNIT
      start: strace -f of every execve by the unit's main process and its
      descendants (whole arguments), and a bpftrace of every execve on the
      host (each argument up to 200 bytes), both into root-only logs; stop:
      end both
  secretsweep.py bridge start|stop --port P
      a loopback stand-in for the bridge's cluster-agent artifact route
  secretsweep.py mark --credential FILE --as NAME --markers FILE
      the client secret in an agent's credential file joins the markers
  secretsweep.py oauth --port P --client ID --secret NAME --as NAME --markers FILE [--basic]
      the client_credentials grant with the secret named NAME (in the form,
      or as client_secret_basic), then tools/list with the issued token,
      which joins the markers as --as
  secretsweep.py scan --markers FILE --unit UNIT --root DIR... --state DIR [--holder FILE]...
      every marker, raw and base64-encoded at each alignment, in: the
      command lines recorded by watch, every process's environment, the
      whole journal, and every file under each --root (with its mode and
      owner; the markers' own directory excepted). A hit under --state is
      a store hit unless the file is a --holder (a credential file)

Values never leave this tool: it prints marker names, counts, paths, modes
and program names only. Each command prints one JSON document on stdout.
"""
import argparse
import base64
import json
import os
import re
import stat
import subprocess
import sys
import time
import urllib.parse
import urllib.request

BINARY = "/home/opute/.local/share/opute/opute-host-agent"
ROOT = "/tmp/e2e-agents"
WORK = "/root/e2e-sweep"
EXEC_LOG = f"{WORK}/exec.log"
TRACE_LOG = f"{WORK}/execve.log"
BRIDGE = f"{WORK}/bridge"
SKIP_DIRS = ("/proc", "/sys", "/dev", "/var/lib/incus", "/var/lib/snapd", "/var/lib/lxcfs", "/snap")
MAX_FILE = 64 * 1024 * 1024
PROTOCOL = "2026-07-28"


def sh(argv, check=True):
    done = subprocess.run(argv, capture_output=True, text=True)
    if check and done.returncode != 0:
        raise SystemExit(json.dumps({"argv": argv[:3], "exit": done.returncode, "stderr": done.stderr[-2000:]}))
    return done


def private_write(path, text):
    os.makedirs(os.path.dirname(path), mode=0o700, exist_ok=True)
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w") as f:
        f.write(text)
    os.chmod(path, 0o600)


def load_markers(path):
    return json.load(open(path))


def forms(value):
    """The bytes a marker can appear as: raw, and base64 at each of the three
    alignments (the encoded run that depends on the marker's bytes alone)."""
    raw = value.encode()
    out = [raw]
    for k in range(3):
        enc = base64.b64encode(b"\0" * k + raw)
        lead = 4 if k else 0
        tail = 4 if (k + len(raw)) % 3 else 0
        middle = enc[lead:len(enc) - tail]
        if len(middle) >= 16:
            out.append(middle)
    return out


def hits(data, markers):
    """Marker names whose value, in any form, is in data."""
    return sorted(name for name, value in markers.items() if any(f in data for f in forms(value)))


# --- plant and agent ---------------------------------------------------------------------------------

def plant(a):
    sets = dict(kv.split("=", 1) for kv in a.set)
    settings = dict(kv.split("=", 1) for kv in a.setting)
    markers = dict(kv.split("=", 1) for kv in a.marker)
    markers.update(sets)
    private_write(a.env, "".join(f"{k}={v}\n" for k, v in {**settings, **sets}.items()))
    private_write(a.markers, json.dumps(markers))
    return {"env": a.env, "envMode": oct(os.stat(a.env).st_mode & 0o777), "markers": sorted(markers)}


def health(port):
    try:
        with urllib.request.urlopen(f"http://127.0.0.1:{port}/health", timeout=2) as r:
            return json.loads(r.read())
    except Exception:
        return None


def agent(a):
    unit = f"e2e-sweep-{a.name}"
    if a.action == "stop":
        sh(["systemctl", "stop", f"{unit}.service"], check=False)
        sh(["systemctl", "reset-failed", f"{unit}.service"], check=False)
        sh(["rm", "-rf", f"{ROOT}/{a.name}"], check=False)
        if os.path.isdir(ROOT) and not os.listdir(ROOT):
            os.rmdir(ROOT)
        return {"stopped": True}
    root = f"{ROOT}/{a.name}"
    for d in ("instance", "state", "home"):
        os.makedirs(f"{root}/{d}", mode=0o700, exist_ok=True)
    env = {"HOST_MCP_PORT": str(a.port), "HOME": f"{root}/home", "OPUTE_HOST_AGENT_INSTANCE_ROOT": f"{root}/instance",
           "OPUTE_STANDALONE_STATE_DIR": f"{root}/state"}
    # As e2etool: 0.4.0 has no `--mode` and reads OPUTE_HOST_ALLOW_MUTATIONS
    # (platform-neutral-host-agent); 0.3.x takes both of the old ones.
    version = sh([a.binary, "--version"], check=False).stdout.strip()
    legacy = version.startswith(("0.2.", "0.3."))
    if not legacy and "OPUTE_STANDALONE_ALLOW_MUTATIONS=true\n" in open(a.env).read():
        env["OPUTE_HOST_ALLOW_MUTATIONS"] = "true"
    mode = ["--mode", "standalone"] if legacy else []
    sh(["systemd-run", f"--unit={unit}", "--property=Restart=no", "--collect", f"--property=EnvironmentFile={a.env}"]
       + [f"--setenv={k}={v}" for k, v in env.items()] + [a.binary, "serve", *mode, "--transport", "http"])
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        h = health(a.port)
        if h is not None:
            return {"unit": f"{unit}.service", "port": a.port, "ok": True}
        time.sleep(0.3)
    raise SystemExit(json.dumps({"error": "agent did not become healthy"}))


# --- watch -------------------------------------------------------------------------------------------

def main_pid(unit):
    pid = sh(["systemctl", "show", "-p", "MainPID", "--value", unit]).stdout.strip()
    if pid in ("", "0"):
        raise SystemExit(json.dumps({"error": f"{unit} has no main process"}))
    return pid


def watch(a):
    if a.action == "stop":
        for u in ("e2e-sweep-strace", "e2e-sweep-trace"):
            sh(["systemctl", "stop", f"{u}.service"], check=False)
            sh(["systemctl", "reset-failed", f"{u}.service"], check=False)
        return {"stopped": True, "execLog": os.path.exists(EXEC_LOG)}
    os.makedirs(WORK, mode=0o700, exist_ok=True)
    for path in (EXEC_LOG, TRACE_LOG):
        private_write(path, "")
    pid = main_pid(a.unit)
    sh(["systemd-run", "--unit=e2e-sweep-strace", "--collect", "strace", "-f", "-qq", "-s", "1048576", "-e",
        "trace=execve", "-o", EXEC_LOG, "-p", pid])
    # every execve on the host, each argument up to 200 bytes: the processes
    # systemd starts for the agent's units are outside its process tree
    sh(["systemd-run", "--unit=e2e-sweep-trace", "--collect", "--setenv=BPFTRACE_MAX_STRLEN=200",
        f"--property=StandardOutput=truncate:{TRACE_LOG}", "bpftrace", "-q", "-e",
        "BEGIN { printf(\"ready\\n\"); } tracepoint:syscalls:sys_enter_execve { join(args.argv, \" \"); }"])
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline and not os.path.getsize(TRACE_LOG):
        time.sleep(0.5)
    active = lambda u: sh(["systemctl", "is-active", u], check=False).stdout.strip() == "active"  # noqa: E731
    return {"strace": active("e2e-sweep-strace.service"), "bpftrace": active("e2e-sweep-trace.service")
            and os.path.getsize(TRACE_LOG) > 0, "agentPid": int(pid)}


# --- bridge ------------------------------------------------------------------------------------------

def bridge(a):
    """A loopback stand-in for the bridge's artifact route: the cluster
    agent it serves is a shell script that sleeps, so install_cluster_agent
    runs to the end and writes its config."""
    unit = "e2e-sweep-bridge"
    if a.action == "stop":
        sh(["systemctl", "stop", f"{unit}.service"], check=False)
        sh(["systemctl", "reset-failed", f"{unit}.service"], check=False)
        return {"stopped": True}
    import gzip
    for arch in ("x64", "arm64"):
        path = f"{BRIDGE}/artifacts/cluster-agent/{arch}.gz"
        os.makedirs(os.path.dirname(path), mode=0o700, exist_ok=True)
        with open(path, "wb") as f:
            f.write(gzip.compress(b"#!/bin/sh\nexec sleep infinity\n", mtime=0))
    sh(["systemd-run", f"--unit={unit}", "--collect", "python3", "-m", "http.server", str(a.port), "--bind",
        "127.0.0.1", "--directory", BRIDGE])
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        try:
            urllib.request.urlopen(f"http://127.0.0.1:{a.port}/artifacts/cluster-agent/x64.gz", timeout=2)
            return {"serving": True}
        except Exception:
            time.sleep(0.3)
    return {"serving": False}


# --- oauth -------------------------------------------------------------------------------------------

def mark(a):
    """A secret the agent generated, read from its credential file, joins the markers."""
    markers = load_markers(a.markers)
    cred = json.load(open(a.credential))
    markers[a.as_] = cred["client_secret"]
    private_write(a.markers, json.dumps(markers))
    return {"marked": a.as_, "client": cred["client_id"],
            "mode": oct(os.stat(a.credential).st_mode & 0o777), "uid": os.stat(a.credential).st_uid}


def oauth(a):
    markers = load_markers(a.markers)
    issuer = f"http://127.0.0.1:{a.port}"
    fields = {"grant_type": "client_credentials", "resource": f"{issuer}/mcp"}
    headers = {}
    if a.basic:
        pair = f"{urllib.parse.quote(a.client, safe='')}:{urllib.parse.quote(markers[a.secret], safe='')}"
        headers["Authorization"] = "Basic " + base64.b64encode(pair.encode()).decode()
    else:
        fields.update(client_id=a.client, client_secret=markers[a.secret])
    form = urllib.parse.urlencode(fields).encode()
    try:
        with urllib.request.urlopen(urllib.request.Request(f"{issuer}/oauth/token", data=form, headers=headers),
                                    timeout=10) as r:
            grant = json.loads(r.read())
    except urllib.error.HTTPError as e:
        return {"issued": False, "status": e.code}
    token = grant.get("access_token", "")
    if not token:
        return {"issued": False}
    markers[a.as_] = token
    private_write(a.markers, json.dumps(markers))
    meta = {"io.modelcontextprotocol/clientCapabilities": {},
            "io.modelcontextprotocol/clientInfo": {"name": "e2e-secret-sweep", "version": "1.0.0"},
            "io.modelcontextprotocol/protocolVersion": PROTOCOL}
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {"_meta": meta}}).encode()
    req = urllib.request.Request(f"{issuer}/mcp", data=body, headers={
        "Authorization": f"Bearer {token}", "Content-Type": "application/json", "Mcp-Method": "tools/list",
        "Accept": "application/json, text/event-stream", "MCP-Protocol-Version": PROTOCOL})
    try:
        with urllib.request.urlopen(req, timeout=20) as r:
            status, listed = r.status, b'"tools"' in r.read()
    except urllib.error.HTTPError as e:
        status, listed = e.code, False
    return {"issued": True, "tokenType": grant.get("token_type"), "listStatus": status, "listed": listed}


# --- scan --------------------------------------------------------------------------------------------

def scan_argv(markers):
    found = {}
    data = open(EXEC_LOG, "rb").read() if os.path.exists(EXEC_LOG) else b""
    for line in data.splitlines():
        names = hits(line, markers)
        if names:
            m = re.search(rb'execve\("([^"]*)"', line)
            program = os.path.basename(m.group(1).decode(errors="replace")) if m else "?"
            for n in names:
                found.setdefault(n, set()).add(program)
    for line in open(TRACE_LOG, "rb").read().splitlines() if os.path.exists(TRACE_LOG) else []:
        for n in hits(line, markers):
            found.setdefault(n, set()).add(os.path.basename(line.split(b" ", 1)[0].decode(errors="replace")))
    return {n: sorted(p) for n, p in found.items()}, len(data.splitlines())


def scan_environ(markers):
    found = {}
    for entry in os.listdir("/proc"):
        if not entry.isdigit():
            continue
        try:
            env = open(f"/proc/{entry}/environ", "rb").read()
            comm = open(f"/proc/{entry}/comm").read().strip()
            unit = re.search(r"/([^/]+\.service)", open(f"/proc/{entry}/cgroup").read())
        except OSError:
            continue
        for n in hits(env, markers):
            found.setdefault(n, set()).add(f"{comm}@{unit.group(1) if unit else '-'}")
    return {n: sorted(p) for n, p in found.items()}


def scan_files(markers, roots, state, holders):
    found = []
    for root in roots:
        for dirpath, dirnames, filenames in os.walk(root):
            dirnames[:] = [d for d in dirnames if not os.path.join(dirpath, d).startswith(SKIP_DIRS)]
            for f in filenames:
                path = os.path.join(dirpath, f)
                if path.startswith(WORK + "/"):
                    continue
                try:
                    st = os.lstat(path)
                    if not stat.S_ISREG(st.st_mode) or st.st_size > MAX_FILE:
                        continue
                    data = open(path, "rb").read()
                except OSError:
                    continue
                names = hits(data, markers)
                if names:
                    found.append({"path": path, "mode": oct(st.st_mode & 0o777), "uid": st.st_uid,
                                  "store": path.startswith(state) and path not in holders, "markers": names})
    return found


def scan(a):
    markers = load_markers(a.markers)
    argv, execs = scan_argv(markers)
    journal = subprocess.run(["journalctl", "--no-pager", "-o", "cat"], capture_output=True).stdout
    files = scan_files(markers, a.root, a.state, set(a.holder))
    environ = scan_environ(markers)
    unit_comm = open(f"/proc/{main_pid(a.unit)}/comm").read().strip() if a.unit else ""
    allowed_environ = {f"{unit_comm}@{a.unit}"} | set(a.allow_environ)
    loose = [f for f in files if not f["store"] and (int(f["mode"], 8) & 0o077 or f["uid"] != 0)]
    stray_environ = {n: [p for p in ps if p not in allowed_environ] for n, ps in environ.items()}
    stray_environ = {n: ps for n, ps in stray_environ.items() if ps}
    out = {
        "markers": sorted(markers),
        "execsTraced": execs,
        "argv": argv,
        "journal": hits(journal, markers),
        "store": sorted({n for f in files if f["store"] for n in f["markers"]}),
        "files": files,
        "looseFiles": [f["path"] for f in loose],
        "environ": environ,
        "strayEnviron": stray_environ,
    }
    out["clean"] = (execs > 0 and not argv and not out["journal"] and not out["store"] and not loose
                    and not stray_environ)
    return out


def main():
    p = argparse.ArgumentParser()
    sub = p.add_subparsers(dest="command", required=True)
    pl = sub.add_parser("plant")
    pl.add_argument("--env", required=True)
    pl.add_argument("--markers", required=True)
    pl.add_argument("--set", action="append", default=[], help="a secret env line, also a marker")
    pl.add_argument("--setting", action="append", default=[], help="a plain env line, not a marker")
    pl.add_argument("--marker", action="append", default=[])
    ag = sub.add_parser("agent")
    ag.add_argument("action", choices=["start", "stop"])
    ag.add_argument("name")
    ag.add_argument("--port", type=int)
    ag.add_argument("--env")
    ag.add_argument("--binary", default=BINARY)
    w = sub.add_parser("watch")
    w.add_argument("action", choices=["start", "stop"])
    w.add_argument("--unit")
    br = sub.add_parser("bridge")
    br.add_argument("action", choices=["start", "stop"])
    br.add_argument("--port", type=int)
    o = sub.add_parser("oauth")
    o.add_argument("--port", type=int, required=True)
    o.add_argument("--client", required=True)
    o.add_argument("--secret", required=True)
    o.add_argument("--as", dest="as_", required=True)
    o.add_argument("--markers", required=True)
    o.add_argument("--basic", action="store_true", help="client_secret_basic instead of the form")
    mk = sub.add_parser("mark")
    mk.add_argument("--credential", required=True)
    mk.add_argument("--as", dest="as_", required=True)
    mk.add_argument("--markers", required=True)
    s = sub.add_parser("scan")
    s.add_argument("--markers", required=True)
    s.add_argument("--unit", required=True)
    s.add_argument("--root", action="append", default=[])
    s.add_argument("--state", required=True)
    s.add_argument("--holder", action="append", default=[], help="a store file meant to hold a secret")
    s.add_argument("--allow-environ", action="append", default=[], help="comm@unit that may hold a marker")
    a = p.parse_args()
    out = {"plant": plant, "agent": agent, "watch": watch, "bridge": bridge, "mark": mark, "oauth": oauth,
           "scan": scan}[a.command](a)
    json.dump(out, sys.stdout, sort_keys=True)


if __name__ == "__main__":
    main()
