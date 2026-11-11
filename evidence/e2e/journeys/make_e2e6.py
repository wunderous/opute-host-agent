#!/usr/bin/env python3
"""Write the e2e-6 journeys (m16 hardening): host faults, admission under
pressure and a resource budget, each on a transient agent of the installed
release candidate under /tmp/e2e-agents (outside the VM state diff).

  python3 evidence/e2e/journeys/make_e2e6.py

Expected outcomes come from the spec: truthful-interrupted-plan for kills,
host-fault-outcomes (D31) for the faults the spec did not state before,
bound-mcp-request-body (D29) for the body limit, the contract spec's typed
admission, and evidence/hardening/resource-budget.json for the budget.
"""
import json
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import make_e2e1 as base  # noqa: E402

a, ok, req, call, mcp, plan, poll_run = base.a, base.ok, base.req, base.call, base.mcp, base.plan, base.poll_run
T = base.T
SPEC = base.SPEC
FAULTS = "openspec/changes/host-fault-outcomes/specs/host-fault-outcomes/spec.md"
BODY = "openspec/changes/bound-mcp-request-body/specs/bound-mcp-request-body/spec.md"
CHILD_SPEC = "openspec/changes/child-process-environment/specs/child-process-environment/spec.md"
RELAY_SPEC = "openspec/changes/bound-relay-request-body/specs/bound-relay-request-body/spec.md"
RETENTION_SPEC = "openspec/changes/audit-retention/specs/audit-retention/spec.md"
Q5_SPEC = "openspec/changes/truthful-interrupted-plan/specs/truthful-interrupted-plan/spec.md"
TASKS_META = base.TASKS_META
BUDGET = json.loads((base.REPO / "evidence/hardening/resource-budget.json").read_text())
ROOT = "/tmp/e2e-agents"
MUT = "--env OPUTE_STANDALONE_ALLOW_MUTATIONS=true"
# The transient agents' systemd units, the slow-endpoint helper and the
# disk-full tmpfs exist only while a journey runs; cleanup removes them, so
# the residue check still holds.
AGENT_UNITS = ("system/units/e2e-agent-*", "system/unitFiles/e2e-agent-*",
               "system/units/e2e-slow*", "system/unitFiles/e2e-slow*",
               "system/units/tmp-e2e*agents-*.mount")


def agent(name: str, port: int, *env: str, root: bool = False) -> dict:
    """A transient agent. `root` runs it as root: the VM's opute user has no
    user manager (no linger), so as opute every run_host_command workload
    fails at once with "Failed to connect to bus"; a journey whose task must
    hold its reservation runs the agent as root, whose workloads go to the
    system manager."""
    extra = " ".join(f"--env {e}" for e in env) + (" --user root" if root else "")
    return {"id": f"{name}Agent", "guest": f"{T} agent start {name} --port {port} --id e2e-{name}-agent "
                                          f"--env MCP_AUTH_TOKEN=${{SECRET:{name}}} {MUT} {extra}".rstrip()}


def on(name: str, port: int) -> dict:
    return {"port": port, "auth": f"secret:{name}"}


def state(name: str) -> str:
    return f"{ROOT}/{name}/state"


def journey(id_, description, requirements, steps, cleanup, minutes=20, files=None, allow=()):
    return base.journey(id_, description, {"agent": f"transient Rust agents of the installed release candidate "
                                                    f"({base.RUST_VERSION}) under {ROOT}",
                                           "files": files or base.TOOL_FILES},
                        requirements, steps, cleanup, allow=AGENT_UNITS + tuple(allow), minutes=minutes, gate="e2e-6")


def remove(*names: str) -> list[dict]:
    return [{"id": "remove", "guest": " ; ".join(f"{T} agent remove {n} >/dev/null" for n in names) + " ; true"}]


def done(id_: str, var: str, target: dict) -> dict:
    """Poll a task until it is terminal."""
    return {"id": id_, "mcp": "tasks/get", "params": {"taskId": "${" + var + "}"}, "meta": TASKS_META, **target,
            "until": {"path": "response.body.result.status", "in": ["completed", "failed", "cancelled"]},
            "timeout": 60, "interval": 2}


def not_error(step: str) -> dict:
    return a(step, "response.body.result.isError", **{"in": [False, "<absent>"]})


def is_error(step: str) -> dict:
    return a(step, "response.body.result.isError", equals=True)


# --- 1. the state directory fills up --------------------------------------------------------------

