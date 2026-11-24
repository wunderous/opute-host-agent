#!/usr/bin/env python3
"""What an agent publishes and prints, for the platform-neutral and D55 journeys.

Run as root inside the dev VM (copied in, hash-pinned); for an agent in its
own network namespace, under `nsenter -t PID -n`.

  neutralprobe.py summary --port P --token T --unit U --binary B
      the tools/list size and digest, the agent's "serving profile" log
      lines, and every Platform identifier found in its journal, its
      tools/list and its help text
  neutralprobe.py oauth --port P --client ID (--cred FILE | --secret S) [--save FILE]
                        [--scope S] [--call TOOL [--arguments JSON]]
      a client_credentials token for the agent's own resource (asking for
      --scope when given) and one tools/list with it; --save keeps the token
      (0600) for `reuse`; --call calls one tool with it
  neutralprobe.py reuse --port P --token-file FILE
      one tools/list with a saved token
  neutralprobe.py interactive --port P --binary B --state-dir D [--scope S]
                              [--call TOOL [--arguments JSON]]
      the bootstrap client's authorization: request (asking for --scope when
      given), the operator's `oauth pending` and `oauth approve --once` as the
      agent's user, the code, the token, and one tool call with it
  neutralprobe.py annotations --port P --token T
      whether every tool's annotations agree with its published effect

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
                      r"OPUTE_STANDALONE_ALLOW_MUTATIONS|OPUTE_HOST_ALLOW_MUTATIONS|opute-mcp-host|opute-platform")
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
    form = {"grant_type": "client_credentials", "resource": f"http://127.0.0.1:{a.port}/mcp",
            "client_id": a.client, "client_secret": secret}
    if a.scope:
        form["scope"] = a.scope
    minted = mcpwire.http(a.port, "POST", "/oauth/token", form=form)
    body = minted.get("body") if isinstance(minted.get("body"), dict) else {}
    token = body.get("access_token")
    out.update({"tokenStatus": minted.get("status"), "error": body.get("error"), "issued": bool(token),
                "scope": body.get("scope")})
    if token:
        listed = mcpwire.mcp(a.port, token, "tools/list")
        out.update({"listStatus": listed.get("status"), "tools": len(tools(listed))})
        if a.call:
            out["call"] = call(a.port, token, a.call, a.arguments)
        if a.save:
            fd = os.open(a.save, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
            with os.fdopen(fd, "w") as f:
                f.write(token)
    return out


def call(port, token, tool, arguments):
    """One tools/call: whether it failed, its text and its typed code."""
    out = mcpwire.mcp(port, token, "tools/call", {"name": tool, "arguments": json.loads(arguments or "{}")})
    result = (out.get("body") or {}).get("result") or {}
    content = result.get("content") or [{}]
    return {"status": out.get("status"), "isError": bool(result.get("isError")),
            "text": content[0].get("text") if isinstance(content[0], dict) else None,
            "code": (result.get("structuredContent") or {}).get("code")}


def reuse(a):
    listed = mcpwire.mcp(a.port, open(a.token_file).read().strip(), "tools/list")
    return {"listStatus": listed.get("status"), "tools": len(tools(listed))}


def interactive(a):
    import base64
    import pwd
    verifier = "e2e-interactive-verifier-" + hashlib.sha256(os.urandom(32)).hexdigest()
    challenge = base64.urlsafe_b64encode(hashlib.sha256(verifier.encode()).digest()).rstrip(b"=").decode()
    resource, redirect = f"http://127.0.0.1:{a.port}/mcp", "http://127.0.0.1/callback"
    form = {"response_type": "code", "client_id": "host-agent-bootstrap", "redirect_uri": redirect,
            "resource": resource, "code_challenge": challenge, "code_challenge_method": "S256", "state": "e2e"}
    if a.scope:
        form["scope"] = a.scope
    page = mcpwire.http(a.port, "POST", "/oauth/authorize", form=form)
    text = (page.get("body") or {}).get("raw", "")
    request = re.search(r"request=([0-9a-f]+)", text)
    out = {"pageStatus": page.get("status"), "pageNamesScope": f"<code>{a.scope or 'read'}</code>" in text}
    if not request:
        return out
    # the operator acts as the agent's user, who owns the state; a non-root probe is that user already
    user = pwd.getpwuid(os.stat(a.state_dir).st_uid).pw_name
    operator = (["runuser", "-u", user, "--"] if os.geteuid() == 0 else []) + [a.binary, "oauth"]
    pending = run(operator + ["pending", "--state-dir", a.state_dir]).strip().splitlines()
    out["pending"] = [" ".join(line.split()[1:2] + [w for w in line.split() if w.startswith("scope=")])
                      for line in pending]
    code = pending[0].split()[0] if pending and "-" in pending[0].split()[0] else ""
    approved = run(operator + ["approve", "--once", code, "--state-dir", a.state_dir]).strip()
    out["approved"] = approved.replace(code, "CODE") if code else approved
    status = mcpwire.http(a.port, "GET", "/oauth/authorize/status", query={"request": request.group(1)})
    location = (status.get("headers") or {}).get("location", "")
    auth = re.search(r"[?&]code=([^&]+)", location)
    if not auth:
        return {**out, "statusCode": status.get("status")}
    minted = mcpwire.http(a.port, "POST", "/oauth/token", form={
        "grant_type": "authorization_code", "code": auth.group(1), "client_id": "host-agent-bootstrap",
        "redirect_uri": redirect, "code_verifier": verifier, "resource": resource})
    body = minted.get("body") if isinstance(minted.get("body"), dict) else {}
    out.update({"tokenStatus": minted.get("status"), "scope": body.get("scope")})
    if body.get("access_token") and a.call:
        out["call"] = call(a.port, body["access_token"], a.call, a.arguments)
    return out


def annotations(a):
    listed = tools(mcpwire.mcp(a.port, a.token, "tools/list"))
    disagree = []
    for tool in listed:
        effect = ((tool.get("_meta") or {}).get("capability") or {}).get("effect")
        hints = tool.get("annotations") or {}
        want = {"readOnlyHint": True} if effect == "read" else {
            "readOnlyHint": False, "destructiveHint": effect == "destructive"}
        if hints != want:
            disagree.append(tool["name"])
    effects = sorted({((t.get("_meta") or {}).get("capability") or {}).get("effect") for t in listed})
    return {"tools": len(listed), "effects": effects, "disagree": disagree}


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
    o.add_argument("--scope", default="")
    o.add_argument("--call")
    o.add_argument("--arguments")
    r = sub.add_parser("reuse")
    r.add_argument("--port", type=int, required=True)
    r.add_argument("--token-file", required=True)
    i = sub.add_parser("interactive")
    i.add_argument("--port", type=int, required=True)
    i.add_argument("--binary", required=True)
    i.add_argument("--state-dir", required=True)
    i.add_argument("--scope", default="")
    i.add_argument("--call")
    i.add_argument("--arguments")
    n = sub.add_parser("annotations")
    n.add_argument("--port", type=int, required=True)
    n.add_argument("--token", required=True)
    a = p.parse_args()
    commands = {"summary": summary, "oauth": oauth, "reuse": reuse, "interactive": interactive,
                "annotations": annotations}
    json.dump(commands[a.command](a), sys.stdout)


if __name__ == "__main__":
    main()
