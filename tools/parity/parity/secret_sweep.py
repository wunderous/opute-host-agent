"""X3 catalog-wide canaries at actual projection/storage boundaries and MCP.

Fixture processes execute the real task, invocation and plan projections and
store methods, without domain effects. The normal binaries then restore those
records and expose supported task status/result responses through tasks/get.
Also capture the pinned refusals for unsupported list/result methods. Scan each state file, including
live WAL/SHM, fixture logs/output, and every recovered HTTP response.
"""
from __future__ import annotations

import base64
import json
import os
import select
import shutil
import subprocess
import time
import uuid
from pathlib import Path

from . import agent, canon, cross_read, evidence_process, runner

MODES = ("standalone", "platform")


def field_cases(catalog: dict, mode: str) -> list[dict]:
    cases = []

    def walk(schema: dict, path: list[str]) -> list[tuple[list[str], dict]]:
        found = [(path, schema)] if schema.get("writeOnly") is True else []
        for key, child in schema.get("properties", {}).items():
            if isinstance(child, dict):
                found.extend(walk(child, path + [key]))
        for key in ["items", "additionalProperties"]:
            if isinstance(schema.get(key), dict):
                found.extend(walk(schema[key], path + ["*" + key]))
        for key in ["allOf", "anyOf", "oneOf", "$defs", "definitions"]:
            children = schema.get(key, [])
            if isinstance(children, dict):
                children = children.values()
            for index, child in enumerate(children):
                if isinstance(child, dict):
                    found.extend(walk(child, path + [f"*{key}:{index}"]))
        return found

    for descriptor in catalog["tools"]:
        for direction in ["inputSchema", "outputSchema"]:
            schema = descriptor.get(direction) or {}
            paths = walk(schema, [])
            if paths and direction != "inputSchema":
                # The pinned catalog currently has no write-only output field.
                # A new output marking requires exercising that actual result
                # boundary, never reusing an input-only fixture accidentally.
                raise ValueError("write-only output schema requires a result-canary fixture")
            for path, child in paths:
                if len(path) != 1:
                    raise ValueError("nested catalog marking requires a nested field fixture")
                name = descriptor["name"]
                field = path[0]
                identifier = f"{mode}:{name}:{field}"
                canary = "M5_SECRET_CANARY_" + uuid.uuid4().hex + uuid.uuid4().hex
                value = {"probe": canary} if child.get("type") == "object" else canary
                args = {field: value, "visible": "safe"}
                action = {"tool": name, "args": args}
                document = {"contractVersion": "host-plan.v1", "planId": identifier, "generation": 1,
                            "idempotencyKey": identifier, "variables": {"inputs": {"recipeSecret": canary, "visible": "safe"}},
                            "nodes": [{"id": "n1", **{role: action for role in ["action", "validate", "compensate", "recover"]}}]}
                context = {"name": "secret", "value": canary, "schema": {"type": "string"},
                           "schemaRevision": "sha256:fixture", "producerNode": "n1", "source": "fixture",
                           "secret": True, "recordedAt": "2026-10-04T00:00:00Z"}
                state = {"runId": identifier, "planId": identifier, "generation": 1, "status": "completed",
                         "nodes": {"n1": {"id": "n1", "status": "completed", "attempts": 1, "output": {"marker": "safe"}}},
                         "outputs": {"n1": {"marker": "safe"}, "derived": {"opaque": canary}},
                         "context": {"secret": context}, "contextHistory": [context]}
                cases.append({"id": identifier, "tool": name, "path": path, "canary": canary,
                              "arguments": args, "document": document, "state": state})
    return cases


def _ready(proc: subprocess.Popen) -> list[str]:
    lines = []
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        if select.select([proc.stdout], [], [], max(0, deadline - time.monotonic()))[0]:
            line = proc.stdout.readline().decode(errors="replace")
            if not line:
                raise RuntimeError(f"projection process exited before sink scan: {lines}")
            lines.append(line.rstrip())
            if line.rstrip().endswith("PARITY_EVIDENCE_READY"):
                return lines
    raise RuntimeError("projection process did not reach sink scan")


