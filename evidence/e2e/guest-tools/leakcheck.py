#!/usr/bin/env python3
"""Count where a secret appears on the dev VM, without printing it.

  leakcheck.py --env FILE --key NAME [--guest NAME --secret NS/NAME/KEY] [--run RUN_ID ...]

The secrets are the value of KEY in the owner's env file and, with --guest, a
Kubernetes secret's value read inside that guest (the tunnel's run token). Each
is searched for in every file under /home/opute but the env file itself, in
the whole journal, and in the agent's answer to get_host_plan_run for each
--run. Prints one JSON object of counts; a secret that could not be read is
reported as missing, so the check can never pass vacuously.
"""
import argparse
import base64
import json
import os
import subprocess
import sys

sys.path.insert(0, "/opt/opute-e2e/guest")
import mcpwire  # noqa: E402

HOME = "/home/opute"


def env_value(path, key):
    for line in open(path):
        name, _, value = line.strip().partition("=")
        if name == key:
            return value.strip()
    return ""


def k8s_secret(guest, spec):
    namespace, name, key = spec.split("/")
    out = subprocess.run(["incus", "--project", "opute-e2e", "exec", guest, "--", "k3s", "kubectl", "-n", namespace,
                          "get", "secret", name, "-o", f"jsonpath={{.data.{key}}}"],
                         capture_output=True, text=True)
    return base64.b64decode(out.stdout).decode() if out.returncode == 0 and out.stdout else ""


def agent_token():
    return env_value("/etc/opute/host-agent.env", "MCP_AUTH_TOKEN")


def places(env_file, runs):
    for root, _, files in os.walk(HOME):
        for f in files:
            path = os.path.join(root, f)
            if os.path.realpath(path) == os.path.realpath(env_file) or os.path.islink(path):
                continue
            try:
                with open(path, "rb") as fh:
                    yield "files", path, fh.read()
            except OSError:
                continue
    journal = subprocess.run(["journalctl", "--no-pager", "-o", "cat"], capture_output=True).stdout
    yield "journal", "journal", journal
    token = agent_token()
    for run in runs:
        body = mcpwire.mcp(3014, token, "tools/call", {"name": "get_host_plan_run", "arguments": {"runId": run}})
        yield "responses", f"get_host_plan_run {run}", json.dumps(body).encode()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--env", required=True)
    parser.add_argument("--key", required=True)
    parser.add_argument("--guest")
    parser.add_argument("--secret")
    parser.add_argument("--run", action="append", default=[])
    args = parser.parse_args()
    secrets = {"apiToken": env_value(args.env, args.key)}
    if args.guest:
        secrets["runToken"] = k8s_secret(args.guest, args.secret)
    counts = {name: {"present": bool(value) and len(value) >= 16, "files": 0, "journal": 0, "responses": 0,
                     "where": []} for name, value in secrets.items()}
    for kind, where, data in places(args.env, args.run):
        for name, value in secrets.items():
            if counts[name]["present"] and value.encode() in data:
                counts[name][kind] += 1
                counts[name]["where"].append(where)
    counts["clean"] = all(c["present"] and c["files"] + c["journal"] + c["responses"] == 0 for c in counts.values())
    print(json.dumps(counts, sort_keys=True))


if __name__ == "__main__":
    main()
