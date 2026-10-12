#!/usr/bin/env python3
"""Compare dynamic descriptors and private candidate catalogs with owning Go functions."""
import argparse
import copy
import gzip
import json
import os
from pathlib import Path
import subprocess
from provider_adapter import ROOT, freeze_binary, sha


def cases():
    operation = {'id':'test.probe','version':1,'inputSchema':{'type':'object'},'outputSchema':{'type':'object'},'effect':'read'}
    manifest = {'provider':{'id':'test.provider','version':'v1'},'services':[{'id':'test.service','capabilityId':'test.capability','version':1,'operations':[operation]}]}
    rows = []
    def add(name, value=manifest, **extra):
        for standalone in [False, True]:
            rows.append({'case':name+'-'+('standalone' if standalone else 'platform'), 'manifest':copy.deepcopy(value),'generation':'test.provider-2','standalone':standalone,**extra})
    def op(name, key, value):
        m = copy.deepcopy(manifest);m['services'][0]['operations'][0][key] = value;add(name,m)
    add('minimal')
    for key,values in {
        'version':[0,-1,7,9223372036854775807],
        'description':['',' \t ',' custom description ','multi\nline','雪<&>'],
        'effect':['','read','READ',' read ','mutation','destructive','credential_bearing'],
        'inputSchema':[None,{}, {'required':['a','',42,None,'a']}, {'properties':{'defaulted':{'default':None},'explicit':{'default':False},'no_default':{'type':'string'}}}, {'number':9007199254740993,'float':1e-9}],
        'outputSchema':[None,{}, {'type':'object','properties':{'uri':{'type':'string'}}}],
        'taskSupport':['','sync_only','bridged',' bridged '],
        'validationSchema':['',' validation.v1 '],
        'observationSchema':['',' \t ',' custom.v1 '],
        'resourceKinds':[None,[],['container','vm','',None,'container']],
        'resourceCost':[None,{}, {'cpuCores':.5,'memoryBytes':9223372036854775807,'diskBytes':17,'tasks':1,'class':'workload','argumentBindings':{'cpuCores':'cpu','memoryBytes':'memory','diskBytes':'disk','tasks':'tasks'}}, {'argumentBindings':{}}, {'cpuCores':-0.0}],
        'requires':[None,[],[{'resourceType':'container','argument':'uri','required':True}]],
        'produces':[None,[],[{'resourceType':'container','sourcePath':'items[*].uri','selectorId':'chosen'}]],
        'idempotent':[False,True], 'supportsReadiness':[False,True],
        'resultTypes':[None,[],[{'id':'test.output','version':7,'selectors':[{'id':'chosen','sourcePath':'items[*].uri','cardinality':'many','labelPath':'items[*].name'}]}]],
        'outputType':['',' test.output '],
    }.items():
        for i,value in enumerate(values):op(key+'-'+str(i),key,value)
    for key,values in {'id':['',' service.id '],'capabilityId':['',' test.capability '],'version':[0,99]}.items():
        for i,value in enumerate(values):
            m=copy.deepcopy(manifest);m['services'][0][key]=value;add('service-'+key+'-'+str(i),m)
    for value in ['incus','test.provider',' test.provider ','']:
        m=copy.deepcopy(manifest);m['provider']['id']=value;add('provider-'+repr(value),m)
    for value in [None,[],[{'id':'empty.service'}]]:
        m=copy.deepcopy(manifest);m['services']=value;add('services-'+repr(value),m)
    m=copy.deepcopy(manifest);m['services'][0]['operations']=[operation,operation];add('duplicate-operation',m)
    m=copy.deepcopy(manifest);m['services']*=2;add('duplicate-service',m)
    producer={**operation,'id':'test.producer','effect':'mutation','produces':[{'resourceType':'container','sourcePath':'items[*].uri','selectorId':'chosen'},{'resourceType':'vm','sourcePath':'vms[*].uri'},{'resourceType':'container','sourcePath':'items[*].uri','selectorId':'chosen'}]}
    consumer={**operation,'id':'test.consumer','requires':[{'resourceType':'container','argument':'uri','required':True},{'resourceType':'container','argument':'other'},{'resourceType':'container','argument':' ','required':True},{'resourceType':'vm','argument':'vmUri'}]}
    linked=copy.deepcopy(manifest);linked['services'][0]['operations']=[consumer,producer];add('linked',linked)
    old=copy.deepcopy(linked);old['services'][0]['operations'][1]['id']='old.producer'
    add('replace-same-provider',linked,previous=old)
    old['provider']['id']='other.provider';add('preserve-other-provider',linked,previous=old)
    gone=copy.deepcopy(manifest);gone['services']=[];add('remove-old-edges',gone,previous=linked)
    collide=copy.deepcopy(linked);collide['services'][0]['operations'][1]['id']='get_host_info';add('builtin-name-first-wins',collide)
    add('generation-change',linked,generation='test.provider-99')
    add('opaque-generation',linked,generation=' opaque generation ')
    for generation in ['test.provider-2', ' opaque generation ', '', ' \t ', '雪/@g']:
        for present in [False, True]:
            for adapter in [False, True]:
                for query in dict.fromkeys([generation, generation.strip(), 'missing']):
                    for name in ['test.probe', ' test.probe ', 'missing', '']:
                        add('lookup-'+repr((generation,present,adapter,query,name)), generation=generation,
                            candidateLookup={'manifestPresent':present,'adapterPresent':adapter,'generation':query,'name':name})
    return rows


