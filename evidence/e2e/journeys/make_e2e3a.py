#!/usr/bin/env python3
"""Write the e2e-3a journeys (HA k3s through the Rust Host Agent) as JSON.

  python3 evidence/e2e/journeys/make_e2e3a.py

The committed JSON files are the journeys; this script is how they were
authored. Each journey starts from the snapshot and builds its own cluster
with the same steps (journey 1), only through typed Host Agent calls:

- host A is the dev VM's system agent (opute-bootstrap-mcp.service) with the
  Rust k3s provider; it provisions the first server and prepares each join;
- host B is a second, transient Rust agent on the same VM with its own k3s
  provider; it provisions the joining servers and redeems the join material
  the source provider sealed to B's receiver key, so the unseal path runs as
  it does between two hosts.

Guest steps only set up the agents and providers, observe, and clean up.
Incus guests live in the `default` project: the agent addresses instances
there (Q16).
"""
from __future__ import annotations

import json

import make_e2e1 as base
from make_e2e1 import HERE, SPEC, T, TASKS_META, a, inp, ok, req

GATE = "e2e-3a"
PAGE = "published docs, host-agents c9f9e775 (v0.2.4) site/public/docs/availability/ (test record 2026-09-25)"
CATALOG = "baseline/inventory/catalog/provider-k3s.json (opute.capability.kubernetes, D22 neutral names)"
Q17_SPEC = "openspec/changes/k3s-quorum-recovery-retirement/specs/k3s-membership/spec.md"
ENDPOINT_SPEC = "openspec/changes/typed-ha-endpoint/specs/typed-ha-endpoint/spec.md"
HOST = "k3s.e2e.test"
SEAM_SPEC = "openspec/changes/three-role-capability-seam/specs/capability-seam/spec.md"
K = "opute.capability.kubernetes."
VERSION = "v1.31.8+k3s1"  # the page's test record
PROVIDER = "/opt/opute-e2e/providers/opute-provider-k3s"
RECIPE = "/home/opute/.config/opute-e2e/k3s-activate.yaml"
B_PORT = 3031
B = {"port": B_PORT, "auth": "secret:tokenB"}
CLUSTER = "cluster:local:e2e-k1"
SERVERS = ("e2e-k1", "e2e-k2", "e2e-k3")
DONE = ["completed", "failed", "cancelled"]
ADB_A = "/home/opute/.config/opute"
ADB_B = "/tmp/e2e-agents/b"
JOURNALS = "opute-bootstrap-mcp e2e-agent-b e2e-provider-k3s e2e-provider-k3s-b"

FILES = {**base.TOOL_FILES,
         "providerK3s": inp("${PARITY}/e2e/release/rust-endpoint/opute-provider-k3s-linux-x64"),
         # The M9 provider-real.k3s activation recipe, byte for byte: the provider's install manifest pins
        # its sha256 (91f0b596...).
         "activate": inp("${REPO}/evidence/e2e/journeys/inputs/k3s-activate.yaml")}



def descriptor(port: int) -> dict:
    return {"schema": "opute-provider-plugin.v1", "pluginId": "com.opute.k3s", "version": "1.0.2",
            "capabilities": [{"id": "opute.capability.kubernetes.v1", "version": 1}],
            "server": {"transport": "streamable_http", "endpoint": f"http://127.0.0.1:{port}/mcp"}}


def call(id_: str, tool: str, args: dict | None = None, host: dict | None = None, **kw) -> dict:
    return {"id": id_, "mcp": "tools/call", "tool": tool, "arguments": args or {}, **(host or {}), **kw}


def task(id_: str, tool: str, args: dict, host: dict | None = None, timeout: int = 600,
         capture: dict | None = None) -> list[dict]:
    """A task-aware call and its poll; the poll step (`id_`) holds the outcome."""
    poll = {"id": id_, "mcp": "tasks/get", "params": {"taskId": "${TID_" + id_ + "}"}, "meta": TASKS_META,
            **(host or {}), "until": {"path": "response.body.result.status", "in": DONE},
            "timeout": timeout, "interval": 3}
    if capture:
        poll["capture"] = capture
    return [call(id_ + "Call", tool, args, host, meta=TASKS_META,
                 capture={"TID_" + id_: "response.body.result.taskId"}), poll]


def res(step: str, path: str = "", **op) -> dict:
    """An assertion on a task's tool result."""
    return a(step, "response.body.result.result" + (("." + path) if path else ""), **op)


def direct(step: str, path: str = "", **op) -> dict:
    return a(step, "response.body.result" + (("." + path) if path else ""), **op)


def target(name: str) -> dict:
    return {"targetUri": f"container:local:{name}", "providerInstanceName": name, "instanceType": "container"}


CL = {"targetUri": CLUSTER, "providerInstanceName": "e2e-k1", "instanceType": "container"}
VM = {"image": "parity-noble", "instanceType": "container", "cpus": 2, "memory": "3GiB", "disk": "15GiB"}


def membership(id_: str, ready: int, timeout: int = 300) -> dict:
    return call(id_, K + "inspect-membership", {**CL, "expectedServers": 3, "requireEmbeddedEtcd": True},
                until={"path": "response.body.result.structuredContent.readyServers", "in": [ready]},
                timeout=timeout, interval=5)


