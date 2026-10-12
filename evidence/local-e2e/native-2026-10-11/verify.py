"""Recompute the 2026-10-11 native validation receipt (D14 final cycle).

Reproduces evidence/local-e2e/native-2026-10-11/validation.json from the raw
captures: the committed head and clean tree before and after, the binaries
the manifest records, every summary's provenance, the logs of the standard
checks, every milestone gate report (rerun and compared byte for byte), and
the attribution of the M5 shape failures to the pre-seam build. Failures are
recorded as failures; nothing is relabeled.
"""
import gzip, hashlib, json, re, subprocess, sys, tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
sys.path.insert(0, str(ROOT / 'tools/parity'))
from parity import runner

sha = lambda path: hashlib.sha256(Path(path).read_bytes()).hexdigest()
load = lambda path: json.loads(Path(path).read_text())
raw = lambda path: json.loads(gzip.decompress(Path(path).read_bytes()))
log = lambda name: (HERE / 'checks' / name).read_text()
lock = load(ROOT / 'baseline/source-lock.json')
manifest = load(ROOT / 'parity-manifest.json')
go, rust = ROOT / '.parity/bin/go-reference', ROOT / 'target/release/opute-host-agent'
EMPTY = hashlib.sha256(b'').hexdigest()

head = (HERE / 'checks/head').read_text().strip()
assert subprocess.run(['git', '-C', str(ROOT), 'merge-base', '--is-ancestor', head, 'HEAD']).returncode == 0
assert (HERE / 'checks/tree.before.sha256').read_text().split()[0] == EMPTY
assert (HERE / 'checks/tree.after.sha256').read_text().split()[0] == EMPTY
binaries = dict(reversed(line.split()) for line in (HERE / 'checks/binaries.sha256').read_text().splitlines())
assert binaries[str(rust)] == sha(rust) == manifest['rustCandidate']['binarySha256']
assert binaries[str(go)] == sha(go) == manifest['goReference']['binarySha256']

# Standard checks.
assert not log('fmt.log').strip()
assert 'warning' not in log('clippy.log') and 'Finished' in log('clippy.log')
tests = [tuple(map(int, m)) for m in re.findall(r'test result: ok\. (\d+) passed; (\d+) failed', log('workspace.log'))]
assert tests and not any(failed for _, failed in tests) and 'FAILED' not in log('workspace.log')
assert log('harness.log').rstrip().endswith('OK')
assert 'Totals: 12 passed, 0 failed' in log('spec.log')
assert 'catalog source is current' in log('catalog-source.log')
assert 'go reference rebuilt identically' in log('go-reference.log')
assert 'FAIL' not in log('go-owner-m7.log') and 'FAIL' not in log('go-owner-core.log')
assert log('go-owner-m7.log').count('--- PASS') == 27 * 5

# Differential captures and harness self-checks.
def differential(name, repeat, right):
    summary = load(HERE / name / 'summary.json')
    prov = summary['provenance']
    assert not prov['changedDuringRun'] and prov['repeat'] == repeat
    assert prov['sourceCommit'] == lock['sourceCommit'] and prov['sourceTree'] == lock['sourceTree']
    assert prov['left']['binarySha256'] == sha(go) and prov['right']['binarySha256'] == sha(right)
    assert sorted(summary['items']) == sorted(item['id'] for item in manifest['items'])
    return {status: sum(1 for item in summary['items'].values() if item['status'] == status)
            for status in sorted({item['status'] for item in summary['items'].values()})}

caught = lambda name: len(re.findall(r'\(ok\)$', log(name), re.M))
contracts = {}
for suite in ('oauth-issuance', 'standalone-read-only-gate', 'reject-without-audit-writes',
              'refuse-before-operation-record', 'evidence-redaction'):
    line = log(f'contract-{suite}.log').strip().splitlines()[-1]
    passed, total = map(int, re.match(r'(\d+)/(\d+) contract scenarios pass', line).groups())
    contracts[suite] = {'passed': passed, 'total': total}

