"""Deterministic SIGKILL inside actual operation/task/plan SQLite statements.

A test-only trigger spills uncommitted WAL pages and then holds the writer.
Observing that spill proves the chosen BEFORE/AFTER checkpoint was reached;
it is stronger than sleeping near a write or killing an admission refusal.
Normal agent binaries create the task and serve the recovered MCP status.
"""
from __future__ import annotations

import concurrent.futures
import datetime
import json
import os
import random
import select
import shutil
import signal
import sqlite3
import subprocess
import time
from pathlib import Path

from . import agent, canon, cross_read, runner, state_process

POINTS = {
    "operation-create": ("operation-create", "INSERT", "operations", "NEW.operation_id"),
    "operation-complete": ("operation-complete", "UPDATE", "operations", "NEW.operation_id"),
    "operation-fail": ("operation-fail", "UPDATE", "operations", "NEW.operation_id"),
    "operation-cancel": ("operation-cancel", "UPDATE", "operations", "NEW.operation_id"),
    "task-snapshot": ("task-snapshot", "UPDATE", "operations", "NEW.operation_id"),
    "plan-update": ("plan-update", "UPDATE", "plan_runs", "NEW.run_id"),
    "plan-complete-first": ("plan-complete", "UPDATE", "plan_runs", "NEW.run_id"),
    "plan-complete-last": ("plan-complete", "UPDATE", "active_capabilities", "NEW.run_id"),
}
CASES = tuple(f"{point}.{when}" for point in POINTS for when in ("BEFORE", "AFTER"))
SPILL_BYTES = 4 * 1024 * 1024


def _setup(proc: subprocess.Popen, timeout: float = 30) -> list[str]:
    lines = []
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if select.select([proc.stdout], [], [], max(0, deadline - time.monotonic()))[0]:
            line = proc.stdout.readline().decode(errors="replace")
            if not line:
                raise RuntimeError(f"fixture exited before setup: {proc.poll()}, {lines}")
            lines.append(line.rstrip())
            if line.rstrip().endswith("PARITY_SETUP"):
                return lines
    raise RuntimeError("fixture setup timed out")


def _install(db: Path, case: str, task: str) -> None:
    point, when = case.split(".")
    _, event, table, field = POINTS[point]
    identity = task if table == "operations" else "new"
    with sqlite3.connect(db) as conn:
        conn.execute("CREATE TABLE parity_crash_checkpoint (pad BLOB)")
        # Large enough to spill beyond both engines' default page caches.
        # The recursive SELECT prevents the outer statement from committing
        # while the parent sees the actual uncommitted WAL spill and kills it.
        conn.execute(f"""CREATE TRIGGER parity_crash_hold {when} {event} ON {table}
            WHEN {field} = '{identity}' BEGIN
            INSERT INTO parity_crash_checkpoint VALUES (zeroblob(33554432));
            SELECT sum(x) FROM (WITH RECURSIVE hold(x) AS
                (VALUES(1) UNION ALL SELECT x+1 FROM hold WHERE x<1000000000) SELECT x FROM hold);
            END""")


def _state(sandbox: agent.Sandbox) -> dict:
    return sandbox.sqlite_rows()["state/state.db"]


def timestamp_valid(value: object) -> bool:
    if not isinstance(value, str):
        return False
    try:
        if canon.MASK_TYPES["rfc3339"](value):
            datetime.datetime.fromisoformat(value.replace("Z", "+00:00"))
        else:
            if len(value) != 19:
                return False
            datetime.datetime.strptime(value, "%Y-%m-%d %H:%M:%S")
        return True
    except ValueError:
        return False


