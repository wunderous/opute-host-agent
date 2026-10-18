#!/usr/bin/env python3
"""Generate scenarios-t2/cluster-domain.json: the M8c cluster-domain T2 matrix.

Run from tools/parity: python3 cluster_t2_scenarios.py

Every scenario boots its twins from the k3s golden (`python3 -m parity t2
golden-k3s`): the nested-Incus golden plus k1, a privileged Incus container
running a single-node k3s server (airgap system images, the helm
controller disabled so a HelmChart is an inert object), the
registry/opute-registry Deployment, curl in k1 and helm on the twin. The
pinned, unchanged Go k3s provider runs in the twin as a transient unit and
is installed through opute.provider.install exactly as in T1; it reaches
k1 through `incus exec`. Nothing outside the twin is touched: no shared
Incus, Kubernetes, tunnel or deployment state, and no real platform.

The snapshots add k1's Kubernetes objects (labels, annotations, spec,
ConfigMap data, Secret values by sha256), the registry's tags, k1's images
and k1's agent files and units to the twin's files, units and firewall. A
teardown snapshot against the first baseline records residue; Go's
residue is listed in "residue" and must be reproduced.
"""
import copy
import json
import sys

import t2_scenarios as hd

SNAP, STOP, call, task_call, shell, done = hd.snap, hd.STOP, hd.call, hd.task_call, hd.shell, hd.done
D19 = hd.D19
K1 = "cluster:local:k1"
# install_helm_chart binds the instance that runs the cluster: in the twin,
# the k1 container.
CT1 = "container:local:k1"
PROVIDER_PORT = "3015"
ROOT = "/opt/parity-t2"
RECIPE_PATH = ROOT + "/recipes/activate.yaml"
COMMANDS = ["incus", "helm"]
REAL_K3S = next(s for s in json.load(open("scenarios/provider-real.json")) if s["id"] == "provider-real.k3s")


def k1(*argv):
    return ["incus", "exec", "k1", "--"] + list(argv)


def kubectl(*argv):
    return k1("k3s", "kubectl", *argv)


# The provider, as T1 runs it: the pinned binary on loopback, its own user's
# incus client. The incus client configuration is created before any
# baseline (the M8b note on INCUS_CLIENT), so it is not attributed to a call.
PROVIDER = [
    # Names and states only: k1's address list grows by the cni0 and
    # flannel.1 interfaces whenever k3s networking comes up, which is twin
    # timing, not an effect (M8c round 3, Go/Go 1 in 20).
    shell("incusClient", ["sudo", "-u", "ubuntu", "-H", "incus", "list", "--format", "csv", "-c", "ns"]),
    shell("provider", ["systemd-run", "--unit=parity-provider", "-p", "User=ubuntu", "-p", "Group=ubuntu",
                       "-p", f"WorkingDirectory={ROOT}", "-E", f"OPUTE_PROVIDER_PORT={PROVIDER_PORT}",
                       "-E", "HOME=/home/ubuntu", f"{ROOT}/opute-provider-k3s", "serve"]),
    shell("providerListening", ["sh", "-c", f"for i in $(seq 1 100); do ss -ltn | grep -q '127.0.0.1:{PROVIDER_PORT} ' "
                                            "&& exit 0; sleep 0.2; done; exit 1"]),
    # Compared by its exit status only: whether kubectl prints a rollout's
    # progress before it completes is twin timing (M8d round 2, Go/Go 1 in 20).
    shell("k3sReady", ["sh", "-c",
                       "(incus exec k1 -- timeout 600 sh -c 'until k3s kubectl get --raw=/readyz >/dev/null 2>&1; "
                       "do sleep 2; done' && incus exec k1 -- k3s kubectl wait --for=condition=Ready node/k1 "
                       "--timeout=600s && for d in kube-system/coredns kube-system/local-path-provisioner "
                       "registry/opute-registry; do incus exec k1 -- k3s kubectl -n ${d%/*} rollout status "
                       "deploy/${d#*/} --timeout=600s || exit 1; done) >/dev/null"]),
]


