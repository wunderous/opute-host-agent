#!/usr/bin/env python3
"""Generate the M8b incus-domain T1 matrix: scenarios/incus-domain.json and
its shim fixtures (fixtures/shims/incus-domain-*.json).

Run from tools/parity: python3 incus_domain_scenarios.py

Every instance operation runs against a recorded incus shim that scripts a
small owned estate: a running VM (vm1), a stopped system container (box1), a
VM listed as owned whose ownership label has since drifted (drift1), and an
absent name (new1). Each family gets success, typed-failure and
partial-failure cases; every case collects the shim trace, so the argv, flags
and stdin hash are compared, not only the answer, and the resource registry
rows the operations leave behind.

The developer host's workload slice is not enforced, so a new launch is
refused by host admission (host_resource_enforcement_unknown) on both sides
here. The launch argv itself runs in the T2 twins, where the slice is
installed first. The stack reset reinstalls through the recorded package
shims of the virt-install fixture: with the shim directory as the whole PATH,
no real package manager runs.
"""
import copy
import json
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
OWNER = "parity-owner"
LABEL = "user.opute.host_agent_instance"

SERVE = {"as": "start", "serve": {"argv": ["serve", "--mode=standalone"], "profile": "standalone", "env": {
    "OPUTE_STANDALONE_ALLOW_MUTATIONS": "true",
    "OPUTE_HOST_RESOURCE_ENFORCEMENT": "advisory",
    "OPUTE_HOST_MAX_NORMAL_OPERATIONS": "64", "OPUTE_HOST_MAX_HEAVY_OPERATIONS": "64",
    "OPUTE_HOST_RESOURCE_CPU_CAPACITY_CORES": "64",
    "OPUTE_HOST_RESOURCE_MEMORY_CAPACITY_BYTES": "68719476736"}}}
OWNED = {"OPUTE_HOST_AGENT_INSTANCE": OWNER, "OPUTE_INCUS_OWNERSHIP_MODE": "enforce"}
STOP = {"as": "stop", "stop": {}}


def serve(**env):
    s = copy.deepcopy(SERVE)
    s["serve"]["env"].update(env)
    return s


def call(as_, name, args):
    return {"as": as_, "mcp": {"method": "tools/call", "name": name,
                               "params": {"name": name, "arguments": args}, "expand": True}}


def task_call(as_, name, args):
    """A task-aware tool answers with a task handle; wait for the task."""
    var = "T_" + as_.upper()
    step = call(as_, name, args)
    step["mcp"]["capture"] = {var: ["body", "result", "taskId"]}
    done = {"as": as_ + "Done", "requires": [var], "mcp": {
        "method": "tasks/get", "name": "${%s}" % var, "params": {"taskId": "${%s}" % var}, "expand": True,
        "until": {"path": ["body", "result", "status"], "in": ["completed", "failed", "cancelled"]}, "timeout": 60}}
    return [step, done]


def flat(steps):
    out = []
    for step in steps:
        out.extend(step if isinstance(step, list) else [step])
    return out


TS = "wall-clock timestamp of this side's run"
MASKS = [{"path": ["steps", "*", "body", "result", k], "type": "rfc3339", "reason": TS, "optional": True}
         for k in ("createdAt", "lastUpdatedAt")]
REGISTRY = {"as": "registry", "query": {"db": "${SANDBOX}/state/state.db", "sql": {
    "resources": "SELECT uri, resource_type, tenant_id, resource_id, coordinates_json, status "
                 "FROM resource_registry ORDER BY uri"}}}
CHECKPOINT = "${SANDBOX}/instance/local-llm-relays/incus-reset-checkpoint.json"


def read_checkpoint(as_):
    return {"as": as_, "readFile": {"path": CHECKPOINT, "json": True}}


def scenario(id_, steps, anchors, fixture, masks=()):
    return {"id": "incus-domain." + id_, "surface": "incus-domain", "owner": "incus",
            "anchors": anchors, "fixture": fixture, "collect": ["trace"],
            "steps": flat(steps), "compare": {"masks": MASKS + list(masks)}}


