"""Scenario execution, twin comparison, and evidence bundles.

A scenario is a versioned JSON document under tools/parity/scenarios/. It runs
unchanged against both sides, each in its own Sandbox. Observations are
normalized (canon.normalize) and diffed. The raw observations, the diff and a
summary are written to an evidence bundle whose file hashes the verifier
re-checks.
"""

from __future__ import annotations

import concurrent.futures
import contextlib
import fcntl
import gzip
import hashlib
import json
import os
import signal
import socket
import threading
import time
from pathlib import Path
from typing import Any

from . import agent, canon, shims

TOOLS_DIR = Path(__file__).resolve().parent.parent
REPO_ROOT = TOOLS_DIR.parent.parent
SCENARIO_DIR = TOOLS_DIR / "scenarios"
FIXTURE_DIR = TOOLS_DIR / "fixtures"
DIVERGENCE_FILE = TOOLS_DIR / "divergences.json"
OBSERVING_MODULES = ("agent", "canon", "runner", "shims")

# Environment profiles. Values may use ${AGENT_ID}, ${PORT}, ${TOKEN} and
# ${SANDBOX}; each side expands them with its own values.
PROFILES: dict[str, dict[str, str]] = {
    "standalone": {
        "OPUTE_REMOTE_AGENT_ID": "${AGENT_ID}",
        "OPUTE_AGENT_MODE": "standalone",
        "OPUTE_STANDALONE_STATE_DIR": "${SANDBOX}/state",
        "OPUTE_HOST_AGENT_INSTANCE_ROOT": "${SANDBOX}/instance",
        "HOST_MCP_BIND_HOST": "127.0.0.1",
        "HOST_MCP_PORT": "${PORT}",
        "MCP_AUTH_TOKEN": "${TOKEN}",
    },
    "platform": {
        "OPUTE_REMOTE_AGENT_ID": "${AGENT_ID}",
        "OPUTE_AGENT_MODE": "platform",
        "OPUTE_STANDALONE_STATE_DIR": "${SANDBOX}/state",
        "OPUTE_HOST_AGENT_INSTANCE_ROOT": "${SANDBOX}/instance",
        "HOST_MCP_BIND_HOST": "127.0.0.1",
        "HOST_MCP_PORT": "${PORT}",
        "MCP_AUTH_TOKEN": "${TOKEN}",
    },
    "empty": {},
}


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def sha256_file(path: Path) -> str:
    return sha256_bytes(Path(path).read_bytes())


def harness_sha256() -> str:
    """Hash of the code and fixtures that produce observations.

    A change to any of these invalidates existing evidence. The verifier,
    capture and canary drivers only read evidence, so they are excluded.
    """
    digest = hashlib.sha256()
    files = ([TOOLS_DIR / "parity" / f"{m}.py" for m in OBSERVING_MODULES]
             + sorted(FIXTURE_DIR.rglob("*.json")) + [DIVERGENCE_FILE])
    for path in files:
        digest.update(str(path.relative_to(TOOLS_DIR)).encode() + b"\0")
        digest.update(path.read_bytes() + b"\0")
    return digest.hexdigest()


def load_scenarios(ids: list[str] | None = None) -> list[dict]:
    scenarios = []
    for path in sorted(SCENARIO_DIR.glob("*.json")):
        doc = json.loads(path.read_text())
        for scenario in doc if isinstance(doc, list) else [doc]:
            scenario["_file"] = str(path.relative_to(REPO_ROOT))
            scenario["_sha256"] = sha256_bytes(canon.canonical_json(
                {k: v for k, v in scenario.items() if not k.startswith("_")}).encode())
            scenarios.append(scenario)
    seen: set[str] = set()
    for s in scenarios:
        if s["id"] in seen:
            raise ValueError(f"duplicate scenario id {s['id']}")
        seen.add(s["id"])
    if ids:
        wanted = set(ids)
        scenarios = [s for s in scenarios if s["id"] in wanted]
        missing = wanted - {s["id"] for s in scenarios}
        if missing:
            raise ValueError(f"unknown scenarios: {sorted(missing)}")
    return scenarios


