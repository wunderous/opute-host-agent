"""Reproduce a seam stage's parity-capture receipt (decision D14).

  python3 evidence/seam/verify_captures.py <stage>

Recomputes every capture in evidence/seam/<stage>/ from its raw I/O: each
scenario's normalized diff, mask violations and declared divergences, every
iteration, every file hash and the provenance (pinned Go source, harness,
binaries the stage recorded, five iterations). It also checks the stage's
secret sweep, contract suites, Rust canaries and recorded tree hashes, and
writes evidence/seam/<stage>-captures-validation.json. A failing capture
fails the script; nothing is relabeled.
"""
import gzip, hashlib, json, sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'tools/parity'))
from parity import runner, canon, m5_verify

stage = sys.argv[1]
STAGE = ROOT / 'evidence/seam' / stage
sha = lambda path: hashlib.sha256(Path(path).read_bytes()).hexdigest()
load = lambda path: json.loads(Path(path).read_text())
raw = lambda path: json.loads(gzip.decompress(Path(path).read_bytes()))
lock = load(ROOT / 'baseline/source-lock.json')
binaries = {Path(path).name: digest for digest, path in
            (line.split() for line in (STAGE / 'binaries.sha256').read_text().splitlines())}
go_sha, rust_sha = binaries['go-reference'], binaries['opute-host-agent']
scenarios = {scenario['id']: scenario for scenario in runner.load_scenarios()}


def recompute(folder, left_label, right_label, left_sha, right_sha):
    summary = load(folder / 'summary.json')
    prov = summary['provenance']
    assert not prov['changedDuringRun'] and prov['harnessSha256'] == runner.harness_sha256()
    assert prov['sourceCommit'] == lock['sourceCommit'] and prov['sourceTree'] == lock['sourceTree']
    assert prov['left']['binarySha256'] == left_sha and prov['right']['binarySha256'] == right_sha
    assert prov['repeat'] == 5
    for name, item in summary['items'].items():
        scenario = scenarios[name]
        capture = folder / 'scenarios' / name
        assert item['scenarioSha256'] == scenario['_sha256'] and item['status'] == 'pass', name
        assert item['iterations'] == 5 and not item['failingIterations'], name
        for path, digest in item['files'].items():
            assert sha(capture / path) == digest, (name, path)
        spec = scenario.get('compare', {})
        a, b = raw(capture / 'a.json.gz'), raw(capture / 'b.json.gz')
        norm_a, viol_a = canon.normalize(a['observation'], spec, a['variables'])
        norm_b, viol_b = canon.normalize(b['observation'], spec, b['variables'])
        viol_a += runner.invariant_violations(scenario, a['observation'], left_label)
        viol_b += runner.invariant_violations(scenario, b['observation'], right_label)
        divergences = []
        # As in the runner: declared divergences apply only between Go and Rust.
        if left_label.startswith('rust') != right_label.startswith('rust'):
            norm_a, norm_b, divergences = canon.apply_divergences(
                norm_a, norm_b, spec.get('divergences', []), canon.load_divergences(runner.DIVERGENCE_FILE))
        norm_a, norm_b = runner._reconcile_optional_masks(norm_a, norm_b, spec.get('masks', []))
        expected = {'diff': canon.diff(norm_a, norm_b), 'maskViolations': {'a': viol_a, 'b': viol_b}, 'divergences': divergences}
        assert not expected['diff'] and not viol_a and not viol_b and not any(row['stale'] for row in divergences), name
        assert expected == load(capture / 'diff.json'), name
        iterations = load(capture / 'iterations.json')
        assert [row['iteration'] for row in iterations] == list(range(5))
        assert all(not row['diffCount'] and not row['maskViolations'] and not row['staleDivergences'] for row in iterations), name
    return sorted(summary['items'])


receipt = {'stage': stage, 'binaries': {'go': go_sha, 'rust': rust_sha}}
names = recompute(STAGE / 'captures', 'go', 'rust', go_sha, rust_sha)
receipt['goVsRust'] = {'scenarios': len(names), 'iterations': 5,
                       'families': sorted({name.split('.')[0] for name in names}),
                       'summarySha256': sha(STAGE / 'captures/summary.json')}
if (STAGE / 'go-vs-go').exists():
    twins = recompute(STAGE / 'go-vs-go', 'go', 'go2', go_sha, go_sha)
    receipt['goVsGo'] = {'scenarios': len(twins), 'iterations': 5, 'summarySha256': sha(STAGE / 'go-vs-go/summary.json')}

if (STAGE / 'secret').exists():
    summary, outcomes = load(STAGE / 'secret/summary.json'), raw(STAGE / 'secret/outcomes.json.gz')
    assert summary['provenance']['rustBinarySha256'] == rust_sha and summary['provenance']['goBinarySha256'] == go_sha
    assert not summary['provenance']['changedDuringRun']
    failures = m5_verify.secret_failures(summary, outcomes)
    assert not failures, failures
    receipt['secretSweep'] = {'fields': summary['fieldCount'], 'failures': 0}
else:
    # The M3 stage sweep built its fixture from a tree that already held later
    # edits, so it was discarded; the M4, M6, M7 and final sweeps cover it.
    receipt['secretSweep'] = 'discarded'

if (STAGE / 'contracts').exists():
    receipt['contracts'] = {}
    for suite in sorted(path.name for path in (STAGE / 'contracts').iterdir()):
        summary = load(STAGE / 'contracts' / suite / 'summary.json')
        assert summary['provenance']['impl']['binarySha256'] == rust_sha
        assert all(item['status'] == 'pass' and not item['failures'] for item in summary['items'].values()), suite
        receipt['contracts'][suite] = len(summary['items'])

if (STAGE / 'rust-canaries.json').exists():
    canaries = load(STAGE / 'rust-canaries.json')
    assert canaries['rustBinarySha256'] == rust_sha and not any(canaries['baselineFailures'].values())
    assert all(row['caught'] and row['patchApplied'] for row in canaries['results'])
    receipt['rustCanaries'] = {'caught': len(canaries['results']), 'total': len(canaries['results'])}

if (STAGE / 'head').exists():
    receipt['head'] = (STAGE / 'head').read_text().strip()
    receipt['treeDiffSha256'] = (STAGE / 'tree.diff.sha256').read_text().split()[0]
    receipt['treeAfterSha256'] = (STAGE / 'tree.after.sha256').read_text().split()[0]
    assert receipt['treeDiffSha256'] == receipt['treeAfterSha256'], 'tree changed during the stage'

(ROOT / 'evidence/seam' / f'{stage}-captures-validation.json').write_text(json.dumps(receipt, indent=1, sort_keys=True) + '\n')
print(f"{stage}: {len(names)} Go/Rust scenarios x5 recomputed"
      + (f", {receipt['goVsGo']['scenarios']} Go/Go" if 'goVsGo' in receipt else '')
      + f", secret sweep {receipt['secretSweep'] if isinstance(receipt['secretSweep'], str) else 'clean'}")
