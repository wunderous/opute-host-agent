#!/usr/bin/env python3
"""Generate scenarios/host-domain.json: the M8a host-domain T1 matrix.

Run from tools/parity: python3 host_domain_scenarios.py scenarios/host-domain.json

Each family gets success, typed-failure and partial-failure cases. Every case
collects the sandbox home tree (type, mode, sha256, symlink target) and the
shim trace, so the external effect is compared, not only the answer.
System-scope writes are never attempted here: T1 runs as an unprivileged
user on the developer host, so system paths are exercised only up to
validation. Writes to /etc run in the T2 sandbox.
"""
import json, sys

SERVE = {"as": "start", "serve": {"argv": ["serve", "--mode=standalone"], "profile": "standalone", "env": {
    "OPUTE_STANDALONE_ALLOW_MUTATIONS": "true",
    "OPUTE_HOST_RESOURCE_ENFORCEMENT": "advisory",
    "OPUTE_HOST_MAX_NORMAL_OPERATIONS": "64", "OPUTE_HOST_MAX_HEAVY_OPERATIONS": "64",
    "OPUTE_HOST_RESOURCE_CPU_CAPACITY_CORES": "64",
    "OPUTE_HOST_RESOURCE_MEMORY_CAPACITY_BYTES": "68719476736"}}}
STOP = {"as": "stop", "stop": {}}


def serve(**env):
    s = json.loads(json.dumps(SERVE))
    s["serve"]["env"].update(env)
    return s


def call(as_, name, args):
    return {"as": as_, "mcp": {"method": "tools/call", "name": name,
                               "params": {"name": name, "arguments": args}}}


def task_call(as_, name, args):
    """A task-aware tool answers with a task handle; wait for the task."""
    var = "T_" + as_.upper()
    step = call(as_, name, args)
    step["mcp"]["capture"] = {var: ["body", "result", "taskId"]}
    done = {"as": as_ + "Done", "requires": [var], "mcp": {
        "method": "tasks/get", "name": "${%s}" % var, "params": {"taskId": "${%s}" % var}, "expand": True,
        "until": {"path": ["body", "result", "status"], "in": ["completed", "failed", "cancelled"]}, "timeout": 20}}
    # A completed task's tasks/get carries the tool result inline.
    return [step, done]


def flat(steps):
    out = []
    for step in steps:
        out.extend(step if isinstance(step, list) else [step])
    return out


TS = "wall-clock timestamp of this side's run"
# A task handle's own timestamps, as tasks/get and the task-aware call
# report them.
MASKS = [{"path": ["steps", "*", "body", "result", k], "type": "rfc3339", "reason": TS, "optional": True}
         for k in ("createdAt", "lastUpdatedAt")]


def scenario(id_, steps, anchors, collect=("homeTree", "trace")):
    return {"id": "host-domain." + id_, "surface": "host-domain", "owner": "host",
            "anchors": anchors, "fixture": None, "collect": list(collect),
            "steps": flat(steps), "compare": {"masks": MASKS}}


FILE = ["internal/domain/host/file.go:EnsureHostFile", "internal/domain/host/file.go:RemoveHostFile",
        "internal/tools/dispatch_host.go"]
CONF = "~/.config/parity/app.conf"
SHA_A = "sha256:" + __import__("hashlib").sha256(b"alpha\n").hexdigest()