def _expand_all(sandbox: agent.Sandbox, value: Any) -> Any:
    if isinstance(value, str):
        return sandbox.expand(value)
    if isinstance(value, list):
        return [_expand_all(sandbox, v) for v in value]
    if isinstance(value, dict):
        return {k: _expand_all(sandbox, v) for k, v in value.items()}
    return value


def _env(step: dict) -> dict:
    env = dict(PROFILES[step.get("profile", "empty")])
    env.update(step.get("env", {}))
    return env


def _lookup(doc: Any, path: list) -> Any:
    for key in path:
        if isinstance(doc, dict):
            doc = doc.get(key)
        elif isinstance(doc, list) and isinstance(key, int) and -len(doc) <= key < len(doc):
            doc = doc[key]
        else:
            return None
    return doc


def _mcp_step(sandbox: agent.Sandbox, spec: dict) -> dict:
    """One MCP request, optionally polled until a response field settles.

    "expand": run variables (for example ${TOOL_PREFIX} or a captured
    ${TASK_ID}) in the tool name and params are replaced by this side's values.
    "until": {"path": [...], "in": [...]} repeats the request until the value
    at path is one of the listed values (or the timeout passes); only the
    final response is observed, so poll counts never reach the comparison.
    "capture": {"NAME": [...]} stores the value at a path as a run variable.
    """
    expand = (lambda v: _expand_all(sandbox, v)) if spec.get("expand") else (lambda v: v)
    until = spec.get("until")
    deadline = time.monotonic() + spec.get("timeout", 30)
    while True:
        result = agent.mcp_call(
            sandbox, spec["method"], expand(spec.get("params")),
            token=spec.get("token", "${TOKEN}"), name=expand(spec.get("name")),
            modern=spec.get("modern", True), headers=spec.get("headers"),
            omit_headers=spec.get("omitHeaders"), request_id=spec.get("id", 1),
            meta=expand(spec.get("meta")))
        if not until or _lookup(result, until["path"]) in until["in"] or time.monotonic() > deadline:
            break
        time.sleep(0.05)
    for name, path in spec.get("capture", {}).items():
        value = _lookup(result, path)
        if isinstance(value, str) and value:
            sandbox.captured[name] = value
    return result


@contextlib.contextmanager
def exclusive_ports(scenario: dict):
    if not scenario.get("exclusive"):
        yield
        return
    lock_path = Path.home() / ".cache/opute-parity/default-ports.lock"
    lock_path.parent.mkdir(parents=True, exist_ok=True)
    with _EXCLUSIVE, lock_path.open("a+b") as lock:
        fcntl.flock(lock, fcntl.LOCK_EX)
        try:
            yield
        finally:
            fcntl.flock(lock, fcntl.LOCK_UN)


def execute(impl: agent.Impl, scenario: dict, side: str, run_id: str,
            other: agent.Impl | None = None) -> tuple[dict, dict]:
    # Capture and every twin/shape driver share the same OS-level lease.
    with exclusive_ports(scenario):
        return _execute(impl, scenario, side, run_id, other)


