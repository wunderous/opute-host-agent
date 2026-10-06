#!/usr/bin/env python3
"""Pinned Go SDK content projection comparison; typed malformed fields and canonical client-map decoding; input-request variants remain open."""
import argparse
import copy
import json
import os
from pathlib import Path
import subprocess
from provider_adapter import ROOT,sha,freeze_binary

def cases():
    rows=[{}, {'unknown':'ignored'}, {'content':None},{'content':[]},{'content':[None]},{'content':[{}]},{'content':[{'type':'unknown'}]}]
    for kind in ['text','image','audio','resource_link','resource','tool_use','tool_result']:
        for content in [{'type':kind},{'type':kind,'text':'fixture','mimeType':'application/octet-stream','data':'AAEC','uri':'opaque:uri','name':'fixture','id':'use','toolUseId':'use','title':'title','description':'description','input':{'x':1},'resource':{'uri':'opaque:uri','text':'fixture','blob':'AAEC'},'size':0,'icons':[{'src':'icon','unknown':'ignored'}],'_meta':{'x':1},'annotations':{'audience':['user'],'priority':0.5,'lastModified':'opaque-time','unknown':'ignored'},'unknown':'ignored'}]:rows.append({'content':[content]})
        rows.append({'content':[{'type':kind,'_meta':{},'annotations':{},'text':'','data':'','resource':{},'input':{},'content':[]}]})
    for value in [False,0,'',[],{},['value'],'primitive',None]:rows.append({'structuredContent':value,'isError':False,'_meta':{},'requestState':'','resultType':''})
    for result_type in ['complete','input_required','unknown']:rows.append({'resultType':result_type,'requestState':'opaque-state','isError':True,'_meta':{'fixture':True}})
    for nested in [[{'type':'text'}],[{'type':'image'}],[{'type':'resource'}],[{'type':'text','text':'fixture'}],[{'type':'tool_use'}],[{'type':'unknown'}],[None]]:rows.append({'content':[{'type':'tool_result','content':nested,'structuredContent':False}]})
    for binary in ['AB==','AA==','AAA=','AAEC','AA\nEC','AA\r\nEC','',None,'%%%','A','AA','AAA','AA=','AA===','A===','AA=A','AAAA=','AA=\n','AA=\n=','AA=\nA','AA==\n!','A\n','\r\n','/w==','_w==','é','AB\r\n==']:
        rows.append({'content':[{'type':'image','data':binary}]})
    for blob in ['',None,'AA==']:rows.append({'content':[{'type':'resource','resource':{'uri':'opaque','blob':blob}}]})
    for value in [9007199254740993,9223372036854775807,1.25,1e21]:
        rows.append({'structuredContent':{'number':value,'nested':[value]},'_meta':{'number':value},'content':[{'type':'tool_use','input':{'number':value}},{'type':'text','annotations':{'priority':value},'_meta':{'number':value}}]})
    bad_values=['bad',True,42,1.5,[],{},None]
    for field in ['_meta','content','isError','requestState','resultType']:
        for value in bad_values:rows.append({field:value})
    for field in ['type','text','mimeType','data','uri','name','title','description','size','_meta','annotations','icons','id','input','toolUseId','content','isError']:
        for value in bad_values:rows.append({'content':[{'type':'text',field:value}]})
    for field in ['audience','lastModified','priority']:
        for value in bad_values:rows.append({'content':[{'type':'text','annotations':{field:value}}]})
    for field in ['uri','mimeType','text','blob','_meta']:
        for value in bad_values:rows.append({'content':[{'type':'resource','resource':{field:value}}]})
    for field in ['src','mimeType','sizes','theme']:
        for value in bad_values:rows.append({'content':[{'type':'resource_link','icons':[{field:value}]}]})
    for field,value in [('audience',[42]),('audience',[None]),('audience',['user',False])]:rows.append({'content':[{'type':'text','annotations':{field:value}}]})
    for value in [[42],[None],[0,255],[-1],[256],[1.5],['bad']]:rows.append({'content':[{'type':'image','data':value}]})
    rows.extend([{'CONTENT':[{'type':'unknown'}]},{'content':[{'type':'text','TEXT':'ignored'}]},{'content':[{'type':'text','data':'%%%'}]},{'content':[{'type':'text','data':'%%%','text':42}]}])
    for value in [9223372036854775807,9223372036854775808,-9223372036854775808,-9223372036854775809,18446744073709551616,1e20,1e-9,-1.5]:
        rows.append({'content':[{'type':'resource_link','size':value}]})
        rows.append({'content':[{'type':'image','data':[value]}]})
    for sizes in [[None],['16x16',None],[False],[1.5]]:rows.append({'content':[{'type':'resource_link','icons':[{'sizes':sizes}]}]})
    for data in ['AA\r\n==!','AA==','AA=\nA','%%%']:
        rows.append({'content':[{'type':'text','data':data,'text':False}]})
        rows.append({'content':[{'type':'resource','resource':{'blob':data,'uri':False}}]})
    return rows

