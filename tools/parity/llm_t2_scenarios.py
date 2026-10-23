#!/usr/bin/env python3
"""Generate scenarios-t2/llm-domain.json and scenarios-t2/serving-domain.json:
the M8f T2 matrices.

Run from tools/parity: python3 llm_t2_scenarios.py

The twins boot the base golden. The model runtimes are the CPU-only stub
t2guest/llmstub.py (milestones.md, 8f), copied before the baseline to the
paths the agent is told to use: an `ollama` whose `serve` is an Ollama API
over a store of JSON files, a `llama-server` that serves /v1/models, and an
`nvidia-smi` that reports one device. Everything else is real: ubuntu's
systemd user manager runs the units the agent writes and starts, the relay
listens on the twin's loopback, and the serving supervisor is a real
transient user unit running a real HTTP server.

Nothing outside the twin is touched, and no real model or GPU is used.
"""
import hashlib
import json
from pathlib import Path

import t2_scenarios as hd

SNAP, STOP, call, task_call, shell = hd.snap, hd.STOP, hd.call, hd.task_call, hd.shell
HERE = Path(__file__).resolve().parent
STUB = (HERE / "t2guest" / "llmstub.py").read_bytes()
STUB_SHA = hashlib.sha256(STUB).hexdigest()
HOME = "/home/ubuntu"
RELAY_DIR = HOME + "/.opute/relays"
MODEL = "parity/tiny:1b"
LLM = ["internal/tools/dispatch_llm.go", "internal/tools/dispatch.go:resolveLocalLLMModelArg"]


def install_stub(as_, path, user="ubuntu"):
    """The stub under a program's name, before the baseline. Run as the
    path's owner, so the directories it creates are that user's too (a
    root-owned ~/.config or ~/.local would stop the agent)."""
    return shell(as_, ["install", "-D", "-m", "0755", "/opt/parity-t2/llmstub.py", path], user=user)


def user_file(as_, path, content):
    """A file of ubuntu's, its directories created as ubuntu."""
    return shell(as_, ["sh", "-c", 'mkdir -p "$(dirname "$1")" && printf %s "$2" > "$1"', "sh", path, content],
                 user="ubuntu")


def scenario(domain, id_, steps, anchors, residue, masks=(), commands=(), rejected_only=False):
    out = hd.scenario(id_, steps, anchors, residue, masks=list(masks), rejected_only=rejected_only)
    out["id"] = f"t2.{domain}-domain.{id_}"
    out["owner"] = domain
    out["commands"] = [] if rejected_only else list(commands)
    # Heavy calls (install, configure, start) reserve 2 cores and 2 GiB; the
    # base twin's 2 GiB never fits them, so these twins get the nested-Incus
    # goldens' memory.
    out["memoryMb"] = 6144
    return out


def http_probe(as_, port, path="/v1/models", token=None, method="GET", body=None):
    """The harness's own client on the twin's loopback: status and body."""
    code = ("import sys,urllib.request,urllib.error\n"
            "r=urllib.request.Request(sys.argv[1],method=sys.argv[2],data=(sys.argv[4].encode() if sys.argv[4] "
            "else None))\n"
            "if sys.argv[3]: r.add_header('Authorization','Bearer '+sys.argv[3])\n"
            "r.add_header('Content-Type','application/json')\n"
            "try:\n    resp=urllib.request.urlopen(r,timeout=30); print(resp.status); print(resp.read().decode())\n"
            "except urllib.error.HTTPError as e:\n    print(e.code); print(e.read().decode())\n"
            "except OSError as e:\n    print(type(e).__name__)\n")
    return shell(as_, ["python3", "-c", code, f"http://127.0.0.1:{port}{path}", method, token or "",
                       json.dumps(body) if body is not None else ""], user="ubuntu")


INCOMING = "parity-incoming-token-0123456789abcdefghij"
RELAY = {"sessionId": "parity-t2-relay", "listenHost": "127.0.0.1", "listenPort": 18434, "targetHost": "127.0.0.1",
         "targetPort": 11434, "incomingToken": INCOMING, "upstreamToken": "", "allowedSourceCIDRs": ["127.0.0.0/8"]}
