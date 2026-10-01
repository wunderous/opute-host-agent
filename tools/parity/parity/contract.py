"""Single-implementation contract suites for declared divergences.

A declared divergence (milestones.md decision table, for example D8) removes
content from the Go-vs-Rust comparison, so the diverging behaviour needs its
own oracle. A contract suite (tools/parity/contracts/<suite>.json) runs each
scenario against one implementation in a fresh sandbox and asserts the
specified outcome: status, headers, JSON fields, CLI output, files and modes,
store rows, and log content.

Steps may `capture` values (a provisioned secret, a user code, an
authorization code) into ${NAME} variables for later steps. An `expect` that
references an undefined variable fails rather than passing vacuously.

Rust canaries (tools/parity/rust-canaries.json) apply one source patch to a
scratch copy of the crate, build it, and must turn their named contract
scenario red, and only the scenarios they list in mayAlsoFail with it.
"""

from __future__ import annotations

import base64
import gzip
import hashlib
import http.client
import json
import os
import re
import shutil
import sqlite3
import stat
import subprocess
import tempfile
import time
import urllib.parse
from pathlib import Path
from typing import Any

from . import agent, canon, runner

CONTRACT_DIR = runner.TOOLS_DIR / "contracts"
RUST_CANARY_FILE = runner.TOOLS_DIR / "rust-canaries.json"
_VAR = re.compile(r"\$\{([A-Za-z_][A-Za-z0-9_]*)\}")


def contract_file(suite: str) -> Path:
    return CONTRACT_DIR / f"{suite}.json"


def contract_sha256(suite: str) -> str:
    """Hash of everything that decides a contract outcome except the binary."""
    digest = hashlib.sha256()
    for path in (Path(__file__), runner.TOOLS_DIR / "parity" / "agent.py", contract_file(suite)):
        digest.update(path.name.encode() + b"\0" + path.read_bytes() + b"\0")
    return digest.hexdigest()


def load(suite: str) -> dict:
    doc = json.loads(contract_file(suite).read_text())
    seen = set()
    for scenario in doc["scenarios"]:
        for key in ("id", "requirement", "specScenario", "steps"):
            if not scenario.get(key):
                raise ValueError(f"{suite}: scenario {scenario.get('id')!r} has no {key}")
        if scenario["id"] in seen:
            raise ValueError(f"{suite}: duplicate scenario {scenario['id']}")
        seen.add(scenario["id"])
    return doc


# --- execution -------------------------------------------------------------------


class Context:
    def __init__(self, sandbox: agent.Sandbox) -> None:
        self.sandbox = sandbox
        self.vars: dict[str, str] = dict(sandbox.variables)

    def expand(self, value: Any, strict: bool = False) -> Any:
        if isinstance(value, str):
            def sub(m: re.Match) -> str:
                if m.group(1) in self.vars:
                    return self.vars[m.group(1)]
                if strict:
                    raise KeyError(m.group(1))
                return m.group(0)
            return _VAR.sub(sub, value)
        if isinstance(value, list):
            return [self.expand(v, strict) for v in value]
        if isinstance(value, dict):
            return {k: self.expand(v, strict) for k, v in value.items()}
        return value


def _http(ctx: Context, spec: dict) -> dict:
    headers = dict(ctx.expand(spec.get("headers", {})))
    body = None
    if "form" in spec:
        body = urllib.parse.urlencode(ctx.expand(spec["form"])).encode()
        headers.setdefault("Content-Type", "application/x-www-form-urlencoded")
    elif "json" in spec:
        body = json.dumps(ctx.expand(spec["json"])).encode()
        headers.setdefault("Content-Type", "application/json")
    elif "body" in spec:
        body = ctx.expand(spec["body"]).encode()
    if "basic" in spec:
        user, password = ctx.expand(spec["basic"])
        token = base64.b64encode(f"{urllib.parse.quote(user)}:{urllib.parse.quote(password)}".encode())
        headers["Authorization"] = "Basic " + token.decode()
    path = spec["path"]
    if "query" in spec:
        path += "?" + urllib.parse.urlencode(ctx.expand(spec["query"]))
    conn = http.client.HTTPConnection("127.0.0.1", ctx.sandbox.port, timeout=30)
    try:
        conn.request(spec.get("method", "GET"), ctx.expand(path), body=body, headers=headers)
        resp = conn.getresponse()
        raw = resp.read()
    finally:
        conn.close()
    text = raw.decode(errors="replace")
    try:
        parsed = json.loads(text)
    except ValueError:
        parsed = None
    hdrs = {k.lower(): v for k, v in resp.getheaders()}
    out = {"status": resp.status, "headers": hdrs, "text": text, "json": parsed}
    if "location" in hdrs:
        parts = urllib.parse.urlsplit(hdrs["location"])
        out["redirect"] = {"base": urllib.parse.urlunsplit(parts._replace(query="", fragment="")),
                           "query": {k: v[0] for k, v in urllib.parse.parse_qs(parts.query).items()}}
    return out


