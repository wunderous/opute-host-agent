#!/usr/bin/env python3
"""Generate the M8f llm- and serving-domain T1 matrices:
scenarios/llm-domain.json, scenarios/serving-domain.json and their shim
fixtures (fixtures/shims/llm-domain-*.json, serving-domain-*.json).

Run from tools/parity: python3 llm_domain_scenarios.py

Nothing reaches the developer host's user manager, GPU or processes: every
fixture shims systemctl, systemd-run, ss, nvidia-smi and kill, and the
Ollama binary is the recording shim (OPUTE_OLLAMA_BINARY). The runtimes'
HTTP APIs are a route stub (Ollama's /api/*, the OpenAI-compatible /v1/*)
that answers by method, path and body, and that a shimmed `systemctl --user
enable --now` brings up through a sandbox marker. The stub records every
request (method, path, JSON body and the headers a relay sets or strips), so
the calls each side makes to a runtime are compared as well as its argv.

- **ollama:** prerequisites, list, probe, install (pull, warm, probe), the
  per-model context (read, persist, steady, change), start and the shared
  stop, each refusal; the generated unit and its wait, an externally owned
  runtime retiring the raced unit, and a unit that fails to start.
- **llama-server:** the pinned manifest's install (CUDA linkage read from the
  managed binary, the unit, readiness, GPU residency), start, probe, stop,
  each manifest refusal, and the binary build's refusals and adoption.
- **relay:** the authenticated relay (source, path and token checks, the
  upstream token swapped in, a streamed chat answer), persisted, restored on
  the domain's first call after a restart, replaced and removed.
- **cluster proxy:** Go's refusals (its catalog entries bind no resource).
- **openai-compatible probe:** discovery, model match and streaming chat.
- **serving:** an assignment launched under its transient unit, observed,
  relaunched for a new generation, adopted, and refused; ingress discovery.

Ports a stub or relay picks are this run's own: strings are normalized as
run variables, integers masked by type.
"""
import copy
import hashlib
import json
import sys
from pathlib import Path

import cluster_domain_scenarios as cds

HERE = Path(__file__).resolve().parent
call, task_call, flat, rule = cds.call, cds.task_call, cds.flat, cds.rule
STOP = {"as": "stop", "stop": {}}
DEFAULT = {"exit": 1, "stderr": "unsupported parity shim invocation\n"}
TS = "wall-clock timestamp of this side's run"
PORT = "a port a stub or listener picked for this run"
RESULT = ["body", "result", "structuredContent"]
TASK_RESULT = ["body", "result", "result", "structuredContent"]
MASKS = [{"path": ["steps", "*", "body", "result", k], "type": "rfc3339", "reason": TS, "optional": True}
         for k in ("createdAt", "lastUpdatedAt")] + [
    {"path": ["homeTree", "*", "sha256"], "type": "hex", "optional": True,
     "reason": "a file the agent writes names this side's ports and sandbox; its content is compared by readFile"}]

MODEL = "parity/tiny:1b"
EMBED = "embed/mini"
ALIAS = "opute/context-" + hashlib.sha256(MODEL.encode()).hexdigest()[:16]
OLLAMA_ENV = {"OPUTE_OLLAMA_BINARY": "${SANDBOX}/shims/ollama", "OPUTE_OLLAMA_PORT": "${LLM_PORT}",
              "OPUTE_OLLAMA_MODEL": MODEL}


def serve(as_="start", **env):
    s = copy.deepcopy(cds.SERVE)
    s["as"] = as_
    s["serve"]["env"].update({"OPUTE_HOST_AGENT_RELAY_DIR": "${SANDBOX}/home/.opute/relays", **env})
    return s


def read(as_, path, json_=False):
    return {"as": as_, "readFile": {"path": path, "json": json_}}


def write(as_, path, content="", mode="0644"):
    return {"as": as_, "write": {"path": path, "content": content, "mode": mode}}


def stub(routes, var="LLM_PORT", as_="runtime"):
    return {"as": as_, "routeStub": {"routes": routes, "portVar": var}}


def requests(as_, stub_="runtime"):
    return {"as": as_, "routeRequests": {"stub": stub_, "clear": True}}


def port_mask(label, *path, task=False, optional=False):
    m = {"path": ["steps", label, *(TASK_RESULT if task else RESULT), *path], "type": "int", "reason": PORT}
    if optional:
        m["optional"] = True
    return m


def scenario(domain, id_, steps, anchors, fixture, masks=(), exclusive=False, collect=("homeTree",)):
    s = {"id": f"{domain}-domain.{id_}", "surface": f"{domain}-domain", "owner": domain,
         "anchors": anchors, "fixture": fixture, "collect": ["trace", "orphans", *collect],
         "steps": flat(steps), "compare": {"masks": MASKS + list(masks)}}
    if exclusive:
        s["exclusive"] = True
    return s


def j(route, status=200, **extra):
    """A route answering with JSON."""
    method, path = route.split(" ", 1)
    out = {"method": method, "path": path, "status": status}
    out.update(extra)
    return out


TAGS = {"models": [{"name": MODEL, "digest": "sha256:" + "a" * 64, "size": 1234567},
                   {"name": EMBED + ":latest", "digest": "sha256:" + "b" * 64, "size": 7654}]}
