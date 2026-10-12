"""Recompute the public provider lifecycle matrix and write its receipt.

Reproduces evidence/m7/provider-lifecycle-validation.json from the raw
captures: every public scenario (including the 35 fake-provider lifecycle
scenarios) over five Go/Rust iterations, the same 35 scenarios Go/Go over five
iterations, the 16-field secret sweep, the check logs and the clean pinned Go
tree. It also asserts what the lifecycle journeys prove on the Go side, so an
equal but wrong pair cannot pass.
"""
import gzip, hashlib, json, subprocess, sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / 'tools/parity'))
from parity import runner, canon, m5_verify

sha = lambda path: hashlib.sha256(Path(path).read_bytes()).hexdigest()
load = lambda path: json.loads(Path(path).read_text())
raw = lambda path: json.loads(gzip.decompress(Path(path).read_bytes()))
lock = load(ROOT / 'baseline/source-lock.json')
go = ROOT / '.parity/bin/go-reference'
rust = ROOT / 'target/release/opute-host-agent'
git = lambda *args: subprocess.check_output(['git', '-C', str(ROOT / '.parity/go-src'), *args], text=True).strip()
assert git('rev-parse', 'HEAD') == lock['sourceCommit'] and git('rev-parse', 'HEAD^{tree}') == lock['sourceTree'] and not git('status', '--porcelain')

scenarios = {scenario['id']: scenario for scenario in runner.load_scenarios()}
lifecycle = [name for name in scenarios if name.startswith('provider-lifecycle.')]
assert len(lifecycle) == 35
generated = subprocess.check_output([sys.executable, str(ROOT / 'tools/parity/provider_lifecycle_scenarios.py'), '/dev/stdout'], cwd=ROOT / 'tools/parity')
assert json.loads(generated) == json.loads((ROOT / 'tools/parity/scenarios/provider-lifecycle.json').read_text())


def recompute(folder, left_label, right_label, names, left_sha, right_sha):
    summary = load(folder / 'summary.json')
    prov = summary['provenance']
    assert not prov['changedDuringRun'] and prov['harnessSha256'] == runner.harness_sha256()
    assert prov['sourceCommit'] == lock['sourceCommit'] and prov['sourceTree'] == lock['sourceTree']
    assert prov['left']['binarySha256'] == left_sha and prov['right']['binarySha256'] == right_sha
    assert prov['repeat'] == 5 and sorted(summary['items']) == sorted(names)
    for name, item in summary['items'].items():
        scenario = scenarios[name]
        capture = folder / 'scenarios' / name
        assert item['scenarioSha256'] == scenario['_sha256'] and item['status'] == 'pass', name
        assert item['iterations'] == 5 and not item['failingIterations'], name
        for path, digest in item['files'].items():
            assert sha(capture / path) == digest, (name, path)
        spec = scenario.get('compare', {})
        a, b = raw(capture / 'a.json.gz'), raw(capture / 'b.json.gz')
        norm_a, viol_a = canon.normalize(a['observation'], spec, a['variables'])
        norm_b, viol_b = canon.normalize(b['observation'], spec, b['variables'])
        viol_a += runner.invariant_violations(scenario, a['observation'], left_label)
        viol_b += runner.invariant_violations(scenario, b['observation'], right_label)
        norm_a, norm_b, divergences = canon.apply_divergences(
            norm_a, norm_b, spec.get('divergences', []), canon.load_divergences(runner.DIVERGENCE_FILE))
        norm_a, norm_b = runner._reconcile_optional_masks(norm_a, norm_b, spec.get('masks', []))
        expected = {'diff': canon.diff(norm_a, norm_b), 'maskViolations': {'a': viol_a, 'b': viol_b}, 'divergences': divergences}
        assert not expected['diff'] and not viol_a and not viol_b and not any(row['stale'] for row in divergences), name
        assert expected == load(capture / 'diff.json'), name
        iterations = load(capture / 'iterations.json')
        assert len(iterations) == 5 and [row['iteration'] for row in iterations] == list(range(5))
        assert all(not row['diffCount'] and not row['maskViolations'] and not row['staleDivergences'] for row in iterations), name
    return summary


public_dir = ROOT / 'evidence/m7/provider-lifecycle-public'
public = recompute(public_dir, 'go', 'rust', list(scenarios), sha(go), sha(rust))
twins_dir = ROOT / 'evidence/m7/provider-lifecycle-go-vs-go'
recompute(twins_dir, 'go', 'go2', lifecycle, sha(go), sha(go))
print(f'{len(scenarios)} public scenarios Go/Rust and {len(lifecycle)} lifecycle scenarios Go/Go recomputed over five iterations')


# What the journeys prove on the Go reference side (the first Go/Rust pair).
def go_steps(name):
    return raw(public_dir / 'scenarios' / name / 'a.json.gz')['observation']['steps']


def structured(step):
    return step['body']['result'].get('structuredContent')


def text(step):
    return ''.join(c.get('text', '') for c in step['body']['result'].get('content') or [])