def _query(ctx: Context, spec: dict) -> dict:
    path = Path(ctx.expand(spec.get("db", "${SANDBOX}/state/authz.sqlite")))
    conn = sqlite3.connect(f"file:{path}?mode=ro", uri=True)
    try:
        rows = conn.execute(ctx.expand(spec["sql"])).fetchall()
    finally:
        conn.close()
    rows = [list(r) for r in rows]
    return {"rows": rows, "count": len(rows), "value": rows[0][0] if rows and rows[0] else None}


def _file(ctx: Context, spec: dict) -> dict:
    path = Path(ctx.expand(spec["path"]))
    if not path.exists():
        return {"exists": False}
    st = path.stat()
    out: dict[str, Any] = {"exists": True, "mode": oct(stat.S_IMODE(st.st_mode)),
                           "type": "dir" if stat.S_ISDIR(st.st_mode) else "file"}
    if out["type"] == "file":
        text = path.read_text(errors="replace")
        out["text"] = text
        try:
            out["json"] = json.loads(text)
        except ValueError:
            out["json"] = None
    return out


def execute(impl: agent.Impl, scenario: dict, run_id: str) -> tuple[dict, list[str]]:
    sandbox = agent.Sandbox(side="c", run_id=run_id, fixture=runner.FIXTURE_DIR / "shims" / "incus-empty.json")
    ctx = Context(sandbox)
    steps: dict[str, Any] = {}
    failures: list[str] = []
    server: agent.Server | None = None
    try:
        for index, step in enumerate(scenario["steps"]):
            label = step.get("as", f"step{index}")
            try:
                if "serve" in step:
                    spec = step["serve"]
                    env = dict(runner.PROFILES[spec.get("profile", "standalone")])
                    env.update(ctx.expand(spec.get("env", {})))
                    server = agent.Server(impl, sandbox, spec.get("argv", ["serve"]), env)
                    result = server.wait_ready(spec.get("timeout", 30))
                elif "stop" in step:
                    result = server.stop() if server else {"exit": None}
                    server = None
                elif "http" in step:
                    result = _http(ctx, step["http"])
                elif "cli" in step:
                    spec = step["cli"]
                    env = dict(runner.PROFILES[spec.get("profile", "standalone")])
                    env.update(ctx.expand(spec.get("env", {})))
                    result = agent.run_cli(impl, sandbox, ctx.expand(spec["argv"]), env)
                elif "sql" in step:
                    spec = step["sql"]
                    result = agent.run_sql(Path(ctx.expand(spec.get("db", "${SANDBOX}/state/authz.sqlite"))),
                                           ctx.expand(spec["statements"]))
                elif "query" in step:
                    result = _query(ctx, step["query"])
                elif "file" in step:
                    result = _file(ctx, step["file"])
                elif "log" in step:
                    log = sandbox.root / "server.log"
                    time.sleep(0.05)  # let the agent flush its last line
                    result = {"text": agent._strip_log_prefix(log.read_text(errors="replace")) if log.exists() else ""}
                elif "repeat" in step:
                    spec = step["repeat"]
                    results = [_http(ctx, spec["http"]) for _ in range(spec["times"])]
                    result = {"statuses": [r["status"] for r in results], "last": results[-1]}
                else:
                    raise ValueError(f"{scenario['id']}: unknown step {step}")
            except (OSError, sqlite3.Error, KeyError, ValueError, http.client.HTTPException) as exc:
                # A step that cannot run fails this scenario, not the suite.
                steps[label] = {"error": f"{type(exc).__name__}: {exc}"}
                failures.append(f"{label}: step error: {type(exc).__name__}: {exc}")
                break
            steps[label] = result
            for name, source in step.get("capture", {}).items():
                value = _resolve(result, source["from"])
                if value is _MISSING:
                    value = None
                if value is not None and "regex" in source:
                    m = re.search(source["regex"], str(value))
                    value = (m.group(1) if m.groups() else m.group(0)) if m else None
                if value is None:
                    failures.append(f"{label}: capture {name} from {source['from']} found nothing")
                else:
                    ctx.vars[name] = str(value)
            for path, matcher in step.get("expect", {}).items():
                try:
                    want = ctx.expand(matcher, strict=True)
                except KeyError as exc:
                    failures.append(f"{label}: expect {path} uses undefined ${{{exc.args[0]}}}")
                    continue
                problem = _check(_resolve(result, path), want)
                if problem:
                    failures.append(f"{label}: {path} {problem}")
    finally:
        if server is not None:
            steps["_implicitStop"] = server.stop()
        shutil.rmtree(sandbox.root, ignore_errors=True)
    return {"steps": steps, "variables": sorted(ctx.vars)}, failures


