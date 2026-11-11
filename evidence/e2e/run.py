#!/usr/bin/env python3
"""Run one E2E journey once on the dev VM and write its receipt.

  python3 evidence/e2e/run.py JOURNEY_ID --run RUN_ID

A journey (journeys/<id>.json) declares:
  inputs        what it starts from (agent, stubs, arguments)
  requirements  the spec requirements it checks, each with a pass condition:
                assertions over the recorded steps (a requirement with no
                assertion is refused, so a pass is never vacuous)
  effects       glob patterns over state-diff paths the journey may change
                ("allow"), and what may remain after cleanup ("persist")
  steps         in order, each one of:
                mcp     a JSON-RPC request to the agent (forwarded port), to a
                        guest port (`port`), or to a self-check stub; with
                        `capture`, `until` polling, `auth` and `meta`
                http    a plain HTTP request (OAuth endpoints, /health)
                guest   a shell command in the VM as root; stdout that parses
                        as JSON is recorded as `json`
                copy    a hash-pinned input file (inputs.files) into the VM
                restore discard the run overlay and boot the snapshot
                `${NAME}` expands a captured value; `${SECRET:name}` a per-run
                random secret; `${TOKEN}` the agent's test token. All three are
                redacted from every record.
  cleanup       steps that undo what the journey did
  maxMinutes    at most 30
A requirement whose outcome depends on an open owner question
(openspec/changes/rust-spec-authority/questions.md) names it in
`openQuestion`: its assertions are recorded as observations and its outcome
is `unverified`, never a pass or a fail.
Every journey starts from the snapshot (a checked restore, preflight).

The run records, under runs/<run>/<journey>/:
  journey.json        the journey as run
  transcript.jsonl    every step's request and response (token redacted)
  state-*.json.gz     the VM state before, after and after cleanup
  results.json        a result per requirement, the schema errors of every
                      MCP response, the effects diff, cleanup residue, timings
  receipt.json        sha256 of every file above and of the harness

verify.py re-derives results.json from the transcript and the state files.
A receipt is never overwritten: each journey runs once.
"""
from __future__ import annotations

import argparse
import fnmatch
import gzip
import http.server
import json
import re
import secrets
import shlex
import sys
import threading
import time
from pathlib import Path
from typing import Any

sys.path.insert(0, str(Path(__file__).resolve().parent))
sys.path.insert(0, str(Path(__file__).resolve().parent / "devvm"))
import e2elib  # noqa: E402

E2E = e2elib.E2E_DIR
JOURNEYS = E2E / "journeys"
RUNS = E2E / "runs"
TOKEN_PATTERN = re.compile(r"e2e-test-token-[0-9a-f]{48}")
STEP_KINDS = ("mcp", "health", "guest", "restore", "copy", "http")
OPS = ("equals", "in", "exists", "absent", "matches", "gte", "lte", "contains", "equalsStep")
QUESTION = re.compile(r"^Q\d+$")
MAX_MINUTES = 30


# --- journey format -------------------------------------------------------------------


def load_journey(journey_id: str) -> dict:
    path = JOURNEYS / f"{journey_id}.json"
    journey = json.loads(path.read_text())
    problems = journey_problems(journey)
    if journey.get("id") != journey_id:
        problems.append("id does not match the file name")
    if problems:
        raise SystemExit(f"refused: journey {journey_id} is malformed: " + "; ".join(problems))
    return journey


