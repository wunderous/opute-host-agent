"""M12 (without Platform): cutover rehearsals on isolated state copies.

Go builds representative standalone state under one explicit identity: a
task waiting for input (A), a cancelled task (B), a resumed task (C), a
task whose operation failed admission (D, completed with a resource_binding
error result), and a task still working, blocked mid-shim, when Go is told
to stop (E).
The stopped state directory is snapshotted with a checksum of every file,
and every phase below runs on its own copy, so the snapshot is never
written.

* reference: Go restarts on a copy. Its own answers are the oracle for a
  restart (the resumed task C restores as failed, "The Host Agent restarted
  before the task completed.", from its stale working snapshot, as M5
  cross-read records; the working task E is run again and ends with its own
  binding error, since the copies have no shim and no Incus), so the target
  is compared with Go's restart, and Go's pre-stop answers are recorded
  next to it.
* forward: the target (Rust, or Go as the control) starts on a copy, is
  queried, restarted and queried again, then accepts work of its own (a new
  waiting task, a cancellation).
* rollback: Go starts on that same copy. Every task is still there with the
  status and message the target last reported (Go's own restart drops the
  pending inputRequests of a task cancelled before it, on either side), or
  Go refuses to start; nothing is dropped silently.

Go -> Go -> Go and Go -> Rust -> Go are compared after the declared masks.
Three drills run on their own sandboxes: a synchronous call in flight when
the agent is told to stop (drain or refuse), a long task in flight when it
is told to stop (and what a restart makes of it), and two agents on one state
directory (Go takes no state-directory lock; the observation is what each
implementation does, compared).
"""
from __future__ import annotations

import hashlib
import json
import os
import shutil
import signal
import threading
import time
from pathlib import Path
from typing import Any

from . import agent, canon, runner, shims

FIXTURE = runner.FIXTURE_DIR / "shims" / "incus-gated-list.json"
IDENTITY = "parity-m12a-agent"
TOKEN = "parity-m12a-token-0123456789abcdef"
ENV = {**runner.PROFILES["standalone"], "OPUTE_STANDALONE_ALLOW_MUTATIONS": "true"}
TERMINAL = ("completed", "failed", "cancelled")
ORIGIN_TASKS = ("A", "B", "C", "D", "E")


def _sandbox(impl: agent.Impl, run_id: str, fixture: Path | None = FIXTURE) -> agent.Sandbox:
    sandbox = agent.Sandbox(impl.label, run_id, fixture)
    # One explicit identity and token for every phase of a rehearsal.
    sandbox.agent_id, sandbox.token = IDENTITY, TOKEN
    return sandbox


def _start(impl: agent.Impl, sandbox: agent.Sandbox, env: dict | None = None) -> agent.Server:
    server = agent.Server(impl, sandbox, ["serve", "--mode=standalone"], env or ENV)
    ready = server.wait_ready(30)
    if not ready.get("ready"):
        server.stop()
        raise RuntimeError(f"{impl.label} did not become ready: {ready}: "
                           f"{(sandbox.root / 'server.log').read_text(errors='replace')[-800:]}")
    return server


def _stop(server: agent.Server) -> dict:
    started = time.monotonic()
    out = server.stop()
    out["seconds"] = round(time.monotonic() - started, 1)
    return out


def _result(response: dict) -> dict:
    return (response.get("body") or {}).get("result") or {}


def _task(sandbox: agent.Sandbox, task_id: str) -> dict:
    response = agent.mcp_call(sandbox, "tasks/get", {"taskId": task_id}, name=task_id)
    return {"status": response.get("status"), "body": response.get("body")}


def _until_terminal(sandbox: agent.Sandbox, task_id: str, timeout: float = 20) -> dict:
    deadline = time.monotonic() + timeout
    while True:
        got = _task(sandbox, task_id)
        if _result(got).get("status") in TERMINAL or time.monotonic() > deadline:
            return got
        time.sleep(0.1)


