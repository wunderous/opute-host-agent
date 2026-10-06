#!/usr/bin/env python3
"""Compare typed provider contract validators with the pinned Go implementation."""
import argparse
import copy
import hashlib
import json
import os
from pathlib import Path
import subprocess

ROOT=Path(__file__).resolve().parents[2]

def cases():
    rows=[]
    descriptor={'schema':'opute-provider-plugin.v1','pluginId':'test.provider','version':'v1','capabilities':[{'id':'test.capability','version':1}],'server':{'transport':'streamable_http','endpoint':'http://localhost:1234/mcp'}}
    operation={'id':'test.operation','version':1,'inputSchema':{'type':'object','properties':{'size':{'type':'integer'},'nested':{'properties':{'size':{'type':'number'}}}}},'outputSchema':{},'effect':'read'}
    manifest={'schema':'opute-provider-install-manifest.v1','provider':{'id':'test.provider','version':'v1'},'provides':[{'id':'test.capability','version':1}],'validation':{'capability':'test.capability','operation':'test.operation'},'services':[{'id':'test.service','capabilityId':'test.capability','version':1,'operations':[operation]}]}
    def add(name,value,kind='manifest'):
        rows.append({'name':name,'kind':kind,'value':value,'expected':manifest['provider']})
    def mutate(name,path,value,base=manifest,kind='manifest'):
        obj=copy.deepcopy(base);node=obj
        for key in path[:-1]:node=node[key]
        node[path[-1]]=value;add(name,obj,kind)
    add('minimal-valid',manifest)
    add('descriptor-valid',descriptor,'descriptor')
    for key,values in {'schema':['','unknown'],'pluginId':['ab','abc','Aaa',' test.provider ','a_b','a'*128,'a'*129],'version':['',' ',' arbitrary '],'capabilities':[[],[{'id':'ab','version':1}],[{'id':'test.capability','version':0}],[{'id':'test.capability','version':1}]*2,[{'id':' test.capability ','version':1},{'id':'test.capability','version':1}]],'requires':[[{'id':'','version':-1}]]}.items():
        for i,value in enumerate(values):mutate(f'descriptor-{key}-{i}',[key],value,descriptor,'descriptor')
    mutate('transport',['server','transport'],'stdio',descriptor,'descriptor')
    endpoints=['','localhost','http:localhost','ftp://localhost','HTTP://LOCALHOST/path','http:///x','http://user:pass@host','http://@host','http://host:abc','http://host:','http://[::1]','http://[bad]','http://[::1','http://host/path%zz','http://host/?q=%zz','http://host/#%zz','http://%41.com','http://%25.com','http://host/path with space','http://ho st/','http://host\n/','http://host:999999999','http://host\\path','http://host/über',' https://host/path ','http://user%zz@host','http://host/%00','http://host:80:90','http://[fe80::1%25eth0]:80','http://host/[%','http://host/?x=%','http://host<name>','http://host\"name','http://[127.0.0.1]','http://prefix[::1]','http://[::1%25]','http://[::1%25a%20b]','http://[::1%25a%0ab]','http://%C3%A9.com','http://user<name>@host','http://user@name@host','http://host/_[]','http://host/|^`{}','http://host~name']
    for i,value in enumerate(endpoints):mutate(f'endpoint-{i}', ['server','endpoint'],value,descriptor,'descriptor')
    for key,values in {'schema':['','unknown'],'provider':[{'id':' test.provider ','version':'v1'},{'id':'test.provider','version':'V1'}],'provides':[[],[{'id':'ab','version':1}],[{'id':'test.capability','version':1}]*2],'requires':[[{'id':'','version':-1}]],'services':[[]]}.items():
        for i,value in enumerate(values):mutate(f'manifest-{key}-{i}',[key],value)
    for key in ['capability','operation']:mutate('validation-'+key,['validation',key],' ')
    recipe={'id':'setup','source':{'uri':'relative/recipe.yaml','revision':'revision','sha256':'not-hex'}}
    mutate('recipe-pinned',['recipes'],[recipe])
    for path in [['id'],['source','uri'],['source','revision'],['source','sha256']]:
        r=copy.deepcopy(recipe);node=r
        for k in path[:-1]:node=node[k]
        node[path[-1]]=' ';mutate('recipe-'+'.'.join(path),['recipes'],[r])
    for key,value in [('id','ab'),('version',0),('capabilityId',''),('dependencies',[{'id':'ab','version':1}]),('dependencies',[{'id':'test.capability','version':1}]),('dependencies',[{'id':'other.capability','version':1}]),('operations',[])]:
        mutate('service-'+key+'-'+str(value),['services',0,key],value)
    mutate('duplicate-service',['services'],manifest['services']*2)
    other=copy.deepcopy(manifest['services'][0]);other['id']='other.service'
    mutate('duplicate-operation-cross-service',['services'],[manifest['services'][0],other])
    for key,values in {'id':['ab','test_operation','test operation',' test.operation '],'version':[0,-1],'inputSchema':[None,{}],'outputSchema':[None,{}],'effect':['','mutation','destructive','credential_bearing','READ'],'taskSupport':['','sync_only','bridged','task',' bridged '],'outputType':['missing']}.items():
        for i,value in enumerate(values):mutate(f'operation-{key}-{i}', ['services',0,'operations',0,key],value)
    result_type={'id':'result.anything','version':1,'selectors':[{'id':'chosen','sourcePath':'$.items[*]','cardinality':'one'}]}
    def output_case(name,types,output='result.anything',produces=None):
        obj=copy.deepcopy(manifest);o=obj['services'][0]['operations'][0];o['resultTypes']=types;o['outputType']=output
        if produces is not None:o['produces']=produces
        add(name,obj)
    output_case('selectors-valid',[result_type])
    output_case('selectors-missing-output',[result_type],'')
    output_case('selectors-wrong-output',[result_type],'missing')
    output_case('selectors-duplicate-type',[result_type,result_type])
    for key,value in [('id',''),('version',0)]:
        t=copy.deepcopy(result_type);t[key]=value;output_case('result-type-'+key,[t])
    for key,values in {'id':['','chosen'],'sourcePath':['',' '],'cardinality':['',' ','one','many',' many ','unknown']}.items():
        for i,value in enumerate(values):
            t=copy.deepcopy(result_type);t['selectors'][0][key]=value;output_case(f'selector-{key}-{i}',[t])
    t=copy.deepcopy(result_type);t['selectors']*=2;output_case('duplicate-selector',[t])
    output_case('produced-known-selector',[result_type],produces=[{'resourceType':'container','sourcePath':'anything','selectorId':'chosen'}])
    output_case('produced-unknown-selector',[result_type],produces=[{'resourceType':'container','sourcePath':'anything','selectorId':'wrong'}])
    for field,values in {'requires':[{'resourceType':''},{'resourceType':'container'},{'resourceType':'container','argument':'id'},{'resourceType':'container','sourcePath':'x'},{'resourceType':'container','argument':'id','selectorId':'chosen'}],'produces':[{'resourceType':''},{'resourceType':'container','argument':'id'},{'resourceType':'container','sourcePath':'x'},{'resourceType':'container','sourcePath':'x','selectorId':'chosen'}]}.items():
        for i,value in enumerate(values):mutate(f'binding-{field}-{i}',['services',0,'operations',0,field],[value])
    for key,values in {'cpuCores':[-1,0,0.5],'memoryBytes':[-1,0,1],'diskBytes':[-1,0,1],'tasks':[-1,0,1],'class':['','control','normal','heavy',' heavy ','HIGH']}.items():
        for i,value in enumerate(values):mutate(f'cost-{key}-{i}',['services',0,'operations',0,'resourceCost'],{key:value})
    for dimension in ['cpuCores','memoryBytes','diskBytes','tasks']:
        for path in ['size','nested.size','missing','.size','size.','size..x','size[0]','nested/size',' size ']:
            for fallback in [0,1]:
                mutate(f'cost-binding-{dimension}-{path}-{fallback}',['services',0,'operations',0,'resourceCost'],{dimension:fallback,'argumentBindings':{dimension:path}})
        for kind in ['string','boolean','array','object',['integer'],None]:
            obj=copy.deepcopy(manifest);o=obj['services'][0]['operations'][0];o['inputSchema']['properties']['size']['type']=kind;o['resourceCost']={dimension:1,'argumentBindings':{dimension:'size'}};add(f'cost-type-{dimension}-{kind}',obj)
    teardown=copy.deepcopy(operation);teardown['id']='opute.provider.teardown';teardown['effect']='destructive'
    mutate('teardown-valid',['teardown'],teardown)
    for key,value in [('id','test.teardown'),('effect','read'),('version',0),('inputSchema',None)]:
        t=copy.deepcopy(teardown);t[key]=value;mutate('teardown-'+key,['teardown'],t)
    return rows

