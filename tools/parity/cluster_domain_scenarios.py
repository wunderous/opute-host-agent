#!/usr/bin/env python3
"""Generate the M8c cluster-domain T1 matrix: scenarios/cluster-domain.json
and its shim fixtures (fixtures/shims/cluster-domain-*.json).

Run from tools/parity: python3 cluster_domain_scenarios.py

The Host Agent never runs kubectl: every cluster operation is a call to the
active Kubernetes provider. Each scenario installs and activates the pinned,
unchanged Go k3s provider (opute-provider-k3s) exactly as provider-real.k3s
does, and the provider runs `incus exec <instance> -- k3s kubectl ...`
through the recorded incus shim. So the trace holds the provider argv and
stdin hash each Host Agent call produced, and both sides are compared on
that as well as on their answers and registry rows.

The estate: k1, a running VM with k3s; c1, a running container without
k3s (the provider skips it); and helm on the host for helm template. Each
family gets success, typed-failure and partial-failure cases. The cluster
agent's VM install runs against k1's scripted guest; its host-native
install stops at Go's own refusal, because sudo is shimmed to fail, so no
host unit is ever written.
"""
import copy
import json
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
TENANT = "local"
K1 = f"cluster:{TENANT}:k1"
VM1 = f"vm:{TENANT}:k1"

SERVE = {"as": "start", "serve": {"argv": ["serve", "--mode=standalone"], "profile": "standalone", "env": {
    "OPUTE_STANDALONE_ALLOW_MUTATIONS": "true",
    "OPUTE_HOST_RESOURCE_ENFORCEMENT": "advisory",
    "OPUTE_HOST_MAX_NORMAL_OPERATIONS": "64", "OPUTE_HOST_MAX_HEAVY_OPERATIONS": "64",
    "OPUTE_HOST_RESOURCE_CPU_CAPACITY_CORES": "64",
    "OPUTE_HOST_RESOURCE_MEMORY_CAPACITY_BYTES": "68719476736",
    "XDG_RUNTIME_DIR": "${SANDBOX}/run",
    "DBUS_SESSION_BUS_ADDRESS": "unix:path=${SANDBOX}/run/bus"}}}
STOP = {"as": "stop", "stop": {}}


def serve(**env):
    s = copy.deepcopy(SERVE)
    s["serve"]["env"].update(env)
    return s


# Both sides are called by the Go name: Go knows no other, and Rust keeps it
# as a hidden alias (D22). The neutral names show in tools/list, compared
# through the D19 divergence.
GO_NAME = {v: k for k, v in json.loads(
    (HERE.parents[1] / "crates/host-agent/catalog/neutral-names.json").read_text())["tools"].items()}


def call(as_, name, args):
    name = GO_NAME.get(name, name)
    return {"as": as_, "mcp": {"method": "tools/call", "name": name,
                               "params": {"name": name, "arguments": args}, "expand": True}}


def task_call(as_, name, args, timeout=60):
    """A task-aware tool answers with a task handle; wait for the task."""
    var = "T_" + as_.upper()
    step = call(as_, name, args)
    step["mcp"]["capture"] = {var: ["body", "result", "taskId"]}
    done = {"as": as_ + "Done", "requires": [var], "mcp": {
        "method": "tasks/get", "name": "${%s}" % var, "params": {"taskId": "${%s}" % var}, "expand": True,
        "until": {"path": ["body", "result", "status"], "in": ["completed", "failed", "cancelled"]},
        "timeout": timeout}}
    return [step, done]


def flat(steps):
    out = []
    for step in steps:
        out.extend(step if isinstance(step, list) else [step])
    return out


REAL_K3S = next(s for s in json.loads((HERE / "scenarios" / "provider-real.json").read_text())
                if s["id"] == "provider-real.k3s")


def k3s_install():
    """provider-real.k3s's own install: the pinned provider, its activation
    recipe, the install call and the wait for its run and task."""
    steps = REAL_K3S["steps"]
    labels = ["provider", "recipe", "install", "installed", "installTask"]
    return [copy.deepcopy(next(s for s in steps if s["as"] == label)) for label in labels]


