"""Independently verify this checkpoint's raw evidence and current provenance."""
import gzip,hashlib,json,subprocess,sys
from pathlib import Path
ROOT=Path(__file__).resolve().parents[3]
sys.path.insert(0,str(ROOT/'tools/parity'))
from parity import runner,canon,m5_verify
sha=lambda p:hashlib.sha256(Path(p).read_bytes()).hexdigest()
load=lambda p:json.loads(Path(p).read_text())
raw=lambda p:json.loads(gzip.decompress(Path(p).read_bytes()))
lock=load(ROOT/'baseline/source-lock.json')
go=ROOT/'.parity/bin/go-reference';rust=ROOT/'target/release/opute-host-agent'
folder=ROOT/'evidence/m7/provider-delegation-bound';summary=load(folder/'summary.json')
assert summary['sourceLock']==lock
for path,digest in summary['provenance'].items():assert sha(ROOT/path)==digest,path
for name,digest in load(folder/'index.json').items():assert sha(folder/name)==digest,name
assert sha(ROOT/'.parity/bin/provider-catalog-oracle')==summary['goBinarySha256']
assert sha(ROOT/'target/debug/deps/opute_host_agent-6ae87315637d33b0')==summary['rustTestBinarySha256']
count=0
for iteration in range(5):
    left=raw(folder/str(iteration)/'go.json.gz');right=raw(folder/str(iteration)/'rust.json.gz');inputs=raw(folder/str(iteration)/'input.json.gz')
    assert len(left)==len(right)==len(inputs)==811
    assert left==right
    rows=[row for row in summary['rows'] if row['iteration']==iteration]
    assert len(rows)==811 and all(row['pass'] for row in rows)
    assert [row['case'] for row in rows]==[row['case'] for row in inputs]
    count+=len(left)
assert summary['passed']==summary['total']==count==4055
runtime=ROOT/'evidence/m7/provider-delegation-runtime';public=load(runtime/'summary.json');prov=public['provenance']
assert not prov['changedDuringRun'] and prov['harnessSha256']==runner.harness_sha256()
assert prov['sourceCommit']==lock['sourceCommit'] and prov['sourceTree']==lock['sourceTree']
assert prov['left']['binarySha256']==sha(go) and prov['right']['binarySha256']==sha(rust)
scenarios={scenario['id']:scenario for scenario in runner.load_scenarios()}
assert len(public['items'])==102
for name,item in public['items'].items():
    scenario=scenarios[name];capture=runtime/'scenarios'/name
    assert item['scenarioSha256']==scenario['_sha256'] and item['status']=='pass' and item['iterations']==5 and not item['failingIterations']
    for path,digest in item['files'].items():assert sha(capture/path)==digest
    spec=scenario.get('compare',{})
    a=raw(capture/'a.json.gz');b=raw(capture/'b.json.gz')
    norm_a,viol_a=canon.normalize(a['observation'],spec,a['variables']);norm_b,viol_b=canon.normalize(b['observation'],spec,b['variables'])
    viol_a+=runner.invariant_violations(scenario,a['observation'],'go');viol_b+=runner.invariant_violations(scenario,b['observation'],'rust')
    norm_a,norm_b,divergences=canon.apply_divergences(norm_a,norm_b,spec.get('divergences',[]),canon.load_divergences(runner.DIVERGENCE_FILE))
    norm_a,norm_b=runner._reconcile_optional_masks(norm_a,norm_b,spec.get('masks',[]))
    expected={'diff':canon.diff(norm_a,norm_b),'maskViolations':{'a':viol_a,'b':viol_b},'divergences':divergences}
    assert not expected['diff'] and not viol_a and not viol_b and not any(row['stale'] for row in divergences),name
    assert expected==load(capture/'diff.json'),name
    iterations=load(capture/'iterations.json')
    assert len(iterations)==5 and [row['iteration'] for row in iterations]==list(range(5))
    assert all(not row['diffCount'] and not row['maskViolations'] and not row['staleDivergences'] for row in iterations)
secret=ROOT/'evidence/m7/provider-delegation-secret';sweep=load(secret/'summary.json');prov=sweep['provenance']
assert not prov['changedDuringRun'] and prov['sourceLock']==lock and prov['harnessSha256']==runner.harness_sha256()
assert prov['goBinarySha256']==sha(go) and prov['rustBinarySha256']==sha(rust)
assert prov['driverSha256']==sha(ROOT/'tools/parity/parity/secret_sweep.py')
for path,digest in prov['fixtures']['runtimeSources'].items():assert sha(ROOT/path)==digest,path
for path,digest in sweep['files'].items():assert sha(secret/path)==digest
assert not m5_verify.secret_failures(sweep,raw(secret/'outcomes.json.gz'))
assert sweep['fieldCount']==16
checks=ROOT/'evidence/m7/provider-delegation-checks'
assert '185 passed; 0 failed; 19 ignored' in (checks/'workspace.log').read_text()
assert (checks/'go-owner.log').read_text().count('--- PASS:')==25
assert 'Totals: 9 passed, 0 failed' in (checks/'spec.log').read_text()
assert not (checks/'fmt.log').read_text()
assert 'Finished' in (checks/'clippy.log').read_text() and 'Finished' in (checks/'release.log').read_text()
cutover=(checks/'cutover.log').read_text();assert 'gate cutover: FAIL' in cutover
assert subprocess.check_output(['git','-C',str(ROOT/'.parity/go-src'),'rev-parse','HEAD'],text=True).strip()==lock['sourceCommit']
assert subprocess.check_output(['git','-C',str(ROOT/'.parity/go-src'),'status','--porcelain'],text=True).strip()==''
receipt={'scope':'Private signed delegation cryptographic owner and active permit lifetime; reservation/task issuer, callback scope, child task inheritance, public composition and full M7-M13 E2E remain open','comparisons':count,'fullRawEqualityVerified':True,'sourceAndBinaryHashesVerified':True,'indexVerified':True,'workspaceTests':185,'ignoredDifferentialFixtures':19,'owningGoTests':25,'publicRuntime':{'scenarios':102,'iterations':5,'rawRecomputed':True},'secretSweep':{'fields':16,'failures':0},'rustReleaseSha256':sha(rust),'goReferenceSha256':sha(go),'harnessSha256':runner.harness_sha256(),'globalCutover':'FAIL; existing global manifest preserved','verificationScriptSha256':sha(Path(__file__)),'sourceLock':lock,'evidenceSummaries':{str(p.relative_to(ROOT)):sha(p) for p in [folder/'summary.json',runtime/'summary.json',secret/'summary.json']}}
(ROOT/'evidence/m7/provider-delegation-validation.json').write_text(json.dumps(receipt,indent=2)+'\n')
print(json.dumps(receipt,indent=2))
