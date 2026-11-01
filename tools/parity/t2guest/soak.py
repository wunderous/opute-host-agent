"""M12 soak guest program: one seeded mixed workload against the agent in this twin.

    python3 soak.py CONFIG.json

Runs detached (the host starts it as a transient unit) and writes, under
config["out"]:

  ops.jsonl      one line per iteration: op, outcome class, latencies, ids
  samples.jsonl  agent RSS, threads, open FDs and state size, every sampleEverySec
  events.jsonl   setup, restarts (SIGTERM or SIGKILL) and readiness
  progress.json  the last finished iteration (the host's heartbeat)
  final.json     journal error lines and every SQLite row, after the last stop
  done.json      written last

The schedule is a pure function of the seed and the iteration index, so the
Go and Rust twins run the same operations in the same order. Iteration i
starts at t0 + i * intervalSec, or at once when the previous one ran late,
so both twins run exactly the same number of iterations.

Nothing here talks to anything outside the VM.
"""
from __future__ import annotations

import json
import os
import random
import re
import sqlite3
import subprocess
import sys
import threading
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import driver  # noqa: E402

UNIT = driver.UNIT
TERMINAL = ("completed", "failed", "cancelled")
# Weights of the mixed workload (read-only, task-aware mutations, input-required
# tasks, plans and provider replacement).
OPS = [
    ("tools_list", 12), ("host_info", 12), ("provider_status", 6),
    ("file_ensure", 10), ("file_remove", 6),
    ("sqlite_ensure", 6), ("sqlite_status", 6), ("sqlite_remove", 4),
    ("input_answer", 6), ("input_cancel", 4),
    ("plan", 8), ("provider_reload", 2),
]


def schedule(seed: int, iterations: int) -> list[str]:
    rng = random.Random(seed)
    names = [name for name, _ in OPS]
    weights = [weight for _, weight in OPS]
    return rng.choices(names, weights=weights, k=iterations)