TS = "wall-clock timestamp of this side's run"
MASKS = [{"path": ["steps", "*", "body", "result", k], "type": "rfc3339", "reason": TS, "optional": True}
         for k in ("createdAt", "lastUpdatedAt")]
REGISTRY = {"as": "registry", "query": {"db": "${SANDBOX}/state/state.db", "sql": {
    "resources": "SELECT uri, resource_type, tenant_id, resource_id, coordinates_json, status "
                 "FROM resource_registry ORDER BY uri"}}}


def scenario(id_, steps, anchors, fixture, masks=(), divergences=(), sets=()):
    s = {"id": "cluster-domain." + id_, "surface": "cluster-domain", "owner": "kubernetes",
         "anchors": anchors, "fixture": fixture, "collect": ["trace", "orphans"],
         "steps": flat(steps), "compare": copy.deepcopy(REAL_K3S["compare"])}
    # The install's own comparison (provider-real.k3s), plus this matrix's.
    # Every scenario lists or calls through the D22 names.
    have = {json.dumps(m["path"]) for m in s["compare"]["masks"]}
    s["compare"]["masks"] += [m for m in MASKS + list(masks) if json.dumps(m["path"]) not in have]
    s["compare"]["divergences"] = ["D19.neutral-host-tool-names", *divergences]
    s["compare"]["sets"] += [list(p) for p in sets]
    return s


# --- fixtures -----------------------------------------------------------------------------

def compact(value):
    return json.dumps(value, separators=(",", ":"))


def rule(argv, stdout=None, exit_=None, stderr=None, **extra):
    r = {"argvPrefix": argv[:-1]} if argv and argv[-1] == "..." else {"argv": argv}
    if stdout is not None:
        r["stdout"] = stdout if isinstance(stdout, str) else compact(stdout)
    if exit_ is not None:
        r["exit"] = exit_
    if stderr is not None:
        r["stderr"] = stderr
    r.update(extra)
    return r


def kubectl(args, stdout=None, exit_=None, stderr=None, **extra):
    return rule(["exec", "k1", "--", "k3s", "kubectl", *args], stdout, exit_, stderr, **extra)


def instance(name, status, kind):
    return {"name": name, "status": status, "type": kind, "config": {}, "devices": {},
            "state": {"network": {}}}


def meta(name, namespace=None, uid=None, created="2026-01-01T00:00:00Z"):
    m = {"name": name, "creationTimestamp": created}
    if namespace:
        m["namespace"] = namespace
    if uid:
        m["uid"] = uid
    return m


def items(*objects):
    return {"apiVersion": "v1", "kind": "List", "items": list(objects)}


NODES = items({"metadata": meta("k1"), "status": {
    "conditions": [{"type": "Ready", "status": "True"}],
    "nodeInfo": {"kubeletVersion": "v1.31.2+k3s1"}}})
DEPLOYMENT = {"apiVersion": "apps/v1", "kind": "Deployment", "metadata": meta("web", "apps", "d-1"),
              "spec": {"replicas": 2}, "status": {"readyReplicas": 2, "availableReplicas": 2}}
NOT_FOUND = 'Error from server (NotFound): deployments.apps "missing" not found\n'
NO_TYPE = 'error: the server doesn\'t have a resource type "clusterissuers"\n'
EVENTS = items(
    {"metadata": meta("e1", "apps"), "type": "Normal", "reason": "Scheduled", "message": "ok", "count": 1,
     "lastTimestamp": "2026-01-01T00:00:01Z"},
    {"metadata": meta("e2", "apps"), "type": "Warning", "reason": "BackOff", "message": "<retry> & wait",
     "count": 3, "lastTimestamp": "2026-01-01T00:00:09Z"})
ISSUER = {"metadata": meta("letsencrypt", "apps"), "spec": {"acme": {}},
          "status": {"conditions": [{"type": "Ready", "status": "True"}]}}
PODS = items(
    {"metadata": meta("web-1", "apps", "p-1"), "spec": {"nodeName": "k1"},
     "status": {"phase": "Running", "podIP": "10.42.0.7",
                "containerStatuses": [{"ready": True, "restartCount": 1}, {"ready": True, "restartCount": 2}]}},
    {"metadata": meta("job-1", "apps", "p-2"), "status": {"containerStatuses": [{"ready": False}]}})