def _new_input_task(sandbox: agent.Sandbox) -> str:
    call = agent.mcp_call(sandbox, "tools/call", {
        "name": "request_task_input", "arguments": {"prompt": "Continue?", "responseType": "boolean"}},
        name="request_task_input")
    task_id = _result(call).get("taskId")
    if not task_id:
        raise RuntimeError(f"accepted task returned no handle: {call}")
    return task_id


def _status(response: dict) -> Any:
    """A task's status, or the JSON-RPC error that answered instead."""
    body = response.get("body") or {}
    if "error" in body:
        return {"error": (body["error"] or {}).get("code")}
    return (body.get("result") or {}).get("status")


def _health(sandbox: agent.Sandbox) -> dict:
    body = agent.http_request(sandbox, "GET", "/health").get("body") or {}
    return {"agentId": body.get("agentId"), "instanceId": body.get("instanceId"), "ok": body.get("ok")}


def _answers(sandbox: agent.Sandbox, ids: dict[str, str]) -> dict:
    return {name: _task(sandbox, task_id) for name, task_id in sorted(ids.items())}


def _copy_state(source: Path, sandbox: agent.Sandbox) -> None:
    for sub in ("state", "instance"):
        shutil.rmtree(sandbox.root / sub)
        shutil.copytree(source / sub, sandbox.root / sub)


def _release(root: Path, timeout: float, gate: str = "list") -> dict:
    """Release the gate if a shim still waits on it. A shim that died with
    its agent leaves a FIFO without a reader (ENXIO): that is recorded, not
    raised."""
    alive = bool(shims.live_shims(root))
    try:
        released = shims.release_gate(root, gate, timeout)["released"]
    except OSError as error:
        released = f"no reader ({error.errno})"
    return {"shimAliveAtRelease": alive, "released": released}


def _drop_dead_markers(root: Path, gate: str) -> list[str]:
    dropped = []
    for marker in sorted((root / "gates").glob(f"{gate}.waiting.*")):
        try:
            os.kill(int(marker.name.rsplit(".", 1)[1]), 0)
        except ProcessLookupError:
            marker.unlink()
            dropped.append(marker.name)
    return dropped


def _stop_in_flight(server: agent.Server, sandbox: agent.Sandbox, gate: str) -> dict:
    """Tell the agent to stop while a shim is blocked on `gate`; record
    whether it waits (drain) or exits (refuse), then let the shim return and
    record how the agent exits."""
    out: dict[str, Any] = {"shimAliveAtSignal": bool(shims.live_shims(sandbox.root))}
    server.proc.send_signal(signal.SIGINT)
    time.sleep(2)
    out["aliveWhileInFlight"] = server.proc.poll() is None
    out["newConnectionAccepted"] = agent.port_open(sandbox.port)
    out["gate"] = _release(sandbox.root, 5, gate)
    try:
        server.proc.wait(30)
    except Exception:
        pass
    out["exit"] = server.proc.poll()
    server.stop()
    return out


def checksums(root: Path) -> dict[str, str]:
    return {str(p.relative_to(root)): hashlib.sha256(p.read_bytes()).hexdigest()
            for p in sorted(root.rglob("*")) if p.is_file()}


