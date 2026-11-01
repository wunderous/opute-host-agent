"""M12 validation 5: the soak, Go and Rust twins side by side.

Two disposable twins from the base golden run one seeded mixed workload
(t2guest/soak.py) for the same number of iterations:

- read-only calls;
- task-aware mutations (host files, SQLite databases);
- input-required tasks, answered or cancelled;
- host plans;
- provider replacement (the pinned Go k3s provider, reloaded);
- periodic restarts, alternating SIGTERM and SIGKILL.

The host mirrors each guest's records every pollSec and never restarts
anything: a twin, guest program or agent that dies ends the run, which is
recorded as a crash.

Receipts per side: ops, samples, events (gzipped JSONL) and final (journal
error lines and every SQLite row). evaluate() re-derives every result from
them against the committed thresholds (soak-thresholds.json).
"""
from __future__ import annotations

import gzip
import hashlib
import json
import math
import shlex
import subprocess
import threading
import time
from pathlib import Path
from typing import Any

from . import agent, canon, runner, shape, t2

GUEST = t2.GUEST_DIR / "soak.py"
THRESHOLDS = t2.TOOLS_DIR / "soak-thresholds.json"
PROVIDER = agent.PROVIDER_BIN / "opute-provider-k3s"
GUEST_OUT = "/opt/parity-t2/soak"
# The heavy admission class reserves 2 GiB (remove_host_file is heavy), which
# a 2 GiB twin never fits; the larger goldens' default size does.
MEMORY_MB = t2.INCUS_MEMORY_MB
SERVE_ENV = {"OPUTE_HOST_AGENT_RELAY_DIR": "/home/ubuntu/.opute/relays",
             "OPUTE_HOST_AGENT_SQLITE_ROOT": "/home/ubuntu/.opute/databases"}
def _recipe() -> str:
    """The k3s activation recipe exactly as the T2 postgres scenarios write it:
    the provider's manifest pins its sha256, so it is read, never retyped."""
    for scenario in json.loads((t2.SCENARIO_DIR / "postgres-domain.json").read_text()):
        for step in scenario["steps"]:
            if step.get("as") == "recipe" and step.get("write", {}).get("path") == "/opt/parity-t2/recipes/activate.yaml":
                return step["write"]["content"]
    raise RuntimeError("the k3s activation recipe is missing from scenarios-t2/postgres-domain.json")


RECIPE = _recipe()
PROVIDER_ARGS = {"descriptor": {"schema": "opute-provider-plugin.v1", "pluginId": "com.opute.k3s", "version": "1.0.2",
                                "capabilities": [{"id": "opute.capability.kubernetes.v1", "version": 1}],
                                "server": {"transport": "streamable_http", "endpoint": "http://127.0.0.1:3015/mcp"}},
                 "activate": True, "recipeSource": "/opt/parity-t2/recipes/activate.yaml"}
# A reload names its descriptor by a local file: Go's handleProviderReload
# refuses an inline descriptor object ("provider reload requires descriptor
# source"), and so does Rust.
DESCRIPTOR = "/opt/parity-t2/recipes/descriptor.json"
RELOAD_ARGS = {"source": DESCRIPTOR, "activate": True, "recipeSource": PROVIDER_ARGS["recipeSource"]}
DRIVER_FILES = [Path(__file__), GUEST, t2.GUEST_DIR / "driver.py", t2.GUEST_DIR / "snapshot.py"]


def driver_sha256() -> str:
    digest = hashlib.sha256()
    for path in DRIVER_FILES:
        digest.update(path.name.encode() + b"\0" + path.read_bytes() + b"\0")
    return digest.hexdigest()


def _ssh(twin: t2.Twin, command: str, stdin: str | None = None, timeout: float = 120) -> subprocess.CompletedProcess:
    return subprocess.run(t2._ssh_base(twin.port) + [command], input=stdin, capture_output=True, text=True,
                          timeout=timeout)


def _scp_to(twin: t2.Twin, files: list[Path], target: str) -> None:
    scp = ["scp", "-q"] + [a for a in t2._ssh_base(twin.port)[1:-3] if a != "-p"] + ["-P", str(twin.port)]
    subprocess.run(scp + [str(f) for f in files] + [f"root@127.0.0.1:{target}"], check=True, timeout=300)