def disk_full() -> dict:
    n, port = "full", 3040
    o = on(n, port)
    f = lambda i: {"path": f".config/opute-e2e/full-{i}.txt", "content": f"e2e {i}\n", "scope": "user"}  # noqa
    home = f"{ROOT}/{n}/home/.config/opute-e2e"
    fill = (f"fallocate -l $(df --output=avail -B1 {state(n)} | tail -1) {state(n)}/filler 2>/dev/null; "
            f"dd if=/dev/zero of={state(n)}/filler2 bs=4k 2>/dev/null; df --output=avail -B1 {state(n)} | tail -1")
    steps = base.TOOL_STEPS + [
        {"id": "mount", "guest": f"mkdir -p {state(n)} && mount -t tmpfs -o size=24m,mode=0700 e2e-full {state(n)} "
                                 "&& echo mounted"},
        agent(n, port, root=True),
        mcp("list", "tools/list", **o),
        call("writeBefore", "ensure_host_file", f(1), **o),
        call("taskBefore", "run_host_command", {"command": "echo before"}, meta=TASKS_META, **o),
        {"id": "dbBefore", "guest": f"{T} audit {state(n)}/state.db"},
        {"id": "fill", "guest": fill},
        call("writeFull", "ensure_host_file", f(2), **o),
        call("taskFull", "run_host_command", {"command": "echo full"}, meta=TASKS_META, **o),
        call("planFull", "run_host_plan", {"plan": plan("e2e6.full", "e2e6-full-1", ".config/opute-e2e/full-p.txt")},
             **o),
        call("readFull", "get_host_info", **o),
        {"id": "noHalfWrite", "guest": f"ls {home}; test ! -e {home}/full-2.txt && test ! -e {home}/full-p.txt "
                                       "&& echo none"},
        {"id": "dbFull", "guest": f"{T} audit {state(n)}/state.db"},
        {"id": "free", "guest": f"rm -f {state(n)}/filler {state(n)}/filler2 && echo freed"},
        call("writeAfter", "ensure_host_file", f(3), **o),
        call("taskAfter", "run_host_command", {"command": "echo after"}, meta=TASKS_META, **o),
        {"id": "dbAfter", "guest": f"{T} audit {state(n)}/state.db"},
    ]
    reqs = [
        req(FAULTS, "A full state directory refuses writes and recovers",
            ok("mount"), ok("fullAgent"), not_error("writeBefore"),
            a("fill", "stdout", matches="^ *[0-9]+\n$"),
            is_error("writeFull"), is_error("taskFull"), is_error("planFull"),
            a("noHalfWrite", "stdout", matches="none\n$"),
            not_error("readFull"),
            a("dbFull", "json.integrity", equals="ok"), a("dbFull", "json.unfinished", equals=0),
            not_error("writeAfter"), a("dbAfter", "json.integrity", equals="ok"),
            scenario="Disk full during a write (D31)"),
        req(FAULTS, "A full state directory refuses writes and recovers",
            a("dbAfter", "json.invocations.ensure_host_file", exists=True),
            a("dbFull", "json.invocations", equalsStep={"step": "dbBefore", "path": "json.invocations"}),
            scenario="no audit gap: a refused write leaves no audit row, and nothing ran unrecorded"),
    ]
    cleanup = remove(n) + [{"id": "unmount", "guest": f"umount {state(n)} 2>/dev/null; rm -rf {ROOT}/{n}; "
                                                       f"rmdir {ROOT} 2>/dev/null; true"}]
    return journey("e2e6.disk-full",
                   "D31: an agent whose state directory is a 24 MiB tmpfs; the directory is filled, durable calls are "
                   "refused with typed errors and leave no file or row, reads answer, and once space is freed the "
                   "same calls succeed without a restart.", reqs, steps, cleanup, minutes=15)


# --- 2. a damaged state store ---------------------------------------------------------------------

