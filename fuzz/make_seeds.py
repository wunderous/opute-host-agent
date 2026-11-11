#!/usr/bin/env python3
"""Write the fuzz seed corpus under fuzz/seeds/ from the repo's own fixtures.

Seeds are the starting inputs of each target and the inputs the stable
`fuzzing::tests::replay` test runs. Regenerate with `python3 fuzz/make_seeds.py`;
the output is deterministic.
"""

import collections
import io
import json
import shutil
import tarfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "fuzz" / "seeds"


def walk(value, keep, found):
    if isinstance(value, dict):
        if keep(value):
            found.append(value)
        for v in value.values():
            walk(v, keep, found)
    elif isinstance(value, list):
        for v in value:
            walk(v, keep, found)


def json_files(*globs):
    for pattern in globs:
        for path in sorted(ROOT.glob(pattern)):
            try:
                yield path, json.loads(path.read_text())
            except (ValueError, UnicodeDecodeError):
                continue


def http(method, target, body=b"", headers=()):
    head = [f"{method} {target} HTTP/1.1", "Host: 127.0.0.1"]
    head += list(headers)
    if body:
        head.append(f"Content-Length: {len(body)}")
    return ("\r\n".join(head) + "\r\n\r\n").encode() + body


def mcp(method, params, rpc_id=1):
    body = json.dumps({"jsonrpc": "2.0", "id": rpc_id, "method": method, "params": params}).encode()
    return http("POST", "/mcp", body, [
        "Content-Type: application/json",
        "Accept: application/json, text/event-stream",
        "Authorization: Bearer not-a-token",
    ])


def tar(members):
    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode="w", format=tarfile.PAX_FORMAT) as t:
        for name, kind, data in members:
            info = tarfile.TarInfo(name)
            info.mtime = 0
            if kind == "dir":
                info.type = tarfile.DIRTYPE
                info.mode = 0o755
                t.addfile(info)
            elif kind == "link":
                info.type = tarfile.SYMTYPE
                info.linkname = data
                t.addfile(info)
            else:
                info.size = len(data)
                info.mode = 0o644
                t.addfile(info, io.BytesIO(data))
    return buf.getvalue()


