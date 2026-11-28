#!/usr/bin/env python3
"""m35: deploy the newest www.opute.io image to the site VM when it changed.

  site-deploy.py [--sha COMMIT]

Finds the site image host-agents' "Publish site image" workflow built from main
(tag sha-<commit>, anonymous pull), compares its digest with the one last
deployed (.parity/site-vm/site-deployed.json), and only when it differs runs the
docs recipe and checks the three URLs. Each run appends one JSON line to
.parity/site-vm/site-deploy.log. Takes the site-VM lease for its run when no one
holds it, and skips (exit 0) when someone else does; a failure exits 1.
"""
import argparse
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import sitelib  # noqa: E402
from sitelib import devvm, e2elib  # noqa: E402

HOLDER = "site-deploy"
LOG = e2elib.DEV / "site-deploy.log"


def deploy(sha=None) -> dict:
    sha, digest = sitelib.latest_digest(sha)
    deployed = (sitelib.deployed() or {}).get("digest")
    out = {"at": e2elib.now(), "main": sha, "digest": digest, "deployed": deployed}
    if digest is None:
        return {**out, "result": "skipped: the image for this commit is not published yet"}
    if digest == deployed:
        return {**out, "result": "skipped: already deployed"}
    if not devvm.pid():
        return {**out, "result": "failed: the site VM is not running"}
    mail = sitelib.mail_records()
    run, _ = sitelib.docs_site(digest, sha)
    sitelib.check_sites(mail)
    return {**out, "result": "deployed", "run": run}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--sha", help="a host-agents main commit instead of the current head")
    args = parser.parse_args()
    held = e2elib.lease_status()
    if held:
        record = {"at": e2elib.now(), "result": f"refused: the site-VM lease is held by {held['holder']}"}
    else:
        e2elib.acquire_lease(HOLDER, 1)
        try:
            record = deploy(args.sha)
        except SystemExit as error:
            record = {"at": e2elib.now(), "result": f"failed: {error}"}
        finally:
            e2elib.release_lease(HOLDER)
    with LOG.open("a") as log:
        log.write(json.dumps(record) + "\n")
    print(json.dumps(record), flush=True)
    return 1 if record["result"].startswith("failed") else 0


if __name__ == "__main__":
    sys.exit(main())