# The Ollama runtime, up once run/ollama-up exists. A generate call makes the
# model resident (run/ollama-warm); /api/show answers the managed alias with
# its persisted num_ctx and a base model without one.
OLLAMA = [
    j("GET /api/tags", ifExists="run/ollama-up", json=TAGS),
    j("GET /api/tags", 503, body="starting\n"),
    j("GET /api/ps", ifExists="run/ollama-warm",
      json={"models": [{"name": MODEL, "size_vram": 1610612736, "context_length": 4096}]}),
    j("GET /api/ps", json={"models": []}),
    j("GET /v1/models", ifExists="run/ollama-up", json={"object": "list", "data": [{"id": MODEL}]}),
    j("GET /v1/models", 503, body="starting\n"),
    j("POST /api/show", bodyContains='"' + ALIAS + '"', json={"parameters": "num_ctx                        8192"}),
    j("POST /api/show", 404, bodyContains='"missing/', json={"error": "model 'missing/none:1b' not found"}),
    j("POST /api/show", json={"parameters": "stop                           \"<|im_end|>\""}),
    j("POST /api/create", json={"status": "success"}),
    j("POST /api/generate", creates=["run/ollama-warm"], json={"model": MODEL, "response": ".", "done": True}),
]

SYSTEMCTL = [
    rule(["--user", "show-environment"], "PATH=/usr/bin\n"),
    rule(["--user", "is-active", "opute-ollama.service"], "inactive\n", exit_=3, unlessExists="run/ollama-unit-active"),
    rule(["--user", "is-active", "opute-ollama.service"], "active\n"),
    rule(["--user", "is-active", "--quiet", "opute-ollama.service"], exit_=3),
    rule(["--user", "daemon-reload"]),
    rule(["--user", "enable", "--now", "opute-ollama.service"], ifExists="run/ollama-enable-fails", exit_=1,
         stderr="Failed to enable unit: Unit opute-ollama.service has a bad unit file setting.\n"),
    rule(["--user", "enable", "--now", "opute-ollama.service"],
         stderr="Created symlink default.target.wants/opute-ollama.service.\n", creates=["run/ollama-up"]),
    rule(["--user", "disable", "--now", "opute-ollama.service"],
         stderr="Removed default.target.wants/opute-ollama.service.\n"),
    rule(["--user", "disable", "--now", "opute-llama-server.service"], ifExists="run/llama-enabled",
         removes=["run/llama-enabled", "run/llama-up"]),
    rule(["--user", "disable", "--now", "opute-llama-server.service"], exit_=1,
         stderr="Failed to disable unit: Unit file opute-llama-server.service does not exist.\n"),
    rule(["--user", "enable", "opute-llama-server.service"], creates=["run/llama-enabled"]),
    rule(["--user", "restart", "opute-llama-server.service"], creates=["run/llama-up"]),
    rule(["--user", "enable", "--now", "opute-llama-server.service"], creates=["run/llama-enabled", "run/llama-up"]),
]
NVIDIA = [
    rule([], "GPU 0: Parity GPU (UUID: GPU-00000000-0000-0000-0000-000000000000)\n"),
    rule(["--query-gpu=memory.used", "--format=csv,noheader,nounits"], "1536\n"),
    rule(["--query-gpu=name,memory.total", "--format=csv,noheader,nounits"], "Parity GPU, 8192\n"),
]
HOST = {
    "systemctl": SYSTEMCTL,
    "nvidia-smi": NVIDIA,
    "ss": [rule(["..."])],
    "kill": [rule(["..."], exit_=1, stderr="kill: no such process\n")],
    "systemd-run": [rule(["..."], exit_=1, stderr="Failed to connect to bus: parity\n")],
}
FIXTURES = {
    "llm-domain-bare": {
        "description": "M8f: no Ollama binary on PATH; the service manager and GPU are shims.",
        "commands": HOST, "default": DEFAULT},
    "llm-domain-host": {
        "description": "M8f: the Ollama binary, service manager and GPU are shims; the runtime APIs are the "
                       "scenario's route stub.",
        "commands": {**HOST, "ollama": [
            rule(["pull", "missing/none:1b"], exit_=1, stderr="Error: pull model manifest: file does not exist\n"),
            rule(["pull", "..."], "pulling manifest\nsuccess\n"),
        ]},
        "default": DEFAULT},
}

LLM = ["internal/tools/dispatch_llm.go", "internal/tools/dispatch.go:resolveLocalLLMModelArg"]
OLLAMA_ANCHORS = LLM + ["internal/domain/llm/ollama.go"]
LLAMA_ANCHORS = LLM + ["internal/domain/llm/server.go", "internal/domain/llm/server_build.go"]
RELAY_ANCHORS = LLM + ["internal/domain/llm/relay.go", "internal/domain/llm/gateway.go:ValidateLocalLLMRelayArgs"]
CONFIG = "${SANDBOX}/home/.config/opute/ollama-runtime.json"
UNIT = "${SANDBOX}/home/.config/systemd/user/opute-ollama.service"
CONFIG_MASKS = [{"path": ["steps", "config", "content", "modelContexts", "*", "updatedAt"], "type": "rfc3339",
                 "reason": TS, "optional": True},
                {"path": ["steps", "config", "content", "port"], "type": "int", "reason": PORT}]




