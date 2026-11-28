# m35: the public sites on a VM of their own

Goal (owner, 2026-11-28): minimal.opute.io, www.opute.io and opute.io run on a VM that journeys never
touch. They come back with one committed command, and they pick up each new site image without me.

## Layout

| | Journey dev VM | Site VM |
| --- | --- | --- |
| Directory | `.parity/e2e/devvm` | `.parity/site-vm` |
| Lease | `.parity/t2/dev-vm.lease.json` | `.parity/t2/site-vm.lease.json` |
| Ports (ssh, MCP) | 42222, 43014 | 42322, 43114 |
| Size | 8 vCPU, 16 GiB, 100 GiB | 4 vCPU, 8 GiB, 80 GiB (3.2 GB used) |
| Agent | the journey's candidate | published 0.5.6 (SHA256SUMS `9185b80d…`, the same bytes as the v0.5.6 release) |

`devvm.py` selects the VM with `E2E_INSTANCE=site`. It is the same code, with the same golden image and provisioning. `run.py`
refuses to run a journey on the site VM, and an unknown instance name is refused. Tests:
`evidence/e2e/tests/test_harness.py` `SiteInstance`.

The site VM serves:
- the minimal site recipe (`evidence/m20`), on guest `minimal-k1`, through tunnel `minimal-opute-io`;
- the docs recipe (`evidence/m34/opute-io-site.recipe.yaml`), on the same cluster, through tunnel `opute-www-opute-io`.

The journey VM no longer serves any site, and the `minimal-opute-io` lease is released.

## Commands

All of them use `E2E_INSTANCE=site` and the site-VM lease.

- [`site-restore.py [--fresh] [--sha COMMIT]`](site-restore.py) brings all three sites up in one command. It:
  1. builds the VM if none exists, or boots its overlay if the VM is stopped;
  2. installs the providers by typed calls;
  3. after a reboot, waits for the cluster's Deployments;
  4. runs both recipes;
  5. checks the three URLs, the served catalog version, and the apex MX and SPF records from 1.1.1.1 and 8.8.8.8.
- [`site-deploy.py [--sha COMMIT]`](site-deploy.py) deploys the newest site image. It finds the image for host-agents main
  (the newest successful "Publish site image" run on main, `sha-<commit>`, anonymous GHCR pull) and runs the docs recipe only when the digest differs from the last deploy. It waits
  until the public install page pins that commit's catalog version, then checks the URLs. Each run appends one line to
  `.parity/site-vm/site-deploy.log`. When someone else holds the lease it skips with exit 0; a failure exits 1.
- [`sitelib.py`](sitelib.py) holds what the two share. It talks to the agent only through typed MCP calls, and
  `cloudflare.env` is only copied into the VM.

## Runs

| Step | Result |
| --- | --- |
| Build the site VM (`devvm.py build`) | 93 s |
| Move: `site-restore.py` on the new VM while the journey VM still served | 218 s; both VMs served at once |
| Journey VM `devvm.py restore --check` | 39 s, equal to its baseline; the sites stayed up |
| Probe of all three URLs every 2 s, across the move and the restore | 160 rounds, no non-200: **no downtime** at that resolution |
| From scratch: VM set aside, `site-restore.py` with nothing in place | 302 s in one command (build 91 s, minimal 109 s, docs 33 s); outage 09:43:18 to 09:48:17 |
| Auto-deploy: 0.5.5 image (`--sha 59efab17`), then main (0.5.6), then main again | deployed in 18 s (site served 0.5.5); deployed in 21 s and retired the 0.5.5 deploy (site served 0.5.6); skipped |
| Probe across 7 deploys and the cleanup | 343 rounds, no non-200 |
| Reboot: VM stopped, then the boot unit's command (`site-restore.py`) | 107 s; outage about 80 s (10:14:41 to 10:16:00) |

## Findings (all fixed here)

| # | Finding | Fix |
| --- | --- | --- |
| S1 | Recipe 1.0.0 moved a shared Service's selector to the new version and left the old Deployment running. cloudflared's kept-alive connections stayed on the old pod, so www.opute.io went on serving the old image after a deploy that reported success. | Recipe 1.1.0: each deploy has its own Deployment and Service, and the tunnel's target names that Service. |
| S2 | A deploy with the same inputs as an earlier run replays that run: going back to an earlier image applied nothing. `get_cloudflare_tunnel_status` reports DNS routing, not the tunnel's target, so a validation can't tell the tunnel is pointing at the wrong version. | `siteVersion` is the digest plus a deploy stamp. The workload is therefore always new, so the tunnel node runs again. The new `retiredVersion` input deletes the previous deploy once both URLs answer. |
| S3 | Deploys reported success without checking what the public site served. | `site-deploy.py` waits for the public install page to pin the deployed commit's catalog version. |
| S4 | After a VM reboot, running the recipes right away re-pushed the minimal image to a registry that was not up yet, and the run failed. Before the prerequisites, the agent has no active Kubernetes provider. | `site-restore.py` installs the prerequisites, then waits for every Deployment to be Available, then runs the recipes. |

| S5 | "Publish site image" is path-filtered: host-agents#50 (an openspec note) merged and built no image, so `sha-<main head>` did not exist and the deployer would have skipped forever. | Without `--sha`, the deployer takes the commit of the newest successful "Publish site image" run on main, through the anonymous GitHub API. After #50 it found `59499ed5` and skipped, since that image was already deployed. |

None of these is a product defect: S1 and S2 are in the m34 recipe, and S3 to S5 are in the scripts.
The objects recipe 1.0.0 left behind were deleted by typed calls. The namespace now holds one Deployment and one
Service.

## For the owner: two user units (a host change, so not installed by me)

[`units/`](units/) has the units. They pass `systemd-analyze --user verify`, and lingering is already on for the user.

- `opute-site-vm.service` (oneshot) runs `site-restore.py` at boot under its own lease holder `site-boot`.
- `opute-site-deploy.timer` runs `opute-site-deploy.service` (`site-deploy.py`) 20 minutes after boot and then
  15 minutes after each run.

To install them:

```bash
cp evidence/m35/units/opute-site-* ~/.config/systemd/user/ && systemctl --user daemon-reload && systemctl --user enable opute-site-vm.service opute-site-deploy.timer && systemctl --user start opute-site-deploy.timer
```

## Limits

- The units run the scripts from the main checkout, so the scripts that run are whatever that checkout has checked out.
- A lease that is still held at boot (for example by me) makes `opute-site-vm.service` fail, and the timer skips
  while any lease holder works. Both are logged.
- The site VM's run overlay grows with each deploy's layers. `site-restore.py --fresh` resets it, with about 5 minutes
  of downtime.
- Disk was at 95% before this round. The site VM adds 3.2 GB, and I deleted a cargo cache
  (`.parity/rust-canary-target`, 5.6 GB) to make room. 33 GB (6%) is free.
