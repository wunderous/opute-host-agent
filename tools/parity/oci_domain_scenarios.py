#!/usr/bin/env python3
"""Generate the M8d oci-domain T1 matrix: scenarios/oci-domain.json and its
shim fixtures (fixtures/shims/oci-domain-*.json).

Run from tools/parity: python3 oci_domain_scenarios.py

The image builder, the image store and the package manager are recording
shims: podman, buildah, sudo and apt-get. Every fixture shims sudo, so no
package install ever reaches the host, and no fixture lets a build or a push
leave the sandbox. Each family gets success, typed-failure and
partial-failure cases; every case collects the shim trace, so the argv,
flags and stdin hash are compared, not only the answer.

- **builder:** podman ready, podman installed but not ready (buildah
  answers), and nothing installed (apt-get through sudo installs it). The
  fallback order is Go's: podman when ready, then buildah, then buildkitd.
- **staging and build-and-push:** a context staged under the HOME
  allowlist, then built and pushed by each builder path. The staged file
  list is a Go map walk, so it is compared as a set.
- **storage:** the policy file (bytes and mode), inspection, and cleanup by
  age, budget and dry run, including an image several tags share.
- **registry:** the k3s provider of the cluster-domain matrix, bound by the
  VM URI that carries the provider instance name.

Calls use Go's selector values (D23): Rust accepts them as hidden aliases
and publishes the neutral value in results, which the D19 divergence maps
back.
"""
import copy
import json
import sys
from pathlib import Path

import cluster_domain_scenarios as cds

HERE = Path(__file__).resolve().parent
OWNER = "parity-owner"
SERVE = cds.SERVE
STOP = {"as": "stop", "stop": {}}
K1 = cds.K1
call, task_call, flat, rule, kubectl = cds.call, cds.task_call, cds.flat, cds.rule, cds.kubectl


def serve(**env):
    s = copy.deepcopy(SERVE)
    s["serve"]["env"].update(env)
    return s


TS = "wall-clock timestamp of this side's run"
MASKS = [{"path": ["steps", "*", "body", "result", k], "type": "rfc3339", "reason": TS, "optional": True}
         for k in ("createdAt", "lastUpdatedAt")]
RESULT = ["body", "result", "result", "structuredContent"]
STAGED_FILES = ["steps", "*", *RESULT, "files"]


def scenario(id_, steps, anchors, fixture, masks=(), sets=(), neutral=True, cluster=False):
    """neutral: the scenario's Rust side publishes a D23 value (or a D19
    name), which the declared divergence maps back to Go's spelling.
    cluster: the scenario installs the k3s provider exactly as
    provider-real.k3s does, so that comparison applies too."""
    compare = copy.deepcopy(cds.REAL_K3S["compare"]) if cluster else {"masks": []}
    have = {json.dumps(m["path"]) for m in compare["masks"]}
    compare["masks"] += [m for m in MASKS + list(masks) if json.dumps(m["path"]) not in have]
    # Staging walks a Go map, so its file list is compared as a set.
    compare["sets"] = compare.get("sets", []) + [list(p) for p in (STAGED_FILES, *sets)]
    compare.pop("divergences", None)
    if neutral or cluster:
        compare["divergences"] = ["D19.neutral-host-tool-names"]
    return {"id": "oci-domain." + id_, "surface": "oci-domain", "owner": "oci",
            "anchors": anchors, "fixture": fixture, "collect": ["trace", "orphans"],
            "steps": flat(steps), "compare": compare}


# --- fixtures -----------------------------------------------------------------------------

def compact(value):
    return json.dumps(value, separators=(",", ":"))


DEFAULT = {"exit": 1, "stderr": "unsupported parity shim invocation\n"}
INFO = {"host": {"arch": "amd64", "os": "linux"},
        "store": {"graphRoot": "/var/lib/parity/containers/storage", "graphDriverName": "overlay",
                  "runRoot": "/run/parity/containers"},
        "version": {"Version": "4.9.3"}}
AAA, BBB, CCC, DDD, EEE = ("sha256:aaa111", "sha256:bbb222", "sha256:ccc333", "sha256:ddd444",
                           "sha256:eee555")