def _mirror(twin: t2.Twin, local: Path) -> None:
    """Copy the guest's records (a tar of the out directory) into local."""
    local.mkdir(parents=True, exist_ok=True)
    done = subprocess.run(t2._ssh_base(twin.port) + [f"tar -C {GUEST_OUT} -cf - ."], capture_output=True,
                          timeout=900)
    if done.returncode == 0:
        subprocess.run(["tar", "-C", str(local), "-xf", "-"], input=done.stdout, check=True)


class Side(threading.Thread):
    """One twin for the whole run, mirrored every pollSec."""

    def __init__(self, label: str, impl: agent.Impl, run_dir: Path, config: dict, poll: float,
                 failed: threading.Event):
        super().__init__(name=f"soak-{label}", daemon=True)
        self.label, self.impl, self.run_dir, self.config, self.poll = label, impl, run_dir, config, poll
        self.failed = failed
        self.local = run_dir / label
        self.outcome: dict[str, Any] = {"label": label, "binary": impl.label}

    def run(self) -> None:
        try:
            with t2.Twin(self.label, self.impl.binary, self.run_dir, memory_mb=MEMORY_MB) as twin:
                self.outcome["qemuArgv"] = twin.record.get("qemuArgv")
                _scp_to(twin, [GUEST, PROVIDER], "/opt/parity-t2/")
                setup = _ssh(twin, "install -d -o ubuntu -g ubuntu /opt/parity-t2/recipes /home/ubuntu/.opute "
                                   "&& cat > /opt/parity-t2/recipes/activate.yaml && "
                                   "chmod 0755 /opt/parity-t2/opute-provider-k3s", stdin=RECIPE)
                _ssh(twin, f"cat > {DESCRIPTOR}", stdin=json.dumps(PROVIDER_ARGS["descriptor"]))
                config = {**self.config, "out": GUEST_OUT}
                _ssh(twin, "cat > /opt/parity-t2/soak-config.json", stdin=json.dumps(config))
                started = _ssh(twin, "systemd-run --unit=parity-soak --collect python3 /opt/parity-t2/soak.py "
                                     "/opt/parity-t2/soak-config.json")
                self.outcome.update(setupExit=setup.returncode, soakStarted=started.returncode == 0,
                                    started=time.time())
                while True:
                    if self.failed.wait(self.poll):
                        self.outcome["stoppedBecause"] = "other side failed"
                        break
                    if twin.process.poll() is not None:
                        self.outcome["crash"] = f"twin exited ({twin.process.returncode})"
                        break
                    state = _ssh(twin, "systemctl show -p ActiveState --value parity-soak; "
                                       f"test -f {GUEST_OUT}/done.json && echo done", timeout=60).stdout.split()
                    _mirror(twin, self.local)
                    if "done" in state:
                        self.outcome["finished"] = time.time()
                        break
                    if not state or state[0] != "active":
                        self.outcome["crash"] = f"guest program not active: {state}"
                        break
                _mirror(twin, self.local)
        except Exception as e:  # recorded, never retried
            self.outcome["crash"] = f"{type(e).__name__}: {e}"
        if "crash" in self.outcome:
            self.failed.set()