def _scan(root: Path, cases: list[dict]) -> tuple[list[dict], list[dict]]:
    hits, scanned = [], []
    for path in sorted(root.rglob("*")):
        if not path.is_file():
            continue
        data = path.read_bytes()
        scanned.append({"path": str(path.relative_to(root)), "bytes": len(data), "sha256": runner.sha256_bytes(data),
                        "dataBase64": base64.b64encode(data).decode()})
        for case in cases:
            if case["canary"].encode() in data:
                hits.append({"sink": str(path.relative_to(root)), "case": case["id"]})
    return hits, scanned


def run_one(impl: agent.Impl, fixture: dict, mode: str, cases: list[dict]) -> dict:
    sandbox = agent.Sandbox(impl.label, "secret-" + mode, None)
    proc = server = None
    try:
        output = sandbox.root / "projected.json"
        spec = sandbox.root / "evidence-input.json"
        spec.write_text(json.dumps({"cases": cases, "output": str(output)}))
        env = sandbox.env_for({**runner.PROFILES[mode], "OPUTE_STANDALONE_ALLOW_MUTATIONS": "true"})
        proc = subprocess.Popen(fixture["command"], env={**env, "PARITY_EVIDENCE_SPEC": str(spec)},
                                cwd=sandbox.root, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                stderr=subprocess.STDOUT, bufsize=0)
        log = _ready(proc)
        before_hits, live_files = _scan(sandbox.root / "state", cases)
        live_rows = sandbox.sqlite_rows()["state/state.db"]
        projected = json.loads(output.read_text())
        proc.stdin.write(b"GO\n")
        proc.stdin.flush()
        proc.wait(timeout=30)
        log.extend(proc.stdout.read().decode(errors="replace").splitlines())
        if proc.returncode != 0:
            raise RuntimeError(f"projection writer failed: {log}")
        after_hits, stopped_files = _scan(sandbox.root / "state", cases)
        server = agent.Server(impl, sandbox, ["serve", f"--mode={mode}"],
                              {**runner.PROFILES[mode], "OPUTE_STANDALONE_ALLOW_MUTATIONS": "true"})
        cross_read._ready(server)
        responses = {"list": agent.mcp_call(sandbox, "tasks/list", {}, name="")}
        variables = sandbox.variables
        for case in cases:
            task_id = projected[case["id"]]["taskId"]
            variables["TASK_" + case["id"]] = task_id
            responses[case["id"]] = {method: agent.mcp_call(sandbox, method, {"taskId": task_id}, name=task_id)
                                      for method in ["tasks/get", "tasks/result"]}
        stop = server.stop()
        if stop != {"exit": 0, "killed": False}:
            raise RuntimeError(f"secret recovery server failed stop: {stop}")
        final_hits, recovered_files = _scan(sandbox.root / "state", cases)
        server_log = (sandbox.root / "server.log").read_text(errors="replace")
        for label, value in [("fixture-log", log), ("projection-output", projected),
                             ("http-responses", responses), ("server-log", server_log)]:
            for case in cases:
                if case["canary"] in json.dumps(value):
                    final_hits.append({"sink": label, "case": case["id"]})
        return {"mode": mode, "projected": projected, "liveRows": live_rows, "responses": responses,
                "fixtureLog": log, "serverLog": server_log, "variables": variables,
                "sinkHits": before_hits + after_hits + final_hits,
                "scannedFiles": {"live": live_files, "stopped": stopped_files, "recovered": recovered_files}}
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
        shutil.rmtree(sandbox.root, ignore_errors=True)


