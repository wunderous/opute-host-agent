#!/usr/bin/env python3
"""Connected activation and mount parity on exclusively owned loopback endpoints."""
import argparse, copy, datetime, gzip, json, os, subprocess, time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from provider_adapter import ROOT, freeze_binary, sha
from provider_adapter import Fixture
import threading
from provider_publication import manifest


def cases():
    rows=[]
    def add(name, steps):
        for mode in [False, True]:
            rows.append({'case':name+'-'+str(mode),'activation':{'standalone':mode,'observeMounts':True,'steps':copy.deepcopy(steps)}})
    def declaration(dependencies=None, chain=False):
        value=manifest(family='opute.capability.test.v1')
        value.update(schema='opute-provider-install-manifest.v1',provides=[{'id':'opute.capability.test.v1','version':1}], validation={'capability':'opute.capability.test.v1','operation':'test.probe'})
        value['services'][0]['version']=1
        if dependencies is not None:value['services'][0]['dependencies']=[{'id':item,'version':1} for item in dependencies]
        if chain:
            service=copy.deepcopy(value['services'][0]);service.update(id='consumer',capabilityId='opute.capability.consumer.v1',dependencies=[{'id':'opute.capability.test.v1','version':1}])
            service['operations'][0]['id']='test.consumer'
            value['services'].append(service);value['provides'].append({'id':'opute.capability.consumer.v1','version':1})
        return value
    def generation(id):
        return {'action':'restore','generation':{'ID':id,'Provider':{'id':'opaque.provider','version':'v1'},'ManifestHash':'hash-'+id,'Endpoint':'fixture://'+id,'State':'candidate','Sessions':0,'CreatedAt':'2026-10-01T00:00:00Z'}}
    def setup(id='g1', value=None):
        return [generation(id),{'action':'candidate','id':id,'manifest':declaration() if value is None else value},
                {'action':'connect','id':id,'descriptor':{'schema':'opute-provider-plugin.v1','pluginId':'opaque.provider','version':'v1','capabilities':[{'id':'opute.capability.test.v1','version':1}],'server':{'transport':'streamable_http','endpoint':'fixture://'+id}}}]
    def activate(id='g1', **meta):return {'action':'activate','metadata':{'activate':True,'providerGenerationId':id,**meta}}
    def probe(id='g1'):return {'action':'probe','id':id,'operation':'test.probe'}
    def cleanup(id='g1'):return {'action':'cleanup','id':id}
    add('connected-activation',setup()+[probe(),activate(),probe(),activate(),probe(),{'action':'unmount','provider':'opaque.provider','id':'g1'},probe(),{'action':'unmount','provider':'opaque.provider','id':'g1'},cleanup(),cleanup()])
    add('connected-chain',setup(value=declaration(chain=True))+[activate(),probe(),{'action':'unmount','provider':'opaque.provider','id':'g1'},probe()])
    for partial in [False,True]:
        broken=declaration(['opute.capability.absent.v1'])
        if partial:
            broken=declaration(chain=True);broken['services'][1]['dependencies']=[{'id':'opute.capability.absent.v1','version':1}]
        add('mount-failure-'+str(partial),setup(value=broken)+[activate(),probe(),cleanup(),probe(),cleanup()])
        add('replacement-mount-failure-'+str(partial),setup()+[activate()]+setup('g2',broken)+[activate('g2'),probe('g1'),probe('g2'),cleanup('g2'),probe('g1'),probe('g2')])
    add('replacement-chain',setup(value=declaration(chain=True))+[activate()]+setup('g2',declaration(chain=True))+[activate('g2'),probe('g1'),probe('g2'),cleanup('g2'),probe('g2')])
    add('reaffirm-duplicate',setup()+[activate(),{'action':'candidate','id':'g1','manifest':declaration()}, {'action':'connect','id':'g1','descriptor':setup()[2]['descriptor']},activate(),probe(),cleanup(),probe()])
    add('denial-cleanup',setup()+[{'action':'admission','denial':'fixture admission denied'},activate(),probe(),cleanup(),probe(),cleanup()])
    add('closed-store-cleanup',setup()+[{'action':'closeState'},activate(),probe(),cleanup(),probe()])
    add('filter-cleanup',setup()+[activate(runtime={'capabilities':['absent']}),probe(),cleanup(),probe()])
    add('completed-before-activation',setup()+[{'action':'complete','id':'g1'},activate(),probe(),cleanup(),probe()])
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
    paths=[Path(__file__),ROOT/'tools/parity/provider_activation.py',ROOT/'tools/parity/provider_activation_parity.py',ROOT/'tools/parity/provider_publication.py',
           ROOT/'tools/parity/provider_adapter.py',ROOT/'tools/parity/build-provider-activation-oracle.py',ROOT/'baseline/source-lock.json',ROOT/'Cargo.lock',ROOT/'Cargo.toml',ROOT/'crates/host-agent/Cargo.toml']
    paths+=sorted((ROOT/'tools/parity/provider-activation-oracle').iterdir())+sorted((ROOT/'crates/host-agent/src').rglob('*.rs'))+sorted((ROOT/'crates/host-agent/src/catalog/schema').glob('*.json'))
    provenance={str(p.relative_to(ROOT)):sha(p) for p in paths}
    def pinned():
        git=lambda *args:subprocess.check_output(['git','-C',str(ROOT/'.parity/go-src'),*args],text=True).strip()
        assert git('rev-parse','HEAD')==lock['sourceCommit'] and git('rev-parse','HEAD^{tree}')==lock['sourceTree'] and not git('status','--porcelain')
    pinned();source=cases()
    def batch_run(job):
        iteration,batch,start=job;chunk=copy.deepcopy(source[start:start+8]);folder=out/str(iteration)/str(batch);folder.mkdir(parents=True,exist_ok=True)
        fixture=Fixture({'discover':{'supportedVersions':['2026-07-28']},'listed':{'tools':[{'name':'opute.provider.get_install_manifest'},{'name':'test.probe'},{'name':'test.consumer'}]},'callResult':{'content':[],'structuredContent':{'ok':True}}})
        thread=threading.Thread(target=fixture.serve_forever,daemon=True);thread.start()
        endpoint='http://127.0.0.1:'+str(fixture.server_port)
        chunk=json.loads(json.dumps(chunk).replace('fixture://g1',endpoint+'/g1/mcp').replace('fixture://g2',endpoint+'/g2/mcp'))
        inp=folder/'input.json';target=folder/'rust.json';inp.write_text(json.dumps(chunk)+'\n')
        clock={'goStarted':time.time()};left=json.loads(subprocess.check_output([str(go)],input=json.dumps(chunk),text=True,timeout=120));clock['goFinished']=time.time();go_requests=list(fixture.records);fixture.records.clear()
        clock['rustStarted']=time.time();subprocess.run([str(rust),'provider::activation::tests::activation_fixture','--exact','--ignored'],env={**os.environ,'PARITY_PROVIDER_ACTIVATION_INPUT':str(inp.resolve()),'PARITY_PROVIDER_ACTIVATION_OUTPUT':str(target.resolve())},capture_output=True,check=True,timeout=120);clock['rustFinished']=time.time();right=json.loads(target.read_text());rust_requests=list(fixture.records);fixture.shutdown();fixture.server_close();thread.join(timeout=2);assert not thread.is_alive()
        def requests(records):
            return [{'path':r['path'],'payload':json.loads(r['body']),'headers':{k:v for k,v in r['headers'].items() if k in ['mcp-protocol-version','mcp-client-name','mcp-client-version','authorization']}} for r in records]
        assert requests(go_requests)==requests(rust_requests), 'connected adapter request mismatch'
        for records in [go_requests,rust_requests]:
            assert any(json.loads(r['body'])['method']=='server/discover' for r in records)
            assert any(json.loads(r['body'])['method']=='tools/call' for r in records)
        (folder/'requests.json').write_text(json.dumps({'go':go_requests,'rust':rust_requests},indent=2)+'\n')
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
    summary={'scope':'Actual connected loopback adapters through owning activation, mount rollback, replacement, calls and cleanup. Public native lifecycle wiring, provider executables, supervised restart and full M7-M13 remain open.',
             'sourceLock':lock,'provenance':provenance,'goBinarySha256':sha(go),'rustTestBinarySha256':sha(rust),
             'normalization':'only fresh event/activation timestamps after real-clock bounds and active/durable consistency checks; all other parsed JSON values exact',
             'rows':rows,'passed':sum(row['pass'] for row in rows),'total':len(rows)}
    (out/'summary.json').write_text(json.dumps(summary,indent=2)+'\n');(out/'index.json').write_text(json.dumps({str(p.relative_to(out)):sha(p) for p in out.rglob('*') if p.is_file() and p.name!='index.json'},indent=2)+'\n')
    print('Provider activation:',summary['passed'],'/',summary['total']);return summary['passed']==summary['total']


if __name__=='__main__':
    parser=argparse.ArgumentParser();parser.add_argument('--go',type=Path,required=True);parser.add_argument('--rust-tests',type=Path,required=True);parser.add_argument('--out',type=Path,required=True)
    args=parser.parse_args();raise SystemExit(0 if run(args.go.resolve(),args.rust_tests.resolve(),args.out.resolve()) else 1)