def store_damage() -> dict:
    n, port = "dmg", 3041
    o = on(n, port)
    db = f"{state(n)}/state.db"
    start_fails = (f"{T} agent start {n} --port {port} --id e2e-{n}-agent --env MCP_AUTH_TOKEN=${{SECRET:{n}}} "
                   f"{MUT} >/dev/null 2>&1; echo exit=$?; {T} agent stop {n} >/dev/null")
    snapshot = f"sha256sum {db} | cut -c1-64; ls {state(n)} | tr '\\n' ' '; echo"
    steps = base.TOOL_STEPS + [
        agent(n, port),
        mcp("list", "tools/list", **o),
        call("task", "run_host_command", {"command": "echo kept"}, meta=TASKS_META, **o,
             capture={"KEPT": "response.body.result.taskId"}),
        {"id": "taskDone", "mcp": "tasks/get", "params": {"taskId": "${KEPT}"}, "meta": TASKS_META, **o,
         "until": {"path": "response.body.result.status", "in": ["completed", "failed"]}, "timeout": 60,
         "interval": 2},
        {"id": "stop", "guest": f"{T} agent stop {n} && cp -a {db} {ROOT}/{n}/state.db.bak && echo saved"},
        # (a) truncated after the header page
        {"id": "truncate", "guest": f"truncate -s 4096 {db} && {snapshot}"},
        {"id": "startTruncated", "guest": start_fails},
        {"id": "listenTruncated", "guest": f"{T} listen {port}"},
        {"id": "afterTruncated", "guest": snapshot},
        # (b) the header overwritten
        {"id": "header", "guest": f"cp -a {ROOT}/{n}/state.db.bak {db} && dd if=/dev/urandom of={db} bs=100 count=1 "
                                  f"conv=notrunc 2>/dev/null && {snapshot}"},
        {"id": "startHeader", "guest": start_fails},
        {"id": "afterHeader", "guest": snapshot},
        # (c) a page in the middle overwritten: whether it opens depends on the page; the agent must not crash
        {"id": "page", "guest": f"cp -a {ROOT}/{n}/state.db.bak {db} && dd if=/dev/urandom of={db} bs=4096 seek=2 "
                                f"count=1 conv=notrunc 2>/dev/null && echo damaged"},
        {"id": "startPage", "guest": f"{T} agent start {n} --port {port} --id e2e-{n}-agent "
                                     f"--env MCP_AUTH_TOKEN=${{SECRET:{n}}} {MUT} >/dev/null 2>&1; echo exit=$?"},
        {"id": "pageRead", "guest": f"curl -s -o /dev/null -w '%{{http_code}}' http://127.0.0.1:{port}/health; echo"},
        {"id": "stopPage", "guest": f"{T} agent stop {n} >/dev/null; echo stopped"},
        # restored: the earlier record reads back
        {"id": "restore", "guest": f"cp -a {ROOT}/{n}/state.db.bak {db} && echo restored"},
        agent(n, port) | {"id": "restoredAgent"},
        mcp("listAgain", "tools/list", **o),
        call("kept", "get_operation", {"operationId": "${KEPT}"}, **o),
    ]
    reqs = [
        req(FAULTS, "A damaged state store stops startup",
            a("taskDone", "response.body.result.status", equals="completed"),
            a("startTruncated", "stdout", matches="^exit=[1-9]"),
            a("listenTruncated", "json.addresses", equals=[]),
            a("afterTruncated", "stdout", equalsStep={"step": "truncate", "path": "stdout"}),
            a("startHeader", "stdout", matches="^exit=[1-9]"),
            a("afterHeader", "stdout", equalsStep={"step": "header", "path": "stdout"}),
            ok("restoredAgent"),
            a("kept", "response.body.result.structuredContent.status", equals="completed"),
            scenario="Truncated store, and an overwritten header: startup exits non-zero, binds nothing and writes "
                     "nothing; the restored store starts with its records intact (D31)"),
        req(FAULTS, "A damaged state store stops startup",
            a("startPage", "stdout", matches="^exit=[0-9]+\n$"),
            a("pageRead", "stdout", **{"in": ["200\n", "000\n"]}),
            scenario="A page overwritten in the middle: the agent either refuses to start or serves; it does not "
                     "crash while starting"),
    ]
    return journey("e2e6.store-damage",
                   "D31: the agent's state.db truncated, its header overwritten, and a middle page overwritten; "
                   "startup refuses without serving or writing, and the restored store starts with the earlier "
                   "task's record intact.", reqs, steps, remove(n), minutes=15)


# --- 3. killed during a plan step and during store writes ----------------------------------------------