llm_cases = [
    # No binary and no runtime: the prerequisites name the blockers, probes
    # report the refused API, and every mutation refuses (after Go persists
    # the runtime config, as it does).
    scenario("llm", "ollama.absent", [
        serve(OPUTE_OLLAMA_PORT="1"),
        call("prerequisites", "check_local_llm_prerequisites", {}),
        call("list", "list_local_llm_models", {}),
        call("probe", "probe_local_llm", {"includeChat": True, "modelRef": MODEL}),
        task_call("install", "install_local_llm_model", {"modelRef": MODEL}),
        task_call("start", "start_local_llm_runtime", {}),
        task_call("stop", "stop_local_llm_runtime", {}),
        task_call("context", "configure_local_llm_model", {"modelRef": MODEL}),
        task_call("contextNoModel", "configure_local_llm_model", {"contextSize": 4096}),
        task_call("contextRange", "configure_local_llm_model", {"modelRef": MODEL, "contextSize": 100}),
        task_call("installBadRef", "install_local_llm_model", {"modelRef": "bad ref!"}),
        task_call("installMismatch", "install_local_llm_model", {"modelRef": MODEL, "model": "other:1b"}),
        task_call("installPreset", "install_local_llm_model", {"modelPreset": "gpt-x"}),
        task_call("installLlamaPreset", "install_local_llm_model", {"runtime": "llama-cpp", "modelPreset": "lfm2-2.6b"}),
        task_call("installRole", "install_local_llm_model", {"modelRef": MODEL, "role": "vision"}),
        call("probeRole", "probe_local_llm", {"role": "vision"}),
        call("probeRuntime", "probe_local_llm", {"runtime": "vllm"}),
        task_call("installRuntime", "install_local_llm_model", {"runtime": "vllm", "modelRef": MODEL}),
        task_call("configureRuntime", "configure_local_llm_model", {"runtime": "vllm"}),
        task_call("startRuntime", "start_local_llm_runtime", {"runtime": "vllm"}),
        task_call("stopRuntime", "stop_local_llm_runtime", {"runtime": "vllm"}),
        call("binaryRuntime", "ensure_local_llm_server_binary", {"runtime": "ollama", "sourceUri": "https://x"}),
        read("config", CONFIG, json_=True),
        STOP], OLLAMA_ANCHORS, "llm-domain-bare"),
    # A healthy runtime: the default model, the model list, an embedding
    # install (no warm, not the default) and a language install (pull, warm,
    # resident), then the per-model context read, persisted, steady and
    # changed; start and the shared stop; a pull that fails.
    scenario("llm", "ollama.lifecycle", [
        write("up", "${SANDBOX}/run/ollama-up"),
        stub(OLLAMA),
        serve(**OLLAMA_ENV),
        call("prerequisites", "check_local_llm_prerequisites", {}),
        call("list", "list_local_llm_models", {}),
        requests("listRequests"),
        task_call("installEmbed", "install_local_llm_model", {"modelRef": EMBED, "role": "embedding"}),
        task_call("install", "install_local_llm_model", {"modelRef": MODEL}),
        requests("installRequests"),
        call("probe", "probe_local_llm", {"includeChat": True}),
        task_call("context", "configure_local_llm_model", {"modelRef": MODEL}),
        task_call("persist", "configure_local_llm_model", {"modelRef": MODEL, "contextSize": 8192}),
        task_call("steady", "configure_local_llm_model", {"modelRef": MODEL, "numCtx": 8192}),
        task_call("change", "configure_local_llm_model", {"modelRef": MODEL, "contextSize": 4096}),
        task_call("missing", "configure_local_llm_model", {"modelRef": "missing/none:1b", "contextSize": 4096}),
        requests("contextRequests"),
        call("probeAfter", "probe_local_llm", {"model": MODEL, "includeChat": True}),
        task_call("start", "start_local_llm_runtime", {"runtime": "ollama"}),
        task_call("stop", "stop_local_llm_runtime", {}),
        task_call("pullFails", "install_local_llm_model", {"modelRef": "missing/none:1b", "setDefault": False}),
        requests("laterRequests"),
        read("config", CONFIG, json_=True),
        STOP], OLLAMA_ANCHORS, "llm-domain-host", CONFIG_MASKS),
    # The runtime is down: install writes the generated unit (models
    # directory and context from the environment), reloads and starts it,
    # waits for the API, pulls and warms. A later start finds the API served
    # by another owner while the unit is inactive and retires the unit.
    scenario("llm", "ollama.unit", [
        stub(OLLAMA),
        serve(**OLLAMA_ENV, OLLAMA_MODELS="${SANDBOX}/models", OPUTE_OLLAMA_CONTEXT_SIZE="16384"),
        call("prerequisites", "check_local_llm_prerequisites", {}),
        task_call("install", "install_local_llm_model", {"modelRef": MODEL, "port": {"$int": "${LLM_PORT}"}},
                  timeout=90),
        read("unit", UNIT),
        task_call("start", "start_local_llm_runtime", {}),
        requests("requests"),
        read("config", CONFIG, json_=True),
        STOP], OLLAMA_ANCHORS, "llm-domain-host", CONFIG_MASKS),
    # The generated unit fails to start.
    scenario("llm", "ollama.unit-fails", [
        write("fails", "${SANDBOX}/run/ollama-enable-fails"),
        stub(OLLAMA),
        serve(**OLLAMA_ENV),
        task_call("start", "start_local_llm_runtime", {}),
        read("unit", UNIT),
        STOP], OLLAMA_ANCHORS, "llm-domain-host"),
]

# --- llama-server ----------------------------------------------------------------------