# --- fixtures -----------------------------------------------------------------------------

def instance(name, status, kind, owner=OWNER, **config):
    cfg = {LABEL: owner, "limits.cpu": "2", "limits.memory": "2GiB", **config}
    return {"name": name, "status": status, "type": kind, "config": cfg,
            "devices": {"root": {"type": "disk", "path": "/", "pool": "default", "size": "10GiB"}},
            "state": {"network": {}}}


def compact(value):
    return json.dumps(value, separators=(",", ":"))


VM1 = instance("vm1", "Running", "virtual-machine")
BOX1 = instance("box1", "Stopped", "container")
DRIFT1 = instance("drift1", "Running", "virtual-machine")
ALIEN1 = instance("alien1", "Running", "container", owner="someone-else")
BOX_STATE = {"network": {"eth0": {"addresses": [
    {"family": "inet6", "address": "fd42::5", "scope": "global"},
    {"family": "inet", "address": "10.0.0.5", "scope": "global"}]}}}
PROFILE = {"devices": {"root": {"type": "disk", "path": "/", "pool": "default"},
                       "eth0": {"type": "nic", "name": "eth0", "network": "incusbr0"}}}
POOL_ZFS = {"name": "default", "driver": "zfs", "config": {"source": "tank/incus"}}
POOL_DIR = {"name": "default", "driver": "dir", "config": {"source": "/nonexistent/parity-pool"}}
DEFAULT = {"exit": 1, "stderr": "unsupported parity shim invocation\n"}


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


def estate_rules(pool):
    gone = "markers/vm1-deleted"
    return [
        rule(["list", "--format", "json"], [BOX1, DRIFT1], ifExists=gone),
        rule(["list", "--format", "json"], [VM1, BOX1, DRIFT1]),
        rule(["list", "vm1", "--format", "csv"], "vm1,RUNNING,virtual-machine", unlessExists=gone),
        rule(["list", "vm1", "--format", "csv"], ""),
        rule(["list", "box1", "--format", "csv"], "box1,STOPPED,container"),
        rule(["list", "drift1", "--format", "csv"], "drift1,RUNNING,virtual-machine"),
        rule(["list", "new1", "--format", "csv"], ""),
        rule(["config", "get", "vm1", LABEL], OWNER),
        rule(["config", "get", "box1", LABEL], OWNER),
        rule(["config", "get", "drift1", LABEL], "someone-else"),
        rule(["query", "/1.0/instances/vm1"], {"type": "virtual-machine", "devices": {
            "root": {"type": "disk", "path": "/", "pool": "default", "size": "10GiB"}}}),
        rule(["query", "/1.0/instances/box1"], {"type": "container", "devices": {}, "expanded_devices": {
            "root": {"type": "disk", "path": "/", "pool": "default"}}}),
        rule(["query", "/1.0/instances/box1/state"], BOX_STATE),
        rule(["query", "/1.0/profiles/default"], PROFILE),
        rule(["query", "/1.0/storage-pools/default"], pool),
        rule(["query", "/1.0/storage-pools"], ["/1.0/storage-pools/default"]),
        rule(["query", "-X", "PATCH", "--wait", "..."]),
        rule(["start", "vm1"], exit_=1, stderr="Error: The instance is already running\n"),
        rule(["start", "box1"]),
        rule(["stop", "vm1", "--force"]),
        rule(["stop", "box1", "--force"]),
        rule(["delete", "vm1", "--force"], creates=[gone]),
        rule(["config", "set", "..."]),
        rule(["config", "device", "show", "box1"], "eth0:\n  type: nic\n"),
        rule(["config", "device", "add", "..."]),
        rule(["config", "device", "get", "box1", "eth0", "ipv4.address"], ""),
        rule(["config", "device", "override", "box1", "eth0", "ipv4.address=10.0.0.9"]),
        rule(["info", "box1", "--format", "json"], {"name": "box1", "status": "Running"}),
        rule(["version"], "Client version: 6.0.4\nServer version: 6.0.4"),
    ]


