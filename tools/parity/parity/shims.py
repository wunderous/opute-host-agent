"""Recording shims for external host tools.

Each shim is a small executable written into a sandbox's shim directory. It
appends one JSON line per invocation to the sandbox trace and replays a
scripted response from the fixture. The sandbox paths are embedded in the
generated file, so a shim still records correctly when the agent under test
starts it with a scrubbed environment.

Fixture format (tools/parity/fixtures/shims/<name>.json):

    {
      "commands": {
        "incus": [
          {"argv": ["list", "--format", "json"], "stdout": "[]"},
          {"argvPrefix": ["query"], "stdout": "{}"}
        ]
      },
      "default": {"exit": 1, "stderr": "unsupported parity shim invocation\\n"}
    }

A rule matches on exact argv or on an argv prefix. The first match wins. An
unmatched invocation uses "default", and is still recorded.

A rule with "gate": "<name>" blocks after it is recorded: the shim marks
gates/<name>.waiting.<pid> and waits on the FIFO gates/<name>.fifo until the
scenario releases the gate (a "gate" step, optionally after "count" shims
block). One release wakes every blocked shim, and the gate stays open. Every running shim holds a file
pids/<pid>, so the runner can prove that no shim outlives its agent (the
NO_ORPHANS invariant).
"""

from __future__ import annotations

import json
import os
import sys
from pathlib import Path

_TEMPLATE = """#!{python}
import sys
sys.path.insert(0, {pkg_parent!r})
from parity.shims import run_shim
sys.exit(run_shim({name!r}, {fixture!r}, {log!r}))
"""


def install(shim_dir: Path, fixture_path: Path, log_path: Path) -> list[str]:
    """Write one executable per command named in the fixture."""
    fixture = json.loads(Path(fixture_path).read_text())
    shim_dir.mkdir(parents=True, exist_ok=True)
    pkg_parent = str(Path(__file__).resolve().parent.parent)
    names = sorted(fixture.get("commands", {}))
    for name in names:
        path = shim_dir / name
        path.write_text(
            _TEMPLATE.format(
                python=sys.executable,
                pkg_parent=pkg_parent,
                name=name,
                fixture=str(Path(fixture_path).resolve()),
                log=str(log_path),
            )
        )
        path.chmod(0o755)
    return names


def _match(rule: dict, argv: list[str]) -> bool:
    if "argv" in rule:
        return rule["argv"] == argv
    if "argvPrefix" in rule:
        prefix = rule["argvPrefix"]
        return argv[: len(prefix)] == prefix
    return False


GATE_TIMEOUT_SECONDS = 240


def wait_gate(root: Path, gate: str) -> None:
    """Block until the runner writes to gates/<gate>.fifo (or a timeout)."""
    import select

    gates = root / "gates"
    gates.mkdir(exist_ok=True)
    if (gates / f"{gate}.open").exists():
        # A gate is one-shot: once released, later invocations pass.
        return
    fifo = gates / f"{gate}.fifo"
    try:
        os.mkfifo(fifo, 0o600)
    except FileExistsError:
        pass
    # A non-blocking open for reading never waits for a writer; select then
    # waits for the release byte, so a forgotten gate cannot hang forever.
    fd = os.open(fifo, os.O_RDONLY | os.O_NONBLOCK)
    try:
        # One marker per blocked shim, so the runner can wait for N of them.
        # The release byte is never read: it keeps the FIFO readable, so every
        # blocked shim wakes on the one release.
        (gates / f"{gate}.waiting.{os.getpid()}").write_text("")
        select.select([fd], [], [], GATE_TIMEOUT_SECONDS)
    finally:
        os.close(fd)


def waiting_count(root: Path, gate: str) -> int:
    gates = root / "gates"
    return len(list(gates.glob(f"{gate}.waiting.*"))) if gates.exists() else 0


def await_gate(root: Path, gate: str, timeout: float = 10.0, count: int = 1) -> bool:
    """Wait until `count` shims block on the gate."""
    import time

    deadline = time.monotonic() + timeout
    while waiting_count(root, gate) < count:
        if time.monotonic() > deadline:
            return False
        time.sleep(0.01)
    return True