def run(go,rust,out):
    go=freeze_binary(go,'go');rust=freeze_binary(rust,'rust');out.mkdir(parents=True,exist_ok=True)
    lock=json.loads((ROOT/'baseline/source-lock.json').read_text())
    def git(*args):return subprocess.check_output(['git','-C',str(ROOT/'.parity/go-src'),*args],text=True).strip()
    assert git('rev-parse','HEAD')==lock['sourceCommit'] and git('rev-parse','HEAD^{tree}')==lock['sourceTree'] and not git('status','--porcelain')
    sources=[Path(__file__),ROOT/'crates/host-agent/src/provider/result.rs',ROOT/'tools/parity/provider-core-oracle/main.go',ROOT/'tools/parity/provider-core-oracle/go.mod',ROOT/'tools/parity/provider-core-oracle/go.sum',ROOT/'tools/parity/provider_adapter.py',ROOT/'Cargo.lock',ROOT/'crates/host-agent/src/mcpsdk.rs',ROOT/'crates/host-agent/src/gojson.rs']
    before={str(p.relative_to(ROOT)):sha(p) for p in sources};rows=[];payload=cases()
    for iteration in range(5):
        folder=out/str(iteration);folder.mkdir(parents=True,exist_ok=True)
        input_path=folder/'input.json';input_path.write_text(json.dumps(payload,indent=2)+'\n')
        reference=json.loads(subprocess.check_output([str(go)],input=json.dumps([{'kind':'result','value':v} for v in payload]),text=True))
        (folder/'go.json').write_text(json.dumps(reference,indent=2)+'\n')
        target=folder/'rust.json';env={**os.environ,'PARITY_PROVIDER_RESULT_INPUT':str(input_path.resolve()),'PARITY_PROVIDER_RESULT_OUTPUT':str(target.resolve())}
        subprocess.run([str(rust),'provider::result::tests::result_fixture','--exact','--ignored'],env=env,check=True,capture_output=True)
        actual=json.loads(target.read_text())
        for i,(a,b) in enumerate(zip(reference,actual,strict=True)):
            passed=a==b;rows.append({'case':i,'iteration':iteration,'pass':passed})
            if not passed:print('FAIL',i,a,b)
    assert before=={str(p.relative_to(ROOT)):sha(p) for p in sources}
    report={'scope':'T0 SDK content/default/omission projection, typed malformed fields, exact case-sensitive decoding, integer/base64 errors and client-map float64 conversion; input requests and adapter promotion remain open','sourceLock':lock,'provenance':before,'goBinarySha256':sha(go),'rustTestBinarySha256':sha(rust),'rows':rows,'total':len(rows),'passed':sum(r['pass'] for r in rows),'normalization':'none; structured projections compared exactly'}
    (out/'summary.json').write_text(json.dumps(report,indent=2)+'\n');(out/'index.json').write_text(json.dumps({str(p.relative_to(out)):sha(p) for p in out.rglob('*.json') if p.name!='index.json'},indent=2)+'\n')
    print('Provider SDK projection:',report['passed'],'/',report['total']);return all(r['pass'] for r in rows)
if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--go',type=Path,required=True);p.add_argument('--rust-tests',type=Path,required=True);p.add_argument('--out',type=Path,required=True)
    a=p.parse_args();raise SystemExit(0 if run(a.go.resolve(),a.rust_tests.resolve(),a.out.resolve()) else 1)
