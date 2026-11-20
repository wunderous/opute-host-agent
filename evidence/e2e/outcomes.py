#!/usr/bin/env python3
"""List the journey steps the m22 and m23 rules flag.

  python3 evidence/e2e/outcomes.py           # every journey, as JSON
  python3 evidence/e2e/outcomes.py --check   # exit 1 if any step is flagged

m22 (unchecked outcomes), run.py's rule: a `tools/call` or `tasks/get` step
must assert its result (`isError`, content or `structuredContent`), a
JSON-RPC error, a refused HTTP status, or a task status other than
`completed`. A task launch is judged at its poll; a step may opt out only
with `expectAnyOutcome` and its reason.

m23 (unconfirmed effects): a step whose tool publishes a mutating effect and
which the journey expects to succeed must be named in `confirms` by a later
guest command, HTTP request or `read` tool call that carries assertions; a
step may opt out only with `effectUnobservable` and its reason. Effects come
from the tools/list records of the gates' committed runs.

m24 (effect allowances): every `allow` pattern must match a path the run
changed and every `persist` pattern a path its cleanup left, unless it is in
`effects.optional` with its reason; no pattern may be broad. Unused patterns
are judged on the gates' committed states; a journey with no committed run
is listed under `noRun`.
"""
import argparse
import gzip
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import run  # noqa: E402

E2E = Path(__file__).resolve().parent


def committed_effects() -> dict[str, str]:
    """Every tool's published effect, from the tools/list records of the gates' committed runs."""
    gates = json.loads((E2E / "gates.json").read_text())
    effects: dict[str, str] = {}
    for spec in gates.values():
        for journey_id in spec.get("journeys") or {}:
            transcript = E2E / "runs" / str(spec.get("run")) / journey_id / "transcript.jsonl"
            if transcript.exists():
                records = [json.loads(line) for line in transcript.read_text().splitlines()]
                for tool, effect in run.tool_effects(records).items():
                    effects.setdefault(tool, effect)
    return effects


def negative_controls(failure: str) -> set[str]:
    """The self-checks a gate expects to fail with this failure: they are flagged on purpose."""
    gates = json.loads((E2E / "gates.json").read_text())
    return {j for spec in gates.values() for j, want in (spec.get("journeys") or {}).items()
            if failure in (want.get("failures") or [])}


def committed_states() -> dict[str, dict]:
    """Each gate journey's before, after and cleanup states, from its committed run."""
    gates = json.loads((E2E / "gates.json").read_text())
    states = {}
    for spec in gates.values():
        for journey_id in spec.get("journeys") or {}:
            run_dir = E2E / "runs" / str(spec.get("run")) / journey_id
            if (run_dir / "state-before.json.gz").exists():
                states[journey_id] = {n: json.loads(gzip.decompress((run_dir / f"state-{n}.json.gz").read_bytes()))
                                      for n in ("before", "after", "cleanup")}
    return states


def main() -> int:
    p = argparse.ArgumentParser()
    p.add_argument("--check", action="store_true")
    args = p.parse_args()
    effects = committed_effects()
    journeys = sorted(run.JOURNEYS.glob("*.json"))
    outcomes, unconfirmed, allowances, no_run = {}, {}, {}, []
    states = committed_states()
    allowance_controls = negative_controls("unused-allowance") | negative_controls("broad-allowance")
    controls = negative_controls("unchecked-outcome") | negative_controls("unconfirmed-effect")
    for path in journeys:
        if path.stem in controls:
            continue
        journey = json.loads(path.read_text())
        if steps := run.unchecked_outcome_steps(journey):
            outcomes[path.stem] = steps
        if steps := run.unconfirmed_effect_steps(journey, effects):
            unconfirmed[path.stem] = steps
        if path.stem in allowance_controls:
            continue
        if path.stem in states:
            st = states[path.stem]
            found = run.allowance_problems(journey, run.state_diff(st["before"], st["after"]),
                                           run.state_diff(st["before"], st["cleanup"]))
        else:
            no_run.append(path.stem)
            optional = journey["effects"].get("optional") or {}
            found = {"broad": [p for p in journey["effects"]["allow"]
                               if p not in optional and run.broad_pattern(p)]}
        if found := {k: v for k, v in found.items() if v}:
            allowances[path.stem] = found
    print(json.dumps({"journeys": len(journeys), "toolsWithEffects": len(effects),
                      "flaggedSteps": sum(map(len, outcomes.values())), "flagged": outcomes,
                      "unconfirmedEffectSteps": sum(map(len, unconfirmed.values())),
                      "unconfirmedEffects": unconfirmed,
                      "allowancePatterns": sum(len(v) for a in allowances.values() for v in a.values()),
                      "allowances": allowances, "noRun": no_run}, indent=1))
    return 1 if args.check and (outcomes or unconfirmed or allowances) else 0


if __name__ == "__main__":
    sys.exit(main())
