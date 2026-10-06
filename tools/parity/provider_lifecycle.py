"""Differential provider generation core oracle; not an M7 wire/E2E gate."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess

ROOT=Path(__file__).resolve().parents[2]
PROVIDER={'id':'com.opute.example','version':'1.0.0'}
ID=PROVIDER['id']

def create():
    return {'action':'create','provider':PROVIDER,'hash':'sha256:manifest','endpoint':'http://127.0.0.1:4318/mcp','revision':'catalog-1'}

def step(action,identifier=1,**args):
    return {'action':action,'id':ID+'-'+str(identifier),**args}

def active():
    return [create(),step('ready'),step('activate')]

def generation(identifier,state):
    return {'ID':identifier,'Provider':PROVIDER,'ManifestHash':'sha256:restored','Endpoint':'http://127.0.0.1:4318/mcp',
            'CatalogRevision':'restored','State':state,'CreatedAt':'2026-09-28T01:02:03Z','ActiveAt':'0001-01-01T00:00:00Z','Sessions':0}

def cases():
    rows={}
    rows['replacement-affinity']=active()+[
        step('open',providerId=ID,session='old'),create(),step('ready',2),step('activate',2),step('get'),
        step('open',providerId=ID,session='new'),step('dispose'),step('drain'),
        step('close',session='old'),step('close',session='old'),step('drain'),
        step('active',providerId=ID),step('get',2),step('close',session='new'),step('dispose',2),step('dispose',2)]
    rows['rollback']=active()+[create(),step('ready',2),step('activate',2),step('open',providerId=ID,session='new'),
        step('rollback',2,previous=ID+'-1'),step('close',session='new'),step('rollback',2,previous='foreign-1'),
        step('rollback',2,previous=ID+'-1'),step('active',providerId=ID),step('get',2)]
    rows['rollback-first']=active()+[step('rollback'),step('active',providerId=ID),step('get')]
    for state in ['candidate','ready','active','draining','failed','stopped','','unknown']:
        rows['restore-'+(state or 'empty')]=[{'action':'restore','generation':generation(ID+'-41',state)},step('get',41),
            step('open-explicit',41,providerId=ID,session='held'),step('close',session='held'),create(),step('get',42)]
    rows['restore-conflict']=[{'action':'restore','generation':generation(ID+'-1','active')},
        {'action':'restore','generation':generation(ID+'-2','active')},step('active',providerId=ID),step('get',2),create(),
        {'action':'restore','generation':generation(ID+'-2','failed')},step('get',2)]
    for suffix in ['+42','042','18446744073709551615','18446744073709551616','-2','abc','']:
        rows['sequence-'+suffix]=[{'action':'restore','generation':generation(ID+'-'+suffix,'candidate')},create()]
    for field,value in [('provider',{'id':'','version':'1'}),('provider',{'id':'p','version':''}),
                        ('hash',''),('endpoint',''),('revision',' '),('hash',' '),('endpoint',' ')]:
        candidate=create();candidate[field]=value;rows['create-'+field+'-'+str(len(rows))]=[candidate]
    rows['refresh']=active()+[step('refresh',hash='sha256:new',revision='catalog-2'),step('get'),step('refresh',hash=' ',revision='catalog-3')]
    rows['failed-candidate']=[create(),step('fail'),step('ready'),step('activate'),step('open-explicit',providerId=ID,session='held'),step('refresh',hash='new',revision='new')]
    rows['candidate-session']=[create(),step('open-explicit',providerId=ID,session='held'),step('get'),step('ready'),step('ready'),step('activate'),step('close',session='held'),step('get')]
    rows['cancelled-drain']=active()+[step('open',providerId=ID,session='held'),step('drain',cancelled=True),step('close',session='held'),step('drain',cancelled=True)]
    rows['forced-drain-still-owned']=active()+[step('open',providerId=ID,session='held'),step('drain'),step('close',session='held'),step('drain')]
    for action in ['ready','activate','rollback','get','dispose','fail','drain','refresh']:
        rows['missing-'+action]=[{'action':action,'id':'missing"\n','hash':'hash','revision':'revision'}]
    rows['session-errors']=[step('open',providerId=ID),{'action':'open-explicit','id':' ','providerId':ID,'session':'one'},step('open-explicit',providerId='other',session='two')]
    for field in ['ID','ManifestHash','Endpoint']:
        candidate=generation(ID+'-1','active');candidate[field]=''
        rows['restore-missing-'+field]=[{'action':'restore','generation':candidate}]
    for field in ['id','version']:
        candidate=generation(ID+'-1','active');candidate['Provider']={**PROVIDER,field:''}
        rows['restore-missing-provider-'+field]=[{'action':'restore','generation':candidate}]
    for count in [-1, 9223372036854775807]:
        candidate=generation(ID+'-1','active');candidate['Sessions']=count
        rows['restored-session-count-'+str(count)]=[{'action':'restore','generation':candidate},
            step('open',providerId=ID,session='held'),step('get'),step('close',session='held'),step('get'),step('dispose')]
    return rows


def canonical(value):
    if isinstance(value,list):return [canonical(v) for v in value]
    if isinstance(value,dict):
        result={k:canonical(v) for k,v in value.items()}
        if {'ID','Provider','State'}<=value.keys():
            for field in ['CreatedAt','ActiveAt']:
                timestamp=value[field]
                if timestamp not in ['0001-01-01T00:00:00Z','2026-09-28T01:02:03Z']:
                    assert re.fullmatch(r'\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(?:\.\d{1,9})?Z',timestamp),timestamp
                    result[field]='${WALL_CLOCK}'
        return result
    return value


def verify_timestamp_affinity(steps, results):
    observed = {}
    def inspect(value, step):
        if isinstance(value, list):
            for item in value: inspect(item, step)
        elif isinstance(value, dict):
            if {'ID','Provider','State'} <= value.keys():
                identity = value['ID']
                previous = observed.get(identity)
                if previous:
                    assert value['CreatedAt'] == previous['CreatedAt'], ('creation changed', identity)
                    if not (step['action'] == 'activate' and step.get('id') == identity):
                        assert value['ActiveAt'] == previous['ActiveAt'], ('activation changed', identity)
                observed[identity] = value
            else:
                for child in value.values(): inspect(child, step)
    for step, result in zip(steps, results, strict=True):
        inspect(result, step)


def sha(path):return hashlib.file_digest(path.open('rb'),'sha256').hexdigest()


def run(go_binary,rust_binary,out):
    rows=[];out.mkdir(parents=True,exist_ok=True)
    for name,steps in cases().items():
        safe=re.sub(r'[^a-zA-Z0-9_.-]','_',name)
        for iteration in range(5):
            folder=out/safe/str(iteration);folder.mkdir(parents=True,exist_ok=True)
            payload={'timeoutMs':20,'cancelOnTimeout':name=='forced-drain-still-owned','steps':steps}
            source=folder/'input.json';source.write_text(json.dumps(payload))
            go_result=subprocess.run([str(go_binary)],input=json.dumps(payload),text=True,capture_output=True,check=True)
            reference=json.loads(go_result.stdout);(folder/'go.json').write_text(json.dumps(reference,indent=2)+'\n')
            rust_output=folder/'rust.json';env={**os.environ,'PARITY_PROVIDER_INPUT':str(source.resolve()),'PARITY_PROVIDER_OUTPUT':str(rust_output.resolve())}
            subprocess.run([str(rust_binary),'provider::tests::lifecycle_fixture','--exact','--ignored'],env=env,text=True,capture_output=True,check=True)
            actual=json.loads(rust_output.read_text())
            verify_timestamp_affinity(steps,reference);verify_timestamp_affinity(steps,actual)
            passed=canonical(reference)==canonical(actual)
            rows.append({'case':name,'iteration':iteration,'pass':passed})
            if not passed:print('FAIL',name,iteration)
    report={'scope':'T0 differential generation core oracle; no M7 MCP/provider gate claim','rows':rows,'passed':sum(r['pass'] for r in rows),'total':len(rows),
        'sourceLock':json.loads((ROOT/'baseline/source-lock.json').read_text()),'goBinarySha256':sha(go_binary),'rustTestBinarySha256':sha(rust_binary),
        'rustSourceSha256':sha(ROOT/'crates/host-agent/src/provider.rs'),'driverSha256':sha(Path(__file__)),
        'normalization':'Only RFC3339 wall-clock generation timestamps; restored explicit timestamps retained'}
    (out/'summary.json').write_text(json.dumps(report,indent=2)+'\n')
    index={str(p.relative_to(out)):sha(p) for p in out.rglob('*.json') if p.name != 'index.json'};(out/'index.json').write_text(json.dumps(index,indent=2)+'\n')
    print('Generation core oracle:',report['passed'],'/',report['total']);return all(r['pass'] for r in rows)

if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--go',required=True,type=Path);parser.add_argument('--rust-tests',required=True,type=Path);parser.add_argument('--out',required=True,type=Path)
    args=parser.parse_args();raise SystemExit(0 if run(args.go.resolve(),args.rust_tests.resolve(),args.out.resolve()) else 1)
