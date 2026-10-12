#!/usr/bin/env python3
"""Owning Go provider publication metadata, retirement and catalog restore."""
import argparse,copy,gzip,json,os,random,subprocess
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from provider_adapter import ROOT,freeze_binary,sha

def manifest(provider='opaque.provider',family='test.family',name='test.probe',schema=None):
    return {'provider':{'id':provider,'version':'v1'},'services':[{'id':'service','capabilityId':family,'operations':[{'id':name,'version':1,'effect':'read','inputSchema':{'type':'object'} if schema is None else schema,'outputSchema':{'type':'object'}}]}]}

def cases():
    rows=[]
    def add(name,steps):
        for standalone in [False,True]:rows.append({'case':name+'-'+str(standalone),'publication':{'standalone':standalone,'steps':steps}})
    def register(value,generation='g1'):return {'action':'register','manifest':value,'generation':generation}
    first=manifest();second=manifest(name='test.replacement')
    add('empty',[{'action':'snapshot','handle':'before'},{'action':'readSnapshot','handle':'before'}])
    add('replace-retained',[register(first),{'action':'handle','id':'test.probe','handle':'old'},{'action':'snapshot','handle':'old'},register(second,'g2'),{'action':'readHandle','handle':'old'},{'action':'readSnapshot','handle':'old'},{'action':'retire','provider':'opaque.provider','generation':'g1'},{'action':'retire','provider':'opaque.provider','generation':'g2'},{'action':'retire','provider':'opaque.provider'}])
    add('replace-provider',[register(first,''),register(second,''),register(first,''),{'action':'retire','provider':'opaque.provider'}])
    add('displacement',[register(first),register(manifest('other.provider',name='other.probe'),'other-g1'),{'action':'retire','provider':'opaque.provider'},{'action':'retire','provider':'other.provider'}])
    for schema in [{'type':'object'}, {'type':'bad-type'}, {'type':'object','properties':{'x':{'type':'bad-type'}}}, {'type':'object','properties':{'x':{'pattern':'['}}}, {'type':'object','required':[1]}, {'type':'object','properties':{'x':False}}]:
        value=manifest('other.provider',name='other.probe',schema=schema)
        add('displacement-schema-'+repr(schema),[register(first),register(value,'other-g1'),{'action':'snapshot','handle':'after-failure'},register(manifest('third.provider','other.family','third.probe'),'third-g1'),{'action':'readSnapshot','handle':'after-failure'}])
    for i,provider in enumerate(['',' ','opaque.provider','other.provider',' Raw.Provider ','INCUS','incus','雪']):
        for gen in ['','g1',' g1 ',' ','opaque/generation@雪']:
            value=manifest(provider,name='test.probe')
            add('provider-'+str(i)+'-'+repr(gen),[register(value,gen),register(value,gen),{'action':'retire','provider':provider,'generation':gen}])
    for family in ['', ' ','test.family',' TEST.FAMILY ','test.family\u0085']:
        add('family-'+repr(family),[register(first),register(manifest('other.provider',family,'other.probe'),'other-g1')])
    for name in ['','Bad.Name','test.probe','get_host_info','x'*129,'bad name','雪']:
        add('operation-'+repr(name),[register(first),register(manifest('other.provider',name=name),'other-g1')])
    duplicate=copy.deepcopy(first);duplicate['services'][0]['operations']*=2;add('duplicate',[register(first),register(duplicate,'g2')])
    for services in [None,[]]:
        value=copy.deepcopy(first);value['services']=services;add('empty-services-'+repr(services),[register(first),register(value,'g2'),{'action':'retire','provider':'opaque.provider'}])
    generation={'ID':'g1','Provider':{'id':'opaque.provider','version':'v1'},'ManifestHash':'hash','Endpoint':'http://127.0.0.1:1','State':'active'}
    add('restore-active',[{'action':'lifecycle','generationState':generation},register(first),register(second,'g2'),{'action':'restore','provider':'opaque.provider','manifest':first}])
    for state in ['candidate','ready','draining','stopped','failed']:
        add('restore-'+state,[{'action':'lifecycle','generationState':{**generation,'State':state}},register(first),{'action':'restore','provider':'opaque.provider','manifest':first},{'action':'restore','provider':'opaque.provider'}])
    randomizer=random.Random(701329)
    for i in range(50):
        steps=[]
        for j in range(5):
            owner=randomizer.choice(['opaque.provider','other.provider']);family=randomizer.choice(['test.family','second.family','']);name=randomizer.choice(['test.probe','other.probe','bad name','get_host_info']);schema=randomizer.choice([{'type':'object'},{'type':'bad-type'}])
            steps.append(register(manifest(owner,family,name,schema),randomizer.choice(['g1','g2',''])))
        add('seeded-'+str(i),steps)
    assert len({row['case'] for row in rows})==len(rows)
    return rows