def wait_tool(port: int, token: str) -> str:
    """Poll tools/list in the guest until the provider's provision tool is listed (at most 60 s)."""
    return ("python3 -c 'import sys, time, json; sys.path.insert(0, \"/opt/opute-e2e/guest\"); import mcpwire\n"
            f"for i in range(120):\n"
            f"    r = mcpwire.mcp({port}, sys.argv[1], \"tools/list\", request_id=i + 1)\n"
            "    names = [t[\"name\"] for t in ((r.get(\"body\") or {}).get(\"result\") or {}).get(\"tools\") or []]\n"
            f"    if \"{K}provision\" in names: break\n"
            "    time.sleep(0.5)\n"
            f"print(json.dumps({{\"listed\": \"{K}provision\" in names, \"tools\": len(names), \"polls\": i + 1}}))' "
            f"'{token}'")


# --- the build (journey 1's steps, reused by 2-4) -------------------------------------------------------

def build_steps(endpoint: bool = False) -> list[dict]:
    """`endpoint`: host A's provider runs with the cluster endpoint enabled (Q19); host B's never does."""
    enable = ("$( [ ${p%%:*} = 4320 ] && echo --setenv=OPUTE_K3S_CLUSTER_ENDPOINT=enabled ) " if endpoint else "")
    steps = base.TOOL_STEPS + [
        {"id": "providerCopy", "copy": "providerK3s", "to": PROVIDER, "mode": "0755"},
        {"id": "recipeCopy", "copy": "activate", "to": RECIPE, "mode": "0644"},
        {"id": "setup", "guest": (
            f"chown -R opute /home/opute/.config/opute-e2e && "
            f"for p in 4320:e2e-provider-k3s 4321:e2e-provider-k3s-b; do systemd-run --collect "
            f"--unit=${{p#*:}} --uid=opute --gid=opute --setenv=HOME=/home/opute "
            f"--setenv=OPUTE_PROVIDER_PORT=${{p%%:*}} {enable}--property=Restart=no {PROVIDER} || exit 1; done && "
            f"{T} agent start b --port {B_PORT} --id e2e-ha-b --env OPUTE_STANDALONE_ALLOW_MUTATIONS=true "
            "--env MCP_AUTH_TOKEN='${SECRET:tokenB}' >/dev/null && "
            "for i in $(seq 60); do ss -ltnH 'sport = :4320' | grep -q . && ss -ltnH 'sport = :4321' | grep -q . "
            "&& break; sleep 0.5; done; ss -ltnH '( sport = :4320 or sport = :4321 )' | wc -l")},
        {"id": "catalogB", "mcp": "tools/list", **B},
        call("installA", "opute.provider.install",
             {"descriptor": descriptor(4320), "activate": True, "recipeSource": RECIPE}),
        call("installB", "opute.provider.install",
             {"descriptor": descriptor(4321), "activate": True, "recipeSource": RECIPE}, B),
        {"id": "providerToolsA", "guest": wait_tool(3014, "${TOKEN}")},
        {"id": "providerToolsB", "guest": wait_tool(B_PORT, "${SECRET:tokenB}")},
        {"id": "catalogAfterA", "mcp": "tools/list"},
        {"id": "catalogAfterB", "mcp": "tools/list", **B},
        call("catalogA", "opute.capability.kubernetes.validate", {}),
        *task("vm1", "provision_vm", {"name": "e2e-k1", **VM}),
        call("info1", "get_vm_info", {"uri": "container:local:e2e-k1"},
             capture={"K1_IP": "response.body.result.structuredContent.ipv4[0]"}),
        *task("provision", K + "provision", {**target("e2e-k1"), "version": VERSION, "clusterInit": True}),
        call("listClusters", "list_compute_clusters", {}),
        *task("prepareHa", K + "prepare-ha",
              {**CL, "expectedServers": 3, "requireEmbeddedEtcd": True, "expectedRole": "server"}),
    ]
    for n in ("2", "3"):
        name = f"e2e-k{n}"
        steps += [
            *task(f"vm{n}", "provision_vm", {"name": name, **VM}, B),
            call(f"key{n}", K + "get-join-receiver-key", target(name), B,
                 capture={f"PUB{n}": "response.body.result.structuredContent.publicKey"}),
            *task(f"prepare{n}", K + "prepare-join",
                  {**CL, "destinationHostId": "e2e-ha-b", "destinationPublicKey": "${PUB" + n + "}",
                   "role": "server"},
                  capture={f"REF{n}": "response.body.result.result.structuredContent.redemptionRef",
                           f"SEALED{n}": {"path": "response.body.result.result.structuredContent.sealedJoinMaterial",
                                          "secret": True}}),
            *task(f"redeem{n}", K + "redeem-join",
                  {**target(name), "redemptionRef": "${REF" + n + "}", "sealedJoinMaterial": "${SEALED" + n + "}",
                   "destinationHostId": "e2e-ha-b", "sourceTargetUri": CLUSTER}, B),
            *task(f"join{n}", K + "join-node",
                  {**target(name), "redemptionRef": "${REF" + n + "}", "serverAddress": "https://${K1_IP}:6443",
                   "clusterInit": False, "role": "server"}, B, timeout=900),
        ]
    steps += [
        membership("members", 3),
        call("status", K + "status", CL),
        {"id": "joinToken", "guest": "incus exec e2e-k1 -- cat /var/lib/rancher/k3s/server/token | tr -d '\\n'",
         "capture": {"K3S_TOKEN": {"path": "stdout", "secret": True}}},
        {"id": "tokenLeaks", "guest": (
            f"{{ grep -a -r -o -F '${{K3S_TOKEN}}' {ADB_A} {ADB_B} 2>/dev/null; "
            f"for u in {JOURNALS}; do journalctl -u $u --no-pager -o cat; done | grep -o -F '${{K3S_TOKEN}}'; }} "
            "| wc -l")},
    ]
    return steps


