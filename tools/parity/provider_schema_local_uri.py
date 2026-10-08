#!/usr/bin/env python3
"""Scoped native local schema checks and Go 1.25 URI differential evidence."""
import argparse
import gzip
import itertools
import json
import os
from pathlib import Path
import random
import string
import subprocess
from provider_adapter import ROOT, freeze_binary, sha
from provider_schema_decoder import equal


def local_cases():
    rows = []
    def add(schema, variants=False):
        rows.append({'case': 'local-'+str(len(rows)), 'schema': schema,
                     'resolverAction': 'local', 'variants': variants})
    conflicts = [{}, {'$defs': {}, 'definitions': {}},
        {'NOT': {'type': 'object'}, 'not': {'type': []}},
        {'NOT': {'items': {}}, 'not': {'items': []}},
        {'NOT': {'dependencies': {'x': {}}}, 'not': {'dependencies': {'x': []}}}]
    labels = [{}, {'$id': 'https://e.example/a'}, {'$anchor': 'anchor'},
              {'$dynamicAnchor': 'dynamic'}, {'$id': 'id', '$anchor': 'a', '$dynamicAnchor': 'd'},
              {'$anchor': '雪\n<&>"', '$dynamicAnchor': 'ignored'}]
    metas = ['', 'https://json-schema.org/draft/2020-12/schema',
             'https://json-schema.org/draft/2020-12/schema#', 'http://json-schema.org/draft-07/schema#']
    vocabularies = [None, {}, {'a': True}, {'a': False}, {'a': None}]
    for conflict, label, meta, vocabulary in itertools.product(conflicts, labels, metas, vocabularies):
        add({**conflict, **label, '$schema': meta, '$vocabulary': vocabulary})
    # Nonempty regexp syntax is unreachable only after a basic failure at that
    # exact node. No regexp success/refusal is synthesized by the fixture.
    for conflict in conflicts[1:]:
        child = next((k for k in ['not'] if k in conflict), None)
        for pattern in ['[', '(?=x)', 'a*', '\\p{Unknown}', '\n']:
            value = json.loads(json.dumps(conflict))
            node = value[child] if child else value
            node['pattern'] = pattern
            node['patternProperties'] = {'[': {}}
            add(value)
    fields = ['$defs', 'definitions', 'properties', 'dependentSchemas',
              'allOf', 'anyOf', 'oneOf', 'prefixItems', 'items', 'not', 'if', 'then', 'else',
              'additionalProperties', 'unevaluatedProperties', 'contentSchema', 'dependencies']
    nodes = [{}, {'$vocabulary': {}}, {'$defs': {}, 'definitions': {}},
             {'NOT': {'type': 'object'}, 'not': {'type': ['string']}},
             {'$schema': metas[1], '$vocabulary': {'v': True}}, None, False, True]
    for field, node in itertools.product(fields, nodes):
        if field in fields[:4]+['dependencies']: schema = {field: {'a~b/c': node}}
        elif field in ['allOf', 'anyOf', 'oneOf', 'prefixItems']: schema = {field: [node]}
        else: schema = {field: node}
        add({'$vocabulary': {}, **schema})
    for a, b in itertools.product(nodes[:-3], repeat=2):
        add({'$vocabulary': {}, 'allOf': [a, b], 'not': {'$anchor': 'child', '$vocabulary': {}},
             'properties': {'z': b, 'a': a}})
    for key in ['', '雪', 'a\n"', 'a/b~c']:
        add({'NOT': {'dependencies': {key: {}}}, 'not': {'dependencies': {key: []}}})
    add({'NOT': {'dependencies': {'a': {}, 'b': {}}},
         'not': {'dependencies': {'a': [], 'b': []}}}, True)
    for field in fields[:4]:
        add({field: {'a': None, 'b': None}}, True)
    for schema in [{'title': False}, {'minItems': 1.5}, {'properties': {'a': 42}},
                   {'pattern': None}, {'patternProperties': {'': {}}},
                   {'PropertyOrder': ['a', 'a']}, {'$vocabulary': False}]: add(schema)
    return rows


