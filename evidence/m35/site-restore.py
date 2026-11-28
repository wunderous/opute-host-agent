#!/usr/bin/env python3
"""m35: bring minimal.opute.io, www.opute.io and opute.io up on the site VM, in one command.

  site-restore.py [--fresh] [--sha COMMIT]

Builds the site VM if .parity/site-vm has none (from the release set named by
SITE_RELEASE, default .parity/e2e/release/rust-0.5.6-rc1, the published 0.5.6 files),
starts it if it is stopped, and with --fresh discards its run overlay first. Then
the providers by typed calls, the minimal site recipe, and the docs recipe with
the site image of host-agents main (or of --sha). Needs the site-VM lease.
"""
import argparse
import os
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import sitelib  # noqa: E402
from sitelib import devvm, e2elib  # noqa: E402


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--fresh", action="store_true")
    parser.add_argument("--sha", help="a host-agents main commit instead of the current head")
    args = parser.parse_args()
    e2elib.require_lease()
    t0 = time.monotonic()
    rebooted = False  # the VM booted on its existing overlay: wait for its cluster before the recipes
    if not (e2elib.DEV / "build.json").exists():
        release = Path(os.environ.get("SITE_RELEASE", e2elib.PARITY / "e2e/release/rust-0.5.6-rc1"))
        record = devvm.build(release.resolve())
        print(f"site VM built in {record['seconds']} s on {record['agent']['version']}", flush=True)
    if args.fresh:
        restored = devvm.restore(True)
        if not restored["equal"]:
            raise SystemExit(f"the site VM could not reproduce its baseline: {restored['diff'][:5]}")
    elif not devvm.pid():
        devvm.check_base()
        rebooted = (e2elib.DEV / "run.qcow2").exists()
        if not rebooted:
            devvm.new_run_overlay()
        devvm.boot(e2elib.DEV / "run.qcow2")
    sha, digest = sitelib.latest_digest(args.sha)
    if not digest:
        raise SystemExit("the site image of that host-agents commit is still building; pass an earlier --sha")
    mail = sitelib.mail_records()
    sitelib.copy_credentials()
    sitelib.prerequisites()
    if rebooted and sitelib.deployed():
        sitelib.wait_for_cluster()
    sitelib.minimal_site()
    sitelib.docs_site(digest, sha)
    sitelib.check_sites(mail)
    print(f"the sites serve from the site VM, {round(time.monotonic() - t0)} s in all", flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
