#!/usr/bin/env python3
"""Write the public-site journey (gate e2e-7, goal m20) as JSON.

  python3 evidence/e2e/journeys/make_e2e7.py

From the snapshot, the journey installs the K3s and Cloudflare providers by
typed calls (D44: a recipe cannot), runs evidence/m20/minimal-site.recipe.yaml
for e2e-<run>.opute.io, fetches the page from the host over verified TLS,
runs the recipe again (every node skips), deletes the pod and fetches again,
counts the Cloudflare token and the tunnel's run token in every file, the
journal and the run records, and runs the teardown recipe twice. Then no CNAME
answers, the tunnel is gone, and so are the guest and the provider units.

The owner's Cloudflare credentials file is copied into the VM unread; the
harness refuses to write any record that holds its token.
"""
from __future__ import annotations

import hashlib
import json
import os

import make_e2e1 as base
from make_e2e1 import HERE, SPEC, a, inp, ok, req
from make_e2e3a import call, direct, res, task
from make_e2e4a import PKGS_BEFORE, PODMAN_CLEANUP, PODMAN_EFFECTS, SUPERVISOR_EFFECTS

GATE = "e2e-7"
REPO = HERE.parent.parent.parent
M20 = REPO / "evidence/m20"
NESTED_SPEC = "openspec/changes/refuse-nested-plan-runs/specs/refuse-nested-plan-runs/spec.md"
VALIDATE_SPEC = "openspec/changes/validation-only-node-validates/specs/validation-only-node-validates/spec.md"
REL = base.REL
CFG = "/home/opute/.config/opute-e2e7"
NAME = "e2e-s7"
VERSION = "v1"
HOST = "e2e-${RUN}.opute.io"
TUNNEL = "e2e-${RUN}"
CLUSTER = f"cluster:local:{NAME}"
CONTEXT = f"/home/opute/.opute/build-contexts/{NAME}-{VERSION}"
SITE = f"{CFG}/minimal-site.recipe.yaml"
DOWN = f"{CFG}/minimal-site-teardown.recipe.yaml"
NESTED = f"{CFG}/nested.recipe.yaml"
ENV = f"{CFG}/cloudflare.env"
UNITS = "/home/opute/.config/systemd/user"
MARKER = "e2e7 public site"
DONE = ["completed", "failed", "cancelled"]
# remove_host_file is task-aware and needs the roots capability too
ROOTS_META = {"io.modelcontextprotocol/clientCapabilities": {"extensions": {"io.modelcontextprotocol/tasks": {}},
                                                             "roots": {}}}
SITE_NODES = ["context", "guest", "guest-address", "cluster", "registry", "image", "workload", "domain", "tunnel",
              "connector", "public"]
VALIDATION_ONLY = ["guest-address", "connector", "public"]

FILES = {**base.TOOL_FILES,
         "leakcheck": inp("${REPO}/evidence/e2e/guest-tools/leakcheck.py"),
         # the owner's file (goal m20, owner step 2): never pinned, never read into a record
         "cloudflareEnv": {"path": "${PARITY}/minimal-site/cloudflare.env", "secret": True,
                           "secretKeys": ["CLOUDFLARE_API_TOKEN"]}}

PROVIDERS = [
    {"id": "k3s", "port": 4320, "env": [], "activate": "k3s-activate.yaml", "probe": "opute.capability.kubernetes.provision",
     "descriptor": {"schema": "opute-provider-plugin.v1", "pluginId": "com.opute.k3s", "version": "1.0.2",
                    "capabilities": [{"id": "opute.capability.kubernetes.v1", "version": 1}]}},
    # The Cloudflare provider calls back into the agent with the agent's own bearer (its env file) and reads
    # the owner's credentials file.
    {"id": "cloudflare", "port": 4330, "activate": "cloudflare-activate-tunneling.yaml", "probe": "ensure_cloudflared_tunnel",
     "env": ["Environment=OPUTE_HOST_AGENT_ENDPOINT=http://127.0.0.1:3014/mcp",
             "EnvironmentFile=/etc/opute/host-agent.env", f"EnvironmentFile={ENV}"],
     "descriptor": {"schema": "opute-provider-plugin.v1", "pluginId": "com.opute.cloudflare", "version": "1.0.1",
                    "capabilities": [{"id": f"opute.capability.{c}.v1", "version": 1}
                                     for c in ("tunneling", "mesh-runtime", "mesh-membership", "public-ingress")]}},
]