def run(left: agent.Impl, right: agent.Impl, out: Path, seed: int, minutes: float, interval: float,
        restart_every: int, sample_every: float = 30, poll: float = 300) -> dict:
    """One soak (or shakedown) round; returns its summary."""
    out.mkdir(parents=True, exist_ok=False)
    work = t2.WORK / "soak" / out.name
    work.mkdir(parents=True, exist_ok=True)
    iterations = int(minutes * 60 / interval)
    config = {"seed": seed, "iterations": iterations, "intervalSec": interval, "restartEvery": restart_every,
              "sampleEverySec": sample_every, "env": t2.AGENT_ENV, "serveEnv": SERVE_ENV,
              "provider": PROVIDER_ARGS, "reload": RELOAD_ARGS}
    provenance = {"sourceLock": json.loads((runner.REPO_ROOT / "baseline/source-lock.json").read_text()),
                  "left": {"label": left.label, "binarySha256": left.sha256()},
                  "right": {"label": right.label, "binarySha256": right.sha256()},
                  "providerSha256": t2.sha256_file(PROVIDER), "golden": t2.golden_record().get("goldenSha256"),
                  "driverSha256": driver_sha256(), "harnessSha256": runner.harness_sha256(),
                  "config": config, "minutes": minutes,
                  # The thresholds in force when the round started: they are
                  # declared, and committed, before the soak.
                  "thresholdsSha256": t2.sha256_file(THRESHOLDS) if THRESHOLDS.exists() else None}
    failed = threading.Event()
    sides = [Side("a", left, work, config, poll, failed), Side("b", right, work, config, poll, failed)]
    for side in sides:
        side.start()
    for side in sides:
        side.join()
    provenance["changedDuringRun"] = (left.sha256() != provenance["left"]["binarySha256"]
                                      or right.sha256() != provenance["right"]["binarySha256"]
                                      or driver_sha256() != provenance["driverSha256"]
                                      or runner.harness_sha256() != provenance["harnessSha256"])
    files = {}
    for side in sides:
        for name in ("ops.jsonl", "samples.jsonl", "events.jsonl", "final.json"):
            source = side.local / name
            if source.exists():
                target = out / f"{side.label}-{name}.gz"
                target.write_bytes(gzip.compress(source.read_bytes(), mtime=0))
                files[target.name] = hashlib.sha256(target.read_bytes()).hexdigest()
    summary = {"provenance": provenance, "sides": [side.outcome for side in sides],
               "crashed": any("crash" in side.outcome for side in sides), "files": files}
    (out / "summary.json").write_text(json.dumps(summary, indent=1, sort_keys=True) + "\n")
    return summary


# --- evaluation ------------------------------------------------------------------------


def _jsonl(path: Path) -> list[dict]:
    return [json.loads(line) for line in gzip.decompress(path.read_bytes()).decode().splitlines() if line.strip()]


def load(bundle: Path) -> dict:
    sides = {}
    for side in ("a", "b"):
        final = bundle / f"{side}-final.json.gz"
        sides[side] = {"ops": _jsonl(bundle / f"{side}-ops.jsonl.gz"),
                       "samples": _jsonl(bundle / f"{side}-samples.jsonl.gz"),
                       "events": _jsonl(bundle / f"{side}-events.jsonl.gz"),
                       "final": json.loads(gzip.decompress(final.read_bytes())) if final.exists() else None}
    return sides


def percentile(values: list[float], p: float) -> float | None:
    if not values:
        return None
    ordered = sorted(values)
    k = (len(ordered) - 1) * p
    lo, hi = math.floor(k), math.ceil(k)
    return round(ordered[lo] + (ordered[hi] - ordered[lo]) * (k - lo), 3)


def latency(ops: list[dict]) -> dict:
    """Per op: p50/p95/p99 of the call and, for tasks, of call + wait."""
    by: dict[str, dict[str, list[float]]] = {}
    for op in ops:
        entry = by.setdefault(op["op"], {"call": [], "total": []})
        entry["call"].append(op["ms"])
        entry["total"].append(op["ms"] + op.get("taskMs", 0))
    return {name: {kind: {f"p{int(p * 100)}": percentile(values, p) for p in (0.5, 0.95, 0.99)}
                   for kind, values in entry.items()} for name, entry in sorted(by.items())}


def slope_per_hour(points: list[tuple[float, float]]) -> float | None:
    """Least-squares slope of value over time (seconds), per hour."""
    if len(points) < 3:
        return None
    n = len(points)
    mx = sum(t for t, _ in points) / n
    my = sum(v for _, v in points) / n
    den = sum((t - mx) ** 2 for t, _ in points)
    return None if den == 0 else round(sum((t - mx) * (v - my) for t, v in points) / den * 3600, 3)


