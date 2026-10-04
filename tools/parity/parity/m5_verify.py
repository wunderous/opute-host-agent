"""M5 evidence checks. Missing exit requirements are never inferred as passed."""
import gzip
import json
from pathlib import Path

from . import canon, cross_read, evidence_process, migration, runner, secret_sweep, shape, state_process, storage_crash, unknown_projection

# These are the six named validations in milestones.md, not a configurable
# subset chosen by whichever checks happen to be green.
REQUIREMENTS = ("shape", "cross-read", "crash", "secrets", "unknown-projection", "migration")


def cross_read_failures(summary: dict, outcomes: dict) -> list[str]:
    failures = []
    expected = {f"{a}-writes-{b}-reads.{case}.{repeat}"
                for a, b in [("go", "rust"), ("rust", "go")]
                for case in cross_read.CASES for repeat in range(3)}
    if set(outcomes) != expected or set(summary.get("directions", [])) != expected:
        failures.append("cross-read matrix incomplete or unexpected")
    for key in expected & set(outcomes):
        result = outcomes[key]
        case = key.split(".")[1]
        try:
            raw = result["raw"]
            task_id = raw["written"]["taskId"]
            if not task_id or task_id not in raw["written"]["persisted"]:
                raise ValueError("writer has no durable task row")
            if raw["written"]["live"]["body"]["result"]["status"] != cross_read.CASES[case][0]:
                raise ValueError("wrong live writer status")
            normalized = []
            for side in ["reference", "readBack"]:
                observation = raw[side]
                if observation["taskId"] != task_id:
                    raise ValueError("task identity changed")
                if observation["tasksGet"]["body"]["result"]["status"] != cross_read.CASES[case][1]:
                    raise ValueError("wrong restored status")
                if observation["tasksList"]["body"]["error"]["code"] != -32601:
                    raise ValueError("missing pinned unsupported list query response")
                row = observation["stateRows"]["tables"]["operations"][task_id]
                if not row["task_snapshot_json"]:
                    raise ValueError("missing restored snapshot")
                value, violations = canon.normalize(observation, cross_read.COMPARE_SPEC, observation["variables"])
                if violations:
                    raise ValueError("invalid typed mask")
                normalized.append(value)
            if canon.diff(*normalized):
                raise ValueError("raw reader observations differ")
            if result.get("error") or result.get("diff") or any(result.get("maskViolations", {}).values()):
                raise ValueError("recorded check failed")
        except (KeyError, TypeError, ValueError) as exc:
            failures.append(f"{key}: {exc}")
    return failures


def migration_failures(summary: dict, outcomes: dict) -> list[str]:
    failures = []
    repeats = summary.get("repeats")
    if not isinstance(repeats, int) or repeats < 3:
        return ["migration repeat count below three"]
    expected = {f"{case}.{i}" for case in migration.CASES for i in range(repeats)}
    if set(outcomes) != expected or summary.get("cases") != list(migration.CASES):
        failures.append("migration matrix incomplete or unexpected")
    for key in expected & set(outcomes):
        case = key.rsplit(".", 1)[0]
        try:
            result = outcomes[key]
            raw = result["raw"]
            normalized = []
            for label in ["go", "rust"]:
                observation = raw[label]
                if observation["fixture"] != {"case": case, "releaseFixtureSha256": runner.sha256_file(migration.RELEASE_FIXTURE)}:
                    raise ValueError("wrong migration fixture")
                problems = migration.preservation_failures(observation, case)
                if problems:
                    raise ValueError("seed state not preserved: " + "; ".join(problems))
                value, violations = canon.normalize(observation, migration.COMPARE_SPEC, {})
                if violations:
                    raise ValueError("invalid migration mask")
                normalized.append(value)
            if canon.diff(*normalized):
                raise ValueError("raw upgraded schemas or rows differ")
            if result.get("diff") or result.get("failures"):
                raise ValueError("recorded migration failed")
        except (KeyError, TypeError, ValueError) as exc:
            failures.append(f"{key}: {exc}")
    return failures


def refused_row_failures(scenario: dict, raw: dict) -> list[str]:
    declared = scenario.get("compare", {}).get("divergences", [])
    if not any(rule.startswith("D12.") for rule in declared):
        return []
    rows = {}
    for side in ["a", "b"]:
        document, variables = shape.database_observation(raw[side])
        document = canon.substitute(document, variables)
        rows[side] = runner._lookup(document, ["sqliteRows", "state/state.db", "tables", "operations"]) or {}
    failures = []
    for key, row in rows["a"].items():
        if key in rows["b"]:
            continue
        try:
            result = json.loads(row["result_json"])
            error = result.get("structuredContent", {})
            if (not key.startswith("${") or row.get("operation_id") != key
                    or result.get("isError") is not True
                    or error.get("owner") != "admission" or error.get("code") != "resource_binding"):
                raise ValueError("omitted row is not a captured, typed refusal")
        except (KeyError, TypeError, ValueError) as exc:
            failures.append(f"{key}: {exc}")
    return failures


