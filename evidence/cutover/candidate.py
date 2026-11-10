#!/usr/bin/env python3
"""Record the release candidate every gate and receipt must name (m13b).

  python3 evidence/cutover/candidate.py --release DIR --upgrade-target DIR

Writes evidence/cutover/candidate.json from a locally built release set
(tools/release/build-artifacts.sh; no tag, no publish):

  name, version   the set's directory name and what its x64 binary reports
  commit          the commit it was built from (DIR.commit, written at build)
  sha256sums      the hash of its SHA256SUMS file
  artifacts       every file SHA256SUMS lists, verified against it
  upgradeTarget   the newer local build of the same sources that the
                  in-place upgrade journeys install
  catalogRevision the served catalog revision pinned in parity-manifest.json
  sourceDigest    the sources it was built from, as readiness.py digests them

`make cutover-readiness` (kind `candidate`) fails unless every gate, the
contract suites and the arm64 install ran this set and the sources are
unchanged since.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import readiness  # noqa: E402

RECORD = readiness.ROOT / "evidence/cutover/candidate.json"


def release_set(directory: Path) -> dict:
    sums = directory / "SHA256SUMS"
    artifacts = {}
    for line in sums.read_text().splitlines():
        digest, name = line.split()
        actual = hashlib.sha256((directory / name).read_bytes()).hexdigest()
        if actual != digest:
            raise SystemExit(f"refused: {directory / name} does not match its SHA256SUMS")
        artifacts[name] = digest
    version = subprocess.run([str(directory / "host-agent-linux-x64"), "--version"], capture_output=True, text=True,
                             check=True).stdout.strip()
    commit = Path(str(directory) + ".commit").read_text().strip()
    return {"name": directory.name, "version": version, "commit": commit,
            "sha256sums": readiness.sha256_file(sums), "artifacts": artifacts}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--release", type=Path, required=True)
    parser.add_argument("--upgrade-target", type=Path, required=True)
    args = parser.parse_args()
    manifest = json.loads((readiness.ROOT / readiness.MANIFEST).read_text())
    item = next(i for i in manifest["items"] if i["kind"] == "candidate")
    pinned = json.loads((readiness.ROOT / "parity-manifest.json").read_text())["catalogRevisions"]
    record = {"schemaVersion": "opute-release-candidate.v1", **release_set(args.release),
              "upgradeTarget": release_set(args.upgrade_target),
              "catalogRevision": {mode: pinned[mode]["rust"] for mode in ("standalone", "platform")},
              "sourceDigest": readiness.source_digest(readiness.ROOT, item["sources"]),
              "built": "locally with tools/release/build-artifacts.sh; no tag, no publish"}
    if record["commit"] != record["upgradeTarget"]["commit"]:
        raise SystemExit("refused: the upgrade target was built from another commit")
    RECORD.write_text(json.dumps(record, indent=1) + "\n")
    print(json.dumps({k: record[k] for k in ("name", "version", "commit", "sha256sums")}))
    return 0


if __name__ == "__main__":
    sys.exit(main())