OLLAMA = HOME + "/bin/ollama"
OLLAMA_ENV = {"OPUTE_OLLAMA_BINARY": OLLAMA, "OPUTE_HOST_AGENT_RELAY_DIR": RELAY_DIR}

llm_cases = [
    # The shared Ollama runtime from nothing: install writes the generated
    # user unit, reloads and starts it, waits for the API, pulls and warms
    # the model; the per-model context is persisted as a managed model;
    # start reuses the healthy runtime and stop leaves it shared. The relay
    # carries an authenticated client to the runtime and is removed.
    scenario("llm", "ollama.lifecycle", [
        hd.LINGER, hd.wait_user(), install_stub("stub", OLLAMA),
        SNAP("baseline"), {"as": "serve", "serve": {"env": OLLAMA_ENV}},
        call("prerequisites", "check_local_llm_prerequisites", {}),
        task_call("install", "install_local_llm_model", {"modelRef": MODEL}, timeout=300),
        SNAP("installed", "baseline"),
        call("list", "list_local_llm_models", {}),
        task_call("persist", "configure_local_llm_model", {"modelRef": MODEL, "contextSize": 8192}),
        task_call("context", "configure_local_llm_model", {"modelRef": MODEL}),
        call("probe", "probe_local_llm", {"modelRef": MODEL, "includeChat": True}),
        task_call("start", "start_local_llm_runtime", {}),
        task_call("stop", "stop_local_llm_runtime", {}),
        SNAP("configured", "installed"),
        call("relay", "ensure_local_llm_relay", RELAY),
        http_probe("throughRelay", 18434, token=INCOMING),
        http_probe("chatThroughRelay", 18434, "/v1/chat/completions", INCOMING, "POST",
                   {"model": MODEL, "stream": True, "messages": [{"role": "user", "content": "hi"}]}),
        http_probe("relayNoToken", 18434),
        call("openai", "probe_openai_compatible_server", {"endpoint": "http://127.0.0.1:18434",
                                                          "bearerToken": INCOMING, "includeChat": True}),
        SNAP("relayed", "configured"),
        call("removeRelay", "remove_local_llm_relay", {"sessionId": "parity-t2-relay"}),
        http_probe("relayGone", 18434, token=INCOMING),
        SNAP("removed", "relayed"),
        STOP, SNAP("teardown", "baseline"),
    ], LLM + ["internal/domain/llm/ollama.go", "internal/domain/llm/relay.go", "internal/domain/llm/runtime_probe.go"],
        "The generated opute-ollama.service stays enabled and running (Go never stops the shared runtime), with "
        "its model store, the runtime config and lingering; the relay state directory stays empty.",
        masks=[{"path": ["steps", "*", "files", change, HOME + "/.config/opute/ollama-runtime.json", *rest],
                "type": "hex", "optional": True,
                "reason": "the runtime config stamps each persisted model context with its updatedAt time; "
                          "its content is compared in T1 (llm-domain.ollama.lifecycle)"}
               for change, rest in (("added", ["sha256"]), ("changed", ["after", "sha256"]),
                                    ("changed", ["before", "sha256"]))],
        commands=["systemctl"]),
]

# llama-server: the managed binary and the artifact are placed before the
# baseline; the stub nvidia-smi stands in for the GPU.
LLAMA_BINARY = HOME + "/.local/share/opute/llama-server/llama-server"
ARTIFACT = b"GGUF parity CPU stub model\n"
ARTIFACT_PATH = HOME + "/models/base-llama.gguf"
LLAMA_MODEL = "qwen3.5-0.8b/base-llama"
MANIFEST = {
    "runtime": "llama-cpp", "modelRef": LLAMA_MODEL, "artifactPath": ARTIFACT_PATH,
    "artifactSha256": hashlib.sha256(ARTIFACT).hexdigest(), "baseModel": "Qwen/Qwen3.5-0.8B", "revision": "r1",
    "tokenizerRevision": "t1", "chatTemplateHash": "d" * 64, "quantization": "Q4_K_M",
    "binaryPath": LLAMA_BINARY, "binaryVersion": "6500", "binarySha256": STUB_SHA, "binarySource": "host-build",
    "sourceRevision": "b6500", "sourceSha256": "c" * 64,
    "binaryBuildSourceUri": "https://github.com/ggml-org/llama.cpp/archive/b6500.tar.gz", "cudaArchitectures": "86",
    "contextSize": 4096,
    # The base golden has no binutils, so the agent's ldd/strings linkage
    # read would find nothing; the manifest declares CUDA instead (T1
    # llm-domain.llama.lifecycle covers the linkage read).
    "cudaEnabled": True,
}
BUILD = {"runtime": "llama-cpp", "sourceUri": MANIFEST["binaryBuildSourceUri"], "sourceSha256": "c" * 64,
         "sourceRevision": "b6500", "cudaArchitectures": "86"}
