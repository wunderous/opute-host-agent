"""A scripted K3s guest behind a fake `incus` (and `sudo`), for provider wire
corpora.

`serve()` writes `incus` and `sudo` executables into a temporary directory
that `env()` puts first on PATH; each forwards its argv and stdin to this
server, which logs the request and answers with the stdout, stderr and exit
status a guest would give. The same server answers an HA endpoint probe at
`/version` (401, a Kubernetes auth challenge). Nothing reaches a real Incus,
a real sudo or the network: any request that is not to loopback goes to a
dead proxy.

The instance name steers the guest:
  ha-a, ha-b   a two-server embedded-etcd cluster (hostname ha-a)
  trio         three servers, one not ready; `delete node` retires a member
  single       one ready server, embedded etcd
  sqlite       one ready server on sqlite
  joiner       the destination of a join: a ready member of a two-server view
  installed    a stopped guest with K3s installed; `container` privilege off
  fresh        a guest without K3s
  broken       every exec fails as a stopped instance does
  badnodes     `get nodes` answers a type error; noname a node without a name
  restartfail  `systemctl restart` fails; applyfail `kubectl apply` fails
  trimdenied   fstrim is refused (FITRIM), so the host fstrim runs; trimfail
               fstrim fails otherwise
  killed       every exec dies of SIGKILL
"""
import json
import os
import stat
import tempfile
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

_bindir = None
TOKEN_SHA = "4f" * 32
VERSION = "k3s version v1.31.8+k3s1 (33429f76)\ngo version go1.25.5\n"

FORWARDER = """#!/usr/bin/env python3
import json, os, sys, urllib.request
data = sys.stdin.buffer.read() if not sys.stdin.isatty() else b""
body = json.dumps({"tool": os.path.basename(sys.argv[0]), "argv": sys.argv[1:],
                   "stdin": data.decode("utf-8", "replace")}).encode()
request = urllib.request.Request(os.environ["K3S_FAKE"] + "exec", data=body, method="POST")
opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
answer = json.load(opener.open(request))
sys.stdout.write(answer["stdout"]); sys.stdout.flush()
sys.stderr.write(answer["stderr"]); sys.stderr.flush()
if answer["code"] == -9:
    os.kill(os.getpid(), 9)
sys.exit(answer["code"])
"""


def node(name, ready=True, roles=("control-plane",), version="v1.31.8+k3s1"):
    labels = {"kubernetes.io/hostname": name}
    labels.update({f"node-role.kubernetes.io/{r}": "true" for r in roles})
    item = {"metadata": {"name": name, "labels": labels},
            "status": {"conditions": [{"type": "MemoryPressure", "status": "False"},
                                      {"type": "Ready", "status": "True" if ready else "False"}]}}
    if version:
        item["status"]["nodeInfo"] = {"kubeletVersion": version}
    return item


class State:
    def __init__(self):
        self.lock = threading.Lock()
        self.log = []
        # One role label per node: Go ranges over the label map, so several
        # would come back in random order.
        self.nodes = {
            "ha-a": [node("ha-a"), node("ha-b", roles=("etcd",))],
            "ha-b": [node("ha-a"), node("ha-b", roles=("etcd",))],
            "trio": [node("trio"), node("trio-b", roles=("etcd",)), node("trio-c", ready=False),
                     node("trio-w", roles=("worker",))],
            "single": [node("single", roles=())],
            "sqlite": [node("sqlite", version="")],
            "joiner": [node("ha-a"), node("joiner")],
            "installed": [node("installed")],
        }
        self.registry_replicas = 1


def ok(stdout=""):
    return {"stdout": stdout, "stderr": "", "code": 0}


def fail(stderr, code=1, stdout=""):
    return {"stdout": stdout, "stderr": stderr, "code": code}


