"""Capture the previous release's state schema from its isolated binary.
Requires a clean .parity/legacy-v0.2.2 checkout of the pinned release commit
and .parity/bin/go-v0.2.2 built with Go 1.25.4. No tool is called and only
the disposable state schema, source revision and binary hashes are retained.
Run from the repository root, with TMPDIR on a filesystem with free space.
"""
import sys, json, subprocess, shutil
from pathlib import Path
sys.path.insert(0, str(Path.cwd()/"tools/parity"))
from parity import agent, runner
source=Path.cwd()/".parity/legacy-v0.2.2"
commit=subprocess.check_output(["git","-C",str(source),"rev-parse","HEAD"],text=True).strip()
assert commit=="9f2a18993b0f268f393483a5e098124a276fa25c"
assert not subprocess.check_output(["git","-C",str(source),"status","--porcelain"],text=True)
impl=agent.Impl("legacy",Path.cwd()/".parity/bin/go-v0.2.2")
sandbox=agent.Sandbox("legacy","release-fixture",None)
server=None
try:
 server=agent.Server(impl,sandbox,["serve","--mode=standalone"],runner.PROFILES["standalone"])
 assert server.wait_ready(30).get("ready")
 schema=sandbox.sqlite_schemas()["state/state.db"]
 assert server.stop()=={"exit":0,"killed":False}
 doc={"release":"v0.2.2","sourceRepository":"https://github.com/wunderous/host-agents",
      "sourceCommit":commit,"sourceTree":subprocess.check_output(["git","-C",str(source),"rev-parse","HEAD^{tree}"],text=True).strip(),
      "storeSourceSha256":runner.sha256_file(source/"internal/state/store.go"),
      "binarySha256":impl.sha256(),"toolchain":"go1.25.4","schema":schema}
 dest=Path.cwd()/"tools/parity/fixtures/m5/v0.2.2-state-schema.json"
 dest.parent.mkdir(parents=True,exist_ok=True); dest.write_text(json.dumps(doc,indent=2,sort_keys=True)+"\n")
 print("captured",len(schema["rowCounts"]),"state tables from",commit)
finally:
 if server: server.stop()
 shutil.rmtree(sandbox.root,ignore_errors=True)