def run(go,rust,out):
    out.mkdir(parents=True,exist_ok=True);go=freeze_binary(go,'catalog-go');rust=freeze_binary(rust,'catalog-rust')
    paths=[Path(__file__),ROOT/'tools/parity/provider_adapter.py',ROOT/'tools/parity/build-provider-catalog-oracle.py',ROOT/'crates/host-agent/src/catalog.rs',ROOT/'crates/host-agent/src/provider/candidates.rs',ROOT/'crates/host-agent/src/catalog/registry.rs',ROOT/'crates/host-agent/src/resource.rs',ROOT/'crates/host-agent/src/provider/definition.rs',ROOT/'crates/host-agent/src/gojson.rs',ROOT/'crates/host-agent/src/goerr.rs',ROOT/'crates/host-agent/src/goerr/printable.rs',ROOT/'crates/host-agent/catalog/source.json',ROOT/'Cargo.lock']
    paths+=sorted((ROOT/'tools/parity/provider-catalog-oracle').iterdir())
    paths+=[ROOT/'.parity/go-src/internal/hostmcp/provider_candidate.go',ROOT/'.parity/go-src/internal/hostmcp/server.go',ROOT/'.parity/go-src/internal/tools/capability.go',ROOT/'.parity/go-src/internal/tools/catalog.go']
    provenance={str(p.relative_to(ROOT)):sha(p) for p in paths};lock=json.loads((ROOT/'baseline/source-lock.json').read_text())
    def pinned():
        def git(*args):return subprocess.check_output(['git','-C',str(ROOT/'.parity/go-src'),*args],text=True).strip()
        assert git('rev-parse','HEAD')==lock['sourceCommit'] and git('rev-parse','HEAD^{tree}')==lock['sourceTree'] and not git('status','--porcelain')
    pinned();source=cases();rows=[]
    for iteration in range(5):
        folder=out/str(iteration);folder.mkdir(parents=True,exist_ok=True)
        inp=folder/'input.json';inp.write_text(json.dumps(source,indent=2)+'\n')
        reference_raw=subprocess.check_output([str(go)],input=json.dumps(source),text=True,timeout=120)
        target=folder/'rust-output.json'
        env={**os.environ,'PARITY_PROVIDER_CATALOG_INPUT':str(inp.resolve()),'PARITY_PROVIDER_CATALOG_OUTPUT':str(target.resolve())}
        subprocess.run([str(rust),'catalog::tests::provider_catalog_fixture','--exact','--ignored'],env=env,capture_output=True,check=True,timeout=120)
        actual_raw=target.read_text();reference=json.loads(reference_raw);actual=json.loads(actual_raw)
        for side,raw in [('go',reference_raw),('rust',actual_raw)]:
            (folder/(side+'.json.gz')).write_bytes(gzip.compress(raw.encode(),compresslevel=1,mtime=0))
        target.unlink()
        assert len(reference)==len(actual)==len(source)
        for case,left,right in zip(source,reference,actual):
            passed=left==right;rows.append({'case':case['case'],'iteration':iteration,'pass':passed})
            if not passed:
                print('FAIL',case['case'],iteration,[k for k in left if left[k]!=right.get(k)],flush=True)
    assert provenance=={str(p.relative_to(ROOT)):sha(p) for p in paths};pinned()
    report={'scope':'T0 actual owning Go provider descriptor and private candidate snapshot projection via build-only overlay bridge. Candidate maps exercise absent manifests, absent adapter keys, null adapter entries, exact opaque generation/name lookup and private snapshot isolation; descriptor lookup uses the owning candidate capability definition. Adapter entries remain inert; connection/readiness/registry admission, actual activation and E2E generation affinity remain open. Raw descriptors, full active and private catalogs, generated edges and exact revision bytes compared.','sourceLock':lock,'provenance':provenance,'goBinarySha256':sha(go),'rustTestBinarySha256':sha(rust),'rows':rows,'passed':sum(r['pass'] for r in rows),'total':len(rows),'normalization':'none; encoded JSON strings including field order and catalog revisions are exact; complete raw outputs are losslessly gzipped with deterministic mtime'}
    (out/'summary.json').write_text(json.dumps(report,indent=2)+'\n')
    (out/'index.json').write_text(json.dumps({str(p.relative_to(out)):sha(p) for p in out.rglob('*') if p.is_file() and p.name!='index.json'},indent=2)+'\n')
    print('Provider catalogs:',report['passed'],'/',report['total']);return all(r['pass'] for r in rows)

if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--go',type=Path,required=True);p.add_argument('--rust-tests',type=Path,required=True);p.add_argument('--out',type=Path,required=True);a=p.parse_args();raise SystemExit(0 if run(a.go.resolve(),a.rust_tests.resolve(),a.out.resolve()) else 1)