def unmanaged_rules():
    pinned = "markers/box1-pinned"
    return [
        rule(["list", "box1", "--format", "csv"], "box1,STOPPED,container"),
        rule(["config", "get", "box1", LABEL], OWNER),
        rule(["query", "/1.0/instances/box1"], {"type": "container"}),
        rule(["query", "/1.0/instances/box1/state"], BOX_STATE),
        rule(["start", "box1"]),
        rule(["config", "set", "..."]),
        rule(["config", "device", "show", "box1"], "eth0:\n  type: nic\nkmsg:\n  type: unix-char\n"),
        rule(["config", "device", "get", "box1", "eth0", "ipv4.address"], ""),
        rule(["config", "device", "override", "box1", "eth0", "ipv4.address=10.0.0.9"], exit_=1,
             stderr="Error: Invalid devices: Device validation failed for \"eth0\": Cannot specify "
                    "\"ipv4.address\" when using an unmanaged parent bridge\n"),
        rule(["config", "device", "override", "box1", "eth0", "ipv4.address=10.0.0.8"], exit_=1,
             stderr="Error: The device already exists\n"),
        rule(["config", "device", "set", "box1", "eth0", "ipv4.address", "10.0.0.8"]),
        rule(["exec", "box1", "--", "ip", "-4", "-o", "addr", "show", "dev", "eth0"],
             "2: eth0    inet 10.0.0.9/22 brd 10.0.3.255 scope global eth0", ifExists=pinned),
        rule(["exec", "box1", "--", "ip", "-4", "-o", "addr", "show", "dev", "eth0"],
             "2: eth0    inet 10.0.0.5/22 brd 10.0.3.255 scope global dynamic eth0"),
        rule(["exec", "box1", "--", "ip", "-4", "route", "show", "default"],
             "default via 10.0.0.1 dev eth0 proto dhcp src 10.0.0.5 metric 100"),
        rule(["exec", "box1", "--", "cat", "/etc/netplan/99-opute-static.yaml"], exit_=1,
             stderr="cat: /etc/netplan/99-opute-static.yaml: No such file or directory\n"),
        rule(["exec", "box1", "--", "sh", "-c", "..."], creates=[pinned]),
    ]


def reset_rules(delete_fails=False, box_present=True):
    """The reset estate plus the reinstall's incus calls (virt-install)."""
    pool, bridge, gone = "markers/pool", "markers/bridge", "markers/box1-deleted"
    estate = [BOX1, ALIEN1, instance("boxer2", "Running", "container")]
    rules = [
        rule(["list", "--format", "json"], [i for i in estate if i["name"] != "box1"], ifExists=gone),
        rule(["list", "--format", "json"], estate if box_present else estate[1:]),
        rule(["config", "get", "box1", LABEL], OWNER),
        rule(["config", "get", "boxer2", LABEL], OWNER),
        (rule(["delete", "box1", "--force"], exit_=1, stderr="Error: Instance is busy\n") if delete_fails
         else rule(["delete", "box1", "--force"], creates=[gone])),
        rule(["delete", "boxer2", "--force"]),
        rule(["version"], "7.3"),
        rule(["storage", "list", "--format", "json"], [{"name": "default", "driver": "dir"}], ifExists=pool),
        rule(["storage", "list", "--format", "json"], "[]"),
        rule(["storage", "create", "default", "dir"], creates=[pool]),
        rule(["network", "list", "--format", "json"], [{"name": "lxdbr0"}, {"name": "incusbr0"}], ifExists=bridge),
        rule(["network", "list", "--format", "json"], [{"name": "lxdbr0"}]),
        rule(["network", "create", "..."], creates=[bridge]),
        rule(["profile", "device", "show", "default"], "root:\n  path: /\n  pool: default\n  type: disk\n"),
        rule(["profile", "device", "add", "..."]),
    ]
    virt = json.loads((HERE / "fixtures" / "shims" / "virt-install.json").read_text())
    commands = {k: v for k, v in virt["commands"].items() if k != "incus"}
    commands["incus"] = rules
    return commands