def wire_cases():
    rows=[]
    originals=cases()
    for row in originals:
        rows.append({**row,'kind':'wire-'+row['kind'],'value':json.dumps(row['value'])})
    descriptor=originals[1]['value'];manifest=originals[0]['value']
    def add(name,kind,value):rows.append({'name':name,'kind':'wire-'+kind,'value':value,'expected':manifest['provider']})
    for kind,base in [('descriptor',descriptor),('manifest',manifest)]:
        for name,value in [('null','null'),('empty','{}'),('array','[]'),('string','"text"'),('number','123'),('boolean','true')]:add(kind+'-root-'+name,kind,value)
        def paths(value,path=()):
            if isinstance(value,dict):
                for key,child in value.items():
                    yield path+(key,)
                    if key not in ['inputSchema','outputSchema','inputs']:yield from paths(child,path+(key,))
            elif isinstance(value,list):
                for i,child in enumerate(value):yield from paths(child,path+(i,))
        for path in paths(base):
            for label,value in [('null',None),('string','text'),('bool',True),('integer',42),('float',1.5),('array',[]),('object',{})]:
                obj=copy.deepcopy(base);node=obj
                for key in path[:-1]:node=node[key]
                node[path[-1]]=value;add(kind+'-'+'.'.join(map(str,path))+'-'+label,kind,json.dumps(obj))
        def upper(value,in_map=False):
            if isinstance(value,dict):return {(k if in_map else k.upper()):upper(v,in_map or k in ['inputSchema','outputSchema','inputs']) for k,v in value.items()}
            if isinstance(value,list):return [upper(v,in_map) for v in value]
            return value
        add(kind+'-uppercase',kind,json.dumps(upper(base)))
    for name,raw in [('scalar-null-keeps','{"schema":"first","SCHEMA":null}'),('scalar-last-wins','{"schema":"first","SCHEMA":"second"}'),('server-merge','{"server":{"endpoint":"http://first"},"SERVER":{"transport":"streamable_http"}}'),('server-null-keeps','{"server":{"endpoint":"http://first"},"server":null}'),('slice-replaced-null','{"capabilities":[{"id":"valid","version":1}],"capabilities":null}'),('slice-null-elements','{"capabilities":[null],"server":{"args":[null]}}'),('slice-slot-reuse','{"capabilities":[{"id":"retained","version":1}],"capabilities":[{"version":2}]}'),('slice-string-reuse','{"server":{"args":["retained"]},"server":{"args":[null]}}'),('slice-hidden-capacity','{"capabilities":[{"id":"first"},{"id":"second"}],"capabilities":[{"version":2}],"capabilities":[null,null]}'),('slice-empty-clears-capacity','{"capabilities":[{"id":"first"}],"capabilities":[],"capabilities":[null]}'),('unknown-ignored','{"unknown":{"schema":4}}'),('invalid-first-not-overwritten','{"schema":42,"schema":"valid"}')]:add(name,'descriptor',raw)
    for name,raw in [('recipe-empty-inputs','{"recipes":[{"id":"setup","inputs":{}}]}'),('recipe-null-inputs','{"recipes":[{"id":"setup","inputs":null}]}'),('recipe-number-inputs','{"recipes":[{"id":"setup","inputs":{"number":9007199254740993}}]}'),('html-unicode-encoding','{"recipes":[{"id":"<&>\\u2028\\u2029","inputs":{"html":"<&>"}}]}'),('pointer-null-clears','{"teardown":{"id":"first"},"teardown":null}'),('pointer-merge','{"teardown":{"id":"first"},"teardown":{"version":2}}'),('map-merges','{"teardown":{"inputSchema":{"one":1}},"teardown":{"inputSchema":{"two":2}}}'),('map-null-clears','{"teardown":{"inputSchema":{"one":1}},"teardown":{"inputSchema":null}}'),('large-generic-number','{"teardown":{"inputSchema":{"value":9007199254740993}}}'),('version-number-exponent','{"services":[{"version":1e0}]}'),('version-overflow','{"services":[{"version":9223372036854775808}]}'),('cost-overflow','{"teardown":{"resourceCost":{"cpuCores":1e400}}}'),('generic-number-overflow','{"teardown":{"inputSchema":{"value":1e400}}}'),('int64-overflow','{"teardown":{"resourceCost":{"tasks":9223372036854775808}}}'),('negative-zero','{"teardown":{"resourceCost":{"cpuCores":-0.0}}}'),('cardinality-type','{"teardown":{"resultTypes":[{"selectors":[{"cardinality":42}]}]}}')]:add(name,'manifest',raw)
    return rows

