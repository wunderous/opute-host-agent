"""Full database schema and row comparisons across the existing wire matrix.

Only declared contract divergences and typed wall-clock fields are normalized.
All raw iterations are retained so the gate can rederive its result.
"""
from __future__ import annotations

import copy
import concurrent.futures
import json
from pathlib import Path

from . import agent, canon, runner


def scenario_for_shape(original: dict) -> dict:
    scenario = copy.deepcopy(original)
    scenario["collect"] = list(dict.fromkeys(scenario.get("collect", []) + ["sqlite", "sqliteRows"]))
    return scenario


def database_observation(raw: dict) -> tuple[dict, dict]:
    observation, variables = raw["observation"], dict(raw["variables"])
    for label, result in observation.get("steps", {}).items():
        task_id = runner._lookup(result, ["body", "result", "taskId"])
        if isinstance(task_id, str) and task_id and task_id not in variables.values():
            variables["M5_TASK_" + label] = task_id
    return {key: observation.get(key, {}) for key in ["sqlite", "sqliteRows"]}, variables


def compare_spec(scenario: dict | None = None) -> dict:
    root = ["sqliteRows", "state/state.db", "tables"]
    masks = []
    for table, columns in {
        "operations": ("created_at", "updated_at"),
        "plan_runs": ("created_at", "updated_at"),
        "provider_generations": ("created_at",),
        "capability_invocations": ("created_at",),
        "resource_registry": ("created_at", "updated_at"),
    }.items():
        for column in columns:
            masks.append({"path": root + [table, "*", column], "type": "rfc3339",
                          "reason": "runtime write wall-clock time", "optional": True})
    for column in ["createdAt", "lastUpdatedAt"]:
        masks.append({"path": root + ["operations", "*", "task_snapshot_json", "$json", column],
                      "type": "rfc3339", "reason": "persisted task wall-clock time", "optional": True})
    spec = {"masks": masks,
            "parseJson": [root + ["*", "*", column] for column in
                          ["task_snapshot_json", "result_json", "plan_json", "recipe_json", "state_json",
                           "arguments_json", "binding_json", "observation_json", "coordinates_json",
                           "descriptor_json", "manifest_json", "input_bindings_json"]]}
    # Durable structured observations have the same live-host volatility as
    # the exact wire result that produced them. Reuse the scenario's reviewed
    # paths and types, rather than invent a broader persisted-data mask.
    original = (scenario or {}).get("compare", {})
    for key in ["masks", "sets", "omitempty"]:
        projected = []
        for rule in original.get(key, []):
            path = rule if key == "sets" else rule["path"]
            if "structuredContent" not in path:
                continue
            suffix = path[path.index("structuredContent") + 1:]
            for column in ["result_json", "observation_json"]:
                target = root + ["capability_invocations", "*", column, "$json", "structured"] + suffix
                if key == "sets":
                    projected.append(target)
                else:
                    new = {**rule, "path": target}
                    if key == "masks":
                        new["optional"] = True  # Error observations omit structured data.
                    projected.append(new)
        spec.setdefault(key, []).extend(projected)
    return spec


