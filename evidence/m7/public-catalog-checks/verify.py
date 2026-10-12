"""Recompute publication equality and validate each source-bound raw batch."""
import gzip
import base64, copy, hmac, uuid
import hashlib
import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / 'tools/parity'))
from provider_publication_http import cases
from parity import runner, canon, m5_verify

sha = lambda path: hashlib.sha256(Path(path).read_bytes()).hexdigest()
load = lambda path: json.loads(Path(path).read_text())
raw = lambda path: json.loads(gzip.decompress(Path(path).read_bytes()))
lock = load(ROOT / 'baseline/source-lock.json')
folder = ROOT / 'evidence/m7/public-catalog-publication'
summary = load(folder / 'summary.json')
assert summary['sourceLock'] == lock
for path, digest in summary['provenance'].items():
    assert sha(ROOT / path) == digest, path
for path, digest in load(folder / 'index.json').items():
    assert sha(folder / path) == digest, path
assert summary['goBinarySha256'] == sha(ROOT / '.parity/bin/provider-catalog-oracle')
assert summary['rustTestBinarySha256'] == sha(ROOT / 'target/debug/deps/opute_host_agent-6ae87315637d33b0')
corpus = cases()
assert len(corpus) == 32
rows = []
for iteration in range(5):
    for batch, start in enumerate(range(0, len(corpus), 12)):
        capture = folder / str(iteration) / str(batch)
        inputs, go, rust = (raw(capture / (name + '.json.gz')) for name in ['input', 'go', 'rust'])
        assert inputs == corpus[start:start + 12]
        assert len(inputs) == len(go) == len(rust)
        for case, left, right in zip(inputs, go, rust):
            assert left == right, (iteration, case['case'])
            if case['case'].startswith('noop-retire-stale-wire-'):
                final=right['observations'][-1]
                assert final['updates']==1 and not final['error']
                assert final['published']!=right['observations'][0]['published']
                assert final['wire']==right['observations'][0]['wire']
                assert any(tool['name']=='test.probe' for tool in final['wire'])
                assert not any(tool['name']=='test.probe' for tool in final['published']['tools'])
            rows.append({'case': case['case'], 'iteration': iteration, 'batch': batch, 'pass': True})
assert rows == summary['rows']
assert summary['passed'] == summary['total'] == len(rows) == 160
print('Publication raw equality, full corpus, five iterations, source and binary provenance verified')


checks=ROOT/'evidence/m7/public-catalog-checks'
assert '187 passed; 0 failed; 23 ignored' in (checks/'workspace.log').read_text()
assert 'Finished' in (checks/'clippy.log').read_text() and 'Finished' in (checks/'release.log').read_text()
assert not (checks/'fmt.log').read_text()
assert 'Totals: 10 passed, 0 failed' in (checks/'spec.log').read_text()
assert (checks/'go-owner.log').read_text().count('--- PASS:')==15
assert 'inventory gaps remain: 76' in (checks/'cutover.log').read_text()
git=lambda *args:subprocess.check_output(['git','-C',str(ROOT/'.parity/go-src'),*args],text=True).strip()
assert git('rev-parse','HEAD')==lock['sourceCommit'] and git('rev-parse','HEAD^{tree}')==lock['sourceTree'] and not git('status','--porcelain')
# Historical draft is a real wire-only divergence, never a current pass.
diagnostic=ROOT/'evidence/m7/public-catalog-diagnostic/noop-retire'
for path,digest in load(diagnostic/'index.json').items():assert sha(diagnostic/path)==digest
before_go=raw(diagnostic/'go.json.gz');before_rust=raw(diagnostic/'rust.json.gz')
for left,right in zip(before_go,before_rust):
    differences=canon.diff(left,right)
    assert differences and all(row['path'][:3]==['observations',2,'wire'] for row in differences)
# Reuse public captures only after exact binary/harness identity and full raw verification.
runtime=ROOT/'evidence/m7/public-catalog-runtime';public=load(runtime/'summary.json');prov=public['provenance']
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
secret=ROOT/'evidence/m7/public-catalog-secret';sweep=load(secret/'summary.json');prov=sweep['provenance']
assert not prov['changedDuringRun'] and prov['sourceLock']==lock and prov['harnessSha256']==runner.harness_sha256()
assert prov['goBinarySha256']==sha(go) and prov['rustBinarySha256']==sha(rust)
assert prov['driverSha256']==sha(ROOT/'tools/parity/parity/secret_sweep.py')
for path,digest in prov['fixtures']['runtimeSources'].items():assert sha(ROOT/path)==digest,path
for path,digest in sweep['files'].items():assert sha(secret/path)==digest
assert not m5_verify.secret_failures(sweep,raw(secret/'outcomes.json.gz')) and sweep['fieldCount']==16

receipt={'scope':'Shared server catalog publication and distinct last-published MCP projection. Dynamic trusted publication is exercised through native authenticated HTTP tools/list and Go SDK listing. Public provider lifecycle commands and executable invocation remain open.',
 'publicationComparisons':160,'dynamicCases':32,'iterations':5,'staleWireRetirementRegressionVerified':True,
 'freshPublicRuntime':{'scenarios':102,'iterations':5,'basis':'fresh captures against the current release; raw comparisons independently recomputed'},
 'freshSecretSweep':{'fields':16,'failures':0},'workspaceTests':187,'ignoredFixtures':23,'owningGoTests':15,
 'sourceLock':lock,'sourceBinaryAndCaptureHashesVerified':True,'rustReleaseSha256':sha(rust),
 'verificationScriptSha256':sha(Path(__file__)),'globalCutover':'FAIL; original manifest preserved'}
(ROOT/'evidence/m7/public-catalog-validation.json').write_text(json.dumps(receipt,indent=2)+'\n')
print(json.dumps(receipt,indent=2))