def sha(path):return hashlib.file_digest(path.open('rb'),'sha256').hexdigest()
def run(go,rust,out,wire=False):
    out.mkdir(parents=True,exist_ok=True)
    lock=json.loads((ROOT/'baseline/source-lock.json').read_text())
    checkout=ROOT/'.parity/go-src'
    def git(*args):return subprocess.check_output(['git','-C',str(checkout),*args],text=True).strip()
    assert git('rev-parse','HEAD')==lock['sourceCommit']
    assert git('rev-parse','HEAD^{tree}')==lock['sourceTree']
    assert not git('status','--porcelain')
    payload=wire_cases() if wire else cases(); rows=[]
    for iteration in range(5):
        folder=out/str(iteration);folder.mkdir(parents=True,exist_ok=True)
        input_path=folder/'input.json';input_path.write_text(json.dumps(payload,indent=2)+'\n')
        reference=json.loads(subprocess.check_output([str(go)],input=json.dumps(payload),text=True))
        (folder/'go.json').write_text(json.dumps(reference,indent=2)+'\n')
        target=folder/'rust.json'
        env={**os.environ,'PARITY_PROVIDER_DEFINITION_INPUT':str(input_path.resolve()),'PARITY_PROVIDER_DEFINITION_OUTPUT':str(target.resolve())}
        subprocess.run([str(rust),'provider::definition::tests::definition_fixture','--exact','--ignored'],env=env,capture_output=True,check=True)
        actual=json.loads(target.read_text());assert len(reference)==len(actual)==len(payload)
        for case,a,b in zip(payload,reference,actual,strict=True):
            passed=a==b;rows.append({'case':case['name'],'iteration':iteration,'pass':passed})
            if not passed:print('FAIL',case['name'],a,b)
    report={'scope':('T0 typed decoding/re-encoding of syntactically valid provider JSON; syntax errors and M7 E2E remain open' if wire else 'T0 typed descriptor/manifest validation only; M7 E2E remains open'),'total':len(rows),'passed':sum(r['pass'] for r in rows),'rows':rows,'sourceLock':lock,'goBinarySha256':sha(go),'rustTestBinarySha256':sha(rust),'driverSha256':sha(Path(__file__)),'rustSourceSha256':sha(ROOT/'crates/host-agent/src/provider/definition.rs'),'goOracleSourceSha256':sha(ROOT/'tools/parity/provider-core-oracle/main.go'),'normalization':'none'}
    (out/'summary.json').write_text(json.dumps(report,indent=2)+'\n')
    index={str(p.relative_to(out)):sha(p) for p in out.rglob('*.json') if p.name!='index.json'}
    (out/'index.json').write_text(json.dumps(index,indent=2)+'\n')
    print('Provider definition oracle:',report['passed'],'/',report['total'])
    return all(r['pass'] for r in rows)
if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--wire',action='store_true');p.add_argument('--go',type=Path,required=True);p.add_argument('--rust-tests',type=Path,required=True);p.add_argument('--out',type=Path,required=True)
    args=p.parse_args();raise SystemExit(0 if run(args.go.resolve(),args.rust_tests.resolve(),args.out.resolve(),args.wire) else 1)
