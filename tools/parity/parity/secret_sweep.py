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
# D24 (write-only-relay-token): fields Rust publishes writeOnly and Go's
# catalog leaves unmarked. Rust is swept for them too; Go, which stores them
# verbatim by design, is not.
WRITE_ONLY = runner.REPO_ROOT / "crates/host-agent/catalog/write-only.json"


def d24_fields() -> dict[str, list[list[str]]]:
    return json.loads(WRITE_ONLY.read_text())["fields"]


def _at(value: dict, path: list[str]):
    for key in path:
        value = value.get(key) if isinstance(value, dict) else None
    return value


def _nested(path: list[str], leaf) -> dict:
    for key in reversed(path[1:]):
        leaf = {key: leaf}
    return {path[0]: leaf}


def _marked(schema: dict, path: list[str]) -> bool:
    for key in path:
        schema = (schema.get("properties") or {}).get(key) if isinstance(schema, dict) else None
    return isinstance(schema, dict) and schema.get("writeOnly") is True


def field_cases(catalog: dict, mode: str, only: dict[str, list[list[str]]] | None = None) -> list[dict]:
    """One case per write-only input field the catalog marks; with `only`,
    the listed fields (D24), marked or not."""
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
            if only is not None:
                paths = [(path, {"type": "string"}) for path in only.get(descriptor["name"], [])
                         if direction == "inputSchema"]
            if paths and direction != "inputSchema":
                # The pinned catalog currently has no write-only output field.
                # A new output marking requires exercising that actual result
                # boundary, never reusing an input-only fixture accidentally.
                raise ValueError("write-only output schema requires a result-canary fixture")
            for path, child in paths:
                if any(key.startswith("*") for key in path):
                    raise ValueError("write-only field under items/additionalProperties/combinators "
                                     "requires its own fixture")
                name = descriptor["name"]
                identifier = f"{mode}:{name}:{'.'.join(path)}"
                canary = "M5_SECRET_CANARY_" + uuid.uuid4().hex + uuid.uuid4().hex
                value = {"probe": canary} if child.get("type") == "object" else canary
                args = {**_nested(path, value), "visible": "safe"}
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
    d24 = {mode: field_cases(captures[mode]["projected"]["_catalog"], mode, only=d24_fields()) for mode in MODES}
    outcomes = {f"{impl.label}.{mode}": run_one(impl, fixtures[impl.label], mode,
                                                cases[mode] + (d24[mode] if impl is rust else []))
                for mode in MODES for impl in [go, rust]}
    data = {"cases": cases, "d24Cases": d24, "outcomes": outcomes}
    problems = failures(data)
    provenance["changedDuringRun"] = (provenance["goBinarySha256"] != go.sha256()
        or provenance["rustBinarySha256"] != rust.sha256()
        or provenance["harnessSha256"] != runner.harness_sha256()
        or provenance["driverSha256"] != runner.sha256_file(Path(__file__)))
    (out / "outcomes.json.gz").write_bytes(runner._gz(data))
    summary = {"fieldCount": sum(map(len, cases.values())), "d24FieldCount": sum(map(len, d24.values())),
               "modes": list(MODES),
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
        # Evidence captured before D24 carries no D24 cases; the M5 gate
        # (m5_verify.secret_failures) requires them of every current capture.
        legacy = "d24Cases" not in data
        d24 = {mode: [] for mode in MODES} if legacy else data["d24Cases"]
        if set(d24) != set(MODES):
            raise ValueError("D24 mode coverage incomplete")
        all_cases = [case for mode in MODES for case in cases[mode] + d24[mode]]
        canaries = [case["canary"] for case in all_cases]
        if not canaries or len(set(canaries)) != len(canaries) or any(len(c) < 64 for c in canaries):
            raise ValueError("secret canaries absent, reused, or too short")
        for mode in MODES:
            projected_sides = []
            for side in ["go", "rust"]:
                key = f"{side}.{mode}"
                observation = outcomes[key]
                projected = observation["projected"]
                catalog = projected["_catalog"]
                if side == "rust":
                    # D19/D21/D22: coverage is derived from Rust's catalog in
                    # Go's spelling, so a renamed secret-bearing tool
                    # (put_cluster_secret, Go put_k8s_secret) is the same case.
                    catalog = canon.apply_divergences(
                        {"_catalog": catalog}, None, ["D19.neutral-host-tool-names"],
                        canon.load_divergences(runner.DIVERGENCE_FILE), rust_side="a")[0]["_catalog"]
                derived = field_cases(catalog, mode)
                expected = {(c["id"], c["tool"], tuple(c["path"])) for c in derived}
                actual = {(c["id"], c["tool"], tuple(c["path"])) for c in cases[mode]}
                if actual != expected or len(actual) != len(cases[mode]):
                    raise ValueError(f"{key}: catalog field coverage incomplete")
                side_cases = cases[mode] + (d24[mode] if side == "rust" else [])
                if side == "rust" and not legacy:
                    # D24: Rust's own catalog marks every declared field the
                    # mode publishes, and the sweep covers each one.
                    raw = {d["name"]: d.get("inputSchema") or {} for d in projected["_catalog"]["tools"]}
                    declared = {(tool, tuple(path)) for tool, paths in d24_fields().items() if tool in raw
                                for path in paths}
                    if declared != {(c["tool"], tuple(c["path"])) for c in d24[mode]}:
                        raise ValueError(f"{key}: D24 field coverage incomplete")
                    if not all(_marked(raw[tool], list(path)) for tool, path in declared):
                        raise ValueError(f"{key}: D24 field not published writeOnly")
                if set(projected) != {"_catalog"} | {c["id"] for c in side_cases}:
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
                for case in side_cases:
                    value = projected[case["id"]]
                    if _at(value["arguments"], case["path"]) != "[redacted]":
                        raise ValueError(f"{key}: marked argument not redacted")
                    document, state = value["document"], value["state"]
                    if document["variables"]["inputs"]["recipeSecret"] != "[redacted]":
                        raise ValueError(f"{key}: secret recipe input not redacted")
                    for role in ["action", "validate", "compensate", "recover"]:
                        if _at(document["nodes"][0][role]["args"], case["path"]) != "[redacted]":
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
                # D24's cases are Rust's alone; the sides are compared on Go's.
                projected = {k: v for k, v in projected.items() if k not in {c["id"] for c in d24[mode]}}
                projected = canon.substitute(projected, observation["variables"])
                if side == "rust":
                    # D19 (neutral-host-tool-names): the projected catalog
                    # carries Rust's neutral names and revision. Rewrite them
                    # to Go's spelling with the wire comparison's own rule;
                    # everything else is compared as captured.
                    projected, _, _ = canon.apply_divergences(
                        projected, None, ["D19.neutral-host-tool-names"],
                        canon.load_divergences(runner.DIVERGENCE_FILE), rust_side="a")
                projected_sides.append(projected)
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