def growth(samples: list[dict], events: list[dict]) -> dict:
    """Leak indicators that survive restarts: the slope of each epoch's last
    sample (between restarts) across the run, for RSS, FDs and threads, and
    the state size slope."""
    restarts = sorted(e["t"] for e in events if e.get("event") == "restart")
    epochs: list[list[dict]] = [[]]
    bound = iter(restarts + [math.inf])
    edge = next(bound)
    for sample in samples:
        while sample["t"] >= edge:
            epochs.append([])
            edge = next(bound)
        if not sample.get("down"):
            epochs[-1].append(sample)
    out: dict[str, Any] = {"epochs": len([e for e in epochs if e])}
    for key in ("rssKb", "fds", "threads"):
        lasts = [(e[-1]["t"], e[-1][key]) for e in epochs if e and key in e[-1]]
        within = [slope_per_hour([(s["t"], s[key]) for s in e]) for e in epochs if len(e) >= 3]
        within = [w for w in within if w is not None]
        out[key] = {"epochEndSlopePerHour": slope_per_hour(lasts), "max": max((v for _, v in lasts), default=None),
                    "medianWithinEpochSlopePerHour": percentile(within, 0.5)}
    out["stateBytes"] = {"slopePerHour": slope_per_hour([(s["t"], s["stateBytes"]) for s in samples]),
                         "last": samples[-1]["stateBytes"] if samples else None}
    out["downSamples"] = sum(1 for s in samples if s.get("down"))
    return out


_RANDOM_ID = __import__("re").compile(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}|[0-9a-f]{16,}")


def reservation_masks(document: dict) -> list[dict]:
    """M5 shape's rule: a reservation id other than "control" carries host
    wall-clock nanoseconds and a per-process sequence."""
    path = ["sqliteRows", "state/state.db", "tables", "capability_invocations"]
    rows = runner._lookup(document, path)
    masks = []
    for key, row in (enumerate(rows) if isinstance(rows, list) else (rows or {}).items()):
        try:
            reservation = json.loads(row["binding_json"]).get("reservationId")
        except (KeyError, TypeError, ValueError):
            reservation = None
        if reservation not in (None, "control"):
            masks.append({"path": path + [key, "binding_json", "$json", "reservationId"], "type": "host-reservation-id",
                          "reason": "host reservation wall-clock nanoseconds and per-process sequence"})
    return masks


LIVE = {"$masked": "live-host-readings"}


def mask_live_readings(document: dict) -> int:
    """get_host_info's `system` object is a snapshot of each twin's own VM at
    that instant: memory, disk, PSI, cgroup, load and admission readings
    (load averages are even left out when they read 0.00). Two Go twins
    differ there, so it is a reading of the VM, not of the implementation.
    It is kept in the stored invocation (observation and result) and in a
    plan's state (the node's output and the plan's outputs). Its presence is
    compared, its values are not; return how many objects were masked."""
    tables = runner._lookup(document, ["sqliteRows", "state/state.db", "tables"]) or {}
    holders = []
    for row in _rows(tables.get("capability_invocations")):
        if row.get("operation_id") == "get_host_info":
            holders += [runner._lookup(row, [column, "$json", "structured"])
                        for column in ("observation_json", "result_json")]
    for row in _rows(tables.get("plan_runs")):
        state = runner._lookup(row, ["state_json", "$json"]) or {}
        holders += list((state.get("outputs") or {}).values())
        holders += [node.get("output") for node in (state.get("nodes") or {}).values() if isinstance(node, dict)]
    masked = 0
    for holder in holders:
        if isinstance(holder, dict) and isinstance(holder.get("system"), dict):
            holder["system"] = dict(LIVE)
            masked += 1
    return masked


def _rows(rows: Any) -> list:
    return list(rows) if isinstance(rows, list) else list((rows or {}).values())


REVISION_KEYS = ("catalog_revision", "catalogRevision")
_REVISION = __import__("re").compile(r"sha256:[0-9a-f]{64}")