def uri_cases():
    rows=[]
    def add(base, ref=None):
        uri={'base':base}
        if ref is not None: uri['ref']=ref
        rows.append({'case':'uri-'+str(len(rows)), 'uri':uri})
    for x in ['', '*', ':', 'a:', '1:', 'a:b', 'a:/b', 'a://b', '//', '///',
              'http:///p', 'HTTP://a/p', '?', '??', '#', '#%', '#a b', 'a?b#c',
              'a%ff#%ff', '/a%2fb', '/a b', 'a/b:c', './a:b', '\n', '#\n']: add(x)
    for c in ''.join(chr(i) for i in range(128))+'雪😀\u0085\u2028\u2029':
        for template in ['http://{}/a', 'http://u{}:p@host/a', '/{}/a', 'a#{}', 'a?{}', 'a:{}']:
            add(template.format(c))
    for byte in range(256):
        for escape in ['%'+format(byte,'02x'), '%'+format(byte,'02X')]:
            for template in ['http://{}/p', 'http://u{}@host/p', '/{}', 'x#{}', 'http://[::1%25{}]/']:
                add(template.format(escape))
    for p in ['%', '%a', '%ax', '%雪', '%C3%A9', '%E2%98%83', '%c0%80']:
        for template in ['http://{}/p', 'http://u{}@host/p', '/{}', 'x#{}', 'http://[::1%25{}]/']:
            add(template.format(p))
    ips=['', '::', '::1', '1:2:3:4:5:6:7:8', '1::2::3', '1:2', '1:2:3:4:5:6:7:8:9',
         '::ffff:1.2.3.4', '1.2.3.4', '1.2.3.', '::1.2.3.', '::001.2.3.4', '::256.2.3.4',
         '::ffff:1.2.3.4.5', '::ffff:1.2.3.x', '::1%25eth0', '::1%25', '::1%25%20',
         'v1.foo', '::ffff:', '1:::', '12345::', '::%ff', '::0.0.0.0', '1:2:3:4:5:6:1.2.3.4']
    for ip, port in itertools.product(ips, ['', ':', ':0', ':65536', ':x', ':-1', ':１２']):
        add('http://['+ip+']'+port+'/p')
    for host in ['][', '][::', '[[::1]', '[::1]]', 'a[::1]', '][::1]', '::1', 'a:b',
                 'a:000', 'a:99999999999999999', 'u@x@y', 'u:p:q@h', ':p@h', '@', 'u:@', 'u@']:
        add('http://'+host+'/p')
    bases=['', 'http://a/b/c/d;p?q#frag', 'http:/a/b', 'urn:example:a', 'a/b',
           'http://u:p@a:3/%2f//d?#x', '//host/p', './a:b', 'http://h', 'x:', 'x://']
    refs=['', 'g', './g', 'g/', '/g', '//g', '?y', 'g?y', '#s', 'g#s', 'g?y#s', ';x',
          'g;x', 'g;x?y#s', '.', './', '..', '../', '../g', '../..', '../../', '../../g',
          '../../../g', '/./g', '/../g', 'g.', '.g', 'g..', '..g', './../g', './g/.',
          'g/./h', 'g/../h', 'g;x=1/./y', 'g;x=1/../y', 'g?y/./x', 'g#s/../x', 'http:g',
          'a:/x', 'a://x', 'g%2fh', '%ff', '?', '#', '//u:@h/x', '///x', '//', '/%2e/../x']
    for base, ref in itertools.product(bases,refs): add(base,ref)
    rng=random.Random(20261007)
    alphabet=string.ascii_letters+string.digits+':/.?@%#[]!$&+=,;_ ~雪'
    for _ in range(12000):
        value=''.join(rng.choice(alphabet) for _ in range(rng.randrange(40)))
        add(value)
        if rng.randrange(4)==0: add(rng.choice(bases),value)
    # Valid and invalid generated IPv6/embedded IPv4/zone/port boundaries.
    for _ in range(2000):
        groups=[rng.choice(['', ':', 'xyz', str(rng.randrange(300)), format(rng.randrange(0x20000),'x')]) for _ in range(rng.randrange(1,11))]
        address=':'.join(groups)
        if rng.randrange(3)==0: address+=':'+'.'.join(str(rng.randrange(300)) for _ in range(rng.randrange(1,6)))
        if rng.randrange(3)==0: address+='%25'+rng.choice(['eth0','a b','%ff','','雪'])
        add('http://['+address+']'+rng.choice(['',':',':12',':bad'])+'/p')
    return rows