FIXTURES = {
    "incus-domain-estate": {
        "description": "M8b: an owned running VM (vm1), an owned stopped container (box1) and a VM whose "
                       "ownership label drifted after listing (drift1), on a zfs pool that enforces root "
                       "disk quotas. Deleting vm1 drops it from later listings.",
        "commands": {"incus": estate_rules(POOL_ZFS)}, "default": DEFAULT},
    "incus-domain-dir-pool": {
        "description": "M8b: the same estate on a dir pool whose source has no mount, so no root disk "
                       "quota can be enforced.",
        "commands": {"incus": estate_rules(POOL_DIR)}, "default": DEFAULT},
    "incus-domain-unmanaged-bridge": {
        "description": "M8b: box1 on a bridge another host owns, so its address is pinned inside the "
                       "guest; a second address takes the existing-override path.",
        "commands": {"incus": unmanaged_rules()}, "default": DEFAULT},
    "incus-domain-reset": {
        "description": "M8b: the stack-reset estate (owned box1 and boxer2, foreign alien1) and the "
                       "reinstall's recorded package and incus shims.",
        "commands": reset_rules(), "default": DEFAULT},
    "incus-domain-reset-busy": {
        "description": "M8b: the stack-reset estate where deleting box1 fails.",
        "commands": reset_rules(delete_fails=True), "default": DEFAULT},
    "incus-domain-reset-resumed": {
        "description": "M8b: the stack-reset estate after box1 was already deleted by an earlier run.",
        "commands": reset_rules(box_present=False), "default": DEFAULT},
}

# --- scenarios ----------------------------------------------------------------------------

LIFECYCLE = ["internal/domain/incus/vm_lifecycle.go", "internal/domain/incus/incus_launch.go",
             "internal/domain/incus/incus_root_disk.go", "internal/domain/incus/incus_storage_quota.go",
             "internal/domain/incus/incus_ownership.go", "internal/tools/dispatch_unassigned.go"]
CONTAINER = ["internal/domain/incus/incus_container.go", "internal/domain/incus/incus_ownership.go",
             "internal/tools/dispatch_incus.go"]
STACK = ["internal/domain/incus/incus_stack.go", "internal/domain/host/tool.go:InstallIncusStack",
         "internal/tools/dispatch_incus.go"]
VM1_URI, BOX1_URI = "vm:local:vm1", "container:local:box1"


def owned(**env):
    return serve(**OWNED, **env)


