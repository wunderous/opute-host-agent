#!/usr/bin/env python3
"""Owning registry transactions and metadata checks; native schema compilation remains open."""
import argparse
import copy
import gzip
import json
import os
from pathlib import Path
import subprocess
from provider_adapter import ROOT,freeze_binary,sha


def cases():
    rows=[]
    def value(name='test.operation',provider='test.provider',generation='g1',marker='one',**operation):
        return {'manifest':{'provider':{'id':provider,'version':'v1'},'services':[{'id':'test.service','capabilityId':'test.family','operations':[{'id':name,'version':1,'effect':'read','inputSchema':{'type':'object'},'outputSchema':{'type':'object'},**operation}]}]},'generation':generation,'marker':marker}
    options={'AuthorizedProviders':{'test.provider':True,'other.provider':True,'false.provider':False}}
    v=value()
    def add(name,steps,base=None,opts=options):rows.append({'case':name,'registry':{'base':base or [],'options':opts,'steps':steps}})
    def step(action='register',v=v,**extra):return {'action':action,'value':copy.deepcopy(v),**extra}
    for action in ['register','registerCapability','registration','upsert','upsertCapability']:
        s=step(action,provider='test.provider',implementation='provider:test.provider')
        add(action,[s,{'action':'lookup','id':'test.operation'},{'action':'resolve','id':'test.operation','revision':'$current','handle':'saved'},{'action':'invoke','handle':'saved'},s])
    for action in ['register','registerCapability','upsertCapability']:
        add(action+'-nil',[step(action,None,provider='test.provider',implementation='provider:test.provider')])
    for action in ['register','registerCapability','upsertCapability','registration','upsert']:
        for version in [0,-1,2]:
            x=value();x['patch']={'version':version};add(action+'-version-'+str(version),[step(action,x,provider='test.provider',implementation='provider:test.provider')])
    for name in ['','a','A','test operation','1test','test/operation','a'*128,'a'*129,'test.雪',' test.operation ','test__-1']:
        add('name-'+repr(name),[step(v=value(name))])
    for action in ['registration','upsert']:
        add(action+'-metadata-only',[step(action,provider='test.provider',implementation='provider:test.provider',nilExecutable=True)])
    add('registration-mismatch',[step('registration',provider='test.provider',implementation='provider:test.provider',descriptorPatch={'name':'different'})])
    add('registration-operation-mismatch',[step('registration',provider='test.provider',implementation='provider:test.provider',descriptorPatch={'operationId':'different'})])
    for provider in ['','unknown.provider','false.provider',' Test.Provider ','OTHER.PROVIDER']:
        add('registration-provider-'+repr(provider),[step('registration',provider=provider,implementation='provider:test.provider')])
    for implementation in ['',' ','different','provider:test.provider']:
        add('implementation-'+repr(implementation),[step('registration',provider='test.provider',implementation=implementation)])
    for allowed in [{},{'provider:test.provider':False},{'other':True},{'provider:test.provider':True}]:
        add('allow-'+repr(allowed),[step()],opts={**options,'AllowedImplementations':allowed})
    for provider in ['','',' unknown.provider ',' TEST.PROVIDER ']:
        add('authorize-'+repr(provider),[{'action':'authorize','provider':provider},step(v=value(provider=provider))])
    add('base-conflict',[step()],base=[v])
    for key,values in {
        'inputSchema':[None,{}, {'type':'array'}], 'outputSchema':[None,{}],
        'effect':['','read','READ','mutation','destructive','credential_bearing','shell'],
        'taskSupport':['','sync_only','bridged',' bridge '],
        'resourceKinds':[['vm'],[' vm '],['unknown'],['database'],['container']],
        'outputType':['result.missing'],
        'resourceCost':[{}, {'cpuCores':-.1},{'memoryBytes':-1},{'diskBytes':-1},{'tasks':-1},{'class':'workload'},{'class':'heavy'},{'argumentBindings':{'tasks':'tasks'}},{'tasks':1,'argumentBindings':{'tasks':'missing'}},{'diskBytes':0,'argumentBindings':{'diskBytes':'disk'}}],
    }.items():
        for i,data in enumerate(values):add(key+'-'+str(i),[step(v=value(**{key:data}))])
    typed={'type':'object','properties':{'uri':{'type':'string'},'wrong':{'type':'number'},'items':{'type':'array','items':{'type':'object','properties':{'uri':{'type':'string'},'name':{'type':'string'}}}},'disk':{'type':'string'},'tasks':{'type':'integer'}}}
    for direction in ['requires','produces']:
        path='argument' if direction=='requires' else 'sourcePath'
        for kind in ['','vm',' vm ','not-kind']:
            for binding_path in ['uri','missing','wrong','items[].uri','items.uri',' uri ','.uri']:
                x=value(inputSchema=typed,outputSchema=typed,**{direction:[{'resourceType':kind,path:binding_path}]})
                add(direction+'-'+kind+'-'+binding_path,[step(v=x)])
    selectors=[{'id':'test.output','version':1,'selectors':[{'id':'uri','sourcePath':'items[].uri','labelPath':'items[].name','cardinality':'many'}]}]
    for cardinality in ['', ' ', 'one','many',' many ','unknown']:
        types=copy.deepcopy(selectors);types[0]['selectors'][0]['cardinality']=cardinality
        add('selector-cardinality-'+repr(cardinality),[step(v=value(outputSchema=typed,outputType='test.output',resultTypes=types))])
    for patch in [{'id':''},{'version':0},{'version':2}]:
        types=copy.deepcopy(selectors);types[0].update(patch);add('selector-type-'+repr(patch),[step(v=value(outputSchema=typed,outputType='test.output',resultTypes=types))])
    for patch in [{'id':''},{'sourcePath':'missing'},{'labelPath':'missing'}]:
        types=copy.deepcopy(selectors);types[0]['selectors'][0].update(patch);add('selector-path-'+repr(patch),[step(v=value(outputSchema=typed,outputType='test.output',resultTypes=types))])
    for binding in [{'sourcePath':'items[].uri','selectorId':'uri'},{'sourcePath':'uri','selectorId':'uri'},{'sourcePath':'items[].uri','selectorId':'missing'}]:
        add('selector-binding-'+repr(binding),[step(v=value(outputSchema=typed,outputType='test.output',resultTypes=selectors,produces=[{'resourceType':'container',**binding}]))])
    add('known-restricted',[step(v=value(resourceKinds=['container']))],opts={**options,'KnownResourceKinds':{'vm':True}})
    x=value(inputSchema=typed,resourceCost={'tasks':1,'argumentBindings':{'tasks':'tasks'},'diskBytes':0,'argumentBindings':{'diskBytes':'disk','tasks':'tasks'}})
    add('dynamic-cost-valid',[step(v=x)])
    for action in ['replaceGeneration','replaceProvider']:
        new=value(generation='g2',marker='two')
        replace={'action':action,'generation':'g2','provider':'test.provider','values':[new]}
        start=[step(),{'action':'resolve','id':'test.operation','revision':'$current','handle':'old'}]
        add(action+'-old-handle',start+[replace,{'action':'invoke','handle':'old'},{'action':'resolve','id':'test.operation','revision':'$current','handle':'new'},{'action':'invoke','handle':'new'}])
        for label,values in [('nil',[None]),('duplicate',[new,new]),('bad-effect',[value(generation='g2',effect='bad')]),('other-owner',[value(generation='g2',provider='other.provider')]),('mixed-owner',[new,value('other.operation',provider='other.provider',generation='g2')]),('wrong-generation',[v]),('empty',[])]:
            add(action+'-'+label,start+[{**replace,'values':values},{'action':'invoke','handle':'old'}])
        add(action+'-blank',start+[{**replace,'generation':' ','provider':' '},{'action':'invoke','handle':'old'}])
        add(action+'-base-conflict',[step(v=value('overlay.operation')),{**replace,'values':[new]}],base=[v])
        add(action+'-overlay-conflict',[step(v=value(provider='other.provider')),{**replace,'values':[new]}])
    for other in ['test.provider',' Test.Provider ','other.provider']:
        add('upsert-owner-'+other,[step(),step('upsert',value(marker='changed'),provider=other,implementation='provider:test.provider')])
    add('upsert-downgrade',[step(v=value(version=3)),step('upsert',v,provider='test.provider',implementation='provider:test.provider')])
    add('upsert-changed-implementation',[step(),step('upsert',v,provider='test.provider',implementation='different')])
    add('resolve-errors',[step(),{'action':'resolve','id':'test.operation','revision':''},{'action':'resolve','id':'test.operation','revision':'stale'},{'action':'resolve','id':'missing','revision':'$current'}])
    add('base-not-executable',[{'action':'resolve','id':'test.operation','revision':'$current'},{'action':'lookup','id':'test.operation'}],base=[v])
    add('unregister',[step(),{'action':'unregister','id':'test.operation'},{'action':'lookup','id':'test.operation'},{'action':'unregister','id':'test.operation'}])
    for owner,families in [('', ['test.family']),('test.provider',[]),('test.provider',[' ']),('test.provider',['test.family']),('other.provider',[' test.family ']),('unknown.provider',['test.family']),('test.provider',['absent'])]:
        add('displace-'+repr((owner,families)),[step(),step(v=value('other.operation',provider='other.provider')),{'action':'displace','provider':owner,'families':families}])
    for field,data in [('outputSchema',None),('inputSchema',None),('inputSchema',{}),('effect','bad'),('version',0)]:
        add('validate-base-'+field,[{'action':'validateBase'}],base=[value(**{field:data})])
    return rows