def guest(state, instance, argv, stdin):
    """One `incus exec <instance> -- argv...`."""
    joined = " ".join(argv)
    if instance == "broken":
        return fail("Error: Instance is not running\n")
    if instance == "killed":
        return {"stdout": "", "stderr": "", "code": -9}
    if argv[:1] == ["test"]:
        if argv[1:] == ["-x", "/usr/local/bin/k3s"]:
            return ok() if instance in state.nodes else fail("", 1)
        if argv[1:] == ["-d", "/var/lib/rancher/k3s/server/db/etcd/member"]:
            return ok() if instance not in ("sqlite", "installed") else fail("", 1)
        if argv[1:] == ["-f", "/var/lib/rancher/k3s/server/db/state.db"]:
            return ok() if instance == "sqlite" else fail("", 1)
    if argv == ["hostname"]:
        return ok({"ha-b": "ha-a"}.get(instance, instance) + "\n")
    if argv == ["sha256sum", "/var/lib/rancher/k3s/server/token"]:
        return ok(f"{TOKEN_SHA}  /var/lib/rancher/k3s/server/token\n") if instance != "sqlite" else fail("no token")
    if argv == ["cat", "/var/lib/rancher/k3s/server/token"]:
        return ok("K10fake::server:transient\n")
    if argv == ["k3s", "--version"]:
        if instance not in state.nodes and instance not in ("badnodes", "noname", "trimdenied", "trimfail", "restartfail", "applyfail"):
            return fail("exec: k3s: not found", 127)
        return ok(VERSION)
    if argv[:2] == ["systemctl", "restart"]:
        return fail("Job for k3s.service failed.") if instance == "restartfail" else ok()
    if argv[:2] == ["systemctl", "stop"] or argv[:2] == ["systemctl", "start"]:
        return ok()
    if argv[:3] == ["install", "-m", "0755"] or argv[:2] == ["rm", "-f"]:
        return ok()
    if argv[:2] == ["k3s", "kubectl"]:
        return kubectl(state, instance, argv[2:], stdin)
    if argv[:2] == ["k3s", "crictl"]:
        return crictl(instance, argv[2:])
    if argv == ["df", "-B1", "/"]:
        return ok("Filesystem      1B-blocks        Used   Available Use% Mounted on\n/dev/sda2   100000000000 40000000000 60000000000  40% /\n")
    if argv[:2] == ["du", "-sb"]:
        if argv[2].endswith("overlayfs"):
            return fail("du: cannot access 'overlayfs': No such file or directory")
        return ok(f"{len(argv[2]) * 1000}\t{argv[2]}\n")
    if argv[:1] == ["fstrim"]:
        if instance == "trimdenied":
            return fail("fstrim: /: FITRIM ioctl failed: Operation not permitted")
        if instance == "trimfail":
            return fail("fstrim: /: the discard operation is not supported")
        return ok("/: 2 GiB (2147483648 bytes) trimmed\n")
    if argv[:2] == ["bash", "-lc"]:
        return shell(state, argv[2], stdin)
    if argv[:2] == ["bash", "-s"]:
        return fail("[ERROR]  installer failed", 1)
    return fail(f"fake guest: unexpected command: {joined}")


def shell(state, script, stdin):
    if "registries.yaml" in script:
        return ok()
    if "k3s-uninstall.sh" in script:
        return ok("Uninstalled\n")
    if script.startswith("du -sb /var/lib/rancher/k3s/storage/*"):
        return ok("2048\t/var/lib/rancher/k3s/storage/pvc-b_data\n1024\t/var/lib/rancher/k3s/storage/pvc-a data\nbogus line\n")
    if script.startswith("curl -sI"):
        return ok("sha256:" + "d" * 64 + "\n") if "/old/app/" not in script else ok("\n")
    if script.startswith("curl -sf"):
        if "/v2/_catalog" in script:
            return ok(json.dumps({"repositories": ["opute/platform", "old/app", "bad repo", ""]}))
        if "/v2/opute/platform/tags/list" in script:
            return ok(json.dumps({"name": "opute/platform", "tags": ["old", "abc", " "]}))
        if "/v2/old/app/tags/list" in script:
            return ok(json.dumps({"name": "old/app", "tags": ["v1"]}))
        if "-X 'DELETE'" in script:
            return ok()
    if script.startswith("systemctl status"):
        return ok("k3s.service - Lightweight Kubernetes\n   Active: failed\nK10fake::server:transient leaked\n")
    if "read -r K3S_TOKEN" in script:
        return fail("[ERROR]  join failed", 1)
    return fail(f"fake guest: unexpected script: {script[:80]}")