def install():
    steps = {s["as"]: s for s in REAL_K3S["steps"]}
    recipe = copy.deepcopy(steps["recipe"])
    recipe["write"]["path"] = RECIPE_PATH
    recipe["write"]["owner"] = "ubuntu:ubuntu"
    out = [recipe]
    # get_runtime_recipe_run is left out: polled the instant the install
    # returns, it races the catalog republished on activation (an unknown
    # tool on one twin or both); the task carries the run's outcome.
    for label in ("install", "installTask"):
        step = copy.deepcopy(steps[label])
        text = json.dumps(step).replace("${PROVIDER_PORT}", PROVIDER_PORT).replace("${SANDBOX}/recipes/activate.yaml",
                                                                                   RECIPE_PATH)
        step = json.loads(text)
        step["mcp"].pop("expand", None)
        step.pop("requires", None)
        out.append(step)
    return out


# The helm controller is disabled in the golden, so k3s registers no
# HelmChart type; the harness registers a schema-less one before the
# baseline, and a HelmChart is then an inert object the snapshot compares.
HELMCHART_CRD = """apiVersion: apiextensions.k8s.io/v1
kind: CustomResourceDefinition
metadata:
  name: helmcharts.helm.cattle.io
spec:
  group: helm.cattle.io
  scope: Namespaced
  names: {plural: helmcharts, singular: helmchart, kind: HelmChart}
  versions:
    - name: v1
      served: true
      storage: true
      schema:
        openAPIV3Schema: {type: object, x-kubernetes-preserve-unknown-fields: true}
"""
CRD = [shell("helmChartType", ["sh", "-c", "printf '%s' \"$CRD\" | incus exec k1 -- k3s kubectl apply -f - && "
                                           "incus exec k1 -- k3s kubectl wait --for=condition=Established "
                                           "crd/helmcharts.helm.cattle.io --timeout=120s"])]
CRD[0]["shell"]["argv"] = ["env", "CRD=" + HELMCHART_CRD] + CRD[0]["shell"]["argv"]


def setup(**env):
    return [{"as": "serve", "serve": {"env": env}}, *PROVIDER, *CRD, *install()]


def scenario(id_, steps, anchors, residue, masks=(), sets=(), rejected_only=False, divergences=()):
    out = hd.scenario(id_, steps, anchors, residue, masks=masks, sets=sets, rejected_only=rejected_only,
                      divergences=divergences)
    out["id"] = "t2.cluster-domain." + id_
    out["owner"] = "kubernetes"
    out["golden"] = "k3s"
    out["commands"] = COMMANDS
    # The install journey's own comparison (provider-real.k3s).
    out["compare"]["masks"] += [m for m in REAL_K3S["compare"]["masks"]
                                if m["path"] not in [x["path"] for x in out["compare"]["masks"]]]
    out["compare"]["sets"] = out["compare"].get("sets", []) + REAL_K3S["compare"]["sets"]
    out["compare"]["omitempty"] = REAL_K3S["compare"]["omitempty"]
    out["compare"]["masks"] += SETUP_MASKS
    # A task-aware call refused after it was accepted carries its task id.
    out["compare"]["masks"].append({"path": ["steps", "*", "body", "result", "taskId"], "type": "uuid",
                                    "match": "[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}",
                                    "optional": True, "reason": "task id this twin's agent assigned to a call "
                                                                "no later step captures"})
    out["compare"]["divergences"] = sorted(set(out["compare"].get("divergences", [])) | {D19})
    return out


# The setup's own run-local values: the incus client's first-run tip (a
# random image name) and this twin's memory as the host probe reads it.
SETUP_MASKS = [{"path": ["steps", "incusClient", "stderr"], "type": "string", "optional": True,
                "reason": "the incus client's first-run tip names a random image"}] + [
    {"path": ["steps", "installTask", "body", "result", "result", "structuredContent"] + where + rest,
     "type": "non-negative-int", "optional": True, "reason": "this twin's memory as the kernel reports it"}
    for where in (["nodes", "host", "output"], ["outputs", "host"])
    for rest in (["system", "memoryTotalBytes"], ["system", "resourceAdmission", "effectiveLimits", "memoryBytes"])]

DOMAIN = ["internal/domain/kubernetes/provider.go", "internal/hostmcp/kubernetes_provider.go",
          "plugins/kubernetes/k3s/cmd/opute-provider-k3s/main.go"]
RESOURCES = DOMAIN + ["internal/domain/kubernetes/generic.go", "internal/domain/kubernetes/secret.go",
                      "internal/domain/kubernetes/exec.go", "internal/domain/kubernetes/lists.go"]