def literal(text: str) -> str:
    """A recipe's own `${...}` references, escaped from the harness's expansion."""
    return text.replace("${", "$${")


def unit(p: dict) -> str:
    return "\n".join(["[Unit]", f"Description=Opute {p['id']} provider (e2e7)", "", "[Service]",
                      f"Environment=OPUTE_PROVIDER_PORT={p['port']}", *p["env"],
                      f"ExecStart={REL}/opute-provider-{p['id']}-linux-x64", "Restart=on-failure", "",
                      "[Install]", "WantedBy=default.target", ""])


def listed(name: str) -> str:
    """Poll tools/list in the guest until `name` is listed (at most 60 s)."""
    return ("python3 -c 'import sys, time, json; sys.path.insert(0, \"/opt/opute-e2e/guest\"); import mcpwire\n"
            "for i in range(120):\n"
            "    r = mcpwire.mcp(3014, sys.argv[1], \"tools/list\", request_id=i + 1)\n"
            "    names = [t[\"name\"] for t in ((r.get(\"body\") or {}).get(\"result\") or {}).get(\"tools\") or []]\n"
            f"    if \"{name}\" in names: break\n"
            "    time.sleep(0.5)\n"
            f"print(json.dumps({{\"listed\": \"{name}\" in names, \"tools\": len(names)}}))' '${{TOKEN}}'")


def sha256(text: str) -> str:
    return hashlib.sha256(text.encode()).hexdigest()


def written(id_: str, files: dict[str, str], *confirmed: str) -> dict:
    """The files as the host holds them, by digest: the writes took effect (m23)."""
    return {"id": id_, "guest": "sha256sum " + " ".join(files) + " | cut -c1-64", "confirms": list(confirmed)}


def digests(files: dict[str, str]) -> str:
    return "".join(sha256(text) + "\n" for text in files.values())


def activation(p: dict) -> dict[str, str]:
    return {f"{CFG}/{p['id']}-activate.yaml": (HERE / "inputs" / p["activate"]).read_text()}


def teardown_recipe() -> str:
    return (M20 / "minimal-site-teardown.recipe.yaml").read_text().replace("__GUEST_NAME__", NAME)


def recipe_files() -> dict[str, str]:
    return {SITE: (M20 / "minimal-site.recipe.yaml").read_text(), DOWN: teardown_recipe(), NESTED: nested_recipe()}


def provider_steps() -> list[dict]:
    steps = [call("supervisor", "ensure_host_service_supervisor", {}),
             {"id": "linger", "confirms": ["supervisor"], "guest": (
                 "for i in $(seq 100); do [ -S /run/user/$(id -u opute)/bus ] && break; sleep 0.1; done; "
                 "loginctl show-user opute -p Linger --value; test -S /run/user/$(id -u opute)/bus && echo bus")},
             PKGS_BEFORE,
             *task("builder", "ensure_oci_builder", {}),
             {"id": "podman", "guest": "command -v podman", "confirms": ["builder"]}]
    for p in PROVIDERS:
        name, act = f"opute-provider-{p['id']}", f"{CFG}/{p['id']}-activate.yaml"
        desc = {**p["descriptor"],
                "server": {"transport": "streamable_http", "endpoint": f"http://127.0.0.1:{p['port']}/mcp"}}
        steps += [
            call(f"{p['id']}Unit", "ensure_host_file",
                 {"path": f"{UNITS}/{name}.service", "content": unit(p), "mode": 0o644, "scope": "user"}),
            call(f"{p['id']}Start", "set_host_service_state",
                 {"uri": f"host-service:local:user/{name}", "state": "restart", "scope": "user"}),
            # the pinned Go tree's recipe, byte for byte: the provider's install manifest pins its sha256
            call(f"{p['id']}Activation", "ensure_host_file",
                 {"path": act, "content": literal((HERE / "inputs" / p["activate"]).read_text()), "mode": 0o644,
                  "scope": "user"}),
            # the unit, started from the file written above, serves the provider
            {"id": f"{p['id']}Port", "guest": f"for i in $(seq 60); do ss -ltnH 'sport = :{p['port']}' | grep -q . "
                                              f"&& break; sleep 0.5; done; ss -ltnH 'sport = :{p['port']}' | wc -l",
             "confirms": [f"{p['id']}Unit", f"{p['id']}Start"]},
            written(f"{p['id']}ActivationFile", activation(p), f"{p['id']}Activation"),
            call(f"{p['id']}Install", "opute.provider.install",
                 {"descriptor": desc, "activate": True, "recipeSource": act}),
            {"id": f"{p['id']}Listed", "guest": listed(p["probe"]), "confirms": [f"{p['id']}Install"]},
        ]
    return steps