SERVICES = items({"metadata": meta("web", "apps")}, {"metadata": meta("db", "apps")})
DEPLOYMENTS = items(DEPLOYMENT, {"metadata": meta("idle", "apps"), "spec": {"replicas": 1}, "status": {}})


def estate_rules():
    return [
        rule(["list", "--format", "json"], [instance("k1", "Running", "virtual-machine"),
                                            instance("c1", "Running", "container")]),
        rule(["exec", "k1", "--", "k3s", "--version"], "k3s version v1.31.2+k3s1 (6da20424)\ngo version go1.22.8\n"),
        rule(["exec", "c1", "--", "k3s", "--version"], exit_=127, stderr="k3s: not found\n"),
        kubectl(["get", "nodes", "-o", "json"], NODES),
        kubectl(["apply", "-f", "-"], "configured\n"),
        kubectl(["get", "deployment", "web", "-o", "json", "-n", "apps"], DEPLOYMENT),
        kubectl(["get", "deployment", "missing", "-o", "json", "-n", "apps"], exit_=1, stderr=NOT_FOUND),
        kubectl(["delete", "deployment", "web", "-n", "apps", "--ignore-not-found=true"], 'deleted\n'),
        kubectl(["delete", "ingress", "gone", "-n", "apps", "--ignore-not-found=true"], exit_=1,
                stderr='Error from server (NotFound): ingresses "gone" not found\n'),
        kubectl(["delete", "ingress", "site", "-n", "apps", "--ignore-not-found=true"], "deleted\n"),
        kubectl(["get", "events", "-o", "json", "-n", "apps"], EVENTS),
        kubectl(["get", "namespaces", "-o", "json"], items({"metadata": meta("default")}, {"metadata": meta("apps")})),
        kubectl(["get", "storageclasses", "-o", "json"], items({"metadata": meta("local-path")})),
        kubectl(["get", "ingressclasses", "-o", "json"], items({"metadata": meta("traefik")})),
        kubectl(["get", "clusterissuers", "-o", "json"], exit_=1, stderr=NO_TYPE),
        kubectl(["get", "issuers", "--all-namespaces", "-o", "json"], items(ISSUER)),
        kubectl(["get", "issuers", "-n", "apps", "-o", "json"], items(ISSUER)),
        kubectl(["get", "pods", "--all-namespaces", "-o", "json"], PODS),
        kubectl(["get", "pods", "-n", "broken", "-o", "json"], items({"metadata": meta("ghost", "broken")})),
        kubectl(["get", "services", "-n", "apps", "-o", "json"], SERVICES),
        kubectl(["get", "deployments", "--all-namespaces", "-o", "json"], DEPLOYMENTS),
        kubectl(["get", "deployments", "-n", "down", "-o", "json"], exit_=1,
                stderr="The connection to the server 127.0.0.1:6443 was refused\n"),
        kubectl(["get", "namespace", "team"], exit_=1, stderr='Error from server (NotFound): namespaces "team" not found\n'),
        kubectl(["create", "namespace", "team"], "namespace/team created\n"),
        kubectl(["get", "namespace", "apps"], "apps   Active   1d\n"),
        kubectl(["version", "--client", "-o", "json"], {"clientVersion": {"gitVersion": "v1.31.2+k3s1"}}),
        # Guest storage (the provider's storage.go and storage_registry.go).
        rule(["exec", "k1", "--", "df", "-B1", "/"],
             "Filesystem     1B-blocks        Used   Available Use% Mounted on\n"
             "/dev/sda2   64000000000 40000000000 24000000000  63% /\n"),
        rule(["exec", "k1", "--", "du", "-sb", "/var/lib/rancher/k3s/agent/containerd"],
             "30000000000\t/var/lib/rancher/k3s/agent/containerd\n"),
        rule(["exec", "k1", "--", "du", "-sb", "/var/lib/rancher/k3s/agent/containerd/io.containerd.content.v1.content"],
             "12000000000\t/var/lib/rancher/k3s/agent/containerd/io.containerd.content.v1.content\n"),
        rule(["exec", "k1", "--", "du", "-sb",
              "/var/lib/rancher/k3s/agent/containerd/io.containerd.snapshotter.v1.overlayfs"], exit_=1,
             stderr="du: cannot access: No such file or directory\n"),
        rule(["exec", "k1", "--", "du", "-sb", "/var/lib/rancher/k3s/storage"],
             "5000000000\t/var/lib/rancher/k3s/storage\n"),
        rule(["exec", "k1", "--", "bash", "-lc", "du -sb /var/lib/rancher/k3s/storage/* 2>/dev/null || true"],
             "4000000000\t/var/lib/rancher/k3s/storage/pvc-1_apps_data\n"
             "1000000000\t/var/lib/rancher/k3s/storage/pvc-2_registry_zot\n"),
        rule(["exec", "k1", "--", "k3s", "crictl", "images", "-o", "json"], {"images": [
            {"id": "sha256:aaa", "repoTags": ["docker.io/library/nginx:1.27"], "repoDigests": [], "size": "70000000"},
            {"id": "sha256:bbb", "repoTags": ["registry.local/app:old"], "repoDigests": [], "size": "300000000"},
            {"id": "sha256:ccc", "repoTags": [], "repoDigests": ["keep@sha256:1"], "size": "1000"}]}),
        rule(["exec", "k1", "--", "k3s", "crictl", "rmi", "sha256:bbb"], "Deleted: registry.local/app:old\n"),
        rule(["exec", "k1", "--", "k3s", "crictl", "rmi", "sha256:ccc"], exit_=1,
             stderr="image is in use by a container\n"),
        kubectl(["-n", "registry", "get", "svc", "opute-registry", "-o", "json"],
                {"metadata": meta("opute-registry", "registry"),
                 "spec": {"clusterIP": "10.43.0.50", "ports": [{"port": 5000}]}}),
        rule(["exec", "k1", "--", "bash", "-lc",
              "curl -sf --max-time 30 -X 'GET' 'http://10.43.0.50:5000/v2/_catalog'"], {"repositories": []}),
        rule(["exec", "k1", "--", "fstrim", "-v", "/"], "/: 2 GiB (2147483648 bytes) trimmed\n"),
        kubectl(["get", "deploy", "-A", "-o", "json"], items(
            DEPLOYMENT, {"metadata": meta("opute-registry", "registry"), "spec": {"replicas": 1,
             "template": {"spec": {"containers": [{"name": "registry", "image": "registry:2"}]}}},
             "status": {"readyReplicas": 1}})),
        kubectl(["get", "pods", "-A", "-o", "json"], items(
            {"metadata": meta("web-1", "apps", "p-1"), "spec": {"containers": [{"image": "docker.io/library/nginx:1.27"}]},
             "status": {"phase": "Running"}})),
        rule(["exec", "k1", "--", "k3s", "crictl", "ps", "-a", "-o", "json"], {"containers": [
            {"id": "c-1", "imageRef": "sha256:aaa", "image": {"image": "sha256:aaa"}, "state": "CONTAINER_RUNNING"}]}),
        kubectl(["get", "pod", "nope"], exit_=1, stderr='Error from server (NotFound): pods "nope" not found\n'),
    ]