LLAMA_MODEL = "qwen3.5-0.8b/base-llama"
ARTIFACT = b"GGUF parity model\n"
BINARY = b"#!/bin/sh\n# linked against libggml-cuda.so and libcudart.so.12\necho 'version: 6500 (parity)'\n"
SHA = lambda data: hashlib.sha256(data).hexdigest()  # noqa: E731
ARTIFACT_PATH = "${SANDBOX}/home/models/" + "model.gguf"
BINARY_PATH = "${SANDBOX}/home/.local/share/opute/llama-server/llama-server"
SOURCE_SHA = "c" * 64
MANIFEST = {
    "runtime": "llama-cpp", "modelRef": LLAMA_MODEL, "artifactPath": ARTIFACT_PATH,
    "artifactSha256": SHA(ARTIFACT), "baseModel": "Qwen/Qwen3.5-0.8B", "revision": "r1",
    "tokenizerRevision": "t1", "chatTemplateHash": "d" * 64, "quantization": "Q4_K_M",
    "binaryPath": BINARY_PATH, "binaryVersion": "6500", "binarySha256": SHA(BINARY),
    "binarySource": "host-build", "sourceRevision": "b6500", "sourceSha256": SOURCE_SHA,
    "binaryBuildSourceUri": "https://github.com/ggml-org/llama.cpp/archive/b6500.tar.gz",
    "cudaArchitectures": "86", "contextSize": 4096,
}
LLAMA = [
    j("GET /v1/models", ifExists="run/llama-up",
      json={"object": "list", "data": [{"id": LLAMA_MODEL + ".gguf", "owned_by": "llamacpp"}]}),
]
LLAMA_CONFIG = "${SANDBOX}/home/.config/opute/llama-server.json"
LLAMA_UNIT = "${SANDBOX}/home/.config/systemd/user/opute-llama-server.service"
LLAMA_MASKS = [{"path": ["steps", "config", "content", "port"], "type": "int", "reason": PORT}]


def manifest(**over):
    out = dict(MANIFEST)
    for k, v in over.items():
        if v is None:
            out.pop(k, None)
        else:
            out[k] = v
    return out


BUILD = {"runtime": "llama-cpp", "sourceUri": MANIFEST["binaryBuildSourceUri"], "sourceSha256": SOURCE_SHA,
         "sourceRevision": "b6500", "cudaArchitectures": "86"}

llm_cases += [
    # The pinned manifest: the binary's CUDA linkage read from the managed
    # file, the previous generation stopped, the GPU checked, the unit
    # written and started, readiness and GPU residency probed. Then the
    # build adopts the installed binary, start restarts, stop disables.
    scenario("llm", "llama.lifecycle", [
        write("artifact", ARTIFACT_PATH, ARTIFACT.decode()),
        write("binary", BINARY_PATH, BINARY.decode(), "0755"),
        stub(LLAMA),
        serve(OPUTE_LLAMA_SERVER_PORT="${LLM_PORT}"),
        call("probeBefore", "probe_local_llm", {"runtime": "llama-cpp"}),
        task_call("install", "install_local_llm_model", manifest(), timeout=90),
        read("unit", LLAMA_UNIT),
        read("config", LLAMA_CONFIG, json_=True),
        call("probe", "probe_local_llm", {"runtime": "llama-cpp", "includeChat": True}),
        call("adopt", "ensure_local_llm_server_binary", BUILD),
        call("adoptOther", "ensure_local_llm_server_binary", {**BUILD, "sourceRevision": "b6501"}),
        task_call("start", "start_local_llm_runtime", {"runtime": "llama-cpp"}, timeout=90),
        task_call("configure", "configure_local_llm_model", {"runtime": "llama-cpp"}),
        task_call("stop", "stop_local_llm_runtime", {"runtime": "llama-cpp"}),
        task_call("stopAgain", "stop_local_llm_runtime", {"runtime": "llama-cpp"}),
        requests("requests"),
        STOP], LLAMA_ANCHORS, "llm-domain-host", LLAMA_MASKS),
    # Every manifest refusal, each before a unit is touched, and the binary
    # build's argument refusals.
    scenario("llm", "llama.refused", [
        write("artifact", ARTIFACT_PATH, ARTIFACT.decode()),
        write("binary", BINARY_PATH, BINARY.decode(), "0755"),
        serve(OPUTE_LLAMA_SERVER_PORT="1"),
        task_call("binaryUri", "install_local_llm_model", manifest(binaryUri="https://x/llama-server")),
        task_call("quantization", "install_local_llm_model", manifest(quantization="Q8_0")),
        task_call("thinking", "install_local_llm_model", manifest(chatTemplateKwargs='{"enable_thinking":true}')),
        task_call("kwargs", "install_local_llm_model", manifest(chatTemplateKwargs="{")),
        task_call("artifactSha", "install_local_llm_model", manifest(artifactSha256="e" * 64)),
        task_call("noArtifact", "install_local_llm_model", manifest(artifactPath=None)),
        task_call("binarySha", "install_local_llm_model", manifest(binarySha256="f" * 64)),
        task_call("templateHash", "install_local_llm_model", manifest(chatTemplateHash="zz")),
        task_call("gpuLayers", "install_local_llm_model", manifest(gpuLayers=10)),
        task_call("context", "install_local_llm_model", manifest(contextSize=256)),
        task_call("source", "install_local_llm_model", manifest(binarySource="download")),
        task_call("revision", "install_local_llm_model", manifest(revision=" ")),
        task_call("missingBinary", "install_local_llm_model",
                  manifest(binaryPath="${SANDBOX}/home/.local/share/opute/llama-server/absent")),
        task_call("start", "start_local_llm_runtime", {"runtime": "llama-cpp"}),
        call("http", "ensure_local_llm_server_binary", {**BUILD, "sourceUri": "http://example.com/x.tar.gz"}),
        call("sha", "ensure_local_llm_server_binary", {**BUILD, "sourceSha256": "abc"}),
        call("revisionChars", "ensure_local_llm_server_binary", {**BUILD, "sourceRevision": "b6500;rm"}),
        call("arch", "ensure_local_llm_server_binary", {**BUILD, "cudaArchitectures": "86,89"}),
        call("relative", "ensure_local_llm_server_binary", {**BUILD, "outputPath": "bin/llama-server"}),
        call("outside", "ensure_local_llm_server_binary", {**BUILD, "outputPath": "/opt/llama-server"}),
        call("toolchain", "ensure_local_llm_server_binary", BUILD),
        STOP], LLAMA_ANCHORS, "llm-domain-host"),
]

