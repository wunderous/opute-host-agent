"""Run the Go baseline's own black-box tests against a candidate binary.

Some Go tests build the agent from source inside the test (for example
test/standalone's buildStandaloneBinary). The oracle applies one recorded
overlay to a scratch worktree of the pinned Go tree so those helpers return
$OPUTE_STANDALONE_BINARY when it is set, then runs the selected tests against
the Go reference (control) and the candidate. The overlay is never pushed.
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import tempfile
from pathlib import Path

from . import agent, canon, runner

ORACLE_FILE = runner.TOOLS_DIR / "oracles.json"


def _apply_overlay(tree: Path, overlay: list[dict]) -> None:
    for patch in overlay:
        target = tree / patch["file"]
        text = target.read_text()
        if text.count(patch["old"]) != 1:
            raise SystemExit(f"oracle overlay anchor for {patch['file']} matched {text.count(patch['old'])} times")
        target.write_text(text.replace(patch["old"], patch["new"]))


NEGATIVE_CONTROL = ("negative-control", Path("/bin/true"))


def run(src: Path, binaries: dict[str, Path], out_path: Path) -> dict:
    """Run every oracle suite for each binary, plus a negative control.

    The control is /bin/true, which accepts every configuration; the suites
    must FAIL against it, proving the overlay really runs the given binary.
    """
    lock = json.loads((runner.REPO_ROOT / "baseline" / "source-lock.json").read_text())
    binaries = {**binaries, NEGATIVE_CONTROL[0]: NEGATIVE_CONTROL[1]}
    spec = json.loads(ORACLE_FILE.read_text())
    results = []
    work = Path(tempfile.mkdtemp(prefix="oracle-"))
    tree = work / "src"
    subprocess.run(["git", "-C", str(src), "worktree", "add", "--detach", "-f", str(tree), "HEAD"],
                   check=True, capture_output=True)
    try:
        for suite in spec:
            _apply_overlay(tree, suite.get("overlay", []))
            for label, binary in binaries.items():
                env = {**os.environ, "OPUTE_STANDALONE_BINARY": str(binary), "CGO_ENABLED": "0"}
                env = {k: v for k, v in env.items() if not (k.startswith("OPUTE_") and k != "OPUTE_STANDALONE_BINARY")}
                proc = subprocess.run(
                    ["go", "test", suite["package"], "-count=1", "-v", "-run", suite["run"]],
                    cwd=tree, env=env, capture_output=True, text=True, timeout=900)
                passed = sorted({line.split()[2] for line in proc.stdout.splitlines()
                                 if line.startswith("--- PASS: ")})
                failed = sorted({line.split()[2] for line in proc.stdout.splitlines()
                                 if line.startswith("--- FAIL: ")})
                results.append({
                    "suite": suite["id"],
                    "binary": label,
                    "binarySha256": agent.Impl(label, binary).sha256(),
                    "exit": proc.returncode,
                    "passed": passed,
                    "failed": failed,
                    "expectedTests": suite["tests"],
                })
                status = "PASS" if proc.returncode == 0 and set(suite["tests"]) <= set(passed) else "FAIL"
                print(f"{suite['id']} [{label}]: {status} passed={passed} failed={failed}")
    finally:
        subprocess.run(["git", "-C", str(src), "worktree", "remove", "--force", str(tree)], capture_output=True)
        shutil.rmtree(work, ignore_errors=True)
    doc = {
        "schemaVersion": 1,
        "sourceCommit": lock["sourceCommit"],
        "oracleFileSha256": runner.sha256_file(ORACLE_FILE),
        "results": results,
    }
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(canon.canonical_json(doc) + "\n")
    return doc