def _execute(impl: agent.Impl, scenario: dict, side: str, run_id: str,
            other: agent.Impl | None = None) -> tuple[dict, dict]:
    """Run one scenario on one side. Returns (observation, variables)."""
    fixture = scenario.get("fixture")
    fixture_path = FIXTURE_DIR / "shims" / f"{fixture}.json" if fixture else None
    sandbox = agent.Sandbox(side=side, run_id=run_id, fixture=fixture_path)
    steps: dict[str, Any] = {}
    server: agent.Server | None = None
    held: list[socket.socket] = []
    stubs: dict[str, agent.HttpStub] = {}
    background: dict[str, concurrent.futures.Future] = {}
    pool = concurrent.futures.ThreadPoolExecutor(max_workers=4)
    try:
        for index, step in enumerate(scenario["steps"]):
            label = step.get("as", f"step{index}")
            missing = [v for v in step.get("requires", []) if v not in sandbox.variables]
            if missing:
                # Both sides skip identically; the evidence records why.
                steps[label] = {"skipped": "requires " + ",".join(missing)}
                continue
            if "cli" in step:
                spec = step["cli"]
                steps[label] = agent.run_cli(
                    impl, sandbox, spec["argv"], _env(spec),
                    timeout=spec.get("timeout", 30), probe_port=spec.get("probePort", False))
            elif "serve" in step:
                spec = step["serve"]
                # "impl": "other" runs the opposite side's binary on this
                # side's state: Go reads what Rust wrote and vice versa.
                chosen = other if spec.get("impl") == "other" and other is not None else impl
                server = agent.Server(chosen, sandbox, spec.get("argv", ["serve"]), _env(spec))
                steps[label] = server.wait_ready(spec.get("timeout", 30), spec.get("readyPort"))
            elif "http" in step:
                spec = step["http"]
                body = spec.get("body")
                raw = json.dumps(body).encode() if isinstance(body, (dict, list)) else (
                    sandbox.expand(body).encode() if isinstance(body, str) else None)
                if raw is not None and "padTo" in spec:
                    # Grow the body to an exact byte size without storing it in
                    # the scenario file: ${PAD} becomes the filler.
                    filler = spec["padTo"] - (len(raw) - len(b"${PAD}"))
                    raw = raw.replace(b"${PAD}", b"x" * filler, 1)
                headers = {k: sandbox.expand(v) for k, v in spec.get("headers", {}).items()}
                steps[label] = agent.http_request(sandbox, spec.get("method", "GET"), spec["path"], headers, raw)
            elif "mcp" in step:
                spec = step["mcp"]
                if "background" in spec:
                    # Runs concurrently with the following steps; a "join"
                    # step records its response.
                    background[spec["background"]] = pool.submit(_mcp_step, sandbox, spec)
                    steps[label] = {"background": spec["background"]}
                else:
                    steps[label] = _mcp_step(sandbox, spec)
            elif "join" in step:
                steps[label] = background.pop(step["join"]).result(timeout=320)
            elif "drain" in step:
                # Wait until every shim process has exited.
                deadline = time.monotonic() + step["drain"].get("timeout", 10)
                while shims.live_shims(sandbox.root) and time.monotonic() < deadline:
                    time.sleep(0.02)
                steps[label] = {"drained": not shims.live_shims(sandbox.root)}
            elif "gate" in step:
                spec = step["gate"]
                # "count": how many shims must be blocked on the gate first.
                count = spec.get("count", 1)
                if "release" in spec:
                    steps[label] = shims.release_gate(sandbox.root, spec["release"], spec.get("timeout", 10), count)
                else:
                    steps[label] = {"gate": spec["await"], "reached": shims.await_gate(
                        sandbox.root, spec["await"], spec.get("timeout", 10), count)}
            elif "raw" in step:
                spec = step["raw"]
                data = sandbox.expand(spec["data"]).encode("latin-1")
                if "padTo" in spec:
                    filler = spec["padTo"] - (len(data) - len(b"${PAD}"))
                    data = data.replace(b"${PAD}", b"a" * filler, 1)
                steps[label] = agent.raw_request(sandbox, data)
            elif "sql" in step:
                spec = step["sql"]
                steps[label] = agent.run_sql(Path(sandbox.expand(spec["db"])),
                                             [sandbox.expand(x) for x in spec["statements"]])
            elif "listeners" in step:
                steps[label] = {"listeners": agent.listeners_of(server.proc.pid)} if server else {"listeners": None}
            elif "write" in step:
                spec = step["write"]
                path = Path(sandbox.expand(spec["path"]))
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(sandbox.expand(spec.get("content", "")))
                path.chmod(int(spec.get("mode", "0644"), 8))
                steps[label] = {"wrote": spec["path"]}
            elif "mkdir" in step:
                Path(sandbox.expand(step["mkdir"]["path"])).mkdir(parents=True, exist_ok=True)
                steps[label] = {"mkdir": step["mkdir"]["path"]}
            elif "occupy" in step:
                holder = socket.socket()
                holder.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
                holder.bind(("127.0.0.1", sandbox.port))
                holder.listen(1)
                held.append(holder)
                steps[label] = {"occupied": True}
            elif "stop" in step:
                sig = getattr(signal, step["stop"].get("signal", "SIGINT"))
                steps[label] = server.stop(sig) if server else {"exit": None}
                server = None
            elif "httpStub" in step:
                spec = step["httpStub"]
                stub = agent.HttpStub(spec.get("responses", []), spec.get("default", {"status": 200}))
                stubs[label] = stub
                sandbox.captured[spec.get("portVar", "STUB_PORT")] = str(stub.port)
                steps[label] = {"port": stub.port}
            elif "sleep" in step:
                time.sleep(step["sleep"].get("seconds", 0))
                steps[label] = {"slept": step["sleep"].get("seconds", 0)}
            else:
                raise ValueError(f"{scenario['id']}: unknown step {step}")
    finally:
        if server is not None:
            steps["_implicitStop"] = server.stop()
        for holder in held:
            holder.close()
        for stub in stubs.values():
            stub.close()
        for name, future in background.items():
            # A background call nobody joined is a scenario bug; record it.
            steps[f"_unjoined.{name}"] = future.result(timeout=320)
        pool.shutdown(wait=True)
    observation: dict[str, Any] = {"steps": steps}
    collect = scenario.get("collect", [])
    if "orphans" in collect:
        # A shim still running after its agent stopped is an orphan. Allow a
        # short grace for a released shim to finish exiting.
        deadline = time.monotonic() + 3
        while shims.live_shims(sandbox.root) and time.monotonic() < deadline:
            time.sleep(0.05)
        observation["orphans"] = shims.live_shims(sandbox.root)
        for orphan in observation["orphans"]:
            try:
                os.kill(orphan["pid"], signal.SIGKILL)
            except OSError:
                pass
    if "shimExits" in collect:
        # How cancelled children died: a catchable signal is recorded by the
        # shim in the trace; SIGKILL leaves only a stale pid file behind.
        observation["shimExits"] = {
            "signalled": sorted(
                f"{e['cmd']} {' '.join(e['argv'])} {e['signal']}"
                for e in shims.read_trace(sandbox.trace_path) if "signal" in e),
            "killed": sorted(e["cmd"] for e in shims.unclean_shims(sandbox.root)),
        }
    if "trace" in collect:
        observation["trace"] = sandbox.trace()
    if "files" in collect:
        observation["files"] = sandbox.files()
    if "sqlite" in collect:
        observation["sqlite"] = sandbox.sqlite_schemas()
    if "sqliteRows" in collect:
        observation["sqliteRows"] = sandbox.sqlite_rows()
    if "serverLog" in collect:
        log = sandbox.root / "server.log"
        observation["serverLog"] = agent._strip_log_prefix(log.read_text(errors="replace")) if log.exists() else None
    return observation, sandbox.variables