# A manifest persisted without the fields the environment supplies: start
# takes the model, artifact, base model, binary, version and build from
# OPUTE_LLAMA_SERVER_* and persists the merged manifest.
PARTIAL = {"artifactSha256": SHA(ARTIFACT), "revision": "r1", "tokenizerRevision": "t1",
           "chatTemplateHash": "d" * 64, "quantization": "Q4_K_M", "binarySha256": SHA(BINARY),
           "binarySource": "host-build", "sourceRevision": "b6500", "sourceSha256": SOURCE_SHA, "cudaEnabled": True}
LLAMA_ENV = {"OPUTE_LLAMA_SERVER_PORT": "${LLM_PORT}", "OPUTE_LLAMA_SERVER_MODEL": LLAMA_MODEL,
             "OPUTE_LLAMA_SERVER_ARTIFACT_PATH": ARTIFACT_PATH, "OPUTE_LLAMA_SERVER_BASE_MODEL": "Qwen/Qwen3.5-0.8B",
             "OPUTE_LLAMA_SERVER_BINARY": "${SANDBOX}/home/bin/llama-server",
             "OPUTE_LLAMA_SERVER_VERSION": "6500", "OPUTE_LLAMA_SERVER_BUILD": "llama.cpp:b6500:cuda"}
llm_cases.append(scenario("llm", "llama.environment", [
    write("artifact", ARTIFACT_PATH, ARTIFACT.decode()),
    write("binary", "${SANDBOX}/home/bin/llama-server", BINARY.decode(), "0755"),
    write("manifest", LLAMA_CONFIG, json.dumps(PARTIAL), "0600"),
    stub(LLAMA),
    serve(**LLAMA_ENV),
    call("probeBefore", "probe_local_llm", {"runtime": "llama-cpp"}),
    task_call("start", "start_local_llm_runtime", {"runtime": "llama-cpp"}, timeout=90),
    read("config", LLAMA_CONFIG, json_=True),
    read("unit", LLAMA_UNIT),
    STOP], LLAMA_ANCHORS + ["internal/domain/llm/server.go:defaultLlamaServerConfig"], "llm-domain-host",
    LLAMA_MASKS))

# --- the relay -----------------------------------------------------------------------

INCOMING = "in-" + "t" * 40
UPSTREAM = "up-" + "u" * 40
RELAY = {"sessionId": "parity-relay", "listenHost": "127.0.0.1", "listenPort": 0, "targetHost": "127.0.0.1",
         "targetPort": {"$int": "${LLM_PORT}"}, "incomingToken": INCOMING, "upstreamToken": UPSTREAM,
         "allowedSourceCIDRs": ["127.0.0.0/8"]}
UPSTREAM_ROUTES = [
    j("GET /v1/models", json={"object": "list", "data": [{"id": MODEL}]}),
    j("GET /api/tags", json=TAGS),
    {"method": "POST", "path": "/v1/chat/completions", "headers": {"Content-Type": "text/event-stream"},
     "chunks": ['data: {"choices":[{"delta":{"content":"READY"}}]}\n\n',
                'data: {"choices":[{"delta":{},"finish_reason":"stop"}]}\n\n', "data: [DONE]\n\n"]},
]
BEARER = {"Authorization": "Bearer " + INCOMING}


def relay(**over):
    out = copy.deepcopy(RELAY)
    for k, v in over.items():
        if v is None:
            out.pop(k, None)
        else:
            out[k] = v
    return out


def through(as_, path, method="GET", headers=None, body=None, port="${RELAY_PORT}"):
    spec = {"method": method, "path": path, "headers": headers if headers is not None else BEARER, "port": port}
    if body is not None:
        spec["body"] = body
    return {"as": as_, "http": spec}


def ensure(as_, args, var="RELAY_PORT"):
    step = call(as_, "ensure_local_llm_relay", args)
    step["mcp"]["capture"] = {var: RESULT + ["listenPort"]}
    return step


PROXY = {"vmName": "k1", "nodePort": 30080, "relayHost": "10.0.100.1", "relayPort": 18080, "relayToken": INCOMING,
         "bearerKey": "k" * 40, "namespace": "opute-llm"}
RELAY_STATE = "${SANDBOX}/home/.opute/relays/parity-relay.json"
RELAY_MASKS = [port_mask(label, "listenPort") for label in ("ensure", "again", "restored")] + [
    {"path": ["steps", "state", "content", "TargetPort"], "type": "int", "reason": PORT},
    {"path": ["steps", "stateRestored", "content", "TargetPort"], "type": "int", "reason": PORT}]

