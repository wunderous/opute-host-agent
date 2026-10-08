#!/usr/bin/env python3
"""Compare native typed schema decoding with the actual pinned Go Schema.UnmarshalJSON."""
import argparse
import copy
import gzip
import json
import os
from pathlib import Path
import random
import subprocess
from provider_adapter import ROOT, freeze_binary, sha
from provider_schema import cases as compiler_cases


def equal(left, right):
    # Python treats True == 1 and False == 0; those are different JSON kinds.
    if isinstance(left, bool) or isinstance(right, bool):
        return type(left) is type(right) and left == right
    if isinstance(left, (int, float)) and isinstance(right, (int, float)):
        return left == right
    if type(left) is not type(right):
        return False
    if isinstance(left, dict):
        return left.keys() == right.keys() and all(equal(v, right[k]) for k, v in left.items())
    if isinstance(left, list):
        return len(left) == len(right) and all(equal(a, b) for a, b in zip(left, right))
    return left == right


def cases():
    rows = compiler_cases()
    def add(name, value):
        rows.append({'case': name, 'group': 'decoder', 'schema': value})
    for field, values in {
        'title': [('old', None), ('old', 'new')],
        'minimum': [(5, None), (None, 5)],
        'minItems': [(5, None), (None, 5), (5, 6)],
        'type': [('object', ['string']), (['string'], 'object'), ('object', ''), (None, 'object')],
        'items': [({}, []), ([], {}), ({}, None), (None, {})],
        'properties': [({'a': {'title': 'old'}}, {'a': {'description': 'new'}}),
                       ({'a': {}}, {'b': {}}), ({'a': {}}, None)],
        'not': [({'title': 'old', 'minItems': 5}, {'description': 'new', 'minItems': None}),
                ({'title': 'old', 'minimum': 5}, {'minimum': None}),
                ({'unknown': 'old'}, {}), ({'title': 'old'}, True),
                ({'title': 'old'}, False), ({'title': 'old'}, None)],
        'allOf': [([{'title': 'old'}], [{}]), ([{'title': 'old'}], []),
                  ([{'title': 'old'}], [True]), ([{'title': 'old'}], [None])],
        'required': [(['old'], [None]), (['old'], []), (['old'], None)],
        'dependentRequired': [({'a': ['old']}, {'a': [None]}), ({'a': ['old']}, {'b': []})],
        'dependencies': [({'a': {}}, {'b': []}), ({'a': {}}, {'a': []}), ({'a': []}, {'a': {}})],
        'default': [(None, False), (False, None)],
        'const': [(None, False), (False, None)],
    }.items():
        for index, (first, second) in enumerate(values):
            add(f'merge-{field}-{index}', {field.upper(): first, field: second})
    for field in ['required', 'allOf', 'prefixItems']:
        old = ['one', 'two'] if field == 'required' else [{'title': 'one'}, {'title': 'two'}]
        last = [None, None] if field == 'required' else [{}, {}]
        for empty in [True, False]:
            add(f'backing-{field}-{empty}', {field.upper(): old,
                field[:1].upper()+field[1:]: [] if empty else old[:1], field: last})
    for schema in [
        {'allOf': [{'title': False}], 'minItems': 1.5},
        {'allOf': [{'title': False}], 'description': 42},
        {'description': 42, 'not': {'title': False}},
        {'TITLE': False, 'title': 'fixed'},
        {'dependencies': 42, 'title': False},
        {'dependentRequired': {'a': [42]}, 'minItems': 1.5},
        {'NOT': {'minItems': 5, 'type': 'string'}, 'not': {'minItems': None}},
        {'PROPERTIES': {'a': {'title': 'old'}}, 'properties': {'a': {}}},
        {'CONST': None, 'const': {'nested': [False, None, 1e20]}},
        {'properties': {'a': {'default': None, 'const': None}, 'b': True, 'c': False}},
        {'NOT': {'type': 'object'}, 'not': {'type': ['string']}},
        {'NOT': {'type': ['string']}, 'not': {'type': 'object'}},
        {'NOT': {'items': {}}, 'not': {'items': []}},
        {'NOT': {'items': []}, 'not': {'items': {}}},
        {'NOT': {'dependencies': {'a': {}}}, 'not': {'dependencies': {'a': []}}},
        {'NOT': {'$defs': {}}, 'not': {'definitions': {}}},
    ]:
        add('context-'+str(len(rows)), schema)
    for index, value in enumerate([-0.0, 1.0, 1e20, 1e-7, '<&>\u2028\u2029雪',
                                    {'value': [-0.0, 1.0, '<&>']}, '\\u003c']):
        add('raw-default-'+str(index), {'default': value, 'const': value})
    # Seeded combinations vary standard-error vs custom-decoder priority,
    # alias reuse, nested schemas and generic metadata without a second schema
    # implementation in the producer. Dependencies in the base corpus have
    # at most one invalid map entry, avoiding ambiguous Go map first errors.
    base = copy.deepcopy(rows)
    rng = random.Random(0xACE7013)
    for index in range(2048):
        value = {}
        for _ in range(rng.randrange(2, 7)):
            sample = copy.deepcopy(rng.choice(base)['schema'])
            if rng.randrange(3) == 0:
                sample = {k.upper(): v for k, v in sample.items()}
            value.update(sample)
        if index % 4 == 0:
            value = {rng.choice(['not', 'additionalProperties', 'items']): value}
        elif index % 4 == 1:
            value = {'properties': {'a/b~c': value}}
        add('seeded-'+str(index), value)
    assert len({r['case'] for r in rows}) == len(rows)
    return [{**r, 'decodeOnly': True} for r in rows]


