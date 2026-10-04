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
    can.add_argument("--manifest", type=Path, default=runner.REPO_ROOT / "parity-manifest.json",
                      help="used only to exempt canaries any gate has waived from this command's own exit code")

    orc = sub.add_parser("oracle", help="run the Go baseline's black-box tests against binaries")
    orc.add_argument("--go-src", required=True, type=Path)
    orc.add_argument("--binary", action="append", required=True, help="LABEL=PATH (repeatable)")
    orc.add_argument("--out", required=True, type=Path)

    con = sub.add_parser("contract", help="run a single-implementation contract suite")
    con.add_argument("--impl", required=True, help="LABEL=PATH")
    con.add_argument("--suite", required=True)
    con.add_argument("--out", type=Path)
    con.add_argument("--workers", type=int, default=6)
    con.add_argument("scenarios", nargs="*")

    rc = sub.add_parser("rust-canaries", help="patched Rust builds must turn their contract scenario red")
    rc.add_argument("--rust", required=True, type=Path, help="unpatched Rust candidate binary")
    rc.add_argument("--out", required=True, type=Path)
    rc.add_argument("--workers", type=int, default=6)

    cs = sub.add_parser("catalog-source", help="regenerate crates/host-agent/catalog/source.json from the pinned Go tree")
    cs.add_argument("--go-src", required=True, type=Path)
    cs.add_argument("--check", action="store_true", help="fail if the committed file is stale")

    cr = sub.add_parser("crash", help="legacy mid-shim diagnostic; does not establish M5 crash acceptance")
    cr.add_argument("--go", required=True, type=Path)
    cr.add_argument("--rust", required=True, type=Path)
    cr.add_argument("--seeds", type=int, default=5)
    cr.add_argument("--mode", default="mid-shim")
    cr.add_argument("--out", required=True, type=Path)

    sc = sub.add_parser("storage-crash", help="M5: SIGKILL at actual SQLite operation, task and plan write checkpoints")
    sc.add_argument("--go", required=True, type=Path)
    sc.add_argument("--rust", required=True, type=Path)
    sc.add_argument("--seeds", type=int, default=200)
    sc.add_argument("--workers", type=int, default=4)
    sc.add_argument("--out", required=True, type=Path)

    ss = sub.add_parser("secret-sweep", help="M5: catalog-derived secrets across durable storage and supported MCP task responses")
    ss.add_argument("--go", required=True, type=Path)
    ss.add_argument("--rust", required=True, type=Path)
    ss.add_argument("--out", required=True, type=Path)

    sp = sub.add_parser("shape", help="M5: compare full database schemas and rows across the wire matrix")
    sp.add_argument("--go", required=True, type=Path)
    sp.add_argument("--rust", required=True, type=Path)
    sp.add_argument("--out", required=True, type=Path)
    sp.add_argument("--repeat", type=int, default=5)
    sp.add_argument("--workers", type=int, default=6)
    sp.add_argument("scenarios", nargs="*")

    mg = sub.add_parser("migration", help="M5: released and pre-column-addition state.db upgrades")
    mg.add_argument("--go", required=True, type=Path)
    mg.add_argument("--rust", required=True, type=Path)
    mg.add_argument("--repeats", type=int, default=3)
    mg.add_argument("--out", required=True, type=Path)

    xr = sub.add_parser("cross-read", help="M5: one side writes, the other starts cold on a copy and reads it back")
    xr.add_argument("--go", required=True, type=Path)
    xr.add_argument("--rust", required=True, type=Path)
    xr.add_argument("--out", required=True, type=Path)

    up = sub.add_parser("unknown-projection", help="M5: unmarked/open-schema fields must fail closed (D13)")
    up.add_argument("--go", required=True, type=Path)
    up.add_argument("--rust", required=True, type=Path)
    up.add_argument("--out", required=True, type=Path)

    sub.add_parser("corpus", help="regenerate scenarios/wire.json from parity/corpus.py")
    man = sub.add_parser("manifest", help="sync derived manifest fields from scenarios and the source lock")
    man.add_argument("--go", type=Path, help="Go reference binary whose hash to record")
    man.add_argument("--rust", type=Path, help="Rust candidate binary whose hash to record")

    args = parser.parse_args(argv)
    if args.command == "contract":
        from . import contract
        summary = contract.run_suite(_impl(args.impl), args.suite, args.out, args.scenarios or None, args.workers)
        failed = [k for k, v in summary["items"].items() if v["status"] != "pass"]
        for key, item in sorted(summary["items"].items()):
            print(f"{item['status']:4}  {key}")
            for failure in item["failures"]:
                print(f"        {failure}")
        print(f"{len(summary['items']) - len(failed)}/{len(summary['items'])} contract scenarios pass")
        return 1 if failed else 0
    if args.command == "rust-canaries":
        from . import contract
        doc = contract.run_rust_canaries(args.rust.resolve(), args.out, args.workers)
        clean = not any(doc["baselineFailures"].values())
        return 0 if clean and all(r.get("caught") for r in doc["results"]) else 1
    if args.command == "catalog-source":
        from . import catalog_source
        if args.check:
            fresh = catalog_source.check(args.go_src.resolve())
            print("catalog source is " + ("current" if fresh else "STALE: run make catalog-source"))
            return 0 if fresh else 1
        print(f"wrote {catalog_source.write(args.go_src.resolve())}")
        return 0
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
        manifest = json.loads(args.manifest.read_text()) if args.manifest.exists() else {}
        waived = {c for gate in manifest.get("gates", {}).values() for c in gate.get("waivedCanaries", {})}
        return 0 if all(r.get("caught") or r["id"] in waived for r in doc["results"]) else 1
    if args.command == "crash":
        from . import crash
        summary = crash.run(agent.Impl("go", args.go.resolve()),
                             agent.Impl("rust", args.rust.resolve()),
                             args.seeds, args.out, mode=args.mode)
        print(f"crash injection: {summary['passed']}/{summary['seeds']} seeds recovered identically")
        return 0 if summary["failed"] == 0 else 1
    if args.command == "migration":
        from . import migration
        summary = migration.run(agent.Impl("go", args.go.resolve()),
                                  agent.Impl("rust", args.rust.resolve()),
                                  args.out, repeats=args.repeats)
        print(f"older-state migration: {summary['passed']}/{summary['total']} runs identical")
        return 0 if summary["failed"] == 0 else 1
    if args.command == "storage-crash":
        from . import storage_crash
        summary = storage_crash.run(agent.Impl("go", args.go.resolve()),
                                    agent.Impl("rust", args.rust.resolve()), args.seeds,
                                    args.out, workers=args.workers)
        print(f"storage crash recovery: {summary['passed']}/{summary['seeds']} seeds identical")
        return 0 if summary["failed"] == 0 and not summary["provenance"]["changedDuringRun"] else 1
    if args.command == "secret-sweep":
        from . import secret_sweep
        summary = secret_sweep.run(agent.Impl("go", args.go.resolve()), agent.Impl("rust", args.rust.resolve()), args.out)
        print(f"secret sweep: {summary['fieldCount']} marked fields; {len(summary['failures'])} failures")
        return 0 if not summary["failures"] and not summary["provenance"]["changedDuringRun"] else 1
    if args.command == "shape":
        from . import shape
        summary = shape.run(agent.Impl("go", args.go.resolve()),
                            agent.Impl("rust", args.rust.resolve()), args.out,
                            repeats=args.repeat, workers=args.workers, ids=args.scenarios or None)
        print(f"database shape: {summary['passed']}/{summary['total']} runs identical")
        return 0 if summary["failed"] == 0 and not summary["provenance"]["changedDuringRun"] else 1
    if args.command == "cross-read":
        from . import cross_read
        summary = cross_read.run(agent.Impl("go", args.go.resolve()),
                                   agent.Impl("rust", args.rust.resolve()),
                                   args.out)
        print(f"cross-read: {summary['passed']}/{len(summary['directions'])} directions identical "
              f"(failed: {summary['failedDirections']})")
        return 0 if summary["failed"] == 0 else 1
    if args.command == "unknown-projection":
        from . import unknown_projection
        summary = unknown_projection.run(agent.Impl("go", args.go.resolve()),
                                          agent.Impl("rust", args.rust.resolve()), args.out)
        print(f"unknown projection: {len(summary['failures'])} failures across {len(summary['cases'])} cases")
        return 0 if not summary["failures"] and not summary["provenance"]["changedDuringRun"] else 1
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
