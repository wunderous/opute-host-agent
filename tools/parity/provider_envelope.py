#!/usr/bin/env python3
"""T0 typed Go client response envelopes, syntax diagnostics and byte-valued input."""
import argparse
import json
import os
from pathlib import Path
import subprocess
from provider_adapter import ROOT,cases,freeze_binary,sha

def payloads():
    rows=[]
    for case in cases().values():
        if 'callBytes' in case:rows.append(case['callBytes'])
        elif 'callBody' in case:rows.append(list(case['callBody'].encode()))
    prefixes=[b'',b'{',b'{"key"',b'{"key":',b'{"key":1',b'[',b'[1',b'"',b'"\\',b'"\\u',b'-',b'1.',b'1e',b'1e+',b'0',b'1.0',b'1e1',b'"\\u0',b'"\\u00',b'"\\u000',b't',b'tr',b'tru',b'f',b'fa',b'fal',b'fals',b'n',b'nu',b'nul',b'{}']
    for prefix in prefixes:
        for byte in range(256):rows.append(list(prefix+bytes([byte])))
    for doc in [b'{"result":{"key":true}}',b'{"error":{"code":1,"message":"refusal"}}',b'"\\uD800\\uDC00"',b'[1.2e-3,false,null]']:
        for length in range(len(doc)+1):rows.append(list(doc[:length]))
    return rows

def run(go,rust,out):
    go=freeze_binary(go,'go');rust=freeze_binary(rust,'rust');out.mkdir(parents=True,exist_ok=True)
    paths=[Path(__file__),ROOT/'tools/parity/provider_adapter.py',ROOT/'tools/parity/provider-core-oracle/main.go',ROOT/'tools/parity/provider-core-oracle/go.mod',ROOT/'tools/parity/provider-core-oracle/go.sum',ROOT/'crates/host-agent/src/provider/adapter/envelope.rs',ROOT/'crates/host-agent/src/gojson.rs',ROOT/'crates/host-agent/src/gojson/validate.rs',ROOT/'crates/host-agent/src/goerr.rs',ROOT/'crates/host-agent/src/goerr/printable.rs',ROOT/'tools/parity/generate_go_isprint.py',ROOT/'Cargo.lock']
    before={str(p.relative_to(ROOT)):sha(p) for p in paths};lock=json.loads((ROOT/'baseline/source-lock.json').read_text())
    def git(*args):return subprocess.check_output(['git','-C',str(ROOT/'.parity/go-src'),*args],text=True).strip()
    assert git('rev-parse','HEAD')==lock['sourceCommit'] and git('rev-parse','HEAD^{tree}')==lock['sourceTree'] and not git('status','--porcelain')
    payload=payloads();rows=[]
    for iteration in range(5):
        folder=out/str(iteration);folder.mkdir(parents=True,exist_ok=True)
        source=folder/'input.json';source.write_text(json.dumps(payload)+'\n')
        reference=json.loads(subprocess.check_output([str(go)],input=json.dumps([{'kind':'envelope','value':p} for p in payload]),text=True));(folder/'go.json').write_text(json.dumps(reference,indent=2)+'\n')
        target=folder/'rust.json';env={**os.environ,'PARITY_PROVIDER_ENVELOPE_INPUT':str(source.resolve()),'PARITY_PROVIDER_ENVELOPE_OUTPUT':str(target.resolve())}
        subprocess.run([str(rust),'provider::adapter::envelope::tests::envelope_fixture','--exact','--ignored'],env=env,capture_output=True,check=True)
        actual=json.loads(target.read_text())
        for i,(a,b) in enumerate(zip(reference,actual,strict=True)):
            passed=a==b;rows.append({'case':i,'iteration':iteration,'pass':passed})
            if not passed and iteration==0:print('FAIL',i,a,b)
    assert before=={str(p.relative_to(ROOT)):sha(p) for p in paths}
    assert git('rev-parse','HEAD')==lock['sourceCommit'] and not git('status','--porcelain')
    report={'scope':'T0 actual pinned mcphttp.Client with a fixed response transport: syntax, typed response envelopes, duplicates/case folding/null, float64 conversion, invalid UTF-8; HTTP transport/cancellation/proxy/TLS/deep runtime processing remain separate','sourceLock':lock,'provenance':before,'goBinarySha256':sha(go),'rustTestBinarySha256':sha(rust),'rows':rows,'total':len(rows),'passed':sum(r['pass'] for r in rows),'normalization':'none; every decoded result/error compared exactly'}
    (out/'summary.json').write_text(json.dumps(report,indent=2)+'\n');(out/'index.json').write_text(json.dumps({str(p.relative_to(out)):sha(p) for p in out.rglob('*.json') if p.name!='index.json'},indent=2)+'\n');print('Typed response envelopes:',report['passed'],'/',report['total']);return all(r['pass'] for r in rows)
if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--go',type=Path,required=True);p.add_argument('--rust-tests',type=Path,required=True);p.add_argument('--out',type=Path,required=True);a=p.parse_args();raise SystemExit(0 if run(a.go.resolve(),a.rust_tests.resolve(),a.out.resolve()) else 1)
