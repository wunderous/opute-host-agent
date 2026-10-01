"""Command line: python3 -m parity <run|verify|capture> ..."""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

from . import agent, runner


def _impl(spec: str) -> agent.Impl:
    label, _, path = spec.partition("=")
    if not path:
        raise SystemExit(f"expected LABEL=PATH, got {spec!r}")
    return agent.Impl(label=label, binary=Path(path).resolve())


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="parity")
    sub = parser.add_subparsers(dest="command", required=True)

    run = sub.add_parser("run", help="run scenarios against two implementations")
    run.add_argument("--left", required=True, help="LABEL=PATH")
    run.add_argument("--right", required=True, help="LABEL=PATH")
    run.add_argument("--suite", required=True)
    run.add_argument("--out", required=True, type=Path)
    run.add_argument("--repeat", type=int, default=1)
    run.add_argument("--workers", type=int, default=6)
    run.add_argument("--surfaces", default="", help="comma-separated surfaces to run (default: all)")
    run.add_argument("scenarios", nargs="*")

    verify = sub.add_parser("verify", help="fail-closed parity manifest verifier")
    verify.add_argument("--manifest", type=Path, default=runner.REPO_ROOT / "parity-manifest.json")
    verify.add_argument("--gate", required=True)
    verify.add_argument("--report", type=Path)

    cap = sub.add_parser("capture", help="capture the Go inventory into baseline/inventory")
    cap.add_argument("--go", required=True, type=Path, help="Go reference binary")
    cap.add_argument("--go-src", required=True, type=Path, help="pinned Go source checkout")

    can = sub.add_parser("canaries", help="build mutation canaries and prove the harness catches them")
    can.add_argument("--go", required=True, type=Path)
    can.add_argument("--go-src", required=True, type=Path)
    can.add_argument("--out", required=True, type=Path)
    can.add_argument("--workers", type=int, default=6)

    orc = sub.add_parser("oracle", help="run the Go baseline's black-box tests against binaries")
    orc.add_argument("--go-src", required=True, type=Path)
    orc.add_argument("--binary", action="append", required=True, help="LABEL=PATH (repeatable)")
    orc.add_argument("--out", required=True, type=Path)

    sub.add_parser("corpus", help="regenerate scenarios/wire.json from parity/corpus.py")
    man = sub.add_parser("manifest", help="sync derived manifest fields from scenarios and the source lock")
    man.add_argument("--go", type=Path, help="Go reference binary whose hash to record")
    man.add_argument("--rust", type=Path, help="Rust candidate binary whose hash to record")

    args = parser.parse_args(argv)
    if args.command == "corpus":
        from . import corpus
        print(f"wire corpus v{corpus.VERSION}: {corpus.write()} steps -> {corpus.OUT_FILE.name}")
        return 0
    if args.command == "oracle":
        from . import oracle
        binaries = {i.label: i.binary for i in map(_impl, args.binary)}
        doc = oracle.run(args.go_src.resolve(), binaries, args.out)
        ok = all((r["exit"] == 0 and set(r["expectedTests"]) <= set(r["passed"]))
                 if r["binary"] != oracle.NEGATIVE_CONTROL[0] else r["exit"] != 0
                 for r in doc["results"])
        return 0 if ok else 1
    if args.command == "manifest":
        from . import manifest
        sha = agent.Impl("go", args.go.resolve()).sha256() if args.go else None
        rust = agent.Impl("rust", args.rust.resolve()).sha256() if args.rust else None
        doc = manifest.sync(sha, rust)
        print(f"manifest: {len(doc['items'])} items")
        return 0
    if args.command == "canaries":
        from . import canaries
        doc = canaries.run(args.go.resolve(), args.go_src.resolve(), args.out, args.workers)
        return 0 if all(r.get("caught") for r in doc["results"]) else 1
    if args.command == "capture":
        from . import capture
        index = capture.write(capture.capture(args.go.resolve(), args.go_src.resolve()))
        gaps = json.loads((capture.INVENTORY_DIR / "gaps.json").read_text())
        print(f"wrote {len(index['files'])} inventory files; {gaps['count']} explicit gaps, "
              f"{gaps['unownedCount']} unowned")
        return 1 if gaps["unownedCount"] else 0
    if args.command == "run":
        lock = json.loads((runner.REPO_ROOT / "baseline" / "source-lock.json").read_text())
        scenarios = runner.load_scenarios(args.scenarios or None)
        if args.surfaces:
            wanted = {s.strip() for s in args.surfaces.split(",") if s.strip()}
            scenarios = [s for s in scenarios if s["surface"] in wanted]
        summary = runner.run_suite(
            _impl(args.left), _impl(args.right), args.suite, args.out,
            scenarios, args.repeat, args.workers, lock)
        failed = [k for k, v in summary["items"].items() if v["status"] != "pass"]
        for key, item in sorted(summary["items"].items()):
            print(f"{item['status']:4}  {key}  ({item['iterations']} iterations)")
        print(f"{len(summary['items']) - len(failed)}/{len(summary['items'])} scenarios pass")
        return 1 if failed else 0
    if args.command == "verify":
        from . import verify as verifier
        report = verifier.verify(args.manifest, args.gate)
        text = json.dumps(report, indent=2, sort_keys=True)
        if args.report:
            args.report.write_text(text + "\n")
        print(verifier.render(report))
        return 0 if report["gate"]["pass"] else 1
    return 2


if __name__ == "__main__":
    sys.exit(main())