def build_requirements() -> list[dict]:
    joined = []
    for n in ("2", "3"):
        joined += [res(f"redeem{n}", "isError", equals=False),
                   res(f"redeem{n}", "structuredContent.redeemed", equals=True),
                   res(f"redeem{n}", "structuredContent.joinMaterialExposed", equals=False),
                   res(f"join{n}", "isError", equals=False),
                   res(f"join{n}", "structuredContent.joined", equals=True),
                   res(f"join{n}", "structuredContent.datastoreMode", equals="embedded-etcd")]
    nodes = []
    for i, name in enumerate(SERVERS):
        nodes += [direct("members", f"structuredContent.nodes[{i}].name", equals=name),
                  direct("members", f"structuredContent.nodes[{i}].status", equals="Ready"),
                  direct("members", f"structuredContent.nodes[{i}].roles", contains="etcd"),
                  direct("members", f"structuredContent.nodes[{i}].version", equals=VERSION)]
    return [
        req(PAGE, "Three Incus server containers provisioned and configured through typed Host Agent operations "
                  "as a K3s cluster with embedded-etcd",
            ok("setup"), a("setup", "stdout", equals="2\n"),
            direct("installA", "isError", **{"in": [False, "<absent>"]}),
            direct("installB", "isError", **{"in": [False, "<absent>"]}),
            a("providerToolsA", "json.listed", equals=True), a("providerToolsB", "json.listed", equals=True),
            direct("catalogA", "isError", **{"in": [False, "<absent>"]}),
            res("vm1", "isError", equals=False), res("vm1", "structuredContent.uri", equals="container:local:e2e-k1"),
            res("vm2", "structuredContent.uri", equals="container:local:e2e-k2"),
            res("vm3", "structuredContent.uri", equals="container:local:e2e-k3"),
            res("provision", "isError", equals=False), res("provision", "structuredContent.provisioned", equals=True),
            res("provision", "structuredContent.version", equals=VERSION),
            direct("listClusters", "structuredContent.clusters[0].uri", equals=CLUSTER),
            res("prepareHa", "structuredContent.readyForServerPromotion", equals=True),
            res("prepareHa", "structuredContent.datastoreMode", equals="embedded-etcd"),
            *joined,
            direct("members", "structuredContent.serverCount", equals=3),
            direct("members", "structuredContent.readyServers", equals=3),
            direct("members", "structuredContent.datastoreMode", equals="embedded-etcd"),
            direct("members", "structuredContent.clusterIdentityKnown", equals=True),
            *nodes,
            direct("status", "structuredContent.readyNodeCount", equals=3),
            scenario="3 Ready servers and 3 etcd members after joining through the sealed handshake"),
        req(SPEC, "Durable and redacted observations",
            res("prepare2", "structuredContent.joinMaterialExposed", equals=False),
            res("prepare2", "structuredContent.secretDelivery", equals="host-agent-handshake"),
            res("prepare3", "structuredContent.joinMaterialExposed", equals=False),
            a("key2", "response.body.result.structuredContent.keyAlgorithm", equals="RSA-OAEP-SHA256"),
            ok("joinToken"), a("joinToken", "stdout", equals="<secret:K3S_TOKEN>"),
            ok("tokenLeaks"), a("tokenLeaks", "stdout", equals="0\n"),
            scenario="Secret-bearing input: the K3s join token reaches no Host Agent state, journal or client-"
                     "visible record; it crosses hosts only sealed to the destination's key"),
    ]


def cleanup_steps(typed: bool = True) -> list[dict]:
    """Typed teardown first (unless the journey already did it); then its own setup is undone."""
    teardown = [*task("cleanRemove", K + "remove", CL),
                *[s for name in SERVERS for s in task(f"cleanDelete{name[-1]}", "delete_vm",
                                                      {"uri": f"container:local:{name}"})]] if typed else []
    return teardown + [
        {"id": "cleanSetup", "guest": (
            f"systemctl stop e2e-provider-k3s e2e-provider-k3s-b 2>/dev/null; {T} agent remove b >/dev/null; "
            "for n in e2e-k1 e2e-k2 e2e-k3; do incus delete -f $n 2>/dev/null; done; "
            "incus storage volume list default -f csv | awk -F, '$1==\"image\"{print $2}' | "
            "xargs -r -I{} incus storage volume delete default image/{}; "
            f"rm -rf /home/opute/.config/opute-e2e /opt/opute-e2e/providers; true")},
    ]


EFFECTS = ["incus/instances/e2e-k*", "incus/volumes/*", "incus/storage/default/used_by*",
           "incus/profiles/default/used_by*", "incus/networks/incusbr0/used_by*",
           "files//home/opute/.config/opute-e2e*", "files//home/opute/.config/incus*",
           "files//opt/opute-e2e/providers*", "system/units/e2e-*", "system/unitFiles/e2e-*",
           "system/units/sys-devices-virtual-net-veth*", "system/units/sys-subsystem-net-devices-veth*"]

INPUTS = {"agentA": base.AGENT + ", with the Rust k3s provider 1.0.2 from the release (e2e-provider-k3s, port 4320)",
          "agentB": f"a transient Rust agent e2e-ha-b on port {B_PORT} (e2etool agent start), with its own Rust k3s "
                    "provider (e2e-provider-k3s-b, port 4321)",
          "k3s": f"K3s {VERSION}, downloaded by the provider from the K3s GitHub release and checked against its "
                 "checksums; images pulled by K3s from public registries",
          "files": FILES}