DEFAULT = {"exit": 1, "stderr": "unsupported parity shim invocation\n"}
FIXTURES = {
    "cluster-domain-estate": {
        "description": "M8c: a running k3s VM (k1) and a container without k3s (c1), scripted for the "
                       "pinned k3s provider's incus exec calls; helm on the host.",
        "commands": {
            "incus": estate_rules(),
            "helm": [rule(["template", "web", "..."], "kind: Service\n"),
                     rule(["template", "bad", "..."], exit_=1, stderr="Error: chart is broken\n")],
            "sudo": [rule(["..."], exit_=1, stderr="sudo: a password is required\n")],
        },
        "default": DEFAULT,
    },
}

# The cluster agent's guest: exec-ready, the bridge answers its health
# probe, x86_64, the install script succeeds and the unit comes up.
AGENT_GUEST = [
    rule(["exec", "k1", "--", "true"]),
    rule(["query", "/1.0/instances/c1"], {"name": "c1", "type": "container", "status": "Running"}),
    rule(["query", "/1.0/instances/k1"], {"name": "k1", "type": "virtual-machine", "status": "Running"}),
    rule(["config", "device", "show", "c1"], "root:\n  path: /\n  pool: default\n  type: disk\n"),
    rule(["config", "device", "add", "c1", "..."], "Device platform-bridge-9093 added to c1\n"),
    rule(["query", "/1.0/instances/absent"], exit_=1, stderr="Error: Instance not found\n"),
    rule(["exec", "k1", "--", "uname", "-m"], "x86_64\n"),
    rule(["exec", "k1", "--", "bash", "-lc", "curl -sf -o /dev/null 'wss://bridge.example/agent/health'"]),
    rule(["exec", "k1", "--", "bash", "-lc", "..."], "installed\n"),
    rule(["exec", "k1", "--", "/usr/bin/systemctl", "is-active", "opute-cluster-agent"], "active\n"),
]
FIXTURES["cluster-domain-agent"] = {
    "description": "M8c: the estate plus k1's scripted guest for the cluster agent's VM install; sudo "
                   "fails, so a host-native install never reaches the host.",
    "commands": {**FIXTURES["cluster-domain-estate"]["commands"],
                 "incus": AGENT_GUEST + estate_rules()},
    "default": DEFAULT,
}

