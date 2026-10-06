#!/usr/bin/env python3
"""Pinned Go SDK content projection comparison; typed malformed fields and canonical client-map decoding; input requests and pointer-marshaler canary."""
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
    def request(method,params):return {'inputRequests':{'request-id':{'method':method,'params':params}}}
    for value in [None,{},False,42,'bad',[],[{}]]:rows.append({'inputRequests':value})
    for entry in [{},False,42,'bad',[],{'method':False,'params':{}},{'method':'unsupported','params':{}},{'method':'roots/list'},{'method':'ROOTS/LIST','params':{}}]:rows.append({'inputRequests':{'request-id':entry}})
    for method in ['elicitation/create','roots/list','sampling/createMessage']:
        for params in [None,{},False,42,'bad',[],{'unknown':'ignored'},{'_meta':{}},{'_meta':{'opaque':9007199254740993}}]:rows.append(request(method,params))
    for params in [{'message':'confirm'},{'mode':'custom','message':'confirm'},{'url':'https://fixture.invalid'},{'elicitationId':'opaque-id'},{'requestedSchema':{'type':'object'}},{'MODE':'url','MESSAGE':'confirm','URL':'opaque'},{'message':None},{'requestedSchema':False}]:rows.append(request('elicitation/create',params))
    for field in ['_meta','mode','message','requestedSchema','url','elicitationId']:
        for value in bad_values:rows.append(request('elicitation/create',{field:value}))
    for field in ['_meta','includeContext','maxTokens','messages','metadata','modelPreferences','stopSequences','systemPrompt','temperature','tools','toolChoice']:
        for value in bad_values:rows.append(request('sampling/createMessage',{field:value}))
    for msg in [{},None,False,42,[],{'role':'user','content':None},{'role':'user','content':[]},{'ROLE':'user','content':{'type':'text'}},{'role':False,'content':{'type':'text'}},{'role':'user','CONTENT':{'type':'text'}}]:rows.append(request('sampling/createMessage',{'messages':[msg]}))
    for kind in ['text','image','audio','tool_use','tool_result','resource','resource_link','unknown']:
        for content in [{'type':kind},[{'type':kind}],[{'type':kind},{'type':'text'}]]:rows.append(request('sampling/createMessage',{'messages':[{'role':'user','content':content}]}))
    for content in [False,42,'bad',{},[None],[{'type':'text','text':42}],[{'type':'image','data':'%%%'}],[{'type':'text','TEXT':'ignored'}]]:rows.append(request('sampling/createMessage',{'messages':[{'role':'user','content':content}]}))
    for prefs in [{},{'hints':[None,{}]},{'hints':[{'NAME':'opaque','unknown':True}]},{'costPriority':.5,'intelligencePriority':.3,'speedPriority':.2}]:rows.append(request('sampling/createMessage',{'modelPreferences':prefs}))
    for field in ['costPriority','intelligencePriority','speedPriority','hints']:
        for value in bad_values:rows.append(request('sampling/createMessage',{'modelPreferences':{field:value}}))
    for hint in [False,42,[],{'name':False},{'name':None}]:rows.append(request('sampling/createMessage',{'modelPreferences':{'hints':[hint]}}))
    for tools in [[],[None],[{}],[{'name':'fixture','inputSchema':{'type':'object'},'annotations':{},'icons':[None,{'src':'icon','sizes':[None]}]}],[{'annotations':{'destructiveHint':False,'openWorldHint':False}}]]:rows.append(request('sampling/createMessage',{'tools':tools}))
    for field in ['name','description','title','inputSchema','outputSchema','annotations','icons','_meta']:
        for value in bad_values:rows.append(request('sampling/createMessage',{'tools':[{field:value}]}))
    for field in ['destructiveHint','openWorldHint','idempotentHint','readOnlyHint','title']:
        for value in bad_values:rows.append(request('sampling/createMessage',{'tools':[{'annotations':{field:value}}]}))
    for value in bad_values:rows.append(request('sampling/createMessage',{'toolChoice':{'mode':value}}))
    rows.append({'inputRequests':{'a':{'method':'roots/list','params':{}},'b':{'method':'elicitation/create','params':{'message':'confirm'}}}})
    for params in [
        {'MESSAGE':'keep','message':None}, {'MODE':'url','mode':None},
        {'MODELPreferences':{'costPriority':.5},'modelPreferences':{'speedPriority':.2}},
        {'modelPreferences':{'HINTS':[{'name':'keep'}],'hints':[{'name':None}]}},
        {'STOPSequences':['keep'],'stopSequences':[None]},
        {'TOOLS':[{'name':'keep','annotations':{'title':'keep'}}],'tools':[{'name':None,'annotations':{}}]},
        {'METADATA':{'keep':True},'metadata':None},
        {'_META':{'keep':True},'_meta':{'other':False}},
    ]:
        method='elicitation/create' if 'message' in params or 'mode' in params else 'sampling/createMessage'
        rows.append(request(method,params))
    for params in [{'maxTokens':False,'messages':[{'content':None}]},{'maxTokens':False,'messages':[{'role':False,'content':{'type':'text'}}]},{'messages':[{'content':{'type':'image','data':'%%%'}}],'systemPrompt':False}]:rows.append(request('sampling/createMessage',params))
    for params in [
        {'STOPSEQUENCES':['first','second'],'StopSequences':['changed'],'stopSequences':[None,None]},
        {'STOPSEQUENCES':['first','second'],'StopSequences':None,'stopSequences':[None]},
        {'MODELPreferences':{'hints':[{'name':'old'}]},'ModelPreferences':None,'modelPreferences':{'hints':[{'name':None}]}},
        {'modelPreferences':{'HINTS':[{'name':'first'},{'name':'second'}],'Hints':[{'name':'changed'}],'hints':[{'name':None},{'name':None}]}},
        {'TOOLS':[{'name':'first'},{'name':'second'}],'Tools':[{'name':'changed'}],'tools':[{'name':None},{'name':None}]},
        {'TOOLS':[{'name':'first','icons':[{'src':'icon'}]}],'Tools':[None],'tools':[{'name':None,'icons':[None]}]},
    ]:rows.append(request('sampling/createMessage',params))
    return rows

