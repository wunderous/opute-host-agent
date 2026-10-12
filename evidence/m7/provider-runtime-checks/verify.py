"""Recompute provider runtime equality and validate each source-bound raw batch."""
import gzip, hashlib, json, subprocess, sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / 'tools/parity'))
from provider_runtime import cases
from parity import runner, canon, m5_verify

sha = lambda path: hashlib.sha256(Path(path).read_bytes()).hexdigest()
load = lambda path: json.loads(Path(path).read_text())
raw = lambda path: json.loads(gzip.decompress(Path(path).read_bytes()))
lock = load(ROOT / 'baseline/source-lock.json')
folder = ROOT / 'evidence/m7/provider-runtime'
summary = load(folder / 'summary.json')
assert summary['sourceLock'] == lock
for path, digest in summary['provenance'].items():
    assert sha(ROOT / path) == digest, path
for path, digest in load(folder / 'index.json').items():
    assert sha(folder / path) == digest, path
assert sha(ROOT / ('.parity/bin/provider-adapter-runtime-go-' + summary['goBinarySha256'])) == summary['goBinarySha256']
assert sha(ROOT / ('.parity/bin/provider-adapter-runtime-rust-' + summary['rustTestBinarySha256'])) == summary['rustTestBinarySha256']
corpus = cases()
assert len(corpus) == 24
headers = ['mcp-protocol-version', 'mcp-client-name', 'mcp-client-version', 'authorization', 'opute-resource-delegation']
requests = lambda values: [{'path': r['path'], 'payload': json.loads(r['body']), 'headers': {k: v for k, v in r['headers'].items() if k in headers}} for r in values]
rows = []
for iteration in range(5):
    for batch, start in enumerate(range(0, len(corpus), 6)):
        capture = folder / str(iteration) / str(batch)
        inputs, go, rust = (raw(capture / (name + '.json.gz')) for name in ['input', 'go', 'rust'])
        expected = corpus[start:start + 6]
        # The driver substitutes each batch's exclusively bound loopback endpoint.
        endpoints = {s['generation']['Endpoint'] for c in inputs for s in c['runtime']['steps'] if s['action'] == 'restore'}
        assert all(e.startswith('http://127.0.0.1:') for e in endpoints)
        assert [c['case'] for c in inputs] == [c['case'] for c in expected]
        assert len(inputs) == len(go) == len(rust) == 6
        traffic = load(capture / 'requests.json')
        assert requests(traffic['go']) == requests(traffic['rust'])
        for case, left, right in zip(inputs, go, rust):
            assert left == right, (iteration, case['case'])
            rows.append({'case': case['case'], 'iteration': iteration, 'batch': batch, 'pass': True})
assert rows == summary['rows']
assert summary['passed'] == summary['total'] == len(rows) == 120
print('Provider runtime raw equality, full corpus, five iterations, request traces and provenance verified')

checks = ROOT / 'evidence/m7/provider-runtime-checks'
assert '188 passed; 0 failed; 24 ignored' in (checks / 'workspace.log').read_text()
assert 'Finished' in (checks / 'clippy.log').read_text() and 'Finished' in (checks / 'release.log').read_text()
assert not (checks / 'fmt.log').read_text()
assert 'Totals: 10 passed, 0 failed' in (checks / 'spec.log').read_text()
owner = (checks / 'go-owner.log').read_text()
assert owner.count('--- PASS:') == 25 and '--- FAIL' not in owner
assert 'inventory gaps remain: 76' in (checks / 'cutover.log').read_text()
git = lambda *args: subprocess.check_output(['git', '-C', str(ROOT / '.parity/go-src'), *args], text=True).strip()
assert git('rev-parse', 'HEAD') == lock['sourceCommit'] and git('rev-parse', 'HEAD^{tree}') == lock['sourceTree'] and not git('status', '--porcelain')
# The header-name capture was a fixture error, never a runtime divergence or pass.
diagnostic = ROOT / 'evidence/m7/provider-runtime-diagnostic/header-name'
for path, digest in load(diagnostic / 'index.json').items():
    assert sha(diagnostic / path) == digest