def journey(id_, description, requirements, steps, cleanup, allow=(), minutes=25):
    return base.journey(id_, description, INPUTS, requirements, steps, cleanup,
                        allow=EFFECTS + list(allow), minutes=minutes, gate=GATE)


# --- 1. build ---------------------------------------------------------------------------------------------

HA_FIRST = {**CL, "expectedServers": 3, "requireEmbeddedEtcd": True, "expectedRole": "server"}


def j1() -> dict:
    steps = build_steps()
    # The goal's order: prepare-ha and ensure-ha-endpoint before the first server is provisioned. Both
    # target a cluster URI, so they run here, after the guest exists but before K3s does.
    first = steps.index(next(s for s in steps if s["id"] == "provisionCall"))
    steps[first:first] = [*task("prepareHaFirst", K + "prepare-ha", HA_FIRST),
                          *task("endpointFirst", K + "ensure-ha-endpoint", {**CL, "endpoint": "https://${K1_IP}:6443"})]
    # The same call once the three servers are members: the only endpoint the VM can offer is a server's
    # own K3s API, whose certificate the host's roots do not trust.
    steps += task("endpoint", K + "ensure-ha-endpoint", {**CL, "endpoint": "https://${K1_IP}:6443"})
    reqs = build_requirements() + [
        req(SPEC, "Typed target and effect admission",
            res("prepareHaFirst", "isError", equals=True),
            res("prepareHaFirst", "structuredContent.code", equals="resource_binding"),
            res("prepareHaFirst", "structuredContent.owner", equals="admission"),
            res("prepareHaFirst", "structuredContent.message", matches="resource not found: cluster:local:e2e-k1$"),
            res("endpointFirst", "isError", equals=True),
            res("endpointFirst", "structuredContent.code", equals="resource_binding"),
            res("endpointFirst", "structuredContent.message", matches="resource not found: cluster:local:e2e-k1$"),
            scenario="The goal's order (prepare-ha, then ensure-ha-endpoint, then provision): before the first "
                     "server is provisioned the cluster URI is unknown, and both calls are refused before "
                     "execution; no success is invented from an unknown target"),
        req(CATALOG, "ensure-ha-endpoint verifies a caller's stable HA endpoint; it creates none",
            res("endpoint", "isError", equals=True),
            res("endpoint", "content[0].text", matches="probe stable HA endpoint: .*invalid peer certificate: "
                                                         "UnknownIssuer"),
            scenario="Against the only endpoint the VM can offer, a server's own K3s API, the probe fails closed "
                     "on the certificate. No typed operation creates a stable endpoint (an owned gap); the "
                     "page's test record also had none"),
    ]
    return journey(
        "e2e3a.build",
        "Build: three K3s server Incus containers with embedded etcd, only through typed Host Agent calls. Host "
        "A provisions e2e-k1 (clusterInit) and checks prepare-ha; host B provisions e2e-k2 and e2e-k3, and each "
        "joins through prepare-join on A (join material sealed to B's receiver key), redeem-join on B (unsealed "
        "by B's provider) and join-node on B. The goal's prepare-ha and ensure-ha-endpoint before provisioning "
        "are refused (unknown cluster target); ensure-ha-endpoint against the built cluster is recorded. Pass: "
        "3 Ready servers, 3 etcd members, and the join token in no record, row or log.",
        reqs, steps, cleanup_steps())


# --- 2. workload ------------------------------------------------------------------------------------------

def workload_steps() -> list[dict]:
    return [
        call("chart", "install_cluster_package",
             {"uri": "container:local:e2e-k1", "vmName": "e2e-k1", "releaseName": "e2e-web", "namespace": "e2e-web",
              "chart": "podinfo", "repo": "https://stefanprodan.github.io/podinfo",
              "values": {"replicaCount": 2, "service": {"type": "NodePort", "nodePort": 30080}}}),
        call("rollout", "exec_cluster_command",
             {"uri": CLUSTER, "command": "rollout",
              "args": ["status", "deployment/e2e-web-podinfo", "-n", "e2e-web", "--timeout=60s"]},
             until={"path": "response.body.result.structuredContent.exitCode", "in": [0]}, timeout=480, interval=5),
        *task("secret", "put_cluster_secret",
              {"uri": CLUSTER, "namespace": "e2e-web", "name": "e2e-secret", "data": {"apiKey": "${SECRET:apiValue}"}}),
        call("secretKeysTool", "list_secret_keys", {"uri": CLUSTER, "namespace": "e2e-web", "secretName": "e2e-secret"}),
        call("secretKeys", "exec_cluster_command",
             {"uri": CLUSTER, "command": "get",
              "args": ["secret", "e2e-secret", "-n", "e2e-web", "-o", "go-template={{range $k, $v := .data}}{{$k}} {{end}}"]}),
        call("pods", "exec_cluster_command",
             {"uri": CLUSTER, "command": "exec",
              "args": ["-n", "e2e-web", "deployment/e2e-web-podinfo", "--", "sh", "-c", "echo e2e-exec-ok"]}),
        call("services", "list_services", {"uri": CLUSTER, "namespace": "e2e-web"}),
        call("info2", "get_vm_info", {"uri": "container:local:e2e-k2"}, B,
             capture={"K2_IP": "response.body.result.structuredContent.ipv4[0]"}),
        call("info3", "get_vm_info", {"uri": "container:local:e2e-k3"}, B,
             capture={"K3_IP": "response.body.result.structuredContent.ipv4[0]"}),
        {"id": "serve", "guest": "for ip in ${K1_IP} ${K2_IP} ${K3_IP}; do curl -s -o /dev/null -w '%{http_code} ' "
                                 "--max-time 10 http://$ip:30080/; done"},
    ]