def run(go,rust,out,kind):
    out.mkdir(parents=True,exist_ok=True)
    go=freeze_binary(go,'schema-local-uri-go');rust=freeze_binary(rust,'schema-local-uri-rust')
    receipt_path=ROOT/'.parity/m7/schema-reference-source.json'
    receipt=json.loads(receipt_path.read_text())
    paths=[Path(__file__),ROOT/'tools/parity/provider_schema_decoder.py',ROOT/'tools/parity/provider_adapter.py',
           ROOT/'tools/parity/build-provider-catalog-oracle.py',ROOT/'Cargo.lock',ROOT/'baseline/source-lock.json',
           ROOT/'.parity/m7/provider-catalog-oracle.mod',ROOT/'.parity/m7/provider-catalog-oracle.sum',receipt_path]
    paths+=list((ROOT/'tools/parity/provider-catalog-oracle').iterdir())
    paths+=list((ROOT/'crates/host-agent/src/catalog/schema').glob('*.rs'))
    paths+=[ROOT/'crates/host-agent/src/catalog/schema.rs',ROOT/'crates/host-agent/src/catalog/schema-LICENSE.txt',
            ROOT/'crates/host-agent/src/gojson.rs',ROOT/'crates/host-agent/src/goerr.rs',
            ROOT/'crates/host-agent/src/goerr/printable.rs',ROOT/'licenses/Go-BSD.txt']
    for folder in [Path(receipt['source']),Path(receipt['snapshot'])]:
        assert all(sha(folder/k)==v for k,v in receipt['files'].items())
        paths += [folder/k for k in receipt['files']]
    for package in ['net/url','net/netip','strconv']:
        paths+=list((ROOT/'.parity/setup/go/src'/package).glob('*.go'))
    hashes={str(p):sha(p) for p in paths}
    lock=json.loads((ROOT/'baseline/source-lock.json').read_text())
    def pinned():
        def git(*args):return subprocess.check_output(['git','-C',str(ROOT/'.parity/go-src'),*args],text=True).strip()
        assert git('rev-parse','HEAD')==lock['sourceCommit'] and git('rev-parse','HEAD^{tree}')==lock['sourceTree']
        assert not git('status','--porcelain')
    pinned();rows=local_cases() if kind=='local' else uri_cases()
    variants=[r for r in rows if r.get('variants')];allowed={}
    if variants:
        raw=[json.loads(subprocess.check_output([str(go)],input=json.dumps(variants),text=True,timeout=60)) for _ in range(128)]
        (out/'variant-input.json').write_text(json.dumps(variants,indent=2)+'\n')
        (out/'variant-go.json.gz').write_bytes(gzip.compress(json.dumps(raw).encode(),mtime=0))
        allowed={r['case']:[v[i] for v in raw] for i,r in enumerate(variants)}
        assert not any(equal({'error':'invented refusal'},v) for values in allowed.values() for v in values)
    comparisons=[]
    for iteration in range(5):
        folder=out/str(iteration);folder.mkdir(parents=True,exist_ok=True)
        inp=folder/'input.json';target=folder/'rust.json'
        inp.write_text(json.dumps(rows,indent=2)+'\n')
        reference=json.loads(subprocess.check_output([str(go)],input=json.dumps(rows),text=True,timeout=60))
        test='resolver_fixture' if kind=='local' else 'uri_fixture'
        prefix='RESOLVER' if kind=='local' else 'URI'
        subprocess.run([str(rust),'catalog::schema::tests::'+test,'--exact','--ignored'],
            env={**os.environ,'PARITY_SCHEMA_'+prefix+'_INPUT':str(inp.resolve()),'PARITY_SCHEMA_'+prefix+'_OUTPUT':str(target.resolve())},
            capture_output=True,check=True,timeout=60)
        actual=json.loads(target.read_text());target.unlink();assert len(rows)==len(reference)==len(actual)
        for side,data in [('go',reference),('rust',actual)]:
            (folder/(side+'.json.gz')).write_bytes(gzip.compress(json.dumps(data).encode(),mtime=0))
        for row,left,right in zip(rows,reference,actual):
            passed=any(equal(right,v) for v in allowed[row['case']]) if row.get('variants') else equal(left,right)
            comparisons.append({'case':row['case'],'iteration':iteration,'pass':passed,'goMapVariants':row.get('variants',False)})
            if not passed and iteration==0:print('FAIL',row,left,right,flush=True)
    assert hashes=={str(p):sha(p) for p in paths};pinned()
    report={'scope':('T0 actual jsonschema.check vs native structural/basic/vocabulary checks with regexp-free fixture or regexp behind basic-check short-circuit; full regexp compilation, URI/anchor/reference graph and registry wiring remain open.' if kind=='local' else 'T0 actual Go1.25.4 net/url.Parse and ResolveReference vs private native byte-preserving URI parser/composer. Schema ID/anchor/ref graph and registry wiring remain open.'),
            'sourceLock':lock,'librarySource':receipt,'provenance':hashes,'goBinarySha256':sha(go),'rustTestBinarySha256':sha(rust),
            'cases':len(rows),'iterations':5,'passed':sum(r['pass'] for r in comparisons),'total':len(comparisons),'rows':comparisons,
            'goMapVariantReferenceRuns':128 if variants else 0,
            'allowedMapOutcomes':{k:sorted({json.dumps(v,sort_keys=True) for v in values}) for k,values in allowed.items()},
            'normalization':'Exact whole outcome comparison, including complete errors, URI component bytes as hex, nil user/password, escaped hints and composed strings. No URL canonicalization or error rewriting. For five multi-fault unordered-map cases, native error must equal an entire observed Go outcome from 128 independent runs; invented refusal rejected.' if kind=='local' else 'Exact whole outcome comparison. Decoded component bytes encoded as hex only to retain invalid UTF-8; raw path/fragment, nil username/password presence, all flags, original query, escaped components and URL.String compared without canonicalization or error rewriting.'}
    (out/'summary.json').write_text(json.dumps(report,indent=2)+'\n')
    (out/'index.json').write_text(json.dumps({str(p.relative_to(out)):sha(p) for p in out.rglob('*') if p.is_file() and p.name!='index.json'},indent=2)+'\n')
    print(kind,report['passed'],'/',report['total']);return report['passed']==report['total']


if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--go',type=Path,required=True)
    parser.add_argument('--rust-tests',type=Path,required=True);parser.add_argument('--out',type=Path,required=True)
    parser.add_argument('--kind',choices=['local','uri'],required=True);args=parser.parse_args()
    raise SystemExit(0 if run(args.go.resolve(),args.rust_tests.resolve(),args.out.resolve(),args.kind) else 1)
