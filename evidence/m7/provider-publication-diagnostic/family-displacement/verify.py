"""Independently inspect the captured pinned Go displacement failure."""
import gzip
import hashlib
import json
from pathlib import Path

FOLDER = Path(__file__).resolve().parent
ROOT = FOLDER.parents[3]
sha = lambda path: hashlib.sha256(path.read_bytes()).hexdigest()
receipt = json.loads((FOLDER / 'receipt.json').read_text())
assert receipt['sourceLock'] == json.loads((ROOT / 'baseline/source-lock.json').read_text())
for path, digest in receipt['captures'].items():
    assert sha(FOLDER / path) == digest, path
for path, digest in receipt['producerSources'].items():
    assert sha(ROOT / path) == digest, path
assert sha(ROOT / '.parity/bin/provider-catalog-oracle') == receipt['goBinarySha256']
raw = lambda name: json.loads(gzip.decompress((FOLDER / (name + '.json.gz')).read_bytes()))
inputs, go = raw('input'), raw('go')
index = receipt['batchIndex']
assert inputs[index]['case'] == receipt['case']
steps = go[index]['observations']
names = lambda step, projection: {
    tool.get('operationId', tool.get('name'))
    for tool in (step[projection]['tools'] if isinstance(step[projection], dict) else step[projection])
}
assert 'test.probe' in names(steps[0], 'registry')
assert steps[1]['error'] == 'capability "other.probe" input schema must declare type object'
assert 'test.probe' not in names(steps[1], 'registry')
for projection in ['published', 'wire']:
    assert 'test.probe' in names(steps[1], projection)
    assert 'test.probe' in names(steps[2], projection)
assert not steps[3]['error']
for projection in ['registry', 'published', 'wire']:
    assert 'test.probe' not in names(steps[3], projection)
    assert 'third.probe' in names(steps[3], projection)
print('Pinned Go registry displacement, stale publication, and later MCP removal verified')
