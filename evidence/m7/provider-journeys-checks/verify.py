"""Recompute the provider journeys checkpoint and write its receipt.

Reproduces evidence/m7/provider-journeys-validation.json from the raw
captures: every public scenario (including the 49 fake-provider lifecycle and
callback scenarios and the five real Go provider journeys) over five Go/Rust
iterations, the same 54 scenarios Go/Go over five iterations, the 16-field
secret sweep, the open unadmitted-callback-task divergence, the reproducible
provider build, the user-bus isolation preflight, the check logs and the clean
pinned Go tree. It also asserts what the journeys prove on the Go side, so an
equal but wrong pair cannot pass.
"""
import gzip, hashlib, json, subprocess, sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / 'tools/parity'))
from parity import runner, canon, m5_verify

WORKSPACE, IGNORED = 194, 24
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
assert len(lifecycle) == 49
real = [name for name in scenarios if name.startswith('provider-real.')]
assert sorted(real) == ['provider-real.' + p for p in ('cloudflare', 'hostos', 'k3s', 'ollama', 'tailscale')]
generated_real = subprocess.check_output([sys.executable, str(ROOT / 'tools/parity/provider_real_scenarios.py'), '/dev/stdout'], cwd=ROOT / 'tools/parity')
assert json.loads(generated_real) == json.loads((ROOT / 'tools/parity/scenarios/provider-real.json').read_text())
generated = subprocess.check_output([sys.executable, str(ROOT / 'tools/parity/provider_lifecycle_scenarios.py'), '/dev/stdout', '/dev/null'], cwd=ROOT / 'tools/parity')
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
        divergences = []
        # As in the runner: declared divergences apply only between Go and Rust.
        if left_label.startswith('rust') != right_label.startswith('rust'):
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


public_dir = ROOT / 'evidence/m7/provider-journeys-public'
public = recompute(public_dir, 'go', 'rust', list(scenarios), sha(go), sha(rust))
twins_dir = ROOT / 'evidence/m7/provider-journeys-go-vs-go'
recompute(twins_dir, 'go', 'go2', lifecycle + real, sha(go), sha(go))
print(f'{len(scenarios)} public scenarios Go/Rust and {len(lifecycle) + len(real)} provider scenarios Go/Go recomputed over five iterations')


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


def task_structured(step):
    return step['body']['result']['result'].get('structuredContent')


def callback(name, status=200):
    steps = go_steps('provider-lifecycle.callback.' + name)
    bridged = sorted(k for k in steps if k.startswith('bridged'))[-1]
    outcome = task_structured(steps[bridged])['callback']
    assert outcome['status'] == status, name
    return outcome['body']


for name in ('host-tool', 'no-delegation'):
    assert callback(name)['result']['structuredContent']['exists'] is False
refusals = {
    'tampered': 'host_resource_delegation_invalid: provider callback delegation signature is invalid',
    'malformed': 'host_resource_delegation_invalid: provider callback delegation is malformed',
    'stale': 'host_resource_delegation_inactive: provider callback delegation is no longer active',
    'lifecycle-tool': 'host_resource_delegation_scope_invalid: callback is outside the signed Host Agent execution scope',
    'provider-tool': 'host_resource_delegation_scope_invalid: callback is outside the signed Host Agent execution scope',
    'standalone-tool': 'host_resource_delegation_scope_invalid: callback is outside the signed Host Agent execution scope',
}
for name, message in refusals.items():
    result = callback(name)['result']
    assert result['isError'] is True and result['content'][0]['text'] == 'Error: ' + message, name
assert callback('unknown-tool', 400)['error']['message'] == 'unknown tool "not_a_host_tool"'
child = callback('child-task-acknowledged')
assert child['result']['status'] == 'working'
ack = task_structured(go_steps('provider-lifecycle.callback.child-task-acknowledged')['bridged0'])['callback']['acknowledged']
assert ack['body']['result']['status'] == 'completed' and ack['body']['result']['result']['structuredContent']['code'] == 'resource_binding'
cancel = go_steps('provider-lifecycle.callback.cancel-bridged-task-refused')
assert cancel['cancel']['body']['error']['message'].startswith('cannot cancel task: ')
assert task_structured(cancel['bridged']) == {'bridged': True}
restart = go_steps('provider-lifecycle.callback.restart-during-bridged-task')
assert restart['stop1'] == {'exit': 0, 'killed': False}
assert restart['restored']['body']['result']['statusMessage'] == 'The Host Agent shut down before the task completed.'
assert [r['terminal_status'] for r in restart['state']['invocations']] == ['success', 'error']
plain = go_steps('provider-lifecycle.callback.bridged-without-task-extension')
assert plain['bridge']['body']['error']['code'] == -32003 and plain['bridge']['body']['error']['message'] == 'Missing required client capability'


