#!/usr/bin/env python3
"""Capture the owning Go schema compiler; compare implemented native components."""
import argparse
import gzip
import json
import os
from pathlib import Path
import subprocess
from provider_adapter import ROOT, freeze_binary, sha


def cases():
    rows = []
    def add(name, schema, group='compiler'):
        rows.append({'case': name, 'group': group, 'schema': schema})
    add('empty', {})
    integers = ['minLength', 'maxLength', 'minItems', 'maxItems', 'minProperties',
                'maxProperties', 'minContains', 'maxContains']
    values = [None, 0, -1, 1, 1.0, 1.5, -1.5, 2147483647, 2147483648,
              -2147483648, -2147483649, 1e20, -1e20, 1e-7, 1.1e-7,
              9223372036854775807, -9223372036854775808,
              True, False, '', '1', '1.5', [], {}, 'not.a.number', '雪']
    for field in integers:
        for i, value in enumerate(values):
            add(f'integer-{field}-{i}', {field: value}, 'integer')
    strings = ['$id', '$schema', '$ref', '$comment', '$anchor', '$dynamicAnchor',
               '$dynamicRef', 'title', 'description', 'pattern', 'contentEncoding',
               'contentMediaType', 'format']
    booleans = ['deprecated', 'readOnly', 'writeOnly', 'uniqueItems']
    floats = ['multipleOf', 'minimum', 'maximum', 'exclusiveMinimum', 'exclusiveMaximum']
    schema_maps = ['$defs', 'definitions', 'properties', 'patternProperties', 'dependentSchemas']
    schema_arrays = ['prefixItems', 'allOf', 'anyOf', 'oneOf']
    schema_pointers = ['additionalItems', 'contains', 'unevaluatedItems',
                       'additionalProperties', 'propertyNames', 'unevaluatedProperties',
                       'not', 'if', 'then', 'else', 'contentSchema']
    for field in strings + booleans + floats + schema_maps + schema_arrays + schema_pointers + [
        'type', 'items', 'dependencies', 'required', 'dependentRequired', '$vocabulary',
        'default', 'enum', 'const', 'examples', 'unknown-keyword', 'PropertyOrder']:
        for i, value in enumerate([None, True, False, '', 'object', 0, 1.5, [], {}, [None], ['string'], {'a': None}]):
            add(f'field-{field}-{i}', {field: value})
    for field in schema_maps:
        for key in ['a', 'a/b', 'a~b', '雪', '']:
            for child in [None, False, True, {'required': [42]}, {'title': False}, {'minItems': 1.5}]:
                add(f'map-child-{field}-{key}-{len(rows)}', {field: {key: child}})
    for field in schema_arrays + schema_pointers + ['items']:
        for child in [None, False, True, 42, 'wrong', {'title': False}, {'minItems': 1.5}]:
            add(f'child-{field}-{len(rows)}', {field: [child] if field in schema_arrays else child})
    for field in strings + booleans + floats + integers + ['type', 'items', 'properties', 'required']:
        for index, spelling in enumerate([field.upper(), field[:1].upper()+field[1:], field.replace('s', 'ſ')]):
            add(f'alias-{field}-{index}-{spelling}', {spelling: False})
    for typ in ['', 'wrong', 'object', ['wrong'], [], [None], ['object', 'object']]:
        add('type-accepted-'+repr(typ), {'type': typ})
    for dep in [None, True, False, 42, [], ['a'], [None], [42], {'title': False}]:
        add('dependency-'+repr(dep), {'dependencies': {'a': dep}})
    for schema in [
        {'$defs': {}, 'definitions': {}}, {'$defs': None, 'definitions': {}},
        {'$vocabulary': {}}, {'$vocabulary': {'test': False}},
        {'$schema': 'https://json-schema.org/draft/2020-12/schema', '$vocabulary': {}},
        {'title': False, 'minItems': 1.5}, {'description': False, 'title': 42},
        {'type': True, 'items': 42}, {'items': 42, 'minItems': 1.5},
        {'properties': {'x': {'title': False}}, 'minItems': 1.5},
        {'properties': {'x': None}, 'allOf': [None]},
        {'$defs': {}, 'definitions': {}, 'pattern': '['},
        {'$vocabulary': {}, 'pattern': '['},
        {'properties': {'a': {'$vocabulary': {}}, 'b': {'pattern': '['}}},
        {'properties': {'a': {'$defs': {}, 'definitions': {}}}},
        {'unknown-keyword': {'properties': {'a': None}, 'pattern': '['}},
    ]:
        add('priority-'+str(len(rows)), schema)
    drafts = [None, 'http://json-schema.org/draft-07/schema#',
              'https://json-schema.org/draft-07/schema#',
              'https://json-schema.org/draft/2020-12/schema', 'unknown']
    for draft in drafts:
        base = {} if draft is None else {'$schema': draft}
        for uri in ['', '#foo', 'relative', '/absolute-path', 'https://host/s',
                    'https://host/s#foo', 'urn:test:schema', 'https://host/%zz',
                    'https://[::1', 'bad\nuri', 'https://host/s?q=1', '//host/s', 'a:b']:
            add(f'id-{draft}-{uri}', {**base, '$id': uri})
        for ref in ['', '#', '#foo', '#/properties/a', '#/missing', '#/properties',
                    '#/type', '#/properties/a~1b', '#/properties/a~0b', '#/allOf/0',
                    '#/allOf/01', '#/allOf/-', '#/allOf/+0', '#/allOf/-1',
                    '#/allOf/1', '#/title', '#/unknown-keyword', '#/%zz', 'other']:
            schema = {**base, 'properties': {'a': {}, 'a/b': True, 'a~b': False},
                      'allOf': [{}], '$anchor': 'foo', 'title': 'fixture',
                      'unknown-keyword': {}, '$ref': ref}
            add(f'ref-{draft}-{ref}', schema)
        for ref in ['#foo', '#bar', '#/properties/a', 'https://host/s#foo',
                    'https://host/other', './s#foo']:
            add(f'absolute-ref-{draft}-{ref}', {**base, '$id': 'https://host/s',
                '$ref': ref, 'properties': {'a': {'$anchor': 'foo'}}})
    for field in ['$ref', '$dynamicRef']:
        for anchor in ['$anchor', '$dynamicAnchor']:
            for ref in ['#a', '#missing', '#/properties/a']:
                add(f'anchor-{field}-{anchor}-{ref}', {field: ref, 'properties': {'a': {anchor: 'a'}}})
    for schema in [
        {'$ref': '#'}, {'properties': {'a': {'$ref': '#'}}},
        {'$defs': {'a': {'$ref': '#/$defs/b'}, 'b': {'$ref': '#/$defs/a'}}},
        {'$id': 'https://host/s', 'properties': {'a': {'$id': 'child', '$ref': '#'}}},
        {'$id': 'https://host/s', 'properties': {'a': {'$id': 'child', '$ref': '#/properties/a'}}},
    ]:
        add('recursive-'+str(len(rows)), schema)
    patterns = ['', 'a', '[', '(', ')', 'a{', 'a{2,1}', '*', 'a**', '\\',
                '\\1', '(?=a)', '(?!a)', '(?<=a)', '(?P<name>a)', '(?<name>a)',
                '(?i:a)', '(?U)a', '\\p{Greek}', '\\p{Unknown}', '[z-a]',
                '[[:alpha:]]', 'a{1001}', '\\C', '雪+', 'a\\z', '\\u1234',
                '(?:a)', '(?x)a', '[\\d-a]']
    for pattern in patterns:
        add('pattern-'+repr(pattern), {'pattern': pattern})
        add('pattern-property-'+repr(pattern), {'patternProperties': {pattern: {}}})
    assert len({row['case'] for row in rows}) == len(rows)
    return rows


