#!/usr/bin/env python3
"""Journey helpers, run as root inside the dev VM (copied in, hash-pinned).

  e2etool.py agent start NAME --port P --id ID [--env K=V]... [--user opute]
      a transient system unit e2e-agent-NAME running the installed release
      binary as USER, with its instance root, state and HOME under
      /tmp/e2e-agents/NAME (outside the VM state diff); waits for /health
  e2etool.py agent stop NAME          stop it and clear its unit state
  e2etool.py agent remove NAME        stop it and delete its /tmp root
  e2etool.py db PATH [--plan-id ID]   integrity, table row counts, operation
                                      and plan-run ids (read-only)
  e2etool.py contains PATH TEXT       rows of a SQLite file that hold TEXT
  e2etool.py unit NAME [--user U]     systemctl show of a unit (system, or U's manager)
  e2etool.py listen PORT              local addresses listening on PORT
  e2etool.py slow start|stop [--port P --delay S]
                                      a transient unit e2e-slow: an HTTP
                                      endpoint that answers 200 after S seconds
  e2etool.py ideconfig HTML   the VS Code mcp.json the page documents,
                                      and the first calls made exactly with it
                                      (token from $E2E_TOKEN)

Each prints one JSON document on stdout.
"""
import argparse
import json
import os
import re
import sqlite3
import subprocess
import sys
import time
import urllib.request

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
BINARY = "/home/opute/.local/share/opute/opute-host-agent"
ROOT = "/tmp/e2e-agents"


def sh(argv, check=True):
    done = subprocess.run(argv, capture_output=True, text=True)
    if check and done.returncode != 0:
        raise SystemExit(json.dumps({"argv": argv, "exit": done.returncode, "stderr": done.stderr[-2000:]}))
    return done


def health(port):
    try:
        with urllib.request.urlopen(f"http://127.0.0.1:{port}/health", timeout=2) as r:
            return json.loads(r.read())
    except Exception:
        return None


def agent_start(a):
    root = f"{ROOT}/{a.name}"
    for d in ("instance", "state", "home"):
        os.makedirs(f"{root}/{d}", exist_ok=True)
    sh(["chown", "-R", f"{a.user}:{a.user}", ROOT])
    env = {"OPUTE_REMOTE_AGENT_ID": a.id, "HOST_MCP_PORT": str(a.port), "HOME": f"{root}/home",
           "OPUTE_HOST_AGENT_INSTANCE_ROOT": f"{root}/instance", "OPUTE_STANDALONE_STATE_DIR": f"{root}/state"}
    env.update(dict(kv.split("=", 1) for kv in a.env))
    argv = ["systemd-run", f"--unit=e2e-agent-{a.name}", f"--uid={a.user}", f"--gid={a.user}",
            "--property=Restart=no", "--collect"] + [f"--setenv={k}={v}" for k, v in env.items()] + \
        [a.binary, "serve", "--mode", a.mode, "--transport", "http"]
    sh(argv)
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        h = health(a.port)
        if h is not None:
            return {"unit": f"e2e-agent-{a.name}.service", "port": a.port, "health": h}
        time.sleep(0.3)
    log = sh(["journalctl", "-u", f"e2e-agent-{a.name}", "--no-pager", "-o", "cat"], check=False).stdout
    raise SystemExit(json.dumps({"error": "agent did not become healthy", "journal": log[-2000:]}))


def agent_stop(a):
    done = sh(["systemctl", "stop", f"e2e-agent-{a.name}.service"], check=False)
    sh(["systemctl", "reset-failed", f"e2e-agent-{a.name}.service"], check=False)
    out = {"stopped": done.returncode == 0}
    if a.action == "remove":
        sh(["rm", "-rf", f"{ROOT}/{a.name}"])
        if os.path.isdir(ROOT) and not os.listdir(ROOT):
            os.rmdir(ROOT)
        out["removed"] = True
    return out


def db(a):
    con = sqlite3.connect(f"file:{a.path}?mode=ro", uri=True)
    tables = [r[0] for r in con.execute("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name")]
    out = {"integrity": con.execute("PRAGMA integrity_check").fetchone()[0],
           "counts": {t: con.execute(f'SELECT COUNT(*) FROM "{t}"').fetchone()[0] for t in tables}}
    if "operations" in tables:
        out["operationIds"] = sorted(r[0] for r in con.execute("SELECT operation_id FROM operations"))
    if "plan_runs" in tables:
        query = "SELECT run_id, status FROM plan_runs" + (" WHERE plan_id=?" if a.plan_id else "")
        rows = con.execute(query, (a.plan_id,) if a.plan_id else ()).fetchall()
        out["planRunIds"] = sorted(r[0] for r in rows)
        out["planRunStatus"] = {r[0]: r[1] for r in rows}
    return out


def contains(a):
    con = sqlite3.connect(f"file:{a.path}?mode=ro", uri=True)
    hits = sum(1 for line in con.iterdump() if a.text in line)
    return {"rows": hits}


