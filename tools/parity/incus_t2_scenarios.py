#!/usr/bin/env python3
"""Generate scenarios-t2/incus-domain.json: the M8b incus-domain T2 matrix.

Run from tools/parity: python3 incus_t2_scenarios.py

Every scenario boots its twins from the nested-Incus golden
(`python3 -m parity t2 golden-incus`): Incus from the zabbly archive, a btrfs
default pool, incusbr0, the default profile, two local images (parity-noble,
a container, and parity-noble-vm) and the enforced opute-workload.slice, so
host admission sees an enforced cgroup and launches are admitted. Nothing
outside the twin is touched: no shared Incus, Kubernetes, tunnel or
deployment state.

Each scenario creates, mutates, reads and deletes real instances in both
twins; the snapshots compare the Incus objects (instances, profiles,
networks, pools, volumes, images, without volatile.* keys and timestamps)
along with files, units and firewall. A teardown snapshot against the first
baseline records residue; Go's residue is listed in "residue" and must be
reproduced. The M8a gaps exec_command and run_instance_command execute
inside a real container here.
"""
import json
import sys

import t2_scenarios as hd

INSTANCE = "parity-t2-incus"
OWNED = {"OPUTE_HOST_AGENT_INSTANCE": INSTANCE, "OPUTE_INCUS_OWNERSHIP_MODE": "enforce"}
BOX, VM = "container:local:box1", "vm:local:vm1"
SNAP, STOP, call, task_call, done = hd.snap, hd.STOP, hd.call, hd.task_call, hd.done
D19 = hd.D19
COMMANDS = ["incus"]

LIFECYCLE = ["internal/domain/incus/vm_lifecycle.go", "internal/domain/incus/incus_launch.go",
             "internal/domain/incus/incus_ownership.go", "internal/domain/incus/incus_inventory.go",
             "internal/domain/incus/incus_root_disk.go", "internal/domain/incus/incus_storage_quota.go"]
CONTAINER = ["internal/domain/incus/incus_container.go", "internal/domain/incus/incus_launch.go",
             "internal/domain/incus/incus_ownership.go", "internal/domain/incus/incus_root_disk.go"]
EXEC = ["internal/domain/host/exec_command.go", "internal/tools/dispatch_host.go"]
STACK = ["internal/domain/incus/incus_stack.go", "internal/domain/host/incus_stack.go"]


def serve(**env):
    return {"as": "serve", "serve": {"env": {**OWNED, **env}}}


def scenario(id_, steps, anchors, residue, masks=(), parse_json=(), sets=(), rejected_only=False,
             divergences=()):
    out = hd.scenario(id_, steps, anchors, residue, masks=masks, sets=sets, parse_json=parse_json,
                      rejected_only=rejected_only, divergences=divergences)
    out["id"] = "t2.incus-domain." + id_
    out["owner"] = "incus"
    out["golden"] = "incus"
    out["commands"] = COMMANDS
    return out


# incusbr0 leases each instance an address from its DHCP range; which one
# is the dnsmasq's choice (the MAC is random), never the operation's.
LEASE = r"10\.0\.100\.([2-9]|[1-9][0-9]|1[0-9][0-9]|2[0-4][0-9]|25[0-4])"
READS = ("createdInfo", "createdList", "mutatedInfo", "mutatedList", "runningInfo", "runningList")
LEASE_MASKS = [{"path": ["steps", "*", "body", "result"] + where + rest, "type": "string", "optional": True,
                "match": LEASE, "reason": "DHCP lease on incusbr0, chosen by dnsmasq"}
               for where in (["structuredContent"], ["content", 0, "text", "$json"])
               for rest in (["ipv4", "*"], ["vms", "*", "ipv4", "*"])]
READ_JSON = [["steps", k, "body", "result", "content", 0, "text"] for k in READS]


def vm_ready(as_):
    """Harness wait (root, read-only): the VM's agent answers and it holds a lease.

    A VM boots its agent and takes a DHCP lease on its own schedule, after
    the operation returned; reading it earlier would race the guest's boot.
    """
    return hd.shell(as_, ["sh", "-c", "for i in $(seq 1 150); do incus exec vm1 -- true 2>/dev/null && "
                                      "incus list vm1 -c 4 -f csv | grep -q '10\\.0\\.100\\.' && exit 0; "
                                      "sleep 2; done; exit 1"])