def run(go, rust, out):
    out.mkdir(parents=True, exist_ok=True)
    go = freeze_binary(go, 'schema-go')
    rust = freeze_binary(rust, 'schema-integer-rust')
    library = Path('/home/houman/go/pkg/mod/github.com/google/jsonschema-go@v0.4.3/jsonschema')
    paths = [Path(__file__), ROOT/'tools/parity/provider_adapter.py',
             ROOT/'tools/parity/build-provider-catalog-oracle.py',
             ROOT/'crates/host-agent/src/catalog/schema.rs', ROOT/'crates/host-agent/src/catalog.rs',
             ROOT/'crates/host-agent/src/gojson.rs', ROOT/'Cargo.lock',
             ROOT/'.parity/go-src/internal/catalog/registry.go']
    paths += sorted((ROOT/'tools/parity/provider-catalog-oracle').iterdir())
    paths += sorted(library.glob('*.go'))
    hashes = {str(p): sha(p) for p in paths}
    lock = json.loads((ROOT/'baseline/source-lock.json').read_text())
    def pinned():
        def git(*args):
            return subprocess.check_output(['git', '-C', str(ROOT/'.parity/go-src'), *args], text=True).strip()
        assert git('rev-parse', 'HEAD') == lock['sourceCommit']
        assert git('rev-parse', 'HEAD^{tree}') == lock['sourceTree']
        assert not git('status', '--porcelain')
    pinned()
    source = cases()
    observations, comparisons = [], []
    for iteration in range(5):
        folder = out/str(iteration)
        folder.mkdir(parents=True, exist_ok=True)
        (folder/'input.json').write_text(json.dumps(source, indent=2)+'\n')
        reference = json.loads(subprocess.check_output([str(go)], input=json.dumps(source), text=True, timeout=60))
        assert len(reference) == len(source)
        (folder/'go.json.gz').write_bytes(gzip.compress(json.dumps(reference).encode(), mtime=0))
        observations.append(reference)
        integer_rows = [r for r in source if r['group'] == 'integer']
        integer_reference = [v for r, v in zip(source, reference) if r['group'] == 'integer']
        inp, target = folder/'integer-input.json', folder/'integer-rust.json'
        inp.write_text(json.dumps(integer_rows)+'\n')
        env = {**os.environ, 'PARITY_SCHEMA_INTEGER_INPUT': str(inp.resolve()),
               'PARITY_SCHEMA_INTEGER_OUTPUT': str(target.resolve())}
        subprocess.run([str(rust), 'catalog::schema::tests::integer_fixture', '--exact', '--ignored'],
                       env=env, capture_output=True, check=True, timeout=60)
        actual = json.loads(target.read_text())
        assert len(actual) == len(integer_reference)
        for case, left, right in zip(integer_rows, integer_reference, actual):
            passed = left == right
            comparisons.append({'case': case['case'], 'iteration': iteration, 'pass': passed})
            if not passed:
                print('FAIL', case['case'], left, right, flush=True)
    unstable = [r['case'] for i, r in enumerate(source)
                if any(v[i] != observations[0][i] for v in observations[1:])]
    assert hashes == {str(p): sha(p) for p in paths}
    pinned()
    report = {
        'scope': 'T0 actual owning Go validateJSONSchema baseline, plus native int32 keyword decoding only. Full Rust schema decoding/resolution and registry wiring remain open. Compiler-only rows are Go observations, not Rust parity passes.',
        'sourceLock': lock, 'provenance': hashes, 'goBinarySha256': sha(go),
        'rustTestBinarySha256': sha(rust), 'oracleCases': len(source), 'oracleIterations': 5,
        'unstableGoCases': unstable, 'rows': comparisons,
        'passed': sum(r['pass'] for r in comparisons), 'total': len(comparisons),
        'normalization': 'none; integer decoded values, nullable pointers and malformed error strings are exact; all full compiler inputs and Go outputs retained',
    }
    (out/'summary.json').write_text(json.dumps(report, indent=2)+'\n')
    (out/'index.json').write_text(json.dumps({str(p.relative_to(out)): sha(p)
        for p in out.rglob('*') if p.is_file() and p.name != 'index.json'}, indent=2)+'\n')
    print('Go compiler cases:', len(source), 'unstable:', len(unstable))
    print('Native integer decoding:', report['passed'], '/', report['total'])
    return all(row['pass'] for row in comparisons)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--go', type=Path, required=True)
    parser.add_argument('--rust-tests', type=Path, required=True)
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    raise SystemExit(0 if run(args.go.resolve(), args.rust_tests.resolve(), args.out.resolve()) else 1)
