"""M5 crash injection: kill -9 while a task-aware write is in flight, restart
the same binary against the same state dir, and compare recovered state
against the other implementation.

This is the first injection point, not yet the full ~200-seed corpus
milestones.md calls for: it kills the agent while `start_vm` is blocked
mid-dispatch on the Incus shim gate, so the operation row's `Create` and
initial snapshot write have committed but `Complete`/`Cancel` have not.
Varying the injection point (which call, which lifecycle stage, randomized
timing within it) is the remaining scaling work.
"""

from __future__ import annotations

import json
import random
import shutil
import signal
import time
from pathlib import Path
from typing import Any

from . import agent, canon, runner, shims

FIXTURE = runner.FIXTURE_DIR / "shims" / "incus-gated-list.json"

# "mid-shim": kill while still blocked on the gate (Create + the initial
# snapshot committed, Complete/Cancel never reached).
# "release-race": release the gate (letting dispatch run to the binding
# failure and call Complete) and kill after a short, seed-derived delay,
# landing at a different, unpredictable point relative to Complete's write
# and the background goroutine/thread's own persist_task call -- this is
# the exact race the live shape-parity run on tasks.cancel.* surfaced
# (evidence/m5/README.md's undecided `operations.status` finding).
MODES = ("mid-shim", "release-race")


def _standalone_env() -> dict[str, str]:
    return {**runner.PROFILES["standalone"], "OPUTE_STANDALONE_ALLOW_MUTATIONS": "true"}


def run_one(impl: agent.Impl, run_id: str, mode: str, seed: int) -> dict[str, Any]:
    """One crash/restart cycle. Returns the recovered, masked-for-comparison
    state, plus the raw observation for debugging a failure."""
    if mode not in MODES:
        raise ValueError(f"unknown crash-injection mode {mode!r}")
    sandbox = agent.Sandbox(impl.label, run_id, FIXTURE)
    env = _standalone_env()
    try:
        server = agent.Server(impl, sandbox, ["serve", "--mode=standalone"], env)
        ready = server.wait_ready(30)
        if not ready.get("ready"):
            return {"error": "server did not become ready", "ready": ready,
                     "log": (sandbox.root / "server.log").read_text(errors="replace")}
        call = agent.mcp_call(
            sandbox, "tools/call",
            {"name": "start_vm", "arguments": {"uri": "vm:local:web1"}},
            name="start_vm",
        )
        task_id = call.get("body", {}).get("result", {}).get("taskId")
        if not task_id:
            return {"error": "start_vm did not return a task id", "call": call}
        blocked = shims.await_gate(sandbox.root, "list", timeout=10)
        if not blocked:
            return {"error": "shim never reached the gate"}
        if mode == "release-race":
            shims.release_gate(sandbox.root, "list")
            # The task's own dispatch finishes in microseconds (no real
            # Incus call for a binding-type mismatch); the server process
            # itself keeps running regardless. This seed-derived delay is
            # what actually varies which side of Complete's write the kill
            # lands on, run to run.
            time.sleep(random.Random(seed).uniform(0, 0.02))
        server.proc.send_signal(signal.SIGKILL)
        server.proc.wait(timeout=10)

        server2 = agent.Server(impl, sandbox, ["serve", "--mode=standalone"], env)
        ready2 = server2.wait_ready(30)
        if not ready2.get("ready"):
            return {"error": "server did not restart", "ready": ready2,
                     "log": (sandbox.root / "server.log").read_text(errors="replace")}
        get_result = agent.mcp_call(sandbox, "tasks/get", {"taskId": task_id}, name=task_id)
        rows = sandbox.sqlite_rows()
        server2.stop()
        return {
            "taskId": task_id,
            "tasksGetAfterRestart": get_result,
            "sqliteRows": rows,
            "variables": {**sandbox.variables, "TASK_ID": task_id},
        }
    finally:
        shutil.rmtree(sandbox.root, ignore_errors=True)