_EXCLUSIVE = threading.Lock()


X2_TABLES = ("operations", "plan_runs", "capability_invocations")


def invariant_violations(scenario: dict, observation: dict, label: str = "") -> list[dict]:
    """Cross-cutting invariants a scenario declares (milestones.md 2.6).

    They hold on each side on its own; equality with the other side is not
    enough. X2: rejected calls have no effects, so the shim trace is empty and
    the state store records no operations, plan runs or invocations.
    """
    violations = []
    # Equal harness failures are not parity. Disk exhaustion or a driver
    # exception on both sides must fail even when no scenario mask happens
    # to require a missing field.
    if observation.get("_executionError"):
        violations.append({"invariant": "EXECUTION", "reason": observation["_executionError"]})
    if "NO_ORPHANS" in scenario.get("invariants", []):
        if "orphans" not in observation:
            violations.append({"invariant": "NO_ORPHANS", "reason": "scenario does not collect orphans"})
        elif observation["orphans"]:
            violations.append({"invariant": "NO_ORPHANS", "reason": "shims outlived the agent",
                               "value": observation["orphans"]})
    if "X2" in scenario.get("invariants", []):
        # "x2Reads": argv prefixes a refusal may still run (resolving an
        # unknown target lists inventory). Anything else is an effect.
        # "argPrefix" further requires the next argument to start with it
        # (["query"] + "/1.0/" admits a GET query but not `query -X PUT`).
        def is_read(entry: dict) -> bool:
            argv = entry.get("argv", [])
            for read in scenario.get("x2Reads", []):
                prefix, arg = read["argvPrefix"], read.get("argPrefix")
                if entry.get("cmd") != read["cmd"] or argv[:len(prefix)] != prefix:
                    continue
                if arg is None or (len(argv) > len(prefix) and argv[len(prefix)].startswith(arg)):
                    return True
            return False

        effects = [e for e in observation.get("trace") or [] if not is_read(e)]
        if effects:
            violations.append({"invariant": "X2", "reason": "shim trace is not empty",
                               "value": effects[:5]})
        # "x2GoGaps": {table: decision} names a table the pinned Go reference
        # writes on a rejected call, recorded as a Go gap by an approved
        # decision. It exempts only the Go side; Rust is always held to X2.
        gaps = scenario.get("x2GoGaps", {}) if label.startswith("go") else {}
        databases = dict(observation.get("sqlite") or {})
        for db, dump in (observation.get("sqliteRows") or {}).items():
            databases[db] = {"rowCounts": {table: len(rows)
                                          for table, rows in dump.get("tables", {}).items()}}
        for db, schema in databases.items():
            for table in X2_TABLES:
                count = (schema.get("rowCounts") or {}).get(table)
                if count and table not in gaps:
                    violations.append({"invariant": "X2", "reason": f"{db} {table} has {count} rows"})
    return violations