def evaluate(scenario: dict, raw: dict) -> dict:
    normalized, violations = {}, {}
    for side, label in [("a", "go"), ("b", "rust")]:
        document, variables = database_observation(raw[side])
        spec = compare_spec(scenario)
        rows = runner._lookup(document, ["sqliteRows", "state/state.db", "tables", "capability_invocations"])
        if isinstance(rows, dict):
            for key, row in rows.items():
                try:
                    reservation = json.loads(row["binding_json"]).get("reservationId")
                except (KeyError, TypeError, ValueError):
                    reservation = None
                if reservation != "control":
                    spec["masks"].append({
                        "path": ["sqliteRows", "state/state.db", "tables", "capability_invocations",
                                 key, "binding_json", "$json", "reservationId"],
                        "type": "host-reservation-id",
                        "reason": "host reservation wall-clock nanoseconds and per-process sequence",
                    })
        normalized[side], violations[side] = canon.normalize(document, spec, variables)
        invariant_observation = dict(raw[side]["observation"])
        # Preserve the scenario's declared X2 scope. Some existing scenarios
        # deliberately invoke accepted control reads and use X2 for their
        # empty execution trace only; new database collection does not turn
        # those reads into rejected work. Their audit rows still compare.
        if "sqlite" not in scenario.get("_originalCollect", scenario.get("collect", [])):
            invariant_observation.pop("sqlite", None)
        if "sqliteRows" not in scenario.get("_originalCollect", scenario.get("collect", [])):
            invariant_observation.pop("sqliteRows", None)
        violations[side] += runner.invariant_violations(scenario, invariant_observation, label)
    registry = canon.load_divergences(runner.DIVERGENCE_FILE)
    # Apply approved database rules only when their path exists on either side.
    # Missing tables are otherwise compared and never inferred from a count.
    declared = list(scenario.get("compare", {}).get("divergences", []))
    # Collecting both databases extends D8's (and D12's) already approved
    # scope to scenarios that previously observed only wire responses: no
    # scenario's own "trace"-only collection ever touches sqlite/sqliteRows,
    # so declaring these there would always be stale in the regular
    # (non-shape) comparison. Detect and apply them dynamically instead,
    # from the actual normalized documents, exactly like D11 below.
    for rule in ["D8.authz-tables", "D8.authz-row-counts", "D8.authz-rows",
                 "D12.refused-task-operation-count"]:
        if rule in declared:
            continue
        _, _, applied = canon.apply_divergences(normalized["a"], normalized["b"], [rule], registry)
        if applied and not applied[0]["stale"]:
            declared.append(rule)
    go_invocations = runner._lookup(normalized["a"], ["sqliteRows", "state/state.db", "tables", "capability_invocations"])
    if isinstance(go_invocations, dict) and any(
            runner._lookup(row, ["result_json", "$json", "error", "code"]) == "invalid_arguments"
            for row in go_invocations.values()):
        declared.append("D11.rejected-invocation-count")
    rules = [rule for rule in dict.fromkeys(declared)
             if rule != "D13.redacted-structured-content"
             and registry[rule]["path"][0] in ("sqlite", "sqliteRows")
             and any(runner._lookup(value, registry[rule]["path"]) is not None
                     for value in normalized.values())]
    a, b, divergences = canon.apply_divergences(normalized["a"], normalized["b"], rules, registry)
    for side, value in [("a", a), ("b", b)]:
        if side == "a" and "D12.refused-task-operation-count" in declared:
            operations = runner._lookup(value, ["sqliteRows", "state/state.db", "tables", "operations"])
            if isinstance(operations, dict):
                retained = {}
                for key, row in operations.items():
                    # A captured task handle and the actual typed refusal are
                    # both required. An admitted operation cannot be dropped.
                    error = runner._lookup(row, ["result_json", "$json", "structuredContent"]) or {}
                    if (key.startswith("${") and row.get("operation_id") == key
                            and error.get("owner") == "admission"
                            and error.get("code") == "resource_binding"
                            and runner._lookup(row, ["result_json", "$json", "isError"]) is True):
                        continue
                    retained[key] = row
                value["sqliteRows"]["state/state.db"]["tables"]["operations"] = retained
        invocations = runner._lookup(value, ["sqliteRows", "state/state.db", "tables", "capability_invocations"])
        if isinstance(invocations, dict):
            # D11 permits only schema-invalid invocation audits to be absent.
            # Accepted calls in the very same scenario remain fully compared.
            if side == "a" and "D11.rejected-invocation-count" in declared:
                invocations = {key: row for key, row in invocations.items()
                               if runner._lookup(row, ["result_json", "$json", "error", "code"]) != "invalid_arguments"}
            rows = []
            for key, row in invocations.items():
                if key != row.get("invocation_id") or not canon.MASK_TYPES["uuid"](key):
                    violations[side].append({"invariant": "INVOCATION_ID", "reason": "invalid row identity"})
                row = dict(row)
                row["invocation_id"] = {"$masked": "uuid"}
                rows.append(row)
            value["sqliteRows"]["state/state.db"]["tables"]["capability_invocations"] = sorted(rows, key=canon.canonical_json)
            # Derive the comparable count from the same retained rows.
            if "D11.rejected-invocation-count" in declared:
                value["sqlite"]["state/state.db"]["rowCounts"].pop("capability_invocations", None)
    # D13 reaches inside each capability_invocations row's own structured
    # content, but those rows are keyed by a random invocation_id that never
    # aligns between Go and Rust -- the loop above masks that id and sorts
    # rows by remaining content first precisely so they become
    # index-comparable, so D13 is applied in this later pass, not alongside
    # the table/row-identity rules above. Unlike those rules, whether D13
    # has anything to converge depends on each scenario's own structured
    # content rather than a per-scenario declaration (no scenario's base
    # "trace"-only collection ever touches sqliteRows, so declaring it there
    # is always stale); detect and apply it dynamically here instead,
    # exactly like the D8.authz-* rules above.
    probe_a, probe_b, probe = canon.apply_divergences(a, b, ["D13.redacted-structured-content"], registry)
    if probe and not probe[0]["stale"]:
        a, b = probe_a, probe_b
        divergences += probe
    return {"diff": canon.diff(a, b), "maskViolations": violations, "divergences": divergences}