def run(go, rust, out):
    assert not equal({'nested': [True]}, {'nested': [1]})
    assert not equal({'nested': [False]}, {'nested': [0]})
    assert not equal([], None)
    out.mkdir(parents=True, exist_ok=True)
    go = freeze_binary(go, 'schema-decoder-go')
    rust = freeze_binary(rust, 'schema-decoder-rust')
    library = Path('/home/houman/go/pkg/mod/github.com/google/jsonschema-go@v0.4.3/jsonschema')
    paths = [Path(__file__), ROOT/'tools/parity/provider_schema.py', ROOT/'tools/parity/provider_adapter.py',
             ROOT/'tools/parity/build-provider-catalog-oracle.py', ROOT/'crates/host-agent/src/catalog/schema.rs',
             ROOT/'crates/host-agent/src/catalog/schema-LICENSE.txt', library.parent/'LICENSE',
             ROOT/'crates/host-agent/src/catalog.rs', ROOT/'crates/host-agent/src/gojson.rs',
             ROOT/'crates/host-agent/src/goerr.rs', ROOT/'crates/host-agent/src/goerr/printable.rs',
             ROOT/'Cargo.lock']
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
        inp, target = folder/'input.json', folder/'rust.json'
        inp.write_text(json.dumps(source, indent=2)+'\n')
        reference = json.loads(subprocess.check_output([str(go)], input=json.dumps(source), text=True, timeout=60))
        env = {**os.environ, 'PARITY_SCHEMA_DECODER_INPUT': str(inp.resolve()),
               'PARITY_SCHEMA_DECODER_OUTPUT': str(target.resolve())}
        subprocess.run([str(rust), 'catalog::schema::tests::decoder_fixture', '--exact', '--ignored'],
                       env=env, capture_output=True, check=True, timeout=60)
        actual = json.loads(target.read_text())
        target.unlink()
        assert len(reference) == len(actual) == len(source)
        for side, data in [('go', reference), ('rust', actual)]:
            (folder/(side+'.json.gz')).write_bytes(gzip.compress(json.dumps(data).encode(), mtime=0))
        observations.append(reference)
        for case, left, right in zip(source, reference, actual):
            passed = equal(left, right)
            comparisons.append({'case': case['case'], 'iteration': iteration, 'pass': passed})
            if not passed and iteration == 0:
                print('FAIL', case['case'], left['error'], right['error'],
                      [k for k in (left['schema'] or {}) if (left['schema'] or {}).get(k) != (right['schema'] or {}).get(k)], flush=True)
    unstable = [r['case'] for i, r in enumerate(source)
                if any(not equal(v[i], observations[0][i]) for v in observations[1:])]
    assert hashes == {str(p): sha(p) for p in paths}
    pinned()
    report = {
        'scope': 'T0 actual pinned Schema.UnmarshalJSON vs native typed schema tree decoding. All reflected exported fields, nested children, nil/empty collections, exact float64 bits, default raw bytes, raw/const presence and malformed errors compared. Schema.Resolve, regexp compilation, URI/reference resolution and registry wiring remain open.',
        'sourceLock': lock, 'provenance': hashes, 'goBinarySha256': sha(go),
        'rustTestBinarySha256': sha(rust), 'cases': len(source), 'iterations': 5,
        'unstableGoCases': unstable, 'rows': comparisons,
        'passed': sum(r['pass'] for r in comparisons), 'total': len(comparisons),
        'normalization': 'No error normalization. The Go oracle reflects its actual typed tree, preserving exported fields, float64 bits, default raw bytes, nil/empty and raw/const presence; numeric JSON spellings in the projection are compared by value, with Booleans kept distinct. This is a private decoder projection, not product wire serialization.',
    }
    (out/'summary.json').write_text(json.dumps(report, indent=2)+'\n')
    (out/'index.json').write_text(json.dumps({str(p.relative_to(out)): sha(p)
        for p in out.rglob('*') if p.is_file() and p.name != 'index.json'}, indent=2)+'\n')
    print('Native schema decoder:', report['passed'], '/', report['total'], 'unstable Go cases:', len(unstable))
    return all(row['pass'] for row in comparisons) and not unstable


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--go', type=Path, required=True)
    parser.add_argument('--rust-tests', type=Path, required=True)
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    raise SystemExit(0 if run(args.go.resolve(), args.rust_tests.resolve(), args.out.resolve()) else 1)
