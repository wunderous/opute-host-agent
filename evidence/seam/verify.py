#!/usr/bin/env python3
"""Reproduce a seam stage's receipt (decision D14).

  python3 evidence/seam/verify.py <stage> [--rerun]

Checks evidence/seam/<stage>/ and writes evidence/seam/<stage>-validation.json:
the report is internally consistent (every capability classified, the gap
list equals the capabilities classified as gaps, every compliant capability
has a service and a consumer, no problems), the verifier passed on that
tree, and the canary turned it red naming its tool, with matching hashes.
With --rerun, the verifier runs again on the working tree and must
reproduce report.json byte for byte, and the gap list must equal
crates/host-agent/seam-gaps.json.
"""
import hashlib, json, os, subprocess, sys, tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
stage = sys.argv[1]
STAGE = ROOT / "evidence/seam" / stage
sha = lambda path: hashlib.sha256(Path(path).read_bytes()).hexdigest()

report = json.loads((STAGE / "report.json").read_text())
assert report["schemaVersion"] == "opute-seam-report.v1" and report["decision"] == "D14"
assert report["provider"] == "host-agent:incus" and report["generation"] == "builtin"
assert report["problems"] == [], report["problems"]
capabilities = report["capabilities"]
assert sum(report["counts"].values()) == len(capabilities) == 150, report["counts"]
statuses = {name: entry["status"] for name, entry in capabilities.items()}
assert set(statuses.values()) <= {"compliant", "gap", "unported"}
assert report["gaps"] == sorted(n for n, s in statuses.items() if s == "gap")
offered = {op for service in report["services"] for op in service["operations"]}
for name, entry in capabilities.items():
    if entry["status"] == "compliant":
        assert name in offered and entry["service"].startswith("host-agent:incus/") and entry["consumers"], name
    else:
        assert name not in offered, name
    if entry["status"] == "gap":
        assert entry["reachedBy"], name
assert "test seam::seam_verifier ... ok" in (STAGE / "verifier.log").read_text()

canary = json.loads((STAGE / "canary/result.json").read_text())
log = (STAGE / "canary/log").read_text()
assert canary["red"] and canary["exitCode"] != 0
assert canary["logSha256"] == sha(STAGE / "canary/log")
assert canary["patchSha256"] == sha(STAGE / "canary/patch.json")
assert "seam gaps differ" in log and f'"{canary["tool"]}"' in log
assert statuses[canary["tool"]] == "unported", "the canary must target a capability no service offers"

rerun = "--rerun" in sys.argv[2:]
if rerun:
    with tempfile.TemporaryDirectory() as tmp:
        out = Path(tmp) / "report.json"
        done = subprocess.run(["cargo", "test", "--locked", "-p", "opute-host-agent", "seam_verifier"],
                              cwd=ROOT, env={**os.environ, "SEAM_REPORT": str(out)}, capture_output=True, text=True)
        assert done.returncode == 0, done.stdout + done.stderr
        assert out.read_bytes() == (STAGE / "report.json").read_bytes(), "rerun report differs"
    assert json.loads((ROOT / "crates/host-agent/seam-gaps.json").read_text()) == report["gaps"]

receipt = {
    "stage": stage,
    "reportSha256": sha(STAGE / "report.json"),
    "verifierLogSha256": sha(STAGE / "verifier.log"),
    "counts": report["counts"],
    "gaps": report["gaps"],
    "services": [s["key"] for s in report["services"]],
    "canary": {"tool": canary["tool"], "red": True, "logSha256": canary["logSha256"], "patchSha256": canary["patchSha256"]},
}
(ROOT / "evidence/seam" / f"{stage}-validation.json").write_text(json.dumps(receipt, indent=1) + "\n")
print(f"{stage}: {report['counts']}, {len(report['gaps'])} gap(s), canary red" + (", rerun reproduced" if rerun else ""))