IMAGES = [
    {"Id": AAA, "Created": 1000000000, "Containers": 0, "Size": 2000000000, "Names": ["registry.local/app:old"]},
    {"Id": BBB, "Created": 1100000000, "Containers": 0, "Size": 1000000000,
     "Names": ["registry.local/app:v1", "registry.local/app:rollback"]},
    {"Id": CCC, "Created": 1050000000, "Containers": 1, "Size": 700000000, "Names": ["registry.local/db:1"]},
    {"Id": DDD, "Created": 4102444800, "Containers": 0, "Size": 300000000, "Names": ["registry.local/app:next"]},
    {"Id": EEE, "CreatedAt": "2002-01-01T00:00:00Z", "Containers": 0, "Size": 500000000, "Names": []},
]


def df(images, reclaimable):
    return [{"Type": "Images", "Total": 5, "Active": 1, "RawSize": images, "RawReclaimable": reclaimable,
             "Size": "ignored", "Reclaimable": "ignored"},
            {"Type": "Containers", "Total": 1, "Active": 1, "RawSize": 100000000, "RawReclaimable": 0},
            {"Type": "Local Volumes", "Total": 1, "Active": 0, "RawSize": "200MB", "RawReclaimable": "200MB"}]


def podman_rules():
    """A ready podman with five images: two age-eligible and unused (aaa,
    eee), one shared by two tags whose removal is refused (bbb), one in use
    (ccc) and one newer than any cutoff (ddd). Removing aaa, then eee,
    shrinks the reported image bytes."""
    gone_a, gone_e = "markers/aaa-removed", "markers/eee-removed"
    return [
        rule(["info", "--format", "json"], INFO),
        rule(["system", "df", "--format", "json"], df(3500000000, 1500000000), ifExists=gone_e),
        rule(["system", "df", "--format", "json"], df(4000000000, 2000000000), ifExists=gone_a),
        rule(["system", "df", "--format", "json"], df(6000000000, 3500000000)),
        rule(["images", "--format", "json"], [i for i in IMAGES if i["Id"] not in (AAA, EEE)], ifExists=gone_e),
        rule(["images", "--format", "json"], [i for i in IMAGES if i["Id"] != AAA], ifExists=gone_a),
        rule(["images", "--format", "json"], IMAGES),
        rule(["image", "rm", AAA], "Untagged: registry.local/app:old\nDeleted: aaa111\n", creates=[gone_a]),
        rule(["image", "rm", EEE], "Deleted: eee555\n", creates=[gone_e]),
        rule(["image", "rm", BBB], exit_=2,
             stderr="Error: unable to delete image \"bbb222\" by ID with more than one tag "
                    "([registry.local/app:v1 registry.local/app:rollback]): please force removal\n"),
        rule(["build", "-f", "...", ], "STEP 1/2: FROM scratch\nCOMMIT\n", argvContains="app:nobuild",
             exit_=1, stderr="Error: building at STEP \"RUN make\": exit status 2\n"),
        rule(["build", "..."], "STEP 1/2: FROM scratch\nSTEP 2/2: COPY . /app\nCOMMIT\n"),
        rule(["push", "..."], exit_=125, stderr="Error: denied: requested access to the resource is denied\n",
             argvContains="app:nopush"),
        rule(["push", "..."], "Copying blob 1234\nWriting manifest to image destination\n"),
        rule(["image", "untag", "127.0.0.1:5000/team/app:keep"], exit_=125,
             stderr="Error: 127.0.0.1:5000/team/app:keep: image not known\n"),
        rule(["image", "untag", "..."]),
    ]


def matched(rules):
    """argvContains narrows a prefix rule: it matches only when both hold.
    The shim knows one matcher per rule, so the narrowed rules become
    argvContains rules guarded by the tool's own verb in this fixture."""
    out = []
    for r in rules:
        if "argvContains" in r and "argvPrefix" in r:
            r = {k: v for k, v in r.items() if k not in ("argvPrefix", "argv")}
        out.append(r)
    return out


def sudo_rules(install_ok=("podman", "buildah")):
    rules = [rule(["-n", "apt-get", "update"], "Reading package lists... Done\n")]
    for package in ("podman", "buildah", "moby-buildkit"):
        command = "buildkitd" if package == "moby-buildkit" else package
        if package in install_ok:
            rules.append(rule(["-n", "apt-get", "install", "-y", package], f"Setting up {package} ...\n",
                              creates=[f"shims/{command}"]))
        else:
            rules.append(rule(["-n", "apt-get", "install", "-y", package], exit_=100,
                              stderr=f"E: Unable to locate package {package}\n"))
    rules.append(rule(["..."], exit_=1, stderr="sudo: a password is required\n"))
    return rules