def context_files() -> dict:
    root = M20 / "e2e-site"
    return {p.name: p.read_text() for p in sorted(root.iterdir()) if p.is_file()}


def site_inputs(nonce: str) -> dict:
    return {"hostname": HOST, "tunnelName": TUNNEL, "name": NAME, "guestImage": "parity-noble",
            "contextDir": CONTEXT, "contextFiles": context_files(), "siteVersion": VERSION, "runNonce": nonce}


def down_inputs(nonce: str) -> dict:
    return {"hostname": HOST, "tunnelName": TUNNEL, "name": NAME, "contextDir": CONTEXT, "runNonce": nonce}


def recipe_run(id_: str, source: str, inputs: dict, timeout: int = 1200) -> list[dict]:
    return [call(id_ + "Start", "run_host_local_recipe", {"source": source, "inputs": inputs},
                 capture={"RUN_" + id_: "response.body.result.structuredContent.runId"}),
            call(id_, "get_host_plan_run", {"runId": "${RUN_" + id_ + "}"},
                 until={"path": "response.body.result.structuredContent.status", "in": DONE},
                 timeout=timeout, interval=5)]


def removal_task(id_: str, tool: str, args: dict) -> list[dict]:
    """A task-aware removal, with the roots capability it requires."""
    steps = task(id_, tool, args)
    for step in steps:
        step["meta"] = ROOTS_META
    return steps


def removal(id_: str, path: str) -> list[dict]:
    return removal_task(id_, "remove_host_file", {"path": path, "confirm": True, "scope": "user"})


def node(step: str, node_id: str, **op) -> dict:
    return direct(step, f"structuredContent.nodes.{node_id}.status", **op)


def nested_recipe() -> str:
    return json.dumps({
        "contractVersion": "host-recipe.v1", "recipeId": "com.opute.e2e.nested", "recipeVersion": "1.0.0",
        "execution": {"coordinator": "host-agent", "mode": "local"},
        "compatibility": {"requiredTools": ["run_host_local_recipe", "get_host_plan_run"]},
        "plan": {"contractVersion": "host-plan.v1", "planId": "com.opute.e2e.nested", "generation": 1,
                 "idempotencyKey": "e2e-nested",
                 "nodes": [{"id": "inner", "action": {"tool": "run_host_local_recipe", "args": {"source": SITE}},
                            "validate": {"tool": "get_host_plan_run", "args": {"runId": "x"},
                                         "assert": [{"path": "/status", "op": "eq", "value": "completed"}]}}]}})


TUNNEL_ARGS = {"bindingId": "site", "hostname": HOST, "tunnelName": TUNNEL,
               "localTarget": "http://traefik.kube-system.svc.cluster.local:80", "placement": "kubernetes",
               "targetUri": CLUSTER}
DOH = "https://cloudflare-dns.com/dns-query?name=" + HOST + "&type=CNAME"