def build_origin(go: agent.Impl, run_id: str, snapshot: Path) -> dict:
    """Go's representative state, its pre-stop answers, and the snapshot."""
    sandbox = _sandbox(go, f"{run_id}-origin")
    server = _start(go, sandbox)
    released = False
    try:
        ids = {"A": _new_input_task(sandbox), "B": _new_input_task(sandbox), "C": _new_input_task(sandbox)}
        agent.mcp_call(sandbox, "tasks/cancel", {"taskId": ids["B"]}, name=ids["B"])
        agent.mcp_call(sandbox, "tasks/update", {"taskId": ids["C"], "inputResponses": {"response": True}},
                       name=ids["C"])
        failed = agent.mcp_call(sandbox, "tools/call", {"name": "start_vm",
                                                        "arguments": {"uri": "vm:other-tenant:web1"}}, name="start_vm")
        ids["D"] = _result(failed).get("taskId")
        _until_terminal(sandbox, ids["D"])
        # /health lists Incus itself: its shim blocks on the gate until the
        # health probe gives up on it. Read health before E, then drop the
        # markers of shims that are gone, so the wait below means E's shim.
        health = _health(sandbox)
        _drop_dead_markers(sandbox.root, "list")
        working = agent.mcp_call(sandbox, "tools/call", {"name": "start_vm", "arguments": {"uri": "vm:local:web1"}},
                                 name="start_vm")
        ids["E"] = _result(working).get("taskId")
        reached = shims.await_gate(sandbox.root, "list", 10)
        pre_stop = {"answers": _answers(sandbox, ids), "health": health, "gateReached": reached}
        # The in-flight drill: Go is told to stop while E is still working.
        pre_stop["stop"] = _stop_in_flight(server, sandbox, "list")
        released = True
        if snapshot.exists():
            shutil.rmtree(snapshot)
        snapshot.mkdir(parents=True)
        for sub in ("state", "instance"):
            shutil.copytree(sandbox.root / sub, snapshot / sub)
        return {"ids": ids, "preStop": pre_stop, "checksums": checksums(snapshot)}
    finally:
        server.stop()
        if not released:
            _release(sandbox.root, 1)
        shutil.rmtree(sandbox.root, ignore_errors=True)


def _phase(impl: agent.Impl, snapshot: Path, run_id: str, ids: dict[str, str], work=None) -> dict:
    """Start impl on a fresh copy, query, restart, query again, optionally
    accept new work; returns the answers and the copy's sandbox root."""
    sandbox = _sandbox(impl, run_id, None)
    _copy_state(snapshot, sandbox)
    out: dict[str, Any] = {}
    server = _start(impl, sandbox)
    try:
        out["first"] = _answers(sandbox, ids)
        out["health"] = _health(sandbox)
        out["stop1"] = _stop(server)
        server = _start(impl, sandbox)
        out["restarted"] = _answers(sandbox, ids)
        if work:
            out["accepted"] = work(sandbox)
            out["afterWork"] = _answers(sandbox, {**ids, **out["accepted"]})
        out["stop2"] = _stop(server)
    finally:
        server.stop()
    out["_root"] = str(sandbox.root)
    out["_sandbox"] = sandbox
    return out


def _accept(sandbox: agent.Sandbox) -> dict[str, str]:
    """Work the target accepts during its era: a new waiting task, and a
    cancellation of A (Go's waiting task)."""
    new = _new_input_task(sandbox)
    return {"G": new}


def rehearse(go: agent.Impl, target: agent.Impl, run_id: str, work_dir: Path) -> dict:
    snapshot = work_dir / f"{run_id}-snapshot"
    origin = build_origin(go, run_id, snapshot)
    ids = origin["ids"]
    reference = _phase(go, snapshot, f"{run_id}-reference", ids)
    shutil.rmtree(reference.pop("_root"), ignore_errors=True)
    reference.pop("_sandbox")

    def work(sandbox: agent.Sandbox) -> dict[str, str]:
        accepted = _accept(sandbox)
        agent.mcp_call(sandbox, "tasks/cancel", {"taskId": ids["A"]}, name=ids["A"])
        return accepted

    forward = _phase(target, snapshot, f"{run_id}-forward", ids, work)
    forward_root = Path(forward.pop("_root"))
    forward.pop("_sandbox")
    # Rollback: Go on the very copy the target wrote.
    rollback_sandbox = _sandbox(go, f"{run_id}-rollback", None)
    _copy_state(forward_root, rollback_sandbox)
    shutil.rmtree(forward_root, ignore_errors=True)
    rollback: dict[str, Any] = {}
    all_ids = {**ids, **forward["accepted"]}
    server = agent.Server(go, rollback_sandbox, ["serve", "--mode=standalone"], ENV)
    ready = server.wait_ready(30)
    rollback["ready"] = bool(ready.get("ready"))
    try:
        if rollback["ready"]:
            rollback["answers"] = _answers(rollback_sandbox, all_ids)
            rollback["health"] = _health(rollback_sandbox)
            rollback["stop"] = _stop(server)
        else:
            rollback["refusal"] = {"ready": ready, "log": (rollback_sandbox.root / "server.log").read_text(
                errors="replace")[-1200:]}
    finally:
        server.stop()
        shutil.rmtree(rollback_sandbox.root, ignore_errors=True)
    unchanged = checksums(snapshot) == origin["checksums"]
    shutil.rmtree(snapshot, ignore_errors=True)
    variables = {**{f"TASK_{k}": v for k, v in all_ids.items()}}
    return {"origin": {"preStop": origin["preStop"], "files": sorted(origin["checksums"])},
            "snapshotUnchanged": unchanged, "reference": reference, "forward": forward,
            "rollback": rollback, "variables": variables}