APT = [rule(["..."], exit_=100, stderr="E: Could not open lock file - open (13: Permission denied)\n")]
BUILDAH = [
    rule(["bud", "..."], "STEP 1/2: FROM scratch\nCOMMIT 127.0.0.1:5000/team/app:1\n"),
    rule(["push", "..."], "Writing manifest to image destination\n"),
]
NOT_READY = [rule(["..."], exit_=125, stderr="Error: unable to connect to Podman socket: no such file\n")]

FIXTURES = {
    "oci-domain-podman": {
        "description": "M8d: a ready podman with five images (two age-eligible and unused, one shared by "
                       "two tags, one in use, one new) and buildkitd; buildah is absent, and sudo apt-get "
                       "installs it.",
        "commands": {"podman": matched(podman_rules()), "buildkitd": [], "sudo": sudo_rules(install_ok=("buildah",)),
                     "apt-get": APT},
        "default": DEFAULT},
    "oci-domain-podman-down": {
        "description": "M8d: podman is installed but not ready; buildah answers builds and pushes, and "
                       "buildkitd is installed.",
        "commands": {"podman": NOT_READY, "buildah": BUILDAH, "buildkitd": [], "sudo": sudo_rules(install_ok=()),
                     "apt-get": APT},
        "default": DEFAULT},
    "oci-domain-buildkit-only": {
        "description": "M8d: podman is installed but not ready, and buildkitd is the only other builder.",
        "commands": {"podman": NOT_READY, "buildkitd": [], "sudo": sudo_rules(install_ok=()), "apt-get": APT},
        "default": DEFAULT},
    "oci-domain-bare": {
        "description": "M8d: no builder installed; sudo apt-get installs podman (a stub that answers every "
                       "call) and buildah, but finds no moby-buildkit.",
        "commands": {"sudo": sudo_rules(), "apt-get": APT},
        "default": DEFAULT},
    "oci-domain-apt-broken": {
        "description": "M8d: no builder installed and the package index update fails.",
        "commands": {"sudo": [rule(["-n", "apt-get", "update"], exit_=100,
                                   stderr="E: Failed to fetch http://archive.ubuntu.com/ubuntu  "
                                          "Temporary failure resolving\n"),
                              rule(["..."], exit_=1, stderr="sudo: a password is required\n")],
                     "apt-get": APT},
        "default": DEFAULT},
    "oci-domain-no-apt": {
        "description": "M8d: nothing installed and no apt-get on the agent's PATH (the shim directory "
                       "alone); sudo is still shimmed.",
        "commands": {"sudo": [rule(["..."], exit_=1, stderr="sudo: a password is required\n")]},
        "default": DEFAULT},
}

FLEX_PVC = {"apiVersion": "v1", "kind": "PersistentVolumeClaim",
            "metadata": cds.meta("flex-data", "registry-system", "pvc-2"),
            "spec": {"accessModes": ["ReadWriteOnce"], "resources": {"requests": {"storage": "10Gi"}}}}
PVC = {"apiVersion": "v1", "kind": "PersistentVolumeClaim",
       "metadata": cds.meta("grown-data", "registry-system", "pvc-1"),
       "spec": {"accessModes": ["ReadWriteOnce"], "storageClassName": "local-path",
                "resources": {"requests": {"storage": "10Gi"}}}}
REGISTRY_DEPLOYMENT = {"apiVersion": "apps/v1", "kind": "Deployment",
                       "metadata": cds.meta("local-registry", "registry-system", "d-9"),
                       "spec": {"replicas": 1}, "status": {"readyReplicas": 1}}