def status_masks(*steps):
    """get_local_status: live host load and memory figures, and its read time."""
    out = []
    for step in steps:
        for where in (["structuredContent"], ["content", 0, "text", "$json"]):
            base = ["steps", step, "body", "result"] + where
            out += [{"path": base + ["system"], "type": "object",
                     "reason": "live host load and memory figures; their shape is compared in T1"},
                    {"path": base + ["checkedAt"], "type": "rfc3339", "reason": hd.TS}]
    return out


def inventory(prefix, uri):
    return [call(prefix + "List", "list_vms", {"fast": True}),
            call(prefix + "Info", "get_vm_info", {"uri": uri, "fast": True})]


SH = "sh"
# The incus CLI's per-user client configuration, created on its first run.
INCUS_CLIENT = "/home/ubuntu/.config/incus"
cases = [
    # A system container from the local image: launch, guest commands
    # (closing the M8a gaps), lifecycle, resources, delete.
    # run_instance_command takes the canonical URI and transient stdin.
    scenario("container.lifecycle", [
        SNAP("baseline"), serve(),
        task_call("provision", "provision_container",
                  {"containerName": "box1", "image": "local:parity-noble", "disk": "4GiB", "nesting": False}),
        SNAP("provisioned", "baseline"),
        inventory("created", BOX),
        # exec_command is catalog-excluded, so Go builds it no resource
        # binding and it refuses even a live, owned container by name or URI.
        call("exec", "exec_command", {"name": "box1", "command": SH, "args": ["-c", "touch /root/exec-ran"]}),
        call("execUri", "exec_command", {"uri": BOX, "command": SH, "args": ["-c", "touch /root/exec-ran"]}),
        call("execRan", "run_instance_command", {"uri": BOX, "command": "test", "args": ["-e", "/root/exec-ran"]}),
        call("run", "run_instance_command", {"uri": BOX, "command": SH,
                                             "args": ["-c", "echo parity-run; uname -s"]}),
        call("runStdin", "run_instance_command", {
            "uri": BOX, "command": SH, "stdin": "parity-stdin-secret\n",
            "args": ["-c", "cat > /root/parity-stdin && sha256sum /root/parity-stdin && stat -c %a /root/parity-stdin"]}),
        call("runFails", "run_instance_command", {"uri": BOX, "command": SH, "args": ["-c", "exit 7"]}),
        call("runTimeout", "run_instance_command", {"uri": BOX, "command": "sleep", "args": ["30"],
                                                    "timeoutMs": 1500}),
        call("runVmUri", "run_instance_command", {"uri": VM, "command": "true"}),
        task_call("stop", "stop_vm", {"uri": BOX}),
        SNAP("stopped", "baseline"),
        task_call("start", "start_vm", {"uri": BOX}),
        task_call("restart", "restart_vm", {"uri": BOX}),
        call("resize", "update_vm_resources", {"uri": BOX, "cpus": 1, "memory": "768MiB"}),
        call("grow", "update_vm_resources", {"uri": BOX, "disk": "5GiB"}),
        call("shrink", "update_vm_resources", {"uri": BOX, "disk": "3GiB"}),
        task_call("reprovision", "provision_container", {"containerName": "box1"}),
        SNAP("mutated", "baseline"),
        inventory("mutated", BOX),
        task_call("delete", "delete_vm", {"uri": BOX}),
        task_call("deleteAgain", "delete_vm", {"uri": BOX}),
        SNAP("deleted", "baseline"),
        STOP, SNAP("teardown", "baseline"),
    ], CONTAINER + LIFECYCLE + EXEC,
        "The container and its volume are deleted. Incus keeps the image volume it unpacked into the default "
        "pool on the first launch from local:parity-noble (its image cache, used by the image); the incus CLI's "
        "client configuration and the coordinator ledger stay. The agent's own state lives under its excluded "
        "root.",
        masks=LEASE_MASKS, parse_json=READ_JSON[:4]),

    # A VM from the local VM image at the smallest size the image boots in.
    scenario("vm.lifecycle", [
        SNAP("baseline"), serve(),
        task_call("create", "create_vm", {"vmName": "vm1", "image": "local:parity-noble-vm", "cpus": 1,
                                          "memory": "1GiB", "disk": "10GiB"}, timeout=900),
        vm_ready("createdReady"),
        SNAP("created", "baseline"),
        inventory("created", VM),
        task_call("createAgain", "create_vm", {"vmName": "vm1", "image": "local:parity-noble-vm"}),
        task_call("kindMismatch", "provision_vm", {"vmName": "vm1", "instanceType": "container"}),
        call("run", "run_instance_command", {"uri": VM, "command": SH, "args": ["-c", "echo parity-vm; nproc"]},),
        task_call("stop", "stop_vm", {"uri": VM}, timeout=900),
        call("resize", "update_vm_resources", {"uri": VM, "cpus": 2, "memory": "1536MiB", "disk": "12GiB"}),
        SNAP("resized", "baseline"),
        task_call("start", "start_vm", {"uri": VM}, timeout=900),
        task_call("restart", "restart_vm", {"uri": VM}, timeout=900),
        vm_ready("restartedReady"),
        inventory("running", VM),
        task_call("delete", "delete_vm", {"uri": VM}, timeout=900),
        SNAP("deleted", "baseline"),
        STOP, SNAP("teardown", "baseline"),
    ], LIFECYCLE,
        "The VM and its volume are deleted. Incus keeps the image volume it unpacked from local:parity-noble-vm "
        "(its image cache); the incus CLI's client configuration and the coordinator ledger stay.",
        masks=LEASE_MASKS, parse_json=READ_JSON[:2] + READ_JSON[4:]),

    # The standalone reads and the GPU probe against a live Incus with no GPU.
    scenario("standalone.reads", [
        serve(), SNAP("baseline"),
        call("prerequisites", "check_local_prerequisites", {}),
        call("status", "get_local_status", {}),
        call("probe", "probe_gpu_container", {}),
        {"as": "after", "snapshot": {"baseline": "baseline", "expectEmpty": {"except": [hd.COORDINATOR, INCUS_CLIENT]}}},
        STOP,
    ], ["internal/domain/incus/standalone.go", "internal/domain/incus/incus_container.go"],
        "None: reads, and a GPU probe that finds no GPU device to attach. The incus CLI writes its own client "
        "configuration the first time it runs as the agent's user; that is the CLI's, not an operation's.",
        parse_json=[["steps", k, "body", "result", "content", 0, "text"] for k in ("prerequisites", "status")],
        masks=status_masks("status") + LEASE_MASKS),

    # reset_virtualization_stack (Go reset_incus_stack) as the shared-host
    # owner: deletes the owned container, purges and reinstalls Incus from
    # the zabbly archive, re-initialises pool, bridge and profile, verifies.
    scenario("reset.owned-container", [
        SNAP("baseline"),
        serve(OPUTE_SHARED_HOST_OWNER_INSTANCE=INSTANCE),
        task_call("provision", "provision_container",
                  {"containerName": "box1", "image": "local:parity-noble", "disk": "4GiB", "nesting": False}),
        task_call("reset", "reset_incus_stack", {
            "instanceNames": ["box1"], "confirm": True, "reinstall": True, "disposableHostFingerprint": "parity-t2-fp",
            "expectedHostFingerprint": "parity-t2-fp", "disposableHostAuthorization": "dispose:parity-t2-fp"},
            timeout=1800),
        SNAP("reset", "baseline"),
        call("status", "get_local_status", {}),
        STOP, SNAP("teardown", "baseline"),
    ], STACK, "Go's reset purges and reinstalls Incus: the local images and anything else Incus held are gone, "
              "the checkpoint file stays, and the reinstalled stack's pool, bridge and profile are what Go's "
              "reconcile creates. Reproduced, not cleaned up.",
        parse_json=[["steps", "status", "body", "result", "content", 0, "text"]],
        masks=status_masks("status") + LEASE_MASKS),
]