def workload_requirements() -> list[dict]:
    return [
        req(CATALOG, "install_cluster_package, put_cluster_secret, exec_cluster_command and list_services on the "
                     "HA cluster",
            direct("chart", "structuredContent.status", equals="installing"),
            direct("rollout", "structuredContent.exitCode", equals=0),
            res("secret", "isError", equals=False), res("secret", "structuredContent.configured", equals=True),
            direct("secretKeys", "structuredContent.stdout", equals="apiKey "),
            direct("pods", "structuredContent.exitCode", equals=0),
            direct("pods", "structuredContent.stdout", equals="e2e-exec-ok\n"),
            direct("services", "structuredContent.services[0].name", equals="e2e-web-podinfo"),
            a("serve", "stdout", equals="200 200 200 "),
            scenario="A Helm chart installed, a secret put, a command run in a pod, and the service exposed inside "
                     "the VM answering HTTP on every server"),
        req(SPEC, "Durable and redacted observations",
            res("secret", "content[0].text", equals="Kubernetes Secret configured."),
            res("secret", "structuredContent.data", absent=True),
            scenario="Secret-bearing input: the secret's value is in no result; its key is read back without it"),
        req(SEAM_SPEC, "Every capability has its three roles",
            direct("secretKeysTool", "isError", equals=True),
            direct("secretKeysTool", "structuredContent.code", equals="not_implemented"),
            direct("secretKeysTool", "structuredContent.owner", equals="capability"),
            scenario="The goal's pass check (list_secret_keys shows the key) cannot pass: list_secret_keys is a "
                     "definition without a provider, registered unported in evidence/seam/final/report.json, and "
                     "fails closed with not_implemented as the spec requires. An owned gap, not added here; the "
                     "key is read back through exec_cluster_command"),
    ]


def j2() -> dict:
    return journey(
        "e2e3a.workload",
        "Workload on the HA cluster built as in e2e3a.build: install the podinfo Helm chart as a NodePort service "
        "(install_cluster_package), put a secret, exec in a pod, list services, and request the service from the VM "
        "on every server. list_secret_keys is published but unported, so the key is read back with a template "
        "that prints keys only.",
        build_requirements() + workload_requirements(), build_steps() + workload_steps(), cleanup_steps())


# --- 3. lose one server -----------------------------------------------------------------------------------

CM_DOWN = ("apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: e2e-during-failure\n  namespace: e2e-web\n"
           "data:\n  written: during-k2-down\n")
SERVE_LOOP = ("start=$(date +%s); n=0; while [ $n -lt 10 ] && [ $(( $(date +%s) - start )) -lt 120 ]; do "
              "ok=1; for ip in ${K1_IP} ${K3_IP}; do code=$(curl -s -o /dev/null -w '%{http_code}' --max-time 5 "
              "http://$ip:30080/); [ \"$code\" = 200 ] || ok=0; done; if [ $ok = 1 ]; then n=$((n+1)); else n=0; "
              "fi; sleep 1; done; printf '{\"consecutive\": %d, \"seconds\": %d}' $n $(( $(date +%s) - start ))")


def j3() -> dict:
    steps = build_steps() + workload_steps() + [
        *task("stop2", "stop_vm", {"uri": "container:local:e2e-k2"}),
        call("degraded", K + "status", CL,
             until={"path": "response.body.result.structuredContent.readyNodeCount", "in": [2]}, timeout=180,
             interval=5),
        call("degradedMembers", K + "inspect-membership", {**CL, "expectedServers": 3}),
        *task("write", "apply_manifest", {"uri": CLUSTER, "manifest": CM_DOWN}),
        call("read", "get_cluster_resource",
             {"uri": CLUSTER, "kind": "configmap", "namespace": "e2e-web", "resourceName": "e2e-during-failure"}),
        {"id": "serveDegraded", "guest": SERVE_LOOP, "timeout": 200},
        *task("start2", "start_vm", {"uri": "container:local:e2e-k2"}),
        membership("rejoined", 3),
        call("readAfter", "get_cluster_resource",
             {"uri": CLUSTER, "kind": "configmap", "namespace": "e2e-web", "resourceName": "e2e-during-failure"}),
    ]
    reqs = build_requirements() + workload_requirements() + [
        req(PAGE, "After one guest stopped, 2 of 3 Kubernetes nodes remained Ready and a typed API write and read "
                  "succeeded; starting the guest restored 3 of 3 Ready nodes",
            res("stop2", "isError", equals=False), res("stop2", "structuredContent.status", equals="stopped"),
            direct("degraded", "structuredContent.readyNodeCount", equals=2),
            direct("degraded", "structuredContent.nodeCount", equals=3),
            direct("degradedMembers", "structuredContent.nodes[1].name", equals="e2e-k2"),
            direct("degradedMembers", "structuredContent.nodes[1].ready", equals=False),
            direct("degradedMembers", "structuredContent.readyServers", equals=2),
            res("write", "isError", equals=False), res("write", "structuredContent.applied", equals=True),
            direct("read", "structuredContent.json", matches="during-k2-down"),
            res("start2", "structuredContent.status", equals="running"),
            direct("rejoined", "structuredContent.readyServers", equals=3),
            direct("rejoined", "structuredContent.serverCount", equals=3),
            direct("rejoined", "structuredContent.nodes[1].status", equals="Ready"),
            direct("rejoined", "structuredContent.nodes[1].roles", contains="etcd"),
            direct("readAfter", "structuredContent.json", matches="during-k2-down"),
            scenario="One Incus guest/server failure on one physical host, stopped and started through typed calls"),
        req(PAGE, "Application serving is recorded separately from control-plane writes",
            a("serveDegraded", "json.consecutive", equals=10),
            a("serveDegraded", "json.seconds", lte=120),
            scenario="The service answers 10 consecutive rounds on both surviving servers within 120 s of the "
                     "server being NotReady (beyond the page's tested scope; recorded, with the time it took)"),
    ]
    return journey(
        "e2e3a.lose-one",
        "Lose one server on the HA cluster built and loaded as in e2e3a.build and e2e3a.workload: stop e2e-k2 with "
        "stop_vm. Pass: 2 of 3 nodes stay Ready, a typed ConfigMap write and read succeed, the service still "
        "answers on the surviving servers; after start_vm, e2e-k2 is Ready and an etcd member again.",
        reqs, steps, cleanup_steps(), minutes=28)


