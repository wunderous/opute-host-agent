"""Compile isolated processes using actual evidence projection and stores."""
from __future__ import annotations

import os
import shutil
import subprocess
from pathlib import Path

from . import runner, state_process

GO_FIXTURE = runner.FIXTURE_DIR / "m5/evidence-writer_test.go"
RUST_FIXTURE = runner.REPO_ROOT / "crates/host-agent/src/evidence_fixture.rs"
RUNTIME_SOURCES = tuple("crates/host-agent/src/" + name + ".rs" for name in
                        ["tools", "tasks", "catalog", "app", "evidence"])


def build() -> dict:
    parent = state_process.build()
    directory = runner.REPO_ROOT / ".parity/m5-fixture-src" / parent["sourceTree"]
    shutil.copy2(GO_FIXTURE, directory / "internal/hostmcp/parity_evidence_test.go")
    binary = runner.REPO_ROOT / ".parity/bin/go-evidence-fixture"
    subprocess.run(["go", "test", "-c", "-o", str(binary), "./internal/hostmcp"], cwd=directory,
                   env={**os.environ, "GOTOOLCHAIN": "go1.25.4", "CGO_ENABLED": "0"}, check=True)
    sha = runner.sha256_file(binary)
    retained = binary.with_name("go-evidence-fixture-" + sha)
    if not retained.exists():
        shutil.copy2(binary, retained)
    if runner.sha256_file(retained) != sha:
        raise ValueError("retained Go projection executable changed")
    command = list(parent["rust"]["command"])
    command[command.index("state_fixture::crash_writer")] = "tools::evidence_fixture::durable_projection"
    return {"go": {"command": [str(retained), "-test.run=^TestParityDurableProjection$", "-test.v"], "binarySha256": sha},
            "rust": {"command": command, "binarySha256": parent["rust"]["binarySha256"]},
            "storageFixtures": parent, "goFixtureSha256": runner.sha256_file(GO_FIXTURE),
            "rustFixtureSha256": runner.sha256_file(RUST_FIXTURE), "builderSha256": runner.sha256_file(Path(__file__)),
            "projectionSha256": runner.sha256_file(runner.REPO_ROOT / "crates/host-agent/src/evidence.rs"),
            "runtimeSources": {path: runner.sha256_file(runner.REPO_ROOT / path) for path in RUNTIME_SOURCES}}