DOMAIN = ["internal/domain/kubernetes/provider.go", "internal/hostmcp/kubernetes_provider.go",
          "internal/hostagent/resource_resolver.go:adoptResource", "plugins/kubernetes/k3s"]
LISTS = ["internal/domain/kubernetes/lists.go", "internal/tools/dispatch_unassigned.go"]
RESOURCES = ["internal/domain/kubernetes/generic.go", "internal/domain/kubernetes/secret.go",
             "internal/domain/kubernetes/exec.go", "internal/tools/dispatch_kubernetes.go"]
PACKAGES = ["internal/domain/kubernetes/helm_chart.go", "internal/domain/kubernetes/helm_template.go"]
EXPOSURE = ["internal/domain/kubernetes/exposure.go"]
FIXTURE = "cluster-domain-estate"

AGENT = DOMAIN + ["internal/domain/cluster/cluster_agent.go",
                  "internal/domain/cluster/cluster_agent_bridge_relay.go"]
VM_INSTALL = {"vmName": "k1", "clusterId": "k1", "clusterName": "k1", "agentId": "agent-1",
              "bridgeUrl": "http://127.0.0.1:9093", "bridgePort": 9093, "bridgeToken": "token-1"}


def cpc(id_, key, name):
    """The CPC container named by one of Go's three keys (the first set wins)
    proxies the guest bridge. OPUTE_PLATFORM_GUEST_HOST stays a non-local
    address, so no listener is ever bound on the host."""
    return scenario("agent-cpc." + id_, [
        serve(OPUTE_PLATFORM_GUEST_HOST="10.0.0.1", **{key: name}), *k3s_install(),
        task_call("vm", "install_cluster_agent", VM_INSTALL, timeout=120),
        STOP], AGENT, "cluster-domain-agent")