def journey_problems(journey: dict) -> list[str]:
    out = []
    for key in ("id", "gate", "description", "inputs", "requirements", "effects", "steps", "cleanup", "maxMinutes"):
        if key not in journey:
            out.append(f"missing {key}")
    if out:
        return out
    if not 0 < journey["maxMinutes"] <= MAX_MINUTES:
        out.append(f"maxMinutes must be in (0, {MAX_MINUTES}]")
    ids = [s.get("id") for s in journey["steps"] + journey["cleanup"]]
    if len(set(ids)) != len(ids) or not all(ids):
        out.append("every step needs a unique id")
    for step in journey["steps"] + journey["cleanup"]:
        kinds = [k for k in STEP_KINDS if k in step]
        if len(kinds) != 1:
            out.append(f"step {step.get('id')}: exactly one of {STEP_KINDS}")
    if not journey["requirements"]:
        out.append("no requirement is checked")
    step_ids = {s["id"] for s in journey["steps"]}
    for req in journey["requirements"]:
        if not req.get("spec") or not req.get("requirement"):
            out.append("a requirement needs spec and requirement")
        if not req.get("pass"):
            out.append(f"requirement {req.get('requirement')!r} has no pass condition")
        if "openQuestion" in req and not QUESTION.match(str(req["openQuestion"])):
            out.append(f"openQuestion {req['openQuestion']!r} is not a question number")
        for a in req.get("pass") or []:
            if a.get("step") not in step_ids:
                out.append(f"assertion on unknown step {a.get('step')!r}")
            if "equalsStep" in a and a["equalsStep"].get("step") not in step_ids:
                out.append(f"equalsStep on unknown step {a['equalsStep'].get('step')!r}")
            if len([op for op in OPS if op in a]) != 1:
                out.append(f"assertion on {a.get('step')!r} needs exactly one of {OPS}")
    files = (journey["inputs"] or {}).get("files", {})
    for step in journey["steps"] + journey["cleanup"]:
        if "copy" in step and (step["copy"] not in files or not files[step["copy"]].get("sha256")):
            out.append(f"step {step['id']}: copy needs a hash-pinned inputs.files entry")
    effects = journey["effects"]
    if not isinstance(effects.get("allow"), list) or not isinstance(effects.get("persist"), list):
        out.append("effects needs allow and persist lists")
    elif not set(effects["persist"]) <= set(effects["allow"]):
        out.append("every persist pattern must also be allowed")
    return out


# --- evaluation (shared with verify.py) -------------------------------------------------


def dig(obj: Any, path: str) -> tuple[bool, Any]:
    """Follow a dotted path (`body.result.tools[0].name`); (found, value)."""
    for part in re.findall(r"[^.\[\]]+|\[\d+\]", path) if path else []:
        if part.startswith("["):
            index = int(part[1:-1])
            if not isinstance(obj, list) or index >= len(obj):
                return False, None
            obj = obj[index]
        else:
            if not isinstance(obj, dict) or part not in obj:
                return False, None
            obj = obj[part]
    return True, obj


def check(assertion: dict, record: dict | None, records: dict | None = None) -> dict:
    found, actual = dig(record, assertion.get("path", "")) if record is not None else (False, None)
    if not found:
        actual = "<absent>"  # an `in` or `equals` may name the sentinel to accept absence
    op = next(op for op in OPS if op in assertion)
    expected = assertion[op]
    if op == "exists":
        ok = found == bool(expected)
    elif op == "absent":
        ok = (not found) == bool(expected)
    elif op == "equals":
        ok = actual == expected
    elif op == "in":
        ok = actual in expected
    elif op == "matches":
        ok = isinstance(actual, str) and re.search(expected, actual) is not None
    elif op == "gte":
        ok = isinstance(actual, (int, float)) and actual >= expected
    elif op == "lte":
        ok = isinstance(actual, (int, float)) and actual <= expected
    elif op == "equalsStep":
        other = (records or {}).get(expected["step"])
        other_found, other_value = dig(other, expected.get("path", "")) if other is not None else (False, None)
        ok = found and other_found and actual == other_value
        return {**assertion, "actual": actual, "other": other_value if other_found else "<absent>", "ok": ok}
    else:  # contains
        ok = isinstance(actual, (list, str, dict)) and expected in actual
    return {**assertion, "actual": actual, "ok": ok}


def state_diff(before: Any, after: Any, prefix: str = "") -> list[str]:
    """Paths of every leaf that differs between two state documents."""
    if isinstance(before, dict) and isinstance(after, dict):
        out = []
        for key in sorted(set(before) | set(after)):
            path = f"{prefix}/{key}" if prefix else str(key)
            if key not in before or key not in after:
                out.append(path)
            else:
                out += state_diff(before[key], after[key], path)
        return out
    return [] if before == after else [prefix]