PACKAGES = DOMAIN + ["internal/domain/kubernetes/helm_chart.go", "internal/domain/kubernetes/helm_template.go"]
STORAGE = DOMAIN + ["internal/domain/kubernetes/guest_storage.go",
                    "plugins/kubernetes/k3s/cmd/opute-provider-k3s/storage.go",
                    "plugins/kubernetes/k3s/cmd/opute-provider-k3s/storage_registry.go"]
AGENT = DOMAIN + ["internal/domain/cluster/cluster_agent.go",
                  "internal/domain/cluster/cluster_agent_bridge_relay.go", "internal/tcprelay"]

CONFIGMAP = ("apiVersion: v1\nkind: ConfigMap\nmetadata:\n  name: parity-config\n  namespace: parity\n"
             "data:\n  greeting: hello\n")
NAMESPACE = "apiVersion: v1\nkind: Namespace\nmetadata:\n  name: parity\n"
# A Deployment scaled to zero: a real apps object whose status never
# depends on a pod's schedule.
DEPLOYMENT = ("apiVersion: apps/v1\nkind: Deployment\nmetadata:\n  name: parity-web\n  namespace: parity\n"
              "spec:\n  replicas: 0\n  selector:\n    matchLabels: {app: parity-web}\n  template:\n"
              "    metadata:\n      labels: {app: parity-web}\n    spec:\n      containers:\n"
              "        - name: web\n          image: rancher/mirrored-pause:3.6\n")
SERVICE = ("apiVersion: v1\nkind: Service\nmetadata:\n  name: parity-web\n  namespace: parity\nspec:\n"
           "  selector: {app: parity-web}\n  ports: [{port: 80, targetPort: 8080}]\n")
MUTATED = CONFIGMAP.replace("hello", "changed")

# Server-assigned values in read answers: the uid, resourceVersion and
# timestamps k1's API server gives an object, and an object's age.
SERVER_ASSIGNED = "assigned by k1's API server (uid, resourceVersion, timestamps); the object is compared in the snapshot"
READ_MASKS = [
    *[{"path": ["steps", "*", "body", "result", "structuredContent", key], "type": "string",
       "reason": "the object as text, with values " + SERVER_ASSIGNED, "optional": True} for key in ("json", "yaml")],
    {"path": ["steps", "*", "body", "result", "content", 0, "text"], "type": "string", "optional": True,
     "match": r"\{.*\"resource\":.*", "reason": "the answer as text, with values " + SERVER_ASSIGNED},
    *[{"path": ["steps", "*", "body", "result", "structuredContent", "resource", "metadata", key], "type": "string",
       "reason": SERVER_ASSIGNED, "optional": True} for key in ("uid", "resourceVersion", "creationTimestamp")],
    *[{"path": ["steps", "*", "body", "result", "structuredContent", "resource", "status", "conditions", "*", key],
       "type": "rfc3339", "reason": "when k1's controllers updated the condition", "optional": True}
      for key in ("lastUpdateTime", "lastTransitionTime")],
    {"path": ["steps", "*", "body", "result", "structuredContent", "resource", "metadata", "managedFields"],
     "type": "array", "reason": SERVER_ASSIGNED, "optional": True},
    {"path": ["steps", "*", "body", "result", "structuredContent", "*", "*", "age"], "type": "string",
     "match": "[0-9]+[smhd]", "reason": "relative to this twin's clock", "optional": True},
]