m5 = {name: {k: v for k, v in load(HERE / 'm5' / name / 'summary.json').items() if not isinstance(v, (dict, list))}
      for name in ('crash', 'cross-read', 'migration', 'secrets', 'unknown-projection', 'shape-final')}

# Gate reports: rerun each and require the committed report byte for byte.
gates = {}
with tempfile.TemporaryDirectory() as tmp:
    for gate in ('m0', 'm1', 'm2', 'm3', 'm4', 'm5', 'm6', 'cutover'):
        out = Path(tmp) / f'{gate}.json'
        code = subprocess.run([sys.executable, '-m', 'parity', 'verify', '--gate', gate, '--report', str(out)],
                              cwd=ROOT / 'tools/parity', capture_output=True).returncode
        assert out.read_bytes() == (HERE / f'verify-{gate}.json').read_bytes(), gate
        report = load(out)
        gates[gate] = {'pass': code == 0, 'problems': len(report['problems']),
                       'problemKinds': sorted({p.split(']')[0].lstrip('[') for p in report['problems']})}
        if gate == 'cutover':
            gates[gate]['inventoryGaps'] = int(re.search(r'inventory gaps remain: (\d+)', log('verify-cutover.log')).group(1))

# M5 shape failures: the same runs fail with the pre-seam build (598d016d).
def failing(folder):
    outcomes = raw(HERE / 'm5' / folder / 'outcomes.json.gz')
    return outcomes, {name for name, row in outcomes.items()
                      if row['diff'] or row['maskViolations']['a'] or row['maskViolations']['b']}

final, final_failing = failing('shape-final')
pre, pre_failing = failing('shape-preseam')
pre_summary = load(HERE / 'm5/shape-preseam/summary.json')
assert pre_summary['provenance']['rustBinarySha256'] == (HERE / 'm5/shape-preseam/rust.sha256').read_text().strip()
assert not pre_summary['provenance']['changedDuringRun']
preseam_commit = (HERE / 'm5/shape-preseam/commit').read_text().strip()
assert subprocess.check_output(['git', '-C', str(ROOT), 'rev-parse', '598d016d'], text=True).strip() == preseam_commit
covered_before = {name.rsplit('.', 1)[0] for name in raw(ROOT / 'evidence/local-e2e/native-2026-10-05/m5/shape-final/outcomes.json.gz')}
failing_scenarios = sorted({name.rsplit('.', 1)[0] for name in final_failing})
assert final_failing == pre_failing
assert not covered_before & set(failing_scenarios)
assert all(name.startswith(('provider-lifecycle.', 'provider-real.')) for name in failing_scenarios)
paths = sorted({'/'.join(str(p) for p in d['path'][:4] + (d['path'][5:6] if d['path'][3] != 'resource_registry' and len(d['path']) > 5 else []))
                for name in final_failing for d in final[name]['diff']})

receipt = {
    'schemaVersion': 'opute-native-validation.v1',
    'head': head,
    'binaries': {'go': sha(go), 'rust': sha(rust)},
    'workspaceTests': {'passed': sum(p for p, _ in tests), 'failed': 0},
    'goVsGo': differential('go-vs-go-final', 20, go),
    'goVsRust': differential('go-vs-rust-final', 5, rust),
    'canaries': {'go': caught('canaries.log'), 'rust': caught('rust-canaries.log')},
    'contracts': contracts,
    'm5': m5,
    'gates': gates,
    'shapeAttribution': {
        'preseamCommit': preseam_commit,
        'failingRuns': len(final_failing),
        'failingScenarios': failing_scenarios,
        'identicalWithPreseamBuild': True,
        'previouslyCoveredScenarios': len(covered_before),
        'previouslyCoveredFailing': 0,
        'diffPaths': paths,
    },
}
(HERE / 'validation.json').write_text(json.dumps(receipt, indent=2, sort_keys=True) + '\n')
print(json.dumps({k: receipt[k] for k in ('goVsGo', 'goVsRust', 'canaries')}),
      {g: v['pass'] for g, v in gates.items()}, f"shape failing runs {len(final_failing)} (pre-seam identical)")