def journey(name):
    observation = raw(public_dir / 'scenarios' / ('provider-real.' + name) / 'a.json.gz')['observation']
    steps = observation['steps']
    assert steps['agentProcesses'] == {'descendants': []} and observation['orphans'] == [], name
    assert steps['providerStopped'] == {'stopped': True, 'signal': 15}, name
    assert len(steps['catalogBefore']['body']['result']['tools']) == 142, name
    return steps


hostos = journey('hostos')
assert text(hostos['install']) == 'Error: provider manifest has no recipe for mode ""'
assert structured(hostos['status'])['active'] is False and len(hostos['catalogAfter']['body']['result']['tools']) == 142
published = {'ollama': 3, 'k3s': 28, 'cloudflare': 13, 'tailscale': 13}
for name, count in published.items():
    steps = journey(name)
    assert structured(steps['installed'])['status'] == 'completed', name
    assert structured(steps['status'])['active'] is True, name
    assert len(steps['catalog']['body']['result']['tools']) == 142 + count, name
ollama = journey('ollama')
assert structured(ollama['call'])['contractVersion'] == 'opute.capability.llm-serving.context-size.v1'
assert ollama['call']['body']['result']['_meta']['io.modelcontextprotocol/serverInfo'] == {'name': 'opute-provider-ollama', 'version': '1.0.0'}
assert structured(ollama['validate'])['provider'] == {'validationDelegated': True}
assert text(ollama['teardown']) == 'Error: node "stop" action set_host_service_state arguments: missing required property "uri"'
assert structured(ollama['statusAfter'])['active'] is True
assert structured(journey('k3s')['call']) == {'contractVersion': 'opute.capability.kubernetes.v1', 'ready': True}
assert structured(journey('cloudflare')['call'])['ready'] is True
assert text(journey('tailscale')['validate']) == 'Error: MCP error 0: target is not enrolled in the overlay'
for name in ('k3s', 'cloudflare', 'tailscale'):
    steps = journey(name)
    assert structured(steps['tornDown'])['status'] == 'completed', name
    assert structured(steps['statusAfter'])['active'] is False and len(steps['catalogAfter']['body']['result']['tools']) == 142, name
flap = go_steps('provider-lifecycle.readiness.flapping')
assert structured(flap['installed'])['status'] == 'failed' and 'not ready' in structured(flap['installed'])['error']
assert structured(flap['status'])['active'] is False and len(flap['catalogBetween']['body']['result']['tools']) == 142
assert structured(flap['installed2'])['status'] == 'completed' and structured(flap['status2'])['generation']['ID'] == 'com.opute.fixture-2'
assert [g['status'] for g in flap['state']['generations']] == ['failed', 'active']
for name in lifecycle:
    for label, step in go_steps(name).items():
        if label.startswith('idle'):
            assert step == {'observed': 0}, (name, label)
print('Go reference lifecycle, callback and real-provider semantics asserted')
open_divergence = subprocess.run([sys.executable, str(ROOT / 'evidence/m7/unadmitted-callback-task/verify.py')], check=True, capture_output=True, text=True)
open_receipt = load(ROOT / 'evidence/m7/unadmitted-callback-task-validation.json')

secret = ROOT / 'evidence/m7/provider-journeys-secret'
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