cases = [
    # Resources: create, read, mutate and delete real objects in k1.
    scenario("resources.lifecycle", [
        *setup(), SNAP("baseline"),
        task_call("namespace", "apply_manifest", {"uri": K1, "manifest": NAMESPACE}),
        task_call("apply", "apply_manifest", {"uri": K1, "manifest": CONFIGMAP + "---\n" + DEPLOYMENT + "---\n" + SERVICE}),
        task_call("secret", "put_k8s_secret", {"uri": K1, "namespace": "parity", "name": "parity-secret",
                                               "data": {"password": "parity-t2-secret-value", "user": "parity"}}),
        SNAP("created", "baseline"),
        call("get", "get_k8s_resource", {"uri": K1, "kind": "configmap", "resourceName": "parity-config",
                                         "namespace": "parity"}),
        call("status", "get_k8s_resource_status", {"uri": K1, "resourceKind": "deployment",
                                                   "resourceName": "parity-web", "namespace": "parity"}),
        call("missing", "get_k8s_resource", {"uri": K1, "kind": "configmap", "resourceName": "absent",
                                             "namespace": "parity"}),
        call("namespaces", "list_namespaces", {"uri": K1}),
        call("services", "list_services", {"uri": K1, "namespace": "parity"}),
        call("deployments", "list_deployments", {"uri": K1, "namespace": "parity"}),
        call("storageClasses", "list_storage_classes", {"uri": K1}),
        call("exec", "exec_kubernetes_command", {"uri": K1, "command": "get",
                                                 "args": ["configmap", "parity-config", "-n", "parity",
                                                          "-o", "jsonpath={.data.greeting}"]}),
        call("execFails", "exec_kubernetes_command", {"uri": K1, "command": "get",
                                                      "args": ["configmap", "absent", "-n", "parity"]}),
        # Exposure: Go builds these no resource binding, so with every argument
        # given they still refuse before reaching the provider (no Ingress).
        task_call("expose", "configure_service_domain", {"vmName": "k1", "namespace": "parity",
                                                         "ingressName": "parity-web", "hostname": "parity.example",
                                                         "serviceName": "parity-web", "servicePort": 80}),
        task_call("unexpose", "remove_service_domain", {"vmName": "k1", "namespace": "parity",
                                                        "ingressName": "parity-web"}),
        task_call("mutate", "apply_manifest", {"uri": K1, "manifest": MUTATED}),
        task_call("rotate", "put_k8s_secret", {"uri": K1, "namespace": "parity", "name": "parity-secret",
                                               "data": {"password": "parity-t2-rotated-value"}}),
        SNAP("mutated", "baseline"),
        task_call("deleteSecret", "delete_k8s_resource", {"uri": K1, "kind": "secret",
                                                          "resourceName": "parity-secret", "namespace": "parity"}),
        task_call("deleteNamespace", "delete_k8s_resource", {"uri": K1, "kind": "namespace",
                                                             "resourceName": "parity"}, timeout=900),
        task_call("deleteGone", "delete_k8s_resource", {"uri": K1, "kind": "namespace", "resourceName": "parity"}),
        STOP, SNAP("teardown", "baseline"),
    ], RESOURCES, "None in k1: the namespace and everything in it are deleted. The agent's own state stays.",
        masks=READ_MASKS),
    # Packages: a HelmChart through the provider, inert with the helm
    # controller disabled, and helm template on the twin.
    scenario("packages.chart-and-render", [
        *setup(), SNAP("baseline"),
        {"as": "chart", "write": {"path": "/home/ubuntu/chart/Chart.yaml", "owner": "ubuntu:ubuntu",
                                  "content": "apiVersion: v2\nname: parity\nversion: 0.1.0\n"}},
        {"as": "chartTemplate", "write": {"path": "/home/ubuntu/chart/templates/cm.yaml", "owner": "ubuntu:ubuntu",
                                          "content": "apiVersion: v1\nkind: ConfigMap\nmetadata:\n"
                                                     "  name: {{ .Release.Name }}\ndata:\n"
                                                     "  value: {{ .Values.value | default \"none\" | quote }}\n"}},
        SNAP("chartWritten", "baseline"),
        call("render", "render_helm_template", {"chartPath": "/home/ubuntu/chart", "releaseName": "parity",
                                                "namespace": "parity", "set": ["value=rendered"]}),
        call("renderBroken", "render_helm_template", {"chartPath": "/home/ubuntu", "releaseName": "parity"}),
        call("chartSystem", "install_helm_chart", {"uri": CT1, "vmName": "k1", "releaseName": "parity-sys",
                                                   "chartSource": "parity", "repo": "http://127.0.0.1:9/charts",
                                                   "values": {"replicaCount": 1}}),
        call("chartTeam", "install_helm_chart", {"uri": CT1, "vmName": "k1", "releaseName": "parity-team",
                                                 "chartSource": "parity", "namespace": "parity-team"}),
        SNAP("installed", "chartWritten"),
        task_call("deleteSys", "delete_k8s_resource", {"uri": K1, "kind": "helmchart",
                                                       "resourceName": "parity-sys", "namespace": "kube-system"}),
        # Go puts every HelmChart in kube-system; the release namespace is only
        # its targetNamespace.
        task_call("deleteTeamChart", "delete_k8s_resource", {"uri": K1, "kind": "helmchart",
                                                             "resourceName": "parity-team",
                                                             "namespace": "kube-system"}),
        task_call("deleteTeam", "delete_k8s_resource", {"uri": K1, "kind": "namespace",
                                                        "resourceName": "parity-team"}, timeout=900),
        STOP, SNAP("teardown", "baseline"),
    ], PACKAGES, "The chart files written by the scenario stay; in k1 both HelmCharts (kube-system) and the namespace "
               "ensure_namespace created are deleted.",
        masks=READ_MASKS),
    # Guest storage: inspect, prune and trim k1, and collect the registry's
    # garbage with the registry scaled down and back.
    scenario("storage.prune-and-collect", [
        *setup(), SNAP("baseline"),
        call("inspect", "inspect_guest_storage", {"uri": K1, "includeRegistry": True}),
        task_call("pruneDry", "prune_unused_cluster_images", {"uri": K1, "dryRun": True}),
        task_call("gcDry", "garbage_collect_cluster_registry", {"uri": K1, "dryRun": True, "includeRegistry": True}),
        SNAP("afterDryRuns", "baseline"),
        task_call("prune", "prune_unused_cluster_images", {"uri": K1}),
        task_call("gc", "garbage_collect_cluster_registry", {"uri": K1, "includeRegistry": True}, timeout=900),
        call("trim", "trim_guest_storage", {"uri": K1}),
        STOP, SNAP("teardown", "baseline"),
    ], STORAGE, "The registry's two seeded tags (parity/app v1, v2) are deleted by the garbage collection, which scales "
       "the registry down and back; the prune removes the exited containers only (Go keeps every image it lists).",
        masks=[{"path": ["steps", "*", "body", "result"] + where + [key], "type": "non-negative-int",
                "reason": "byte counts of k1's live filesystem", "optional": True}
               for where in (["structuredContent"], ["result", "structuredContent"],
                             ["structuredContent", "containerd"])
               for key in ("usedBytes", "availBytes", "availableBytes", "totalBytes", "sizeBytes",
                           "containerdBytes", "contentBytes", "snapshotsBytes", "k3sStorageBytes",
                           "beforeImageBytes", "afterImageBytes", "reclaimedImageBytes", "reclaimableImageBytes")]
        + [{"path": ["steps", "*", "body", "result", "structuredContent", "rootFilesystem", key],
            "type": "non-negative-int", "reason": "byte counts of k1's live filesystem", "optional": True}
           for key in ("usedBytes", "availBytes", "sizeBytes", "totalBytes")]
        + [{"path": ["steps", "*", "body", "result", "structuredContent", "persistentVolumes", "*", "bytes"],
            "type": "non-negative-int", "reason": "byte counts of k1's live filesystem", "optional": True},
           {"path": ["steps", "inspect", "body", "result", "content", 0, "text"], "type": "string",
            "match": r"\{\"clusterId\":\"k1\".*", "optional": True,
            "reason": "the answer as text, with k1's live byte counts; structuredContent is compared"}]
        + [{"path": ["steps", "trim", "body", "result", "structuredContent", "output"], "type": "string",
            "match": r"/: [0-9.]+ [KMGT]?i?B \([0-9]+ bytes\) trimmed", "reason": "fstrim's count", "optional": True}]),
]