_MISSING = object()


def _resolve(doc: Any, path: str) -> Any:
    """Dotted path; a segment may be a key or a list index."""
    cur = doc
    for seg in path.split(".") if path else []:
        if isinstance(cur, dict) and seg in cur:
            cur = cur[seg]
        elif isinstance(cur, list) and seg.lstrip("-").isdigit() and -len(cur) <= int(seg) < len(cur):
            cur = cur[int(seg)]
        else:
            return _MISSING
    return cur


def _check(actual: Any, want: Any) -> str | None:
    """None when actual satisfies want, else a short reason."""
    if isinstance(want, dict) and len(want) >= 1 and all(k.startswith("$") for k in want):
        for op, arg in want.items():
            problem = _op(op, actual, arg)
            if problem:
                return problem
        return None
    if actual is _MISSING:
        return f"is absent, want {want!r}"
    if actual != want:
        return f"= {_short(actual)}, want {want!r}"
    return None


def _op(op: str, actual: Any, arg: Any) -> str | None:
    if op == "$absent":
        return None if (actual is _MISSING) == bool(arg) else f"= {_short(actual)}, want absent={arg}"
    if actual is _MISSING:
        return f"is absent ({op} {arg!r})"
    text = actual if isinstance(actual, str) else json.dumps(actual)
    if op == "$ne":
        return None if actual != arg else f"= {_short(actual)}, want != {arg!r}"
    if op == "$contains":
        missing = [a for a in (arg if isinstance(arg, list) else [arg]) if a not in text]
        return f"lacks {missing!r} in {_short(actual)}" if missing else None
    if op == "$notContains":
        present = [a for a in (arg if isinstance(arg, list) else [arg]) if a and a in text]
        return f"must not contain {len(present)} forbidden value(s)" if present else None
    if op == "$re":
        return None if re.search(arg, text) else f"= {_short(actual)}, want /{arg}/"
    if op == "$prefix":
        return None if text.startswith(arg) else f"= {_short(actual)}, want prefix {arg!r}"
    if op == "$sha256Of":
        return None if actual == hashlib.sha256(arg.encode()).hexdigest() else "is not sha256 of the value"
    if op == "$in":
        return None if actual in arg else f"= {_short(actual)}, want one of {arg!r}"
    raise ValueError(f"unknown matcher {op}")


def _short(value: Any) -> str:
    text = repr(value)
    return text if len(text) <= 160 else text[:157] + "..."


# --- suites and evidence ---------------------------------------------------------