class Soak:
    def __init__(self, config: dict):
        self.config = config
        self.agent = {"env": config["env"]}
        self.port = int(config["env"]["HOST_MCP_PORT"])
        self.token = config["env"]["MCP_AUTH_TOKEN"]
        self.out = config["out"]
        os.makedirs(self.out, exist_ok=True)
        self.files = {name: open(os.path.join(self.out, name), "a", buffering=1)
                      for name in ("ops.jsonl", "samples.jsonl", "events.jsonl")}
        self.t0 = time.monotonic()
        self.iteration = -1
        self.stop_sampling = threading.Event()
        self.lock = threading.Lock()

    # --- records ------------------------------------------------------------

    def write(self, name: str, record: dict) -> None:
        record = {"t": round(time.monotonic() - self.t0, 3), **record}
        with self.lock:
            self.files[name].write(json.dumps(record, sort_keys=True) + "\n")

    # --- the agent ----------------------------------------------------------

    def call(self, method: str, params: dict | None = None, name: str | None = None) -> tuple[dict, float]:
        started = time.monotonic()
        try:
            result = driver.mcp(self.port, self.token, method, params, name, timeout=600)
        except Exception as e:  # recorded as a transport outcome
            result = {"status": None, "body": None, "transport": type(e).__name__}
        return result, round((time.monotonic() - started) * 1000, 3)

    def tool(self, name: str, arguments: dict) -> tuple[dict, float]:
        return self.call("tools/call", {"name": name, "arguments": arguments}, name)

    def until_terminal(self, task_id: str, accept=TERMINAL, timeout: float = 600) -> tuple[dict, float]:
        started = time.monotonic()
        while True:
            got, _ = self.call("tasks/get", {"taskId": task_id}, task_id)
            status = driver.lookup(got, ["body", "result", "status"])
            if status in accept or time.monotonic() - started > timeout or not isinstance(got.get("body"), dict) \
                    or "error" in got["body"]:
                return got, round((time.monotonic() - started) * 1000, 3)
            time.sleep(0.25)

    @staticmethod
    def classify(result: dict) -> str:
        if result.get("transport"):
            return "transport:" + result["transport"]
        body = result.get("body")
        if not isinstance(body, dict):
            return f"http:{result.get('status')}"
        if "error" in body:
            return f"rpc:{(body['error'] or {}).get('code')}"
        res = body.get("result") or {}
        if res.get("isError"):
            return "tool_error:" + str((res.get("structuredContent") or {}).get("code"))
        if "status" in res and "taskId" in res:
            inner = res.get("result") or {}
            suffix = ""
            if inner.get("isError"):
                suffix = ":" + str((inner.get("structuredContent") or {}).get("code"))
            return f"task:{res['status']}{suffix}"
        if "tools" in res:
            return f"ok:{len(res['tools'])}"
        return "ok"

    @staticmethod
    def detail(result: dict) -> str | None:
        """The first text of an error answer, for diagnosis only (not compared)."""
        body = result.get("body")
        if not isinstance(body, dict):
            return None
        if "error" in body:
            return str((body["error"] or {}).get("message"))[:300]
        res = body.get("result") or {}
        res = res.get("result") if "taskId" in res and isinstance(res.get("result"), dict) else res
        if res.get("isError"):
            text = [c.get("text") for c in res.get("content") or [] if isinstance(c, dict)]
            return (text[0] if text else "")[:300]
        return None

    def sync(self, record: dict, result: dict, latency: float) -> None:
        record.update(ms=latency, **{"class": self.classify(result)})
        if self.detail(result):
            record["detail"] = self.detail(result)

    def task_op(self, result: dict, latency: float, record: dict) -> None:
        task_id = driver.lookup(result, ["body", "result", "taskId"]) \
            or driver.lookup(result, ["body", "result", "structuredContent", "runId"])
        record["accepted"] = self.classify(result)
        if isinstance(task_id, str) and task_id:
            record["id"] = task_id
            final, waited = self.until_terminal(task_id)
            record["class"] = self.classify(final)
            record["taskMs"] = waited
            if self.detail(final):
                record["detail"] = self.detail(final)
        else:
            record["class"] = record["accepted"]
            if self.detail(result):
                record["detail"] = self.detail(result)
        record["ms"] = latency

    def serve(self) -> dict:
        return driver.serve({"env": self.config.get("serveEnv", {})}, self.agent)

    def restart(self, kind: str) -> None:
        before = self.main_pid()
        started = time.monotonic()
        if kind == "sigkill":
            driver.run(["systemctl", "kill", "-s", "KILL", UNIT])
            for _ in range(200):
                if driver.run(["systemctl", "is-active", "--quiet", UNIT])["exit"] != 0:
                    break
                time.sleep(0.1)
            stopped = {"exit": None}
        else:
            stopped = driver.stop()
        stop_ms = round((time.monotonic() - started) * 1000, 3)
        driver.run(["systemctl", "reset-failed", UNIT])
        served = self.serve()
        self.write("events.jsonl", {"event": "restart", "kind": kind, "iteration": self.iteration,
                                    "pidBefore": before, "stopExit": stopped.get("exit"), "stopMs": stop_ms,
                                    "started": served["started"], "ready": served["ready"],
                                    "readyMs": round((time.monotonic() - started) * 1000, 3)})

    def main_pid(self) -> int:
        out = driver.run(["systemctl", "show", "-p", "MainPID", "--value", UNIT])["stdout"].strip()
        return int(out) if out.isdigit() else 0

    # --- sampling -----------------------------------------------------------

    def sample_loop(self) -> None:
        every = self.config["sampleEverySec"]
        state = self.config["env"]["OPUTE_STANDALONE_STATE_DIR"]
        while not self.stop_sampling.wait(every):
            pid = self.main_pid()
            record = {"iteration": self.iteration, "pid": pid}
            try:
                with open(f"/proc/{pid}/status") as fh:
                    status = fh.read()
                record["rssKb"] = int(re.search(r"VmRSS:\s+(\d+)", status).group(1))
                record["threads"] = int(re.search(r"Threads:\s+(\d+)", status).group(1))
                record["fds"] = len(os.listdir(f"/proc/{pid}/fd"))
            except (OSError, AttributeError, ValueError):
                record["down"] = True
            record["stateBytes"] = sum(os.path.getsize(os.path.join(state, f)) for f in os.listdir(state)
                                       if os.path.isfile(os.path.join(state, f))) if os.path.isdir(state) else 0
            self.write("samples.jsonl", record)

    # --- the workload -------------------------------------------------------

    def run_op(self, i: int, op: str) -> dict:
        record: dict = {"i": i, "op": op}
        slot = i % 16
        if op == "tools_list":
            result, ms = self.call("tools/list", {})
            self.sync(record, result, ms)
        elif op == "host_info":
            result, ms = self.tool("get_host_info", {})
            self.sync(record, result, ms)
        elif op == "provider_status":
            result, ms = self.tool("opute.provider.status", {})
            self.sync(record, result, ms)
        elif op == "file_ensure":
            result, ms = self.tool("ensure_host_file", {"path": f"~/soak/f{slot}.txt", "content": f"iteration {i}\n"})
            self.task_op(result, ms, record)
        elif op == "file_remove":
            result, ms = self.tool("remove_host_file", {"path": f"~/soak/f{slot}.txt", "confirm": True})
            self.task_op(result, ms, record)
        elif op in ("sqlite_ensure", "sqlite_status", "sqlite_remove"):
            name = {"sqlite_ensure": "ensure_sqlite_database", "sqlite_status": "get_sqlite_database_status",
                    "sqlite_remove": "remove_sqlite_database"}[op]
            args = {"consumerId": "soak", "databaseName": f"db{i % 4}"}
            if op == "sqlite_remove":
                args["confirm"] = True
            result, ms = self.tool(name, args)
            self.task_op(result, ms, record)
        elif op in ("input_answer", "input_cancel"):
            result, ms = self.tool("request_task_input", {"prompt": "Continue?", "responseType": "boolean"})
            task_id = driver.lookup(result, ["body", "result", "taskId"])
            record.update(ms=ms, accepted=self.classify(result))
            if isinstance(task_id, str) and task_id:
                record["id"] = task_id
                waiting, _ = self.until_terminal(task_id, accept=TERMINAL + ("input_required",), timeout=60)
                record["waiting"] = self.classify(waiting)
                if op == "input_answer":
                    self.call("tasks/update", {"taskId": task_id, "inputResponses": {"response": i % 2 == 0}},
                              task_id)
                else:
                    self.call("tasks/cancel", {"taskId": task_id}, task_id)
                final, waited = self.until_terminal(task_id)
                record.update(taskMs=waited, **{"class": self.classify(final)})
            else:
                record["class"] = record["accepted"]
        elif op == "plan":
            plan = {"contractVersion": "host-plan.v1", "planId": "soak", "generation": 1,
                    "idempotencyKey": f"soak-{i}",
                    "nodes": [{"id": "marker", "action": {"tool": "inspect_host_file",
                                                          "args": {"path": "~/soak/marker"}}},
                              {"id": "host", "dependsOn": ["marker"],
                               "action": {"tool": "get_host_info", "args": {}}}]}
            result, ms = self.tool("run_host_plan", {"plan": plan})
            self.task_op(result, ms, record)
        elif op == "provider_reload":
            result, ms = self.tool("opute.provider.reload", self.config["reload"])
            self.task_op(result, ms, record)
        return record

    def setup(self) -> None:
        driver.run(["install", "-d", "-o", "ubuntu", "-g", "ubuntu", "/home/ubuntu/soak"])
        with open("/home/ubuntu/soak/marker", "w") as fh:
            fh.write("soak\n")
        subprocess.run(["chown", "ubuntu:ubuntu", "/home/ubuntu/soak/marker"], check=True)
        provider = driver.run(["systemd-run", "--unit=parity-provider", "-p", "User=ubuntu", "-p", "Group=ubuntu",
                               "-p", "WorkingDirectory=/opt/parity-t2", "-E", "OPUTE_PROVIDER_PORT=3015",
                               "-E", "HOME=/home/ubuntu", "/opt/parity-t2/opute-provider-k3s", "serve"])
        listening = driver.run(["sh", "-c", "for i in $(seq 1 100); do ss -ltn | grep -q ':3015 ' "
                                            "&& exit 0; sleep 0.2; done; exit 1"])
        served = self.serve()
        installed, ms = self.tool("opute.provider.install", self.config["provider"])
        record: dict = {}
        self.task_op(installed, ms, record)
        self.write("events.jsonl", {"event": "setup", "provider": provider["exit"], "providerListening":
                                    listening["exit"], "started": served["started"], "ready": served["ready"],
                                    "install": record})

    def final(self) -> None:
        stopped = driver.stop()
        journal = driver.run(["journalctl", "-u", UNIT, "--no-pager", "-o", "cat"])["stdout"].splitlines()
        levels: dict[str, int] = {}
        errors = []
        for line in journal:
            match = re.search(r"\blevel=([A-Z]+)", line)
            level = match.group(1) if match else ("PANIC" if "panic" in line else "OTHER")
            levels[level] = levels.get(level, 0) + 1
            if level in ("ERROR", "PANIC") or "panicked" in line:
                errors.append(line)
        databases = {}
        for root_key in ("OPUTE_STANDALONE_STATE_DIR", "OPUTE_HOST_AGENT_INSTANCE_ROOT"):
            root = self.config["env"][root_key]
            for dirpath, _, names in os.walk(root):
                for name in sorted(names):
                    path = os.path.join(dirpath, name)
                    if name.endswith(("-wal", "-shm")):
                        continue
                    with open(path, "rb") as fh:
                        if fh.read(16) != b"SQLite format 3\x00":
                            continue
                    databases[os.path.relpath(path, os.path.dirname(root.rstrip("/")))] = rows(path)
        with open(os.path.join(self.out, "final.json"), "w") as fh:
            json.dump({"stopExit": stopped["exit"], "journalLevels": levels, "journalErrors": errors,
                       "sqliteRows": databases}, fh, sort_keys=True)

    def run(self) -> None:
        cfg = self.config
        ops = schedule(cfg["seed"], cfg["iterations"])
        self.setup()
        # The schedule starts once setup is done, so no iteration starts late.
        start = time.monotonic()
        sampler = threading.Thread(target=self.sample_loop, daemon=True)
        sampler.start()
        kinds = ("sigterm", "sigkill")
        restarts = 0
        for i, op in enumerate(ops):
            due = start + i * cfg["intervalSec"]
            delay = due - time.monotonic()
            if delay > 0:
                time.sleep(delay)
            if i and cfg["restartEvery"] and i % cfg["restartEvery"] == 0:
                self.restart(kinds[restarts % 2])
                restarts += 1
            record = self.run_op(i, op)
            self.write("ops.jsonl", record)
            self.iteration = i
            if i % 50 == 0 or i == len(ops) - 1:
                with open(os.path.join(self.out, "progress.json.tmp"), "w") as fh:
                    json.dump({"iteration": i, "of": len(ops), "t": round(time.monotonic() - self.t0, 1)}, fh)
                os.replace(os.path.join(self.out, "progress.json.tmp"), os.path.join(self.out, "progress.json"))
        self.stop_sampling.set()
        sampler.join()
        self.final()
        for fh in self.files.values():
            fh.close()
        with open(os.path.join(self.out, "done.json"), "w") as fh:
            json.dump({"iterations": len(ops), "restarts": restarts,
                       "seconds": round(time.monotonic() - self.t0, 1)}, fh)


