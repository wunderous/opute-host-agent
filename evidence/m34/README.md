# m34: www.opute.io from the dev machine's K3s

Owner request (2026-11-28): fix the documentation site and host it with a recipe on the dev
machine's K3s deployment.

## Why the site was down

www.opute.io and opute.io answered 530 (Cloudflare: origin unreachable) from 00:37Z on
2026-11-28 at the latest (m33, W1). Both names are CNAMEs to the dedicated tunnel
`opute-www-opute-io`. The tunnel's connector ran on `cluster:local:opute-ha-a`, a cluster
that the private site-deployment controller (`wunderous/opute-site-deploy`, last pushed
2026-09-29) deployed to through the Go Host Agent. That cluster no longer exists: this
workstation has no Incus. So the tunnel had no connector.

## What was found

| # | Finding | Result |
| --- | --- | --- |
| C1 | `ensure_cloudflared_tunnel` deleted every other DNS record at a hostname before creating the tunnel's CNAME. The apex `opute.io` has three MX records and an SPF TXT record (Cloudflare Email Routing), so routing the apex would have deleted the zone's mail records. | Defect, fixed: `tunnel-cname-keeps-other-records`. Only A, AAAA and CNAME records are replaced. Failing-first test `a_tunnel_cname_keeps_the_apex_mail_records`: with 0.5.5's rule the MX and TXT records are in the delete list. |

## The recipe

[`opute-io-site.recipe.yaml`](opute-io-site.recipe.yaml) serves the site image that
host-agents' *Publish site image* workflow builds from main
(`ghcr.io/wunderous/host-agent-www`, pinned to its digest) on a cluster the agent already
runs:

1. `workload`: namespace `host-agent-site`, a Deployment named after the image version, and
   the `host-agent-www` Service.
2. `tunnel`: the existing tunnel `opute-www-opute-io`, for www.opute.io and opute.io, with
   its connector `cloudflared-www` in namespace `host-agent-www-tunnel`.
3. `connector`, `www`, `apex`: validation only. The connector is ready, and both URLs answer
   200 over verified TLS.

It doesn't touch the minimal site's namespaces or tunnel.

## Runs

The site VM was rebuilt on `rust-0.5.6-rc1` (from `20cfbe135`: 0.5.5 plus C1; SHA256SUMS
`9185b80d…`). The 0.5.3 site VM is set aside in `.parity/m34/devvm-0.5.3-site`. On the new VM,
the providers were installed by typed calls. The minimal site recipe then ran on guest
`minimal-k1` (run `63af773a…`), and this recipe ran on `cluster:local:minimal-k1`:

| Run | Inputs | Result |
| --- | --- | --- |
| [`f9b896a4…`](www-run-1.json) | image `sha256:ebd3f1a8…` (host-agents `76b52345`) | completed in 33 s: `workload` and `tunnel` applied; `connector`, `www` and `apex` satisfied |
| [`718204ee…`](www-run-2.json) | the same, `runNonce: m34-second` | completed in 1 s: every node satisfied |
| [`d4b9f117…`](www-run-3.json) | image `sha256:81a2761a…` (host-agents `59efab17`, the 0.5.5 catalog) | completed in 24 s: the new version's `workload` applied and `tunnel` reapplied; the site shows 0.5.5 |
| [`7dee7e96…`](www-run-4.json) | image `sha256:a3523953…` (host-agents `59499ed5`, the 0.5.6 catalog, after 0.5.6 was published from this candidate) | completed in 18 s: the new version's `workload` applied and `tunnel` reapplied; the site shows 0.5.6 |

Checked from this workstation afterwards:
- https://www.opute.io/ and https://opute.io/ answer 200 with the documentation site, and
  minimal.opute.io answers 200.
- The apex MX (route1, route2 and route3.mx.cloudflare.net) and its SPF TXT record are
  unchanged, from 1.1.1.1 and 8.8.8.8.

## Limits

- The site runs on the dev VM. A journey that restores or rebuilds that VM takes all three
  sites down until `.parity/m34/site-restore.py` runs again. The script is local, like m30's
  restore.
- A new site image is deployed by running the recipe again with its digest and a new
  `siteVersion`. Nothing deploys it automatically, as the private controller did. Old
  Deployments stay until they are deleted.
- The private controller's OpenSpec change in host-agents still describes the old
  deployment.