# --- 4. quorum and removal --------------------------------------------------------------------------------

CM_BEFORE = ("apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: e2e-before-loss\n  namespace: default\n"
             "data:\n  written: before-quorum-loss\n")
CM_NOQ = ("apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: e2e-no-quorum\n  namespace: default\n"
          "data:\n  written: without-quorum\n")


def j4() -> dict:
    steps = build_steps() + [
        *task("writeBefore", "apply_manifest", {"uri": CLUSTER, "manifest": CM_BEFORE}),
        *task("stop2", "stop_vm", {"uri": "container:local:e2e-k2"}),
        *task("stop3", "stop_vm", {"uri": "container:local:e2e-k3"}),
        {"id": "settle", "guest": "sleep 20"},
        *task("writeNoQuorum", "apply_manifest", {"uri": CLUSTER, "manifest": CM_NOQ}),
        call("statusNoQuorum", K + "status", CL),
        *task("recover", K + "recover-quorum", {**CL, "acknowledgeSingleMemberReset": True}, timeout=900),
        call("readRecovered", "get_cluster_resource",
             {"uri": CLUSTER, "kind": "configmap", "namespace": "default", "resourceName": "e2e-before-loss"}),
        # D28 (Q17): remove-node itself waits for recovery to settle (no wait here)
        *task("removeNode2", K + "remove-node", {**CL, "nodeName": "e2e-k2", "role": "server"}, timeout=600),
        *task("removeNode3", K + "remove-node", {**CL, "nodeName": "e2e-k3", "role": "server"}, timeout=600),
        call("membersAfter", K + "inspect-membership", {**CL, "requireEmbeddedEtcd": True}),
        *task("remove", K + "remove", CL),
        call("clustersAfter", "list_compute_clusters", {}),
        *[s for name in SERVERS for s in task(f"delete{name[-1]}", "delete_vm", {"uri": f"container:local:{name}"})],
        call("vmsAfter", "list_vms", {}),
    ]
    reqs = build_requirements() + [
        req(PAGE, "K3s datastore quorum: with a majority of servers lost, Kubernetes writes are not available",
            res("writeBefore", "isError", equals=False),
            res("stop2", "structuredContent.status", equals="stopped"),
            res("stop3", "structuredContent.status", equals="stopped"),
            res("writeNoQuorum", "isError", equals=True),
            direct("statusNoQuorum", "isError", equals=True),
            scenario="Two of three servers stopped: a typed write and the status read fail closed with an error "
                     "result, never a success"),
        req(CATALOG, "recover-quorum restores a single-member etcd majority",
            res("recover", "isError", equals=False),
            res("recover", "structuredContent.quorumRestored", equals=True),
            direct("readRecovered", "structuredContent.json", matches="before-quorum-loss"),
            scenario="After the acknowledged reset the data written before the loss reads back"),
        req(Q17_SPEC, "Dead servers are retired once quorum recovery settles",
            res("removeNode2", "isError", equals=False),
            res("removeNode2", "structuredContent.removedNodeWasReady", equals=False),
            res("removeNode3", "isError", equals=False),
            res("removeNode3", "structuredContent.removedNodeWasReady", equals=False),
            direct("membersAfter", "structuredContent.serverCount", equals=1),
            direct("membersAfter", "structuredContent.readyServers", equals=1),
            scenario="Each stopped server is retired only once it is observed NotReady, and the cluster keeps "
                     "serving from the survivor (D28)"),
        req(CATALOG, "remove and delete_vm leave no cluster or guest",
            res("remove", "isError", equals=False), res("remove", "structuredContent.removed", equals=True),
            direct("clustersAfter", "structuredContent.total", equals=0),
            res("delete1", "structuredContent.deleted", equals=True),
            res("delete2", "structuredContent.deleted", equals=True),
            res("delete3", "structuredContent.deleted", equals=True),
            direct("vmsAfter", "structuredContent.vms", **{"in": [[], "<absent>"]}),
            scenario="The cluster is gone; the VM diff after cleanup is empty"),
    ]
    return journey(
        "e2e3a.quorum-removal",
        "Quorum loss and removal on the HA cluster built as in e2e3a.build: stop e2e-k2 and e2e-k3; typed calls "
        "must fail closed. Recover quorum on e2e-k1 with the acknowledged single-member reset, wait until the "
        "stopped servers are NotReady, retire e2e-k2, then remove the cluster and delete the guests through "
        "typed calls. Pass: the cluster is gone and nothing remains after cleanup.",
        reqs, steps, cleanup_steps(typed=False), minutes=28)