def run(go,rust,out):
    go=freeze_binary(go,'go');rust=freeze_binary(rust,'rust');out.mkdir(parents=True,exist_ok=True)
    lock=json.loads((ROOT/'baseline/source-lock.json').read_text())
    def git(*args):return subprocess.check_output(['git','-C',str(ROOT/'.parity/go-src'),*args],text=True).strip()
    assert git('rev-parse','HEAD')==lock['sourceCommit'] and git('rev-parse','HEAD^{tree}')==lock['sourceTree'] and not git('status','--porcelain')
    sources=[Path(__file__),ROOT/'crates/host-agent/src/provider/result.rs',ROOT/'tools/parity/provider-core-oracle/main.go',ROOT/'tools/parity/provider-core-oracle/go.mod',ROOT/'tools/parity/provider-core-oracle/go.sum',ROOT/'tools/parity/provider_adapter.py',ROOT/'Cargo.lock',ROOT/'crates/host-agent/src/mcpsdk.rs',ROOT/'crates/host-agent/src/gojson.rs',ROOT/'crates/host-agent/src/provider/result/requests.rs']
    before={str(p.relative_to(ROOT)):sha(p) for p in sources};rows=[];payload=cases()
    canary_input={'content':[],'inputRequests':{},'resultType':'complete'}
    canary=json.loads(subprocess.check_output([str(go)],input=json.dumps([{'kind':'result','value':canary_input},{'kind':'result-value-canary','value':canary_input}]),text=True))
    assert canary[0]=={'result':canary_input} and canary[1]!=canary[0], 'oracle does not exercise SDK pointer JSON marshaler'
    (out/'serializer-canary.json').write_text(json.dumps({'input':canary_input,'pointer':canary[0],'byValue':canary[1],'brokenByValueDetected':True},indent=2)+'\n')
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
    report={'scope':'T0 SDK content/default/omission projection, typed malformed fields, exact case-sensitive decoding, integer/base64 errors and client-map float64 conversion; elicitation/roots/sampling input requests; null-entry Go panic is excluded pending a separately reviewed refusal contract; public activation, broader HTTP failures and complete M7 E2E remain open','sourceLock':lock,'provenance':before,'goBinarySha256':sha(go),'rustTestBinarySha256':sha(rust),'rows':rows,'total':len(rows),'passed':sum(r['pass'] for r in rows),'normalization':'none; structured projections compared exactly'}
    (out/'summary.json').write_text(json.dumps(report,indent=2)+'\n');(out/'index.json').write_text(json.dumps({str(p.relative_to(out)):sha(p) for p in out.rglob('*.json') if p.name!='index.json'},indent=2)+'\n')
    print('Provider SDK projection:',report['passed'],'/',report['total']);return all(r['pass'] for r in rows)
if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--go',type=Path,required=True);p.add_argument('--rust-tests',type=Path,required=True);p.add_argument('--out',type=Path,required=True)
    a=p.parse_args();raise SystemExit(0 if run(a.go.resolve(),a.rust_tests.resolve(),a.out.resolve()) else 1)
