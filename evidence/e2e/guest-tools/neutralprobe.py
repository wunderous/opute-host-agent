#!/usr/bin/env python3
"""What a 0.4.0 agent publishes and prints, for the platform-neutral journeys.

Run as root inside the dev VM (copied in, hash-pinned); for an agent in its
own network namespace, under `nsenter -t PID -n`.

  neutralprobe.py summary --port P --token T --unit U --binary B
      the tools/list size and digest, the agent's "serving profile" log
      lines, and every Platform identifier found in its journal, its
      tools/list and its help text
  neutralprobe.py oauth --port P --client ID (--cred FILE | --secret S) [--save FILE]
      a client_credentials token for the agent's own resource and one
      tools/list with it; --save keeps the token (0600) for `reuse`
  neutralprobe.py reuse --port P --token-file FILE
      one tools/list with a saved token

Each prints one JSON document on stdout; no secret is printed.
"""
import argparse
import hashlib
import json
import os
import re
import subprocess
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import mcpwire  # noqa: E402

# The identifiers the boundary test forbids (crates/host-agent/src/boundary.rs).
PLATFORM = re.compile(r"(?<![A-Za-z])Platform(?![A-Za-z])|opute\.io|OPUTE_AGENT_MODE|"
                      r"OPUTE_STANDALONE_ALLOW_MUTATIONS|opute-mcp-host|opute-platform")
PROFILE = re.compile(r'msg="serving profile" (.*)$')


def run(argv):
    return subprocess.run(argv, capture_output=True, text=True).stdout


def tools(listed):
    return ((listed.get("body") or {}).get("result") or {}).get("tools") or []


def summary(a):
    listed = mcpwire.mcp(a.port, a.token, "tools/list")
    journal = run(["journalctl", "-u", a.unit, "--no-pager", "-o", "cat"])
    sources = {"journal": journal, "tools/list": json.dumps(listed.get("body")),
               "help": run([a.binary, "help"])}
    hits = sorted({f"{name}: {m.group(0)}" for name, text in sources.items() for m in PLATFORM.finditer(text)})
    names = sorted(t["name"] for t in tools(listed))
    return {"status": listed.get("status"), "tools": len(names),
            "catalogSha": hashlib.sha256(json.dumps(tools(listed), sort_keys=True).encode()).hexdigest(),
            "profileLines": [m.group(1) for line in journal.splitlines() if (m := PROFILE.search(line))],
            "helpBytes": len(sources["help"]), "hits": hits}


def oauth(a):
    secret = a.secret
    out = {}
    if a.cred:
        raw = open(a.cred, "rb").read()
        secret = json.loads(raw)["client_secret"]
        out["credSha256"] = hashlib.sha256(raw).hexdigest()
    minted = mcpwire.http(a.port, "POST", "/oauth/token", form={
        "grant_type": "client_credentials", "resource": f"http://127.0.0.1:{a.port}/mcp",
        "client_id": a.client, "client_secret": secret})
    body = minted.get("body") if isinstance(minted.get("body"), dict) else {}
    token = body.get("access_token")
    out.update({"tokenStatus": minted.get("status"), "error": body.get("error"), "issued": bool(token)})
    if token:
        listed = mcpwire.mcp(a.port, token, "tools/list")
        out.update({"listStatus": listed.get("status"), "tools": len(tools(listed))})
        if a.save:
            fd = os.open(a.save, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
            with os.fdopen(fd, "w") as f:
                f.write(token)
    return out


def reuse(a):
    listed = mcpwire.mcp(a.port, open(a.token_file).read().strip(), "tools/list")
    return {"listStatus": listed.get("status"), "tools": len(tools(listed))}


def main():
    p = argparse.ArgumentParser()
    sub = p.add_subparsers(dest="command", required=True)
    s = sub.add_parser("summary")
    s.add_argument("--port", type=int, required=True)
    s.add_argument("--token", required=True)
    s.add_argument("--unit", required=True)
    s.add_argument("--binary", required=True)
    o = sub.add_parser("oauth")
    o.add_argument("--port", type=int, required=True)
    o.add_argument("--client", required=True)
    o.add_argument("--cred")
    o.add_argument("--secret", default="")
    o.add_argument("--save")
    r = sub.add_parser("reuse")
    r.add_argument("--port", type=int, required=True)
    r.add_argument("--token-file", required=True)
    a = p.parse_args()
    json.dump({"summary": summary, "oauth": oauth, "reuse": reuse}[a.command](a), sys.stdout)


if __name__ == "__main__":
    main()