REGISTRY_RULES = [
    kubectl(["get", "pvc", "local-registry-data", "-o", "json", "-n", "registry-system"], exit_=1,
            stderr='Error from server (NotFound): persistentvolumeclaims "local-registry-data" not found\n'),
    kubectl(["get", "pvc", "grown-data", "-o", "json", "-n", "registry-system"], PVC),
    kubectl(["get", "pvc", "flex-data", "-o", "json", "-n", "registry-system"], FLEX_PVC),
    kubectl(["get", "pvc", "broken-data", "-o", "json", "-n", "registry-system"], exit_=1,
            stderr="The connection to the server 127.0.0.1:6443 was refused\n"),
    kubectl(["get", "storageclass", "local-path", "-o", "json"],
            {"metadata": cds.meta("local-path"), "provisioner": "rancher.io/local-path"}),
    kubectl(["get", "storageclass", "expandable", "-o", "json"],
            {"metadata": cds.meta("expandable"), "allowVolumeExpansion": True}),
    kubectl(["get", "deployment", "local-registry", "-o", "json", "-n", "registry-system"], REGISTRY_DEPLOYMENT),
    kubectl(["get", "deployment", "absent", "-o", "json", "-n", "registry-system"], exit_=1,
            stderr='Error from server (NotFound): deployments.apps "absent" not found\n'),
    kubectl(["delete", "namespace", "registry-system", "--ignore-not-found=true"],
            'namespace "registry-system" deleted\n'),
]
# configure_oci_storage and the registry tools bind a cluster (their catalog
# requires a cluster uri), so every fixture carries the cluster-domain estate
# (k1 with k3s, through the pinned provider) and a registry namespace: no PVC
# for the default registry, a 10Gi PVC for 'grown', and a PVC read that fails
# for 'broken'. The OCI commands are this matrix's own; sudo is always shimmed.
for fixture in FIXTURES.values():
    fixture["commands"] = {**cds.FIXTURES["cluster-domain-estate"]["commands"],
                           "incus": REGISTRY_RULES + cds.estate_rules(), **fixture["commands"]}
    fixture["description"] += " The cluster-domain estate (k1 with k3s) answers the provider."

# --- scenarios ----------------------------------------------------------------------------

OCI = "internal/domain/oci/"
DISPATCH = "internal/tools/dispatch_oci.go"
BUILDER = [OCI + "builder.go", OCI + "container_runtime.go", DISPATCH, "internal/exec/process.go"]
STAGE = [OCI + "stage_build_context.go", DISPATCH]
BUILD = [OCI + "build_and_push.go", OCI + "container_runtime.go", OCI + "storage.go", DISPATCH]
STORAGE = [OCI + "storage.go", OCI + "container_runtime.go", DISPATCH]
REGISTRY = [OCI + "registry.go", OCI + "registry_storage.go", DISPATCH] + cds.DOMAIN
POLICY = "${SANDBOX}/xdg/host-resource-coordinator/oci-storage-policy.json"
CONTEXT = "${SANDBOX}/home/.opute/build-contexts/app"


def read_policy(as_):
    return {"as": as_, "readFile": {"path": POLICY}}


def write_policy(as_, content):
    return {"as": as_, "write": {"path": POLICY, "content": content, "mode": "0600"}}


def stage(as_, files, dest=CONTEXT, **extra):
    return task_call(as_, "stage_build_context", {"destDir": dest, "files": files, **extra})


def build(as_, image, **extra):
    return task_call(as_, "build_and_push_oci_image", {"contextDir": CONTEXT, "image": image, **extra})


def configure(as_, args):
    """configure_oci_storage binds the cluster its catalog entry requires."""
    return task_call(as_, "configure_oci_storage", {"uri": K1, **args})


APP = {"Dockerfile": "FROM scratch\nCOPY . /app\n", "src/main.txt": "hello\n"}
DIGEST = "127.0.0.1:5000/team/app@sha256:" + "ab" * 32

