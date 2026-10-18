#!/usr/bin/env python3
"""Generate scenarios-t2/oci-domain.json: the M8d oci-domain T2 matrix.

Run from tools/parity: python3 oci_t2_scenarios.py

Every scenario boots its twins from the OCI golden (`python3 -m parity t2
golden-oci`): the k3s golden plus podman and buildah from the Ubuntu
archive, run rootless as ubuntu, and two old images in ubuntu's store
(localhost/parity/old:1, and localhost/parity/shared under two tags). The
pinned Go k3s provider is installed exactly as in the cluster-domain T2
matrix: the registry tools and configure_oci_storage bind the cluster k1.

Images are built FROM scratch out of a context the agent stages, so no
image is pulled from outside the twin, and pushed only to the registry
install_oci_registry deploys in k1, through its NodePort on k1's reserved
address. Nothing outside the twin is touched: no shared Incus, Kubernetes,
registry, tunnel or deployment state.

The snapshots add the user's image store (image names; a build's own
timestamps make its id twin-local) and the OCI registry's catalog to the
cluster-domain projection.
"""
import copy
import json
import re
import sys

import cluster_t2_scenarios as ct
import t2_scenarios as hd
from parity import t2

SNAP, STOP, call, task_call, shell = hd.snap, hd.STOP, hd.call, hd.task_call, hd.shell
K1 = ct.K1
REGISTRY = f"{t2.OCI_K1_ADDRESS}:30500"
CONTEXT = "/home/ubuntu/.opute/build-contexts/app"
COMMANDS = ["podman", "buildah", "incus"]
OCI = "internal/domain/oci/"
DISPATCH = "internal/tools/dispatch_oci.go"


# The agent runs as a system service of ubuntu with no user session, as on a
# host where it is installed as one: no XDG_RUNTIME_DIR, so podman keeps its
# rootless runtime state in its own temporary directory. (The driver's
# default names /run/user/1000, which does not exist without a session.)
NO_SESSION = ["XDG_RUNTIME_DIR"]


def setup():
    steps = copy.deepcopy(ct.setup())
    steps[0]["serve"]["unset"] = NO_SESSION
    return steps


def scenario(id_, steps, anchors, residue, masks=(), rejected_only=False):
    for step in steps:
        if isinstance(step, dict) and "serve" in step:
            step["serve"]["unset"] = sorted(set(step["serve"].get("unset", [])) | set(NO_SESSION))
    out = ct.scenario(id_, steps, anchors, residue, masks=masks, rejected_only=rejected_only)
    out["id"] = "t2.oci-domain." + id_
    out["owner"] = "oci"
    out["golden"] = "oci"
    out["commands"] = [] if rejected_only else COMMANDS
    # Staging walks a Go map, so its file list is compared as a set.
    out["compare"]["sets"].append(["steps", "*", "body", "result", "result", "structuredContent", "files"])
    return out


APP = {"Dockerfile": "FROM scratch\nCOPY . /app\n", "src/main.txt": "hello from the parity twin\n"}
REGISTRY_ARGS = {"uri": K1, "image": "registry:2.8.3", "storageSize": "1Gi"}
# The registry's rollout: a pod on k1's own clock, waited for before a push.
# Compared by its exit status only, like the k3s readiness wait: kubectl
# prints the rollout's progress on its own clock.
REGISTRY_READY = shell("registryReady", ["sh", "-c", "incus exec k1 -- k3s kubectl -n registry-system rollout status "
                                                     "deploy/local-registry --timeout=600s >/dev/null"])
SERVER_ASSIGNED = "assigned by k1's API server and controllers; the Deployment is compared in the snapshot"
STATUS_MASKS = [
    {"path": ["steps", "status", "body", "result", "content", 0, "text"], "type": "string",
     "match": r"\{\"deployment\":.*", "reason": "the answer as text, with values " + SERVER_ASSIGNED},
    *[{"path": ["steps", "status", "body", "result", "structuredContent", "deployment", "metadata", key],
       "type": "string", "reason": SERVER_ASSIGNED, "optional": True}
      for key in ("uid", "resourceVersion", "creationTimestamp")],
    {"path": ["steps", "status", "body", "result", "structuredContent", "deployment", "metadata", "managedFields"],
     "type": "array", "reason": SERVER_ASSIGNED, "optional": True},
    *[{"path": ["steps", "status", "body", "result", "structuredContent", "deployment", "status", "conditions", "*",
                key], "type": "rfc3339", "reason": "when k1's controllers updated the condition", "optional": True}
      for key in ("lastUpdateTime", "lastTransitionTime")],
]

def push_refused(label, registry):
    """A push refused after its build: podman names the layer it tried, whose
    digest covers this twin's build timestamps, and stamps its retries; the
    registry it was refused by is compared."""
    return {"path": ["steps", label + "Done", "body", "result", "result", "content", 0, "text"], "type": "string",
            "match": r"(?s)Error: push image: exit status 125: Getting image source signatures\n"
                     r"Copying blob sha256:[0-9a-f]{64}\n.*" + re.escape(f"pinging container registry {registry}: ")
                     + r".*",
            "reason": "the layer digest covers this twin's build timestamps; retries carry their own time"}