def run(go,rust,out):
    out.mkdir(parents=True,exist_ok=True);go=freeze_binary(go,'publication-go');rust=freeze_binary(rust,'publication-rust')
    paths=[Path(__file__),ROOT/'tools/parity/provider_adapter.py',ROOT/'tools/parity/build-provider-catalog-oracle.py',ROOT/'baseline/source-lock.json',ROOT/'Cargo.lock']
    paths+=sorted((ROOT/'crates/host-agent/src').rglob('*.rs'))
    paths+=sorted((ROOT/'crates/host-agent/src/catalog/schema').glob('*.json'))
    paths+=sorted((ROOT/'tools/parity/provider-catalog-oracle').iterdir())
    paths+=[ROOT/'crates/host-agent/Cargo.toml',ROOT/'Cargo.toml',ROOT/'.parity/go-src/internal/hostmcp/server.go',ROOT/'.parity/go-src/internal/capability/capability.go',ROOT/'.parity/go-src/internal/hostmcp/provider_candidate.go',ROOT/'.parity/go-src/internal/plan/schema.go']
    paths+=[ROOT/'.parity/go-src'/path for path in ['internal/hostmcp/recipe_run.go','internal/catalog/registry.go']]
    sources={str(p.relative_to(ROOT)):sha(p) for p in paths};lock=json.loads((ROOT/'baseline/source-lock.json').read_text())
    def pinned():
        def git(*args):return subprocess.check_output(['git','-C',str(ROOT/'.parity/go-src'),*args],text=True).strip()
        assert git('rev-parse','HEAD')==lock['sourceCommit'] and git('rev-parse','HEAD^{tree}')==lock['sourceTree'] and not git('status','--porcelain')
    pinned();source=cases();rows=[]
    def batch_run(job):
        iteration,batch,start=job
        chunk=source[start:start+12];capture=out/str(iteration)/str(batch);capture.mkdir(parents=True,exist_ok=True)
        inp=capture/'input.json';target=capture/'rust.json';inp.write_text(json.dumps(chunk)+'\n')
        reference=json.loads(subprocess.check_output([str(go)],input=json.dumps(chunk),text=True,timeout=120))
        subprocess.run([str(rust),'provider::publication::tests::publication_fixture','--exact','--ignored'],env={**os.environ,'PARITY_PROVIDER_PUBLICATION_INPUT':str(inp.resolve()),'PARITY_PROVIDER_PUBLICATION_OUTPUT':str(target.resolve())},capture_output=True,check=True,timeout=120)
        actual=json.loads(target.read_text());assert len(reference)==len(actual)==len(chunk)
        for name,value in [('input',chunk),('go',reference),('rust',actual)]:
            with gzip.open(capture/(name+'.json.gz'),'wt',compresslevel=1) as file:json.dump(value,file)
        inp.unlink();target.unlink();results=[]
        for case,left,right in zip(chunk,reference,actual):
            passed=left==right;results.append({'case':case['case'],'iteration':iteration,'batch':batch,'pass':passed})
            if not passed:
                from parity.canon import diff
                print('FAIL',case['case'],diff(left,right)[:5],flush=True)
        print('Publication batch',iteration,batch,'completed',flush=True)
        return results
    jobs=[(iteration,batch,start) for iteration in range(5) for batch,start in enumerate(range(0,len(source),12))]
    # Each process owns its loopback SDK connection, native registry, and capture
    # directory. No runtime lease or provider generation is shared across jobs.
    with ThreadPoolExecutor(max_workers=8) as workers:
        for results in workers.map(batch_run,jobs):rows.extend(results)
    assert sources=={str(p.relative_to(ROOT)):sha(p) for p in paths};pinned()
    summary={'scope':'Actual owning Go registerProviderServicesForGeneration, retireProviderCapabilities and restoreProviderManifest metadata ownership, provider executable descriptors and retained immutable snapshots. MCP refresh runs on Go; process execution, activation persistence and public native wiring remain open','sourceLock':lock,'provenance':sources,'goBinarySha256':sha(go),'rustTestBinarySha256':sha(rust),'normalization':'none; exact error and encoded publication bytes','rows':rows,'passed':sum(row['pass'] for row in rows),'total':len(rows)}
    (out/'summary.json').write_text(json.dumps(summary,indent=2)+'\n');(out/'index.json').write_text(json.dumps({str(p.relative_to(out)):sha(p) for p in out.rglob('*') if p.is_file() and p.name!='index.json'},indent=2)+'\n')
    print('Provider publications:',summary['passed'],'/',summary['total']);return summary['passed']==summary['total']
if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('--go',type=Path,required=True);p.add_argument('--rust-tests',type=Path,required=True);p.add_argument('--out',type=Path,required=True);a=p.parse_args();raise SystemExit(0 if run(a.go.resolve(),a.rust_tests.resolve(),a.out.resolve()) else 1)