def pair_reload_revisions(a: Any, b: Any) -> tuple[list[dict], list[str]]:
    """Every provider reload activates a new generation, and each generation
    publishes its own catalog revision. D19 maps only the revisions pinned in
    parity-manifest.json (the served catalogs and the first k3s generation);
    the soak's later generations are not pinned. At each catalog-revision
    leaf where Go and Rust still differ, the pair must be one-to-one across
    the whole state (one Go revision always stands opposite the same Rust
    revision, and the reverse), and no pinned Go revision may appear in it.
    Rust's value is then rewritten to Go's. Returns the pairs and any
    problem; a problem fails final-state parity."""
    registry = canon.load_divergences(runner.DIVERGENCE_FILE)
    pinned_go = set(canon._rename_tables(registry["D19.neutral-host-tool-names"])[2].values())
    forward: dict[str, str] = {}
    backward: dict[str, str] = {}
    problems: list[str] = []

    def walk(x: Any, y: Any) -> None:
        if isinstance(x, dict) and isinstance(y, dict):
            for k in x:
                if k in y:
                    walk(x[k], y[k])
                    if k in REVISION_KEYS and isinstance(x[k], str) and isinstance(y[k], str) and x[k] != y[k] \
                            and _REVISION.fullmatch(x[k]) and _REVISION.fullmatch(y[k]):
                        go, rust = x[k], y[k]
                        if go in pinned_go:
                            problems.append(f"pinned Go revision {go} opposite unmapped {rust}")
                        elif forward.setdefault(go, rust) != rust or backward.setdefault(rust, go) != go:
                            problems.append(f"revision pairing is not one-to-one at {go} / {rust}")
                        else:
                            y[k] = go
        elif isinstance(x, list) and isinstance(y, list):
            for u, v in zip(x, y):
                walk(u, v)

    walk(a, b)
    return [{"go": go, "rust": rust} for go, rust in forward.items()], sorted(set(problems))


def soak_spec() -> dict:
    """M5 shape's durable-state spec plus the soak's plan-node rules: a node's
    start and finish are wall-clock times, and get_host_info's node output
    carries supportedTools in Go map order (inventory finding F-2), as the
    plan's outputs do."""
    root = ["sqliteRows", "state/state.db", "tables", "plan_runs", "*", "state_json", "$json", "nodes", "*"]
    spec = shape.compare_spec()
    spec["masks"] += [{"path": root + [column], "type": "rfc3339", "reason": "plan node wall-clock time",
                       "optional": True} for column in ("startedAt", "completedAt")]
    spec["masks"].append({"path": ["sqliteRows", "state/state.db", "tables", "provider_generations", "*", "active_at"],
                          "type": "rfc3339", "reason": "provider activation wall-clock time", "optional": True})
    spec["sets"].append(root + ["output", "supportedTools"])
    return spec


def align_rows(databases: dict, variables: dict) -> None:
    """A table keyed by ids no operation reported (invocation, generation and
    plan-node ids are random per side) becomes a list in insertion order,
    and each such id a per-row variable, so its other occurrences compare
    too. Tables keyed by captured task ids stay keyed. Insertion order is
    the workload's order: both twins run the same operations one by one."""
    captured = set(variables.values())
    for name, database in databases.items():
        for table, rows in database.get("tables", {}).items():
            if isinstance(rows, dict) and rows and not set(rows) <= captured:
                ordered = sorted(rows.items(), key=lambda kv: kv[1].get("_rowid", 0))
                for index, (key, _) in enumerate(ordered):
                    if key not in captured and _RANDOM_ID.fullmatch(key):
                        variables[f"SOAK_ROW_{table}_{index}"] = key
                rows = [row for _, row in ordered]
            if isinstance(rows, dict):
                rows = {k: {c: v for c, v in r.items() if c != "_rowid"} for k, r in rows.items()}
            else:
                rows = [{c: v for c, v in r.items() if c != "_rowid"} for r in rows]
            database["tables"][table] = rows


