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
from provider_publication import cases
from parity import runner, canon, m5_verify

sha = lambda path: hashlib.sha256(Path(path).read_bytes()).hexdigest()
load = lambda path: json.loads(Path(path).read_text())
raw = lambda path: json.loads(gzip.decompress(Path(path).read_bytes()))
lock = load(ROOT / 'baseline/source-lock.json')
folder = ROOT / 'evidence/m7/provider-publication-parallel'
summary = load(folder / 'summary.json')
assert summary['sourceLock'] == lock
for path, digest in summary['provenance'].items():
    assert sha(ROOT / path) == digest, path
for path, digest in load(folder / 'index.json').items():
    assert sha(folder / path) == digest, path
assert summary['goBinarySha256'] == sha(ROOT / '.parity/bin/provider-catalog-oracle')
assert summary['rustTestBinarySha256'] == sha(ROOT / 'target/debug/deps/opute_host_agent-6ae87315637d33b0')
corpus = cases()
assert len(corpus) == 242
rows = []
for iteration in range(5):
    for batch, start in enumerate(range(0, len(corpus), 12)):
        capture = folder / str(iteration) / str(batch)
        inputs, go, rust = (raw(capture / (name + '.json.gz')) for name in ['input', 'go', 'rust'])
        assert inputs == corpus[start:start + 12]
        assert len(inputs) == len(go) == len(rust)
        for case, left, right in zip(inputs, go, rust):
            assert left == right, (iteration, case['case'])
            rows.append({'case': case['case'], 'iteration': iteration, 'batch': batch, 'pass': True})
assert rows == summary['rows']
assert summary['passed'] == summary['total'] == len(rows) == 1210
print('Publication raw equality, full corpus, five iterations, source and binary provenance verified')

checks={}
def normalize(output,input,clock):
    value=copy.deepcopy(output)
    if not value['issued']:return value
    payload,signature=value['token'].split('.')
    decode=lambda s:base64.urlsafe_b64decode(s+'='*(-len(s)%4))
    assert hmac.compare_digest(decode(signature),hmac.new(bytes.fromhex(input['key']),payload.encode(),hashlib.sha256).digest())
    encoded=decode(payload).decode();claims=json.loads(encoded);identifier=uuid.UUID(claims['ID'])
    assert str(identifier)==claims['ID'] and identifier.version==4 and identifier.variant==uuid.RFC_4122
    assert int(clock['started'])+3600<=claims['ExpiresAt']<=int(clock['finished'])+3600
    assert claims['Version']=='host-resource-delegation.v1'
    assert claims['ReservationID']==input['reservation']['id']
    assert claims['AgentID']==input['agent'].strip() and claims['TaskID']==input['task'].strip()
    assert claims['TaskOperation']==input['operation'].strip()
    for field,key in [('ProviderID','provider'),('ProviderGeneration','generation'),('ProviderOperation','providerOperation')]:assert claims[field]==input[key]
    if not value['verificationError']:
        assert value['claims']==encoded
        assert value['releaseError']=='host_resource_delegation_inactive: provider callback delegation is no longer active'
        value['claims']=encoded.replace('"ID":"'+claims['ID']+'"','"ID":"<uuid-v4>"').replace('"ExpiresAt":'+str(claims['ExpiresAt'])+'}','"ExpiresAt":<one-hour-expiry>}')
    value['token']='<verified-signed-token>'
    return value

for name,total in [('issuance',3570),('callback',4445),('verification',4055),('binding',560)]:
    folder=ROOT/'evidence/m7'/('provider-publication-'+name);summary=load(folder/'summary.json')
    assert summary['sourceLock']==lock
    for path,digest in summary['provenance'].items():assert sha(ROOT/path)==digest,path
    for path,digest in load(folder/'index.json').items():assert sha(folder/path)==digest,path
    assert summary['goBinarySha256']==sha(ROOT/'.parity/bin/provider-catalog-oracle')
    assert summary['rustTestBinarySha256']==sha(ROOT/'target/debug/deps/opute_host_agent-6ae87315637d33b0')
    count=0;cross_count=0
    for iteration in range(5):
        inputs=raw(folder/str(iteration)/'input.json.gz');left=raw(folder/str(iteration)/'go.json.gz');right=raw(folder/str(iteration)/'rust.json.gz')
        assert len(inputs)==len(left)==len(right)==total//5
        if name=='issuance':
            clock=load(folder/str(iteration)/'clock.json')
            for case,a,b in zip(inputs,left,right):assert normalize(a,case['issuance'],clock)==normalize(b,case['issuance'],clock),case['case']
            cross_inputs=raw(folder/str(iteration)/'cross-input.json.gz');cross_go=raw(folder/str(iteration)/'cross-go.json.gz')
            expected=[]
            for case,value in zip(inputs,right):
                if value['issued'] and not value['verificationError']:
                    claims=json.loads(value['claims']);expected.append({'delegation':{'key':case['issuance']['key'],'token':value['token'],'active':{claims['ID']:value['token']}}})
            assert cross_inputs==expected
            assert cross_go==[{'claims':base64.urlsafe_b64decode(row['delegation']['token'].split('.')[0]+'='*(-len(row['delegation']['token'].split('.')[0])%4)).decode(),'error':''} for row in cross_inputs]
            cross_count+=len(cross_go)
        else:assert left==right
        count+=len(left)
        rows=[row for row in summary['rows'] if row['iteration']==iteration]
        assert len(rows)==len(inputs) and all(row['pass'] for row in rows)
        assert [row['case'] for row in rows]==[row['case'] for row in inputs]
    assert count==summary['passed']==summary['total']==total
    checks[name]={'comparisons':count,'fullRawComparisonVerified':True,'sourceBinaryAndIndexHashesVerified':True}
    if name=='issuance':checks[name]['nativeTokensAcceptedByActualGoVerifier']=cross_count