def evaluate(journey: dict, transcript: list[dict], states: dict[str, Any]) -> dict:
    """Everything a journey's verdict depends on, from its recorded evidence only."""
    records = {r["step"]: r for r in transcript if r.get("phase") == "step"}
    requirements = []
    for req in journey["requirements"]:
        checks = [check(a, records.get(a["step"]), records) for a in req["pass"]]
        entry = {"spec": req["spec"], "requirement": req["requirement"], "assertions": checks}
        if "openQuestion" in req:
            entry.update(outcome="unverified", openQuestion=req["openQuestion"],
                         observedAsAsserted=all(c["ok"] for c in checks))
        else:
            entry.update(outcome="pass" if all(c["ok"] for c in checks) else "fail")
        requirements.append(entry)
    # Every MCP response is validated against the tools/list its own target
    # published earlier in the same run.
    schema = []
    tools: dict[str, dict] = {}
    for r in transcript:
        if r.get("kind") != "mcp":
            continue
        target = r.get("target", "agent")
        catalog = tools.setdefault(target, {})
        errors = e2elib.validate_response(r["method"], r["requestId"], {**r["response"], "tool": r.get("tool")},
                                          catalog)
        if r["method"] == "tools/list" and not errors:
            for tool in ((r["response"].get("body") or {}).get("result") or {}).get("tools") or []:
                catalog[tool["name"]] = tool
        if errors:
            schema.append({"step": r["step"], "phase": r["phase"], "errors": errors})
    # A client that hides the JSON-RPC envelope (pkg/hostagentclient) reports
    # each call's structured result; each is validated against the outputSchema
    # its target published.
    for r in transcript:
        if r.get("kind") != "guest" or not r.get("schemaResults"):
            continue
        found, results = dig(r, r["schemaResults"])
        catalog = tools.get(r.get("schemaTarget", "agent"), {})
        errors = []
        if not found or not isinstance(results, list) or not results:
            errors.append(f"{r['schemaResults']}: no client results to validate")
        for i, item in enumerate(results if isinstance(results, list) else []):
            schema_ = (catalog.get(item.get("tool")) or {}).get("outputSchema")
            if item.get("tool") not in catalog:
                errors.append(f"[{i}] tool {item.get('tool')!r} was not in its target's tools/list")
            elif schema_ and not item.get("isError") and "structuredContent" in item:
                errors += e2elib.validate(item["structuredContent"], schema_, f"[{i}].structuredContent")
        if errors:
            schema.append({"step": r["step"], "phase": r["phase"], "errors": errors})
    changed = state_diff(states["before"], states["after"])
    residue = state_diff(states["before"], states["cleanup"])
    allowed = journey["effects"]["allow"]
    persist = journey["effects"]["persist"]
    undeclared = [p for p in changed if not any(fnmatch.fnmatchcase(p, g) for g in allowed)]
    left = [p for p in residue if not any(fnmatch.fnmatchcase(p, g) for g in persist)]
    harness_errors = [{"step": r["step"], "error": r["harnessError"]} for r in transcript if r.get("harnessError")]
    seconds = sum(r.get("ms", 0) for r in transcript) / 1000
    reasons = []
    if any(r["outcome"] == "fail" for r in requirements):
        reasons.append("pass-condition")
    if schema:
        reasons.append("schema")
    if undeclared:
        reasons.append("undeclared-effect")
    if left:
        reasons.append("cleanup-residue")
    if harness_errors:
        reasons.append("harness-error")
    if seconds > journey["maxMinutes"] * 60:
        reasons.append("time-limit")
    unverified = sorted({r["openQuestion"] for r in requirements if r["outcome"] == "unverified"})
    return {"journey": journey["id"], "gate": journey["gate"], "pass": not reasons, "failures": reasons,
            "unverified": unverified,
            "requirements": requirements, "schemaErrors": schema,
            "effects": {"changed": changed, "undeclared": undeclared, "residue": residue, "residueUndeclared": left},
            "harnessErrors": harness_errors,
            "timings": {"stepsSeconds": round(seconds, 1),
                        "steps": {r["step"]: r.get("ms") for r in transcript if r.get("phase") == "step"}}}


# --- running -----------------------------------------------------------------------------