# --- 5. the optional cluster endpoint (Q19) ----------------------------------------------------------------

EP = {"endpoint": f"https://{HOST}:6443", "caBundleRef": "${CAREF}"}


def count(step: str, path: str, n: int) -> list[dict]:
    """Exactly n items in a task result's list."""
    return [res(step, f"{path}[{n - 1}]", exists=True), res(step, f"{path}[{n}]", absent=True)]


def sans(names: tuple[str, ...]) -> str:
    """Per server: 1 when its serving certificate (at its own address, SNI the hostname) lists the hostname."""
    return " ".join(
        # eth0's IPv4 only: a K3s server's address column also lists its CNI interfaces.
        f"ip=$(incus list {n} -f json | python3 -c \"import json,sys; i=json.load(sys.stdin)[0]; "
        f"print([a['address'] for a in i['state']['network']['eth0']['addresses'] if a['family']=='inet'][0])\"); "
        f"echo | timeout 10 openssl s_client -connect $ip:6443 "
        f"-servername {HOST} 2>/dev/null | openssl x509 -noout -ext subjectAltName 2>/dev/null | "
        f"grep -c 'DNS:{HOST}';" for n in names)


def resolved() -> str:
    """The hostname as a client on the bridge resolves it, and the bridge's host-record lines."""
    return (f"incus exec e2e-k1 -- getent ahostsv4 {HOST} | awk '{{print $1}}' | sort -u | tr '\\n' ' '; echo; "
            f"incus network get incusbr0 raw.dnsmasq | grep -c 'host-record={HOST},'")


def listed(port: int, token: str, name: str) -> str:
    return ("python3 -c 'import sys, json; sys.path.insert(0, \"/opt/opute-e2e/guest\"); import mcpwire\n"
            f"r = mcpwire.mcp({port}, sys.argv[1], \"tools/list\")\n"
            "names = [t[\"name\"] for t in ((r.get(\"body\") or {}).get(\"result\") or {}).get(\"tools\") or []]\n"
            f"print(json.dumps({{\"listed\": \"{name}\" in names}}))' '{token}'")


THROUGH = (f"incus exec e2e-k1 -- sh -c 'sed \"s#https://127.0.0.1:6443#https://{HOST}:6443#\" "
           "/etc/rancher/k3s/k3s.yaml > /tmp/ep.yaml; start=$(date +%s); for i in $(seq 60); do "
           "k3s kubectl --kubeconfig /tmp/ep.yaml get --raw /version >/dev/null 2>&1 && "
           "k3s kubectl --kubeconfig /tmp/ep.yaml create configmap e2e-through-endpoint -n default "
           "--from-literal=via=endpoint >/dev/null 2>&1 && break; sleep 2; done; "
           "echo seconds=$(( $(date +%s) - start )); k3s kubectl --kubeconfig /tmp/ep.yaml get configmap "
           "e2e-through-endpoint -n default -o jsonpath={.data.via}; rm -f /tmp/ep.yaml'")