def run(go: agent.Impl, rust: agent.Impl, out: Path) -> dict:
    out.mkdir(parents=True, exist_ok=True)
    fixtures = evidence_process.build()
    provenance = {"sourceLock": json.loads((runner.REPO_ROOT / "baseline/source-lock.json").read_text()),
                  "goBinarySha256": go.sha256(), "rustBinarySha256": rust.sha256(),
                  "harnessSha256": runner.harness_sha256(), "driverSha256": runner.sha256_file(Path(__file__)),
                  "fixtures": fixtures}
    captures = {mode: run_one(go, fixtures["go"], mode, []) for mode in MODES}
    for capture in captures.values():
        if "error" in capture:
            raise ValueError("catalog fixture failed: " + capture["error"])
    cases = {mode: field_cases(captures[mode]["projected"]["_catalog"], mode) for mode in MODES}
    outcomes = {f"{impl.label}.{mode}": run_one(impl, fixtures[impl.label], mode, cases[mode])
                for mode in MODES for impl in [go, rust]}
    data = {"cases": cases, "outcomes": outcomes}
    problems = failures(data)
    provenance["changedDuringRun"] = (provenance["goBinarySha256"] != go.sha256()
        or provenance["rustBinarySha256"] != rust.sha256()
        or provenance["harnessSha256"] != runner.harness_sha256()
        or provenance["driverSha256"] != runner.sha256_file(Path(__file__)))
    (out / "outcomes.json.gz").write_bytes(runner._gz(data))
    summary = {"fieldCount": sum(map(len, cases.values())), "modes": list(MODES),
               "files": {"outcomes.json.gz": runner.sha256_file(out / "outcomes.json.gz")},
               "failures": problems, "provenance": provenance}
    (out / "summary.json").write_text(json.dumps(summary, indent=2, sort_keys=True))
    return summary


