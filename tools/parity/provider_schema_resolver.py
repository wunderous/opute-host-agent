#!/usr/bin/env python3
"""Owning typed JSON Pointer and structural index differential evidence."""
import argparse
import gzip
import json
import math
import os
from pathlib import Path
import random
import struct
import subprocess
from provider_adapter import ROOT, freeze_binary, sha
from provider_schema_decoder import equal


def cases():
    rows = []
    def add(name, schema, pointer='', action='pointer', variants=False):
        rows.append({'case': name, 'schema': schema, 'pointer': pointer,
                     'resolverAction': action, 'variants': variants})
    strings = ['$id', '$schema', '$ref', '$comment', '$anchor', '$dynamicAnchor', '$dynamicRef',
               'title', 'description', 'pattern', 'contentEncoding', 'contentMediaType', 'format']
    floats = ['multipleOf', 'minimum', 'maximum', 'exclusiveMinimum', 'exclusiveMaximum']
    integers = ['minLength', 'maxLength', 'minItems', 'maxItems', 'minContains', 'maxContains', 'minProperties', 'maxProperties']
    bools = ['deprecated', 'readOnly', 'writeOnly', 'uniqueItems']
    maps = ['$defs', 'definitions', 'properties', 'patternProperties', 'dependentSchemas']
    lists = ['prefixItems', 'allOf', 'anyOf', 'oneOf']
    children = ['additionalItems', 'contains', 'unevaluatedItems', 'additionalProperties',
                'propertyNames', 'unevaluatedProperties', 'not', 'if', 'then', 'else', 'contentSchema']
    rich = {**{k: 'label/~\n雪<&>' for k in strings}, **{k: 1 for k in floats+integers},
            **{k: True for k in bools}, **{k: {'a': {'title': 'selected'}, 'a/b': False,
                'a~b': True, 'a~1': {}, '': None} for k in maps},
            **{k: [None, {'title': 'selected'}, False] for k in lists},
            **{k: {'title': 'selected'} for k in children},
            'type': ['string', None], 'items': [None, False, {}],
            'dependencies': {'a': {'title': 'selected'}, 'strings': ['a']},
            'required': ['one', None], 'dependentRequired': {'a': ['one', None], 'nil': None},
            '$vocabulary': {'a': True, 'nil': None}, 'enum': [None, False, 1e6, {'a': [1e-9, True]}],
            'examples': [None, {'a': [True, 1e20]}], 'const': None,
            'default': {'a': [None, True, 1e9, '<&>雪']}, 'unknown': {'a': {}}, 'PropertyOrder': ['a']}
    fields = list(rich)+['Type', 'Extra', 'Properties', 'unknown-field']
    suffixes = ['', '/0', '/1', '/2', '/0/next', '/a', '/a/not', '/a/title/next',
                '/a~1b', '/a~0b', '/a~01', '/', '//not', '/strings', '/nil', '/nil/0',
                '/-1', '/-0', '/00', '/01', '/-', '/+0', '/+01', '/ 0',
                '/9223372036854775808', '/-9223372036854775809', '/雪']
    for label, schema in [('empty', {}), ('rich', rich)]:
        for pointer in ['', 'a', '/', '//', 'a/b', '~1', '\n', '雪']:
            add(label+'-parse-'+repr(pointer), schema, pointer)
        for field in fields:
            for suffix in suffixes:
                add(label+'-'+field+'-'+suffix, schema, '/'+field+suffix)
    for value in [None, True, False, {}, [], [{'title': 'target'}]]:
        for field in children+['items']:
            for suffix in ['', '/not', '/0', '/0/not', '/title', '/title/x']:
                add('child-'+str(len(rows)), {field: value}, '/'+field+suffix)
    for key in ['', '~', '~0', '~1', '~01', '/', 'a/b', 'a~b', '雪', '\n']:
        escaped = key.replace('~', '~0').replace('/', '~1')
        for suffix in ['', '/not', '/title/x']:
            add('escaped-'+str(len(rows)), {'properties': {key: {'title': 'selected'}}}, '/properties/'+escaped+suffix)
    for value in ['', 'object', [], ['object'], [None]]:
        for suffix in ['', '/0', '/0/next']:
            add('type-'+str(len(rows)), {'type': value}, '/type'+suffix)
    for value in [None, False, True, -0.0, '<&>雪', {'a': [False, 1e-9]}]:
        for index in range(24):
            for suffix in ['', '/next']:
                add('raw-'+str(len(rows)), {'default': value}, '/default/'+str(index)+suffix)
    for schema in [{'title': False}, {'minItems': 1.5}, {'properties': {'a': 42}},
                   {'NOT': {'items': {}}, 'not': {'items': []}},
                   {'NOT': {'type': 'object'}, 'not': {'type': ['string']}}]:
        add('decoder-'+str(len(rows)), schema, '/not/items')
    rng = random.Random(0xACE7013)
    numbers = [-0.0, 0, 1, 1e-4, 1e-5, 1e6, 1e20, 1e-9, 5e-324, -5e-324]
    while len(numbers) < 522:
        value = struct.unpack('>d', rng.getrandbits(64).to_bytes(8, 'big'))[0]
        if math.isfinite(value): numbers.append(value)
    for index, value in enumerate(numbers):
        add('float-format-'+str(index), {'enum': [value]}, '/enum/0/next')
    for value in [None, False, True, '', '雪', [False, 1e6, None],
                  {'a': [False, 1e-9], 'b': {'x': 'text'}, 'c': None}]:
        add('reflect-format-'+str(len(rows)), {'enum': [value]}, '/enum/0/next')
    structural = [{}]
    for field in maps:
        for key in ['', 'a/b', 'a~b', '雪']:
            for value in [None, True, False, {'not': {'properties': {'x/y~z': {}}}}]:
                structural.append({field: {key: value}})
    for field in lists+children+['items', 'dependencies']:
        for value in [None, False, True, {}]:
            structural.append({field: [value] if field in lists else {'a': value} if field == 'dependencies' else value})
    structural += [{'unknown': {'properties': {'a': None}}}, {'default': {'not': None}},
                   {'NOT': {'items': {}}, 'not': {'items': [None]}},
                   {'allOf': [None], 'properties': {'a': None}},
                   {'not': {'properties': {'a~b/c': None}}}]
    for index, schema in enumerate(structural): add('structure-'+str(index), schema, action='structure')
    for field in maps:
        add('map-order-'+field, {field: {'a/b': None, 'a~b': None}}, action='structure', variants=True)
        add('nested-map-order-'+field, {field: {'a': {'not': {'properties': {'x': None}}},
            'b': {'not': {'allOf': [None]}}}}, action='structure', variants=True)
    assert len({r['case'] for r in rows}) == len(rows)
    return rows