llm_cases += [
    scenario("llm", "relay.lifecycle", [
        stub(UPSTREAM_ROUTES),
        serve(),
        ensure("ensure", relay()),
        ensure("again", relay()),
        through("models", "/v1/models"),
        through("tags", "/api/tags"),
        through("chat", "/v1/chat/completions", "POST", {**BEARER, "Content-Type": "application/json"},
                {"model": MODEL, "stream": True, "messages": [{"role": "user", "content": "hi"}]}),
        through("wrongToken", "/v1/models", headers={"Authorization": "Bearer " + "x" * 43}),
        through("noToken", "/v1/models", headers={}),
        through("path", "/api/generate", "POST"),
        through("root", "/"),
        requests("upstreamRequests"),
        read("state", RELAY_STATE, json_=True),
        STOP,
        # A restart: the relay is restored on the domain's first call.
        serve("restart"),
        call("firstCall", "probe_openai_compatible_server", {"endpoint": "http://127.0.0.1:1"}),
        ensure("restored", relay(), "RELAY_PORT2"),
        through("modelsRestored", "/v1/models", port="${RELAY_PORT2}"),
        read("stateRestored", RELAY_STATE, json_=True),
        # Another source is forbidden; a changed token replaces the session.
        ensure("replaced", relay(allowedSourceCIDRs=[], allowedSourceIP="10.9.9.9", incomingToken="",
                                 relayToken=INCOMING + "z"), "RELAY_PORT3"),
        through("forbidden", "/v1/models", port="${RELAY_PORT3}"),
        call("remove", "remove_local_llm_relay", {"sessionId": "parity-relay"}),
        call("removeAgain", "remove_local_llm_relay", {"sessionId": "parity-relay"}),
        read("stateRemoved", RELAY_STATE, json_=True),
        call("noSession", "ensure_local_llm_relay", relay(sessionId=" ")),
        call("hostName", "ensure_local_llm_relay", relay(listenHost="localhost")),
        call("wildcard", "ensure_local_llm_relay", relay(listenHost="0.0.0.0")),
        call("targetPort", "ensure_local_llm_relay", relay(targetPort=0)),
        call("noCidr", "ensure_local_llm_relay", relay(allowedSourceCIDRs=[])),
        call("badCidr", "ensure_local_llm_relay", relay(allowedSourceCIDRs=["10.0.0.0/33"])),
        call("shortToken", "ensure_local_llm_relay", relay(incomingToken="short")),
        call("upstreamNewline", "ensure_local_llm_relay", relay(upstreamToken="a\nb")),
        call("badSessionPath", "ensure_local_llm_relay", relay(sessionId="a/b")),
        call("removeBad", "remove_local_llm_relay", {"sessionId": "a/b"}),
        STOP], RELAY_ANCHORS, "llm-domain-bare",
        RELAY_MASKS + [port_mask("replaced", "listenPort")]),
    # Go's cluster proxy takes its VM from a resource binding its catalog
    # entries never declare: every call refuses before any effect.
    scenario("llm", "cluster-proxy", [
        serve(),
        call("ensure", "ensure_local_llm_k3s_proxy", PROXY),
        call("ensureUri", "ensure_local_llm_k3s_proxy", {**PROXY, "uri": cds.K1}),
        call("ensureSchema", "ensure_local_llm_k3s_proxy", {"relayHost": "10.0.100.1", "relayPort": 18080}),
        call("remove", "remove_local_llm_k3s_proxy", {"vmName": "k1"}),
        call("removeUri", "remove_local_llm_k3s_proxy", {"vmName": "k1", "uri": cds.K1}),
        STOP], LLM + ["internal/domain/llm/gateway.go"], "llm-domain-bare"),
    # The runtime-neutral probe: discovery with a bearer token, a model
    # match, a streamed chat, a missing model, a refused endpoint and bad
    # URLs.
    scenario("llm", "openai-probe", [
        stub(UPSTREAM_ROUTES + [j("GET /broken/v1/models", 500, body="boom"),
                                j("GET /garbled/v1/models", body="not json")]),
        serve(),
        call("probe", "probe_openai_compatible_server",
             {"endpoint": "http://127.0.0.1:${LLM_PORT}/", "bearerToken": "secret-token", "includeChat": True}),
        call("model", "probe_openai_compatible_server",
             {"endpoint": "http://127.0.0.1:${LLM_PORT}/v1?x=1#f", "modelRef": "tiny:1b", "includeChat": True}),
        call("missingModel", "probe_openai_compatible_server",
             {"endpoint": "http://127.0.0.1:${LLM_PORT}", "modelRef": "other:1b", "includeChat": True}),
        call("broken", "probe_openai_compatible_server", {"endpoint": "http://127.0.0.1:${LLM_PORT}/broken"}),
        call("garbled", "probe_openai_compatible_server", {"endpoint": "http://127.0.0.1:${LLM_PORT}/garbled"}),
        call("refused", "probe_openai_compatible_server", {"endpoint": "http://127.0.0.1:1"}),
        call("relative", "probe_openai_compatible_server", {"endpoint": "/v1"}),
        call("scheme", "probe_openai_compatible_server", {"endpoint": "ftp://127.0.0.1/v1"}),
        requests("requests"),
        STOP], ["internal/tools/dispatch_host.go", "internal/domain/llm/runtime_probe.go"], "llm-domain-bare"),
]

# --- serving ---------------------------------------------------------------------------