# No field here is specific to one run: the task id is substituted via the
# TASK_ID variable (both call sites register it), and every timestamp is a
# wall-clock value neither side controls.
COMPARE_SPEC = {
    "masks": [
        {"path": ["tasksGetAfterRestart", "body", "result", "createdAt"],
         "type": "rfc3339", "reason": "task wall-clock time"},
        {"path": ["tasksGetAfterRestart", "body", "result", "lastUpdatedAt"],
         "type": "rfc3339", "reason": "task wall-clock time"},
        {"path": ["sqliteRows", "state/state.db", "tables", "operations", "*", "created_at"],
         "type": "rfc3339", "reason": "operation row wall-clock time"},
        {"path": ["sqliteRows", "state/state.db", "tables", "operations", "*", "updated_at"],
         "type": "string", "reason": "operation row wall-clock time: the open-time "
          "migration writes a plain datetime('now') with no fractional seconds, "
          "unlike every other write's RFC3339Nano"},
        {"path": ["sqliteRows", "state/state.db", "tables", "operations", "*",
                  "task_snapshot_json", "$json", "createdAt"],
         "type": "rfc3339", "reason": "task snapshot wall-clock time", "optional": True},
        {"path": ["sqliteRows", "state/state.db", "tables", "operations", "*",
                  "task_snapshot_json", "$json", "lastUpdatedAt"],
         "type": "rfc3339", "reason": "task snapshot wall-clock time", "optional": True},
    ],
    "parseJson": [
        ["sqliteRows", "state/state.db", "tables", "operations", "*", "task_snapshot_json"],
    ],
    "divergences": ["D8.authz-rows"],
}


def compare_once(go: agent.Impl, rust: agent.Impl, run_id: str, mode: str, seed: int) -> dict[str, Any]:
    go_obs = run_one(go, f"{run_id}-go", mode, seed)
    rust_obs = run_one(rust, f"{run_id}-rust", mode, seed)
    for label, obs in (("go", go_obs), ("rust", rust_obs)):
        if "error" in obs:
            return {"error": f"{label}: {obs['error']}", "raw": {"go": go_obs, "rust": rust_obs}}
    norm_go, viol_go = canon.normalize(go_obs, COMPARE_SPEC, go_obs["variables"])
    norm_rust, viol_rust = canon.normalize(rust_obs, COMPARE_SPEC, rust_obs["variables"])
    divergences: list[dict] = []
    if COMPARE_SPEC.get("divergences"):
        norm_go, norm_rust, divergences = canon.apply_divergences(
            norm_go, norm_rust, COMPARE_SPEC["divergences"],
            canon.load_divergences(runner.DIVERGENCE_FILE))
    diff = canon.diff(norm_go, norm_rust)
    return {
        "diff": diff,
        "maskViolations": {"go": viol_go, "rust": viol_rust},
        "divergences": divergences,
        "raw": {"go": go_obs, "rust": rust_obs},
    }


def run(go: agent.Impl, rust: agent.Impl, seeds: int, out: Path, mode: str = "mid-shim") -> dict[str, Any]:
    """Repeats one injection point `seeds` times and writes a summary plus
    every failure (not just the first: a timing-dependent mode like
    "release-race" can fail at some seeds and not others, and seeing only
    one hides whether it's one bad seed or a systematic gap)."""
    out.mkdir(parents=True, exist_ok=True)
    results = []
    failures = []
    for seed in range(seeds):
        outcome = compare_once(go, rust, f"seed{seed:03d}", mode, seed)
        failed = bool(
            outcome.get("error")
            or outcome.get("diff")
            or any(outcome.get("maskViolations", {}).values())
        )
        results.append({"seed": seed, "failed": failed, "error": outcome.get("error")})
        if failed:
            failures.append({"seed": seed, "outcome": outcome})
    summary = {
        "mode": mode,
        "seeds": seeds,
        "passed": sum(1 for r in results if not r["failed"]),
        "failed": sum(1 for r in results if r["failed"]),
        "results": results,
    }
    (out / "summary.json").write_text(json.dumps(summary, indent=2, sort_keys=True))
    if failures:
        (out / "failures.json").write_text(
            json.dumps(failures, indent=2, sort_keys=True, default=str))
    return summary