def run(go, rust, out):
    out.mkdir(parents=True, exist_ok=True)
    go = freeze_binary(go, 'schema-resolver-go'); rust = freeze_binary(rust, 'schema-resolver-rust')
    source_receipt = ROOT/'.parity/m7/schema-reference-source.json'
    receipt = json.loads(source_receipt.read_text())
    paths = [Path(__file__), ROOT/'tools/parity/provider_schema_decoder.py',
             ROOT/'tools/parity/provider_schema.py', ROOT/'tools/parity/provider_adapter.py',
             ROOT/'tools/parity/build-provider-catalog-oracle.py', ROOT/'Cargo.lock',
             ROOT/'crates/host-agent/src/catalog/schema.rs', ROOT/'crates/host-agent/src/catalog/schema/resolve.rs',
             ROOT/'crates/host-agent/src/catalog/schema-LICENSE.txt', ROOT/'crates/host-agent/src/gojson.rs',
             ROOT/'crates/host-agent/src/goerr.rs', ROOT/'crates/host-agent/src/goerr/printable.rs',
             ROOT/'.parity/m7/provider-catalog-oracle.mod', ROOT/'.parity/m7/provider-catalog-oracle.sum', source_receipt]
    paths += sorted((ROOT/'tools/parity/provider-catalog-oracle').iterdir())
    for folder in [Path(receipt['source']), Path(receipt['snapshot'])]:
        assert all(sha(folder/k) == v for k, v in receipt['files'].items())
        paths += [folder/k for k in receipt['files']]
    hashes = {str(p): sha(p) for p in paths}
    lock = json.loads((ROOT/'baseline/source-lock.json').read_text())
    def pinned():
        def git(*args): return subprocess.check_output(['git', '-C', str(ROOT/'.parity/go-src'), *args], text=True).strip()
        assert git('rev-parse', 'HEAD') == lock['sourceCommit'] and git('rev-parse', 'HEAD^{tree}') == lock['sourceTree']
        assert not git('status', '--porcelain')
    pinned(); source = cases(); variant_rows = [r for r in source if r['variants']]
    (out/'variant-input.json').write_text(json.dumps(variant_rows, indent=2)+'\n')
    reference_variants = []
    for _ in range(128):
        reference_variants.append(json.loads(subprocess.check_output([str(go)], input=json.dumps(variant_rows), text=True, timeout=60)))
    (out/'variant-go.json.gz').write_bytes(gzip.compress(json.dumps(reference_variants).encode(), mtime=0))
    allowed = {case['case']: [v[i] for v in reference_variants] for i, case in enumerate(variant_rows)}
    def matches(case, left, right):
        return any(equal(right, a) for a in allowed[case['case']]) if case['variants'] else equal(left, right)
    assert not matches(variant_rows[0], {}, {'error': 'invented refusal', 'paths': None})
    comparisons = []
    for iteration in range(5):
        folder = out/str(iteration); folder.mkdir(parents=True, exist_ok=True)
        inp, target = folder/'input.json', folder/'rust.json'; inp.write_text(json.dumps(source, indent=2)+'\n')
        reference = json.loads(subprocess.check_output([str(go)], input=json.dumps(source), text=True, timeout=60))
        subprocess.run([str(rust), 'catalog::schema::tests::resolver_fixture', '--exact', '--ignored'],
            env={**os.environ, 'PARITY_SCHEMA_RESOLVER_INPUT': str(inp.resolve()), 'PARITY_SCHEMA_RESOLVER_OUTPUT': str(target.resolve())}, capture_output=True, check=True, timeout=60)
        actual = json.loads(target.read_text()); target.unlink(); assert len(reference) == len(actual) == len(source)
        for side, data in [('go', reference), ('rust', actual)]:
            (folder/(side+'.json.gz')).write_bytes(gzip.compress(json.dumps(data).encode(), mtime=0))
        for case, left, right in zip(source, reference, actual):
            passed = matches(case, left, right); comparisons.append({'case': case['case'], 'iteration': iteration, 'pass': passed, 'goMapVariants': case['variants']})
            if not passed and iteration == 0: print('FAIL', case['case'], left.get('error'), right.get('error'), flush=True)
    assert hashes == {str(p): sha(p) for p in paths}; pinned()
    report = {'scope': 'T0 actual private Go dereferenceJSONPointer and checkStructure vs native typed pointer resolution and structural indexing. Resolve/local checks, regexp, URI/anchor/reference resolution and registry wiring remain open.',
        'sourceLock': lock, 'librarySource': receipt, 'provenance': hashes,
        'goBinarySha256': sha(go), 'rustTestBinarySha256': sha(rust), 'cases': len(source),
        'iterations': 5, 'rows': comparisons, 'passed': sum(r['pass'] for r in comparisons), 'total': len(comparisons),
        'goMapVariantReferenceRuns': 128,
        'allowedMapOutcomes': {k: sorted({json.dumps(v, sort_keys=True) for v in values}) for k, values in allowed.items()},
        'normalization': 'Pointer errors, nil targets and full typed target projections compare without error normalization; float64 bits and raw bytes retained. Successful structural paths are sorted by the test-only Go bridge because its index is a map. For ten multi-fault map cases, native refusal must exactly equal a complete observed owning Go outcome from 128 independent runs; no error string is rewritten. An invented refusal is a negative canary.'}
    (out/'summary.json').write_text(json.dumps(report, indent=2)+'\n')
    (out/'index.json').write_text(json.dumps({str(p.relative_to(out)): sha(p) for p in out.rglob('*') if p.is_file() and p.name != 'index.json'}, indent=2)+'\n')
    print('Schema pointers/structure:', report['passed'], '/', report['total'])
    return all(r['pass'] for r in comparisons)


if __name__ == '__main__':
    p = argparse.ArgumentParser(description=__doc__); p.add_argument('--go', type=Path, required=True)
    p.add_argument('--rust-tests', type=Path, required=True); p.add_argument('--out', type=Path, required=True)
    a = p.parse_args(); raise SystemExit(0 if run(a.go.resolve(), a.rust_tests.resolve(), a.out.resolve()) else 1)