cases = [
    # The builder, a staged context, the registry in k1, and an image built
    # and pushed to it by podman (untagged after the push) and by buildah
    # (the legacy path keeps its tag); then the registry is deleted.
    scenario("build.push-to-twin-registry", [
        *setup(), SNAP("baseline"),
        task_call("builder", "ensure_oci_builder", {}),
        task_call("buildah", "ensure_oci_builder", {"builder": "buildah"}),
        task_call("stage", "stage_build_context", {"destDir": CONTEXT, "files": APP}),
        SNAP("staged", "baseline"),
        task_call("install", "install_oci_registry", REGISTRY_ARGS),
        REGISTRY_READY,
        call("status", "get_oci_registry_status", {"uri": K1}),
        SNAP("registry", "staged"),
        task_call("podman", "build_and_push_oci_image", {"contextDir": CONTEXT, "image": REGISTRY + "/parity/app:1",
                                                         "builder": "podman", "insecureRegistry": True,
                                                         "buildArgs": {"VERSION": "1"}}, timeout=900),
        SNAP("pushed", "registry"),
        task_call("legacy", "build_and_push_oci_image", {"contextDir": CONTEXT, "image": REGISTRY + "/parity/app:2",
                                                         "builder": "buildah", "insecureRegistry": True},
                  timeout=900),
        task_call("secure", "build_and_push_oci_image", {"contextDir": CONTEXT, "image": REGISTRY + "/parity/app:3"},
                  timeout=900),
        SNAP("legacyPushed", "pushed"),
        task_call("delete", "delete_oci_registry", {"uri": K1}, timeout=900),
        STOP, SNAP("teardown", "baseline"),
    ], [OCI + f for f in ("builder.go", "stage_build_context.go", "registry.go", "registry_storage.go",
                          "build_and_push.go", "container_runtime.go")] + [DISPATCH] + ct.DOMAIN,
        "The staged context stays. podman untags app:1 after its push and keeps the image (the storage policy "
        "decides when it goes); buildah's app:2 keeps its tag (the legacy path never untags); app:3, an identical "
        "podman build, tags that kept image again and keeps the tag, because its TLS push to the plain-HTTP "
        "registry is refused. The registry namespace is deleted from k1.",
        masks=STATUS_MASKS + [push_refused("secure", REGISTRY)]),
    # Storage: inspect, the policy file, a dry run, then the cleanup that
    # removes the old image and names the shared one it never forces; a
    # budget then holds the next build's policy runs.
    scenario("storage.policy-and-cleanup", [
        *setup(), SNAP("baseline"),
        call("inspect", "inspect_container_storage", {}),
        task_call("configure", "configure_oci_storage", {"uri": K1, "runtime": "podman", "minAgeSeconds": 3600}),
        SNAP("configured", "baseline"),
        task_call("dryRun", "cleanup_container_storage", {"dryRun": True}),
        SNAP("afterDryRun", "configured"),
        task_call("cleanup", "cleanup_container_storage", {}),
        SNAP("cleaned", "configured"),
        task_call("budget", "configure_oci_storage", {"uri": K1, "maxBytes": 1073741824, "pruneNow": True}),
        task_call("stage", "stage_build_context", {"destDir": CONTEXT, "files": APP}),
        task_call("buildOnly", "build_and_push_oci_image", {"contextDir": CONTEXT,
                                                            "image": "localhost:1/parity/unreachable:1"},
                  timeout=900),
        STOP, SNAP("teardown", "baseline"),
    ], [OCI + f for f in ("storage.go", "container_runtime.go", "build_and_push.go")] + [DISPATCH],
        "The policy file stays (1 GiB budget); the old image is removed and the shared one kept under both tags; the "
        "staged context stays, and the image built before the refused push keeps its tag.",
        masks=[push_refused("buildOnly", "localhost:1")]),
]

# Every M4-rejected oci call, with effectful arguments aimed at the twin's
# store, the registry and k1, leaves the twin unchanged.
REJECTED = [
    ("build_and_push_oci_image", {"contextDir": "/home/ubuntu", "image": REGISTRY + "/parity/rejected:1",
                                  "insecureRegistry": True}),
    ("cleanup_container_storage", {}),
    ("configure_oci_storage", {"uri": K1, "maxBytes": 0, "pruneNow": True}),
    ("delete_oci_registry", {"uri": K1, "namespace": "registry"}),
    ("ensure_oci_builder", {"builder": "buildkit"}),
    ("install_oci_registry", REGISTRY_ARGS),
    ("stage_build_context", {"destDir": "/home/ubuntu/.opute/build-contexts/rejected",
                             "files": {"Dockerfile": "FROM scratch\n"}}),
]
OCI_TOOLS = {t["tool"] for t in json.load(open("../../baseline/inventory/oci-domain.json"))["tools"]}
assert hd.M4_GATED & OCI_TOOLS == {n for n, _ in REJECTED}, sorted((hd.M4_GATED & OCI_TOOLS) ^ {n for n, _ in REJECTED})


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
    for i, (name, args) in enumerate(b for b in hd.M4_BINDING if b[0] in OCI_TOOLS):
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
], hd.ADMISSION + ["internal/domain/oci"],
    "None: the provider is installed before the baseline, and every rejected call is refused before it reaches "
    "the image store, the registry or k1.", rejected_only=True))

if __name__ == "__main__":
    out = sys.argv[1] if len(sys.argv) > 1 else "scenarios-t2/oci-domain.json"
    with open(out, "w") as f:
        json.dump(cases, f, indent=1)
        f.write("\n")
    print(f"wrote {len(cases)} T2 scenarios to {out}")