class Stub:
    """A loopback MCP endpoint that answers from the journey's declared input
    (harness self-checks only): tools/list returns inputs.stub.tools and
    tools/call returns inputs.stub.results[name]."""

    def __init__(self, spec: dict):
        self.spec = spec

        class Handler(http.server.BaseHTTPRequestHandler):
            def do_POST(handler) -> None:  # noqa: N802
                request = json.loads(handler.rfile.read(int(handler.headers["Content-Length"])))
                method = request["method"]
                if method == "tools/list":
                    result = {"tools": spec["tools"]}
                elif method == "tools/call":
                    result = spec["results"][request["params"]["name"]]
                else:
                    result = {}
                body = json.dumps({"jsonrpc": "2.0", "id": request["id"], "result": result}).encode()
                handler.send_response(200)
                handler.send_header("Content-Type", "application/json")
                handler.send_header("Content-Length", str(len(body)))
                handler.end_headers()
                handler.wfile.write(body)

            def log_message(handler, *args: Any) -> None:
                pass

        self.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.port = self.server.server_address[1]
        threading.Thread(target=self.server.serve_forever, daemon=True).start()

    def close(self) -> None:
        self.server.shutdown()


def scrub(value: Any, ctx: dict) -> Any:
    """Every test credential the run knows, replaced by its name."""
    text = json.dumps(value)
    for secret, name in [(ctx["token"], "<test-token>")] + [(v, f"<secret:{k}>") for k, v in ctx["secrets"].items()]:
        if secret and secret in text:
            text = text.replace(secret, name)
    return json.loads(text)


_VAR = re.compile(r"\$\{(SECRET:)?([A-Za-z0-9_.-]+)\}")


def expand(value: Any, ctx: dict) -> Any:
    """`${TOKEN}`, `${SECRET:name}` (a per-run random secret) and `${NAME}` (a capture)."""
    if isinstance(value, str):
        def sub(m: re.Match) -> str:
            if m.group(1):
                return ctx["secrets"].setdefault(m.group(2), f"e2e-secret-{secrets.token_hex(16)}")
            if m.group(2) == "TOKEN":
                return ctx["token"]
            if m.group(2) not in ctx["vars"]:
                raise KeyError(f"${{{m.group(2)}}} was never captured")
            return str(ctx["vars"][m.group(2)])
        return _VAR.sub(sub, value)
    if isinstance(value, list):
        return [expand(v, ctx) for v in value]
    if isinstance(value, dict):
        return {k: expand(v, ctx) for k, v in value.items()}
    return value


def capture(step: dict, record: dict, ctx: dict) -> None:
    for name, spec in (step.get("capture") or {}).items():
        spec = spec if isinstance(spec, dict) else {"path": spec}
        found, value = dig(record, spec["path"])
        if not found:
            raise KeyError(f"capture {name}: {spec['path']} is absent")
        if spec.get("secret"):
            ctx["secrets"][name] = str(value)
        ctx["vars"][name] = value


def bearer(auth: str, ctx: dict) -> str | None:
    if auth == "token":
        return ctx["token"]
    if auth == "none":
        return None
    if auth.startswith("var:"):
        return str(ctx["vars"][auth[4:]])
    if auth.startswith("literal:"):
        return auth[8:]
    if auth.startswith("secret:"):
        return expand(f"${{SECRET:{auth[7:]}}}", ctx)
    raise ValueError(f"unknown auth {auth!r}")


GUEST_WIRE = "/opt/opute-e2e/guest/mcpwire.py"


def wire(kind: str, port: int | None, ctx: dict, **request: Any) -> dict:
    """A request from the host to the forwarded agent port, or from inside the guest to a guest port."""
    import devvm
    if port is None:
        return getattr(e2elib, kind)(devvm.MCP_PORT, **request)
    if not ctx.get("guestWire"):
        devvm.guest("install -d /opt/opute-e2e/guest", check=True)
        devvm.scp([E2E / "mcpwire.py"], GUEST_WIRE)
        ctx["guestWire"] = True
    done = devvm.guest(f"python3 {GUEST_WIRE}", input=json.dumps({"kind": kind, "port": port, **request}),
                       timeout=request.get("timeout", 120) + 30, check=True)
    return json.loads(done.stdout)


