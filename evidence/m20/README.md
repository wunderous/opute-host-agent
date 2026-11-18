# m20: a public website from one recipe run

`minimal-site.recipe.yaml` brings a static website up as a K3s Service and
publishes it at a Cloudflare hostname. `minimal-site-teardown.recipe.yaml`
removes it again. Both are `host-recipe.v1`, run with `run_host_local_recipe`
and read back with `get_host_plan_run`. Every step below is a typed Host Agent
tool call. The only manual steps are the owner's two:

1. Put the site's build context (a `Dockerfile` at its root) in
   `.parity/minimal-site/context/`. Staged files are written 0600, so the
   Dockerfile must make them readable to the web server, as
   `e2e-site/Dockerfile` does with `RUN chmod -R a+rX /usr/share/nginx/html`.
2. Write `CLOUDFLARE_ACCOUNT_ID`, `CLOUDFLARE_ZONE_ID` and
   `CLOUDFLARE_API_TOKEN` (Tunnel edit, and DNS edit on opute.io) to a 0600
   env file that the Cloudflare provider unit reads.

## Host prerequisites (typed calls, once per host)

A recipe node cannot install a provider, because the install runs a recipe of
its own (D44). The providers are therefore set up before the first run. These
are the calls `journeys/make_e2e7.py` makes, in this order:

| Call | Arguments |
|---|---|
| `ensure_host_service_supervisor` | `{}` |
| `ensure_oci_builder` | `{}` (task) |
| `ensure_host_file` | the provider's user unit, `~/.config/systemd/user/opute-provider-<id>.service`: `OPUTE_PROVIDER_PORT` (k3s 4320, cloudflare 4330) and `ExecStart` the release's `opute-provider-<id>-linux-x64`. The Cloudflare unit adds `OPUTE_HOST_AGENT_ENDPOINT=http://127.0.0.1:3014/mcp`, the agent's env file (for the callback bearer) and the owner's credentials file. |
| `set_host_service_state` | `{"uri": "host-service:local:user/opute-provider-<id>", "state": "restart", "scope": "user"}` |
| `ensure_host_file` | the provider's activation recipe, byte for byte (`../e2e/journeys/inputs/k3s-activate.yaml`, `cloudflare-activate-tunneling.yaml`) |
| `opute.provider.install` | `{"descriptor": …, "activate": true, "recipeSource": "<activation recipe path>"}`: `com.opute.k3s` 1.0.2, and `com.opute.cloudflare` 1.0.1 with its four capabilities |

## The two recipe calls

Each recipe is written to the host with `ensure_host_file` first. For the
teardown, `__GUEST_NAME__` is replaced with the guest's name (see Notes).

```json
{"name": "run_host_local_recipe", "arguments": {
  "source": "/home/opute/.config/opute-m20/minimal-site.recipe.yaml",
  "inputs": {"hostname": "minimal.opute.io", "tunnelName": "minimal-opute-io", "name": "minimal-k1",
             "guestImage": "parity-noble", "siteVersion": "v1",
             "contextDir": "/home/opute/.opute/build-contexts/minimal-k1-v1",
             "contextFiles": {"Dockerfile": "…", "index.html": "…"}}}}
```

```json
{"name": "run_host_local_recipe", "arguments": {
  "source": "/home/opute/.config/opute-m20/minimal-site-teardown-minimal-k1.recipe.yaml",
  "inputs": {"hostname": "minimal.opute.io", "tunnelName": "minimal-opute-io", "name": "minimal-k1",
             "contextDir": "/home/opute/.opute/build-contexts/minimal-k1-v1"}}}
```

`contextFiles` maps each file of the build context to its text. A new site
version needs a new `siteVersion` and `contextDir`. A new `runNonce` starts a
new run with the same inputs, and that run skips every node.

## Notes on the recipe

- **The cluster pulls `localhost:30500/site/<version>:latest`.** That is the
  in-cluster registry's NodePort on the node itself, and containerd pulls
  from it over plain HTTP with no registry configuration. The host pushes the
  same image to `<guest address>:30500`.
- **A node's preflight read.** A node reads its effect before acting, unless a
  node it depends on was applied in this run. A read of a guest or cluster
  that does not exist yet is refused. So each node that creates one depends on
  the node applied just before it, starting with the staged context, and the
  teardown removes the staged context to start that chain again.
- **Assertion values are compared literally** (`${…}` is not interpolated in
  an assertion). So each site version is its own Deployment, and the Ingress
  is named after the hostname. The teardown asserts the guest's absence from
  `list_vms` by a literal name, which the caller fills in, as the
  www-opute-io deploy script fills in its recipe's expected image.
- **Without credentials, the teardown's tunnel node passes without removing
  anything**, because the provider then reports no route and no tunnel. The
  journey therefore checks public DNS directly as well.
- **The teardown removes the guest before the tunnel.** The connector runs in
  the guest's cluster, and `remove-host-tunnel` stops only a host-placed
  connector. Cloudflare refuses to delete a tunnel that still has connections
  (code 1022) and closes them within minutes of the connector stopping, so the
  tunnel node retries up to six times, 10 s apart and doubling.

## Decisions

| | Finding | Decision |
|---|---|---|
| D44 | A recipe node that ran `run_host_local_recipe` validated, then never finished: the inner run waits for the heavy slot the outer run holds. Go v0.2.4 has the same gap. | Plan and recipe validation refuse all seven tools that start a plan run ([refuse-nested-plan-runs](../../openspec/changes/refuse-nested-plan-runs/proposal.md)). Failing first: `d44-before.log`. |
| D45 | A validation-only node failed at once after an applied dependency: the executor skipped its preflight to force an action it does not have. Go v0.2.4 does the same. | A node without an action always evaluates its validation ([validation-only-node-validates](../../openspec/changes/validation-only-node-validates/proposal.md)). Failing first: `d45-before.log`. |

## Results

Gate e2e-7 ran `e2e7.public-site` on `rust-0.4.0-rc3` under run `m20-r1`
(`evidence/e2e/runs/m20-r1/e2e7.public-site/`), and
`evidence/e2e/verify.py --gate e2e-7` passes. From the snapshot, in 317 s:

| Step | Outcome |
|---|---|
| Site recipe, first run | completed in 110 s; eight nodes applied, the three validation-only nodes satisfied (D45) |
| Public page | `https://e2e-m20-r1.opute.io/` answers 200 with the site's marker, fetched by the host over verified TLS |
| Site recipe, second run | completed in 5 s; every node satisfied |
| Pod deleted | replaced in 3 s; the page answers again |
| Secrets | the Cloudflare API token and the tunnel's run token: in no file but the owner's credentials file, in no journal line, in neither run record |
| Teardown, first run | completed in 90 s; guest, tunnel (attempt 4 of 6) and context applied |
| Teardown, second run | every node satisfied |
| After | no CNAME answers, the tunnel is gone by name, no guest, no provider unit, no residue |

The first attempt was retired (`.parity/retired/m20-r1-e2e7-first/`). It
brought the site up and served it, then found the teardown order above, and
three journey defects: the pod deletion lacked the client capabilities
`delete_cluster_resource` requires, four steps read an incus project the
agent's guests are not in, and a direct provider-tool call was checked against
the catalog read before the providers were installed. The journey reran once
under the same run on the same build.