def recovery_failures(observation: dict, case: str, seed: int) -> list[str]:
    problems = []
    try:
        if observation["case"] != case or observation["seed"] != seed:
            problems.append("wrong crash checkpoint or seed")
        checkpoint = observation["checkpoint"]
        expected_delay = random.Random(seed).uniform(0.001, 0.02)
        if checkpoint["walBytes"] < SPILL_BYTES or checkpoint["delaySeconds"] != expected_delay:
            problems.append("uncommitted checkpoint not observed")
        if checkpoint["exit"] != -signal.SIGKILL or "PARITY_COMMITTED" in checkpoint["log"]:
            problems.append("writer was not killed inside the held write")
        if observation["integrity"] != ["ok"]:
            problems.append("SQLite integrity check failed")
        if observation["before"] != observation["afterKill"]:
            problems.append("killed transaction changed committed state")
        if observation["afterKill"]["tables"].get("parity_crash_checkpoint") != []:
            problems.append("uncommitted fixture marker survived")
        task = observation["taskId"]
        tables = observation["recovered"]["rows"]["tables"]
        if tables["operations"][task]["status"] != "unknown":
            problems.append("interrupted operation invented an outcome")
        if json.loads(tables["operations"][task]["task_snapshot_json"])["status"] != "input_required":
            problems.append("uncommitted task snapshot survived")
        if tables["plan_runs"]["new"]["status"] != "unknown":
            problems.append("interrupted plan invented an outcome")
        if tables["plan_runs"]["old"]["status"] != "completed":
            problems.append("previous completed plan lost")
        if tables["active_capabilities"]["fixture"]["run_id"] != "old":
            problems.append("torn active selection or invented plan success")
        if json.loads(tables["plan_runs"]["new"]["state_json"]) != {"checkpoint": "original"}:
            problems.append("uncommitted plan state survived")
        if observation["recovered"]["task"]["body"]["result"]["status"] != "input_required":
            problems.append("MCP task status differs from committed snapshot")
        if set(tables) != {"operations", "plan_runs", "active_capabilities", "active_runtimes",
                           "capability_invocations", "provider_generations", "resource_registry"}:
            problems.append("recovered state table set changed")
        for table, rows in tables.items():
            for row in rows.values():
                for column in ["created_at", "updated_at", "activated_at"]:
                    if column in row and not timestamp_valid(row[column]):
                        problems.append(f"invalid {table}.{column} timestamp")
    except (KeyError, TypeError, ValueError, AttributeError) as exc:
        problems.append(f"malformed crash observation: {exc}")
    return problems


def run_one(impl: agent.Impl, fixture: dict, case: str, seed: int) -> dict:
    sandbox = None
    proc = server = None
    try:
        written, sandbox = cross_read._write(impl, f"storage-crash-{seed}", "input-required")
        task = written["taskId"]
        point = case.split(".")[0]
        env = {**os.environ, "PARITY_STATE_DIR": str(sandbox.root / "state"),
               "PARITY_TASK_ID": task, "PARITY_STATE_ACTION": POINTS[point][0]}
        proc = subprocess.Popen(fixture["command"], env=env, cwd=sandbox.root,
                                stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                bufsize=0)
        log = _setup(proc)
        db = sandbox.root / "state/state.db"
        _install(db, case, task)
        before = _state(sandbox)
        proc.stdin.write(b"GO\n")
        proc.stdin.flush()
        wal = Path(str(db) + "-wal")
        deadline = time.monotonic() + 30
        while (not wal.exists() or wal.stat().st_size < SPILL_BYTES) and time.monotonic() < deadline:
            if proc.poll() is not None:
                raise RuntimeError(f"writer exited before crash checkpoint: {proc.returncode}")
            time.sleep(0.001)
        size = wal.stat().st_size if wal.exists() else 0
        if size < SPILL_BYTES:
            raise RuntimeError("selected trigger did not spill uncommitted WAL pages")
        delay = random.Random(seed).uniform(0.001, 0.02)
        time.sleep(delay)
        proc.send_signal(signal.SIGKILL)
        proc.wait(timeout=10)
        log.extend(proc.stdout.read().decode(errors="replace").splitlines())
        checkpoint = {"walBytes": size, "delaySeconds": delay, "exit": proc.returncode, "log": "\n".join(log)}
        after = _state(sandbox)
        with sqlite3.connect(db) as conn:
            integrity = [row[0] for row in conn.execute("PRAGMA integrity_check")]
            conn.execute("DROP TRIGGER parity_crash_hold")
            conn.execute("DROP TABLE parity_crash_checkpoint")
        server = agent.Server(impl, sandbox, ["serve", "--mode=standalone"], runner.PROFILES["standalone"])
        cross_read._ready(server)
        get_task = cross_read._task(sandbox, task)
        rows = _state(sandbox)
        schema = sandbox.sqlite_schemas()["state/state.db"]
        stop = server.stop()
        if stop != {"exit": 0, "killed": False}:
            raise RuntimeError(f"recovery server did not stop cleanly: {stop}")
        return {"case": case, "seed": seed, "taskId": task, "checkpoint": checkpoint,
                "integrity": integrity, "before": before, "afterKill": after,
                "recovered": {"task": get_task, "rows": rows, "schema": schema},
                "variables": {**sandbox.variables, "TASK_ID": task}}
    except Exception as exc:
        return {"error": f"{type(exc).__name__}: {exc}"}
    finally:
        if proc is not None:
            if proc.poll() is None:
                proc.kill()
                proc.wait(timeout=10)
            for stream in [proc.stdin, proc.stdout]:
                if stream:
                    stream.close()
        if server is not None:
            server.stop()
        if sandbox is not None:
            shutil.rmtree(sandbox.root, ignore_errors=True)