def kill() -> dict:
    n, port = "kp", 3042
    o = on(n, port)
    rel = ".config/opute-e2e/kill.txt"
    steps = base.TOOL_STEPS + [
        {"id": "slow", "guest": f"{T} slow start --port 8799 --delay 40"},
        agent(n, port, root=True),
        mcp("list", "tools/list", **o),
        call("plan", "run_host_plan", {"plan": plan("e2e6.kill", "e2e6-kill-1", rel, slow=True)}, **o,
             capture={"RUN": "response.body.result.structuredContent.runId"}),
        poll_run("running", "RUN", done=("running", "working"), timeout=60, **o),
        {"id": "kill", "guest": f"sleep 3; systemctl kill -s KILL e2e-agent-{n}.service; sleep 1; "
                                f"! systemctl is-active -q e2e-agent-{n}.service && echo killed"},
        agent(n, port, root=True) | {"id": "restart"},
        mcp("listAgain", "tools/list", **o),
        call("runAfter", "get_host_plan_run", {"runId": "${RUN}"}, **o),
        {"id": "settle", "guest": "sleep 45; echo waited"},
        call("runLater", "get_host_plan_run", {"runId": "${RUN}"}, **o),
        {"id": "loop", "guest": f"E2E_TOKEN=${{SECRET:{n}}} {T} killloop {n} --port {port} --id e2e-{n}-agent "
                                f"--env MCP_AUTH_TOKEN=${{SECRET:{n}}} --env OPUTE_STANDALONE_ALLOW_MUTATIONS=true "
                                "--rounds 12 --calls 4 --user root"},
        call("afterLoop", "get_host_info", **o),
        {"id": "audit", "guest": f"{T} audit {state(n)}/state.db"},
    ]
    reqs = [
        req(Q5_SPEC, "An interrupted plan or task reads back truthfully",
            a("kill", "stdout", matches="killed\n$"), ok("restart"),
            a("runAfter", "response.body.result.structuredContent.status", equals="unknown"),
            a("runLater", "response.body.result.structuredContent.status", equals="unknown"),
            scenario="A plan killed with SIGKILL during its slow step reads back unknown and is not run again"),
        req(SPEC, "Durable and redacted observations",
            a("loop", "json.restarts", equals=12), a("loop", "json.acceptedTotal", gte=1),
            not_error("afterLoop"),
            a("audit", "json.integrity", equals="ok"), a("audit", "json.unfinished", equals=0),
            scenario="Twelve SIGKILLs with tasks in flight: the store passes its integrity check, every operation "
                     "row is terminal, and the agent serves after each restart"),
    ]
    cleanup = [{"id": "slowStop", "guest": f"{T} slow stop >/dev/null; true"}] + remove(n)
    return journey("e2e6.kill",
                   "An agent killed with SIGKILL during a plan's slow step, then twelve times with task calls in "
                   "flight; the plan reads back unknown and stays so, and the store stays whole.",
                   reqs, steps, cleanup, minutes=20)


# --- 4. a provider command that hangs or prints garbage -------------------------------------------------

def commands() -> dict:
    o_h, o_g = on("hang", 3043), on("junk", 3044)
    stub = f"{ROOT}/stubs"
    steps = base.TOOL_STEPS + [
        {"id": "stubs", "guest": f"mkdir -p {stub} && {T} stub {stub}/incus-hang hang >/dev/null && "
                                 f"{T} stub {stub}/incus-junk garbage >/dev/null && chmod 755 {stub} && echo ready"},
        agent("hang", 3043, f"OPUTE_INCUS_BINARY_PATH={stub}/incus-hang"),
        agent("junk", 3044, f"OPUTE_INCUS_BINARY_PATH={stub}/incus-junk"),
        mcp("listHang", "tools/list", **o_h),
        mcp("listJunk", "tools/list", **o_g),
        call("hangCall", "list_vms", **o_h),
        # get_host_info runs three incus commands, 45 s each; the next call
        # must be one that does not wait on incus.
        call("hangNext", "detect_host_platform", **o_h),
        call("junkCall", "list_vms", **o_g),
        call("junkInfo", "get_vm_info", {"name": "e2e-none"}, **o_g),
        call("junkNext", "get_host_info", **o_g),
        {"id": "noStubLeft", "guest": "sleep 2; pgrep -f incus-hang | wc -l"},
    ]
    reqs = [
        req(FAULTS, "A failing provider command ends the call, not the agent",
            ok("stubs"), is_error("hangCall"), a("hangCall", "response.status", equals=200),
            a("hangCall", "ms", lte=60000), not_error("hangNext"),
            scenario="Hanging command: list_vms ends in a typed error within the 45 s discovery timeout, and the "
                     "next call is served (D31)"),
        req(FAULTS, "A failing provider command ends the call, not the agent",
            is_error("junkCall"), is_error("junkInfo"), not_error("junkNext"),
            scenario="Garbage output: list_vms and get_vm_info end in typed errors, and the next call is served "
                     "(D31)"),
    ]
    cleanup = remove("hang", "junk") + [{"id": "stubsGone", "guest": f"pkill -f incus-hang; rm -rf {stub}; "
                                                                     f"rmdir {ROOT} 2>/dev/null; true"}]
    return journey("e2e6.commands",
                   "D31: an agent whose incus is a stand-in that never exits, and one whose incus prints garbage; "
                   "each call ends in a typed error and the agent keeps serving.", reqs, steps, cleanup, minutes=10)