def run(go: agent.Impl, rust: agent.Impl, out: Path, repeats: int = 5,
        workers: int = 6, ids: list[str] | None = None) -> dict:
    out.mkdir(parents=True, exist_ok=True)
    scenarios = runner.load_scenarios(ids)
    provenance = {"sourceLock": json.loads((runner.REPO_ROOT / "baseline/source-lock.json").read_text()),
                  "goBinarySha256": go.sha256(), "rustBinarySha256": rust.sha256(),
                  "harnessSha256": runner.harness_sha256(), "driverSha256": runner.sha256_file(Path(__file__)),
                  "scenarioHashes": {s["id"]: s["_sha256"] for s in scenarios}}

    def job(item: tuple[dict, int]) -> tuple[str, dict]:
        original, iteration = item
        scenario = scenario_for_shape(original)
        scenario["_originalCollect"] = original.get("collect", [])
        raw = {}
        # execute() already serializes an exclusive (fixed-port) scenario
        # through runner._EXCLUSIVE internally (see exclusive_ports()); an
        # outer acquisition here would re-enter that same non-reentrant
        # lock on the same thread and deadlock forever, exactly as
        # runner.compare_once's identical two-sides sequence relies on
        # execute()'s own serialization rather than wrapping it again.
        for side, impl, other in [("a", go, rust), ("b", rust, go)]:
            observation, variables = runner._execute_safe(impl, scenario, side, f"shape-{iteration}", other)
            raw[side] = {"observation": observation, "variables": variables}
        return f"{original['id']}.{iteration}", {"raw": raw, **evaluate(scenario, raw)}

    with concurrent.futures.ThreadPoolExecutor(max_workers=workers) as pool:
        outcomes = dict(pool.map(job, [(s, i) for i in range(repeats) for s in scenarios]))
    failed = [key for key, result in outcomes.items()
              if result["diff"] or any(result["maskViolations"].values()) or runner.stale_divergences(result)]
    (out / "outcomes.json.gz").write_bytes(runner._gz(outcomes))
    provenance["changedDuringRun"] = any([
        provenance["goBinarySha256"] != go.sha256(), provenance["rustBinarySha256"] != rust.sha256(),
        provenance["harnessSha256"] != runner.harness_sha256(),
        provenance["driverSha256"] != runner.sha256_file(Path(__file__)),
        provenance["scenarioHashes"] != {s["id"]: s["_sha256"] for s in runner.load_scenarios(ids)},
    ])
    summary = {"repeats": repeats, "total": len(outcomes), "passed": len(outcomes) - len(failed),
               "failed": len(failed), "failedCases": failed, "provenance": provenance,
               "files": {"outcomes.json.gz": runner.sha256_file(out / "outcomes.json.gz")}}
    (out / "summary.json").write_text(json.dumps(summary, indent=2, sort_keys=True))
    return summary