cases = [
    # Discovery: the provider's inventory registers each cluster; a URI the
    # registry has never seen is adopted by asking that inventory.
    scenario("discovery", [
        serve(), {"as": "list", "mcp": {"method": "tools/list"}},
        call("noProvider", "list_compute_clusters", {}),
        call("noProviderBound", "get_cluster_resource", {"uri": K1, "kind": "deployment", "resourceName": "web"}),
        *k3s_install(),
        call("adopted", "get_cluster_resource",
             {"uri": K1, "kind": "deployment", "resourceName": "web", "namespace": "apps"}),
        call("unlisted", "get_cluster_resource", {"uri": f"cluster:{TENANT}:c1", "kind": "pod", "resourceName": "x"}),
        call("clusters", "list_compute_clusters", {"source": "k3s-vm"}),
        call("containers", "list_compute_clusters", {"source": "k3s-container"}),
        call("badSource", "list_compute_clusters", {"source": " incus "}),
        REGISTRY, STOP], DOMAIN + ["internal/domain/kubernetes/provider.go:ListKubernetesClusters"], FIXTURE),
    # Resources: each a provider call against the bound cluster.
    scenario("resources", [
        serve(), *k3s_install(),
        task_call("apply", "apply_manifest", {"uri": K1, "manifest": "apiVersion: v1\nkind: ConfigMap\n"}),
        task_call("applyEmpty", "apply_manifest", {"uri": K1, "manifest": "  \n"}),
        call("get", "get_cluster_resource", {"uri": K1, "kind": "deployment", "resourceName": "web",
                                             "namespace": "apps"}),
        call("getMissing", "get_cluster_resource", {"uri": K1, "kind": "deployment", "resourceName": "missing",
                                                    "namespace": "apps"}),
        call("getBadName", "get_cluster_resource", {"uri": K1, "kind": "deploy ment", "resourceName": "web"}),
        call("status", "get_cluster_resource_status", {"uri": K1, "resourceKind": "deployment",
                                                       "resourceName": "web", "namespace": "apps"}),
        call("statusMissing", "get_cluster_resource_status", {"uri": K1, "resourceKind": "deployment",
                                                              "resourceName": "missing", "namespace": "apps"}),
        call("statusNoKind", "get_cluster_resource_status", {"uri": K1, "resourceKind": " ", "resourceName": "web",
                                                             "namespace": "apps"}),
        task_call("delete", "delete_cluster_resource", {"uri": K1, "kind": "deployment", "resourceName": "web",
                                                        "namespace": "apps"}),
        call("events", "list_cluster_events", {"uri": K1, "namespace": "apps", "limit": 500}),
        call("eventsDefault", "list_cluster_events", {"uri": K1, "namespace": "apps", "limit": -3}),
        task_call("secret", "put_cluster_secret", {"uri": K1, "namespace": "apps", "name": "db",
                                                   "data": {"password": "s3cr3t", "skip": 5}}),
        task_call("secretBadKey", "put_cluster_secret", {"uri": K1, "name": "db", "data": {"bad key": "x"}}),
        task_call("secretEmpty", "put_cluster_secret", {"uri": K1, "name": "db", "data": {"n": 1}}),
        call("exec", "exec_cluster_command", {"uri": K1, "command": " version ", "args": ["--client", "-o", "json"]}),
        call("execFails", "exec_cluster_command", {"uri": K1, "command": "get", "args": ["pod", "nope"]}),
        call("execNoCommand", "exec_cluster_command", {"uri": K1, "command": "  "}),
        REGISTRY, STOP], DOMAIN + RESOURCES, FIXTURE),
    # Listings: provider kubectl get, shaped and registered as Go does.
    scenario("listings", [
        serve(), *k3s_install(),
        call("namespaces", "list_namespaces", {"uri": K1}),
        call("storageClasses", "list_storage_classes", {"uri": K1}),
        call("ingressClasses", "list_ingress_classes", {"uri": K1}),
        call("issuers", "list_certificate_issuers", {"uri": K1}),
        call("issuersNs", "list_certificate_issuers", {"uri": K1, "namespace": "apps"}),
        call("pods", "list_pods", {"uri": K1}),
        call("podsNoUid", "list_pods", {"uri": K1, "namespace": "broken"}),
        call("services", "list_services", {"uri": K1, "namespace": "apps"}),
        call("deployments", "list_deployments", {"uri": K1}),
        call("deploymentsDown", "list_deployments", {"uri": K1, "namespace": "down"}),
        REGISTRY, STOP], DOMAIN + LISTS, FIXTURE,
        masks=[{"path": ["steps", "*", "body", "result", "structuredContent", "*", "*", "age"], "type": "string",
                "match": "-?[0-9]+[smhd]", "reason": "relative to this side's clock", "optional": True}]),
    # Packages: a HelmChart manifest through the provider; helm template on
    # the host (the helm shim stands in for the binary).
    scenario("packages", [
        serve(), *k3s_install(),
        {"as": "chart", "write": {"path": "${SANDBOX}/chart/Chart.yaml", "content": "name: web\n"}},
        call("chartInstall", "install_cluster_package", {"uri": VM1, "vmName": "k1", "chartName": "web",
                                                         "chart": "bitnami/nginx", "repo": "https://charts.example",
                                                         "values": {"replicaCount": 2}}),
        call("chartNs", "install_cluster_package", {"uri": VM1, "vmName": "k1", "name": "web", "chartSource": "nginx",
                                                    "namespace": "team"}),
        call("chartExistingNs", "install_cluster_package", {"uri": VM1, "vmName": "k1", "releaseName": "api",
                                                            "chartSource": "api", "namespace": "apps"}),
        call("chartNoSource", "install_cluster_package", {"uri": VM1, "vmName": "k1", "releaseName": "x"}),
        call("chartNoVm", "install_cluster_package", {"uri": VM1, "vmName": " ", "releaseName": "x",
                                                      "chartSource": "x"}),
        call("render", "render_cluster_package", {"chartPath": "${SANDBOX}/chart", "releaseName": "web",
                                                  "namespace": "apps", "set": ["a=1", " "]}),
        call("renderBroken", "render_cluster_package", {"chartPath": "${SANDBOX}/chart", "releaseName": "bad"}),
        call("renderNoDir", "render_cluster_package", {"chartPath": "${SANDBOX}/absent", "releaseName": "web"}),
        call("renderNoValues", "render_cluster_package", {"chartPath": "${SANDBOX}/chart", "releaseName": "web",
                                                          "valuesFiles": ["${SANDBOX}/absent.yaml"]}),
        STOP], DOMAIN + PACKAGES, FIXTURE),
    # Exposure: an Ingress applied through the provider; a missing one is
    # already removed.
    scenario("exposure", [
        serve(), *k3s_install(),
        task_call("configure", "configure_service_domain", {"vmName": "k1", "namespace": "apps", "ingressName": "site",
                                                            "hostname": "web.example.com", "serviceName": "web",
                                                            "servicePort": 8080}),
        task_call("badHost", "configure_service_domain", {"vmName": "k1", "namespace": "apps", "ingressName": "site",
                                                          "hostname": "localhost", "serviceName": "web",
                                                          "servicePort": 80}),
        task_call("noPort", "configure_service_domain", {"vmName": "k1", "namespace": "apps", "ingressName": "site",
                                                         "hostname": "a.b", "serviceName": "web",
                                                         "servicePort": 0}),
        task_call("badName", "configure_service_domain", {"vmName": "k1", "namespace": "Apps", "ingressName": "site",
                                                          "hostname": "a.b", "serviceName": "web",
                                                          "servicePort": 80}),
        task_call("bound", "configure_service_domain", {"uri": K1, "vmName": "k1", "namespace": "apps",
                                                        "ingressName": "site", "hostname": "a.b",
                                                        "serviceName": "web", "servicePort": 80}),
        task_call("noVm", "configure_service_domain", {"vmName": "absent", "namespace": "apps",
                                                       "ingressName": "site", "hostname": "a.b",
                                                       "serviceName": "web", "servicePort": 80}),
        task_call("remove", "remove_service_domain", {"vmName": "k1", "namespace": "apps", "ingressName": "site"}),
        task_call("removeGone", "remove_service_domain", {"vmName": "k1", "namespace": "apps", "ingressName": "gone"}),
        STOP], DOMAIN + EXPOSURE, FIXTURE),
    # Guest storage: the provider inspects and trims the guest, prunes
    # unused images and collects the registry's garbage.
    scenario("storage", [
        serve(), *k3s_install(),
        call("inspect", "inspect_guest_storage", {"uri": K1}),
        call("inspectRegistry", "inspect_guest_storage", {"uri": K1, "includeRegistry": True,
                                                          "extraKeepTags": ["keep:1", " "],
                                                          "registryNamespace": " registry ", "registryName": "zot"}),
        call("trim", "trim_guest_storage", {"uri": K1}),
        task_call("prune", "prune_unused_cluster_images", {"uri": K1, "dryRun": True, "minAgeSeconds": 7200}),
        task_call("pruneYoung", "prune_unused_cluster_images", {"uri": K1, "dryRun": True, "minAgeSeconds": 60}),
        task_call("pruneLive", "prune_unused_cluster_images", {"uri": K1, "extraKeepTags": ["keep:1"]}),
        task_call("gc", "garbage_collect_cluster_registry", {"uri": K1, "dryRun": True, "includeRegistry": True}),
        task_call("gcLive", "garbage_collect_cluster_registry", {"uri": K1}),
        task_call("gcMissing", "garbage_collect_cluster_registry", {"uri": K1, "includeRegistry": True,
                                                                    "registryName": "absent"}),
        STOP], DOMAIN + ["internal/domain/kubernetes/guest_storage.go", "plugins/kubernetes/k3s/cmd/opute-provider-k3s/storage.go",
                         "plugins/kubernetes/k3s/cmd/opute-provider-k3s/storage_registry.go"], FIXTURE,
        # The provider sizes the guest's directories concurrently: the
        # calls are compared as a set, the answers in order.
        sets=[["trace"]]),
    # The cluster agent: a VM install into k1's scripted guest; a cluster
    # with no VM installs host-native, which stops at Go's own sudo refusal.
    scenario("agent", [
        serve(OPUTE_PLATFORM_URL="http://127.0.0.1:9", OPUTE_PLATFORM_PUBLIC_URL="https://platform.example",
              OPUTE_PLATFORM_GUEST_HOST="10.0.0.1"), *k3s_install(),
        task_call("vm", "install_cluster_agent", {"vmName": "k1", "clusterId": "k1", "clusterName": "k1",
                                                  "agentId": "agent-1",
                                                  "bridgeUrl": "wss://bridge.example/agent",
                                                  "bridgeToken": "token-1", "apiEndpoint": "https://api.example"},
                  timeout=120),
        task_call("vmLoopback", "install_cluster_agent", {"vmName": "k1", "clusterId": "k1", "clusterName": "k1",
                                                          "agentId": "agent-1", "bridgeUrl": "http://127.0.0.1:9093",
                                                          "bridgePort": 9093, "bridgeToken": "token-1"},
                  timeout=120),
        task_call("noVm", "install_cluster_agent", {"source": "k3s-vm", "clusterId": "k1",
                                                    "clusterName": "k1"}),
        task_call("hostNative", "install_cluster_agent", {"source": "k3s-host", "clusterId": "h1",
                                                          "clusterName": "h1",
                                                          "agentId": "agent-2",
                                                          "bridgeUrl": "wss://bridge.example/agent",
                                                          "bridgeToken": "token-2"}),
        STOP], DOMAIN + ["internal/domain/cluster/cluster_agent.go",
                         "internal/domain/cluster/cluster_agent_bridge_relay.go"], "cluster-domain-agent"),
    cpc("k3s-vm", "OPUTE_K3S_VM", "c1"),
    cpc("gateway-vm", "OPUTE_DOGFOOD_GATEWAY_VM", "k1"),
    cpc("k3s-container", "OPUTE_K3S_CONTAINER", "absent"),
    # Unpublished Go tools stay unreachable on both sides.
    scenario("unreachable", [
        serve(), *k3s_install(),
        call("listClusters", "list_clusters", {}),
        call("details", "get_cluster_details", {"vmName": "k1"}),
        call("runtimeDetails", "get_cluster_runtime_details", {"vmName": "k1"}),
        call("uninstall", "uninstall_helm_chart", {"uri": K1, "releaseName": "web"}),
        STOP], ["internal/tools/catalog.go", "internal/tools/dispatch_cluster.go"], FIXTURE),
]

if __name__ == "__main__":
    out = Path(sys.argv[1]) if len(sys.argv) > 1 else HERE / "scenarios" / "cluster-domain.json"
    for name, fixture in FIXTURES.items():
        (HERE / "fixtures" / "shims" / f"{name}.json").write_text(json.dumps(fixture, indent=1) + "\n")
    out.write_text(json.dumps(cases, indent=1) + "\n")
    print(f"wrote {len(cases)} scenarios to {out} and {len(FIXTURES)} fixtures")