def drain_drill(impl: agent.Impl, run_id: str) -> dict:
    """A synchronous read blocked mid-shim when the agent is told to stop:
    does the agent finish it (drain) or cut it off (refuse)?"""
    sandbox = _sandbox(impl, run_id)
    server = _start(impl, sandbox, {**runner.PROFILES["standalone"]})
    holder: dict[str, Any] = {}

    def call() -> None:
        holder["response"] = agent.mcp_call(sandbox, "tools/call", {"name": "list_vms", "arguments": {}},
                                            name="list_vms")

    thread = threading.Thread(target=call)
    out: dict[str, Any] = {}
    try:
        thread.start()
        out["gateReached"] = shims.await_gate(sandbox.root, "list", 10)
        server.proc.send_signal(signal.SIGINT)
        time.sleep(1.5)
        out["aliveWhileInFlight"] = server.proc.poll() is None
        out["newConnectionAccepted"] = agent.port_open(sandbox.port)
        out["gate"] = _release(sandbox.root, 5)
        thread.join(30)
        response = holder.get("response") or {}
        result = _result(response)
        out["response"] = {"status": response.get("status"), "isError": result.get("isError"),
                           "hasContent": bool(result.get("structuredContent") or result.get("content"))}
        try:
            server.proc.wait(15)
        except Exception:
            pass
        out["exit"] = server.proc.poll()
    finally:
        server.stop()
        shutil.rmtree(sandbox.root, ignore_errors=True)
    return out


# Long tasks told to stop while blocked mid-shim. "binding": start_vm blocks
# in resource binding, before admission (D12: Rust keeps nothing durable for
# a call that has not passed admission; Go has an operation row from task
# creation). "admitted": start_vm on an owned, stopped container passes
# binding and admission and blocks in `incus start`, so both sides hold an
# accepted operation (the M8b estate and its env).
LONG_TASKS = {
    "binding": {"fixture": FIXTURE, "gate": "list", "tool": "start_vm", "arguments": {"uri": "vm:local:web1"},
                "env": {}},
    "admitted": {"fixture": runner.FIXTURE_DIR / "shims" / "incus-gated-start.json", "gate": "start",
                 "tool": "start_vm", "arguments": {"uri": "container:local:box1"},
                 "env": {"OPUTE_HOST_RESOURCE_ENFORCEMENT": "advisory", "OPUTE_HOST_MAX_NORMAL_OPERATIONS": "64",
                         "OPUTE_HOST_MAX_HEAVY_OPERATIONS": "64", "OPUTE_HOST_RESOURCE_CPU_CAPACITY_CORES": "64",
                         "OPUTE_HOST_RESOURCE_MEMORY_CAPACITY_BYTES": "68719476736",
                         "OPUTE_HOST_AGENT_INSTANCE": "parity-owner", "OPUTE_INCUS_OWNERSHIP_MODE": "enforce"}},
}


# D12 (refuse-before-operation-record): Rust persists an operation only for
# a call that passed admission. A task still in resource binding when the
# agent stops has a Go operation row from task creation, which Go restores
# cancelled; Rust has nothing durable, so the restarted agent does not know
# the handle. Everything else in that drill is compared; these per-side
# expectations pin the one difference.
D12_LONG_TASK = {
    "binding": {
        "path": ["afterRestart"],
        "go": {"status": "cancelled", "statusMessage": "The Host Agent shut down before the task completed.",
               "isError": None, "code": None},
        "rust": {"status": {"error": -32603}, "statusMessage": None, "isError": None, "code": None},
    },
}


