"""Mutation canaries: deliberately broken Go builds the harness must catch.

Each canary applies one exact source patch to a scratch copy of the pinned Go
tree, builds it with the reference flags, and runs every scenario with the
unpatched Go reference on the left and the canary on the right. The canary is
caught when its named scenario fails, and only scenarios it lists in
mayAlsoFail fail alongside it. A canary that leaves its scenario green is a
harness gap. Patches are never pushed anywhere.
"""

from __future__ import annotations

import json
import shutil
import subprocess
import tempfile
from pathlib import Path

from . import agent, canon, runner

CANARY_FILE = runner.TOOLS_DIR / "canaries.json"


def build_canary(src: Path, canary: dict, workdir: Path, version: str) -> tuple[Path | None, str]:
    tree = workdir / "src"
    subprocess.run(["git", "-C", str(src), "worktree", "add", "--detach", "-f", str(tree), "HEAD"],
                   check=True, capture_output=True)
    try:
        target = tree / canary["patch"]["file"]
        text = target.read_text()
        count = text.count(canary["patch"]["old"])
        if count != 1:
            return None, f"patch anchor matched {count} times"
        target.write_text(text.replace(canary["patch"]["old"], canary["patch"]["new"]))
        out = workdir / "canary-bin"
        build = subprocess.run(
            ["go", "build", "-trimpath", "-buildvcs=false",
             f"-ldflags=-s -w -buildid= -X github.com/wunderous/host-agents/internal/version.Version={version}",
             "-o", str(out), "./cmd/opute-host-agent"],
            cwd=tree, env={**__import__("os").environ, "CGO_ENABLED": "0"},
            capture_output=True, text=True)
        if build.returncode != 0:
            return None, "build failed: " + build.stderr[-400:]
        return out, "ok"
    finally:
        subprocess.run(["git", "-C", str(src), "worktree", "remove", "--force", str(tree)],
                       capture_output=True)


def run(go_binary: Path, src: Path, out_path: Path, workers: int = 6) -> dict:
    lock = json.loads((runner.REPO_ROOT / "baseline" / "source-lock.json").read_text())
    version = lock["publishedPackage"].rsplit("@", 1)[-1]
    canaries = json.loads(CANARY_FILE.read_text())
    scenarios = runner.load_scenarios()
    left = agent.Impl(label="go", binary=go_binary)
    results = []
    for canary in canaries:
        work = Path(tempfile.mkdtemp(prefix=f"canary-{canary['id']}-"))
        try:
            binary, status = build_canary(src, canary, work, version)
            entry = {"id": canary["id"], "patchApplied": binary is not None, "buildStatus": status}
            if binary is not None:
                right = agent.Impl(label="canary", binary=binary)
                summary = runner.run_suite(left, right, f"canary-{canary['id']}", work / "evidence",
                                           scenarios, repeat=1, workers=workers, source_lock=lock)
                entry["failedScenarios"] = sorted(k for k, v in summary["items"].items() if v["status"] != "pass")
                entry["canaryBinarySha256"] = right.sha256()
                entry["caught"] = canary["scenario"] in entry["failedScenarios"]
            results.append(entry)
            print(f"{canary['id']}: {'caught' if entry.get('caught') else 'NOT CAUGHT'} "
                  f"failed={entry.get('failedScenarios')} ({status})")
        finally:
            shutil.rmtree(work, ignore_errors=True)
    doc = {
        "schemaVersion": 1,
        "sourceCommit": lock["sourceCommit"],
        "goBinarySha256": left.sha256(),
        "harnessSha256": runner.harness_sha256(),
        "canaryFileSha256": runner.sha256_file(CANARY_FILE),
        "results": results,
    }
    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(canon.canonical_json(doc) + "\n")
    return doc