def kubectl(state, instance, args, stdin):
    if args == ["get", "nodes", "-o", "json"]:
        if instance == "badnodes":
            return ok('{"items":[{"metadata":{"name":"x","labels":{"a":1}}}]}')
        if instance == "noname":
            return ok('{"items":[{"metadata":{"name":""}}]}')
        if instance not in state.nodes:
            return fail("The connection to the server 127.0.0.1:6443 was refused")
        return ok(json.dumps({"apiVersion": "v1", "items": state.nodes[instance]}))
    if args[:2] == ["delete", "node"]:
        state.nodes[instance] = [n for n in state.nodes[instance] if n["metadata"]["name"] != args[2]]
        return ok(f'node "{args[2]}" deleted\n')
    if args[:3] == ["apply", "-f", "-"]:
        if instance == "applyfail":
            return fail('error: error parsing STDIN: error converting YAML to JSON: yaml: line 2: mapping values are not allowed in this context')
        return ok("configured\n")
    if args[:2] == ["get", "events"]:
        events = [
            {"type": "Normal", "reason": "Pulled", "message": "pulled", "count": 1,
             "lastTimestamp": "2026-01-01T00:00:02Z", "metadata": {"name": "e1", "namespace": "default"}},
            {"type": "Warning", "reason": "BackOff", "eventTime": "2026-01-01T00:00:05.000000Z",
             "metadata": {"name": "e2", "namespace": "kube-system"}, "involvedObject": {"kind": "Pod", "name": "p"}},
            None,
            {"type": "Normal", "reason": "Old", "firstTimestamp": "2025-12-31T00:00:00Z"},
        ]
        return ok(json.dumps({"items": events}))
    if args[:1] == ["get"] and len(args) >= 3 and args[1] not in ("pods", "deploy", "svc", "job", "events"):
        name = args[2]
        if name == "missing":
            return fail(f'Error from server (NotFound): {args[1]} "missing" not found')
        if name == "garbage":
            return ok("not json")
        if name == "array":
            return ok("[1]")
        if name == "nullres":
            return ok("null")
        resources = {
            "web": {"kind": "Deployment", "spec": {"replicas": 2}, "status": {"readyReplicas": 2}},
            "slow": {"kind": "Deployment", "spec": {"replicas": 3}, "status": {"readyReplicas": 1}},
            "zero": {"kind": "Deployment", "spec": {}, "status": {"readyReplicas": 1}},
            "pod-run": {"kind": "Pod", "status": {"phase": "Running"}},
            "pod-pend": {"kind": "Pod", "status": {"phase": "Pending"}},
            "claim": {"kind": "PersistentVolumeClaim", "status": {"phase": "Bound"}},
        }
        doc = dict(resources.get(name, {"kind": args[1]}), metadata={"name": name})
        return ok(json.dumps(doc, indent=2) + "\n")
    if args[:1] == ["delete"]:
        return ok()
    if args == ["get", "pods", "-A", "-o", "json"]:
        pods = {"items": [
            {"spec": {"containers": [{"image": "10.43.0.50:5000/opute/platform:abc"}],
                      "initContainers": [{"image": "docker.io/library/busybox:1.36"}]}},
            {"spec": {"ephemeralContainers": [{"image": "registry:3"}]}},
        ]}
        return ok(json.dumps(pods))
    if args == ["get", "deploy", "-A", "-o", "json"]:
        deploys = {"items": [
            {"metadata": {"name": "web", "namespace": "default"}},
            {"metadata": {"name": "opute-registry", "namespace": "registry-system"},
             "spec": {"replicas": 1, "template": {"spec": {
                 "containers": [{"image": " registry:3 "}],
                 "volumes": [{"name": "config", "configMap": {"name": "opute-registry-config"}},
                             {"name": "data", "persistentVolumeClaim": {"claimName": "opute-registry-data"}}]}}}},
        ]}
        return ok(json.dumps(deploys))
    if args[:1] == ["-n"]:
        rest = args[2:]
        if rest[:2] == ["get", "svc"]:
            return ok(json.dumps({"spec": {"clusterIP": "10.43.0.50", "ports": [{"port": 5000}]}}))
        if rest[:1] == ["scale"]:
            state.registry_replicas = int(rest[2].split("=")[1])
            return ok()
        if rest[:2] == ["get", "deploy"]:
            return ok(json.dumps({"status": {"replicas": state.registry_replicas}}))
        if rest[:1] == ["wait"] or rest[:2] == ["delete", "job"]:
            return ok()
        if rest[:2] == ["get", "job"]:
            return ok("0")
    if args[:1] in (["version"], ["get"]):
        return ok(f"args: {' '.join(args)}\nstdin: {stdin!r}\n")
    return fail(f"fake kubectl: unexpected: {' '.join(args)}")


