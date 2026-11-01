"""Recompute the M12b (M12 validation 5, soak) validation receipt from the raw captures.

  python3 evidence/m12b/verify.py

Reproduces evidence/m12b/validation.json. It covers the committed heads and a
clean tree around both phases; the manifest's Go reference and Rust candidate
before and after; the harness log; both shakedowns' provenance; the declared
thresholds re-derived byte for byte from the Go/Go shakedown's raw receipts
and committed before the soak started (the soak's head contains them and its
provenance records their hash); the soak re-derived from its raw samples
(soak_verify) with every check passing; the Go/Rust shakedown evaluated
against the same thresholds; the inventory gap ownership; the seam stage;
and every milestone gate report rerun and compared byte for byte. The m12a
suites are not rerun: the binaries and harness are unchanged, and the gates
re-derive them from the committed m12a receipts. Failures are recorded as
failures; nothing is relabelled.
"""
import hashlib, json, subprocess, sys, tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
sys.path.insert(0, str(ROOT / 'tools/parity'))
from parity import soak, soak_verify, t2  # noqa: E402

sha = lambda path: hashlib.sha256(Path(path).read_bytes()).hexdigest()
load = lambda path: json.loads(Path(path).read_text())
log = lambda name: (HERE / 'checks' / f'{name}.log').read_text()
lock = load(ROOT / 'baseline/source-lock.json')
manifest = load(ROOT / 'parity-manifest.json')
go, rust = ROOT / '.parity/bin/go-reference', ROOT / 'dist/host-agent-linux-x64'
thresholds_path = ROOT / 'tools/parity/soak-thresholds.json'
EMPTY_TREE = hashlib.sha256(b'').hexdigest()
git = lambda *args: subprocess.run(['git', '-C', str(ROOT), *args], capture_output=True, text=True)

heads = {phase: (HERE / f'checks/{name}').read_text().strip()
         for phase, name in (('shakedown', 'head.shakedown'), ('soak', 'head'))}
for head in heads.values():
    assert git('merge-base', '--is-ancestor', head, 'HEAD').returncode == 0, head
for name in ('tree.shakedown.before', 'tree.shakedown.after', 'tree.before'):
    assert (HERE / f'checks/{name}.sha256').read_text().split()[0] == EMPTY_TREE, name
# The shakedown commit tracked checks/exits.log, so the soak phase's own
# appended exit lines are the one change its tree.after can show. Rebuild
# that exact state at the soak head and require the recorded hash.
SOAK_EXITS = 'soak exit=0\nsoak-evaluate exit=0\nseam exit=0\n'
with tempfile.TemporaryDirectory() as tmp:
    work = Path(tmp) / 'tree'
    assert git('worktree', 'add', '-q', '--detach', str(work), heads['soak']).returncode == 0
    try:
        with open(work / 'evidence/m12b/checks/exits.log', 'a') as fh:
            fh.write(SOAK_EXITS)
        state = subprocess.run('{ git status --porcelain --untracked-files=no; git diff HEAD; } | sha256sum', shell=True,
                               cwd=work, capture_output=True, text=True).stdout.split()[0]
    finally:
        git('worktree', 'remove', '--force', str(work))
assert (HERE / 'checks/tree.after.sha256').read_text().split()[0] == state, 'tree.after'
assert log('soak') and (HERE / 'checks/exits.log').read_text().count(SOAK_EXITS) == 1
for name in ('binaries.shakedown', 'binaries', 'binaries.after'):
    pinned = dict(reversed(line.split()) for line in (HERE / f'checks/{name}.sha256').read_text().splitlines())
    assert pinned[str(rust)] == sha(rust) == manifest['rustCandidate']['binarySha256'], name
    assert pinned[str(go)] == sha(go) == manifest['goReference']['binarySha256'], name
exits = dict(line.split(' exit=') for line in (HERE / 'checks/exits.log').read_text().splitlines())
assert exits['harness'] == '0' and log('harness').rstrip().endswith('OK')

# Thresholds: declared from the Go/Go shakedown and committed before the soak.
spec = manifest['soakEvidence']
assert spec['thresholdsSha256'] == sha(thresholds_path)
assert (HERE / 'checks/thresholds.sha256').read_text().split()[0] == sha(thresholds_path)
committed = git('show', f"{heads['soak']}:tools/parity/soak-thresholds.json")
assert committed.returncode == 0 and hashlib.sha256(committed.stdout.encode()).hexdigest() == sha(thresholds_path)
assert git('merge-base', '--is-ancestor', heads['shakedown'], heads['soak']).returncode == 0
shakedowns = {}
for name, right in (('shakedown-go-go', go), ('shakedown-go-rust', rust)):
    summary = load(HERE / name / 'summary.json')
    prov = summary['provenance']
    assert exits[name] == '0' and summary['crashed'] is False and prov['changedDuringRun'] is False, name
    assert prov['sourceLock'] == lock and prov['golden'] == t2.golden_record()['goldenSha256'], name
    assert prov['left']['binarySha256'] == sha(go) and prov['right']['binarySha256'] == sha(right), name
    assert prov['minutes'] <= 30, name
    for file, digest in summary['files'].items():
        assert sha(HERE / name / file) == digest, (name, file)
    shakedowns[name] = {'minutes': prov['minutes'], 'seed': prov['config']['seed']}
