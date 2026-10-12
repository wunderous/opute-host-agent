#!/usr/bin/env python3
"""Capture one seam stage (decision D14): the verifier's report and its canary.

  python3 evidence/seam/capture.py <stage>

Writes evidence/seam/<stage>/: report.json (the seam verifier's report for
the working tree), verifier.log, and canary/{patch.json,log,result.json}. The
canary copies the workspace, patches a tool-name match into the dispatch
executor, and requires the seam verifier to fail naming that tool.
"""
import hashlib, json, os, shutil, subprocess, sys, tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
STAGE = ROOT / "evidence/seam" / sys.argv[1]
CANARY_TOOL = "list_operations"
ANCHOR = """    let Some(descriptor) = catalog_snapshot
        .find(name)
        .or_else(|| catalog::internal().tools.iter().find(|d| d.name == name))
    else {
        return not_implemented(name);
    };
"""
PATCH = ANCHOR + f"""    if name == "{CANARY_TOOL}" {{
        return structured_result(json!({{"operations": []}}), None);
    }}
"""
sha = lambda data: hashlib.sha256(data).hexdigest()


def run(cwd, env, target):
    return subprocess.run(["cargo", "test", "--locked", "-p", "opute-host-agent", "seam_verifier", "--", "--nocapture"],
                          cwd=cwd, env={**os.environ, **env, "CARGO_TARGET_DIR": str(target)},
                          capture_output=True, text=True)


def main():
    if STAGE.exists():
        shutil.rmtree(STAGE)
    (STAGE / "canary").mkdir(parents=True)
    report = STAGE / "report.json"
    verified = run(ROOT, {"SEAM_REPORT": str(report)}, ROOT / "target")
    (STAGE / "verifier.log").write_text(verified.stdout + verified.stderr)
    if verified.returncode != 0:
        sys.exit("seam verifier failed on the working tree; see verifier.log")
    with tempfile.TemporaryDirectory() as tmp:
        work = Path(tmp)
        for item in ("Cargo.toml", "Cargo.lock", "crates", "baseline", "parity-manifest.json"):
            source, dest = ROOT / item, work / item
            dest.parent.mkdir(parents=True, exist_ok=True)
            (shutil.copytree if source.is_dir() else shutil.copy2)(source, dest)
        tools = work / "crates/host-agent/src/tools.rs"
        text = tools.read_text()
        if text.count(ANCHOR) != 1:
            sys.exit("canary anchor is not unique in tools.rs")
        tools.write_text(text.replace(ANCHOR, PATCH))
        canary = run(work, {}, ROOT / "target/seam-canary")
    log = canary.stdout + canary.stderr
    (STAGE / "canary/log").write_text(log)
    (STAGE / "canary/patch.json").write_text(json.dumps({"file": "crates/host-agent/src/tools.rs", "old": ANCHOR, "new": PATCH}, indent=1) + "\n")
    red = canary.returncode != 0 and "seam gaps differ" in log and f'"{CANARY_TOOL}"' in log
    (STAGE / "canary/result.json").write_text(json.dumps({
        "tool": CANARY_TOOL, "exitCode": canary.returncode, "red": red,
        "logSha256": sha(log.encode()), "patchSha256": sha((STAGE / "canary/patch.json").read_bytes()),
    }, indent=1) + "\n")
    if not red:
        sys.exit("canary did not turn the seam verifier red")
    print(f"{sys.argv[1]}: report and red canary captured")


if __name__ == "__main__":
    main()