def steps() -> list[dict]:
    teardown = teardown_recipe()
    return base.TOOL_STEPS + [
        {"id": "leakCopy", "copy": "leakcheck", "to": "/opt/opute-e2e/guest/leakcheck.py"},
        {"id": "envCopy", "copy": "cloudflareEnv", "to": ENV, "mode": "0600"},
        {"id": "envOwner", "guest": f"chown -R opute:opute {CFG} && stat -c '%a %U' {ENV}"},
        *provider_steps(),
        # the providers' tools, for the schema check of the direct calls below
        {"id": "catalogProviders", "mcp": "tools/list"},
        call("siteRecipe", "ensure_host_file", {"path": SITE, "content": literal((M20 / "minimal-site.recipe.yaml").read_text()),
                                                "mode": 0o644, "scope": "user"}),
        call("downRecipe", "ensure_host_file", {"path": DOWN, "content": literal(teardown), "mode": 0o644, "scope": "user"}),
        call("nestedRecipe", "ensure_host_file", {"path": NESTED, "content": nested_recipe(), "mode": 0o644,
                                                  "scope": "user"}),
        written("recipeFiles", recipe_files(), "siteRecipe", "downRecipe", "nestedRecipe"),
        call("validateSite", "validate_host_local_recipe", {"source": SITE, "inputs": site_inputs("up")}),
        call("validateDown", "validate_host_local_recipe", {"source": DOWN, "inputs": down_inputs("down")}),
        call("validateNested", "validate_host_local_recipe", {"source": NESTED}),
        # one run brings the site up
        *recipe_run("up", SITE, site_inputs("up")),
        {"id": "page", "fetch": f"https://{HOST}/", "until": {"path": "response.status", "in": [200]},
         "retryTimeout": 300, "interval": 10, "confirms": ["upStart"]},
        # a second run with the same inputs finds every effect in place
        *recipe_run("again", SITE, site_inputs("again"), timeout=600),
        # a deleted pod comes back and the site answers again
        call("pods", "list_pods", {"uri": CLUSTER, "namespace": "site"},
             capture={"POD": "response.body.result.structuredContent.pods[0].name"}),
        *removal_task("deletePod", "delete_cluster_resource",
                      {"uri": CLUSTER, "kind": "Pod", "namespace": "site", "resourceName": "${POD}"}),
        {"id": "podBack", "guest": (
            f"for i in $(seq 90); do n=$(incus exec {NAME} -- k3s kubectl -n site get pods "
            "-l app.kubernetes.io/name=site -o jsonpath='{range .items[?(@.status.containerStatuses[0].ready==true)]}"
            "{.metadata.name}{\"\\n\"}{end}' | grep -v -x '${POD}' | head -1); [ -n \"$n\" ] && break; sleep 2; "
            "done; [ -n \"$n\" ] && echo replaced"), "timeout": 240, "confirms": ["deletePod"]},
        # after the second run and the replaced pod, the site still answers
        {"id": "pageAgain", "fetch": f"https://{HOST}/", "until": {"path": "response.status", "in": [200]},
         "retryTimeout": 120, "interval": 5, "confirms": ["againStart"]},
        # no file, journal line or run record holds the Cloudflare token or the tunnel's run token
        {"id": "leaks", "guest": (f"python3 /opt/opute-e2e/guest/leakcheck.py --env {ENV} --key CLOUDFLARE_API_TOKEN "
                                  f"--guest {NAME} --secret cloudflare-connector/cloudflared-token/token "
                                  "--run '${RUN_up}' --run '${RUN_again}'")},
        # one run removes it all; a second finds nothing to remove
        *recipe_run("down", DOWN, down_inputs("down"), timeout=600),
        *recipe_run("downAgain", DOWN, down_inputs("downAgain"), timeout=300),
        {"id": "dns", "fetch": DOH, "headers": {"Accept": "application/dns-json"},
         # Cloudflare answers a name its zone does not hold with no Answer (NOERROR, not NXDOMAIN)
         "until": {"path": "response.body", "matches": "^(?![\\s\\S]*\"Answer\")"}, "retryTimeout": 360,
         "interval": 10},
        call("tunnelGone", "get_cloudflare_tunnel_status", TUNNEL_ARGS),
        {"id": "guestGone", "guest": f"incus list -f csv -c n | grep -c -x {NAME}; true",
         "confirms": ["downStart", "downAgainStart"]},
        # the providers were host prerequisites; their units go the same typed way
        *[s for p in PROVIDERS for s in (
            call(f"{p['id']}Stop", "set_host_service_state",
                 {"uri": f"host-service:local:user/opute-provider-{p['id']}", "state": "stop", "scope": "user"}),
            *removal(f"{p['id']}UnitRemove", f"{UNITS}/opute-provider-{p['id']}.service"))],
        {"id": "unitsGone", "guest": (f"ls {UNITS} 2>/dev/null | grep -c -E '^opute-(provider|cloudflare)'; "
                                      "systemctl list-units --all --no-legend 'cloudflared*' | wc -l; "
                                      "ss -ltnH '( sport = :4320 or sport = :4330 )' | wc -l"),
         "confirms": [f"{p['id']}{s}" for p in PROVIDERS for s in ("Stop", "UnitRemove")]},
    ]


