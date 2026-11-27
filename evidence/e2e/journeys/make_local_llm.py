#!/usr/bin/env python3
"""Write the local-LLM journey (m32) as JSON.

  python3 evidence/e2e/journeys/make_local_llm.py

The serving journey (e2e3.serving) uses a test-fixture runtime. Here a real one runs: the published recipe
docs/recipes/local-llm.recipe.yaml installs Ollama from its upstream release (pinned by SHA-256), points the
agent at it, pulls a small real model and activates the runtime after a streaming chat completion. Everything
the agent does goes through typed MCP calls; the guest commands only observe, and the chat request is the one a
user's client would send. CPU only: the llama.cpp path is GPU-only by contract (evidence/m32/inventory.md).
"""
from __future__ import annotations

import json
import os

import make_e2e1 as base
import make_e2e3a as ha
import make_e2e4a as e4
from make_e2e1 import HERE, a, inp, ok, req
from make_e2e3a import call, direct, res, task
from make_e2e4a import USER_SESSION, USER_SYSTEMCTL, linger, svc

SPEC = "openspec/changes/recorded-session-and-llm-tools/specs/recorded-session-and-llm-tools/spec.md"
GOAL = ".agents/goals/m32-local-llm.md (work 2 and 3)"
HOME = e4.HOME
UID = e4.OPUTE_UID
RECIPE = f"{HOME}/m32/local-llm.recipe.yaml"
OLLAMA = "v0.40.17"
OLLAMA_DIR = f"{HOME}/.local/share/opute/ollama/{OLLAMA}"
ARCHIVE = f"{HOME}/.cache/opute/ollama/ollama-{OLLAMA}-linux-amd64.tar.zst"
MODEL = "hf.co/Qwen/Qwen2.5-0.5B-Instruct-GGUF:Q4_K_M"
# qwen2.5-0.5b-instruct-q4_k_m.gguf at Qwen/Qwen2.5-0.5B-Instruct-GGUF revision 9217f5db79a2 (Hugging Face LFS oid)
GGUF_SHA = "74a4da8c9fdbcd15bd1f6d01d621410d31c6fc00986f5eb687824e7b93d7a9db"
UNIT = "opute-ollama.service"
UNIT_FILE = f"{HOME}/.config/systemd/user/{UNIT}"
ENDPOINT = "http://127.0.0.1:11434"
VERSION = os.environ.get("E2E_VERSION", base.RUST_VERSION)
AGENT = base.AGENT.replace(base.RUST_VERSION, VERSION)
FILES = {**base.TOOL_FILES, "recipe": inp("${REPO}/docs/recipes/local-llm.recipe.yaml")}

# The request a user's client sends: one streaming chat completion, timed to its first content token.
CHAT = ("python3 - <<'PY'\nimport json, time, urllib.request\n"
        f"body = {{'model': '{MODEL}', 'stream': True, 'temperature': 0, 'max_tokens': 32, "
        "'messages': [{'role': 'user', 'content': 'Reply with one word: what colour is the sky on a clear day?'}]}\n"
        f"req = urllib.request.Request('{ENDPOINT}/v1/chat/completions', method='POST', "
        "data=json.dumps(body).encode(), headers={'Content-Type': 'application/json'})\n"
        "start = time.monotonic(); first = None; text = ''\n"
        "with urllib.request.urlopen(req, timeout=120) as r:\n"
        "    status = r.status\n"
        "    for line in r:\n"
        "        line = line.decode().strip()\n"
        "        if not line.startswith('data: ') or line == 'data: [DONE]':\n"
        "            continue\n"
        "        delta = json.loads(line[6:])['choices'][0].get('delta', {}).get('content') or ''\n"
        "        if delta and first is None:\n"
        "            first = time.monotonic()\n"
        "        text += delta\n"
        "end = time.monotonic()\n"
        "print(json.dumps({'status': status, 'answered': bool(text.strip()), 'content': text.strip(), "
        "'firstTokenMs': round((first - start) * 1000), 'totalMs': round((end - start) * 1000)}))\nPY")
LEFT = (f"for p in {ARCHIVE} {OLLAMA_DIR}/bin/ollama {HOME}/.config/opute/ollama-runtime.json {UNIT_FILE} "
        f"{HOME}/.ollama/models/blobs/sha256-{GGUF_SHA}; do test -e $p && echo \"kept $p\"; done; "
        f"du -sm {HOME}/.cache/opute/ollama {HOME}/.local/share/opute/ollama {HOME}/.ollama | cut -f1")