def _execute_safe(impl: agent.Impl, scenario: dict, side: str, run_id: str,
                  other: agent.Impl | None = None) -> tuple[dict, dict]:
    """Like execute(), but a crash on one side (a real bug, or a mutation
    this scenario was never designed to survive, e.g. a wrongly-admitted
    extra concurrent call jamming a gate built for an exact count) becomes a
    recorded difference instead of an unhandled exception that kills the
    whole suite. The other side's normal observation then makes canon.diff
    flag it, which is exactly the signal a comparison run must produce."""
    try:
        return execute(impl, scenario, side, run_id, other)
    except Exception as exc:
        return {"steps": {}, "_executionError": f"{type(exc).__name__}: {exc}"}, {}


_MISSING = object()


def _get_at(doc: Any, path: list) -> Any:
    """Read-only lookup mirroring canon._apply_at's own traversal rules."""
    cur = doc
    for seg in path:
        if isinstance(cur, dict) and seg in cur:
            cur = cur[seg]
        elif isinstance(cur, list) and isinstance(seg, int) and -len(cur) <= seg < len(cur):
            cur = cur[seg]
        else:
            return _MISSING
    return cur


def _without_at(doc: Any, path: list) -> Any:
    """Copy-on-write delete of `path`, leaving everything outside the path
    untouched (and unshared), matching canon._apply_at's own safety: `doc`
    may still be the caller's raw observation in branches a mask never
    visited, so a plain in-place `del` would corrupt that evidence."""
    if not path:
        return doc
    head, rest = path[0], path[1:]
    if isinstance(doc, dict) and head in doc:
        doc = dict(doc)
        if rest:
            doc[head] = _without_at(doc[head], rest)
        else:
            del doc[head]
        return doc
    if isinstance(doc, list) and isinstance(head, int) and -len(doc) <= head < len(doc):
        doc = list(doc)
        if rest:
            doc[head] = _without_at(doc[head], rest)
        else:
            del doc[head]
        return doc
    return doc