# --- 5. the clock steps back --------------------------------------------------------------------

def clock() -> dict:
    n, port = "clk", 3045
    o = on(n, port)
    steps = base.TOOL_STEPS + [
        agent(n, port, root=True),
        mcp("list", "tools/list", **o),
        {"id": "cred", "guest": f"cat {state(n)}/credentials/host-agent-provider.json",
         "capture": {"CRED_ID": "json.client_id", "CRED_SECRET": {"path": "json.client_secret", "secret": True}}},
        {"id": "token", "http": "POST", "port": port, "path": "/oauth/token",
         "form": {"grant_type": "client_credentials", "resource": f"http://127.0.0.1:{port}/mcp"},
         "basic": ["${CRED_ID}", "${CRED_SECRET}"],
         "capture": {"ISSUED": {"path": "response.body.access_token", "secret": True}}},
        call("held", "run_host_command", {"command": "sleep 40"}, meta=TASKS_META, **o,
             capture={"HELD": "response.body.result.taskId"}),
        call("capBefore", "get_host_capacity", **o),
        {"id": "stepBack", "guest": "timedatectl set-ntp false; date -s '-1 hour' >/dev/null && date -u +%FT%T"},
        call("bootstrapAfter", "get_host_info", **o),
        call("issuedAfter", "get_host_info", port=port, auth="var:ISSUED"),
        call("capAfter", "get_host_capacity", **o),
        call("newWork", "run_host_command", {"command": "echo after step"}, meta=TASKS_META, **o),
        {"id": "heldDone", "mcp": "tasks/get", "params": {"taskId": "${HELD}"}, "meta": TASKS_META, **o,
         "until": {"path": "response.body.result.status", "in": ["completed", "failed", "cancelled"]},
         "timeout": 90, "interval": 3},
    ]
    reqs = [
        req(FAULTS, "A clock stepped back does not break the agent",
            a("stepBack", "exit", equals=0), not_error("bootstrapAfter"), not_error("issuedAfter"),
            a("capAfter", "response.body.result.structuredContent.reservations",
              equalsStep={"step": "capBefore", "path": "response.body.result.structuredContent.reservations"}),
            a("heldDone", "response.body.result.status", equals="completed"),
            scenario="Clock steps back an hour: bootstrap and issued tokens still authenticate, the running "
                     "task's reservation is kept, and new work is admitted (D31)"),
    ]
    cleanup = [{"id": "clockBack", "guest": "date -s '+1 hour' >/dev/null; timedatectl set-ntp true; "
                                            "systemctl stop systemd-timedated.service; true"}] + remove(n)
    return journey("e2e6.clock",
                   "D31: the guest clock stepped back one hour while an agent holds a reservation for a running "
                   "task; tokens still authenticate, the reservation is kept and new work is admitted.",
                   reqs, steps, cleanup, minutes=10,
                   # Turning NTP off disables timesyncd until cleanup turns it back on.
                   allow=("files//etc/systemd/system/dbus-org.freedesktop.timesync1.service",
                          "files//etc/systemd/system/sysinit.target.wants/systemd-timesyncd.service",
                          "system/unitFiles/dbus-org.freedesktop.timesync1.service",
                          "system/unitFiles/systemd-timesyncd.service",
                          "system/units/systemd-timesyncd.service", "system/units/systemd-timedated.service"))


# --- 6. admission under pressure, and the body limit -------------------------------------------------