cases = [
    # Go's fallback order with podman ready: auto and podman answer podman;
    # buildah is installed through sudo apt-get and then found; buildkit's
    # package is missing; an unknown value is refused by the schema.
    scenario("builder.podman", [
        serve(),
        {"as": "list", "mcp": {"method": "tools/list"}},
        task_call("auto", "ensure_oci_builder", {}),
        task_call("autoExplicit", "ensure_oci_builder", {"builder": "auto"}),
        task_call("podman", "ensure_oci_builder", {"builder": "podman"}),
        task_call("buildah", "ensure_oci_builder", {"builder": "buildah"}),
        task_call("buildahAgain", "ensure_oci_builder", {"builder": "buildah"}),
        task_call("buildkit", "ensure_oci_builder", {"builder": "buildkit"}),
        task_call("unknown", "ensure_oci_builder", {"builder": "docker"}),
        STOP], BUILDER, "oci-domain-podman"),
    # podman is installed but not ready: auto falls back to buildah, an
    # explicit podman reports the readiness error.
    scenario("builder.podman-down", [
        serve(),
        task_call("auto", "ensure_oci_builder", {}),
        task_call("podman", "ensure_oci_builder", {"builder": "podman"}),
        task_call("buildah", "ensure_oci_builder", {"builder": "buildah"}),
        task_call("buildkit", "ensure_oci_builder", {"builder": "buildkit"}),
        STOP], BUILDER, "oci-domain-podman-down"),
    # buildkitd is the last fallback; a build with it has no image store and
    # no buildah, so it is refused before anything runs.
    scenario("builder.buildkit-only", [
        serve(),
        task_call("auto", "ensure_oci_builder", {}),
        task_call("buildkit", "ensure_oci_builder", {"builder": "buildkit"}),
        stage("stage", APP),
        build("buildAuto", "127.0.0.1:5000/team/app:1"),
        build("buildBuildkit", "127.0.0.1:5000/team/app:1", builder="buildkit"),
        STOP], BUILDER + BUILD, "oci-domain-buildkit-only"),
    # Nothing installed: auto installs podman, whose stub answers every
    # call, so the next auto finds it ready.
    scenario("builder.install", [
        serve(),
        task_call("auto", "ensure_oci_builder", {}),
        task_call("autoAgain", "ensure_oci_builder", {"builder": "auto"}),
        task_call("buildkit", "ensure_oci_builder", {"builder": "buildkit"}),
        STOP], BUILDER, "oci-domain-bare"),
    scenario("builder.apt-broken", [
        serve(),
        task_call("auto", "ensure_oci_builder", {}),
        task_call("buildah", "ensure_oci_builder", {"builder": "buildah"}),
        STOP], BUILDER, "oci-domain-apt-broken", neutral=False),
    scenario("builder.no-apt", [
        serve(PATH="${SANDBOX}/shims"),
        task_call("auto", "ensure_oci_builder", {}),
        task_call("buildkit", "ensure_oci_builder", {"builder": "buildkit"}),
        STOP], BUILDER, "oci-domain-no-apt", neutral=False),
    # Staging: the HOME allowlist, both encodings, and each refusal.
    scenario("stage", [
        serve(),
        stage("app", APP),
        stage("binary", {"blob.bin": "AAECAwQ="}, dest=CONTEXT + "/bin", fileEncoding="base64"),
        stage("again", {"Dockerfile": "FROM scratch\n"}, fileEncoding="utf8"),
        stage("upper", {"Dockerfile": "FROM scratch\n"}, fileEncoding=" UTF8 "),
        {"as": "dockerfile", "readFile": {"path": CONTEXT + "/Dockerfile"}},
        {"as": "blob", "readFile": {"path": CONTEXT + "/bin/blob.bin"}},
        stage("outside", {"a": "b"}, dest="${SANDBOX}/elsewhere"),
        stage("relative", {"a": "b"}, dest="relative/dir"),
        stage("noDest", {"a": "b"}, dest="  "),
        stage("noFiles", {}),
        stage("dotdot", {"../escape": "x"}),
        stage("absolute", {"/etc/escape": "x"}),
        stage("blank", {" ": "x"}),
        stage("badBase64", {"x": "not base64!"}, fileEncoding="base64"),
        stage("badEncoding", {"x": "y"}, fileEncoding="hex"),
        {"as": "escaped", "readFile": {"path": "${SANDBOX}/home/.opute/build-contexts/escape"}},
        STOP], STAGE, "oci-domain-podman", neutral=False),
    # Build and push with podman: build args sorted, the insecure push, the
    # untag and its two skips, and each refusal before anything runs.
    scenario("build.podman", [
        serve(),
        stage("stage", APP),
        build("build", "127.0.0.1:5000/team/app:1", builder="podman", insecureRegistry=True,
              platform="linux/amd64", buildArgs={"VERSION": "1.2.3", "API_URL": "https://api.example"}),
        build("auto", " 127.0.0.1:5000/team/app:2 "),
        build("digest", DIGEST, builder="auto"),
        build("keepWarning", "127.0.0.1:5000/team/app:keep", builder="podman"),
        build("dockerfile", "127.0.0.1:5000/team/app:3", dockerfile="src/main.txt"),
        build("buildFails", "127.0.0.1:5000/team/app:nobuild"),
        build("pushFails", "127.0.0.1:5000/team/app:nopush"),
        build("noContext", "127.0.0.1:5000/team/app:1", contextDir=" "),
        build("noImage", " "),
        build("badImage", "team/app:1;rm"),
        build("contextMissing", "127.0.0.1:5000/team/app:1", contextDir="${SANDBOX}/home/absent"),
        build("dockerfileMissing", "127.0.0.1:5000/team/app:1", dockerfile="Containerfile"),
        build("badArgName", "127.0.0.1:5000/team/app:1", buildArgs={"1BAD": "x"}),
        build("badArgValue", "127.0.0.1:5000/team/app:1", buildArgs={"GOOD": "a\nb"}),
        build("buildkit", "127.0.0.1:5000/team/app:4", builder="buildkit"),
        STOP], BUILD + STAGE, "oci-domain-podman"),
    # Without a ready podman, buildah builds and pushes; no untag runs.
    scenario("build.buildah", [
        serve(),
        stage("stage", APP),
        build("buildah", "127.0.0.1:5000/team/app:1", builder="buildah", insecureRegistry=True,
              platform="linux/arm64", buildArgs={"A": "1"}),
        build("auto", "127.0.0.1:5000/team/app:2"),
        build("buildkit", "127.0.0.1:5000/team/app:3", builder="buildkit"),
        build("podman", "127.0.0.1:5000/team/app:4", builder="podman"),
        STOP], BUILD + STAGE, "oci-domain-podman-down"),
    # A storage budget is enforced before and after a podman build.
    scenario("build.policy", [
        serve(), *cds.k3s_install(),
        stage("stage", APP),
        configure("policy", {"maxBytes": 5368709120}),
        build("build", "127.0.0.1:5000/team/app:1", builder="podman"),
        STOP], BUILD + STORAGE + STAGE, "oci-domain-podman", cluster=True),
    # Inspection with the default policy, the explicit runtime, and the
    # policy file configure writes (bytes and mode).
    scenario("storage.inspect-configure", [
        serve(), *cds.k3s_install(),
        call("inspect", "inspect_container_storage", {}),
        call("inspectPodman", "inspect_container_storage", {"runtime": "podman"}),
        read_policy("noPolicy"),
        configure("configure", {"runtime": "podman", "maxBytes": 10737418240,
                                                         "minAgeSeconds": 7200}),
        read_policy("policy"),
        call("inspectPolicy", "inspect_container_storage", {"runtime": "auto"}),
        configure("keep", {"maxBytes": 0}),
        read_policy("policyKept"),
        configure("tooSmall", {"maxBytes": 1024}),
        configure("tooYoung", {"minAgeSeconds": 60}),
        configure("tooOld", {"minAgeSeconds": 31536001}),
        configure("badRuntime", {"runtime": "docker"}),
        task_call("noUri", "configure_oci_storage", {"maxBytes": 0}),
        read_policy("policyAfterRefusals"),
        STOP], STORAGE, "oci-domain-podman", cluster=True),
    # A policy file another agent wrote: invalid values and invalid JSON.
    scenario("storage.policy-file", [
        write_policy("invalid", '{"maxBytes": 5}'),
        serve(),
        call("inspectInvalid", "inspect_container_storage", {}),
        task_call("cleanupInvalid", "cleanup_container_storage", {"dryRun": True}),
        write_policy("broken", "{not json"),
        call("inspectBroken", "inspect_container_storage", {}),
        write_policy("zeroAge", '{"runtime": "podman", "maxBytes": 0, "minAgeSeconds": 0}'),
        call("inspectZeroAge", "inspect_container_storage", {}),
        STOP], STORAGE, "oci-domain-podman"),
    # Cleanup: a dry run, then the real cleanup without a budget (the shared
    # image is skipped and named, the build cache warning added).
    scenario("storage.cleanup", [
        serve(),
        task_call("dryRun", "cleanup_container_storage", {"dryRun": True, "minAgeSeconds": 3600}),
        task_call("cleanup", "cleanup_container_storage", {}),
        task_call("again", "cleanup_container_storage", {"runtime": "podman"}),
        task_call("badBudget", "cleanup_container_storage", {"maxBytes": 5}),
        task_call("badAge", "cleanup_container_storage", {"minAgeSeconds": 31536001}),
        STOP], STORAGE, "oci-domain-podman"),
    # A budget stops the cleanup once the store fits; pruneNow runs the same
    # cleanup from configure.
    scenario("storage.budget", [
        serve(), *cds.k3s_install(),
        configure("budget", {"maxBytes": 5368709120, "pruneNow": True}),
        read_policy("policy"),
        task_call("fits", "cleanup_container_storage", {"maxBytes": 5368709120}),
        STOP], STORAGE, "oci-domain-podman", cluster=True),
    scenario("storage.no-runtime", [
        serve(), *cds.k3s_install(),
        call("inspectAuto", "inspect_container_storage", {}),
        call("inspectPodman", "inspect_container_storage", {"runtime": "podman"}),
        configure("configure", {"maxBytes": 0}),
        read_policy("policy"),
        task_call("cleanup", "cleanup_container_storage", {"runtime": "podman"}),
        STOP], STORAGE, "oci-domain-podman-down", neutral=False, cluster=True),
    scenario("storage.not-installed", [
        serve(),
        call("inspectPodman", "inspect_container_storage", {"runtime": "podman"}),
        call("inspectAuto", "inspect_container_storage", {}),
        STOP], STORAGE, "oci-domain-bare", neutral=False),
    # Only the shared-host owner changes the policy or prunes.
    scenario("storage.shared-owner", [
        serve(OPUTE_HOST_AGENT_INSTANCE=OWNER, OPUTE_SHARED_HOST_OWNER_INSTANCE="parity-other-instance"), *cds.k3s_install(),
        configure("configure", {"maxBytes": 0}),
        task_call("cleanup", "cleanup_container_storage", {"dryRun": True}),
        call("inspect", "inspect_container_storage", {}),
        read_policy("policy"),
        STOP], STORAGE, "oci-domain-podman", cluster=True),
]