# The cluster agent: a harness-owned platform stub on the twin's loopback
# serves /health and a stand-in agent (a gzip'd shell script that sleeps).
# The bridge URL is loopback, so the agent opens its own guest bridge relay
# on incusbr0 (Go's tcprelay) and k1 reaches the stub through it.
STUB = ("import gzip,http.server\n"
        "AGENT=gzip.compress(b'#!/bin/sh\\nexec sleep infinity\\n', mtime=0)\n"
        "class H(http.server.BaseHTTPRequestHandler):\n"
        "    def do_GET(self):\n"
        "        body={'/health':b'ok','/artifacts/cluster-agent/x64.gz':AGENT}.get(self.path)\n"
        "        self.send_response(200 if body is not None else 404); self.end_headers()\n"
        "        self.wfile.write(body or b'')\n"
        "    def log_message(self,*a): pass\n"
        "http.server.ThreadingHTTPServer(('127.0.0.1',9093),H).serve_forever()\n")
cases.append(scenario("agent.vm-install", [
    {"as": "stubFile", "write": {"path": ROOT + "/platform-stub.py", "content": STUB}},
    shell("stub", ["systemd-run", "--unit=parity-platform-stub", "python3", ROOT + "/platform-stub.py"]),
    *setup(), SNAP("baseline"),
    task_call("install", "install_cluster_agent", {"vmName": "k1", "clusterId": "k1", "clusterName": "k1",
                                                   "agentId": "parity-agent", "bridgeUrl": "http://127.0.0.1:9093",
                                                   "bridgePort": 9093, "bridgeToken": "parity-t2-bridge-token"},
              timeout=900),
    shell("agentActive", k1("systemctl", "is-active", "opute-cluster-agent")),
    SNAP("installed", "baseline"),
    task_call("noVm", "install_cluster_agent", {"source": "k3s-vm", "clusterId": "k1", "clusterName": "k1",
                                                "agentId": "parity-agent"}),
    STOP, SNAP("teardown", "baseline"),
], AGENT, "Go leaves the agent installed in k1 (its binary, config and enabled unit): no tool uninstalls it."))