def local_llm() -> dict:
    steps = base.TOOL_STEPS + [
        {"id": "recipeCopy", "copy": "recipe", "to": RECIPE, "mode": "0644"},
        {"id": "recipeOwner", "guest": f"chown -R opute:opute {HOME}/m32 && echo owned"},
        {"id": "catalog", "mcp": "tools/list"},
        # the user manager the runtime's user unit runs under
        call("supervisor", "ensure_host_service_supervisor", {}),
        {"id": "userBus", "guest": "for i in $(seq 100); do [ -S /run/user/$(id -u opute)/bus ] && break; sleep 0.1; "
                                   "done; test -S /run/user/$(id -u opute)/bus && echo up"},
        call("supervisorAgain", "ensure_host_service_supervisor", {}),
        linger(),
        # before: no Ollama on the host
        call("prereqBefore", "check_local_llm_prerequisites", {}),
        call("validate", "validate_runtime_recipe", {"source": RECIPE}),
        # the recipe, activated only after its streaming chat check
        call("runCall", "run_runtime_recipe", {"source": RECIPE, "activate": True},
             capture={"RUN": "response.body.result.structuredContent.runId"}),
        call("run", "get_runtime_recipe_run", {"runId": "${RUN}"},
             until={"path": "response.body.result.structuredContent.status", "in": ha.DONE}, timeout=1500,
             interval=5),
        {"id": "installed", "confirms": ["runCall"], "guest": (
            f"sha256sum {ARCHIVE} | cut -c1-64; test -x {OLLAMA_DIR}/bin/ollama && echo binary; "
            f"python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))[\"binaryPath\"])' "
            f"{HOME}/.config/opute/ollama-runtime.json; "
            f"sha256sum {HOME}/.ollama/models/blobs/sha256-{GGUF_SHA} | cut -c1-64")},
        {"id": "serving", "confirms": ["runCall"], "guest": (
            f"{USER_SYSTEMCTL} is-active {UNIT}; ss -ltnH 'sport = :11434' | wc -l")},
        {"id": "chat", "guest": CHAT, "confirms": ["runCall"]},
        call("probe", "probe_openai_compatible_server",
             {"endpoint": ENDPOINT, "modelRef": MODEL, "includeChat": True}),
        call("prereqAfter", "check_local_llm_prerequisites", {}),
        call("models", "list_local_llm_models", {"runtime": "ollama"}),
        # the legacy stop leaves the shared runtime running, as recorded; the unit is stopped as a host service
        *task("stop", "stop_local_llm_runtime", {"runtime": "ollama"}),
        {"id": "stillServing", "guest": f"{USER_SYSTEMCTL} is-active {UNIT}", "confirms": ["stop"]},
        svc("stopUnit", "stop", UNIT),
        {"id": "stopped", "confirms": ["stopUnit"], "guest": (
            f"{USER_SYSTEMCTL} is-active {UNIT}; ss -ltnH 'sport = :11434' | wc -l")},
        # the legacy remove keeps the shared model, as recorded
        *task("removeModel", "remove_local_llm_model", {"modelRef": MODEL, "purge": True}),
        {"id": "left", "guest": LEFT, "confirms": ["removeModel"]},
    ]
    reqs = [
        req(GOAL, "A real model runs on the host from the published recipe",
            direct("supervisor", "isError", **{"in": [False, "<absent>"]}),
            direct("supervisorAgain", "structuredContent.status", equals="ready"),
            a("linger", "stdout", equals="yes\nbus\n"), ok("recipeOwner"),
            direct("prereqBefore", "structuredContent.ollamaBinaryPresent", equals=False),
            direct("prereqBefore", "structuredContent.blockers", equals=["ollama binary is not installed"]),
            direct("validate", "structuredContent.valid", equals=True),
            direct("runCall", "isError", **{"in": [False, "<absent>"]}),
            direct("run", "structuredContent.status", equals="completed"),
            direct("run", "structuredContent.activeRuntime.capability", equals="llm-serving"),
            direct("run", "structuredContent.activeRuntime.servingContract", equals="openai-chat.v1"),
            direct("run", "structuredContent.activeRuntime.observation.streamingChatReady", equals=True),
            a("installed", "stdout", equals=(
                "1df9d6228bac5a22e420b242708c0d10e5b886db97bab99d446a41760f4e79af\nbinary\n"
                f"{OLLAMA_DIR}/bin/ollama\n{GGUF_SHA}\n")),
            a("serving", "stdout", equals="active\n1\n"),
            a("chat", "json.status", equals=200), a("chat", "json.answered", equals=True),
            direct("probe", "structuredContent.streamingChatReady", equals=True),
            direct("prereqAfter", "structuredContent.ollamaBinaryPresent", equals=True),
            direct("prereqAfter", "structuredContent.ollamaServiceActive", equals=True),
            direct("models", "isError", **{"in": [False, "<absent>"]}),
            scenario="On a CPU-only host with no Ollama, run_runtime_recipe with activate installs Ollama v0.40.17 "
                     "from its SHA-256-pinned upstream release, configures the agent to use it, pulls "
                     "Qwen2.5-0.5B-Instruct Q4_K_M (the pulled GGUF matches its Hugging Face SHA-256), and activates "
                     "llm-serving after a streaming chat completion; a client's own chat request is answered"),
        req(SPEC, "The legacy LLM wrappers act on the shared Ollama runtime",
            res("stop", "structuredContent.stopped", equals=False),
            res("stop", "structuredContent.shared", equals=True),
            a("stillServing", "stdout", equals="active\n"),
            direct("stopUnit", "structuredContent.status", equals="applied"),
            a("stopped", "stdout", equals="inactive\n0\n"),
            res("removeModel", "structuredContent.removed", equals=False),
            res("removeModel", "structuredContent.shared", equals=True),
            a("left", "stdout", matches=f"kept {HOME}/.ollama/models/blobs/sha256-{GGUF_SHA}\n"),
            scenario="stop_local_llm_runtime leaves the shared Ollama runtime running and says so; the unit stops "
                     "through set_host_service_state and its port closes; remove_local_llm_model keeps the shared "
                     "model and says so, so the pulled model stays on disk until the user removes it"),
    ]
    cleanup = [{"id": "cleanLocalLlm", "guest": (
        f"{USER_SYSTEMCTL} disable --now {UNIT} 2>/dev/null; rm -f {UNIT_FILE}; {USER_SYSTEMCTL} daemon-reload; "
        "loginctl disable-linger opute; "
        f"rm -rf {HOME}/.ollama {HOME}/.cache/opute/ollama {HOME}/.local/share/opute/ollama {HOME}/m32 "
        f"{HOME}/.config/opute/ollama-runtime.json; "
        f"rmdir {HOME}/.cache/opute {HOME}/.cache {HOME}/.config/systemd/user/default.target.wants "
        f"{HOME}/.config/systemd/user {HOME}/.config/systemd 2>/dev/null; true")}]
    workload = f"processes//user.slice/user-{UID}.slice/user@{UID}.service/opute.slice/opute-workload.slice"
    allow = [f"files/{HOME}/m32", f"files/{HOME}/m32/local-llm.recipe.yaml",
             f"files/{HOME}/.local/share/opute/ollama*", f"files/{HOME}/.ollama*",
             f"files/{HOME}/.config/opute/ollama-runtime.json",
             f"files/{HOME}/.config/systemd", f"files/{HOME}/.config/systemd/user",
             f"files/{HOME}/.config/systemd/user/{UNIT}",
             f"files/{HOME}/.config/systemd/user/default.target.wants*",
             f"opute/units/{UNIT}", f"{workload}/{UNIT} ollama", f"{workload}/{UNIT} llama-server",
             "sockets/tcp 127.0.0.1:11434 ollama", "sockets/tcp 127.0.0.1:* llama-server"] + USER_SESSION
    optional = {p: "the user manager starts D-Bus on demand" for p in USER_SESSION if "dbus.service" in p}
    # Ollama's model runner listens on a port it picks at random
    optional["sockets/tcp 127.0.0.1:* llama-server"] = "Ollama's model runner listens on a random local port"
    return e4.journey(
        "e2e8.local-llm", "e2e-local-llm",
        "A real local LLM (m32): on a CPU-only host, the published recipe installs Ollama from its pinned upstream "
        "release, points the agent at it, pulls Qwen2.5-0.5B-Instruct Q4_K_M and activates the runtime after a "
        "streaming chat completion. Pass: the run completes and activates llm-serving, the pinned hashes match, a "
        "client's chat request is answered, and stop closes the runtime.",
        reqs, steps, cleanup, allow=allow, minutes=30, optional=optional,
        inputs={"agent": AGENT + "; Ollama v0.40.17 and the model are downloaded from upstream during the run",
                "files": FILES})


JOURNEYS = [local_llm]


def render() -> dict[str, str]:
    return {f"{j['id']}.json": json.dumps(j, indent=1) + "\n" for j in (make() for make in JOURNEYS)}


if __name__ == "__main__":
    for name, text in render().items():
        (HERE / name).write_text(text)
        print(name)
