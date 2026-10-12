#!/usr/bin/env python3
"""Owning activation metadata/durable parity; adapter/process E2E stays open."""
import argparse, copy, datetime, gzip, json, os, subprocess, time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from provider_adapter import ROOT, freeze_binary, sha
from provider_activation import cases as reference_cases
from provider_publication import manifest


def cases():
    rows = reference_cases()
    def add(name, steps):
        for mode in [False, True]:
            rows.append({'case': name+'-'+str(mode), 'activation': {'standalone':mode, 'steps':copy.deepcopy(steps)}})
    def generation(id='g1', state='candidate', provider='opaque.provider', version='v1', sessions=0):
        return {'ID':id,'Provider':{'id':provider,'version':version},'ManifestHash':'hash-'+id,
                'Endpoint':'http://127.0.0.1:1','State':state,'Sessions':sessions,'CreatedAt':'2026-10-01T00:00:00Z'}
    def declaration(schema=None):
        value=manifest(family='opute.capability.test.v1',schema=schema)
        value.update(schema='opute-provider-install-manifest.v1',provides=[{'id':'opute.capability.test.v1','version':1}],
                     validation={'capability':'opute.capability.test.v1','operation':'test.probe'})
        value['services'][0]['version']=1
        return value
    def restore(g):return {'action':'restore','generation':g}
    def candidate(id='g1', value=None):return {'action':'candidate','id':id,'manifest':declaration() if value is None else value}
    def activate(id='g1', **meta):return {'action':'activate','metadata':{'activate':True,'providerGenerationId':id,**meta}}
    start=[restore(generation()),candidate()]
    for state in ['candidate','ready','active','draining','stopped','failed','unknown']:
        for present in [False,True]:
            add('state-'+state+'-'+str(present),[restore(generation(state=state))]+([candidate()] if present else [])+[activate(),activate()])
    for i,enabled in enumerate([None,False,True,'true',1,[],{}]):
        for id in ['', ' ', 'missing', ' g1 ']:
            add('metadata-'+str(i)+'-'+repr(id),start+[{'action':'activate','metadata':{'activate':enabled,'providerGenerationId':id}}])
    for i,capabilities in enumerate([None,[],['test'],['opute.capability.test.v1'],['opute.capability.test'],['unknown'],[' ',1,None],['unknown','test'],True,'test']):
        add('filter-'+str(i),start+[activate(runtime={'capabilities':capabilities})])
        add('document-filter-'+str(i),start+[activate(recipeDocument={'runtime':{'capabilities':capabilities}})])
    add('runtime-precedes-document',start+[activate(runtime={},recipeDocument={'runtime':{'capabilities':['unknown']}})])
    add('malformed-runtime-falls-back',start+[activate(runtime=[],recipeDocument={'runtime':{'capabilities':['test']}})])
    for i,schema in enumerate([{'type':'bad-type'},{'type':'object','properties':{'x':{'type':'bad-type'}}},{'type':'object','properties':{'x':{'pattern':'['}}}]):
        add('publish-failure-'+str(i),[restore(generation()),candidate(value=declaration(schema)),activate(),activate()])
    add('denial-then-retry',start+[{'action':'admission','denial':'policy denied'},activate(),{'action':'admission'},activate()])
    old=declaration();new=declaration();new['services'][0]['operations'][0]['id']='test.replacement'
    prefix=[restore(generation(state='active')),{'action':'register','id':'g1','manifest':old},restore(generation('g2')),candidate('g2',new)]
    add('replacement',prefix+[activate('g2'),activate('g2')])
    add('replacement-denied',prefix+[{'action':'admission','denial':'denied replacement'},activate('g2')])
    add('replacement-filter-refused',prefix+[activate('g2',runtime={'capabilities':['unknown']})])
    for i,schema in enumerate([{'type':'bad-type'},{'type':'object','properties':{'x':{'pattern':'['}}}]):
        add('replacement-publish-failure-'+str(i),prefix+[candidate('g2',declaration(schema)),activate('g2')])
    for previous_state in ['draining','active','failed']:
        for sessions in [0,1]:
            add('rollback-'+previous_state+'-'+str(sessions),[restore(generation(state=previous_state)),
                {'action':'remember','manifest':old},restore(generation('g2','active',sessions=sessions)),
                {'action':'rollback','id':'g2','previousID':'g1','manifest':new,'cause':'fixture activation failure'}])
    add('rollback-no-predecessor',[restore(generation(state='active')),
        {'action':'rollback','id':'g1','manifest':old,'cause':'fixture failure'}])
    add('rollback-closed-store',[restore(generation(state='draining')),{'action':'remember','manifest':old},
        restore(generation('g2','active')),{'action':'closeState'},
        {'action':'rollback','id':'g2','previousID':'g1','manifest':new,'cause':'fixture failure'}])
    assert len({row['case'] for row in rows})==len(rows)
    return rows