def final_state(sides: dict) -> dict:
    """Go's and Rust's final state.db rows after M5 shape's masks, with task
    ids replaced by their iteration and D19 names mapped back to Go's."""
    normalized, violations = {}, {}
    for side in ("a", "b"):
        final = sides[side]["final"] or {}
        document = {"sqliteRows": {k: v for k, v in (final.get("sqliteRows") or {}).items()
                                   if k == "state/state.db"}}
        variables = {f"SOAK_TASK_{op['i']}": op["id"] for op in sides[side]["ops"] if op.get("id")}
        install = next((e.get("install", {}).get("id") for e in sides[side]["events"] if e.get("event") == "setup"),
                       None)
        if install:
            variables["SOAK_TASK_INSTALL"] = install
        align_rows(document["sqliteRows"], variables)
        spec = soak_spec()
        spec["masks"] += reservation_masks(document)
        normalized[side], violations[side] = canon.normalize(document, spec, variables)
    registry = canon.load_divergences(runner.DIVERGENCE_FILE)
    a, b, renamed = canon.apply_divergences(normalized["a"], normalized["b"], ["D19.neutral-host-tool-names"],
                                            registry)
    divergences = [r for r in renamed if not r["stale"]]
    if divergences:
        normalized["a"], normalized["b"] = a, b
    # D13: Rust stores open structured leaves as "[redacted]". Rows are
    # aligned by insertion order, so the positional drop pairs the same rows.
    a, b, redacted = canon.apply_divergences(normalized["a"], normalized["b"], ["D13.redacted-structured-content"],
                                             registry)
    if [r for r in redacted if not r["stale"]]:
        normalized["a"], normalized["b"] = a, b
        divergences += [r for r in redacted if not r["stale"]]
    live = sum(mask_live_readings(normalized[side]) for side in ("a", "b"))
    pairs, revision_problems = pair_reload_revisions(normalized["a"], normalized["b"])
    diff = canon.diff(normalized["a"], normalized["b"])
    return {"diffCount": len(diff), "diff": diff[:50], "divergences": sorted({r["id"] for r in divergences}),
            "liveReadingsMasked": live, "reloadRevisionPairs": pairs, "revisionProblems": revision_problems,
            "violations": {k: v[:20] for k, v in violations.items()}}


def evaluate(bundle: Path, thresholds: dict | None) -> dict:
    """Re-derive a round's results from its raw receipts; without
    thresholds (a shakedown that declares them) the checks are left out."""
    sides = load(bundle)
    a, b = sides["a"], sides["b"]
    result: dict[str, Any] = {}
    # Every iteration's outcome class must match, in order.
    mismatches = [{"i": x["i"], "op": x["op"], "a": x.get("class"), "b": y.get("class")}
                  for x, y in zip(a["ops"], b["ops"]) if x.get("class") != y.get("class") or x["op"] != y["op"]]
    result["iterations"] = {"a": len(a["ops"]), "b": len(b["ops"])}
    result["classMismatches"] = {"count": len(mismatches), "first": mismatches[:50]}

    def histogram(ops: list[dict]) -> dict:
        out: dict[str, dict[str, int]] = {}
        for op in ops:
            out.setdefault(op["op"], {}).setdefault(op.get("class", "?"), 0)
            out[op["op"]][op.get("class", "?")] += 1
        return out

    result["classes"] = {"a": histogram(a["ops"]), "b": histogram(b["ops"])}
    result["latency"] = {"a": latency(a["ops"]), "b": latency(b["ops"])}
    result["growth"] = {"a": growth(a["samples"], a["events"]), "b": growth(b["samples"], b["events"])}
    result["restarts"] = {side: [{k: e[k] for k in ("kind", "started", "ready")}
                                 for e in sides[side]["events"] if e.get("event") == "restart"] for side in sides}
    result["journal"] = {side: {"levels": (sides[side]["final"] or {}).get("journalLevels"),
                                "errors": len((sides[side]["final"] or {}).get("journalErrors") or [])}
                         for side in sides}
    result["finalState"] = final_state(sides)
    if thresholds is None:
        return result
    result["checks"] = checks(result, thresholds)
    result["pass"] = all(c["pass"] for c in result["checks"].values())
    return result