cases += [
    # The registry through the k3s provider: install admits the PVC size
    # (absent, grow-only, expansion), status and delete; without a binding
    # Go has no VM name.
    scenario("registry", [
        serve(), *cds.k3s_install(),
        task_call("install", "install_oci_registry", {"uri": K1}),
        task_call("installCustom", "install_oci_registry", {"uri": K1, "name": "grown", "storageSize": "10Gi",
                                                            "nodePort": 31000, "image": "registry:2.8.3"}),
        task_call("shrink", "install_oci_registry", {"uri": K1, "name": "grown", "storageSize": "5Gi"}),
        task_call("growFixed", "install_oci_registry", {"uri": K1, "name": "grown", "storageSize": "20Gi"}),
        task_call("growExpandable", "install_oci_registry", {"uri": K1, "name": "grown", "storageSize": "20Gi",
                                                             "storageClass": "expandable"}),
        task_call("growFlexible", "install_oci_registry", {"uri": K1, "name": "flex", "storageSize": "20Gi",
                                                           "storageClass": "expandable"}),
        task_call("growFlexibleDefault", "install_oci_registry", {"uri": K1, "name": "flex", "storageSize": "20Gi"}),
        task_call("badSize", "install_oci_registry", {"uri": K1, "name": "flex", "storageSize": "lots"}),
        task_call("pvcUnreadable", "install_oci_registry", {"uri": K1, "name": "broken"}),
        task_call("badNamespace", "install_oci_registry", {"uri": K1, "namespace": "Bad_NS"}),
        task_call("badPort", "install_oci_registry", {"uri": K1, "nodePort": 80}),
        task_call("badImage", "install_oci_registry", {"uri": K1, "image": "registry:3'"}),
        task_call("noBinding", "install_oci_registry", {}),
        call("status", "get_oci_registry_status", {"uri": K1}),
        call("statusAbsent", "get_oci_registry_status", {"uri": K1, "name": "absent"}),
        call("statusNoBinding", "get_oci_registry_status", {}),
        task_call("delete", "delete_oci_registry", {"uri": K1}),
        task_call("deleteBadNamespace", "delete_oci_registry", {"uri": K1, "namespace": "-x"}),
        task_call("deleteNoBinding", "delete_oci_registry", {}),
        cds.REGISTRY, STOP], REGISTRY, "oci-domain-podman", cluster=True),
]
if __name__ == "__main__":
    out = Path(sys.argv[1]) if len(sys.argv) > 1 else HERE / "scenarios" / "oci-domain.json"
    for name, fixture in FIXTURES.items():
        (HERE / "fixtures" / "shims" / f"{name}.json").write_text(json.dumps(fixture, indent=1) + "\n")
    out.write_text(json.dumps(cases, indent=1) + "\n")
    print(f"wrote {len(cases)} scenarios to {out} and {len(FIXTURES)} fixtures")