def _reconcile_optional_masks(norm_l: Any, norm_r: Any, masks: list[dict]) -> tuple[Any, Any]:
    """An `optional` mask means a field may legitimately be absent on
    either side (e.g. a plan admission-rejected on one side never reaches
    its node fields at all). `canon.apply_masks` already suppresses the
    "mask path not found" violation for that case, but it masks each side
    independently and can't see the other -- so a field present-and-masked
    on one side and genuinely absent on the other still reads as a diff.
    Reconcile that one specific shape (present vs. absent) by dropping the
    masked value from whichever side has it, for every mask that declared
    itself optional; an outright type mismatch still surfaces as a mask
    violation regardless, since that list is built before this runs."""
    for mask in masks:
        if not mask.get("optional"):
            continue
        path = list(mask["path"])
        if "*" in path or "@json" in path:
            continue
        l_present = _get_at(norm_l, path) is not _MISSING
        r_present = _get_at(norm_r, path) is not _MISSING
        if l_present and not r_present:
            norm_l = _without_at(norm_l, path)
        elif r_present and not l_present:
            norm_r = _without_at(norm_r, path)
    return norm_l, norm_r


def compare_once(left: agent.Impl, right: agent.Impl, scenario: dict, run_id: str) -> dict:
    spec = scenario.get("compare", {})
    if scenario.get("exclusive"):
        # Fixed-port sides run serially. execute() holds the cross-process
        # lease, including when a source inventory uses execute() directly.
        obs_l, vars_l = _execute_safe(left, scenario, "a", run_id, right)
        obs_r, vars_r = _execute_safe(right, scenario, "b", run_id, left)
    else:
        with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
            fut_l = pool.submit(_execute_safe, left, scenario, "a", run_id, right)
            fut_r = pool.submit(_execute_safe, right, scenario, "b", run_id, left)
            obs_l, vars_l = fut_l.result()
            obs_r, vars_r = fut_r.result()
    norm_l, viol_l = canon.normalize(obs_l, spec, vars_l)
    norm_r, viol_r = canon.normalize(obs_r, spec, vars_r)
    viol_l += invariant_violations(scenario, obs_l, left.label)
    viol_r += invariant_violations(scenario, obs_r, right.label)
    divergences: list[dict] = []
    if cross_implementation(left, right):
        norm_l, norm_r, divergences = canon.apply_divergences(
            norm_l, norm_r, spec.get("divergences", []), canon.load_divergences(DIVERGENCE_FILE))
    norm_l, norm_r = _reconcile_optional_masks(norm_l, norm_r, spec.get("masks", []))
    differences = canon.diff(norm_l, norm_r)
    return {
        "raw": {"a": {"observation": obs_l, "variables": vars_l},
                "b": {"observation": obs_r, "variables": vars_r}},
        "normalized": {"a": norm_l, "b": norm_r},
        "diff": differences,
        "maskViolations": {"a": viol_l, "b": viol_r},
        "divergences": divergences,
    }


def cross_implementation(left: agent.Impl, right: agent.Impl) -> bool:
    """Declared divergences apply only between Go and a Rust build."""
    return left.label.startswith("rust") != right.label.startswith("rust")


def stale_divergences(outcome: dict) -> int:
    return sum(1 for d in outcome.get("divergences", []) if d["stale"])


def _gz(data: Any) -> bytes:
    return gzip.compress(canon.canonical_json(data).encode(), mtime=0)


