"""Live evidence for unmarked fields and unknown capability admission.

D13 (`redact-unmarked-projections`, owner decision 2026-10-04): the pinned
Go reference persists unmarked/open-schema content verbatim; Rust now
diverges and redacts it before it reaches a durable sink. Go is unchanged
and its verbatim persistence is the expected, declared-divergent baseline
here -- it is not re-litigated by this driver. Rust persisting the canary
anywhere is the one remaining failure this driver can report.
"""
from __future__ import annotations

import json
import shutil
from pathlib import Path

from . import agent, cross_read, runner, shims

CANARY = "M5_UNMARKED_FIELD_CANARY_7e35d8bc"
CASES = ("input-unmarked", "task-unmarked", "result-unmarked", "unknown-capability")


def run_one(impl: agent.Impl, case: str) -> dict:
    sandbox = agent.Sandbox(impl.label, "unknown-" + case, None)
    server = None
    try:
        if case == "result-unmarked":
            # A declared object result whose additionalProperties schema is
            # true has no writeOnly marking for state.incusStatus. This exact
            # handler field carries the shim's value into the typed result.
            fixture = sandbox.root / "unknown-result-fixture.json"
            fixture.write_text(json.dumps({"commands": {"incus": [
                {"argv": ["list", "--format", "json"], "stdout": json.dumps([{
                    "name": "probe", "type": "virtual-machine", "status": CANARY,
                    "config": {"user.opute.host_agent_instance": "standalone"}}])}
            ]}}))
            sandbox.shim_names = shims.install(sandbox.root / "shims", fixture, sandbox.trace_path)
        server = agent.Server(impl, sandbox, ["serve", "--mode=standalone"], runner.PROFILES["standalone"])
        cross_read._ready(server)
        name, arguments = {
            "input-unmarked": ("get_host_info", {"unmarked": CANARY}),
            "task-unmarked": ("request_task_input", {"prompt": "Continue?", "responseType": "boolean", "unmarked": CANARY}),
            "result-unmarked": ("get_vm_info", {"uri": "vm:local:probe", "fast": True}),
            "unknown-capability": ("m5_unknown_capability", {"unmarked": CANARY}),
        }[case]
        response = agent.mcp_call(sandbox, "tools/call", {"name": name, "arguments": arguments}, name=name)
        stop = server.stop()
        if stop != {"exit": 0, "killed": False}:
            raise RuntimeError(f"projection server failed graceful stop: {stop}")
        tables = sandbox.sqlite_rows()["state/state.db"]["tables"]
        occurrences = []
        for table, rows in tables.items():
            for key, row in rows.items():
                for column, value in row.items():
                    if isinstance(value, str) and CANARY in value:
                        occurrences.append({"table": table, "key": key, "column": column})
        return {"case": case, "response": response, "stateRows": tables, "verbatimFields": occurrences}
    except Exception as exc:
        return {"error": f"{type(exc).__name__}: {exc}"}
    finally:
        if server is not None:
            server.stop()
        shutil.rmtree(sandbox.root, ignore_errors=True)


def failures(outcomes: dict) -> list[str]:
    problems = []
    expected = {f"{label}.{case}" for label in ["go", "rust"] for case in CASES}
    if set(outcomes) != expected:
        problems.append("unknown projection matrix incomplete or unexpected")
    for key, observation in outcomes.items():
        try:
            label, case = key.split(".", 1)
            if observation["case"] != case:
                raise ValueError("wrong projection case")
            tables = observation["stateRows"]
            # Recompute the sink sweep from raw rows, never trust a green
            # recorded occurrence list.
            raw = json.dumps(tables)
            canary_persisted = CANARY in raw
            if label == "rust" and canary_persisted:
                problems.append(f"{key}: unmarked canary persisted verbatim (D13 regression)")
            elif label == "go" and case != "unknown-capability" and not canary_persisted:
                # D13 declares Go's verbatim persistence as the known,
                # unchanged baseline this driver exercises. If Go ever stops
                # persisting it, the divergence is stale and must be
                # re-reviewed, not silently treated as an improved match.
                problems.append(f"{key}: expected pinned-Go verbatim persistence (D13) went missing")
            response = observation["response"]["body"]
            if not isinstance(response, dict) or not ({"result", "error"} & set(response)):
                problems.append(f"{key}: missing protocol result or refusal")
                continue
            if case == "unknown-capability":
                if "error" not in response or any(tables[table] for table in ["operations", "plan_runs", "capability_invocations"]):
                    problems.append(f"{key}: unknown capability did not fail closed")
            elif response.get("result", {}).get("isError") is True or "error" in response:
                # Rejection is allowed by the M5 unknown-projection requirement,
                # but source parity still needs its own owner-approved decision.
                pass
            elif case == "result-unmarked":
                if CANARY not in json.dumps(response):
                    problems.append(f"{key}: injected result field was not exercised")
        except (KeyError, TypeError, ValueError) as exc:
            problems.append(f"{key}: malformed projection observation: {exc}")
    return problems


def run(go: agent.Impl, rust: agent.Impl, out: Path) -> dict:
    out.mkdir(parents=True, exist_ok=True)
    provenance = {"sourceLock": json.loads((runner.REPO_ROOT / "baseline/source-lock.json").read_text()),
                  "goBinarySha256": go.sha256(), "rustBinarySha256": rust.sha256(),
                  "harnessSha256": runner.harness_sha256(), "driverSha256": runner.sha256_file(Path(__file__))}
    outcomes = {f"{impl.label}.{case}": run_one(impl, case) for impl in [go, rust] for case in CASES}
    issues = failures(outcomes)
    (out / "outcomes.json").write_text(json.dumps(outcomes, indent=2, sort_keys=True))
    provenance["changedDuringRun"] = (go.sha256() != provenance["goBinarySha256"]
                                     or rust.sha256() != provenance["rustBinarySha256"]
                                     or runner.harness_sha256() != provenance["harnessSha256"]
                                     or runner.sha256_file(Path(__file__)) != provenance["driverSha256"])
    summary = {"cases": list(CASES), "failures": issues,
               "files": {"outcomes.json": runner.sha256_file(out / "outcomes.json")}, "provenance": provenance}
    (out / "summary.json").write_text(json.dumps(summary, indent=2, sort_keys=True))
    return summary
