"""Capture the open unadmitted-callback-task divergence, Go vs Rust.

The case lives outside the public parity suite
(tools/parity/open-divergences/unadmitted-callback-task.json) because its
difference awaits an owner decision
(openspec/changes/unadmitted-callback-task-record). This driver runs it
through the ordinary runner five times; the bundle is expected to fail and
verify.py pins exactly why.
"""
import json, sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT / 'tools/parity'))
from parity import agent, canon, runner

source = ROOT / 'tools/parity/open-divergences/unadmitted-callback-task.json'
scenarios = json.loads(source.read_text())
for scenario in scenarios:
    scenario['_file'] = str(source.relative_to(ROOT))
    scenario['_sha256'] = runner.sha256_bytes(canon.canonical_json(
        {k: v for k, v in scenario.items() if not k.startswith('_')}).encode())
lock = json.loads((ROOT / 'baseline/source-lock.json').read_text())
runner.run_suite(agent.Impl('go', (ROOT / '.parity/bin/go-reference').resolve()),
                 agent.Impl('rust', (ROOT / 'target/release/opute-host-agent').resolve()),
                 'open-divergences', Path(__file__).resolve().parent / 'capture', scenarios, 5, 1, lock)
