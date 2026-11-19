#!/usr/bin/env python3
"""List the journey steps whose outcome no assertion decides (m22).

  python3 evidence/e2e/outcomes.py           # every journey, as JSON
  python3 evidence/e2e/outcomes.py --check   # exit 1 if any step is flagged

The rule is run.py's: a `tools/call` or `tasks/get` step must assert its
result (`isError`, content or `structuredContent`), a JSON-RPC error, a refused
HTTP status, or a task status other than `completed`. A task launch is judged
at its poll; a step may opt out only with `expectAnyOutcome` and its reason.
"""
import argparse
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import run  # noqa: E402


def main() -> int:
    p = argparse.ArgumentParser()
    p.add_argument("--check", action="store_true")
    args = p.parse_args()
    flagged = {}
    for path in sorted(run.JOURNEYS.glob("*.json")):
        steps = run.unchecked_outcome_steps(json.loads(path.read_text()))
        if steps:
            flagged[path.stem] = steps
    print(json.dumps({"journeys": len(list(run.JOURNEYS.glob("*.json"))),
                      "flaggedSteps": sum(map(len, flagged.values())), "flagged": flagged}, indent=1))
    return 1 if args.check and flagged else 0


if __name__ == "__main__":
    sys.exit(main())