cases = [
    # Provisioning an existing instance is a status/start, never a create or
    # resize: the observed kind wins when none is asked for, and a mismatch or
    # a drifted owner refuses before anything changes.
    scenario("provision.existing", [
        owned(),
        # Larger than vm1 (2 CPUs, 2GiB), so a resize would show, yet small
        # enough that the heavy reservation always fits: effective memory is
        # the host's own total less the agent cgroup's memory.current, page
        # cache included, and 16GiB was refused on either side when that
        # cache passed about 14GiB (M8b round 2, M8c round 3).
        task_call("existingVm", "provision_vm", {"vmName": "vm1", "cpus": 4, "memory": "4GiB"}),
        task_call("createExisting", "create_vm", {"name": " vm1 ", "image": "ubuntu:24.04"}),
        task_call("kindMismatch", "provision_vm", {"vmName": "vm1", "instanceType": "container"}),
        task_call("kindInvalid", "provision_vm", {"vmName": "vm1", "instanceType": " LXC "}),
        task_call("kindVmAlias", "provision_vm", {"vmName": "vm1", "instanceType": "Virtual_Machine"}),
        task_call("noName", "provision_vm", {"image": "ubuntu:24.04"}),
        task_call("drift", "provision_vm", {"vmName": "drift1"}),
        task_call("reuseContainer", "provision_vm", {"vmName": "box1", "image": "local:noble"}),
        REGISTRY,
        STOP], LIFECYCLE + CONTAINER, "incus-domain-estate"),
    # A new instance is admitted against the root-disk pool, then against
    # host admission, which refuses on this host (enforcement unknown).
    scenario("provision.new-refused", [
        owned(),
        task_call("newVm", "create_vm", {"vmName": "new1", "cpus": 2, "memory": "1GiB"}),
        task_call("newContainer", "provision_vm", {"vmName": "new1", "disk": "30GiB"}),
        task_call("newSystemContainer", "provision_container", {"containerName": "new1", "gpu": True}),
        REGISTRY,
        STOP], LIFECYCLE + CONTAINER, "incus-domain-estate"),
    scenario("provision.dir-pool", [
        owned(),
        task_call("explicitDisk", "create_vm", {"vmName": "new1", "disk": "5GiB"}),
        task_call("defaultDisk", "create_vm", {"vmName": "new1"}),
        task_call("containerDisk", "provision_container", {"containerName": "new1", "disk": "8GiB"}),
        call("grow", "update_vm_resources", {"uri": VM1_URI, "disk": "20GiB"}),
        STOP], LIFECYCLE, "incus-domain-dir-pool"),
    # Without an instance id nothing is labelled or ownership-checked.
    scenario("provision.unowned", [
        serve(),
        task_call("existingVm", "provision_vm", {"vmName": "vm1"}),
        task_call("newVm", "create_vm", {"vmName": "new1"}),
        REGISTRY,
        STOP], LIFECYCLE, "incus-domain-estate"),
    # A reused system container: pin, start, autostart, runtime devices, GPU
    # devices (restart while running), the HTTP proxy, the model volume and
    # an address before it is reported.
    scenario("container.reuse", [
        owned(),
        task_call("reuse", "provision_container", {
            "containerName": "box1", "image": "ubuntu:24.04", "cpus": 4, "memory": "4GiB",
            "gpu": True, "wslGpuLibs": True, "port": 8080, "modelVolume": " models-vol ",
            "ipv4Address": " 10.0.0.9 "}),
        task_call("noNesting", "provision_container", {"containerName": "box1", "nesting": False}),
        task_call("onVm", "provision_container", {"containerName": "vm1"}),
        task_call("whitespace", "provision_container", {"containerName": "box 1"}),
        task_call("noName", "provision_container", {"containerName": "  "}),
        task_call("badAddress", "provision_container", {"containerName": "box1", "ipv4Address": "10.0.0"}),
        task_call("ipv6Address", "provision_container", {"containerName": "box1", "ipv4Address": "fd42::9"}),
        task_call("drift", "provision_container", {"containerName": "drift1"}),
        REGISTRY,
        STOP], CONTAINER, "incus-domain-estate"),
    scenario("container.unmanaged-bridge", [
        owned(),
        task_call("guestPin", "provision_container", {"containerName": "box1", "ipv4Address": "10.0.0.9"}),
        task_call("overrideExists", "provision_container", {"containerName": "box1", "ipv4Address": "10.0.0.8"}),
        STOP], CONTAINER, "incus-domain-unmanaged-bridge"),
    # Lifecycle on the canonical binding: success, a refused start (the VM
    # is already running), restart's partial failure, the resource update's
    # grow-only root disk, then delete and the binding that no longer
    # resolves.
    scenario("lifecycle.vm", [
        owned(),
        task_call("startRunning", "start_vm", {"uri": VM1_URI}),
        task_call("startBox", "start_vm", {"uri": BOX1_URI}),
        task_call("stop", "stop_vm", {"uri": VM1_URI}),
        task_call("restartFails", "restart_vm", {"uri": VM1_URI}),
        task_call("restartBox", "restart_vm", {"uri": BOX1_URI}),
        call("updateNothing", "update_vm_resources", {"uri": VM1_URI}),
        call("update", "update_vm_resources", {"uri": VM1_URI, "cpus": 4, "memory": " 4GiB ", "disk": "20GiB"}),
        call("updateSame", "update_vm_resources", {"uri": VM1_URI, "disk": "10GiB"}),
        call("updateShrink", "update_vm_resources", {"uri": VM1_URI, "disk": "5GiB"}),
        call("updateBadSize", "update_vm_resources", {"uri": VM1_URI, "disk": "lots"}),
        call("updateFractionCpu", "update_vm_resources", {"uri": VM1_URI, "cpus": 2.9}),
        call("updateContainer", "update_vm_resources", {"uri": BOX1_URI, "disk": "30GiB"}),
        call("drift", "update_vm_resources", {"uri": "vm:local:drift1", "cpus": 1}),
        task_call("stopDrift", "stop_vm", {"uri": "vm:local:drift1"}),
        task_call("delete", "delete_vm", {"uri": VM1_URI}),
        task_call("startDeleted", "start_vm", {"uri": VM1_URI}),
        REGISTRY,
        STOP], LIFECYCLE, "incus-domain-estate"),
    # The disposable GPU probe: on a host without WSL GPU prerequisites it
    # reports the host tier and launches nothing.
    scenario("container.gpu-probe", [
        owned(),
        call("probe", "probe_gpu_container", {}),
        STOP], CONTAINER + ["internal/domain/host/gpu.go"], "incus-domain-estate"),
]