def release_gate(root: Path, gate: str, timeout: float = 10.0, count: int = 1) -> dict:
    """Wait until `count` shims block on the gate, then release them all."""
    if not await_gate(root, gate, timeout, count):
        return {"gate": gate, "released": False}
    (root / "gates" / f"{gate}.open").write_text("")
    fd = os.open(root / "gates" / f"{gate}.fifo", os.O_WRONLY | os.O_NONBLOCK)
    try:
        os.write(fd, b"go")
    finally:
        os.close(fd)
    return {"gate": gate, "released": True}


def live_shims(root: Path) -> list[dict]:
    """Shim processes that are still running (pid files whose process lives)."""
    out = []
    pids = root / "pids"
    if not pids.exists():
        return out
    for path in sorted(pids.iterdir()):
        try:
            os.kill(int(path.name), 0)
        except (ProcessLookupError, ValueError):
            continue
        except PermissionError:
            pass
        try:
            out.append({"pid": int(path.name), "cmd": path.read_text()})
        except FileNotFoundError:
            continue  # the shim exited between the scan and the read
    return out


def unclean_shims(root: Path) -> list[dict]:
    """Shims that died without cleaning up their pid file: SIGKILL (or a
    crash). Catchable signals are recorded in the trace instead."""
    out = []
    pids = root / "pids"
    if not pids.exists():
        return out
    for path in sorted(pids.iterdir()):
        try:
            os.kill(int(path.name), 0)
            continue  # still running: an orphan, not an unclean exit
        except (ProcessLookupError, ValueError):
            pass
        except PermissionError:
            continue
        try:
            out.append({"cmd": path.read_text()})
        except FileNotFoundError:
            continue
    return out


def _append(log_path: str, entry: dict) -> None:
    line = (json.dumps(entry, sort_keys=True) + "\n").encode()
    fd = os.open(log_path, os.O_WRONLY | os.O_APPEND | os.O_CREAT, 0o600)
    try:
        os.write(fd, line)
    finally:
        os.close(fd)


def run_shim(name: str, fixture_path: str, log_path: str) -> int:
    import signal

    root = Path(log_path).parent
    pids = root / "pids"
    pids.mkdir(exist_ok=True)
    pid_file = pids / str(os.getpid())
    pid_file.write_text(" ".join([name] + sys.argv[1:]))

    def on_signal(signum, _frame):
        # The agent's cancellation reached the child: record which signal,
        # then exit as an uncaught signal would (128 + signum).
        _append(log_path, {"cmd": name, "argv": sys.argv[1:], "signal": signal.Signals(signum).name})
        pid_file.unlink(missing_ok=True)
        os._exit(128 + signum)

    for sig in (signal.SIGTERM, signal.SIGINT, signal.SIGHUP):
        signal.signal(sig, on_signal)
    try:
        return _run_shim(name, fixture_path, log_path, root)
    finally:
        pid_file.unlink(missing_ok=True)


def _run_shim(name: str, fixture_path: str, log_path: str, root: Path) -> int:
    import hashlib

    argv = sys.argv[1:]
    stdin_digest = None
    if not sys.stdin.isatty():
        try:
            data = sys.stdin.buffer.read()
            if data:
                stdin_digest = hashlib.sha256(data).hexdigest()
        except (OSError, ValueError):
            pass
    fixture = json.loads(Path(fixture_path).read_text())
    rules = fixture.get("commands", {}).get(name, [])
    chosen, index = fixture.get("default", {"exit": 1}), None
    for i, rule in enumerate(rules):
        if _match(rule, argv):
            chosen, index = rule, i
            break
    entry = {"cmd": name, "argv": argv, "rule": index}
    if stdin_digest:
        entry["stdinSha256"] = stdin_digest
    _append(log_path, entry)
    if chosen.get("gate"):
        wait_gate(root, chosen["gate"])
    if chosen.get("stdout"):
        sys.stdout.write(chosen["stdout"])
        if not chosen["stdout"].endswith("\n"):
            sys.stdout.write("\n")
    if chosen.get("stderr"):
        sys.stderr.write(chosen["stderr"])
    return int(chosen.get("exit", 0))


def read_trace(log_path: Path) -> list[dict]:
    if not log_path.exists():
        return []
    return [json.loads(line) for line in log_path.read_text().splitlines() if line.strip()]