def run(go,rust,out):
    out.mkdir(parents=True,exist_ok=True);go=freeze_binary(go,'registry-go');rust=freeze_binary(rust,'registry-rust')
    paths=[Path(__file__),ROOT/'tools/parity/provider_adapter.py',ROOT/'tools/parity/build-provider-catalog-oracle.py',ROOT/'crates/host-agent/src/catalog.rs',ROOT/'crates/host-agent/src/catalog/registry.rs',ROOT/'crates/host-agent/src/resource.rs',ROOT/'crates/host-agent/src/provider/definition.rs',ROOT/'crates/host-agent/src/gojson.rs',ROOT/'crates/host-agent/src/goerr.rs',ROOT/'crates/host-agent/src/goerr/printable.rs',ROOT/'Cargo.lock']
    paths+=sorted((ROOT/'tools/parity/provider-catalog-oracle').iterdir())
    paths+=[ROOT/'.parity/go-src/internal/catalog/registry.go',ROOT/'.parity/go-src/internal/hostmcp/provider_candidate.go']
    hashes={str(p.relative_to(ROOT)):sha(p) for p in paths};lock=json.loads((ROOT/'baseline/source-lock.json').read_text())
    def pinned():
        def git(*args):return subprocess.check_output(['git','-C',str(ROOT/'.parity/go-src'),*args],text=True).strip()
        assert git('rev-parse','HEAD')==lock['sourceCommit'] and git('rev-parse','HEAD^{tree}')==lock['sourceTree'] and not git('status','--porcelain')
    pinned();source=cases();rows=[]
    for iteration in range(5):
        folder=out/str(iteration);folder.mkdir(parents=True,exist_ok=True);inp=folder/'input.json';inp.write_text(json.dumps(source,indent=2)+'\n')
        reference=json.loads(subprocess.check_output([str(go)],input=json.dumps(source),text=True,timeout=60))
        target=folder/'rust-output.json';env={**os.environ,'PARITY_PROVIDER_REGISTRY_INPUT':str(inp.resolve()),'PARITY_PROVIDER_REGISTRY_OUTPUT':str(target.resolve())}
        subprocess.run([str(rust),'catalog::registry::tests::registry_fixture','--exact','--ignored'],env=env,capture_output=True,check=True,timeout=60)
        actual=json.loads(target.read_text());target.unlink()
        for side,data in [('go',reference),('rust',actual)]:
            (folder/(side+'.json.gz')).write_bytes(gzip.compress(json.dumps(data).encode(),mtime=0))
        assert len(reference)==len(actual)==len(source)
        for case,left,right in zip(source,reference,actual):
            passed=left==right;rows.append({'case':case['case'],'iteration':iteration,'pass':passed})
            if not passed:print('FAIL',case['case'],iteration,[(l['error'],r['error'],l['snapshot']==r['snapshot']) for l,r in zip(left['observations'],right['observations']) if l!=r],flush=True)
    assert hashes=={str(p.relative_to(ROOT)):sha(p) for p in paths};pinned()
    report={'scope':'T0 owning Go executable registry vs Rust registry: authorization/identity, metadata validation, atomic generation/provider replacement, lookup/upsert/removal and retained executable handles. Rust schema callback is fixture-only and asserts a declared reference-free well-formed schema subset. Native schema compilation, malformed schema parity, ambiguous multi-negative cost ordering, runtime lifecycle and public activation remain open.','sourceLock':lock,'provenance':hashes,'goBinarySha256':sha(go),'rustTestBinarySha256':sha(rust),'rows':rows,'passed':sum(r['pass'] for r in rows),'total':len(rows),'normalization':'none; observations, encoded snapshots and revisions and invoked old/new executable markers are exact'}
    (out/'summary.json').write_text(json.dumps(report,indent=2)+'\n');(out/'index.json').write_text(json.dumps({str(p.relative_to(out)):sha(p) for p in out.rglob('*') if p.is_file() and p.name!='index.json'},indent=2)+'\n');print('Provider registry:',report['passed'],'/',report['total']);return all(r['pass'] for r in rows)

if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--go',type=Path,required=True);p.add_argument('--rust-tests',type=Path,required=True);p.add_argument('--out',type=Path,required=True);a=p.parse_args();raise SystemExit(0 if run(a.go.resolve(),a.rust_tests.resolve(),a.out.resolve()) else 1)