COMPARE_SPEC = {
    "parseJson": [["rows", "tables", "operations", "*", "task_snapshot_json"]],
    "masks": [
        {"path": ["rows", "tables", table, "*", column], "type": "string",
         "reason": "validated RFC3339 or SQLite startup datetime; wall-clock value differs"}
        for table, columns in {"operations": ["created_at", "updated_at"],
                               "plan_runs": ["created_at", "updated_at"],
                               "active_capabilities": ["activated_at"]}.items() for column in columns
    ] + [
        {"path": ["task", "body", "result", column], "type": "rfc3339", "reason": "original task wall-clock time"}
        for column in ["createdAt", "lastUpdatedAt"]
    ] + [
        {"path": ["rows", "tables", "operations", "*", "task_snapshot_json", "$json", column],
         "type": "rfc3339", "reason": "original snapshot wall-clock time"}
        for column in ["createdAt", "lastUpdatedAt"]
    ],
}


def evaluate(raw: dict, case: str, seed: int) -> dict:
    failures = []
    normalized = []
    for label in ["go", "rust"]:
        observation = raw[label]
        problems = recovery_failures(observation, case, seed)
        failures.extend(f"{label}: {problem}" for problem in problems)
        if problems:
            continue
        norm, violations = canon.normalize(observation["recovered"], COMPARE_SPEC, observation["variables"])
        failures.extend(f"{label}: invalid mask {v}" for v in violations)
        normalized.append(norm)
    return {"failures": failures, "diff": canon.diff(*normalized) if len(normalized) == 2 else []}


def run(go: agent.Impl, rust: agent.Impl, seeds: int, out: Path, workers: int = 4) -> dict:
    out.mkdir(parents=True, exist_ok=True)
    fixtures = state_process.build()
    provenance = {"sourceLock": json.loads((runner.REPO_ROOT / "baseline/source-lock.json").read_text()),
                  "goBinarySha256": go.sha256(), "rustBinarySha256": rust.sha256(),
                  "harnessSha256": runner.harness_sha256(), "driverSha256": runner.sha256_file(Path(__file__)),
                  "fixtures": fixtures}

    def job(seed: int) -> tuple[str, dict]:
        case = CASES[seed % len(CASES)]
        raw = {impl.label: run_one(impl, fixtures[impl.label], case, seed) for impl in [go, rust]}
        return str(seed), {"raw": raw, **evaluate(raw, case, seed)}

    with concurrent.futures.ThreadPoolExecutor(max_workers=workers) as pool:
        outcomes = dict(pool.map(job, range(seeds)))
    failed = [key for key, value in outcomes.items() if value["failures"] or value["diff"]]
    (out / "outcomes.json.gz").write_bytes(runner._gz(outcomes))
    provenance["changedDuringRun"] = (go.sha256() != provenance["goBinarySha256"]
                                     or rust.sha256() != provenance["rustBinarySha256"]
                                     or runner.harness_sha256() != provenance["harnessSha256"]
                                     or runner.sha256_file(Path(__file__)) != provenance["driverSha256"])
    summary = {"seeds": seeds, "cases": list(CASES), "passed": seeds - len(failed), "failed": len(failed),
               "failedSeeds": failed, "provenance": provenance,
               "files": {"outcomes.json.gz": runner.sha256_file(out / "outcomes.json.gz")}}
    (out / "summary.json").write_text(json.dumps(summary, indent=2, sort_keys=True))
    return summary