cases = [
    # Create, keep, change mode, change content, read back, then remove with
    # a wrong hash, without confirmation, with an upper-case hash, and again.
    scenario("files.lifecycle", [
        serve(),
        call("create", "ensure_host_file", {"path": CONF, "content": "alpha\n"}),
        call("unchanged", "ensure_host_file", {"path": CONF, "content": "alpha\n", "mode": 384}),
        call("modeChange", "ensure_host_file", {"path": CONF, "content": "alpha\n", "mode": 420}),
        call("contentChange", "ensure_host_file", {"path": CONF, "content": "beta\n", "mode": 420}),
        call("back", "ensure_host_file", {"path": CONF, "content": "alpha\n", "mode": 420}),
        call("inspect", "inspect_host_file", {"path": CONF, "expectedSha256": SHA_A}),
        task_call("wrongHash", "remove_host_file", {"path": CONF, "confirm": True, "expectedSha256": "sha256:" + "0" * 64}),
        task_call("unconfirmed", "remove_host_file", {"path": CONF}),
        task_call("remove", "remove_host_file", {"path": CONF, "confirm": True,
                                           "expectedSha256": SHA_A.upper().replace("SHA256:", "")}),
        task_call("again", "remove_host_file", {"path": CONF, "confirm": True}),
        STOP], FILE),
    # Relative, home-rooted and absolute forms, the executable bit, and
    # untrimmed content written raw.
    scenario("files.path-forms", [
        serve(),
        call("relative", "ensure_host_file", {"path": "  bin/run.sh ", "content": " #!/bin/sh\n", "mode": 493}),
        call("homeRoot", "ensure_host_file", {"path": "~/top.txt", "content": "", "scope": " USER "}),
        call("absolute", "ensure_host_file", {"path": "${SANDBOX}/home/abs/x.txt", "content": "x"}),
        call("inspectExec", "inspect_host_file", {"path": "bin/run.sh"}),
        task_call("removeRelative", "remove_host_file", {"path": "bin/run.sh", "confirm": True}),
        STOP], FILE),
    # Typed refusals: every one must leave the home tree unchanged.
    scenario("files.refusals", [
        serve(),
        {"as": "outside", "mkdir": {"path": "${SANDBOX}/outside"}},
        {"as": "link", "symlink": {"path": "${SANDBOX}/home/linked", "target": "${SANDBOX}/outside"}},
        {"as": "dir", "mkdir": {"path": "${SANDBOX}/home/adir"}},
        call("scope", "ensure_host_file", {"path": "x", "content": "x", "scope": "bogus"}),
        call("empty", "ensure_host_file", {"path": "  ", "content": "x"}),
        call("escape", "ensure_host_file", {"path": "../escape.txt", "content": "x"}),
        call("absoluteOutside", "ensure_host_file", {"path": "/etc/hostname", "content": "x"}),
        call("throughLink", "ensure_host_file", {"path": "linked/x.txt", "content": "x"}),
        task_call("linkItself", "remove_host_file", {"path": "linked", "confirm": True}),
        call("modeLow", "ensure_host_file", {"path": "m.txt", "content": "x", "mode": 256}),
        call("modeHigh", "ensure_host_file", {"path": "m.txt", "content": "x", "mode": 511}),
        call("modeNegative", "ensure_host_file", {"path": "m.txt", "content": "x", "mode": -1}),
        call("modeFraction", "ensure_host_file", {"path": "m.txt", "content": "x", "mode": 420.9}),
        call("modeString", "ensure_host_file", {"path": "s.txt", "content": "x", "mode": "644"}),
        call("modeTypeBits", "ensure_host_file", {"path": "t.txt", "content": "x", "mode": 33188}),
        call("isDirectory", "ensure_host_file", {"path": "adir", "content": "x"}),
        task_call("removeDirectory", "remove_host_file", {"path": "adir", "confirm": True}),
        call("systemNotUnit", "ensure_host_file", {"path": "/etc/systemd/system/x.conf", "content": "x", "scope": "system"}),
        call("systemOutside", "ensure_host_file", {"path": "/etc/x.service", "content": "x", "scope": "system"}),
        task_call("systemBadName", "remove_host_file", {"path": "/etc/systemd/system/a b.service", "confirm": True, "scope": "system"}),
        task_call("systemMissing", "remove_host_file", {"path": "/etc/systemd/system/opute-parity-absent.service",
                                                  "confirm": True, "scope": "system"}),
        STOP], FILE),
    # A configured shared-host owner that is not this instance refuses every
    # mutation before it looks at its arguments; reads still answer.
    scenario("files.shared-owner-refused", [
        serve(OPUTE_SHARED_HOST_OWNER_INSTANCE="parity-other-instance"),
        call("ensure", "ensure_host_file", {"path": "x.txt", "content": "x"}),
        task_call("remove", "remove_host_file", {"path": "x.txt"}),
        call("inspect", "inspect_host_file", {"path": "x.txt"}),
        STOP], FILE + ["internal/hostruntime/shared.go:RequireSharedHostOwner"]),
    scenario("files.shared-owner-matches", [
        serve(OPUTE_HOST_AGENT_INSTANCE="parity-owner", OPUTE_SHARED_HOST_OWNER_INSTANCE=" parity-owner "),
        call("ensure", "ensure_host_file", {"path": "x.txt", "content": "x"}),
        task_call("remove", "remove_host_file", {"path": "x.txt", "confirm": True}),
        STOP], FILE + ["internal/hostruntime/shared.go:RequireSharedHostOwner"]),
]

if __name__ == "__main__":
    out = sys.argv[1] if len(sys.argv) > 1 else "scenarios/host-domain.json"
    with open(out, "w") as f:
        json.dump(cases, f, indent=1)
        f.write("\n")
    print(f"wrote {len(cases)} scenarios to {out}")