llm_cases.append(scenario("llm", "llama.lifecycle", [
    hd.LINGER, hd.wait_user(),
    install_stub("binary", LLAMA_BINARY), install_stub("gpu", "/usr/local/bin/nvidia-smi", "root"),
    user_file("artifact", ARTIFACT_PATH, ARTIFACT.decode()),
    SNAP("baseline"), {"as": "serve", "serve": {"env": {"OPUTE_LLAMA_SERVER_PORT": "18080"}}},
    task_call("install", "install_local_llm_model", MANIFEST, timeout=300),
    SNAP("installed", "baseline"),
    call("probe", "probe_local_llm", {"runtime": "llama-cpp", "includeChat": True}),
    call("adopt", "ensure_local_llm_server_binary", BUILD),
    call("toolchain", "ensure_local_llm_server_binary", {**BUILD, "sourceRevision": "b6501"}),
    task_call("start", "start_local_llm_runtime", {"runtime": "llama-cpp"}, timeout=300),
    SNAP("restarted", "installed"),
    task_call("stop", "stop_local_llm_runtime", {"runtime": "llama-cpp"}),
    SNAP("stopped", "baseline"),
    STOP, SNAP("teardown", "baseline"),
], LLM + ["internal/domain/llm/server.go", "internal/domain/llm/server_build.go"],
    "The llama-server manifest and its disabled, stopped unit file stay, with lingering; the managed binary, "
    "artifact and stub nvidia-smi predate the baseline.", commands=["systemctl", "bash"]))

# Every M4-rejected llm and serving call, with arguments that would change
# the twin if admitted, leaves it unchanged.
REJECTED = [
    ("configure_local_llm_model", {"modelRef": MODEL, "contextSize": 8192}),
    ("ensure_local_llm_k3s_proxy", {"vmName": "k1", "nodePort": 30080, "relayHost": "10.0.100.1", "relayPort": 18434,
                                    "relayToken": INCOMING, "bearerKey": "k" * 40}),
    ("ensure_local_llm_relay", RELAY),
    ("ensure_local_llm_server_binary", BUILD),
    ("install_local_llm_model", {"modelRef": MODEL}),
    ("reconcile_serving_assignment", None),
    ("remove_local_llm_k3s_proxy", {"vmName": "k1"}),
    ("remove_local_llm_relay", {"sessionId": "parity-t2-relay"}),
    ("start_local_llm_runtime", {}),
    ("stop_local_llm_runtime", {"runtime": "llama-cpp"}),
]

# --- serving -----------------------------------------------------------------------------

WWW = HOME + "/www"
SVC_UNIT = HOME + "/.config/systemd/user/parity-svc.service"
ASSIGNMENT = {
    "contractVersion": "serving-assignment.v1", "assignmentId": "parity-t2/web", "generation": 1,
    "idempotencyKey": "rev-1", "service": "web", "mode": "dev-process", "runtime": "process",
    "target": {"hostId": "twin", "resourceId": "web", "kind": "host", "instanceType": "host"},
    "artifact": {"kind": "source", "sourceDir": WWW,
                 "command": ["python3", "-m", "http.server", "18090", "--bind", "127.0.0.1"],
                 "environment": {"PYTHONDONTWRITEBYTECODE": "1"}},
    "endpoints": [{"name": "http", "protocol": "http", "port": 18090}],
    "readiness": [{"endpointName": "http", "path": "/"}],
    "exposure": {"kind": "none"}, "restartPolicy": "on-failure",
    "serviceUnit": "parity-svc.service", "desiredState": "start",
}
REJECTED = [(n, a if a is not None else ASSIGNMENT) for n, a in REJECTED]
WAIT_READY = shell("ready", ["sh", "-c", "for i in $(seq 1 100); do python3 -c 'import urllib.request; "
                                         "urllib.request.urlopen(\"http://127.0.0.1:18090/\", timeout=1)' "
                                         "2>/dev/null && exit 0; sleep 0.1; done; exit 1"], user="ubuntu")