# Reuse the earlier public captures only after rechecking their exact current
# binary identities, full raw observations, harness, and secret evidence.
runtime=ROOT/'evidence/m7/provider-owner-runtime';public=load(runtime/'summary.json');prov=public['provenance']
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
secret=ROOT/'evidence/m7/provider-owner-secret';sweep=load(secret/'summary.json');prov=sweep['provenance']
assert not prov['changedDuringRun'] and prov['sourceLock']==lock and prov['harnessSha256']==runner.harness_sha256()
assert prov['goBinarySha256']==sha(go) and prov['rustBinarySha256']==sha(rust)
assert prov['driverSha256']==sha(ROOT/'tools/parity/parity/secret_sweep.py')
for path,digest in prov['fixtures']['runtimeSources'].items():assert sha(ROOT/path)==digest,path
for path,digest in sweep['files'].items():assert sha(secret/path)==digest
assert not m5_verify.secret_failures(sweep,raw(secret/'outcomes.json.gz')) and sweep['fieldCount']==16

# The activation oracle remains a Go reference, with no native parity claim.
from provider_activation import cases as activation_cases, verify as verify_activation
reference_folder=ROOT/'evidence/m7/provider-activation-reference'
reference=load(reference_folder/'summary.json')
assert reference['sourceLock']==lock and reference['referenceChecks']==8
for path,digest in reference['provenance'].items():assert sha(ROOT/path)==digest,path
for path,digest in load(reference_folder/'index.json').items():assert sha(reference_folder/path)==digest,path
assert reference['goBinarySha256']==sha(ROOT/'.parity/bin/provider-activation-oracle')
reference_inputs=raw(reference_folder/'input.json.gz');reference_go=raw(reference_folder/'go.json.gz')
assert reference_inputs==activation_cases()
verify_activation(reference_inputs,reference_go)

checks_folder=ROOT/'evidence/m7/provider-publication-checks'
assert '186 passed; 0 failed; 22 ignored' in (checks_folder/'workspace.log').read_text()
assert not (checks_folder/'fmt.log').read_text()
assert 'Finished' in (checks_folder/'clippy.log').read_text()
assert 'Finished' in (checks_folder/'release.log').read_text()
assert 'Totals: 10 passed, 0 failed' in (checks_folder/'spec.log').read_text()
assert (checks_folder/'go-owner.log').read_text().count('--- PASS:')==20
assert 'gate cutover: FAIL' in (checks_folder/'cutover.log').read_text()
assert subprocess.check_output(['git','-C',str(ROOT/'.parity/go-src'),'status','--porcelain'],text=True).strip()==''
assert subprocess.check_output(['git','-C',str(ROOT/'.parity/go-src'),'rev-parse','HEAD'],text=True).strip()==lock['sourceCommit']
receipt={'scope':'Private publication composition and native schema compiler assembly. Actual owning Go publication/retirement/restore and SDK list projection; native executable metadata and retained handles. Public native provider dispatch, activation persistence, process and domain E2E remain open.',
'publicationComparisons':1210,'publicationComparisonMethod':'parsed JSON structural equality; object member order is immaterial; no value normalization','activationGoReferenceChecks':8,'ownerComparisons':checks,'normalization':'none','workspaceTests':186,'ignoredDifferentialFixtures':22,'owningGoTests':20,'reusedPublicRuntime':{'scenarios':102,'iterations':5,'basis':'release bytes and harness are identical; raw observations independently recomputed'},'reusedSecretSweep':{'fields':16,'failures':0},'sourceLock':lock,'rustReleaseSha256':sha(rust),'globalCutover':'FAIL; original manifest preserved','verificationScriptSha256':sha(Path(__file__))}
(ROOT/'evidence/m7/provider-publication-validation.json').write_text(json.dumps(receipt,indent=2)+'\n')
print(json.dumps(receipt,indent=2))