def live_omitempty(label):
    """host-read's omitempty rules for the same live metrics: a PSI sample
    of zero is omitted by Go (internal/heartbeat/capacity.go), so absent and
    0 are one value. Without them, the host's own I/O pressure decides
    whether a key is present (M8e round 1, Go/Go 1 in 20)."""
    source = next(sc for sc in json.loads((HERE / "scenarios" / "host-read.json").read_text())
                  if sc["id"] == "host-read.host-info.inventory-mix")
    return [{**rule, "path": ["steps", label] + rule["path"][2:]} for rule in source["compare"]["omitempty"]
            if rule["path"][:2] == ["steps", "hostInfo"]]


def live_masks(label):
    """The host-read get_host_info masks for live system metrics, at this
    scenario's step: the same metadata, sampled at call time."""
    source = next(sc for sc in json.loads((HERE / "scenarios" / "host-read.json").read_text())
                  if sc["id"] == "host-read.host-info.inventory-mix")
    out = []
    for mask in source["compare"]["masks"]:
        path = mask["path"]
        if path[:2] == ["steps", "hostInfo"] and "system" in path:
            out.append({**mask, "path": ["steps", label] + path[2:], "optional": True})
    for root in (["body", "result", "structuredContent"], ["body", "result", "content", 0, "text", "$json"]):
        out.append({"path": ["steps", label] + root + ["checkedAt"], "type": "rfc3339", "reason": TS})
    return out


STANDALONE = ["internal/domain/incus/standalone.go", "internal/tools/dispatch_unassigned.go"]


def standalone(id_, steps, fixture, masks=()):
    """structuredResult(out, ""): the text is the result's own JSON."""
    case = scenario(id_, steps, STANDALONE, fixture, masks)
    case["compare"]["parseJson"] = [["steps", label, "body", "result", "content", "*", "text"]
                                    for label in ("prerequisites", "status")]
    return case


standalone_cases = [
    # The standalone reads: prerequisites (commands, provider readiness, the
    # root-disk quota of the default pool) and the fast local status.
    standalone("standalone.status", [
        owned(),
        call("prerequisites", "check_local_prerequisites", {}),
        call("status", "get_local_status", {}),
        REGISTRY,
        STOP], "incus-domain-estate", masks=live_masks("status")),
    standalone("standalone.dir-pool", [
        owned(),
        call("prerequisites", "check_local_prerequisites", {}),
        STOP], "incus-domain-dir-pool"),
    # The missing-provider error names the stack install tool, which D19
    # publishes as install_virtualization_stack.
    standalone("standalone.no-provider", [
        serve(),
        call("prerequisites", "check_local_prerequisites", {}),
        call("status", "get_local_status", {}),
        STOP], None),
]
standalone_cases[-1]["compare"]["divergences"] = ["D19.neutral-host-tool-names"]
standalone_cases[0]["compare"]["omitempty"] = live_omitempty("status")
cases += standalone_cases