# Every M4-rejected kubernetes and cluster call, with effectful arguments
# aimed at k1, leaves k1 and the twin unchanged.
REJECTED = [
    ("apply_manifest", {"uri": K1, "manifest": NAMESPACE}),
    ("configure_service_domain", {"vmName": "k1", "namespace": "parity", "ingressName": "parity",
                                  "hostname": "parity.example", "serviceName": "web", "servicePort": 80}),
    ("delete_k8s_resource", {"uri": K1, "kind": "deployment", "resourceName": "opute-registry",
                             "namespace": "registry"}),
    ("exec_kubernetes_command", {"uri": K1, "command": "create", "args": ["namespace", "parity"]}),
    ("garbage_collect_cluster_registry", {"uri": K1, "includeRegistry": True}),
    ("install_cluster_agent", {"vmName": "k1", "clusterId": "k1", "clusterName": "k1", "agentId": "a",
                               "bridgeUrl": "http://127.0.0.1:9093"}),
    ("install_helm_chart", {"uri": CT1, "vmName": "k1", "releaseName": "parity", "chartSource": "parity"}),
    ("prune_unused_cluster_images", {"uri": K1}),
    ("put_k8s_secret", {"uri": K1, "namespace": "default", "name": "parity", "data": {"k": "v"}}),
    ("remove_service_domain", {"vmName": "k1", "namespace": "registry", "ingressName": "parity"}),
    ("trim_guest_storage", {"uri": K1}),
]
M4_GATED, M4_BINDING = hd.M4_GATED, hd.M4_BINDING
CLUSTER_TOOLS = {t["tool"] for t in json.load(open("../../baseline/inventory/cluster-domain.json"))["tools"]}
assert M4_GATED & CLUSTER_TOOLS == {n for n, _ in REJECTED}, sorted((M4_GATED & CLUSTER_TOOLS) ^ {n for n, _ in REJECTED})


def rejected(prefix, meta=None):
    out = []
    for name, args in REJECTED:
        step = call(prefix + "." + name, name, args)
        if meta:
            step["mcp"]["meta"] = meta
        out.append(step)
    return out


def wrong_binding():
    defaults = dict(REJECTED)
    out = []
    for i, (name, args) in enumerate(b for b in M4_BINDING if b[0] in CLUSTER_TOOLS):
        merged = {k: v for k, v in defaults[name].items() if k != "uri"}
        merged.update(args)
        out.append(call(f"wrongBinding.{name}.{i}", name, merged))
    return out


def empty(as_):
    return {"as": as_, "snapshot": {"baseline": "baseline", "expectEmpty": True}}


cases.append(scenario("admission.rejected-no-effect", [
    *setup(), {"as": "stopInstall", "stop": {}},
    {"as": "serveClosed", "serve": {"unset": ["OPUTE_STANDALONE_ALLOW_MUTATIONS"]}},
    SNAP("baseline"),
    rejected("gateClosed"),
    empty("afterGateClosed"),
    {"as": "stopClosed", "stop": {}},
    {"as": "serveOpen", "serve": {}},
    rejected("staleRevision", meta={"catalogRevision": "sha256:0000"}),
    wrong_binding(),
    empty("afterOpen"),
    STOP, empty("teardown"),
], hd.ADMISSION + ["internal/domain/kubernetes", "internal/domain/cluster"],
    "None: the provider is installed before the baseline, and every rejected call is refused before it reaches "
    "the provider or k1.", rejected_only=True))

if __name__ == "__main__":
    out = sys.argv[1] if len(sys.argv) > 1 else "scenarios-t2/cluster-domain.json"
    with open(out, "w") as f:
        json.dump(cases, f, indent=1)
        f.write("\n")
    print(f"wrote {len(cases)} T2 scenarios to {out}")