def task_drill(impl: agent.Impl, run_id: str, case: dict) -> dict:
    """A long task blocked mid-shim when the agent is told to stop: does the
    agent wait for it (drain) or exit (refuse), how does it exit once the
    shim returns, and what is the task after a restart on the same state?"""
    sandbox = _sandbox(impl, run_id, case["fixture"])
    env = {**ENV, **case["env"]}
    server = _start(impl, sandbox, env)
    out: dict[str, Any] = {}
    try:
        call = agent.mcp_call(sandbox, "tools/call", {"name": case["tool"], "arguments": case["arguments"]},
                              name=case["tool"])
        task_id = _result(call).get("taskId")
        out["taskHandle"] = bool(task_id)
        out["gateReached"] = shims.await_gate(sandbox.root, case["gate"], 10)
        out["beforeStop"] = _status(_task(sandbox, task_id))
        out["stop"] = _stop_in_flight(server, sandbox, case["gate"])
        server = _start(impl, sandbox, env)
        got = _until_terminal(sandbox, task_id)
        result = _result(got)
        inner = result.get("result") or {}
        out["afterRestart"] = {"status": _status(got), "statusMessage": result.get("statusMessage"),
                               "isError": inner.get("isError"),
                               "code": (inner.get("structuredContent") or {}).get("code")}
        out["stop2"] = _stop(server)
    finally:
        server.stop()
        shutil.rmtree(sandbox.root, ignore_errors=True)
    return out


def two_writers(impl: agent.Impl, run_id: str) -> dict:
    """Two agents on one state directory (Go takes no lock on it): does the
    second start, does each accept work, and what does a later restart see?"""
    first = _sandbox(impl, f"{run_id}-1", None)
    second = _sandbox(impl, f"{run_id}-2", None)
    shared = {"OPUTE_STANDALONE_STATE_DIR": str(first.root / "state"),
              "OPUTE_HOST_AGENT_INSTANCE_ROOT": str(first.root / "instance")}
    out: dict[str, Any] = {}
    one = _start(impl, first)
    two = agent.Server(impl, second, ["serve", "--mode=standalone"], {**ENV, **shared})
    ready = two.wait_ready(30)
    out["secondStarted"] = bool(ready.get("ready"))
    try:
        ids = {"X": _new_input_task(first)}
        if out["secondStarted"]:
            ids["Y"] = _new_input_task(second)
            out["crossVisible"] = {"XfromSecond": _status(_task(second, ids["X"])),
                                   "YfromFirst": _status(_task(first, ids["Y"]))}
        else:
            out["secondRefusal"] = (second.root / "server.log").read_text(errors="replace")[-600:]
        out["stops"] = [_stop(one), _stop(two)]
        third = _start(impl, first)
        out["afterRestart"] = {k: _status(_task(first, v)) for k, v in ids.items()}
        out["stops"].append(_stop(third))
    finally:
        one.stop()
        two.stop()
        shutil.rmtree(first.root, ignore_errors=True)
        shutil.rmtree(second.root, ignore_errors=True)
    return out


_RFC3339 = __import__("re").compile(r"^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(\.\d+)?(Z|[+-]\d\d:\d\d)$")
_TIME_KEYS = ("createdAt", "lastUpdatedAt")