def rows(path: str) -> dict:
    """Every row of every table, keyed by a single-column primary key (the
    host harness's sqlite_rows rule, so the same masks apply)."""
    conn = sqlite3.connect(f"file:{path}?mode=ro", uri=True)
    conn.row_factory = sqlite3.Row
    try:
        version = conn.execute("PRAGMA user_version").fetchone()[0]
        tables = {}
        for (name,) in conn.execute("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name"):
            if name.startswith("sqlite_"):
                continue
            quoted = name.replace('"', '""')
            try:
                # Insertion order, so the host can align rows whose keys are
                # per-side random ids no operation reported.
                data = [dict(row) for row in conn.execute(f'SELECT rowid AS _rowid, * FROM "{quoted}" ORDER BY rowid')]
            except sqlite3.OperationalError:  # WITHOUT ROWID
                data = [dict(row) for row in conn.execute(f'SELECT * FROM "{quoted}"')]
            pk = [r[1] for r in conn.execute(f'PRAGMA table_info("{quoted}")') if r[5] > 0]
            if len(pk) == 1 and all(pk[0] in r for r in data):
                tables[name] = {str(r[pk[0]]): r for r in data}
            else:
                tables[name] = data
        return {"userVersion": version, "tables": tables}
    finally:
        conn.close()


if __name__ == "__main__":
    with open(sys.argv[1]) as fh:
        Soak(json.load(fh)).run()
