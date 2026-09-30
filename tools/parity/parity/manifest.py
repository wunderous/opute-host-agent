"""Keep parity-manifest.json in step with the scenarios and source lock.

The gates and evidence sections are reviewed by hand. `sync` only rewrites
the derived parts: one item per scenario (with its surface and owner) and the
source-lock hash. It never marks anything as passed.
"""

from __future__ import annotations

import hashlib
import json
from pathlib import Path

from . import canon, runner

MANIFEST = runner.REPO_ROOT / "parity-manifest.json"


def sync(go_binary_sha256: str | None = None) -> dict:
    manifest = json.loads(MANIFEST.read_text())
    lock_path = runner.REPO_ROOT / manifest["sourceLock"]
    manifest["sourceLockSha256"] = hashlib.sha256(lock_path.read_bytes()).hexdigest()
    if go_binary_sha256:
        manifest["goReference"]["binarySha256"] = go_binary_sha256
    manifest["items"] = [
        {"id": s["id"], "scenario": s["id"], "surface": s["surface"], "owner": s["owner"],
         "anchors": s.get("anchors", [])}
        for s in runner.load_scenarios()
    ]
    MANIFEST.write_text(canon.canonical_json(manifest) + "\n")
    return manifest