checks = ROOT / 'evidence/m7/provider-journeys-checks'
workspace = (checks / 'workspace.log').read_text()
assert f'{WORKSPACE} passed; 0 failed; {IGNORED} ignored' in workspace
assert 'Finished' in (checks / 'clippy.log').read_text() and 'Finished' in (checks / 'release.log').read_text()
assert not (checks / 'fmt.log').read_text()
assert 'Totals: 11 passed, 0 failed' in (checks / 'spec.log').read_text()
assert 'Ran 141 tests' in (checks / 'harness.log').read_text() and 'OK' in (checks / 'harness.log').read_text()
owner = (checks / 'go-owner.log').read_text()
owner_tests = 27
assert owner.count('--- PASS:') == owner_tests * 5 and '--- FAIL' not in owner
builds = load(checks / 'providers.json')
assert builds['sourceCommit'] == lock['sourceCommit'] and builds['sourceTree'] == lock['sourceTree'] and builds['goVersion'] == lock['goToolchain']
for name, digest in builds['binaries'].items():
    assert sha(ROOT / '.parity/bin/providers' / name) == digest, name
assert sorted(builds['binaries']) == sorted('opute-provider-' + p for p in ('cloudflare', 'hostos', 'k3s', 'ollama', 'tailscale'))
bus = (checks / 'bus-isolation.log').read_text().splitlines()
assert bus[0].startswith('Failed to connect to user scope bus') and bus[1] != 'exit=0' and bus[-1] == 'units-exit=1' and len(bus) == 3
cutover = (checks / 'cutover.log').read_text()
assert 'FAIL' in cutover

receipt = {
    'scope': 'Provider callbacks and bridged tasks, a flapping readiness probe, and the five pinned Go providers unchanged through the public lifecycle '
             '(install with their own activation recipes, catalog cell, a provider-reached call, validation, teardown), '
             'with no agent descendants and no orphaned shims. The 35 lifecycle scenarios are rerun.',
    'publicScenarios': {'total': len(scenarios), 'lifecycle': len(lifecycle), 'realProviders': len(real), 'iterations': 5, 'rawRecomputed': True},
    'providerGoTwins': {'scenarios': len(lifecycle) + len(real), 'iterations': 5, 'rawRecomputed': True},
    'freshSecretSweep': {'fields': 16, 'failures': 0},
    'workspaceTests': WORKSPACE, 'ignoredFixtures': IGNORED, 'harnessTests': 141, 'owningGoTests': owner_tests, 'owningGoTestRuns': owner_tests * 5,
    'realProviders': {
        'binaries': builds['binaries'], 'reproducibleBuild': 'tools/parity/build-go-providers.sh',
        'isolation': 'loopback port, sandbox HOME and recording shims, isolated user bus (preflight: bus-isolation.log), dead egress proxy',
        'goOutcomes': {
            'hostos': 'public install refused: the manifest declares no recipe',
            'ollama': 'installed and called; teardown refused by plan validation (set_host_service_state requires uri); stays active',
            'k3s': 'installed, called, validated, torn down',
            'cloudflare': 'installed, called, torn down; validation reaches the provider and reports its error',
            'tailscale': 'installed, validation reaches the provider, torn down; every published tool binds a resource URI that needs an Incus inventory',
        },
    },
    'goTransientsSynchronized': [
        'Go releases the install run heavy reservation after its task reports completion and refuses normal-class work while it is held; '
        'bridged calls first poll get_host_capacity until no reservation is held (only the settled count is observed, and the poll audit rows are excluded)',
    ],
    'declaredDivergences': {'D12.refused-callback-child-operation': 'the refused callback child operation row, isolated into its own query'},
    'openDivergences': {'unadmitted-callback-task-record': open_receipt['status'],
                        'reject-null-provider-input-requests': 'open divergence, pending owner decision; not Go parity'},
    'sourceLock': lock, 'rustReleaseSha256': sha(rust), 'goReferenceSha256': sha(go), 'harnessSha256': runner.harness_sha256(),
    'evidenceSummaries': {str(p.relative_to(ROOT)): sha(p) for p in [public_dir / 'summary.json', twins_dir / 'summary.json', secret / 'summary.json',
                          ROOT / 'evidence/m7/unadmitted-callback-task/capture/summary.json', checks / 'providers.json']},
    'verificationScriptSha256': sha(Path(__file__)),
    'globalCutover': 'FAIL; candidate hash preserved, derived scenario items synced',
}
(ROOT / 'evidence/m7/provider-journeys-validation.json').write_text(json.dumps(receipt, indent=2) + '\n')
print(json.dumps(receipt, indent=2))