def _normalized(doc: Any, variables: dict) -> tuple[Any, list]:
    """Task ids become ${TASK_X}; task wall-clock times (second resolution)
    become <rfc3339> when they have that format, and are reported when they
    do not."""
    text = json.dumps(doc, sort_keys=True)
    for name, literal in sorted(variables.items(), key=lambda kv: -len(kv[1])):
        text = text.replace(literal, "${" + name + "}")
    violations: list = []

    def walk(value: Any, path: str) -> Any:
        if isinstance(value, dict):
            out = {}
            for k, v in value.items():
                if k in _TIME_KEYS and isinstance(v, str):
                    if _RFC3339.match(v):
                        out[k] = "<rfc3339>"
                    else:
                        violations.append(f"{path}/{k}")
                        out[k] = v
                else:
                    out[k] = walk(v, f"{path}/{k}")
            return out
        if isinstance(value, list):
            return [walk(v, path) for v in value]
        return value

    return walk(json.loads(text), ""), violations


def evaluate(outcome: dict) -> dict:
    """Checks that hold for one rehearsal on its own, and the comparison of
    the Go control with Rust."""
    out: dict[str, Any] = {}
    for target in ("go", "rust"):
        rehearsals = outcome["rehearsals"][target]
        checks = []
        for r in rehearsals:
            if "error" in r:
                checks.append({"error": r["error"]})
                continue
            norm = lambda doc: _normalized(doc, r["variables"])[0]
            ref = norm(r["reference"]["first"])
            fwd1 = norm(r["forward"]["first"])
            fwd2 = norm(r["forward"]["restarted"])
            after = _statuses(r["forward"]["afterWork"])
            back = _statuses(r["rollback"].get("answers", {}))
            checks.append({
                "snapshotUnchanged": r["snapshotUnchanged"] is True,
                "originInFlightAtStop": _origin_in_flight(r["origin"]["preStop"]),
                "forwardEqualsGoRestart": fwd1 == ref,
                "restartStable": fwd2 == fwd1,
                "rollbackKeepsTargetWork": r["rollback"]["ready"] is True and back == after,
                "identityKept": all(h.get("agentId") == IDENTITY for h in
                                    (r["origin"]["preStop"]["health"], r["reference"]["health"],
                                     r["forward"]["health"], r["rollback"].get("health", {}))),
                "cleanStops": all(s == {"exit": 0, "killed": False} for s in
                                  ({k: v for k, v in st.items() if k != "seconds"} for st in
                                   (r["reference"]["stop1"], r["reference"]["stop2"], r["forward"]["stop1"],
                                    r["forward"]["stop2"], r["rollback"].get("stop", {})))),
            })
        out[target] = checks
    comparisons = []
    for g, r in zip(outcome["rehearsals"]["go"], outcome["rehearsals"]["rust"]):
        if "error" in g or "error" in r:
            comparisons.append(["error"])
            continue
        gn, _ = _normalized(_drop_timing(g), g["variables"])
        rn, _ = _normalized(_drop_timing(r), r["variables"])
        comparisons.append(canon.diff(gn, rn))
    drills = {"drain": canon.diff(outcome["drain"]["go"], outcome["drain"]["rust"]),
              **{f"longTask.{case}": _long_task_diff(case, outcome["longTask"][case]) for case in LONG_TASKS},
              "twoWriters": canon.diff(outcome["twoWriters"]["go"], outcome["twoWriters"]["rust"])}
    passed = (all(all(v is True for v in c.values()) for t in ("go", "rust") for c in out[t])
              and all(not c for c in comparisons) and not any(drills.values()))
    return {"checks": out, "comparisons": comparisons, "drills": drills, "pass": passed}


def _long_task_diff(case: str, sides: dict) -> list:
    """Go/Rust differences in one long-task drill, after the declared D12
    projection; a side that misses its D12 expectation is a difference."""
    go, rust = _drop_timing(sides["go"]), _drop_timing(sides["rust"])
    declared = D12_LONG_TASK.get(case)
    if not declared:
        return canon.diff(go, rust)
    key = declared["path"][0]
    out = [{"path": [key, "expectation", side], "expected": declared[side], "got": doc.get(key)}
           for side, doc in (("go", go), ("rust", rust)) if doc.get(key) != declared[side]]
    return out + canon.diff({k: v for k, v in go.items() if k != key}, {k: v for k, v in rust.items() if k != key})


