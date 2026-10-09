#!/usr/bin/env python3
"""Owning registry transactions with the assembled native schema compiler."""
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
    # One defect per schema keeps Go's randomized map traversal out of the
    # expected error choice. Compare whole observations, including revisions,
    # catalog snapshots and retained executable handles after each mutation.
    schemas = [
        {}, {'type':'object'}, {'type':'unknown'}, {'type':1},
        {'properties':{'x':None}}, {'properties':{'x':{'type':False}}},
        {'items':1}, {'allOf':[None]}, {'required':'x'},
        {'required':['x','x']}, {'minItems':-1}, {'minItems':1.5},
        {'minimum':'x'}, {'exclusiveMaximum':True}, {'additionalProperties':3},
        {'$defs':{'x':{}}}, {'$defs':{'x':{}},'$ref':'#/$defs/x'},
        {'$defs':{'x':False},'$ref':'#/$defs/x'},
        {'$ref':'#missing'}, {'$ref':'#/$defs/missing'}, {'$ref':'%'},
        {'$ref':'https://example.invalid/schema'}, {'$id':'%'},
        {'$id':'https://example.invalid/root','$ref':'#'},
        {'$anchor':'anchor','$ref':'#anchor'}, {'$anchor':'bad anchor'},
        {'$dynamicAnchor':'anchor','$dynamicRef':'#anchor'},
        {'$defs':{'a/b~c':{}},'$ref':'#/$defs/a~1b~0c'},
        {'$ref':'#/properties/x','properties':{'x':{}}},
        {'pattern':'['}, {'pattern':'(?=a)'}, {'pattern':r'\p{L}+'},
        {'pattern':'(?i:k)'}, {'pattern':'a{1001}'},
        {'patternProperties':{'[':{}}},
        {'patternProperties':{'[a-z]+':{'type':'string'}}},
        {'dependentRequired':{'x':['x','x']}}, {'enum':[1,1]},
        {'enum':['x','x']}, {'enum':['x',1]}, {'enum':[]},
        {'$schema':'http://json-schema.org/draft-07/schema#','exclusiveMinimum':True},
        {'$schema':'https://json-schema.org/draft/2020-12/schema','items':[{}]},
    ]
    for field in ['inputSchema','outputSchema']:
        for i,schema in enumerate(schemas):
            schema=copy.deepcopy(schema)
            if field=='inputSchema' and 'type' not in schema:
                schema['type']='object'
            new=value(generation='g2',marker='two',**{field:schema})
            old=[step(),{'action':'resolve','id':'test.operation','revision':'$current','handle':'old'}]
            after=[{'action':'invoke','handle':'old'},
                   {'action':'resolve','id':'test.operation','revision':'$current','handle':'current'},
                   {'action':'invoke','handle':'current'}]
            for action in ['register','registerCapability','registration','upsert','upsertCapability']:
                candidate=new if action.startswith('upsert') else value('new.operation',generation='g2',**{field:schema})
                add('schema-'+field+'-'+str(i)+'-'+action,
                    old+[step(action,candidate,provider='test.provider',implementation='provider:test.provider')]+after)
            for action in ['replaceGeneration','replaceProvider']:
                for order in ['first','last']:
                    other=value('other.operation',generation='g2')
                    values=[new,other] if order=='first' else [other,new]
                    add('schema-'+field+'-'+str(i)+'-'+action+'-'+order,
                        old+[{'action':action,'generation':'g2','provider':'test.provider','values':values}]+after)
            add('schema-'+field+'-'+str(i)+'-base',[{'action':'validateBase'}],base=[value(**{field:schema})])
    return rows