RESET_OK = {"confirm": True, "reinstall": True, "disposableHostFingerprint": "fp-1",
            "expectedHostFingerprint": " fp-1 ", "disposableHostAuthorization": "dispose:fp-1"}
# A checkpoint read back from disk carries its write time; the in-memory one a
# fresh reset returns was never stamped (Go stamps only the copy it writes).
CHECKPOINT_MASKS = [{"path": ["steps", "*", "content", "updatedAt"], "type": "rfc3339", "reason": TS,
                     "optional": True},
                    {"path": ["steps", "*", "body", "result", "result", "structuredContent", "checkpoint",
                              "updatedAt"], "type": "rfc3339", "optional": True, "match": r".+",
                     "reason": TS + "; an unstamped (empty) one still compares literally"}]


def reset(as_, **args):
    return task_call(as_, "reset_incus_stack", {**RESET_OK, **args})


def reset_serve(**env):
    return owned(PATH="${SANDBOX}/shims", TMPDIR="${SANDBOX}/home/tmp",
                 OPUTE_SHARED_HOST_OWNER_INSTANCE=OWNER, **env)


TMP = {"as": "tmp", "mkdir": {"path": "${SANDBOX}/home/tmp"}}
TMP_MASK = [{"path": ["trace", "*", "argv", "*"], "type": "string", "optional": True,
             "match": r"(\$\{SANDBOX\}|/tmp/parity-[^/]+)/home/tmp/opute-zabbly-(key|source)-[0-9]+",
             "reason": "os.CreateTemp names its key and source files randomly; argvFiles compares their bytes"}]


def checkpoint_doc(phase, deleted, targets=("box1",), owner=OWNER, version="incus-reset.v1"):
    return json.dumps({"version": version, "instanceId": owner,
                       "targets": [{"name": t, "status": "Stopped", "type": "container", "owner": OWNER}
                                   for t in targets],
                       "deleted": list(deleted), "phase": phase, "updatedAt": "2026-10-01T00:00:00Z"}, indent=2)


def write_checkpoint(as_, content):
    return {"as": as_, "write": {"path": CHECKPOINT, "content": content, "mode": "0600"}}