# M8 validation 4 for the incus domain: every incus mutation M4 rejects,
# re-run against a live owned container with arguments that would change
# it if admitted. Each phase's snapshot must be empty (expectEmpty).
REJECTED = [
    ("provision_container", {"containerName": "rejected1", "image": "local:parity-noble"}),
    ("probe_gpu_container", {}),
    ("reset_incus_stack", {"confirm": True, "reinstall": True, "disposableHostFingerprint": "parity-t2-fp",
                           "expectedHostFingerprint": "parity-t2-fp",
                           "disposableHostAuthorization": "dispose:parity-t2-fp"}),
    ("create_vm", {"vmName": "rejected2", "image": "local:parity-noble-vm"}),
    ("provision_vm", {"vmName": "rejected3", "image": "local:parity-noble", "instanceType": "container"}),
    ("start_vm", {"uri": BOX}),
    ("stop_vm", {"uri": BOX}),
    ("restart_vm", {"uri": BOX}),
    ("update_vm_resources", {"uri": BOX, "cpus": 1}),
    ("delete_vm", {"uri": BOX}),
]
INCUS_TOOLS = {t["tool"] for t in json.load(open("../../baseline/inventory/incus-domain.json"))["tools"]
               if t["effect"] != "EffectRead"}
assert {n for n, _ in REJECTED} <= hd.M4_GATED, sorted({n for n, _ in REJECTED} - hd.M4_GATED)
assert hd.M4_GATED & INCUS_TOOLS == {n for n, _ in REJECTED}, sorted((hd.M4_GATED & INCUS_TOOLS) ^ {n for n, _ in REJECTED})