def derive_thresholds(shakedown: dict, source: str) -> dict:
    """Thresholds from a Go/Go shakedown's raw evaluation: the spread two
    identical Go twins show is the noise floor, and Rust may differ from Go
    by twice that spread, never by less than the stated floors.

    - latency: maxRatio[p] = max(floor, 2 x the largest b/a ratio seen at p
      over every op and kind); slackMs = max(5, 2 x the largest |b - a| at
      p50);
    - growth: slopeSlackPerHour = max(floor, 2 x |b - a| epoch-end slope);
      stateBytes maxRatio = 2 and slopeSlackPerHour likewise;
    - downSampleSlack = max(1, 2 x |b - a| down samples).
    """
    lat_a, lat_b = shakedown["latency"]["a"], shakedown["latency"]["b"]
    ratios: dict[str, float] = {"p50": 1.0, "p95": 1.0, "p99": 1.0}
    spread = 0.0
    for op, kinds in lat_b.items():
        for kind, ps in kinds.items():
            for p, value in ps.items():
                base = lat_a.get(op, {}).get(kind, {}).get(p)
                if base and value is not None:
                    ratios[p] = max(ratios[p], value / base, base / value if value else 1.0)
                    if p == "p50":
                        spread = max(spread, abs(value - base))
    floors = {"p50": 1.5, "p95": 2.0, "p99": 3.0}
    growth_a, growth_b = shakedown["growth"]["a"], shakedown["growth"]["b"]

    def slack(key: str, floor: float) -> float:
        sa, sb = growth_a[key]["epochEndSlopePerHour"], growth_b[key]["epochEndSlopePerHour"]
        return round(max(floor, 2 * abs((sb or 0) - (sa or 0))), 3)

    state_a = growth_a["stateBytes"]["slopePerHour"] or 0
    state_b = growth_b["stateBytes"]["slopePerHour"] or 0
    return {
        "schemaVersion": "opute-soak-thresholds.v1",
        "derivedFrom": source,
        "rule": derive_thresholds.__doc__.strip(),
        "latency": {"maxRatio": {p: round(max(floors[p], 2 * ratios[p]), 2) for p in ratios},
                    "slackMs": round(max(5.0, 2 * spread), 1)},
        "growth": {"rssKb": {"slopeSlackPerHour": slack("rssKb", 2048)},
                   "fds": {"slopeSlackPerHour": slack("fds", 8)},
                   "threads": {"slopeSlackPerHour": slack("threads", 4)},
                   "stateBytes": {"maxRatio": 2.0,
                                  "slopeSlackPerHour": round(max(65536, 2 * abs(state_b - state_a)), 1)}},
        "downSampleSlack": max(1, 2 * abs(growth_b["downSamples"] - growth_a["downSamples"])),
    }


def checks(result: dict, thresholds: dict) -> dict:
    out: dict[str, dict] = {}
    out["sameIterations"] = {"pass": result["iterations"]["a"] == result["iterations"]["b"] > 0}
    out["classesEqual"] = {"pass": result["classMismatches"]["count"] == 0
                           and result["classes"]["a"] == result["classes"]["b"]}
    out["restartsEqual"] = {"pass": result["restarts"]["a"] == result["restarts"]["b"]
                            and all(r["started"] and r["ready"] for r in result["restarts"]["b"])}
    lat = thresholds["latency"]
    worse = []
    for op, kinds in result["latency"]["b"].items():
        for kind, ps in kinds.items():
            for p, value in ps.items():
                base = result["latency"]["a"].get(op, {}).get(kind, {}).get(p)
                if base is None or value is None:
                    continue
                if value > base * lat["maxRatio"][p] + lat["slackMs"]:
                    worse.append({"op": op, "kind": kind, "p": p, "a": base, "b": value})
    out["latencyWithinBounds"] = {"pass": not worse, "exceeded": worse}
    grow = thresholds["growth"]
    over = []
    for key in ("rssKb", "fds", "threads"):
        ga, gb = result["growth"]["a"][key], result["growth"]["b"][key]
        sa, sb = ga["epochEndSlopePerHour"], gb["epochEndSlopePerHour"]
        if sb is not None and sa is not None and sb > max(sa, 0) + grow[key]["slopeSlackPerHour"]:
            over.append({"key": key, "a": sa, "b": sb})
    sa, sb = result["growth"]["a"]["stateBytes"]["slopePerHour"], result["growth"]["b"]["stateBytes"]["slopePerHour"]
    if sa is not None and sb is not None and sb > max(sa, 0) * grow["stateBytes"]["maxRatio"] + \
            grow["stateBytes"]["slopeSlackPerHour"]:
        over.append({"key": "stateBytes", "a": sa, "b": sb})
    out["growthWithinBounds"] = {"pass": not over, "exceeded": over}
    out["noDownSamplesOutsideRestarts"] = {"pass": result["growth"]["b"]["downSamples"]
                                           <= result["growth"]["a"]["downSamples"] + thresholds["downSampleSlack"]}
    ja, jb = result["journal"]["a"], result["journal"]["b"]
    out["journalErrors"] = {"pass": jb["errors"] <= ja["errors"], "a": ja["errors"], "b": jb["errors"]}
    out["finalStateParity"] = {"pass": result["finalState"]["diffCount"] == 0
                               and not any(result["finalState"]["violations"].values())
                               and not result["finalState"].get("revisionProblems")}
    return out