def pressure() -> dict:
    lim, lo, body = on("lim", 3046), on("low", 3047), on("big", 3048)
    steps = base.TOOL_STEPS + [
        agent("lim", 3046, "OPUTE_HOST_MAX_NORMAL_OPERATIONS=1", root=True),
        mcp("listLim", "tools/list", **lim),
        call("first", "run_host_command", {"command": "sleep 20"}, meta=TASKS_META, **lim, retryOn=[],
             capture={"FIRST": "response.body.result.taskId"}),
        call("second", "run_host_command", {"command": "echo second"}, meta=TASKS_META, **lim, retryOn=[],
             capture={"SECOND": "response.body.result.taskId"}),
        done("secondDone", "SECOND", lim),
        call("third", "ensure_host_file", {"path": ".config/opute-e2e/p.txt", "content": "x\n", "scope": "user"},
             **lim, retryOn=[]),
        call("readWhileFull", "get_host_info", **lim),
        {"id": "firstDone", "mcp": "tasks/get", "params": {"taskId": "${FIRST}"}, "meta": TASKS_META, **lim,
         "until": {"path": "response.body.result.status", "in": ["completed", "failed", "cancelled"]},
         "timeout": 60, "interval": 2},
        call("afterFirst", "run_host_command", {"command": "echo after"}, meta=TASKS_META, **lim, retryOn=[]),
        {"id": "limOps", "guest": f"{T} audit {state('lim')}/state.db"},
        agent("low", 3047, "OPUTE_HOST_MIN_AVAILABLE_MEMORY_BYTES=1125899906842624",
              "OPUTE_HOST_MIN_AVAILABLE_DISK_BYTES=1125899906842624"),
        mcp("listLow", "tools/list", **lo),
        call("lowWrite", "ensure_host_file", {"path": ".config/opute-e2e/q.txt", "content": "x\n", "scope": "user"},
             **lo, retryOn=[]),
        call("lowTask", "run_host_command", {"command": "echo low"}, meta=TASKS_META, **lo, retryOn=[],
             capture={"LOW": "response.body.result.taskId"}),
        done("lowTaskDone", "LOW", lo),
        call("lowRead", "get_host_info", **lo),
        agent("big", 3048),
        mcp("listBig", "tools/list", **body),
        {"id": "rssBefore", "guest": f"{T} procstat e2e-agent-big.service"},
        {"id": "bigBody", "guest": f"E2E_TOKEN=${{SECRET:big}} {T} bigbody --port 3048 --declared 1073741824 "
                                   "--send 8388608"},
        {"id": "rssAfter", "guest": f"{T} procstat e2e-agent-big.service"},
        call("bigNext", "get_host_info", **body),
        # D41: the serving relay, with nothing upstream, refuses an oversized inference body
        call("relay", "ensure_local_llm_relay", {
            "sessionId": "e2e-relay", "listenHost": "127.0.0.1", "listenPort": 3051, "targetHost": "127.0.0.1",
            "targetPort": 9, "incomingToken": "${SECRET:relayIn}", "allowedSourceCIDRs": ["127.0.0.1/32"]},
             **body),
        {"id": "relayRssBefore", "guest": f"{T} procstat e2e-agent-big.service"},
        {"id": "relayBody", "guest": f"E2E_TOKEN=${{SECRET:relayIn}} {T} bigbody --port 3051 --path "
                                     "/v1/chat/completions --declared 1073741824 --send 41943040 --limit 33554432"},
        {"id": "relayRssAfter", "guest": f"{T} procstat e2e-agent-big.service"},
        call("relayNext", "get_host_info", **body),
    ]
    refused = lambda s: [is_error(s), a(s, "response.body.result.structuredContent.owner", equals="admission")]  # noqa
    # A task's admission runs inside the task: the refusal is its result.
    task_refused = lambda s: [a(s, "response.body.result.result.isError", equals=True),  # noqa
                              a(s, "response.body.result.result.structuredContent.owner", equals="admission")]
    reqs = [
        req(SPEC, "Typed target and effect admission",
            a("first", "response.body.result.status", equals="working"),
            *task_refused("secondDone"), *refused("third"), not_error("readWhileFull"),
            a("firstDone", "response.body.result.status", equals="completed"),
            a("afterFirst", "response.body.result.status", equals="working"),
            a("limOps", "json.operations.run_host_command", exists=True),
            scenario="Beyond the operation limit new work is refused with a typed admission error (a task's as its "
                     "result) and nothing is queued; the running task finishes and new work is admitted after it"),
        req(SPEC, "Typed target and effect admission",
            *refused("lowWrite"), *task_refused("lowTaskDone"), not_error("lowRead"),
            scenario="Below the memory and disk floors, effectful work is refused with a typed admission error"),
        req(BODY, "An MCP request body is read only up to its limit",
            a("bigBody", "json.status", equals=413), a("bigBody", "json.bodyLimitText", equals=True),
            a("bigBody", "json.seconds", lte=10), not_error("bigNext"),
            a("rssAfter", "json.rssBytes", lte=BUDGET["bodyLimitRssBytesMax"]),
            scenario="A 1 GiB declared body is answered 413 after 4 MiB, and the agent's resident memory stays "
                     "under the budget (D29)"),
        req(RELAY_SPEC, "The serving relay reads a request body only up to 32 MiB",
            a("relay", "response.body.result.structuredContent.ready", equals=True),
            a("relayBody", "json.status", equals=413), a("relayBody", "json.bodyLimitText", equals=True),
            a("relayBody", "json.seconds", lte=10), not_error("relayNext"),
            a("relayRssAfter", "json.rssBytes", lte=BUDGET["bodyLimitRssBytesMax"]),
            scenario="A 1 GiB declared inference body is answered 413 after 32 MiB, and the agent's resident "
                     "memory stays under the budget (D41)"),
    ]
    return journey("e2e6.pressure",
                   "Admission under pressure: an agent with one normal operation slot refuses work beyond it and "
                   "admits again after; one whose memory and disk floors cannot be met refuses effectful work; one "
                   "sent a 1 GiB body answers 413 with bounded memory (D29), and so does its serving relay (D41).",
                   reqs, steps, remove("lim", "low", "big"), minutes=10)