def requirements() -> list[dict]:
    up, again = "up", "again"
    return [
        req(SPEC, "One typed plan and recipe execution path",
            direct("validateSite", "structuredContent.plan.valid", equals=True),
            direct("validateDown", "structuredContent.plan.valid", equals=True),
            direct(up, "structuredContent.status", equals="completed"),
            *[node(up, n, equals="applied") for n in SITE_NODES if n not in VALIDATION_ONLY],
            direct(again, "structuredContent.status", equals="completed"),
            *[node(again, n, equals="satisfied") for n in SITE_NODES],
            a("page", "response.status", equals=200),
            a("page", "response.tlsVerified", equals=True),
            a("page", "response.body", contains=MARKER),
            res("deletePod", "isError", **{"in": [False, "<absent>"]}),
            a("podBack", "stdout", equals="replaced\n"),
            a("pageAgain", "response.status", equals=200),
            a("pageAgain", "response.body", contains=MARKER),
            direct("down", "structuredContent.status", equals="completed"),
            *[node("down", n, equals="applied") for n in ("tunnel", "guest", "context")],
            direct("downAgain", "structuredContent.status", equals="completed"),
            *[node("downAgain", n, equals="satisfied") for n in ("tunnel", "guest", "context")],
            a("dns", "response.json.Status", **{"in": [0, 3]}),
            a("dns", "response.json.Answer", absent=True),
            direct("tunnelGone", "structuredContent.routed", equals=False),
            direct("tunnelGone", "structuredContent.tunnelId", equals=""),
            a("guestGone", "stdout", equals="0\n"),
            a("unitsGone", "stdout", equals="0\n0\n0\n"),
            scenario="One host-local recipe run brings a K3s Service up at a public hostname that the host fetches "
                     "over verified TLS, a second run skips every node, a deleted pod is replaced, and one teardown "
                     "run removes the CNAME, the tunnel and the guest (goal m20)"),
        req(NESTED_SPEC, "A plan node never starts a plan run of its own",
            direct("validateNested", "isError", equals=True),
            direct("validateNested", "content[0].text",
                   matches="recipe node \"inner\" cannot recursively run run_host_local_recipe"),
            direct("k3sInstall", "isError", **{"in": [False, "<absent>"]}),
            direct("cloudflareInstall", "isError", **{"in": [False, "<absent>"]}),
            direct(up, "structuredContent.status", equals="completed"),
            scenario="A recipe that runs another recipe is refused at validation; the providers installed by typed "
                     "calls first serve the site recipe's nodes (D44)"),
        req(VALIDATE_SPEC, "A validation-only node always evaluates its validation",
            *[node(up, n, equals="satisfied") for n in VALIDATION_ONLY],
            node(up, "guest", equals="applied"),
            node(up, "tunnel", equals="applied"),
            scenario="guest-address, connector and public each follow an applied node and are satisfied by their "
                     "validation (D45)"),
        req(SPEC, "Durable and redacted observations",
            a("envOwner", "stdout", equals="600 opute\n"),
            a("leaks", "json.apiToken.present", equals=True),
            a("leaks", "json.runToken.present", equals=True),
            a("leaks", "json.clean", equals=True),
            scenario="The Cloudflare API token and the tunnel's run token are in no file under the agent's home but "
                     "the owner's credentials file, in no journal line and in neither run record"),
        req(SPEC, "Revisioned provider-neutral catalog and lifecycle",
            direct("supervisor", "structuredContent.status", equals="ready"),
            res("builder", "isError", equals=False),
            *[direct(f"{p['id']}{s}", "isError", **{"in": [False, "<absent>"]})
              for p in PROVIDERS for s in ("Unit", "Start", "Activation", "Stop")],
            *[res(f"{p['id']}UnitRemove", "isError", equals=False) for p in PROVIDERS],
            *[direct(s, "isError", **{"in": [False, "<absent>"]}) for s in ("siteRecipe", "downRecipe", "nestedRecipe")],
            a("k3sListed", "json.listed", equals=True),
            a("linger", "stdout", equals="yes\nbus\n"), a("podman", "stdout", equals="/usr/bin/podman\n"),
            *[a(f"{p['id']}Port", "stdout", equals="1\n") for p in PROVIDERS],
            *[a(f"{p['id']}ActivationFile", "stdout", equals=digests(activation(p))) for p in PROVIDERS],
            a("recipeFiles", "stdout", equals=digests(recipe_files())),
            a("cloudflareListed", "json.listed", equals=True),
            scenario="The K3s and Cloudflare providers, started as user units and installed by opute.provider.install, "
                     "publish their tools in the agent's catalog"),
    ]