assert not load(diagnostic / 'requests.json')['go'] and not load(diagnostic / 'requests.json')['rust']
# Reuse public captures only after exact binary/harness identity and full raw verification.
runtime=ROOT/'evidence/m7/provider-runtime-public';public=load(runtime/'summary.json');prov=public['provenance']
go=ROOT/'.parity/bin/go-reference';rust=ROOT/'target/release/opute-host-agent'
assert not prov['changedDuringRun'] and prov['harnessSha256']==runner.harness_sha256()
assert prov['sourceCommit']==lock['sourceCommit'] and prov['sourceTree']==lock['sourceTree']
assert prov['left']['binarySha256']==sha(go) and prov['right']['binarySha256']==sha(rust)
scenarios={scenario['id']:scenario for scenario in runner.load_scenarios()};assert len(public['items'])==102
for name,item in public['items'].items():
    scenario=scenarios[name];capture=runtime/'scenarios'/name
    assert item['scenarioSha256']==scenario['_sha256'] and item['status']=='pass' and item['iterations']==5 and not item['failingIterations']
    for path,digest in item['files'].items():assert sha(capture/path)==digest
    spec=scenario.get('compare',{});a=raw(capture/'a.json.gz');b=raw(capture/'b.json.gz')
    norm_a,viol_a=canon.normalize(a['observation'],spec,a['variables']);norm_b,viol_b=canon.normalize(b['observation'],spec,b['variables'])
    viol_a+=runner.invariant_violations(scenario,a['observation'],'go');viol_b+=runner.invariant_violations(scenario,b['observation'],'rust')
    norm_a,norm_b,divergences=canon.apply_divergences(norm_a,norm_b,spec.get('divergences',[]),canon.load_divergences(runner.DIVERGENCE_FILE))
    norm_a,norm_b=runner._reconcile_optional_masks(norm_a,norm_b,spec.get('masks',[]))
    expected={'diff':canon.diff(norm_a,norm_b),'maskViolations':{'a':viol_a,'b':viol_b},'divergences':divergences}
    assert not expected['diff'] and not viol_a and not viol_b and not any(row['stale'] for row in divergences),name
    assert expected==load(capture/'diff.json'),name
    iterations=load(capture/'iterations.json');assert len(iterations)==5 and [row['iteration'] for row in iterations]==list(range(5))
    assert all(not row['diffCount'] and not row['maskViolations'] and not row['staleDivergences'] for row in iterations)
secret=ROOT/'evidence/m7/provider-runtime-secret';sweep=load(secret/'summary.json');prov=sweep['provenance']
assert not prov['changedDuringRun'] and prov['sourceLock']==lock and prov['harnessSha256']==runner.harness_sha256()
assert prov['goBinarySha256']==sha(go) and prov['rustBinarySha256']==sha(rust)
assert prov['driverSha256']==sha(ROOT/'tools/parity/parity/secret_sweep.py')
for path,digest in prov['fixtures']['runtimeSources'].items():assert sha(ROOT/path)==digest,path
for path,digest in sweep['files'].items():assert sha(secret/path)==digest
assert not m5_verify.secret_failures(sweep,raw(secret/'outcomes.json.gz')) and sweep['fieldCount']==16

receipt={'scope':summary['scope'],
 'runtimeComparisons':120,'cases':24,'iterations':5,'providerRequestTracesEqual':True,
 'freshPublicRuntime':{'scenarios':102,'iterations':5,'basis':'fresh captures against the current release; raw comparisons independently recomputed'},
 'freshSecretSweep':{'fields':16,'failures':0},'workspaceTests':188,'ignoredFixtures':24,'owningGoTests':25,
 'diagnostics':{'header-name':'fixture error (blank tools/call name); not a runtime divergence or pass'},
 'sourceLock':lock,'sourceBinaryAndCaptureHashesVerified':True,'rustReleaseSha256':sha(rust),
 'verificationScriptSha256':sha(Path(__file__)),'globalCutover':'FAIL; original manifest preserved'}
(ROOT/'evidence/m7/provider-runtime-validation.json').write_text(json.dumps(receipt,indent=2)+'\n')
print(json.dumps(receipt,indent=2))
