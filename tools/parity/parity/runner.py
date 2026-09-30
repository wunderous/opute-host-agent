"""Scenario execution, twin comparison, and evidence bundles.

A scenario is a versioned JSON document under tools/parity/scenarios/. It runs
unchanged against both sides, each in its own Sandbox. Observations are
normalized (canon.normalize) and diffed. The raw observations, the diff and a
summary are written to an evidence bundle whose file hashes the verifier
re-checks.
"""

from __future__ import annotations

import concurrent.futures
import gzip
import hashlib
import json
import signal
import socket
import threading
import time
from pathlib import Path
from typing import Any

from . import agent, canon

TOOLS_DIR = Path(__file__).resolve().parent.parent
REPO_ROOT = TOOLS_DIR.parent.parent
SCENARIO_DIR = TOOLS_DIR / "scenarios"
FIXTURE_DIR = TOOLS_DIR / "fixtures"
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
    files = [TOOLS_DIR / "parity" / f"{m}.py" for m in OBSERVING_MODULES] + sorted(FIXTURE_DIR.rglob("*.json"))
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


def _env(step: dict) -> dict:
    env = dict(PROFILES[step.get("profile", "empty")])
    env.update(step.get("env", {}))
    return env


def execute(impl: agent.Impl, scenario: dict, side: str, run_id: str) -> tuple[dict, dict]:
    """Run one scenario on one side. Returns (observation, variables)."""
    fixture = scenario.get("fixture")
    fixture_path = FIXTURE_DIR / "shims" / f"{fixture}.json" if fixture else None
    sandbox = agent.Sandbox(side=side, run_id=run_id, fixture=fixture_path)
    steps: dict[str, Any] = {}
    server: agent.Server | None = None
    held: list[socket.socket] = []
    try:
        for index, step in enumerate(scenario["steps"]):
            label = step.get("as", f"step{index}")
            if "cli" in step:
                spec = step["cli"]
                steps[label] = agent.run_cli(
                    impl, sandbox, spec["argv"], _env(spec),
                    timeout=spec.get("timeout", 30), probe_port=spec.get("probePort", False))
            elif "serve" in step:
                spec = step["serve"]
                server = agent.Server(impl, sandbox, spec.get("argv", ["serve"]), _env(spec))
                steps[label] = server.wait_ready(spec.get("timeout", 30), spec.get("readyPort"))
            elif "http" in step:
                spec = step["http"]
                body = spec.get("body")
                raw = json.dumps(body).encode() if isinstance(body, (dict, list)) else (
                    body.encode() if isinstance(body, str) else None)
                headers = {k: sandbox.expand(v) for k, v in spec.get("headers", {}).items()}
                steps[label] = agent.http_request(sandbox, spec.get("method", "GET"), spec["path"], headers, raw)
            elif "mcp" in step:
                spec = step["mcp"]
                steps[label] = agent.mcp_call(
                    sandbox, spec["method"], spec.get("params"),
                    token=spec.get("token", "${TOKEN}"), name=spec.get("name"),
                    modern=spec.get("modern", True), headers=spec.get("headers"),
                    omit_headers=spec.get("omitHeaders"), request_id=spec.get("id", 1))
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
            else:
                raise ValueError(f"{scenario['id']}: unknown step {step}")
    finally:
        if server is not None:
            steps["_implicitStop"] = server.stop()
        for holder in held:
            holder.close()
    observation: dict[str, Any] = {"steps": steps}
    collect = scenario.get("collect", [])
    if "trace" in collect:
        observation["trace"] = sandbox.trace()
    if "files" in collect:
        observation["files"] = sandbox.files()
    if "sqlite" in collect:
        observation["sqlite"] = sandbox.sqlite_schemas()
    if "serverLog" in collect:
        log = sandbox.root / "server.log"
        observation["serverLog"] = agent._strip_log_prefix(log.read_text(errors="replace")) if log.exists() else None
    return observation, sandbox.variables


_EXCLUSIVE = threading.Lock()


def compare_once(left: agent.Impl, right: agent.Impl, scenario: dict, run_id: str) -> dict:
    spec = scenario.get("compare", {})
    if scenario.get("exclusive"):
        # Scenarios that use fixed default ports run one side at a time and
        # never overlap another exclusive scenario.
        with _EXCLUSIVE:
            obs_l, vars_l = execute(left, scenario, "a", run_id)
            obs_r, vars_r = execute(right, scenario, "b", run_id)
    else:
        with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
            fut_l = pool.submit(execute, left, scenario, "a", run_id)
            fut_r = pool.submit(execute, right, scenario, "b", run_id)
            obs_l, vars_l = fut_l.result()
            obs_r, vars_r = fut_r.result()
    norm_l, viol_l = canon.normalize(obs_l, spec, vars_l)
    norm_r, viol_r = canon.normalize(obs_r, spec, vars_r)
    differences = canon.diff(norm_l, norm_r)
    return {
        "raw": {"a": {"observation": obs_l, "variables": vars_l},
                "b": {"observation": obs_r, "variables": vars_r}},
        "normalized": {"a": norm_l, "b": norm_r},
        "diff": differences,
        "maskViolations": {"a": viol_l, "b": viol_r},
    }


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

    def job(item: tuple[dict, int]) -> tuple[str, int, dict]:
        scenario, i = item
        return scenario["id"], i, compare_once(left, right, scenario, f"{i:02d}")

    with concurrent.futures.ThreadPoolExecutor(max_workers=workers) as pool:
        for sid, i, outcome in pool.map(job, jobs):
            results[sid].append({"iteration": i, "diffCount": len(outcome["diff"]),
                                 "maskViolations": sum(len(v) for v in outcome["maskViolations"].values())})
            if sid not in last or i >= last[sid]["iteration"]:
                last[sid] = {"iteration": i, **outcome}
            failed = outcome["diff"] or any(outcome["maskViolations"].values())
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
                "diff": outcome["diff"], "maskViolations": outcome["maskViolations"]}).encode(),
            "iterations.json": canon.canonical_json(iterations).encode(),
        }
        # Keep the first failing iteration too: a flake that the last
        # iteration does not show must still be diagnosable from evidence.
        failure = first_failure.get(sid)
        if failure is not None:
            files["first-failure.json.gz"] = _gz({
                "iteration": failure["iteration"], "diff": failure["diff"],
                "maskViolations": failure["maskViolations"], "raw": failure["raw"]})
        hashes = {}
        for name, data in files.items():
            (sdir / name).write_bytes(data)
            hashes[name] = sha256_bytes(data)
        clean = all(r["diffCount"] == 0 and r["maskViolations"] == 0 for r in iterations)
        summary_items[sid] = {
            "status": "pass" if clean else "fail",
            "iterations": len(iterations),
            "failingIterations": [r["iteration"] for r in iterations
                                  if r["diffCount"] or r["maskViolations"]],
            "scenarioFile": scenario["_file"],
            "scenarioSha256": scenario["_sha256"],
            "files": hashes,
        }

    summary = {
        "schemaVersion": 1,
        "suite": suite,
        "provenance": {
            "left": {"label": left.label, "binarySha256": left.sha256()},
            "right": {"label": right.label, "binarySha256": right.sha256()},
            "harnessSha256": harness_sha256(),
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
