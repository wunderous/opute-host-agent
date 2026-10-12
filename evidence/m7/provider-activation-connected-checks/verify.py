"""Recompute activation parity and independently validate every time mask."""
import copy, datetime, gzip, hashlib, json, subprocess, sys
from pathlib import Path

ROOT=Path(__file__).resolve().parents[3]
sys.path.insert(0,str(ROOT/'tools/parity'))
from provider_activation_connected import cases
from parity import runner, canon, m5_verify
sha=lambda path:hashlib.sha256(Path(path).read_bytes()).hexdigest()
load=lambda path:json.loads(Path(path).read_text())
raw=lambda path:json.loads(gzip.decompress(Path(path).read_bytes()))
folder=ROOT/'evidence/m7/provider-activation-connected'
summary=load(folder/'summary.json');lock=load(ROOT/'baseline/source-lock.json')
assert summary['sourceLock']==lock
for path,digest in summary['provenance'].items():assert sha(ROOT/path)==digest,path
for path,digest in load(folder/'index.json').items():assert sha(folder/path)==digest,path
assert summary['goBinarySha256']==sha(ROOT/'.parity/bin/provider-activation-oracle')
assert summary['rustTestBinarySha256']==sha(ROOT/'target/debug/deps/opute_host_agent-6ae87315637d33b0')

def normalize(value,input,clock,side):
    value=copy.deepcopy(value)
    known={'','0001-01-01T00:00:00Z'}
    known.update(step['generation'].get('ActiveAt','') for step in input['activation']['steps'] if 'generation' in step)
    def fresh(text):
        if text in known:return text
        instant=datetime.datetime.fromisoformat(text.replace('Z','+00:00')).timestamp()
        assert clock[side+'Started']-0.001<=instant<=clock[side+'Finished']+0.001,text
        return '<verified-fresh-time>'
    for step in value['observations']:
        for provider,active in step['active'].items():assert active==step['generations'][active['ID']],provider
        for row in step['durable'] or []:
            if row['Status']=='active':
                actual=step['generations'][row['GenerationID']]['ActiveAt']
                assert row['ActiveAt']==('' if actual=='0001-01-01T00:00:00Z' else actual)
        for event in step['events']:event['at']=fresh(event['at'])
        for key in ['generations','active']:
            for generation in step[key].values():generation['ActiveAt']=fresh(generation['ActiveAt'])
        for row in step['durable'] or []:row['ActiveAt']=fresh(row['ActiveAt'])
    return value