def checked(output, case, started, finished):
    value=copy.deepcopy(output)
    fixed={'','0001-01-01T00:00:00Z'}
    fixed.update(step['generation'].get('ActiveAt','') for step in case['activation']['steps'] if 'generation' in step)
    def timestamp(text):
        if text in fixed:return text
        instant=datetime.datetime.fromisoformat(text.replace('Z','+00:00')).timestamp()
        assert started-0.001<=instant<=finished+0.001,(text,started,finished)
        return '<checked-fresh-time>'
    for step in value['observations']:
        for key,generation in step['active'].items():assert generation==step['generations'][generation['ID']],key
        for row in step['durable'] or []:
            if row['Status']=='active':
                observed=step['generations'][row['GenerationID']]['ActiveAt']
                assert row['ActiveAt']==('' if observed=='0001-01-01T00:00:00Z' else observed)
        for event in step['events']:event['at']=timestamp(event['at'])
        for collection in ['generations','active']:
            for generation in step[collection].values():generation['ActiveAt']=timestamp(generation['ActiveAt'])
        for row in step['durable'] or []:row['ActiveAt']=timestamp(row['ActiveAt'])
    return value


def run(go,rust,out):
    out.mkdir(parents=True,exist_ok=True)
    go=freeze_binary(go,'activation-parity-go');rust=freeze_binary(rust,'activation-parity-rust')
    lock=json.loads((ROOT/'baseline/source-lock.json').read_text())
    paths=[Path(__file__),ROOT/'tools/parity/provider_activation.py',ROOT/'tools/parity/provider_publication.py',
           ROOT/'tools/parity/provider_adapter.py',ROOT/'tools/parity/build-provider-activation-oracle.py',ROOT/'baseline/source-lock.json',ROOT/'Cargo.lock',ROOT/'Cargo.toml',ROOT/'crates/host-agent/Cargo.toml']
    paths+=sorted((ROOT/'tools/parity/provider-activation-oracle').iterdir())+sorted((ROOT/'crates/host-agent/src').rglob('*.rs'))+sorted((ROOT/'crates/host-agent/src/catalog/schema').glob('*.json'))
    provenance={str(p.relative_to(ROOT)):sha(p) for p in paths}
    def pinned():
        git=lambda *args:subprocess.check_output(['git','-C',str(ROOT/'.parity/go-src'),*args],text=True).strip()
        assert git('rev-parse','HEAD')==lock['sourceCommit'] and git('rev-parse','HEAD^{tree}')==lock['sourceTree'] and not git('status','--porcelain')
    pinned();source=cases()
    def batch_run(job):
        iteration,batch,start=job;chunk=source[start:start+8];folder=out/str(iteration)/str(batch);folder.mkdir(parents=True,exist_ok=True)
        inp=folder/'input.json';target=folder/'rust.json';inp.write_text(json.dumps(chunk)+'\n')
        clock={'goStarted':time.time()};left=json.loads(subprocess.check_output([str(go)],input=json.dumps(chunk),text=True,timeout=120));clock['goFinished']=time.time()
        clock['rustStarted']=time.time();subprocess.run([str(rust),'provider::activation::tests::activation_fixture','--exact','--ignored'],env={**os.environ,'PARITY_PROVIDER_ACTIVATION_INPUT':str(inp.resolve()),'PARITY_PROVIDER_ACTIVATION_OUTPUT':str(target.resolve())},capture_output=True,check=True,timeout=120);clock['rustFinished']=time.time();right=json.loads(target.read_text())
        assert len(chunk)==len(left)==len(right)
        for name,value in [('input',chunk),('go',left),('rust',right)]:
            with gzip.open(folder/(name+'.json.gz'),'wt',compresslevel=1) as file:json.dump(value,file)
        (folder/'clock.json').write_text(json.dumps(clock)+'\n');inp.unlink();target.unlink();rows=[]
        for case,a,b in zip(chunk,left,right):
            a=checked(a,case,clock['goStarted'],clock['goFinished']);b=checked(b,case,clock['rustStarted'],clock['rustFinished']);passed=a==b
            rows.append({'case':case['case'],'iteration':iteration,'batch':batch,'pass':passed})
            if not passed:
                from parity.canon import diff
                print('FAIL',case['case'],diff(a,b)[:5],flush=True)
        print('Activation batch',iteration,batch,'completed',flush=True);return rows
    jobs=[(iteration,batch,start) for iteration in range(5) for batch,start in enumerate(range(0,len(source),8))]
    rows=[]
    with ThreadPoolExecutor(max_workers=8) as workers:
        for result in workers.map(batch_run,jobs):rows.extend(result)
    pinned();assert provenance=={str(p.relative_to(ROOT)):sha(p) for p in paths}
    summary={'scope':'Owning Go activation metadata and durable composition, without adapters, transport, public native wiring or outer recipe cleanup. Full M7-M13 remains open.',
             'sourceLock':lock,'provenance':provenance,'goBinarySha256':sha(go),'rustTestBinarySha256':sha(rust),
             'normalization':'only fresh event/activation timestamps after real-clock bounds and active/durable consistency checks; all other parsed JSON values exact',
             'rows':rows,'passed':sum(row['pass'] for row in rows),'total':len(rows)}
    (out/'summary.json').write_text(json.dumps(summary,indent=2)+'\n');(out/'index.json').write_text(json.dumps({str(p.relative_to(out)):sha(p) for p in out.rglob('*') if p.is_file() and p.name!='index.json'},indent=2)+'\n')
    print('Provider activation:',summary['passed'],'/',summary['total']);return summary['passed']==summary['total']


if __name__=='__main__':
    parser=argparse.ArgumentParser();parser.add_argument('--go',type=Path,required=True);parser.add_argument('--rust-tests',type=Path,required=True);parser.add_argument('--out',type=Path,required=True)
    args=parser.parse_args();raise SystemExit(0 if run(args.go.resolve(),args.rust_tests.resolve(),args.out.resolve()) else 1)