def main():
    seeds = {}

    def add(target, name, data):
        if isinstance(data, str):
            data = data.encode()
        seeds.setdefault(target, {})[name] = data

    add("http_wire", "health", http("GET", "/health"))
    add("http_wire", "options-star", http("OPTIONS", "*"))
    add("http_wire", "initialize", mcp("initialize", {
        "protocolVersion": "2026-07-28", "capabilities": {},
        "clientInfo": {"name": "fuzz", "version": "0"}}))
    add("http_wire", "tools-call", mcp("tools/call", {"name": "get_host_info", "arguments": {}}))
    add("http_wire", "token-form", http("POST", "/oauth/token",
        b"grant_type=client_credentials&client_id=opute&client_secret=x",
        ["Content-Type: application/x-www-form-urlencoded"]))
    add("http_wire", "authorize", http("GET",
        "/oauth/authorize?response_type=code&client_id=c&redirect_uri=http%3A%2F%2F127.0.0.1%2Fcb"
        "&code_challenge=abc&code_challenge_method=S256&state=s"))
    add("http_wire", "chunked", (
        "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nTransfer-Encoding: chunked\r\n"
        "Content-Type: application/json\r\n\r\n5\r\n{\"a\":\r\n2\r\n1}\r\n0\r\n\r\n"))
    add("http_wire", "pipelined", http("GET", "/health") + http("GET", "/.well-known/oauth-protected-resource"))

    add("json_document", "object", '{"a":[1,2.5e3,"\\u00e9",true,null],"b":{"c":"d"}}')
    add("json_document", "deep", "[" * 64 + "]" * 64)
    add("json_document", "numbers", "[-0, 1e400, 0.1, 9007199254740993]")

    add("tool_arguments", "empty", b"\x00{}")
    add("tool_arguments", "vm", b"\x05" + json.dumps({"name": "web-1", "image": "ubuntu:24.04", "cpu": 2}).encode())
    add("tool_arguments", "path", b"\x11" + json.dumps({"path": "~/x", "content": "y", "mode": "0644"}).encode())

    plans, recipes = [], []
    for _, doc in json_files("tools/parity/scenarios/*.json", "evidence/e2e/journeys/*.json"):
        walk(doc, lambda o: o.get("contractVersion") == "host-plan.v1" and "nodes" in o, plans)
        walk(doc, lambda o: o.get("contractVersion") == "host-recipe.v1", recipes)
    for i, plan in enumerate(plans[:12]):
        add("plan_document", f"plan-{i}", json.dumps(plan))
    recipe_yaml = (ROOT / "evidence/e2e/journeys/inputs/e2e-cross-domain-recipe.yaml").read_text()
    add("host_recipe", "cross-domain-yaml", recipe_yaml)
    for i, recipe in enumerate(recipes[:6]):
        add("host_recipe", f"recipe-{i}", json.dumps(recipe))
    add("plan_document", "empty-nodes", '{"contractVersion":"host-plan.v1","planId":"p","generation":1,'
        '"idempotencyKey":"k","nodes":[]}')

    for i, value in enumerate(["vm:default:web-1", "container:tenant-a:db 1", "volume:t:x:y",
                               "10GiB", "1.5TB", "-1", "18446744073709551616B"]):
        add("resource_identity", f"value-{i}", value)

    add("env_file", "unit", "OPUTE_AGENT_MODE=standalone\nHOST_MCP_PORT=3014\n# comment\n"
        "export OPUTE_REMOTE_AGENT_ID=\"agent one\"\nBAD LINE\n=novalue\n")

    add("state_store", "empty", b"")
    add("state_store", "sqlite-header", b"SQLite format 3\x00" + b"\x10\x00\x01\x01" + b"\x00" * 92)
    add("reservation_file", "records", json.dumps({"r1": {"taskId": "t", "class": "normal",
        "cpuCores": 1, "memoryBytes": 1024, "expiresAt": "2026-11-10T00:00:00Z", "pid": 1}}))
    add("reservation_file", "null", "null")
    add("credential_file", "valid", json.dumps({"client_id": "opute", "client_secret": "s",
        "token_endpoint_auth_method": "client_secret_basic", "rotated_at": 0}))

    descriptors = []
    for _, doc in json_files("tools/parity/scenarios/provider-*.json", "tools/parity/fixtures/**/*.json"):
        walk(doc, lambda o: "providerId" in o and ("services" in o or "capabilities" in o), descriptors)
    for i, d in enumerate(descriptors[:8]):
        add("provider_descriptor", f"descriptor-{i}", json.dumps(d))
    add("provider_descriptor", "minimal", '{"providerId":"p","version":"1.0.0","services":[]}')

    add("provider_result", "text", json.dumps({"content": [{"type": "text", "text": "ok"}],
        "structuredContent": {"a": 1}, "isError": False}))
    add("provider_result", "tool-use", json.dumps({"content": [{"type": "tool_use", "id": "1",
        "name": "x", "input": {"k": [1, 2]}}]}))

    for path, doc in json_files("tools/parity/fixtures/shims/incus-*.json"):
        for i, step in enumerate(doc.get("commands", {}).get("incus", [])):
            if step.get("argv", [])[:1] == ["list"] and step.get("stdout"):
                add("incus_output", f"{path.stem}-{i}", step["stdout"])

    # command_output: real fixture output per parser family, keyed by the
    # selector byte `fuzzing::command_output` dispatches on.
    families = {"systemctl": [0], "incus": [10, 11, 12, 14, 15], "nvidia-smi": [13],
                "podman": [16, 17, 18, 19], "buildah": [18, 19], "ollama": [24, 25],
                "powershell.exe": [6, 7, 8, 9], "helm": [26, 29]}
    picked = collections.Counter()
    for path, doc in json_files("tools/parity/fixtures/shims/*.json"):
        for cmd, steps in sorted((doc.get("commands") or {}).items()):
            for step in steps if isinstance(steps, list) else []:
                out = step.get("stdout") if isinstance(step, dict) else None
                for sel in families.get(cmd, []):
                    if out and picked[sel] < 3:
                        picked[sel] += 1
                        add("command_output", f"{sel:02d}-{cmd}-{picked[sel]}", bytes([sel]) + out.encode())
    samples = {
        1: 'NAME="Ubuntu"\nVERSION_ID="24.04"\nID=ubuntu\n',
        2: "processor\t: 0\nmodel name\t: Fuzz CPU\nflags\t\t: vmx\n",
        3: "kvm\n", 4: "8080", 5: '{"status":"ok"}', 20: '{"a":1}', 21: '{"a":"b"}', 22: '{"Name":"x"}',
        23: "LISTEN 0 4096 127.0.0.1:5432 0.0.0.0:*", 28: "a%2Fb%zz",
        27: '{"nodes":[{"name":"n1","roles":["etcd"]}]}',
        30: '{"images":[{"id":"sha256:1","repoTags":["a:1"],"size":"10"}]}',
        31: '{"containers":[{"id":"c","imageRef":"sha256:1","state":"CONTAINER_RUNNING"}]}',
        32: "Filesystem 1B-blocks Used Available\n/dev/sda 100 40 60\n",
        33: "inet 100.96.0.1/32 scope global CloudflareWARP\n",
        29: '{"metadata":{"name":"web"},"spec":{"replicas":2,"template":{"spec":{"containers":[{"name":"a",'
            '"env":[{"name":"K","value":"v"},{"name":"S","valueFrom":{"secretKeyRef":{"name":"s","key":"k"}}}],'
            '"envFrom":[{"configMapRef":{"name":"c"}}]}]}}}}',
        34: '{"jsonrpc":"2.0","id":1,"result":{}}', 35: "https://raw.githubusercontent.com/o/r/main/x.yaml",
    }
    for sel, sample in samples.items():
        add("command_output", f"{sel:02d}-sample", bytes([sel]) + sample.encode())

    add("incus_output", "instance-devices", b'{"devices":{"eth1":{"type":"nic","nictype":"bridged","parent":"br0"}},'
        b'"expanded_devices":{"eth0":{"type":"nic","network":"incusbr0"},'
        b'"eth1":{"type":"nic","nictype":"bridged","parent":"br0"},"root":{"type":"disk","path":"/"}}}')
    add("console_output", "login", b"\x10" + b"\r\nUbuntu 24.04 e2e-v1 ttyS0\r\n" + b"\x05" + "é-login: ".encode())
    add("console_output", "split", b"\x01\xe2\x02\x82\xac\x00")
    add("authz_store", "empty", b"")
    add("authz_store", "sqlite-header", b"SQLite format 3\x00" + b"\x10\x00\x01\x01" + b"\x00" * 92)

    archive = tar([
        ("app", "dir", None),
        ("app/config.txt", "file", b"hello\n"),
        ("app/current", "link", "config.txt"),
        ("app/../escape", "file", b"x"),
        ("app/out", "link", "../../etc/passwd"),
    ])
    add("tar_stream", "mixed", archive)
    add("host_archive", "mixed", b"\x01" + archive)
    add("host_archive", "empty", b"\x01")

    if OUT.exists():
        shutil.rmtree(OUT)
    for target, files in seeds.items():
        (OUT / target).mkdir(parents=True)
        for name, data in files.items():
            (OUT / target / name).write_bytes(data)
    print({t: len(f) for t, f in sorted(seeds.items())})


if __name__ == "__main__":
    main()