def run_suite(impl: agent.Impl, suite: str, out_dir: Path | None, ids: list[str] | None = None,
              workers: int = 6) -> dict:
    import concurrent.futures

    doc = load(suite)
    scenarios = [s for s in doc["scenarios"] if not ids or s["id"] in ids]
    started = time.time()
    with concurrent.futures.ThreadPoolExecutor(max_workers=workers) as pool:
        outcomes = list(pool.map(lambda s: (s, *execute(impl, s, f"{s['id'][:24]}")), scenarios))
    items = {}
    for scenario, observation, failures in outcomes:
        item = {"status": "pass" if not failures else "fail", "failures": failures,
                "requirement": scenario["requirement"], "specScenario": scenario["specScenario"]}
        if out_dir is not None:
            data = gzip.compress(canon.canonical_json(observation).encode(), mtime=0)
            sdir = out_dir / "scenarios" / scenario["id"]
            sdir.mkdir(parents=True, exist_ok=True)
            (sdir / "observation.json.gz").write_bytes(data)
            item["files"] = {"observation.json.gz": runner.sha256_bytes(data)}
        items[scenario["id"]] = item
    summary = {
        "schemaVersion": 1,
        "suite": suite,
        "decision": doc.get("decision"),
        "provenance": {
            "impl": {"label": impl.label, "binarySha256": impl.sha256()},
            "contractSha256": contract_sha256(suite),
            "startedAt": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(started)),
            "durationSeconds": round(time.time() - started, 1),
        },
        "items": items,
    }
    if out_dir is not None:
        out_dir.mkdir(parents=True, exist_ok=True)
        (out_dir / "summary.json").write_text(canon.canonical_json(summary) + "\n")
    return summary


# --- Rust canaries -----------------------------------------------------------------


def build_rust_canary(canary: dict, workdir: Path, target_dir: Path) -> tuple[Path | None, str]:
    """Copy the crate sources, apply one patch, build a debug binary."""
    src = runner.REPO_ROOT
    tree = workdir / "src"
    tree.mkdir(parents=True)
    for name in ("Cargo.toml", "Cargo.lock"):
        shutil.copy2(src / name, tree / name)
    shutil.copytree(src / "crates", tree / "crates")
    target = tree / canary["patch"]["file"]
    text = target.read_text()
    count = text.count(canary["patch"]["old"])
    if count != 1:
        return None, f"patch anchor matched {count} times"
    target.write_text(text.replace(canary["patch"]["old"], canary["patch"]["new"]))
    build = subprocess.run(["cargo", "build", "--locked", "--quiet", "-p", "opute-host-agent"],
                           cwd=tree, env={**os.environ, "CARGO_TARGET_DIR": str(target_dir)},
                           capture_output=True, text=True)
    if build.returncode != 0:
        return None, "build failed: " + build.stderr[-400:]
    out = workdir / "canary-bin"
    shutil.copy2(target_dir / "debug" / "opute-host-agent", out)
    return out, "ok"


def run_rust_canaries(rust_binary: Path, out_path: Path, workers: int = 6) -> dict:
    canaries = json.loads(RUST_CANARY_FILE.read_text())
    target_dir = runner.REPO_ROOT / ".parity" / "rust-canary-target"
    baseline: dict[str, dict] = {}
    results = []
    for canary in canaries:
        suite = canary["contract"]
        if suite not in baseline:
            # The canaries are meaningful only if the unpatched build is green.
            baseline[suite] = run_suite(agent.Impl("rust", rust_binary), suite, None, workers=workers)
        work = Path(tempfile.mkdtemp(prefix=f"rust-canary-{canary['id']}-"))
        try:
            binary, status = build_rust_canary(canary, work, target_dir)
            entry = {"id": canary["id"], "contract": suite, "patchApplied": binary is not None,
                     "buildStatus": status}
            if binary is not None:
                summary = run_suite(agent.Impl("rust-canary", binary), suite, None, workers=workers)
                entry["failedScenarios"] = sorted(k for k, v in summary["items"].items() if v["status"] != "pass")
                entry["caught"] = canary["scenario"] in entry["failedScenarios"]
            results.append(entry)
            print(f"{canary['id']}: {'caught' if entry.get('caught') else 'NOT CAUGHT'} "
                  f"failed={entry.get('failedScenarios')} ({status})")
        finally:
            shutil.rmtree(work, ignore_errors=True)
    doc = {
        "schemaVersion": 1,
        "rustBinarySha256": agent.Impl("rust", rust_binary).sha256(),
        "canaryFileSha256": runner.sha256_file(RUST_CANARY_FILE),
        "contractSha256": {s: contract_sha256(s) for s in baseline},
        "baselineFailures": {s: sorted(k for k, v in b["items"].items() if v["status"] != "pass")
                             for s, b in baseline.items()},
        "results": results,
    }
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(canon.canonical_json(doc) + "\n")
    return doc