happy = go_steps('provider-lifecycle.install-call-teardown')
assert structured(happy['installed'])['status'] == 'completed' and structured(happy['status'])['active'] is True
assert structured(happy['probe']) == {'ready': True} and structured(happy['tornDown'])['status'] == 'completed'
assert structured(happy['statusAfter'])['active'] is False and happy['state']['generations'][0]['status'] == 'stopped'
assert [r['name'] for r in happy['requests2']['requests']] == ['opute.provider.teardown'] * 2
assert [r['arguments']['phase'] for r in happy['requests2']['requests']] == ['prepare', 'finalize']
replace = go_steps('provider-lifecycle.replacement.in-flight')
assert structured(replace['heldResult']) == {'generation': 'g1'} and structured(replace['probeNew']) == {'generation': 'g2'}
assert [g['status'] for g in replace['stateWhileHeld']['generations']] == ['draining', 'active']
assert structured(replace['status3'])['generation']['ID'] == 'com.opute.fixture-2'
crash = go_steps('provider-lifecycle.crash.after-activation')
assert 'connect: connection refused' in text(crash['teardownBlocked'])
assert structured(crash['forced'])['status'] == 'completed' and structured(crash['forced'])['recipe']['providerTeardownForced'] is True
assert structured(crash['statusAfter'])['active'] is False
idem = go_steps('provider-lifecycle.teardown.idempotent')
assert 'confirm=true' in text(idem['teardownUnconfirmed']) and 'has no active generation' in text(idem['teardown2'])
restart = go_steps('provider-lifecycle.restart.active-provider')
assert structured(restart['statusRestored'])['connected'] is True and structured(restart['probe']) == {'ready': True}
down = go_steps('provider-lifecycle.restart.provider-down')
assert structured(down['statusRestored'])['active'] is True and structured(down['statusRestored'])['connected'] is False
for name in lifecycle:
    if name.startswith('provider-lifecycle.bad-manifest.') and name != 'provider-lifecycle.bad-manifest.validation-operation-undeclared':
        steps = go_steps(name)
        assert steps['install']['body']['result'].get('isError') or structured(steps['installed'])['status'] == 'failed', name
        assert structured(steps['status'])['active'] is False and len(steps['catalog']['body']['result']['tools']) == 142, name
print('Go reference lifecycle semantics asserted')

secret = ROOT / 'evidence/m7/provider-lifecycle-secret'
sweep = load(secret / 'summary.json')
prov = sweep['provenance']
assert not prov['changedDuringRun'] and prov['sourceLock'] == lock and prov['harnessSha256'] == runner.harness_sha256()
assert prov['goBinarySha256'] == sha(go) and prov['rustBinarySha256'] == sha(rust)
assert prov['driverSha256'] == sha(ROOT / 'tools/parity/parity/secret_sweep.py')
for path, digest in prov['fixtures']['runtimeSources'].items():
    assert sha(ROOT / path) == digest, path
for path, digest in sweep['files'].items():
    assert sha(secret / path) == digest
assert not m5_verify.secret_failures(sweep, raw(secret / 'outcomes.json.gz')) and sweep['fieldCount'] == 16

checks = ROOT / 'evidence/m7/provider-lifecycle-checks'
workspace = (checks / 'workspace.log').read_text()
assert '192 passed; 0 failed; 24 ignored' in workspace
assert 'Finished' in (checks / 'clippy.log').read_text() and 'Finished' in (checks / 'release.log').read_text()
assert not (checks / 'fmt.log').read_text()
assert 'Totals: 10 passed, 0 failed' in (checks / 'spec.log').read_text()
assert 'Ran 141 tests' in (checks / 'harness.log').read_text() and 'OK' in (checks / 'harness.log').read_text()
owner = (checks / 'go-owner.log').read_text()
owner_tests = 17
assert owner.count('--- PASS:') == owner_tests * 5 and '--- FAIL' not in owner
cutover = (checks / 'cutover.log').read_text()
assert 'FAIL' in cutover

receipt = {
    'scope': 'Public fake-provider lifecycle through opute.provider.install/status/validate/teardown/reload, '
             'generation-routed calls, in-flight replacement, normal/forced/idempotent teardown and restart restore. '
             'Callbacks, bridged tasks, process supervision and the five real providers remain open.',
    'publicScenarios': {'total': len(scenarios), 'lifecycle': len(lifecycle), 'iterations': 5, 'rawRecomputed': True},
    'lifecycleGoTwins': {'scenarios': len(lifecycle), 'iterations': 5, 'rawRecomputed': True},
    'freshSecretSweep': {'fields': 16, 'failures': 0},
    'workspaceTests': 192, 'ignoredFixtures': 24, 'harnessTests': 141, 'owningGoTests': owner_tests, 'owningGoTestRuns': owner_tests * 5,
    'goTransientsDocumented': [
        'tools/list and tools/call race the post-retire MCP tool refresh (unknown tool); scenarios wait for the whole catalog',
        'a provider teardown run reads completed before its finalize hook records failed; that case waits for the post-hook state',
        'a plan task completes after its run reads completed; journeys wait on tasks/get before stopping the agent',
    ],
    'openGaps': ['provider callbacks and bridged task support', 'Close cancelling still-working tasks',
                 'inspect_host_service and named host-service cleanup (M8)', 'process supervision',
                 'five real providers in a leased disposable runtime'],
    'sourceLock': lock, 'rustReleaseSha256': sha(rust), 'goReferenceSha256': sha(go), 'harnessSha256': runner.harness_sha256(),
    'evidenceSummaries': {str(p.relative_to(ROOT)): sha(p) for p in [public_dir / 'summary.json', twins_dir / 'summary.json', secret / 'summary.json']},
    'verificationScriptSha256': sha(Path(__file__)),
    'globalCutover': 'FAIL; candidate hash preserved, derived scenario items synced',
}
(ROOT / 'evidence/m7/provider-lifecycle-validation.json').write_text(json.dumps(receipt, indent=2) + '\n')
print(json.dumps(receipt, indent=2))