def j5() -> dict:
    ensure = K + "ensure-cluster-endpoint"
    steps = build_steps(endpoint=True) + [
        {"id": "listedA", "guest": listed(3014, "${TOKEN}", ensure)},
        {"id": "listedB", "guest": listed(B_PORT, "${SECRET:tokenB}", ensure)},
        {"id": "untouched", "guest": sans(SERVERS) + " " + resolved()},
        call("info3", "get_vm_info", {"uri": "container:local:e2e-k3"},
             capture={"K3_IP": "response.body.result.structuredContent.ipv4[0]"}),
        # opt in
        *task("optIn", ensure, {**CL, "hostname": HOST}, timeout=900,
              capture={"CAREF": "response.body.result.result.structuredContent.caBundleRef"}),
        {"id": "sansIn", "guest": sans(SERVERS)},
        {"id": "resolvesIn", "guest": resolved()},
        *task("verifyIn", K + "ensure-ha-endpoint", {**CL, **EP}),
        *task("optInAgain", ensure, {**CL, "hostname": HOST}, timeout=900),
        # lose one server
        *task("stop3", "stop_vm", {"uri": "container:local:e2e-k3"}),
        membership("degraded", 2),
        {"id": "through", "guest": THROUGH, "timeout": 200},
        *task("verifyDegraded", K + "ensure-ha-endpoint", {**CL, **EP}),
        # retire it
        *task("retire", K + "remove-node", {**CL, "nodeName": "e2e-k3", "role": "server"}, timeout=600),
        {"id": "resolvesRetired", "guest": resolved() + "; incus exec e2e-k1 -- getent ahostsv4 " + HOST +
                                           " | awk -v ip='${K3_IP}' '$1 == ip' | wc -l"},
        *task("verifyRetired", K + "ensure-ha-endpoint", {**CL, **EP}),
        # opt out
        *task("optOut", K + "remove-cluster-endpoint", CL, timeout=900),
        {"id": "sansOut", "guest": sans(SERVERS[:2])},
        {"id": "resolvesOut", "guest": f"incus exec e2e-k1 -- getent ahostsv4 {HOST} | wc -l; "
                                       f"incus network get incusbr0 raw.dnsmasq | grep -c '{HOST}'"},
        {"id": "recordOut", "guest": "incus exec e2e-k1 -- k3s kubectl -n kube-system get configmap "
                                     "opute-cluster-endpoint 2>&1 | grep -c NotFound"},
        {"id": "directOut", "guest": "incus exec e2e-k1 -- k3s kubectl get nodes -o name | wc -l"},
        *task("verifyOut", K + "ensure-ha-endpoint", {**CL, **EP}),
        *task("optOutAgain", K + "remove-cluster-endpoint", CL),
    ]
    reqs = build_requirements() + [
        req(ENDPOINT_SPEC, "The HA registration endpoint is optional and off by default",
            a("listedA", "json.listed", equals=True),
            a("listedB", "json.listed", equals=False),
            a("untouched", "stdout", equals="0\n0\n0\n\n0\n"),
            scenario="The capability is listed only on the host whose provider enables it; the cluster built "
                     "without opting in has no certificate name, resolver entry or record for the hostname"),
        req(ENDPOINT_SPEC, "An opted-in endpoint is stable across server loss",
            res("optIn", "isError", equals=False),
            res("optIn", "structuredContent.endpoint", equals=f"https://{HOST}:6443"),
            res("optIn", "structuredContent.caBundleRef", equals="cluster-endpoint:local:e2e-k1"),
            res("optIn", "structuredContent.restartedServers", equals=list(SERVERS)),
            *count("optIn", "structuredContent.resolvesTo", 3),
            a("sansIn", "stdout", equals="1\n1\n1\n"),
            a("resolvesIn", "stdout", matches="^(\\S+ ){3}\n3\n$"),
            res("verifyIn", "isError", equals=False),
            res("verifyIn", "structuredContent.endpointAvailable", equals=True),
            *count("verifyIn", "structuredContent.resolvedAddresses", 3),
            res("optInAgain", "structuredContent.changed", equals=False),
            res("optInAgain", "structuredContent.restartedServers", equals=[]),
            res("stop3", "structuredContent.status", equals="stopped"),
            direct("degraded", "structuredContent.readyServers", equals=2),
            a("through", "stdout", matches="^seconds=\\d+\nendpoint$"),
            res("verifyDegraded", "structuredContent.endpointAvailable", equals=True),
            scenario="Opting in at k3s.e2e.test: every server's certificate lists it, it resolves on the bridge to "
                     "the three servers, ensure-ha-endpoint verifies it with the CA reference (no UnknownIssuer), "
                     "a repeat restarts nothing; with e2e-k3 stopped a read and a typed write through the hostname "
                     "succeed and the endpoint still verifies"),
        req(ENDPOINT_SPEC, "An opted-in endpoint follows membership",
            res("retire", "isError", equals=False),
            *count("retire", "structuredContent.endpointResolvesTo", 2),
            a("resolvesRetired", "stdout", matches="^(\\S+ ){2}\n2\n0\n$"),
            res("verifyRetired", "structuredContent.endpointAvailable", equals=True),
            scenario="Retiring the dead e2e-k3 removes it from resolution in the same operation, and the endpoint "
                     "still verifies"),
        req(ENDPOINT_SPEC, "Opting out is a typed operation",
            res("optOut", "isError", equals=False),
            res("optOut", "structuredContent.removed", equals=True),
            res("optOut", "structuredContent.restartedServers", equals=list(SERVERS[:2])),
            a("sansOut", "stdout", equals="0\n0\n"),
            a("resolvesOut", "stdout", equals="0\n0\n"),
            a("recordOut", "stdout", equals="1\n"),
            a("directOut", "stdout", equals="2\n"),
            res("verifyOut", "isError", equals=True),
            res("optOutAgain", "structuredContent.changed", equals=False),
            scenario="After opting out no certificate, resolver entry or record names the hostname, the CA "
                     "reference resolves to nothing, a client addressing a server directly still works, and a "
                     "repeat reports unchanged"),
        req(ENDPOINT_SPEC, "Changing TLS names is an explicit, bounded effect",
            res("optIn", "structuredContent.restartedServers", equals=list(SERVERS)),
            res("optOut", "structuredContent.restartedServers", equals=list(SERVERS[:2])),
            scenario="Opting in and out restarted the Ready servers one at a time, each Ready again before the "
                     "next (the operation's own order); a server that does not return is covered by the unit test"),
    ]
    cleanup = cleanup_steps() + [{"id": "cleanDns", "guest": (
        "incus network get incusbr0 raw.dnsmasq | grep -q opute-cluster-endpoint && "
        "incus network unset incusbr0 raw.dnsmasq; true")}]
    return journey(
        "e2e3a.endpoint",
        "The optional cluster endpoint (Q19) on the HA cluster built as in e2e3a.build, with host A's provider "
        "enabled for it and host B's not: opt in at k3s.e2e.test, verify certificates, bridge resolution and "
        "ensure-ha-endpoint with the CA reference; stop e2e-k3 and use the hostname; retire e2e-k3; opt out.",
        reqs, steps, cleanup, allow=["incus/networks/incusbr0*"], minutes=29)


JOURNEYS = [j1, j2, j3, j4, j5]


def render() -> dict[str, str]:
    return {f"{j['id']}.json": json.dumps(j, indent=1) + "\n" for j in (make() for make in JOURNEYS)}


if __name__ == "__main__":
    for name, text in render().items():
        (HERE / name).write_text(text)
        print(name)