corpus=cases();assert len(corpus)==24
rows=[]
for iteration in range(5):
    for batch,start in enumerate(range(0,len(corpus),8)):
        capture=folder/str(iteration)/str(batch)
        inputs,go,rust=(raw(capture/(name+'.json.gz')) for name in ['input','go','rust'])
        clock=load(capture/'clock.json')
        from urllib.parse import urlparse
        endpoint=urlparse(inputs[0]['activation']['steps'][0]['generation']['Endpoint'])
        assert endpoint.scheme=='http' and endpoint.hostname=='127.0.0.1' and endpoint.port>0
        base='http://127.0.0.1:'+str(endpoint.port)
        expected=json.loads(json.dumps(corpus[start:start+8]).replace('fixture://g1',base+'/g1/mcp').replace('fixture://g2',base+'/g2/mcp'))
        assert inputs==expected and len(inputs)==len(go)==len(rust)
        records=load(capture/'requests.json')
        def requests(values):
            return [{'path':r['path'],'payload':json.loads(r['body']),'headers':{k:v for k,v in r['headers'].items() if k in ['mcp-protocol-version','mcp-client-name','mcp-client-version','authorization']}} for r in values]
        assert requests(records['go'])==requests(records['rust'])
        for side in ['go','rust']:
            methods=[json.loads(row['body'])['method'] for row in records[side]]
            assert methods.count('server/discover')==methods.count('tools/list') and 'tools/call' in methods
            assert all(row['path'] in ['/g1/mcp','/g2/mcp'] for row in records[side])
        for input,left,right in zip(inputs,go,rust):
            assert normalize(left,input,clock,'go')==normalize(right,input,clock,'rust'),input['case']
            for value in [left,right]:
                observations=value['observations'];steps=input['activation']['steps']
                activations=[(step,observed) for step,observed in zip(steps,observations) if step['action']=='activate']
                if input['case'].startswith('connected-activation-'):
                    assert all(not observed['error'] and len(observed['mounts'])==1 for _,observed in activations)
                    assert any(o['error']=='provider adapter is closed' for o in observations)
                if input['case'].startswith('connected-chain-'):
                    assert len(activations[0][1]['mounts'])==2
                    assert len(activations[0][1]['mounts'][1]['inject'])==1
                if input['case'].startswith('mount-failure-'):
                    assert activations[0][1]['error'].startswith('mount provider generation:')
                    assert not activations[0][1]['mounts'] and not activations[0][1]['active']
                if input['case'].startswith('replacement-mount-failure-'):
                    failed=activations[1][1]
                    assert failed['error'].startswith('mount provider generation:')
                    assert failed['active']['opaque.provider']['ID']=='g1'
                    assert failed['generations']['g2']['State']=='failed'
                    assert len(failed['mounts'])==1 and failed['mounts'][0]['generation']=='g1'
                if input['case'].startswith('replacement-chain-'):
                    assert len(activations[1][1]['mounts'])==2
                    assert all(m['generation']=='g2' for m in activations[1][1]['mounts'])
                    assert any(step['action']=='probe' and step['id']=='g1' and observed['error']=='provider adapter is closed' for step,observed in zip(steps,observations))
            rows.append({'case':input['case'],'iteration':iteration,'batch':batch,'pass':True})
assert summary['rows']==rows and summary['passed']==summary['total']==len(rows)==120
checks=ROOT/'evidence/m7/provider-activation-connected-checks'
assert '187 passed; 0 failed; 23 ignored' in (checks/'workspace.log').read_text()
assert 'caller_abort_does_not_strand_accepted_activation ... ok' in (checks/'workspace.log').read_text()
assert 'ok  \tgithub.com/wunderous/host-agents/internal/hostmcp' in (checks/'go-owner.log').read_text()
assert 'Finished' in (checks/'clippy.log').read_text() and 'Finished' in (checks/'release.log').read_text()
assert not (checks/'fmt.log').read_text()
assert 'Totals: 10 passed, 0 failed' in (checks/'spec.log').read_text()
git=lambda *args:subprocess.check_output(['git','-C',str(ROOT/'.parity/go-src'),*args],text=True).strip()
assert git('rev-parse','HEAD')==lock['sourceCommit'] and git('rev-parse','HEAD^{tree}')==lock['sourceTree'] and not git('status','--porcelain')
# Reuse public captures only after exact binary/harness identity and full raw verification.
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

receipt={'scope':'Connected private activation and durable rollback with real loopback MCP adapters, mount dependencies, replacement, raw connection probes and explicit candidate cleanup. Public lifecycle wiring, supervised executables, restart and full M7-M13 E2E remain open.',
         'comparisons':120,'connectedCases':24,'iterations':5,'semanticMountAndRollbackChecksVerified':True,'requestPayloadsAndProtocolHeadersCompared':True,'reusedPublicRuntime':{'scenarios':102,'iterations':5,'basis':'unchanged release bytes and harness; raw comparisons independently recomputed'},'reusedSecretSweep':{'fields':16,'failures':0},'freshTimeBoundsAndDurableConsistencyVerified':True,'sourceBinaryAndCaptureHashesVerified':True,
         'workspaceTests':187,'ignoredDifferentialFixtures':23,'owningGoTests':'mount, unmount, swap and idempotent activation tests; five iterations','sourceLock':lock,
         'rustReleaseSha256':sha(ROOT/'target/release/opute-host-agent'),'verificationScriptSha256':sha(Path(__file__)),
         'globalCutover':'FAIL; original manifest preserved'}
(ROOT/'evidence/m7/provider-activation-connected-validation.json').write_text(json.dumps(receipt,indent=2)+'\n')
print(json.dumps(receipt,indent=2))
