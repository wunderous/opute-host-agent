#!/usr/bin/env python3
"""Compare assembled native compiler with owning catalog validateJSONSchema."""
import argparse
import gzip
import json
import os
from pathlib import Path
import subprocess
from provider_adapter import ROOT, freeze_binary, sha
from provider_schema_decoder import cases as decode_cases, equal
from provider_schema_graph import cases as graph_cases
from provider_schema_regexp import grammar
from generate_regexp_names import source, data_source


def cases(facts):
    rows = [{'case': 'decode-'+r['case'], 'schema': r['schema']} for r in decode_cases()]
    # The owning registry passes Resolve(nil): no externally supplied base URI.
    rows += [{'case': 'graph-'+r['case'], 'schema': r['schema'],
              'variants': r.get('variants', False)} for r in graph_cases()
             if r['resolverAction'] == 'graph' and not r['baseURI']]
    for r in grammar(facts):
        for field in ['pattern', 'patternProperties']:
            rows.append({'case': field+'-'+r['case'], 'schema': {
                field: r['regexp'] if field == 'pattern' else {r['regexp']: {}}}})
    assert len({r['case'] for r in rows}) == len(rows)
    return rows


def run(go, rust, out):
    out.mkdir(parents=True, exist_ok=True)
    generator = ROOT/'tools/parity/go-regexp-data/main.go'
    facts = json.loads(subprocess.check_output(
        [str(ROOT/'.parity/setup/go/bin/go'), 'run', str(generator)],
        env={**os.environ, 'GOTOOLCHAIN': 'go1.25.4'}, text=True, timeout=60))
    assert source(facts) == (ROOT/'crates/host-agent/src/catalog/schema/regexp_names.rs').read_text()
    assert data_source(facts) == (ROOT/'crates/host-agent/src/catalog/schema/regexp_data.json').read_text()
    go = freeze_binary(go, 'schema-compiler-go')
    rust = freeze_binary(rust, 'schema-compiler-rust')
    library = Path('/home/houman/go/pkg/mod/github.com/google/jsonschema-go@v0.4.3/jsonschema')
    paths = [Path(__file__), generator, ROOT/'Cargo.lock', ROOT/'baseline/source-lock.json',
             ROOT/'crates/host-agent/src/catalog/schema.rs', ROOT/'crates/host-agent/src/gojson.rs',
             ROOT/'.parity/go-src/internal/catalog/registry.go']
    paths += [ROOT/'tools/parity'/name for name in [
        'provider_adapter.py', 'provider_schema.py', 'provider_schema_decoder.py',
        'provider_schema_graph.py', 'provider_schema_local_uri.py', 'provider_schema_regexp.py',
        'generate_regexp_names.py', 'build-provider-catalog-oracle.py']]
    paths += sorted((ROOT/'crates/host-agent/src/catalog/schema').iterdir())
    paths += sorted((ROOT/'tools/parity/provider-catalog-oracle').iterdir())
    paths += sorted(library.glob('*.go'))
    for package in ['regexp', 'regexp/syntax', 'unicode']:
        paths += sorted((ROOT/'.parity/setup/go/src'/package).glob('*.go'))
    hashes = {str(p): sha(p) for p in paths if p.is_file()}
    lock = json.loads((ROOT/'baseline/source-lock.json').read_text())

    def pinned():
        def git(*args):
            return subprocess.check_output(['git', '-C', str(ROOT/'.parity/go-src'), *args], text=True).strip()
        assert git('rev-parse', 'HEAD') == lock['sourceCommit']
        assert git('rev-parse', 'HEAD^{tree}') == lock['sourceTree']
        assert not git('status', '--porcelain')

    pinned()
    rows = cases(facts)
    comparisons = []
    ambiguous = [r for r in rows if r.get('variants')]
    variants = {r['case']: [] for r in ambiguous}
    samples = []
    for _ in range(128 if ambiguous else 0):
        observed = json.loads(subprocess.check_output([str(go)], input=json.dumps(ambiguous), text=True, timeout=60))
        samples.append(observed)
        for row, value in zip(ambiguous, observed):
            if not any(equal(value, prior) for prior in variants[row['case']]):
                variants[row['case']].append(value)
    (out/'map-order-reference.json.gz').write_bytes(gzip.compress(json.dumps({
        'input': ambiguous, 'samples': samples}).encode(), mtime=0))
    encoded = json.dumps(rows)
    for iteration in range(5):
        folder = out/str(iteration)
        folder.mkdir(parents=True, exist_ok=True)
        inp, target = folder/'input.json', folder/'rust.json'
        inp.write_text(encoded+'\n')
        reference = json.loads(subprocess.check_output([str(go)], input=encoded, text=True, timeout=60))
        subprocess.run([str(rust), 'catalog::schema::tests::compiler_fixture', '--exact', '--ignored'],
                       env={**os.environ, 'PARITY_SCHEMA_COMPILER_INPUT': str(inp.resolve()),
                            'PARITY_SCHEMA_COMPILER_OUTPUT': str(target.resolve())},
                       capture_output=True, check=True, timeout=60)
        actual = json.loads(target.read_text())
        assert len(rows) == len(reference) == len(actual)
        (folder/'input.json.gz').write_bytes(gzip.compress(inp.read_bytes(), mtime=0))
        inp.unlink()
        target.unlink()
        for side, data in [('go', reference), ('rust', actual)]:
            (folder/(side+'.json.gz')).write_bytes(gzip.compress(json.dumps(data).encode(), mtime=0))
        for row, left, right in zip(rows, reference, actual):
            choices = [left]+variants.get(row['case'], [])
            passed = any(equal(right, v) for v in choices)
            comparisons.append({'case': row['case'], 'iteration': iteration, 'pass': passed})
            if not passed:
                print('FAIL', row['case'], repr(left['error']), repr(right['error']), flush=True)
    assert hashes == {path: sha(Path(path)) for path in hashes}
    pinned()
    report = {
        'scope': 'T0 private assembled typed schema decoder, local checks, URI/anchor/reference graph and native regexp grammar versus actual owning catalog validateJSONSchema. Scoped assembly comparisons; regexp compiler-resource cache gap and production registry wiring remain open. No schema-instance validation or provider activation claim.',
        'sourceLock': lock, 'provenance': hashes, 'goBinarySha256': sha(go),
        'rustTestBinarySha256': sha(rust), 'cases': len(rows), 'iterations': 5,
        'passed': sum(r['pass'] for r in comparisons), 'total': len(comparisons),
        'failed': sum(not r['pass'] for r in comparisons), 'rows': comparisons,
        'normalization': 'Complete exact error or empty success. Three flagged multi-fault structural-map cases may match an entire observed owning Go outcome from 128 reference samples. No substring/error rewrite, forced acceptance, resource waiver or Go runtime fallback.'}
    (out/'summary.json').write_text(json.dumps(report, indent=2)+'\n')
    (out/'index.json').write_text(json.dumps({str(p.relative_to(out)): sha(p)
        for p in out.rglob('*') if p.is_file() and p.name != 'index.json'}, indent=2)+'\n')
    print('Assembled compiler:', report['passed'], '/', report['total'], 'failed:', report['failed'])
    return report['failed'] == 0


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--go', type=Path, required=True)
    parser.add_argument('--rust-tests', type=Path, required=True)
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    raise SystemExit(0 if run(args.go.resolve(), args.rust_tests.resolve(), args.out.resolve()) else 1)