# The process is launched without waiting for its port, so whether the same
# call's own probe already sees it listening is the launch's timing.
LAUNCH_RACE = [{"path": ["steps", label, "body", "result", "structuredContent", "readiness", 0, key],
                "type": t, "reason": "the probe right after a launch races the new process binding its port"}
               for label in ("launch", "generation") for key, t in (("ready", "bool"), ("message", "string"))]
serving_cases = [
    scenario("serving", "assignment.lifecycle", [
        hd.LINGER, hd.wait_user(),
        user_file("www", WWW + "/index.html", "parity\n"),
        user_file("unit", SVC_UNIT, "[Unit]\nDescription=Opute parity T2 serving unit\n\n[Service]\n"
                                    "ExecStart=/bin/sleep infinity\n"),
        SNAP("baseline"), hd.SERVE,
        call("launch", "reconcile_serving_assignment", ASSIGNMENT),
        dict(WAIT_READY, **{"as": "launchReady"}),
        call("again", "reconcile_serving_assignment", ASSIGNMENT),
        SNAP("running", "baseline"),
        call("generation", "reconcile_serving_assignment", {**ASSIGNMENT, "generation": 2,
                                                            "idempotencyKey": "rev-2", "desiredState": "restart"}),
        dict(WAIT_READY, **{"as": "generationReady"}),
        call("steady", "reconcile_serving_assignment", {**ASSIGNMENT, "generation": 2, "idempotencyKey": "rev-2"}),
        SNAP("relaunched", "running"),
        STOP, SNAP("teardown", "baseline"),
    ], ["internal/tools/dispatch_serving.go", "internal/domain/serving/assignment.go",
        "internal/hostagent/serving_domain.go"],
        "The assignment's transient user unit keeps serving and parity-svc.service keeps running (Go has no "
        "inverse of a reconcile), with lingering; the pid and state files are under /tmp, outside the snapshot.",
        masks=LAUNCH_RACE, commands=["systemctl", "systemd-run", "kill", "bash"]),
]

TOOLS = {t["tool"] for d in ("llm", "serving") for t in json.load(open(f"../../baseline/inventory/{d}-domain.json"))["tools"]}
GATED = hd.M4_GATED & TOOLS
assert GATED == {n for n, _ in REJECTED}, sorted(GATED ^ {n for n, _ in REJECTED})


def rejected(prefix, meta=None):
    out = []
    for name, args in REJECTED:
        step = call(prefix + "." + name, name, args)
        if meta:
            step["mcp"]["meta"] = meta
        out.append(step)
    return out


def empty(as_):
    return {"as": as_, "snapshot": {"baseline": "baseline", "expectEmpty": True}}


llm_cases.append(scenario("llm", "admission.rejected-no-effect", [
    install_stub("stub", OLLAMA),
    {"as": "serveClosed", "serve": {"env": OLLAMA_ENV, "unset": ["OPUTE_STANDALONE_ALLOW_MUTATIONS"]}},
    SNAP("baseline"),
    rejected("gateClosed"),
    empty("afterGateClosed"),
    {"as": "stopClosed", "stop": {}},
    {"as": "serveOpen", "serve": {"env": OLLAMA_ENV}},
    rejected("staleRevision", meta={"catalogRevision": "sha256:0000"}),
    empty("afterOpen"),
    STOP, empty("teardown"),
], hd.ADMISSION + ["internal/domain/llm", "internal/domain/serving"],
    "None: every call is refused before it reaches the twin's user manager, loopback or files.",
    rejected_only=True))
llm_cases[-1]["compare"]["divergences"] = [hd.D19]

if __name__ == "__main__":
    for domain, cases in (("llm", llm_cases), ("serving", serving_cases)):
        out = HERE / "scenarios-t2" / f"{domain}-domain.json"
        out.write_text(json.dumps(cases, indent=1) + "\n")
        print(f"wrote {len(cases)} T2 scenarios to {out}")
