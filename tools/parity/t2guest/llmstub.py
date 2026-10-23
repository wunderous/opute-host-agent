#!/usr/bin/python3
"""CPU-only model runtime stub for the M8f T2 twins (milestones.md, 8f).

Installed under the name of the program it stands in for, it behaves as
much of that program as the Host Agent's llm domain drives, without a
model, a GPU or a network:

  ollama serve          an Ollama API on $OLLAMA_HOST (default
                        127.0.0.1:11434) over a model store of JSON files in
                        $OLLAMA_MODELS (default ~/.ollama/models): /api/tags,
                        /api/ps, /api/show, /api/create, /api/pull,
                        /api/generate, /api/embed, /api/version, /v1/models
                        and a streamed /v1/chat/completions
  ollama pull MODEL     asks that API to pull MODEL, as the real CLI does
  llama-server ...      an OpenAI-compatible /v1/models naming --alias, on
                        --host:--port; `--version` prints a version
  nvidia-smi ...        one stub device and its memory use

Every answer is a pure function of the request and the store, so two twins
that drive it identically hold identical stores. The bytes below name
libggml-cuda.so so that the agent's CUDA linkage check (ldd/strings over the
managed binary) recognises a llama-server built for CUDA.

Runs on the guest's python3 with the standard library only.
"""
import hashlib
import http.server
import json
import os
import sys
import urllib.request

LINKAGE = "libggml-cuda.so libcudart.so.12"  # read by the agent's strings(1) check
VERSION = "version: 6500 (parity-cpu-stub)"


def canonical(name):
    name = (name or "").strip()
    last = name.rsplit("/", 1)[-1]
    return name if ":" in last else name + ":latest"


class Store:
    def __init__(self, root):
        self.root = root
        self.loaded = {}

    def _path(self, name):
        return os.path.join(self.root, hashlib.sha256(canonical(name).encode()).hexdigest() + ".json")

    def get(self, name):
        try:
            with open(self._path(name)) as f:
                return json.load(f)
        except OSError:
            return None

    def put(self, name, parameters=""):
        os.makedirs(self.root, exist_ok=True)
        entry = {"name": canonical(name), "digest": "sha256:" + hashlib.sha256(canonical(name).encode()).hexdigest(),
                 "size": 1024 + len(canonical(name)), "parameters": parameters}
        with open(self._path(name), "w") as f:
            json.dump(entry, f, sort_keys=True)
        return entry

    def all(self):
        out = []
        for entry in sorted(os.listdir(self.root)) if os.path.isdir(self.root) else []:
            with open(os.path.join(self.root, entry)) as f:
                out.append(json.load(f))
        return sorted(out, key=lambda e: e["name"])


def num_ctx(parameters):
    fields = parameters.split()
    for i, field in enumerate(fields[:-1]):
        if field == "num_ctx":
            return int(fields[i + 1])
    return 0