cases += [
    # Every guard refuses before the inventory is read.
    scenario("reset.refusals", [
        TMP, reset_serve(),
        reset("unconfirmed", confirm=False),
        reset("noReinstall", reinstall=False),
        reset("fingerprintMismatch", expectedHostFingerprint="fp-2"),
        reset("noFingerprint", disposableHostFingerprint=" ", expectedHostFingerprint=""),
        reset("wrongAuthorization", disposableHostAuthorization="dispose:fp-2"),
        reset("noSelection"),
        reset("emptyInventory", instancePrefix="zzz"),
        reset("foreign", instanceNames=["alien1"]),
        reset("mixed", instanceNames=["box1", " alien1 "]),
        read_checkpoint("checkpoint"),
        STOP], STACK, "incus-domain-reset", masks=TMP_MASK + CHECKPOINT_MASKS),
    scenario("reset.ownership-required", [
        TMP, serve(PATH="${SANDBOX}/shims", OPUTE_SHARED_HOST_OWNER_INSTANCE=OWNER),
        reset("notEnforced", instanceNames=["box1"]),
        STOP], STACK, "incus-domain-reset"),
    scenario("reset.shared-owner-required", [
        TMP, owned(PATH="${SANDBOX}/shims"),
        reset("noSharedOwner", instanceNames=["box1"]),
        STOP], STACK, "incus-domain-reset"),
    scenario("reset.shared-owner-other", [
        TMP, owned(PATH="${SANDBOX}/shims", OPUTE_SHARED_HOST_OWNER_INSTANCE="parity-other-instance"),
        reset("otherOwner", instanceNames=["box1"]),
        STOP], STACK, "incus-domain-reset"),
    # Dry run, then the full delete-reinstall-verify, then a rerun that finds
    # the reconciled checkpoint and nothing left to select.
    scenario("reset.lifecycle", [
        TMP, reset_serve(),
        reset("dryRun", instancePrefix="box", dryRun=True),
        read_checkpoint("afterDryRun"),
        reset("full", instanceNames=["box1", "boxer2", "ghost"]),
        read_checkpoint("afterFull"),
        reset("again", instanceNames=["box1"]),
        STOP], STACK, "incus-domain-reset", masks=TMP_MASK + CHECKPOINT_MASKS),
    # A failed delete interrupts; the rerun resumes from the checkpoint.
    scenario("reset.interrupted", [
        TMP, reset_serve(),
        reset("busy", instanceNames=["box1"]),
        read_checkpoint("afterBusy"),
        reset("busyAgain", instancePrefix="box1"),
        read_checkpoint("afterRetry"),
        STOP], STACK, "incus-domain-reset-busy", masks=TMP_MASK + CHECKPOINT_MASKS),
    # Every selected instance is already gone: the checkpoint's targets are
    # resumed, never a broader selection.
    scenario("reset.resumed", [
        TMP, write_checkpoint("seed", checkpoint_doc("interrupted", ["box1"])),
        reset_serve(),
        reset("resume", instanceNames=["box1"]),
        read_checkpoint("afterResume"),
        STOP], STACK, "incus-domain-reset-resumed", masks=TMP_MASK + CHECKPOINT_MASKS),
    scenario("reset.checkpoint-invalid", [
        TMP, write_checkpoint("seed", "{not json"),
        reset_serve(),
        reset("invalid", instanceNames=["box1"]),
        write_checkpoint("foreign", checkpoint_doc("interrupted", [], owner="someone-else")),
        reset("foreignCheckpoint", instanceNames=["box1"]),
        write_checkpoint("oldVersion", checkpoint_doc("interrupted", [], version="incus-reset.v0")),
        reset("oldVersion", instanceNames=["box1"]),
        write_checkpoint("typed", '{"version": "incus-reset.v1", "instanceId": "parity-owner", "deleted": [1]}'),
        reset("typedInvalid", instanceNames=["box1"]),
        STOP], STACK, "incus-domain-reset"),
]

# D21 (neutral-instance-tool-names): Rust lists reset_virtualization_stack and
# provision_container's hostGpuLibs, and keeps the Go names as hidden aliases
# with Go's input schema. tools/list is compared through the declared
# divergence; the Go-name call and the Go field must match Go exactly, an
# unmapped near-name stays an unknown tool, and the operation and audit rows
# name the called alias.
D21 = ["openspec/changes/neutral-instance-tool-names", "crates/host-agent/catalog/neutral-names.json",
       "internal/tools/dispatch_incus.go"]
ROWS = {"as": "rows", "query": {"db": "${SANDBOX}/state/state.db", "sql": {
    "operations": "SELECT tool_name, status FROM operations ORDER BY tool_name",
    "invocations": "SELECT operation_id, catalog_revision, terminal_status FROM capability_invocations "
                   "ORDER BY operation_id"}}}
d21 = scenario("d21.aliases", [
    owned(OPUTE_SHARED_HOST_OWNER_INSTANCE=OWNER),
    {"as": "list", "mcp": {"method": "tools/list"}},
    reset("alias", instancePrefix="box", dryRun=True),
    task_call("goField", "provision_container", {"containerName": "box1", "gpu": True, "wslGpuLibs": True}),
    call("unknown", "reset_instance_stack", {}),
    ROWS,
    STOP], D21 + STACK + CONTAINER, "incus-domain-estate")
d21["compare"]["divergences"] = ["D19.neutral-host-tool-names"]
cases += [d21]

if __name__ == "__main__":
    out = Path(sys.argv[1]) if len(sys.argv) > 1 else HERE / "scenarios" / "incus-domain.json"
    for name, fixture in FIXTURES.items():
        (HERE / "fixtures" / "shims" / f"{name}.json").write_text(json.dumps(fixture, indent=1) + "\n")
    out.write_text(json.dumps(cases, indent=1) + "\n")
    print(f"wrote {len(cases)} scenarios to {out} and {len(FIXTURES)} fixtures")