def rejected(prefix, meta=None):
    out = []
    for name, args in REJECTED:
        step = call(prefix + "." + name, name, args)
        if meta:
            step["mcp"]["meta"] = meta
        out.append(step)
    return out


TASK_AWARE = {t["tool"] for t in json.load(open("../../baseline/inventory/incus-domain.json"))["tools"]
              if t["taskMode"] == "TaskAware"}


def wrong_binding():
    """M4's binding rejections; a task-aware tool's refusal is its task's terminal result."""
    defaults = dict(REJECTED)
    out = []
    for i, (name, args) in enumerate(b for b in hd.M4_BINDING if b[0] in INCUS_TOOLS):
        merged = {k: v for k, v in defaults[name].items() if k != "uri"}
        merged.update(args)
        label = f"wrongBinding.{name}.{i}"
        out.append(task_call(label, name, merged) if name in TASK_AWARE else call(label, name, merged))
    return hd.flat(out)


def empty(as_):
    return {"as": as_, "snapshot": {"baseline": "baseline", "expectEmpty": True}}


cases += [
    scenario("admission.rejected-no-effect", [
        serve(),
        task_call("fixture", "provision_container",
                  {"containerName": "box1", "image": "local:parity-noble", "disk": "4GiB", "nesting": False}),
        {"as": "stopFixture", "stop": {}},
        {"as": "serveClosed", "serve": {"env": OWNED, "unset": ["OPUTE_STANDALONE_ALLOW_MUTATIONS"]}},
        SNAP("baseline"),
        rejected("gateClosed"),
        empty("afterGateClosed"),
        {"as": "stopClosed", "stop": {}},
        {"as": "serveOpen", "serve": {"env": OWNED}},
        rejected("staleRevision", meta={"catalogRevision": "sha256:0000"}),
        wrong_binding(),
        empty("afterOpen"),
        STOP, empty("teardown"),
    ], hd.ADMISSION + ["internal/domain/incus"],
        "The fixture container box1 stays (it is created before the baseline); every rejected call is refused "
        "before it reaches Incus.",
        rejected_only=True, divergences=[D19]),
]

if __name__ == "__main__":
    out = sys.argv[1] if len(sys.argv) > 1 else "scenarios-t2/incus-domain.json"
    with open(out, "w") as f:
        json.dump(cases, f, indent=1)
        f.write("\n")
    print(f"wrote {len(cases)} T2 scenarios to {out}")