def run_suite(left: agent.Impl, right: agent.Impl, suite: str, out_dir: Path,
              scenarios: list[dict], repeat: int = 1, workers: int = 6,
              source_lock: dict | None = None) -> dict:
    """Run every scenario `repeat` times and write an evidence bundle."""
    out_dir.mkdir(parents=True, exist_ok=True)
    jobs = [(s, i) for i in range(repeat) for s in scenarios]
    results: dict[str, list[dict]] = {s["id"]: [] for s in scenarios}
    last: dict[str, dict] = {}
    first_failure: dict[str, dict] = {}
    started = time.time()
    started_harness_sha = harness_sha256()
    started_binary_shas = (left.sha256(), right.sha256())

    def job(item: tuple[dict, int]) -> tuple[str, int, dict]:
        scenario, i = item
        return scenario["id"], i, compare_once(left, right, scenario, f"{i:02d}")

    with concurrent.futures.ThreadPoolExecutor(max_workers=workers) as pool:
        for sid, i, outcome in pool.map(job, jobs):
            results[sid].append({"iteration": i, "diffCount": len(outcome["diff"]),
                                 "maskViolations": sum(len(v) for v in outcome["maskViolations"].values()),
                                 "staleDivergences": stale_divergences(outcome)})
            if sid not in last or i >= last[sid]["iteration"]:
                last[sid] = {"iteration": i, **outcome}
            failed = (outcome["diff"] or any(outcome["maskViolations"].values())
                      or stale_divergences(outcome))
            if failed and (sid not in first_failure or i < first_failure[sid]["iteration"]):
                first_failure[sid] = {"iteration": i, **outcome}

    summary_items = {}
    for scenario in scenarios:
        sid = scenario["id"]
        outcome = last[sid]
        sdir = out_dir / "scenarios" / sid
        sdir.mkdir(parents=True, exist_ok=True)
        iterations = sorted(results[sid], key=lambda r: r["iteration"])
        files = {
            "a.json.gz": _gz(outcome["raw"]["a"]),
            "b.json.gz": _gz(outcome["raw"]["b"]),
            "diff.json": canon.canonical_json({
                "diff": outcome["diff"], "maskViolations": outcome["maskViolations"],
                "divergences": outcome["divergences"]}).encode(),
            "iterations.json": canon.canonical_json(iterations).encode(),
        }
        # Keep the first failing iteration too: a flake that the last
        # iteration does not show must still be diagnosable from evidence.
        failure = first_failure.get(sid)
        if failure is not None:
            files["first-failure.json.gz"] = _gz({
                "iteration": failure["iteration"], "diff": failure["diff"],
                "maskViolations": failure["maskViolations"], "divergences": failure["divergences"],
                "raw": failure["raw"]})
        hashes = {}
        for name, data in files.items():
            (sdir / name).write_bytes(data)
            hashes[name] = sha256_bytes(data)
        clean = all(r["diffCount"] == 0 and r["maskViolations"] == 0 and not r["staleDivergences"]
                    for r in iterations)
        summary_items[sid] = {
            "status": "pass" if clean else "fail",
            "iterations": len(iterations),
            "failingIterations": [r["iteration"] for r in iterations
                                  if r["diffCount"] or r["maskViolations"] or r["staleDivergences"]],
            "divergences": sorted({d["id"] for d in outcome["divergences"]}),
            "scenarioFile": scenario["_file"],
            "scenarioSha256": scenario["_sha256"],
            "files": hashes,
        }

    summary = {
        "schemaVersion": 1,
        "suite": suite,
        "provenance": {
            "left": {"label": left.label, "binarySha256": started_binary_shas[0]},
            "right": {"label": right.label, "binarySha256": started_binary_shas[1]},
            "harnessSha256": started_harness_sha,
            "changedDuringRun": (started_harness_sha != harness_sha256()
                                 or started_binary_shas != (left.sha256(), right.sha256())),
            "sourceCommit": (source_lock or {}).get("sourceCommit"),
            "sourceTree": (source_lock or {}).get("sourceTree"),
            "repeat": repeat,
            "startedAt": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(started)),
            "durationSeconds": round(time.time() - started, 1),
        },
        "items": summary_items,
    }
    (out_dir / "summary.json").write_text(canon.canonical_json(summary) + "\n")
    return summary