def _origin_in_flight(pre_stop: dict) -> bool:
    """E was working, blocked on the gate, when Go was told to stop; Go
    waited for it and exited 0 once the shim returned."""
    e = _result(pre_stop["answers"]["E"]).get("status")
    stop = pre_stop["stop"]
    return (pre_stop["gateReached"] is True and e == "working" and stop["shimAliveAtSignal"] is True
            and stop["aliveWhileInFlight"] is True and stop["newConnectionAccepted"] is False
            and stop["exit"] == 0)


def _statuses(answers: dict) -> dict:
    """Each task's status and message, or the error that answered."""
    out = {}
    for name, got in answers.items():
        body = got.get("body") or {}
        result = body.get("result") or {}
        out[name] = ({"error": body["error"]} if "error" in body else
                     {"status": result.get("status"), "statusMessage": result.get("statusMessage")})
    return out


def _drop_timing(doc: Any) -> Any:
    """Stop durations are the machine's; the exit status is compared."""
    if isinstance(doc, dict):
        return {k: _drop_timing(v) for k, v in doc.items() if k not in ("seconds", "variables", "files")}
    if isinstance(doc, list):
        return [_drop_timing(v) for v in doc]
    return doc


def run(go: agent.Impl, rust: agent.Impl, out: Path, repeat: int = 3) -> dict:
    out.mkdir(parents=True, exist_ok=False)
    work = out.parent / f".{out.name}-work"
    work.mkdir(parents=True, exist_ok=True)
    provenance = {"sourceLock": json.loads((runner.REPO_ROOT / "baseline/source-lock.json").read_text()),
                  "goBinarySha256": go.sha256(), "rustBinarySha256": rust.sha256(),
                  "driverSha256": runner.sha256_file(Path(__file__)), "harnessSha256": runner.harness_sha256(),
                  "repeat": repeat,
                  # The copies run without shims; nothing on the agents'
                  # PATH (/usr/bin:/bin after the sandbox's own) is Incus.
                  "noIncusOnPath": shutil.which("incus", path="/usr/bin:/bin") is None}
    if not provenance["noIncusOnPath"]:
        raise RuntimeError("an incus binary is on /usr/bin:/bin; the rehearsal copies would reach it")
    outcome: dict[str, Any] = {"rehearsals": {"go": [], "rust": []}, "drain": {}, "longTask": {}, "twoWriters": {}}
    for i in range(repeat):
        for label, target in (("go", go), ("rust", rust)):
            try:
                outcome["rehearsals"][label].append(rehearse(go, target, f"m12a-{label}-{i}", work))
            except Exception as error:  # recorded, never retried
                outcome["rehearsals"][label].append({"error": f"{type(error).__name__}: {error}"})
    for label, impl in (("go", go), ("rust", rust)):
        outcome["drain"][label] = drain_drill(impl, f"m12a-drain-{label}")
        for case, spec in LONG_TASKS.items():
            outcome["longTask"].setdefault(case, {})[label] = task_drill(impl, f"m12a-task-{case}-{label}", spec)
        outcome["twoWriters"][label] = two_writers(impl, f"m12a-writers-{label}")
    shutil.rmtree(work, ignore_errors=True)
    provenance["changedDuringRun"] = (go.sha256() != provenance["goBinarySha256"]
                                      or rust.sha256() != provenance["rustBinarySha256"]
                                      or runner.harness_sha256() != provenance["harnessSha256"]
                                      or runner.sha256_file(Path(__file__)) != provenance["driverSha256"])
    (out / "outcome.json").write_text(json.dumps(outcome, indent=1, sort_keys=True) + "\n")
    result = evaluate(outcome)
    summary = {"provenance": provenance, "pass": result["pass"],
               "files": {"outcome.json": runner.sha256_file(out / "outcome.json")}}
    (out / "results.json").write_text(json.dumps(result, indent=1, sort_keys=True) + "\n")
    summary["files"]["results.json"] = runner.sha256_file(out / "results.json")
    (out / "summary.json").write_text(json.dumps(summary, indent=1, sort_keys=True) + "\n")
    return summary