def shape_failures(summary: dict, outcomes: dict) -> list[str]:
    scenarios = runner.load_scenarios()
    repeats = summary.get("repeats")
    if not isinstance(repeats, int) or repeats < 5:
        return ["database shape repeat count below five"]
    hashes = {s["id"]: s["_sha256"] for s in scenarios}
    if summary.get("provenance", {}).get("scenarioHashes") != hashes:
        return ["database shape scenario matrix changed or incomplete"]
    expected = {f"{s['id']}.{i}" for s in scenarios for i in range(repeats)}
    failures = []
    if set(outcomes) != expected:
        failures.append("database shape raw iteration matrix incomplete or unexpected")
    for scenario in scenarios:
        extended = shape.scenario_for_shape(scenario)
        extended["_originalCollect"] = scenario.get("collect", [])
        for iteration in range(repeats):
            key = f"{scenario['id']}.{iteration}"
            if key not in outcomes:
                continue
            try:
                result = outcomes[key]
                raw = result["raw"]
                if set(raw) != {"a", "b"}:
                    raise ValueError("missing twin observations")
                for side in raw.values():
                    observation = side["observation"]
                    if not all(isinstance(observation.get(field), dict) for field in ["steps", "sqlite", "sqliteRows"]):
                        raise ValueError("missing database or execution observations")
                    expected_steps = {step.get("as", f"step{i}") for i, step in enumerate(scenario["steps"])}
                    observed_steps = set(observation["steps"])
                    if not expected_steps <= observed_steps or observed_steps - expected_steps - {"_implicitStop"}:
                        raise ValueError("scenario execution steps missing or unexpected")
                    if set(observation["sqlite"]) != set(observation["sqliteRows"]):
                        raise ValueError("schema and row database sets differ")
                    if any(step.get("ready") is True for step in observation["steps"].values() if isinstance(step, dict)):
                        if "state/state.db" not in observation["sqliteRows"]:
                            raise ValueError("started agent has no state database observations")
                actual = shape.evaluate(extended, raw)
                if refused_row_failures(scenario, raw):
                    raise ValueError("D12 hides an admitted or unidentified operation row")
                if actual["diff"] or any(actual["maskViolations"].values()) or runner.stale_divergences(actual):
                    raise ValueError("raw schemas, rows, or invariants differ")
                if result.get("diff") or any(result.get("maskViolations", {}).values()) or runner.stale_divergences(result):
                    raise ValueError("recorded shape check failed")
            except (KeyError, TypeError, ValueError, AttributeError) as exc:
                failures.append(f"{key}: {exc}")
    return failures


def crash_failures(summary: dict, outcomes: dict) -> list[str]:
    seeds = summary.get("seeds")
    if not isinstance(seeds, int) or seeds < 200:
        return ["crash corpus below 200 seeds"]
    if summary.get("cases") != list(storage_crash.CASES):
        return ["crash checkpoint matrix changed or incomplete"]
    problems = []
    expected = {str(seed) for seed in range(seeds)}
    if set(outcomes) != expected:
        problems.append("crash raw seed matrix incomplete or unexpected")
    for seed in range(seeds):
        if str(seed) not in outcomes:
            continue
        case = storage_crash.CASES[seed % len(storage_crash.CASES)]
        try:
            result = outcomes[str(seed)]
            if set(result["raw"]) != {"go", "rust"}:
                raise ValueError("missing twin crash observations")
            actual = storage_crash.evaluate(result["raw"], case, seed)
            if actual["diff"] or actual["failures"]:
                raise ValueError("raw recovery failed: " + "; ".join(actual["failures"]))
            if result.get("diff") or result.get("failures"):
                raise ValueError("recorded crash check failed")
        except (KeyError, TypeError, ValueError, AttributeError) as exc:
            problems.append(f"seed {seed}: {exc}")
    return problems


def unknown_projection_failures(summary: dict, outcomes: dict) -> list[str]:
    problems = unknown_projection.failures(outcomes)
    if summary.get("cases") != list(unknown_projection.CASES):
        problems.append("unknown projection case list changed")
    return problems


def secret_failures(summary: dict, outcomes: dict) -> list[str]:
    problems = secret_sweep.failures(outcomes)
    if summary.get("modes") != list(secret_sweep.MODES) or summary.get("fieldCount") != sum(len(c) for c in outcomes.get("cases", {}).values()):
        problems.append("secret field count or mode coverage changed")
    if summary.get("failures"):
        problems.append("recorded secret sweep failed")
    return problems


