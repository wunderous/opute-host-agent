"""M5 cross-read of admitted task state, using the writer's own restart as
an oracle. Each reader opens a separate copy after the writer has stopped.
The live writer is not an oracle for restart: Go can persist a stale working
snapshot when tasks/update resumes successfully.
"""
from __future__ import annotations

import json
import shutil
from pathlib import Path
from typing import Any

from . import agent, canon, runner

CASES = {"input-required": ("input_required", "input_required"),
         "cancelled": ("cancelled", "cancelled"),
         "resumed": ("completed", "failed")}


def _env() -> dict[str, str]:
    return dict(runner.PROFILES["standalone"])


def _ready(server: agent.Server) -> None:
    ready = server.wait_ready(30)
    if not ready.get("ready"):
        raise RuntimeError(f"server did not become ready: {ready}")


def _task(sandbox: agent.Sandbox, task_id: str) -> dict:
    return agent.mcp_call(sandbox, "tasks/get", {"taskId": task_id}, name=task_id)


def _write(impl: agent.Impl, run_id: str, case: str) -> tuple[dict, agent.Sandbox]:
    sandbox = agent.Sandbox(impl.label, run_id, None)
    server = None
    try:
        server = agent.Server(impl, sandbox, ["serve", "--mode=standalone"], _env())
        _ready(server)
        call = agent.mcp_call(sandbox, "tools/call", {
            "name": "request_task_input", "arguments": {"prompt": "Continue?", "responseType": "boolean"}},
            name="request_task_input")
        task_id = call.get("body", {}).get("result", {}).get("taskId")
        if not task_id:
            raise RuntimeError(f"accepted task returned no handle: {call}")
        if case == "cancelled":
            agent.mcp_call(sandbox, "tasks/cancel", {"taskId": task_id}, name=task_id)
        elif case == "resumed":
            agent.mcp_call(sandbox, "tasks/update", {"taskId": task_id, "inputResponses": {"response": True}}, name=task_id)
        got = _task(sandbox, task_id)
        actual = got.get("body", {}).get("result", {}).get("status")
        if actual != CASES[case][0]:
            raise RuntimeError(f"{case}: writer status {actual!r}, expected {CASES[case][0]!r}")
        stop = server.stop()
        if stop != {"exit": 0, "killed": False}:
            raise RuntimeError(f"writer failed graceful stop: {stop}")
        rows = sandbox.sqlite_rows()["state/state.db"]["tables"]["operations"]
        if task_id not in rows or not rows[task_id]["task_snapshot_json"]:
            raise RuntimeError("accepted task snapshot was not persisted")
        return {"taskId": task_id, "live": got, "persisted": rows}, sandbox
    except Exception:
        if server is not None:
            server.stop()
        shutil.rmtree(sandbox.root, ignore_errors=True)
        raise


def _read(impl: agent.Impl, source: agent.Sandbox, task_id: str, run_id: str, case: str) -> dict:
    sandbox = agent.Sandbox(impl.label, run_id, None)
    server = None
    try:
        shutil.rmtree(sandbox.root / "state")
        shutil.copytree(source.root / "state", sandbox.root / "state")
        server = agent.Server(impl, sandbox, ["serve", "--mode=standalone"], _env())
        _ready(server)
        got = _task(sandbox, task_id)
        if got.get("body", {}).get("result", {}).get("status") != CASES[case][1]:
            raise RuntimeError(f"{case}: wrong restored status: {got}")
        listed = agent.mcp_call(sandbox, "tasks/list", {}, name="")
        if listed.get("body", {}).get("error", {}).get("code") != -32601:
            raise RuntimeError(f"pinned unsupported list boundary changed: {listed}")
        rows = sandbox.sqlite_rows()["state/state.db"]
        stop = server.stop()
        if stop != {"exit": 0, "killed": False}:
            raise RuntimeError(f"reader failed graceful stop: {stop}")
        return {"taskId": task_id, "tasksGet": got, "tasksList": listed, "stateRows": rows,
                "variables": {**source.variables, "TASK_ID": task_id}}
    finally:
        if server is not None:
            server.stop()
        shutil.rmtree(sandbox.root, ignore_errors=True)


COMPARE_SPEC = {
    "masks": [{"path": ["stateRows", "tables", "operations", "*", "updated_at"],
               "type": "string", "reason": "Go startup migration writes datetime(now) on each copy"}],
}


def run_direction(writer: agent.Impl, reader: agent.Impl, run_id: str, case: str = "cancelled") -> dict[str, Any]:
    source = None
    try:
        written, source = _write(writer, f"{run_id}-writer", case)
        task_id = written["taskId"]
        reference = _read(writer, source, task_id, f"{run_id}-reference", case)
        read_back = _read(reader, source, task_id, f"{run_id}-reader", case)
        norm_ref, viol_ref = canon.normalize(reference, COMPARE_SPEC, reference["variables"])
        norm_read, viol_read = canon.normalize(read_back, COMPARE_SPEC, read_back["variables"])
        return {"diff": canon.diff(norm_ref, norm_read),
                "maskViolations": {"reference": viol_ref, "reader": viol_read},
                "raw": {"written": written, "reference": reference, "readBack": read_back}}
    except Exception as exc:
        return {"error": f"{type(exc).__name__}: {exc}"}
    finally:
        if source is not None:
            shutil.rmtree(source.root, ignore_errors=True)


def run(go: agent.Impl, rust: agent.Impl, out: Path) -> dict[str, Any]:
    out.mkdir(parents=True, exist_ok=True)
    provenance = {"sourceLock": json.loads((runner.REPO_ROOT / "baseline/source-lock.json").read_text()),
                  "goBinarySha256": go.sha256(), "rustBinarySha256": rust.sha256(),
                  "driverSha256": runner.sha256_file(Path(__file__)), "harnessSha256": runner.harness_sha256()}
    outcomes = {}
    for repeat in range(3):
        for case in CASES:
            for writer, reader in [(go, rust), (rust, go)]:
                key = f"{writer.label}-writes-{reader.label}-reads.{case}.{repeat}"
                outcomes[key] = run_direction(writer, reader, key, case)
    failed = [key for key, result in outcomes.items()
              if result.get("error") or result.get("diff") or any(result.get("maskViolations", {}).values())]
    provenance["changedDuringRun"] = (go.sha256() != provenance["goBinarySha256"]
        or rust.sha256() != provenance["rustBinarySha256"]
        or runner.harness_sha256() != provenance["harnessSha256"]
        or runner.sha256_file(Path(__file__)) != provenance["driverSha256"])
    summary = {"directions": list(outcomes), "passed": len(outcomes)-len(failed),
               "failed": len(failed), "failedDirections": failed,
               "provenance": provenance}
    (out / "directions.json").write_text(json.dumps(outcomes, indent=2, sort_keys=True))
    summary["files"] = {"directions.json": runner.sha256_file(out / "directions.json")}
    (out / "summary.json").write_text(json.dumps(summary, indent=2, sort_keys=True))
    return summary