def ideconfig(a):
    import mcpwire
    page = open(a.html, encoding="utf-8").read()
    import html as htmllib
    blocks = [htmllib.unescape(re.sub(r"<[^>]+>", "", b)) for b in re.findall(r"(?s)<pre><code>(.*?)</code></pre>", page)]
    config = next(json.loads(b) for b in blocks if '"servers"' in b)
    server = next(iter(config["servers"].values()))
    header = server["headers"]["Authorization"]
    token = os.environ["E2E_TOKEN"]
    url = server["url"]
    port = int(re.match(r"http://127\.0\.0\.1:(\d+)/mcp$", url).group(1))
    bearer = header.replace("${input:opute-host-token}", token)
    assert bearer.startswith("Bearer ")
    listed = mcpwire.mcp(port, bearer[len("Bearer "):], "tools/list", request_id=1, client="vscode-config")
    called = mcpwire.mcp(port, bearer[len("Bearer "):], "tools/call", {"name": "get_host_info", "arguments": {}},
                         request_id=2, client="vscode-config")
    result = (called.get("body") or {}).get("result") or {}
    return {"config": {"type": server["type"], "url": url, "authorization": header,
                       "inputs": [{k: i.get(k) for k in ("type", "id", "password")} for i in config.get("inputs", [])]},
            "toolsListStatus": listed.get("status"),
            "tools": len(((listed.get("body") or {}).get("result") or {}).get("tools") or []),
            "results": [{"tool": "get_host_info", "isError": bool(result.get("isError")),
                         "structuredContent": result.get("structuredContent")}],
            "callStatus": called.get("status")}


def slow(a):
    if a.action == "serve":
        import http.server

        class Handler(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
                time.sleep(a.delay)
                self.send_response(200)
                self.send_header("Content-Length", "2")
                self.end_headers()
                self.wfile.write(b"ok")

            def log_message(self, *args):
                pass
        http.server.ThreadingHTTPServer(("127.0.0.1", a.port), Handler).serve_forever()
    if a.action == "start":
        sh(["systemd-run", "--unit=e2e-slow", "--collect", "python3", os.path.abspath(__file__), "slow", "serve",
            "--port", str(a.port), "--delay", str(a.delay)])
        return {"unit": "e2e-slow.service", "port": a.port, "delay": a.delay}
    sh(["systemctl", "stop", "e2e-slow.service"], check=False)
    return {"stopped": True}


UNIT_PROPS = ["LoadState", "ActiveState", "SubState", "UnitFileState", "Result", "ExecMainStatus", "NRestarts",
              "User", "FragmentPath", "DropInPaths", "EnvironmentFiles"]


def unit(a):
    argv = ["systemctl", "show", "-p", ",".join(UNIT_PROPS), a.name]
    if a.user:
        uid = sh(["id", "-u", a.user]).stdout.strip()
        argv = ["runuser", "-u", a.user, "--", "env", f"XDG_RUNTIME_DIR=/run/user/{uid}", "systemctl", "--user"] + argv[1:]
    out = dict(line.split("=", 1) for line in sh(argv).stdout.splitlines() if "=" in line)
    out["EnvironmentFiles"] = out.get("EnvironmentFiles", "").split(" (")[0]
    return out


def listen(a):
    text = sh(["ss", "-ltnH", f"sport = :{a.port}"]).stdout
    return {"addresses": sorted({line.split()[3] for line in text.splitlines() if line.strip()})}


def main():
    p = argparse.ArgumentParser()
    sub = p.add_subparsers(dest="command", required=True)
    ag = sub.add_parser("agent")
    ag.add_argument("action", choices=["start", "stop", "remove"])
    ag.add_argument("name")
    ag.add_argument("--port", type=int)
    ag.add_argument("--id")
    ag.add_argument("--env", action="append", default=[])
    ag.add_argument("--user", default="opute")
    ag.add_argument("--mode", default="standalone")
    ag.add_argument("--binary", default=BINARY)
    d = sub.add_parser("db")
    d.add_argument("path")
    d.add_argument("--plan-id")
    c = sub.add_parser("contains")
    c.add_argument("path")
    c.add_argument("text")
    u = sub.add_parser("unit")
    u.add_argument("name")
    u.add_argument("--user")
    ls = sub.add_parser("listen")
    ls.add_argument("port", type=int)
    sl = sub.add_parser("slow")
    sl.add_argument("action", choices=["start", "stop", "serve"])
    sl.add_argument("--port", type=int, default=8799)
    sl.add_argument("--delay", type=float, default=20)
    i = sub.add_parser("ideconfig")
    i.add_argument("html")
    a = p.parse_args()
    if a.command == "agent":
        out = agent_start(a) if a.action == "start" else agent_stop(a)
    else:
        out = {"db": db, "contains": contains, "ideconfig": ideconfig, "slow": slow,
               "unit": unit, "listen": listen}[a.command](a)
    json.dump(out, sys.stdout, sort_keys=True)


if __name__ == "__main__":
    main()
