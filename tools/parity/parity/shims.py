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


def run_shim(name: str, fixture_path: str, log_path: str) -> int:
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
    line = (json.dumps(entry, sort_keys=True) + "\n").encode()
    fd = os.open(log_path, os.O_WRONLY | os.O_APPEND | os.O_CREAT, 0o600)
    try:
        os.write(fd, line)
    finally:
        os.close(fd)
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