def check(root: Path, manifest: dict, lock: dict, report, load, sha) -> dict[str, str]:
    statuses = {}
    evidence = manifest.get("m5Evidence", {})
    for requirement in REQUIREMENTS:
        scope = f"m5:{requirement}"
        status = "unverified"
        entry = evidence.get(requirement, {})
        if not entry.get("summary"):
            report.problem(f"{requirement}: missing evidence", scope)
            statuses[requirement] = status
            continue
        path = root / entry["summary"]
        summary, error = load(path)
        try:
            if error:
                raise ValueError(error)
            prov = summary["provenance"]
            if prov["sourceLock"] != lock:
                raise ValueError("stale source lock")
            if prov["goBinarySha256"] != manifest["goReference"]["binarySha256"] or prov["rustBinarySha256"] != manifest["rustCandidate"]["binarySha256"]:
                raise ValueError("stale binary provenance")
            if prov["harnessSha256"] != runner.harness_sha256():
                raise ValueError("stale harness provenance")
            if prov.get("changedDuringRun"):
                raise ValueError("evidence producer changed during run")
            drivers = {"cross-read": (cross_read, "directions.json", cross_read_failures),
                       "migration": (migration, "outcomes.json", migration_failures),
                       "shape": (shape, "outcomes.json.gz", shape_failures),
                       "crash": (storage_crash, "outcomes.json.gz", crash_failures),
                       "secrets": (secret_sweep, "outcomes.json.gz", secret_failures),
                       "unknown-projection": (unknown_projection, "outcomes.json", unknown_projection_failures)}
            if requirement not in drivers:
                raise ValueError("requirement checker not implemented")
            driver, filename, validate = drivers[requirement]
            if prov["driverSha256"] != sha(Path(driver.__file__)):
                raise ValueError("stale driver provenance")
            if requirement == "migration" and prov["releaseFixtureSha256"] != sha(migration.RELEASE_FIXTURE):
                raise ValueError("stale release fixture provenance")
            if requirement in ["crash", "secrets"]:
                fixture = prov["fixtures"]
                if requirement == "secrets":
                    if fixture["runtimeSources"] != {path: sha(root / path) for path in evidence_process.RUNTIME_SOURCES}:
                        raise ValueError("stale projection runtime sources")
                    for field, source in [("builderSha256", Path(evidence_process.__file__)),
                                          ("goFixtureSha256", evidence_process.GO_FIXTURE),
                                          ("rustFixtureSha256", evidence_process.RUST_FIXTURE),
                                          ("projectionSha256", root / "crates/host-agent/src/evidence.rs")]:
                        if fixture[field] != sha(source):
                            raise ValueError(f"stale projection fixture {field}")
                    for label in ["go", "rust"]:
                        if fixture[label]["binarySha256"] != sha(Path(fixture[label]["command"][0])):
                            raise ValueError("stale projection fixture executable")
                    fixture = fixture["storageFixtures"]
                if fixture["sourceCommit"] != lock["sourceCommit"] or fixture["sourceTree"] != lock["sourceTree"]:
                    raise ValueError("wrong pinned crash fixture source")
                for field, source in [
                    ("builderSha256", Path(state_process.__file__)),
                    ("goFixtureSha256", state_process.GO_FIXTURE),
                    ("rustFixtureSha256", state_process.RUST_FIXTURE),
                    ("goStoreSha256", state_process.GO_SOURCE / "internal/state/store.go"),
                    ("rustStoreSha256", root / "crates/host-agent/src/store.rs"),
                ]:
                    if fixture[field] != sha(source):
                        raise ValueError(f"stale crash fixture {field}")
                for label in ["go", "rust"]:
                    if fixture[label]["binarySha256"] != sha(Path(fixture[label]["command"][0])):
                        raise ValueError("stale crash fixture executable")
            outcome_path = path.parent / filename
            if summary["files"][filename] != sha(outcome_path):
                raise ValueError("modified raw observations")
            if filename.endswith(".gz"):
                outcomes = json.loads(gzip.decompress(outcome_path.read_bytes()))
                error = None
            else:
                outcomes, error = load(outcome_path)
            if error:
                raise ValueError(error)
            failures = validate(summary, outcomes)
            for failure in failures:
                report.problem(failure, scope)
            status = "fail" if failures else "pass"
        except (KeyError, TypeError, ValueError, OSError) as exc:
            report.problem(f"{requirement}: {exc}", scope)
        statuses[requirement] = status
    return statuses