def serve(host, port, routes):
    class Handler(http.server.BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def _answer(self):
            length = int(self.headers.get("Content-Length") or 0)
            try:
                body = json.loads(self.rfile.read(length) or b"null") if length else None
            except ValueError:
                body = None
            status, payload, stream = routes(self.command, self.path.split("?", 1)[0], body)
            self.send_response(status)
            if stream is not None:
                self.send_header("Content-Type", "text/event-stream")
                self.send_header("Transfer-Encoding", "chunked")
                self.end_headers()
                for chunk in stream:
                    data = chunk.encode()
                    self.wfile.write(b"%x\r\n%s\r\n" % (len(data), data))
                self.wfile.write(b"0\r\n\r\n")
                return
            data = json.dumps(payload).encode()
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        do_GET = do_POST = _answer  # noqa: N815 (stdlib names)

        def log_message(self, *_args):
            pass

    http.server.ThreadingHTTPServer((host, port), Handler).serve_forever()


def chat_stream():
    return ['data: {"choices":[{"index":0,"delta":{"role":"assistant","content":"READY"}}]}\n\n',
            'data: {"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}\n\n', "data: [DONE]\n\n"]


def ollama_address():
    raw = os.environ.get("OLLAMA_HOST", "127.0.0.1:11434")
    host, _, port = raw.rpartition(":")
    return host or "127.0.0.1", int(port)


def ollama_serve():
    store = Store(os.environ.get("OLLAMA_MODELS") or os.path.expanduser("~/.ollama/models"))
    context = int(os.environ.get("OLLAMA_CONTEXT_LENGTH") or 4096)

    def routes(method, path, body):
        body = body if isinstance(body, dict) else {}
        model = body.get("model") or body.get("name") or ""
        if (method, path) == ("GET", "/api/version"):
            return 200, {"version": "0.0.0-parity"}, None
        if (method, path) == ("GET", "/api/tags"):
            return 200, {"models": [{k: e[k] for k in ("name", "digest", "size")} for e in store.all()]}, None
        if (method, path) == ("GET", "/api/ps"):
            return 200, {"models": [{"name": n, "size_vram": 0, "context_length": c}
                                    for n, c in sorted(store.loaded.items())]}, None
        if (method, path) == ("GET", "/v1/models"):
            return 200, {"object": "list", "data": [{"id": e["name"], "object": "model"} for e in store.all()]}, None
        if (method, path) == ("POST", "/api/pull"):
            store.put(model)
            return 200, {"status": "success"}, None
        if (method, path) in (("POST", "/api/show"), ("POST", "/api/generate"), ("POST", "/api/create"),
                              ("POST", "/api/embed"), ("POST", "/v1/chat/completions")):
            source = body.get("from") if path == "/api/create" else model
            entry = store.get(source)
            if entry is None:
                return 404, {"error": f"model '{source}' not found"}, None
            if path == "/api/show":
                return 200, {"parameters": entry["parameters"]}, None
            if path == "/api/create":
                params = " ".join(f"{k} {v}" for k, v in sorted((body.get("parameters") or {}).items()))
                store.put(model, params)
                return 200, {"status": "success"}, None
            if path == "/api/generate":
                store.loaded[entry["name"]] = num_ctx(entry["parameters"]) or context
                return 200, {"model": model, "response": ".", "done": True}, None
            if path == "/api/embed":
                return 200, {"model": model, "embeddings": [[0.0, 0.0, 0.0]]}, None
            return 200, None, chat_stream()
        return 404, {"error": "not found"}, None

    host, port = ollama_address()
    serve(host, port, routes)


def ollama_pull(model):
    host, port = ollama_address()
    request = urllib.request.Request(f"http://{host}:{port}/api/pull", data=json.dumps({"model": model}).encode(),
                                     headers={"Content-Type": "application/json"})
    try:
        urllib.request.urlopen(request, timeout=30).read()
    except OSError as err:
        print(f"Error: {err}", file=sys.stderr)
        return 1
    print("pulling manifest\nsuccess")
    return 0


def llama_server(args):
    if "--version" in args:
        print(VERSION, file=sys.stderr)
        return 0
    option = {args[i]: args[i + 1] for i in range(len(args) - 1) if args[i].startswith("--")}
    alias = option.get("--alias") or os.path.basename(option.get("--model", "model"))

    def routes(method, path, body):
        if (method, path) == ("GET", "/v1/models"):
            return 200, {"object": "list", "data": [{"id": alias, "object": "model", "owned_by": "llamacpp"}]}, None
        if (method, path) == ("GET", "/health"):
            return 200, {"status": "ok"}, None
        if (method, path) == ("POST", "/v1/chat/completions"):
            return 200, None, chat_stream()
        return 404, {"error": "not found"}, None

    serve(option.get("--host", "127.0.0.1"), int(option.get("--port", "8080")), routes)
    return 0


def nvidia_smi(args):
    if any(a.startswith("--query-gpu=memory.used") for a in args):
        print("1536")
    elif any(a.startswith("--query-gpu=name") for a in args):
        print("Parity CPU stub, 8192")
    else:
        print("GPU 0: Parity CPU stub (UUID: GPU-00000000-0000-0000-0000-000000000000)")
    return 0


def main():
    program, args = os.path.basename(sys.argv[0]), sys.argv[1:]
    if program == "ollama":
        if args[:1] == ["serve"]:
            ollama_serve()
            return 0
        if args[:1] == ["pull"] and len(args) == 2:
            return ollama_pull(args[1])
        print(f"Error: unsupported ollama command {args}", file=sys.stderr)
        return 1
    if program == "llama-server":
        return llama_server(args)
    if program == "nvidia-smi":
        return nvidia_smi(args)
    print(f"llmstub: unknown program {program}", file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main())