# --- 7. resource budget under 15 minutes of mixed load --------------------------------------------------

def budget() -> dict:
    n, port = "bud", 3049
    o = on(n, port)
    b = BUDGET
    steps = base.TOOL_STEPS + [
        agent(n, port, f"OPUTE_AUDIT_RETENTION_ROWS={b['auditRetentionRows']}", root=True),
        mcp("list", "tools/list", **o),
        {"id": "load", "guest": f"E2E_TOKEN=${{SECRET:{n}}} {T} load --port {port} --seconds {b['seconds']} "
                                f"--sample-unit e2e-agent-{n}.service --state {state(n)}",
         # A guest step's default limit is 600 s; the load runs for the budget's seconds.
         "timeout": b["seconds"] + 180},
        {"id": "audit", "guest": f"{T} audit {state(n)}/state.db"},
        # one prune interval after the load: every audit row beyond the bound is gone
        {"id": "pruned", "guest": f"sleep 70; {T} audit {state(n)}/state.db"},
    ]
    g = "json.growthAfterWarmup."
    reqs = [
        req(SPEC, "Durable and redacted observations",
            a("load", "json.calls", gte=b["minCalls"]), a("load", "json.errors", equals=0),
            a("load", g + "rssBytes", lte=b["rssGrowthBytesMax"]), a("load", g + "fds", lte=b["fdGrowthMax"]),
            a("load", g + "threads", lte=b["threadGrowthMax"]),
            a("load", g + "storeBytesPerCall", lte=b["storeBytesPerCallMax"]),
            a("load", "json.peak.rssBytes", lte=b["peakRssBytesMax"]),
            a("audit", "json.integrity", equals="ok"), a("audit", "json.unfinished", equals=0),
            scenario="Fifteen minutes of mixed read, write, task and Incus calls stay inside "
                     "evidence/hardening/resource-budget.json after a two-minute warm-up"),
        req(RETENTION_SPEC, "Audit rows are kept within an age and a row bound",
            a("load", "json.calls", gte=b["auditRetentionRows"] * 2),
            a("load", g + "storeBytesLastFiveMinutes", lte=b["storeLateGrowthBytesMax"]),
            a("pruned", "json.auditRows", lte=b["auditRetentionRows"]),
            a("pruned", "json.integrity", equals="ok"),
            scenario="With a row bound far below the call count, one prune interval after the load the audit "
                     "holds at most the bound, and the store's growth over the last five minutes stays under the "
                     "budget (D42)"),
    ]
    return journey("e2e6.budget",
                   "Resource budget (m16): fifteen minutes of mixed calls on one agent; resident memory, open file "
                   "descriptors, threads and store size are sampled every 30 s and their growth after warm-up is "
                   "held to the committed budget; with an audit row bound below the call count, the audit stays "
                   "within it (D42).", reqs, steps, remove(n), minutes=25)


# --- 8. a child's environment (D40) ----------------------------------------------------------------

CHILD_SECRETS = ("MCP_AUTH_TOKEN", "CLOUDFLARE_API_TOKEN", "TAILSCALE_AUTH_KEY", "OPUTE_E2E_MARKER")
V6 = "vm:local:e2e-v6"
INCUS_ALLOW = ("incus/*", "files//home/opute/.config/incus*", "system/units/sys-devices-virtual-net-tap*",
               "system/units/sys-subsystem-net-devices-tap*", "system/units/var-lib-incus-devices-*")