def run(go,rust,out):
    out.mkdir(parents=True,exist_ok=True);go=freeze_binary(go,'registry-go');rust=freeze_binary(rust,'registry-rust')
    paths=[Path(__file__),ROOT/'tools/parity/provider_adapter.py',ROOT/'tools/parity/build-provider-catalog-oracle.py',ROOT/'crates/host-agent/src/catalog.rs',ROOT/'crates/host-agent/src/catalog/registry.rs',ROOT/'crates/host-agent/src/resource.rs',ROOT/'crates/host-agent/src/provider/definition.rs',ROOT/'crates/host-agent/src/gojson.rs',ROOT/'crates/host-agent/src/goerr.rs',ROOT/'crates/host-agent/src/goerr/printable.rs',ROOT/'Cargo.lock']
    paths+=sorted((ROOT/'tools/parity/provider-catalog-oracle').iterdir())
    paths+=[ROOT/'.parity/go-src/internal/catalog/registry.go',ROOT/'.parity/go-src/internal/hostmcp/provider_candidate.go']
    paths += [ROOT/'baseline/source-lock.json', ROOT/'crates/host-agent/src/catalog/schema.rs']
    paths += [ROOT/'crates/host-agent/src/plan/schema.rs',
              ROOT/'crates/host-agent/src/schema.rs',
              ROOT/'.parity/go-src/internal/capability/adapters.go',
              ROOT/'.parity/go-src/internal/plan/schema.go']
    paths += sorted((ROOT/'crates/host-agent/src/catalog/schema').iterdir())
    library=Path('/home/houman/go/pkg/mod/github.com/google/jsonschema-go@v0.4.3/jsonschema')
    paths += sorted(library.glob('*.go'))
    for package in ['regexp','regexp/syntax','unicode']:
        paths += sorted((ROOT/'.parity/setup/go/src'/package).glob('*.go'))
    hashes={str(p):sha(p) for p in paths if p.is_file()};lock=json.loads((ROOT/'baseline/source-lock.json').read_text())
    def pinned():
        def git(*args):return subprocess.check_output(['git','-C',str(ROOT/'.parity/go-src'),*args],text=True).strip()
        assert git('rev-parse','HEAD')==lock['sourceCommit'] and git('rev-parse','HEAD^{tree}')==lock['sourceTree'] and not git('status','--porcelain')
    pinned();source=cases();rows=[];atomic_checks=0
    for iteration in range(5):
        folder=out/str(iteration);folder.mkdir(parents=True,exist_ok=True);inp=folder/'input.json';inp.write_text(json.dumps(source,indent=2)+'\n')
        reference=json.loads(subprocess.check_output([str(go)],input=json.dumps(source),text=True,timeout=60))
        target=folder/'rust-output.json';env={**os.environ,'PARITY_PROVIDER_REGISTRY_INPUT':str(inp.resolve()),'PARITY_PROVIDER_REGISTRY_OUTPUT':str(target.resolve())}
        subprocess.run([str(rust),'catalog::registry::tests::registry_fixture','--exact','--ignored'],env=env,capture_output=True,check=True,timeout=60)
        actual=json.loads(target.read_text());target.unlink()
        (folder/'input.json.gz').write_bytes(gzip.compress(inp.read_bytes(),mtime=0));inp.unlink()
        for side,data in [('go',reference),('rust',actual)]:
            (folder/(side+'.json.gz')).write_bytes(gzip.compress(json.dumps(data).encode(),mtime=0))
        assert len(reference)==len(actual)==len(source)
        for case,left,right in zip(source,reference,actual):
            if case['case'].startswith('schema-') and len(case['registry']['steps'])==6:
                for observed in [left,right]:
                    observations=observed['observations']
                    assert observations[3]['result']=={'marker':'one','generation':'g1'}
                    if observations[2]['error']:
                        assert observations[2]['snapshot']==observations[1]['snapshot']
                        assert observations[4]['resolved']==observations[1]['resolved']
                        assert observations[5]['result']=={'marker':'one','generation':'g1'}
                        atomic_checks+=1
            passed=left==right;rows.append({'case':case['case'],'iteration':iteration,'pass':passed})
            if not passed:print('FAIL',case['case'],iteration,[(l['error'],r['error'],l['snapshot']==r['snapshot']) for l,r in zip(left['observations'],right['observations']) if l!=r],flush=True)
    assert hashes=={str(p):sha(p) for p in paths if p.is_file()};pinned()
    report={'scope':'T0 owning Go executable registry vs Rust registry with assembled native schema compiler: authorization/identity, metadata and schema validation, atomic generation/provider replacement, lookup/upsert/removal and retained executable handles. Malformed, local/URI/reference and regexp schemas exercise every registration/replacement path and base validation. Ambiguous multi-negative cost ordering, runtime lifecycle and public activation remain open.','sourceLock':lock,'provenance':hashes,'goBinarySha256':sha(go),'rustTestBinarySha256':sha(rust),'rows':rows,'passed':sum(r['pass'] for r in rows),'total':len(rows),'normalization':'none; observations, encoded snapshots and revisions and invoked old/new executable markers are exact'}
    report['rejectedSchemaAtomicChecks']=atomic_checks
    (out/'summary.json').write_text(json.dumps(report,indent=2)+'\n');(out/'index.json').write_text(json.dumps({str(p.relative_to(out)):sha(p) for p in out.rglob('*') if p.is_file() and p.name!='index.json'},indent=2)+'\n');print('Provider registry:',report['passed'],'/',report['total']);return all(r['pass'] for r in rows)

if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--go',type=Path,required=True);p.add_argument('--rust-tests',type=Path,required=True);p.add_argument('--out',type=Path,required=True);a=p.parse_args();raise SystemExit(0 if run(a.go.resolve(),a.rust_tests.resolve(),a.out.resolve()) else 1)
