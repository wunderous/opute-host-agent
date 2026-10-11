"""Pin the open unadmitted-callback-task divergence and write its receipt.

Recomputes the five-iteration Go/Rust bundle from raw observations and
requires exactly the difference described in
openspec/changes/unadmitted-callback-task-record: Go persists a delegated
child before admission and restores it cancelled after a shutdown before
acknowledgement; Rust (D12) keeps nothing. Every other compared path must be
equal. This is not Go parity and is not reported as such.
"""
import gzip, hashlib, json, subprocess, sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / 'tools/parity'))
from parity import canon, runner

HERE = Path(__file__).resolve().parent
sha = lambda path: hashlib.sha256(Path(path).read_bytes()).hexdigest()
load = lambda path: json.loads(Path(path).read_text())
raw = lambda path: json.loads(gzip.decompress(Path(path).read_bytes()))
lock = load(ROOT / 'baseline/source-lock.json')
go, rust = ROOT / '.parity/bin/go-reference', ROOT / 'target/release/opute-host-agent'
source = ROOT / 'tools/parity/open-divergences/unadmitted-callback-task.json'
generated = subprocess.check_output([sys.executable, str(ROOT / 'tools/parity/provider_lifecycle_scenarios.py'), '/dev/null', '/dev/stdout'], cwd=ROOT / 'tools/parity')
assert json.loads(generated) == load(source)
[scenario] = load(source)
scenario_sha = runner.sha256_bytes(canon.canonical_json(scenario).encode())
name = scenario['id']

summary = load(HERE / 'capture/summary.json')
prov = summary['provenance']
assert not prov['changedDuringRun'] and prov['harnessSha256'] == runner.harness_sha256()
assert prov['sourceCommit'] == lock['sourceCommit'] and prov['sourceTree'] == lock['sourceTree']
assert prov['left']['binarySha256'] == sha(go) and prov['right']['binarySha256'] == sha(rust)
item = summary['items'][name]
assert list(summary['items']) == [name] and item['scenarioSha256'] == scenario_sha
assert item['status'] == 'fail' and item['iterations'] == 5 and item['failingIterations'] == [0, 1, 2, 3, 4]
capture = HERE / 'capture/scenarios' / name
for path, digest in item['files'].items():
    assert sha(capture / path) == digest, path

spec = scenario['compare']
a, b = raw(capture / 'a.json.gz'), raw(capture / 'b.json.gz')
norm_a, viol_a = canon.normalize(a['observation'], spec, a['variables'])
norm_b, viol_b = canon.normalize(b['observation'], spec, b['variables'])
viol_a += runner.invariant_violations(scenario, a['observation'], 'go')
viol_b += runner.invariant_violations(scenario, b['observation'], 'rust')
norm_a, norm_b = runner._reconcile_optional_masks(norm_a, norm_b, spec.get('masks', []))
diff = canon.diff(norm_a, norm_b)
assert not viol_a and not viol_b and load(capture / 'diff.json')['diff'] == diff
assert sorted(tuple(d['path']) for d in diff) == sorted([
    ('steps', 'childRestored', 'body', 'error'), ('steps', 'childRestored', 'body', 'result'),
    ('steps', 'state', 'operations'), ('steps', 'stateRestored', 'operations')])
iterations = load(capture / 'iterations.json')
assert len(iterations) == 5 and all(row['diffCount'] == 4 and not row['maskViolations'] for row in iterations)

go_steps, rust_steps = a['observation']['steps'], b['observation']['steps']
restored = go_steps['childRestored']['body']['result']
assert restored['status'] == 'cancelled'
assert restored['statusMessage'] == 'The Host Agent shut down before the callback task handle was acknowledged.'
assert rust_steps['childRestored']['body']['error']['message'] == 'task not found: ' + b['variables']['CHILD']
child = {'tool_name': 'configure_network', 'status': 'working', 'error_message': None}
assert go_steps['state']['operations'] == [{'tool_name': 'opute.capability.fixture.bridge', 'status': 'working', 'error_message': None}, child]
assert rust_steps['state']['operations'] == go_steps['state']['operations'][:1]
assert [r['tool_name'] for r in go_steps['stateRestored']['operations']] == ['opute.capability.fixture.bridge', 'configure_network']
assert rust_steps['stateRestored']['operations'] == go_steps['stateRestored']['operations'][:1]
# The provider received the same working task handle on both sides.
for steps in (go_steps, rust_steps):
    handle = steps['bridged0']['body']['result']['result']['structuredContent']['callback']
    assert handle['status'] == 200 and handle['body']['result']['status'] == 'working'

receipt = {
    'status': 'open divergence, pending owner decision; not Go parity',
    'proposal': 'openspec/changes/unadmitted-callback-task-record',
    'scenario': name, 'iterations': 5, 'differences': [list(d['path']) for d in diff],
    'go': 'persists the delegated child operation before admission (internal/hostmcp/server.go:1642-1647); after shutdown before acknowledgement, restart restores it cancelled',
    'rust': 'D12 admit-before-record: no row for the unadmitted child; after restart tasks/get reports task not found',
    'sourceLock': lock, 'rustReleaseSha256': sha(rust), 'goReferenceSha256': sha(go), 'harnessSha256': runner.harness_sha256(),
    'scenarioSha256': scenario_sha, 'summarySha256': sha(HERE / 'capture/summary.json'),
    'verificationScriptSha256': sha(Path(__file__)), 'captureScriptSha256': sha(HERE / 'capture.py'),
}
(ROOT / 'evidence/m7/unadmitted-callback-task-validation.json').write_text(json.dumps(receipt, indent=2) + '\n')
print(json.dumps(receipt, indent=2))