def child_env() -> dict:
    n, port = "cenv", 3050
    o = on(n, port)
    recipe = f"{ROOT}/child-env-recipe.yaml"
    probe = lambda match: (f"{T} childenv e2e-agent-{n}.service '{match}' "  # noqa: E731
                           + " ".join(f"--expect {s}" for s in CHILD_SECRETS) + " --keep PATH")
    steps = base.TOOL_STEPS + [
        agent(n, port, "CLOUDFLARE_API_TOKEN=${SECRET:cfToken}", "TAILSCALE_AUTH_KEY=${SECRET:tsKey}",
              "OPUTE_E2E_MARKER=e2e-marker", root=True),
        mcp("list", "tools/list", **o),
        call("vmCall", "provision_vm", {"name": "e2e-v6", "instanceType": "virtual-machine",
                                        "image": "parity-noble-vm", "cpus": 1, "memory": "1GiB", "disk": "10GiB"},
             meta=TASKS_META, **o, capture={"VM": "response.body.result.taskId"}),
        {**done("vm", "VM", o), "timeout": 900, "interval": 3},
        call("console", "stream_vm_console", {"uri": V6}, **o),
        {"id": "consoleEnv", "guest": probe("console e2e-v6")},
        call("ctrlA", "send_console_input", {"operationId": "console-e2e-v6", "data": "\u0001"}, **o),
        call("detach", "send_console_input", {"operationId": "console-e2e-v6", "data": "q"}, **o),
        {"id": "recipeCopy", "copy": "recipe", "to": recipe, "mode": "0644"},
        call("recipeRun", "run_host_local_recipe", {"source": recipe}, **o,
             capture={"RRUN": "response.body.result.structuredContent.runId"}),
        # the step's workload runs 40 s; the agent's systemd-run client waits on it
        {"id": "recipeEnv", "guest": (f"for i in $(seq 40); do out=$({probe('systemd-run')}); "
                                      "echo \"$out\" | grep -q '\"found\": [1-9]' && break; sleep 1; done; "
                                      "echo \"$out\"")},
        poll_run("recipeDone", "RRUN", timeout=120, **o),
    ]
    clean = lambda step: [a(step, "json.agentHasExpected", equals=True), a(step, "json.found", gte=1),  # noqa: E731
                          a(step, "json.leaked", equals=[]), a(step, "json.childrenKeep", equals=True)]
    reqs = [
        req(CHILD_SPEC, "A child process never receives the agent's secrets",
            a("vm", "response.body.result.status", equals="completed"),
            a("console", "response.body.result.structuredContent.status", equals="started"),
            *clean("consoleEnv"),
            scenario="With the bearer, two provider credentials and an OPUTE_ setting in the agent's environment, "
                     "its incus console process holds PATH and none of them"),
        req(CHILD_SPEC, "A child process never receives the agent's secrets",
            *clean("recipeEnv"),
            a("recipeDone", "response.body.result.structuredContent.status", equals="completed"),
            scenario="While a recipe step's run_host_command workload runs, the systemd-run client the agent "
                     "started for it holds PATH and none of the agent's secrets"),
    ]
    files = {**base.TOOL_FILES, "recipe": base.inp("${REPO}/evidence/e2e/journeys/inputs/e2e-child-env-recipe.yaml")}
    # as e2e2.incus: the VM, then the image volume its launch cached
    cleanup = [{"id": "cleanVm", "guest": (
        f"incus delete -f e2e-v6 2>/dev/null; rm -f {recipe}; incus storage volume list default -f csv | "
        "awk -F, '$1==\"image\"{print $2}' | xargs -r -I{} incus storage volume delete default image/{}; true")}] \
        + remove(n)
    return journey("e2e6.child-env",
                   "A child's environment (D40): an agent holding its bearer, two provider credentials and an "
                   "OPUTE_ setting opens a VM console and runs a recipe step; /proc shows neither child holds "
                   "any of them.", reqs, steps, cleanup, minutes=20, files=files, allow=INCUS_ALLOW)


JOURNEYS = [disk_full, store_damage, kill, commands, clock, pressure, budget, child_env]


def render() -> dict[str, str]:
    return {f"{j['id']}.json": json.dumps(j, indent=1) + "\n" for j in (make() for make in JOURNEYS)}


if __name__ == "__main__":
    for name, text in render().items():
        (HERE / name).write_text(text)
        print(name)