def failures(data: dict) -> list[str]:
    """Recompute coverage, actual durable writes, and leaks from retained bytes."""
    problems = []
    try:
        cases, outcomes = data["cases"], data["outcomes"]
        if set(cases) != set(MODES) or set(outcomes) != {f"{side}.{mode}" for side in ["go", "rust"] for mode in MODES}:
            raise ValueError("secret mode/implementation matrix incomplete")
        all_cases = [case for mode in MODES for case in cases[mode]]
        canaries = [case["canary"] for case in all_cases]
        if not canaries or len(set(canaries)) != len(canaries) or any(len(c) < 64 for c in canaries):
            raise ValueError("secret canaries absent, reused, or too short")
        for mode in MODES:
            projected_sides = []
            for side in ["go", "rust"]:
                key = f"{side}.{mode}"
                observation = outcomes[key]
                projected = observation["projected"]
                derived = field_cases(projected["_catalog"], mode)
                expected = {(c["id"], c["tool"], tuple(c["path"])) for c in derived}
                actual = {(c["id"], c["tool"], tuple(c["path"])) for c in cases[mode]}
                if actual != expected or len(actual) != len(cases[mode]):
                    raise ValueError(f"{key}: catalog field coverage incomplete")
                if set(projected) != {"_catalog"} | {c["id"] for c in cases[mode]}:
                    raise ValueError(f"{key}: projected case coverage incomplete")
                for phase in ["live", "stopped", "recovered"]:
                    files = observation["scannedFiles"][phase]
                    paths = [file["path"] for file in files]
                    required = {"state.db", "state.db-wal", "state.db-shm"} if phase == "live" else {"state.db"}
                    if not required <= set(paths) or len(paths) != len(set(paths)):
                        raise ValueError(f"{key}: {phase} sink inventory incomplete")
                    for file in files:
                        raw = base64.b64decode(file["dataBase64"], validate=True)
                        if len(raw) != file["bytes"] or runner.sha256_bytes(raw) != file["sha256"] or (file["path"] in required and not raw):
                            raise ValueError(f"{key}: invalid retained {phase} sink bytes")
                        if any(canary.encode() in raw for canary in canaries):
                            raise ValueError(f"{key}: canary persisted in {phase}/{file['path']}")
                for sink in ["projected", "responses", "fixtureLog", "serverLog"]:
                    if any(canary in json.dumps(observation[sink]) for canary in canaries):
                        raise ValueError(f"{key}: canary in {sink}")
                if observation["sinkHits"]:
                    raise ValueError(f"{key}: recorded canary leak")
                tables = observation["liveRows"]["tables"]
                for case in cases[mode]:
                    value = projected[case["id"]]
                    field = case["path"][0]
                    if value["arguments"].get(field) != "[redacted]":
                        raise ValueError(f"{key}: marked argument not redacted")
                    document, state = value["document"], value["state"]
                    if document["variables"]["inputs"]["recipeSecret"] != "[redacted]":
                        raise ValueError(f"{key}: secret recipe input not redacted")
                    for role in ["action", "validate", "compensate", "recover"]:
                        if document["nodes"][0][role]["args"].get(field) != "[redacted]":
                            raise ValueError(f"{key}: {role} argument not redacted")
                    if state["outputs"]["derived"] != {"redacted": True}:
                        raise ValueError(f"{key}: untyped derived output not redacted")
                    if "value" in state["context"]["secret"] or "value" in state["contextHistory"][0]:
                        raise ValueError(f"{key}: secret context value retained")
                    task_id = value["taskId"]
                    operation = tables["operations"][task_id]
                    if not operation["task_snapshot_json"]:
                        raise ValueError(f"{key}: no durable task snapshot")
                    if case["id"] not in tables["plan_runs"] or case["id"] not in tables["active_capabilities"]:
                        raise ValueError(f"{key}: no durable plan/active selection")
                    if not any(row["operation_id"] == case["tool"] and json.loads(row["arguments_json"]) == value["arguments"]
                               for row in tables["capability_invocations"].values()):
                        raise ValueError(f"{key}: no invocation audit")
                    snapshot = json.loads(operation["task_snapshot_json"])
                    plan = tables["plan_runs"][case["id"]]
                    active = tables["active_capabilities"][case["id"]]
                    if (snapshot["toolArgs"] != value["arguments"]
                            or any(json.loads(plan[column]) != expected_value for column, expected_value in
                                   [("plan_json", document), ("recipe_json", document), ("state_json", state)])
                            or json.loads(active["input_bindings_json"]) != value["arguments"]
                            or json.loads(active["observation_json"]) != state):
                        raise ValueError(f"{key}: durable projections absent or changed")
                    response = observation["responses"][case["id"]]
                    if response["tasks/get"]["body"]["result"]["status"] != "completed":
                        raise ValueError(f"{key}: restored task not completed")
                    if response["tasks/result"]["body"]["error"]["code"] != -32603:
                        raise ValueError(f"{key}: unsupported result method changed")
                if observation["responses"]["list"]["body"]["error"]["code"] != -32601:
                    raise ValueError(f"{key}: unsupported list method changed")
                projected_sides.append(canon.substitute(projected, observation["variables"]))
            # D13 (redact-unmarked-projections) reaches these same plan/state
            # documents through the forward-looking plan-document/run-state
            # storage projections: an untyped test field like "visible" or
            # "marker" -- unrelated to any writeOnly-marked secret -- is
            # fail-closed redacted on the Rust side only. Converge both sides
            # on that declared divergence before diffing, exactly as shape.py
            # does for sqliteRows, rather than inventing a second rule.
            reduced_left, reduced_right, removed_left, removed_right = canon._drop_value_deep_paired(
                projected_sides[0], projected_sides[1], canon.REDACTED_MARKER)
            if canon.diff(reduced_left, reduced_right):
                raise ValueError(f"{mode}: Go/Rust durable projections differ")
            if canon._set_key(removed_left) == canon._set_key(removed_right):
                raise ValueError(f"{mode}: D13.redacted-structured-content declared but not exercised (stale)")
    except (KeyError, TypeError, ValueError, AttributeError) as exc:
        problems.append(str(exc))
    return problems