ASSIGNMENT_ID = "parity-m8f/web"
TOKEN = "parity-m8f-web"
TMP = f"/tmp/serving-assignment-{TOKEN}"
CLEAN = {"as": "clean", "remove": {"paths": [TMP + ".pid", TMP + ".state.json", TMP + "-supervisor.log",
                                             "/tmp/serving-assignment-parity-m8f-api*"]}}
TARGET = {"hostId": "host-1", "resourceId": "res-1", "kind": "host", "instanceType": "host"}
ASSIGNMENT = {
    "contractVersion": "serving-assignment.v1", "assignmentId": ASSIGNMENT_ID, "generation": 1,
    "idempotencyKey": "rev-1", "service": "web", "mode": "dev-process", "runtime": "process", "target": TARGET,
    "artifact": {"kind": "source", "sourceDir": "/srv/web", "command": ["npm", "run", "start"],
                 "environment": {"PORT": "3000"}},
    "endpoints": [{"name": "http", "protocol": "http", "port": 1}],
    "readiness": [{"endpointName": "http", "path": "/healthz"}],
    "exposure": {"kind": "none"},
}
READY_ENDPOINTS = [{"name": "http", "protocol": "http", "port": {"$int": "${WEB_PORT}"}},
                   {"name": "admin", "protocol": "tcp", "port": {"$int": "${WEB_PORT}"}},
                   {"name": "grpc", "protocol": "grpc", "port": 50051}]


def assignment(**over):
    out = copy.deepcopy(ASSIGNMENT)
    for k, v in over.items():
        if v is None:
            out.pop(k, None)
        else:
            out[k] = v
    return out


SERVING_SYSTEMCTL = [
    rule(["--user", "is-active", "--quiet", "host-serving-" + TOKEN], ifExists="run/serving-active"),
    rule(["--user", "is-active", "--quiet", "..."], exit_=3),
    rule(["--user", "stop", "..."]),
    rule(["--user", "reset-failed", "..."]),
    # No main PID: bash's builtin kill would probe a real pid, so the
    # supervisor loop never reaches it; the scenario writes the pid file.
    rule(["--user", "show", "..."], "0\n"),
]
FIXTURES["serving-domain-host"] = {
    "description": "M8f: the serving supervisor's user manager (systemctl, systemd-run) and kill are shims; "
                   "the unit never reports a main PID, and kill -0 finds pid 424242 alive.",
    "commands": {
        "systemctl": SERVING_SYSTEMCTL,
        "systemd-run": [rule(["..."], stderr="Running as unit: host-serving.service\n")],
        "kill": [rule(["-0", "424242"]), rule(["..."], exit_=1, stderr="kill: (1) - No such process\n")],
        "ss": [rule(["..."])], "nvidia-smi": NVIDIA,
    },
    "default": DEFAULT,
}
SERVING = ["internal/tools/dispatch_serving.go", "internal/domain/serving/assignment.go"]
WEB = [j("GET /healthz", body="ok\n"), j("GET /", body="ok\n")]


def reconcile(as_, args):
    return call(as_, "reconcile_serving_assignment", args)


serving_cases = [
    scenario("serving", "assignment.lifecycle", [
        CLEAN,
        stub(WEB, "WEB_PORT", "web"),
        serve(),
        # Not ready, no supervisor: launched under its transient unit.
        reconcile("launch", assignment(restartPolicy="on-failure")),
        read("pid", TMP + ".pid"),
        read("state", TMP + ".state.json", json_=True),
        # The same generation again: the live pid means it is starting.
        write("pidAlive", TMP + ".pid", "424242\n", "0600"),
        reconcile("again", assignment(restartPolicy="on-failure")),
        # Ready under its active supervisor: nothing to do.
        write("active", "${SANDBOX}/run/serving-active"),
        reconcile("ready", assignment(endpoints=READY_ENDPOINTS)),
        # A new generation relaunches.
        reconcile("generation", assignment(endpoints=READY_ENDPOINTS, generation=2, idempotencyKey="rev-2",
                                           restartPolicy="always")),
        read("stateAfter", TMP + ".state.json", json_=True),
        # A ready endpoint no supervisor owns: refused without preStart,
        # adopted with it.
        reconcile("unowned", assignment(assignmentId="parity-m8f/api", endpoints=READY_ENDPOINTS)),
        reconcile("adopt", assignment(assignmentId="parity-m8f/api", endpoints=READY_ENDPOINTS,
                                      artifact={**ASSIGNMENT["artifact"], "preStartCommand": "true"})),
        # Runtimes the host agent only admits and probes.
        reconcile("oci", assignment(mode="oci-release", runtime="podman",
                                    artifact={"kind": "oci", "digest": "sha256:" + "1" * 64})),
        STOP, CLEAN], SERVING, "serving-domain-host", exclusive=True),
    scenario("serving", "assignment.refused", [
        serve(),
        reconcile("contract", assignment(contractVersion="v2")),
        reconcile("assignmentId", assignment(assignmentId="-bad")),
        reconcile("generation", assignment(generation=0)),
        reconcile("mode", assignment(mode="batch")),
        reconcile("runtime", assignment(runtime="docker")),
        reconcile("noTarget", assignment(target=None)),
        reconcile("targetIds", assignment(target={**TARGET, "hostId": ""})),
        reconcile("hostType", assignment(target={**TARGET, "instanceType": "vm"})),
        reconcile("container", assignment(target={**TARGET, "kind": "system-container", "instanceType": "host"})),
        reconcile("cluster", assignment(target={**TARGET, "kind": "kubernetes-cluster", "instanceType": "host"})),
        reconcile("vm", assignment(target={**TARGET, "kind": "vm"})),
        reconcile("devPodman", assignment(runtime="podman")),
        reconcile("ociDigest", assignment(mode="oci-release", runtime="podman", artifact={"kind": "oci"})),
        reconcile("artifactKind", assignment(artifact={"kind": "oci", "command": ["x"]})),
        reconcile("noCommand", assignment(artifact={"kind": "source", "sourceDir": "/srv"})),
        reconcile("noEndpoints", assignment(endpoints=[])),
        reconcile("restart", assignment(restartPolicy="sometimes")),
        reconcile("unit", assignment(serviceUnit="bad unit")),
        reconcile("desired", assignment(serviceUnit="web.service", desiredState="stop")),
        reconcile("noSourceDir", assignment(artifact={"kind": "source", "command": ["x"]})),
        reconcile("blankCommand", assignment(artifact={"kind": "source", "sourceDir": "/srv", "command": [" "]})),
        reconcile("envKey", assignment(artifact={**ASSIGNMENT["artifact"], "environment": {"1BAD": "x"}})),
        reconcile("envValue", assignment(artifact={**ASSIGNMENT["artifact"], "environment": {"N": 1}})),
        reconcile("readinessShape", assignment(readiness=["http"])),
        STOP], SERVING, "serving-domain-host"),
]