def cleanup() -> list[dict]:
    """Whatever failed above: the teardown recipe again (it removes the CNAME and tunnel), then the setup."""
    return [
        *recipe_run("cleanDown", DOWN, down_inputs("cleanup"), timeout=600),
        {"id": "cleanSetup", "guest": (
            f"incus delete -f {NAME} 2>/dev/null; "
            "runuser -u opute -- env XDG_RUNTIME_DIR=/run/user/$(id -u opute) systemctl --user stop "
            "opute-provider-k3s opute-provider-cloudflare 2>/dev/null; "
            f"rm -f {UNITS}/opute-provider-*.service; rmdir {UNITS} /home/opute/.config/systemd 2>/dev/null; "
            f"rm -rf {CFG} {CONTEXT} /opt/opute-e2e/guest/leakcheck.py; "
            "incus storage volume list default -f csv | awk -F, '$1==\"image\"{print $2}' | "
            "xargs -r -I{} incus storage volume delete default image/{}; "
            "loginctl disable-linger opute; " + PODMAN_CLEANUP + "true")},
    ]


EFFECTS = PODMAN_EFFECTS + SUPERVISOR_EFFECTS + ["incus/*", "files//home/opute/.config/opute-e2e7*", "files//home/opute/.config/systemd/user*",
           "files//home/opute/.opute*", "files//home/opute/.config/incus*", "files//home/opute/.local/share*",
           "files//home/opute/.config/opute/instances*", "system/units/*", "system/unitFiles/*"]
PERSIST = ["files//home/opute/.opute*", "files//home/opute/.local/share*", "files//home/opute/.config/opute/instances*"]

INPUTS = {"agent": base.AGENT,
          "providers": "the release's K3s and Cloudflare provider executables, started as user units by typed calls",
          "hostname": f"{HOST}, a CNAME the Cloudflare provider creates in the owner's opute.io zone and removes",
          "k3s": "K3s v1.31.8+k3s1, downloaded by the provider; nginx and cloudflared images pulled from public "
                 "registries",
          "site": "evidence/m20/e2e-site, a static page built into an nginx image",
          "files": FILES}


def journey() -> dict:
    return base.journey(
        "e2e7.public-site",
        "A public website from one host-local recipe: providers installed by typed calls, then "
        "evidence/m20/minimal-site.recipe.yaml brings a K3s Service up at e2e-<run>.opute.io through a dedicated "
        "Cloudflare tunnel, the host fetches it over verified TLS, a second run skips every node, a deleted pod is "
        "replaced, no record holds a token, and the teardown recipe removes the CNAME, tunnel and guest.",
        INPUTS, requirements(), steps(), cleanup(), allow=EFFECTS, persist=PERSIST, minutes=30, gate=GATE)


def main() -> None:
    j = journey()
    (HERE / f"{j['id']}.json").write_text(json.dumps(j, indent=1, sort_keys=True) + "\n")
    print(j["id"], len(j["steps"]), "steps")


if __name__ == "__main__":
    os.chdir(REPO)
    main()
