"""Build test-only processes that call the actual pinned storage methods.

The Go seam lives in an owned source copy; Rust's seam is an ignored test in
the normal crate. Neither seam adds a production command or endpoint.
"""
from __future__ import annotations

import json
import os
import shutil
import subprocess
from pathlib import Path

from . import runner

GO_SOURCE = runner.REPO_ROOT / ".parity/go-src"
GO_FIXTURE = runner.FIXTURE_DIR / "m5/crash-writer.go"
RUST_FIXTURE = runner.REPO_ROOT / "crates/host-agent/src/state_fixture.rs"


def build() -> dict:
    lock = json.loads((runner.REPO_ROOT / "baseline/source-lock.json").read_text())
    commit = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=GO_SOURCE, text=True).strip()
    tree = subprocess.check_output(["git", "rev-parse", "HEAD^{tree}"], cwd=GO_SOURCE, text=True).strip()
    dirty = subprocess.check_output(["git", "status", "--porcelain"], cwd=GO_SOURCE, text=True)
    if commit != lock["sourceCommit"] or tree != lock["sourceTree"] or dirty:
        raise ValueError("Go fixture source is not the clean pinned tree")
    directory = runner.REPO_ROOT / ".parity/m5-fixture-src" / tree
    marker = directory / ".parity-fixture-owner.json"
    owner = {"sourceCommit": commit, "sourceTree": tree}
    if directory.exists() and (not marker.exists() or json.loads(marker.read_text()) != owner):
        raise ValueError("fixture copy lacks matching ownership marker")
    directory.mkdir(parents=True, exist_ok=True)
    marker.write_text(json.dumps(owner))
    paths = subprocess.check_output(["git", "ls-files", "-z"], cwd=GO_SOURCE).decode().split("\0")
    for relative in filter(None, paths):
        destination = directory / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        if (GO_SOURCE / relative).is_symlink() and destination.is_symlink():
            if os.readlink(GO_SOURCE / relative) != os.readlink(destination):
                raise ValueError("owned source copy has a changed symbolic link")
            continue
        shutil.copy2(GO_SOURCE / relative, destination, follow_symlinks=False)
    main = directory / "internal/parity-state-fixture/main.go"
    main.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(GO_FIXTURE, main)
    go_binary = runner.REPO_ROOT / ".parity/bin/go-state-fixture"
    env = {**os.environ, "GOTOOLCHAIN": "go1.25.4", "CGO_ENABLED": "0"}
    subprocess.run(["go", "build", "-trimpath", "-buildvcs=false", "-ldflags=-s -w -buildid=",
                    "-o", str(go_binary), "./internal/parity-state-fixture"], cwd=directory, env=env, check=True)
    built = subprocess.run(["cargo", "test", "--release", "--no-run", "--message-format=json"],
                           cwd=runner.REPO_ROOT, check=True, text=True, stdout=subprocess.PIPE)
    artifacts = [json.loads(line) for line in built.stdout.splitlines() if line.startswith("{")]
    executables = [Path(item["executable"]) for item in artifacts
                   if item.get("reason") == "compiler-artifact" and item.get("executable")
                   and item.get("profile", {}).get("test") and item.get("target", {}).get("name") == "opute-host-agent"]
    if len(executables) != 1:
        raise ValueError("Rust storage test process not uniquely built")
    rust_binary = executables[0]
    # Cargo can replace its test executable during later fixture builds.
    # Evidence must retain the exact executable that performed the writes.
    rust_sha = runner.sha256_file(rust_binary)
    retained = runner.REPO_ROOT / ".parity/bin" / ("rust-state-fixture-" + rust_sha)
    if retained.exists() and runner.sha256_file(retained) != rust_sha:
        raise ValueError("retained fixture executable changed")
    if not retained.exists():
        shutil.copy2(rust_binary, retained)
    rust_binary = retained
    return {"go": {"command": [str(go_binary)], "binarySha256": runner.sha256_file(go_binary)},
            "rust": {"command": [str(rust_binary), "--exact", "state_fixture::crash_writer", "--ignored", "--nocapture", "--test-threads=1"],
                     "binarySha256": runner.sha256_file(rust_binary)},
            "sourceCommit": commit, "sourceTree": tree,
            "goFixtureSha256": runner.sha256_file(GO_FIXTURE), "rustFixtureSha256": runner.sha256_file(RUST_FIXTURE),
            "builderSha256": runner.sha256_file(Path(__file__)),
            "rustStoreSha256": runner.sha256_file(runner.REPO_ROOT / "crates/host-agent/src/store.rs"),
            "goStoreSha256": runner.sha256_file(GO_SOURCE / "internal/state/store.go")}