def run_mcp(step: dict, ctx: dict) -> dict:
    target = step.get("target", "agent")
    port = ctx["stub"].port if target == "stub" else None
    guest_port = step.get("port")
    key = f"guest:{guest_port}" if guest_port else target
    auth = step.get("auth", "token")
    if step["mcp"] == "tools/call":
        params = {"name": expand(step["tool"], ctx), "arguments": expand(step.get("arguments", {}), ctx),
                  **expand(step.get("params", {}), ctx)}
    else:
        params = expand(step.get("params"), ctx)
    deadline = time.monotonic() + step.get("timeout", 0)
    retry_deadline = time.monotonic() + step.get("retryTimeout", 120)
    attempts = 0
    while True:
        ctx["requestId"] += 1
        attempts += 1
        request = dict(token=bearer(auth, ctx), method=step["mcp"], params=params, request_id=ctx["requestId"],
                       modern=step.get("modern", True), meta=expand(step.get("meta"), ctx))
        if target == "stub":
            response = e2elib.mcp(port, **request)
        else:
            response = wire("mcp", guest_port, ctx, **request)
        record = {"kind": "mcp", "target": key, "method": step["mcp"], "requestId": ctx["requestId"],
                  "tool": params.get("name") if step["mcp"] == "tools/call" else None,
                  "auth": auth.split(":")[0], "params": params, "response": response, "attempts": attempts}
        # retryOn: an admission refusal the step may wait out, honouring the advertised retryAfterMs
        refused = (((response or {}).get("body") or {}).get("result") or {}).get("structuredContent") or {}
        if refused.get("code") in step.get("retryOn", ()) and time.monotonic() < retry_deadline:
            time.sleep(min(max(refused.get("retryAfterMs", 1000), 100), 5000) / 1000)
            continue
        until = step.get("until")
        if not until:
            return record
        if until_met(until, record):
            return record
        if time.monotonic() >= deadline:
            record["untilTimedOut"] = True
            return record
        time.sleep(step.get("interval", 1))


def until_met(until: dict, record: dict) -> bool:
    """An `until` holds when its path's value is one of `in`, or a string `matches` searches."""
    found, value = dig(record, until["path"])
    if not found:
        return False
    if "matches" in until:
        return isinstance(value, str) and re.search(until["matches"], value) is not None
    return value in until["in"]


def run_step(step: dict, phase: str, ctx: dict) -> dict:
    import devvm
    record: dict[str, Any] = {"step": step["id"], "phase": phase}
    t0 = time.monotonic()
    try:
        if "mcp" in step:
            record.update(run_mcp(step, ctx))
        elif "http" in step:
            request = {k: expand(step[k], ctx) for k in ("path", "query", "form", "basic", "headers") if k in step}
            if "json" in step:
                request["json_body"] = expand(step["json"], ctx)
            record.update(kind="http", method=step["http"], port=step.get("port"), request=request,
                          response=wire("http", step.get("port"), ctx, method=step["http"], **request))
        elif "health" in step:
            port = step.get("port")
            record.update(kind="health", port=port, response=wire("health", port, ctx) if port
                          else e2elib.health(devvm.MCP_PORT))
        elif "guest" in step:
            command = step["guest"]
            done = devvm.guest(expand(command, ctx), timeout=step.get("timeout", 600))
            record.update(kind="guest", command=command, exit=done.returncode, stdout=done.stdout[-8000:],
                          stderr=done.stderr[-4000:])
            try:
                record["json"] = json.loads(done.stdout)
            except ValueError:
                pass
            for key in ("schemaResults", "schemaTarget"):
                if key in step:
                    record[key] = step[key]
        elif "copy" in step:
            spec = ctx["files"][step["copy"]]
            source = Path(expand_path(spec["path"]))
            actual = e2elib.sha256_file(source)
            if actual != spec["sha256"]:
                raise ValueError(f"input {step['copy']} sha256 {actual} != pinned {spec['sha256']}")
            devvm.guest(f"install -d {shlex.quote(str(Path(step['to']).parent))}", check=True)
            devvm.scp([source], step["to"])
            if "mode" in step:
                devvm.guest(f"chmod {step['mode']} {shlex.quote(step['to'])}", check=True)
            record.update(kind="copy", input=step["copy"], to=step["to"], sha256=actual)
        else:
            record.update(kind="restore", response=devvm.restore(check=True))
        capture(step, record, ctx)
    except Exception as error:  # recorded, never retried: the verdict says harness-error
        record["harnessError"] = f"{type(error).__name__}: {error}"
    record["ms"] = round((time.monotonic() - t0) * 1000)
    return scrub(record, ctx)


def expand_path(path: str) -> str:
    return path.replace("${PARITY}", str(e2elib.PARITY)).replace("${REPO}", str(e2elib.REPO_ROOT))