# Ingress discovery: k1's bridge address from the instance list, the
# ingress Service's load balancer through the k3s provider's kubectl.
K1_ADDRESSES = {"name": "k1", "status": "Running", "type": "virtual-machine", "config": {}, "devices": {},
                "state": {"network": {
                    "lo": {"addresses": [{"family": "inet", "address": "127.0.0.1", "scope": "local"}]},
                    "cni0": {"addresses": [{"family": "inet", "address": "10.42.0.1", "scope": "global"}]},
                    "enp5s0": {"addresses": [{"family": "inet", "address": "10.0.100.17", "scope": "global"},
                                             {"family": "inet6", "address": "fd42::17", "scope": "global"}]}}}}
FIXTURES["serving-domain-ingress"] = {
    "description": "M8f: k1 with a bridge address and k3s; the ingress Services answered through the pinned "
                   "k3s provider's incus exec calls.",
    "commands": {"incus": [
        rule(["list", "--format", "json"], [K1_ADDRESSES, cds.instance("c1", "Running", "container")]),
        cds.kubectl(["get", "svc", "traefik", "-n", "kube-system", "-o", "json"], {
            "metadata": cds.meta("traefik", "kube-system"),
            "status": {"loadBalancer": {"ingress": [{"ip": "192.168.50.10"}, {"hostname": "lb.example"}]}}}),
        cds.kubectl(["get", "svc", "edge", "-n", "ingress", "-o", "json"], {
            "metadata": cds.meta("edge", "ingress"),
            "status": {"loadBalancer": {"ingress": [{"hostname": "edge.example"}]}}}),
        cds.kubectl(["get", "svc", "missing", "-n", "ingress", "-o", "json"], exit_=1,
                    stderr='Error from server (NotFound): services "missing" not found\n'),
        *cds.estate_rules()]},
    "default": DEFAULT,
}
ENDPOINTS = [{"name": "web", "hostname": "web.example"}, {"name": "api", "hostname": "api.example"}]
serving_cases.append(scenario("serving", "ingress", [
    serve(), *cds.k3s_install(),
    call("default", "discover_service_ingress", {"uri": cds.K1, "endpoints": ENDPOINTS}),
    call("named", "discover_service_ingress", {"uri": cds.K1, "endpoints": ENDPOINTS[:1],
                                               "ingressNamespace": "ingress", "ingressService": "edge"}),
    call("missing", "discover_service_ingress", {"uri": cds.K1, "endpoints": ENDPOINTS[:1],
                                                 "ingressNamespace": "ingress", "ingressService": "missing"}),
    call("blankName", "discover_service_ingress", {"uri": cds.K1, "endpoints": [{"name": " ", "hostname": "x"}]}),
    call("noEndpoints", "discover_service_ingress", {"uri": cds.K1, "endpoints": []}),
    call("container", "discover_service_ingress", {"uri": "cluster:local:c1", "endpoints": ENDPOINTS[:1]}),
    STOP], ["internal/tools/dispatch_serving.go", "internal/domain/serving/ingress.go",
            "internal/hostagent/serving_domain.go"], "serving-domain-ingress"))
serving_cases[-1]["compare"] = copy.deepcopy(cds.REAL_K3S["compare"])
_have = {json.dumps(m["path"]) for m in serving_cases[-1]["compare"]["masks"]}
serving_cases[-1]["compare"]["masks"] += [m for m in MASKS if json.dumps(m["path"]) not in _have]
serving_cases[-1]["compare"]["divergences"] = ["D19.neutral-host-tool-names"]

if __name__ == "__main__":
    for name, value in FIXTURES.items():
        (HERE / "fixtures" / "shims" / f"{name}.json").write_text(json.dumps(value, indent=1) + "\n")
    for domain, cases in (("llm", llm_cases), ("serving", serving_cases)):
        out = HERE / "scenarios" / f"{domain}-domain.json"
        out.write_text(json.dumps(cases, indent=1) + "\n")
        print(f"wrote {len(cases)} scenarios to {out}")
    print(f"wrote {len(FIXTURES)} fixtures")