derived = soak.derive_thresholds(soak.evaluate(HERE / 'shakedown-go-go', None), 'evidence/m12b/shakedown-go-go')
assert json.dumps(derived, indent=1, sort_keys=True) + '\n' == thresholds_path.read_text(), 'thresholds do not re-derive'
thresholds = load(thresholds_path)
trial = soak.evaluate(HERE / 'shakedown-go-rust', thresholds)
assert load(HERE / 'shakedown-go-rust/evaluation.json') == json.loads(json.dumps(trial))
shakedowns['shakedown-go-rust']['checks'] = {k: v['pass'] for k, v in trial['checks'].items()}

# The soak, re-derived from its raw samples.
assert exits['soak'] == '0' and exits['soak-evaluate'] == '0'
problems = []
soak_status = soak_verify.check(ROOT, manifest, lock, problems.append)
assert not problems, problems
assert soak_status and all(v == 'pass' for v in soak_status.values()), soak_status
evaluation = load(HERE / 'soak/evaluation.json')
summary = load(HERE / 'soak/summary.json')
assert summary['provenance']['minutes'] >= spec['minMinutes'] and summary['provenance']['minutes'] <= 30
assert summary['provenance']['config']['seed'] not in {s['seed'] for s in shakedowns.values()}
assert load(HERE / 't2-golden.json')['goldenSha256'] == summary['provenance']['golden']
soak_receipt = {
    'status': soak_status,
    'minutes': summary['provenance']['minutes'],
    'seed': summary['provenance']['config']['seed'],
    'iterations': evaluation['iterations'],
    'classes': evaluation['classes']['a'],
    'restarts': [r['kind'] for r in evaluation['restarts']['b']],
    'journal': evaluation['journal'],
    'finalState': {k: evaluation['finalState'][k] for k in
                   ('diffCount', 'divergences', 'liveReadingsMasked', 'reloadRevisionPairs')},
    'growth': {side: {k: evaluation['growth'][side][k] for k in ('rssKb', 'fds', 'threads', 'stateBytes',
                                                                  'downSamples', 'epochs')}
               for side in ('a', 'b')},
    'latency': evaluation['latency'],
}

seam = load(ROOT / 'evidence/seam/m12b/report.json')
canary = load(ROOT / 'evidence/seam/m12b/canary/result.json')
assert canary['red'] and exits['seam'] == '0'
assert seam['gaps'] == [] and seam['problems'] == [], (seam['gaps'], seam['problems'])

# Gate reports: rerun each and require the committed report byte for byte.
gates = {}
with tempfile.TemporaryDirectory() as tmp:
    for gate in ('m0', 'm1', 'm2', 'm3', 'm4', 'm5', 'm6', 'm8a', 'm8b', 'm8c', 'm8d', 'm8e', 'm8f', 'm10', 'm12a',
                 'm12b', 'cutover'):
        out = Path(tmp) / f'{gate}.json'
        code = subprocess.run([sys.executable, '-m', 'parity', 'verify', '--gate', gate, '--report', str(out)],
                              cwd=ROOT / 'tools/parity', capture_output=True).returncode
        assert out.read_bytes() == (HERE / f'verify-{gate}.json').read_bytes(), gate
        report = load(out)
        gates[gate] = {'pass': code == 0, 'failures': report['gate'].get('failures', []),
                       'problems': len(report['problems'])}

gaps = load(ROOT / 'baseline/inventory/gaps.json')
assert gaps['unownedCount'] == 0
owned_gaps = {owner: sorted(f"{g['file']}:{g['item']}" for g in gaps['items'] if g.get('owner') == owner)
              for owner in sorted({g.get('owner') for g in gaps['items']})}
# What remains after M12b: the M10 gaps (Windows artifacts, arm64 and WSL2
# installs, the three WSL tools' T2 effects), the M11 dependency (platform),
# the writer lock (D25, waiting on the Go release), the M13 evidence and the
# 0.2.3 publish decision. The soak gap is closed.
assert owned_gaps == {
    'core': ['cutover.json:state-directory writer lock'],
    'host': ['t2.json:compact_wsl_disk', 't2.json:shutdown_wsl', 't2.json:terminate_wsl_distribution'],
    'platform': ['cutover.json:M11 Platform-driven journey', 'cutover.json:M11 client first-success',
                 'cutover.json:M11 staging validation identity'],
    'release': ['cutover.json:M13 cutover evidence', 'cutover.json:stable 0.2.3 publish decision',
                'distribution.json:WSL2 install', 'distribution.json:host-agent-windows-x64.exe',
                'distribution.json:host-agent-windows-x64.exe.gz', 'distribution.json:host-agent-windows-x64.gz',
                'distribution.json:linux-arm64 clean-machine install'],
}, owned_gaps
cutover_items = {i['name']: i['status'] for i in load(ROOT / 'baseline/inventory/cutover.json')['items']}
assert cutover_items.get('soak') == 'covered', cutover_items

receipt = {
    'schemaVersion': 'opute-native-validation.v1',
    'milestone': 'M12b',
    'heads': heads,
    'binaries': {'go': sha(go), 'rust': sha(rust)},
    'thresholds': {'sha256': sha(thresholds_path), 'derivedFrom': thresholds['derivedFrom'],
                   'latency': thresholds['latency'], 'growth': thresholds['growth'],
                   'downSampleSlack': thresholds['downSampleSlack']},
    'shakedowns': shakedowns,
    'soak': soak_receipt,
    'cutoverInventory': cutover_items,
    'seam': {'gaps': seam.get('gaps'), 'canaryRed': canary['red']},
    'gates': gates,
    'inventory': {'gaps': gaps['count'], 'byMilestone': gaps['byMilestone'], 'owned': owned_gaps},
}
(HERE / 'validation.json').write_text(json.dumps(receipt, indent=2, sort_keys=True) + '\n')
print(json.dumps(soak_status), {g: v['pass'] for g, v in gates.items()})