def crictl(instance, args):
    if args == ["images", "-o", "json"]:
        images = {"images": [
            {"id": "sha256:keep", "repoTags": ["docker.io/library/busybox:1.36"], "size": "1000", "createdAt": "1577836800"},
            {"id": "sha256:old", "repoTags": ["docker.io/library/old:1", "<none>"], "size": 2000, "createdAt": 1577836800},
            {"id": "sha256:nano", "repoTags": [], "repoDigests": ["x@sha256:aa"], "size": 3000.0, "createdAt": "1577836800000000000"},
            {"id": "sha256:future", "repoTags": ["new:1"], "size": "40", "createdAt": "2099-01-01T00:00:00Z"},
            {"id": "sha256:undated", "repoTags": ["u:1"], "size": "bogus"},
            {"id": "sha256:pinned", "repoTags": ["pause:3.9"], "size": 5, "pinned": True},
        ]}
        return ok(json.dumps(images))
    if args == ["ps", "-a", "-o", "json"]:
        containers = {"containers": [
            {"id": "c-run", "state": "CONTAINER_RUNNING", "image": {"image": "docker.io/library/new:1"}, "imageRef": "sha256:future"},
            {"id": "c-exit", "state": "CONTAINER_EXITED", "image": {"image": "old:1"}, "imageRef": "sha256:old"},
            {"id": " ", "state": "CONTAINER_EXITED"},
        ]}
        return ok(json.dumps(containers))
    if args[:1] == ["rm"]:
        return ok()
    if args[:1] == ["rmi"]:
        return fail("image is in use") if args[1] == "sha256:nano" else ok()
    return fail(f"fake crictl: unexpected: {' '.join(args)}")


def incus(state, argv, stdin):
    if argv == ["list", "--format", "json"]:
        return ok(json.dumps([
            {"name": "ha-a", "type": "container", "status": "Running"},
            {"name": "single", "type": "virtual-machine", "status": "Running"},
            {"name": "fresh", "type": "container", "status": "Stopped"},
            {"name": "", "type": "container"},
            {"name": "image-1", "type": "image"},
            None,
        ]))
    if argv[:1] == ["exec"] and "--" in argv:
        split = argv.index("--")
        return guest(state, argv[1], argv[split + 1:], stdin)
    if argv[:1] == ["start"]:
        return fail("Error: The instance is already running") if argv[1] in ("ha-a", "single") else ok()
    if argv[:1] == ["stop"]:
        return fail("Error: The instance is already stopped (not running)") if argv[1] == "installed" else ok()
    if argv[:2] == ["config", "get"]:
        return ok("true\n" if argv[3] == "security.privileged" else "\n")
    if argv[:2] == ["config", "set"] or argv[:2] == ["file", "push"]:
        return ok()
    return fail(f"Error: fake incus: unexpected: {' '.join(argv)}")


def serve():
    state = State()

    class Handler(BaseHTTPRequestHandler):
        def do_POST(self):
            raw = self.rfile.read(int(self.headers.get("Content-Length") or 0))
            request = json.loads(raw)
            with state.lock:
                state.log.append(request)
                if request["tool"] == "sudo":
                    answer = ok("/: 1 GiB (1073741824 bytes) trimmed on /dev/sda2\n")
                else:
                    answer = incus(state, request["argv"], request["stdin"])
            self.reply(200, json.dumps(answer))

        def do_GET(self):
            with state.lock:
                state.log.append({"probe": self.path})
            self.reply(401, '{"kind":"Status","code":401}')

        def reply(self, status, text):
            data = text.encode()
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def log_message(self, *args):
            pass

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    bindir = tempfile.mkdtemp(prefix="fake-k3s-")
    for tool in ("incus", "sudo"):
        path = os.path.join(bindir, tool)
        with open(path, "w") as f:
            f.write(FORWARDER)
        os.chmod(path, os.stat(path).st_mode | stat.S_IXUSR)
    global _bindir
    _bindir = bindir
    return f"http://127.0.0.1:{server.server_address[1]}/", state, server


def env(base: str) -> list[str]:
    """The fake tools first on PATH; anything not to loopback goes to a dead
    proxy."""
    return [f"PATH={_bindir}:/usr/bin:/bin", f"K3S_FAKE={base}", "HTTPS_PROXY=http://127.0.0.1:9",
            "HTTP_PROXY=http://127.0.0.1:9", "NO_PROXY="]


# Random per process: join references, their expiry, sealed material and the
# receiver key.
MASKS = [
    (r"join-[0-9a-f]{48}", "join-${REF}"),
    (r'"expiresAt": "[0-9T:.\-]+Z"', '"expiresAt": "${TIME}"'),
    (r'"(sealedJoinMaterial|publicKey)": "[A-Za-z0-9_\-]+"', r'"\1": "${KEY}"'),
    (r'"fingerprint": "sha256:[0-9a-f]{64}"', '"fingerprint": "sha256:${KEY}"'),
]
