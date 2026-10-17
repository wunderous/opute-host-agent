"""Run the Go baseline's own live tests against a binary inside T2 twins.

Some Go black-box tests need a real provider: test/live drives Incus. They
cannot run on the developer host (no shared Incus may be touched), so each
suite in oracles-t2.json is compiled once from a scratch worktree of the
pinned Go tree with its recorded overlay applied, and the test binary runs
inside a fresh twin booted from the suite's golden, once per binary under
test plus a negative control (/bin/true, which must fail every test). The
overlay is never pushed; the twin is deleted afterwards.
"""

from __future__ import annotations

import json
import os
import re
import shlex
import shutil
import subprocess
import tempfile
from pathlib import Path

from . import agent, canon, oracle, runner, t2

ORACLE_FILE = runner.TOOLS_DIR / "oracles-t2.json"
TEST_LOG = re.compile(r"^\s+\w+_test\.go:\d+: ")


def _compile(src: Path, suite: dict, work: Path) -> Path:
    tree = work / "src"
    subprocess.run(["git", "-C", str(src), "worktree", "add", "--detach", "-f", str(tree), "HEAD"],
                   check=True, capture_output=True)
    try:
        oracle._apply_overlay(tree, suite.get("overlay", []))
        out = work / f"{suite['id']}.test"
        argv = ["go", "test", "-c", "-o", str(out)]
        if suite.get("tags"):
            argv += ["-tags", suite["tags"]]
        subprocess.run(argv + [suite["package"]], cwd=tree, check=True, capture_output=True, text=True,
                       env={**os.environ, "CGO_ENABLED": "0"})
        return out
    finally:
        subprocess.run(["git", "-C", str(src), "worktree", "remove", "--force", str(tree)], capture_output=True)


def _run_in_twin(suite: dict, test_binary: Path, label: str, binary: Path, run_dir: Path) -> dict:
    with t2.Twin(label, binary, run_dir, suite.get("golden", "base")) as twin:
        scp = ["scp", "-q"] + [a for a in t2._ssh_base(twin.port)[1:-3] if a != "-p"] + ["-P", str(twin.port)]
        subprocess.run(scp + [str(test_binary), "root@127.0.0.1:/opt/parity-t2/oracle.test"], check=True)
        # As ubuntu (incus-admin), from a scratch directory; only the test
        # binary is told which agent to start.
        command = ("chmod 0755 /opt/parity-t2/oracle.test && cd \"$(mktemp -d)\" && "
                   "setpriv --reuid=ubuntu --regid=ubuntu --init-groups env -i HOME=/home/ubuntu USER=ubuntu "
                   "PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin "
                   "TMPDIR=/tmp OPUTE_STANDALONE_BINARY=/opt/parity-t2/agent "
                   "/opt/parity-t2/oracle.test -test.v -test.count=1 -test.timeout=75m -test.run "
                   + shlex.quote(suite["run"]))
        done = subprocess.run(t2._ssh_base(twin.port) + [command], stdin=subprocess.DEVNULL, capture_output=True,
                              text=True, timeout=5400)
    lines = done.stdout.splitlines()

    def named(prefix: str) -> list[str]:
        return sorted({line.split()[2] for line in lines if line.startswith(prefix)})
    # What each test reported (t.Fatalf / t.Skipf lines), compared across binaries.
    messages = [line.strip() for line in lines if TEST_LOG.match(line)]
    return {"exit": done.returncode, "passed": named("--- PASS: "), "failed": named("--- FAIL: "),
            "skipped": named("--- SKIP: "), "messages": messages,
            "stdout": done.stdout[-20000:], "stderr": done.stderr[-4000:]}


OUTCOME = ("passed", "failed", "skipped", "messages")


def outcome(row: dict) -> dict:
    return {k: row.get(k) for k in OUTCOME}


def judge(suite: dict, rows: dict[str, dict]) -> dict[str, str]:
    """Per binary: PASS when mustPass passes and, for a candidate, every test's
    outcome equals the Go reference's; the negative control must pass nothing."""
    out = {}
    go = rows.get("go")
    for label, row in rows.items():
        if label == oracle.NEGATIVE_CONTROL[0]:
            out[label] = "PASS" if not row.get("passed") else "FAIL"
            continue
        ok = set(suite["mustPass"]) <= set(row.get("passed", []))
        if label != "go":
            ok = ok and go is not None and outcome(row) == outcome(go)
        out[label] = "PASS" if ok else "FAIL"
    return out


def run(src: Path, binaries: dict[str, Path], out_path: Path) -> dict:
    lock = json.loads((runner.REPO_ROOT / "baseline" / "source-lock.json").read_text())
    binaries = {**binaries, oracle.NEGATIVE_CONTROL[0]: oracle.NEGATIVE_CONTROL[1]}
    spec = json.loads(ORACLE_FILE.read_text())
    results = []
    work = Path(tempfile.mkdtemp(prefix="oracle-t2-"))
    try:
        for suite in spec:
            test_binary = _compile(src, suite, work)
            rows = {}
            for label, binary in binaries.items():
                result = _run_in_twin(suite, test_binary, label, binary, work / "twins" / suite["id"])
                rows[label] = {"suite": suite["id"], "binary": label,
                               "binarySha256": agent.Impl(label, binary).sha256(),
                               "testBinarySha256": runner.sha256_file(test_binary),
                               "expectedTests": suite["tests"], "mustPass": suite["mustPass"], **result}
            for label, status in judge(suite, rows).items():
                row = rows[label]
                row["status"] = status
                results.append(row)
                print(f"{suite['id']} [{label}]: {status} passed={row['passed']} failed={row['failed']} "
                      f"skipped={row['skipped']}")
    finally:
        shutil.rmtree(work, ignore_errors=True)
    doc = {"schemaVersion": 1, "sourceCommit": lock["sourceCommit"],
           "oracleFileSha256": runner.sha256_file(ORACLE_FILE),
           "goldens": {"base": t2.golden_record()["goldenSha256"],
                       **({"incus": t2.incus_golden_record()["goldenSha256"]}
                          if any(s.get("golden") == "incus" for s in spec) else {})},
           "results": results}
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(canon.canonical_json(doc) + "\n")
    return doc