def write_gz(path: Path, obj: Any) -> None:
    path.write_bytes(gzip.compress(json.dumps(obj, sort_keys=True).encode(), mtime=0))


def harness_hashes() -> dict:
    files = [E2E / "run.py", E2E / "e2elib.py", E2E / "mcpwire.py", E2E / "verify.py", E2E / "devvm/devvm.py",
             E2E / "devvm/guest/state.py", e2elib.REPO_ROOT / "tools/parity/t2guest/snapshot.py"]
    return {str(f.relative_to(e2elib.REPO_ROOT)): e2elib.sha256_file(f) for f in files}


def run(journey_id: str, run_id: str) -> dict:
    import devvm
    lease = e2elib.require_lease()
    devvm.check_base()
    if not devvm.pid():
        raise SystemExit("refused: the dev VM is not running (devvm.py start)")
    journey = load_journey(journey_id)
    out = RUNS / run_id / journey_id
    if out.exists():
        raise SystemExit(f"refused: {out} exists; a journey runs once")
    out.mkdir(parents=True)
    ctx = {"token": devvm.token(), "requestId": 0, "vars": {}, "secrets": {},
           "files": journey["inputs"].get("files", {}),
           "stub": Stub(journey["inputs"]["stub"]) if "stub" in journey["inputs"] else None}
    started = e2elib.now()
    transcript = []
    states = {}
    try:
        # Every journey starts from the snapshot; a restore that does not
        # return the baseline is a harness error, not a journey result.
        restored = run_step({"id": "snapshot", "restore": True}, "preflight", ctx)
        if not restored.get("harnessError") and not restored["response"].get("equal"):
            restored["harnessError"] = "restore did not return the baseline"
        transcript.append(restored)
        catalog = {"id": "catalog", "mcp": "tools/list"}
        transcript.append(run_step(catalog, "preflight", ctx))
        states["before"] = devvm.state()
        for step in journey["steps"]:
            transcript.append(run_step(step, "step", ctx))
        states["after"] = devvm.state()
        for step in journey["cleanup"]:
            transcript.append(run_step(step, "cleanup", ctx))
        states["cleanup"] = devvm.state()
    finally:
        if ctx["stub"]:
            ctx["stub"].close()
    results = evaluate(journey, transcript, states)
    (out / "journey.json").write_text(json.dumps(journey, indent=1, sort_keys=True) + "\n")
    (out / "transcript.jsonl").write_text("".join(json.dumps(r, sort_keys=True) + "\n" for r in transcript))
    for name, doc in states.items():
        write_gz(out / f"state-{name}.json.gz", doc)
    (out / "results.json").write_text(json.dumps(results, indent=1, sort_keys=True) + "\n")
    for path in out.iterdir():
        data = gzip.decompress(path.read_bytes()) if path.suffix == ".gz" else path.read_bytes()
        if TOKEN_PATTERN.search(data.decode(errors="replace")) or any(
                v.encode() in data for v in [ctx["token"], *ctx["secrets"].values()] if v):
            for p in out.iterdir():
                p.unlink()
            out.rmdir()
            raise SystemExit(f"refused: {path.name} would carry a test credential; nothing written")
    build = json.loads((e2elib.DEV / "build.json").read_text())
    receipt = {
        "journey": journey_id, "gate": journey["gate"], "run": run_id, "started": started, "finished": e2elib.now(),
        "verdict": {"pass": results["pass"], "failures": results["failures"], "unverified": results["unverified"]},
        "journeySourceSha256": e2elib.sha256_file(JOURNEYS / f"{journey_id}.json"),
        "files": {p.name: e2elib.sha256_file(p) for p in sorted(out.iterdir())},
        "harness": harness_hashes(),
        "devvm": {"baselineSha256": build["baselineSha256"], "baseSha256": build["baseSha256"],
                  "rustRelease": build["releases"]["rust"]["files"], "lease": lease["holder"]},
    }
    (out / "receipt.json").write_text(json.dumps(receipt, indent=1, sort_keys=True) + "\n")
    return receipt


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("journey")
    parser.add_argument("--run", required=True, help="run id, e.g. e2e-0-r1")
    args = parser.parse_args()
    receipt = run(args.journey, args.run)
    print(json.dumps(receipt["verdict"]))
    return 0 if receipt["verdict"]["pass"] else 1


if __name__ == "__main__":
    sys.exit(main())
