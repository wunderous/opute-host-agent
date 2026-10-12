#!/usr/bin/env python3
"""Generate scenarios/host-domain.json: the M8a host-domain T1 matrix.

Run from tools/parity: python3 host_domain_scenarios.py scenarios/host-domain.json

Each family gets success, typed-failure and partial-failure cases. Every case
collects the sandbox home tree (type, mode, sha256, symlink target) and the
shim trace, so the external effect is compared, not only the answer.
System-scope writes are never attempted here: T1 runs as an unprivileged
user on the developer host, so system paths are exercised only up to
validation. Writes to /etc run in the T2 sandbox.
"""
import json, sys

SERVE = {"as": "start", "serve": {"argv": ["serve", "--mode=standalone"], "profile": "standalone", "env": {
    "OPUTE_STANDALONE_ALLOW_MUTATIONS": "true",
    "OPUTE_HOST_RESOURCE_ENFORCEMENT": "advisory",
    "OPUTE_HOST_MAX_NORMAL_OPERATIONS": "64", "OPUTE_HOST_MAX_HEAVY_OPERATIONS": "64",
    "OPUTE_HOST_RESOURCE_CPU_CAPACITY_CORES": "64",
    "OPUTE_HOST_RESOURCE_MEMORY_CAPACITY_BYTES": "68719476736"}}}
STOP = {"as": "stop", "stop": {}}


def serve(**env):
    s = json.loads(json.dumps(SERVE))
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
        "until": {"path": ["body", "result", "status"], "in": ["completed", "failed", "cancelled"]}, "timeout": 20}}
    # A completed task's tasks/get carries the tool result inline.
    return [step, done]


def flat(steps):
    out = []
    for step in steps:
        out.extend(step if isinstance(step, list) else [step])
    return out


TS = "wall-clock timestamp of this side's run"
# A task handle's own timestamps, as tasks/get and the task-aware call
# report them.
MASKS = [{"path": ["steps", "*", "body", "result", k], "type": "rfc3339", "reason": TS, "optional": True}
         for k in ("createdAt", "lastUpdatedAt")] + [
    {"path": ["steps", "stub", "port"], "type": "non-negative-int", "optional": True,
     "reason": "the stub listens on a freshly allocated ephemeral port per side"}]


def scenario(id_, steps, anchors, collect=("homeTree", "trace")):
    return {"id": "host-domain." + id_, "surface": "host-domain", "owner": "host",
            "anchors": anchors, "fixture": None, "collect": list(collect),
            "steps": flat(steps), "compare": {"masks": MASKS}}


FILE = ["internal/domain/host/file.go:EnsureHostFile", "internal/domain/host/file.go:RemoveHostFile",
        "internal/tools/dispatch_host.go"]
CONF = "~/.config/parity/app.conf"
SHA_A = "sha256:" + __import__("hashlib").sha256(b"alpha\n").hexdigest()

cases = [
    # Create, keep, change mode, change content, read back, then remove with
    # a wrong hash, without confirmation, with an upper-case hash, and again.
    scenario("files.lifecycle", [
        serve(),
        call("create", "ensure_host_file", {"path": CONF, "content": "alpha\n"}),
        call("unchanged", "ensure_host_file", {"path": CONF, "content": "alpha\n", "mode": 384}),
        call("modeChange", "ensure_host_file", {"path": CONF, "content": "alpha\n", "mode": 420}),
        call("contentChange", "ensure_host_file", {"path": CONF, "content": "beta\n", "mode": 420}),
        call("back", "ensure_host_file", {"path": CONF, "content": "alpha\n", "mode": 420}),
        call("inspect", "inspect_host_file", {"path": CONF, "expectedSha256": SHA_A}),
        task_call("wrongHash", "remove_host_file", {"path": CONF, "confirm": True, "expectedSha256": "sha256:" + "0" * 64}),
        task_call("unconfirmed", "remove_host_file", {"path": CONF}),
        task_call("remove", "remove_host_file", {"path": CONF, "confirm": True,
                                           "expectedSha256": SHA_A.upper().replace("SHA256:", "")}),
        task_call("again", "remove_host_file", {"path": CONF, "confirm": True}),
        STOP], FILE),
    # Relative, home-rooted and absolute forms, the executable bit, and
    # untrimmed content written raw.
    scenario("files.path-forms", [
        serve(),
        call("relative", "ensure_host_file", {"path": "  bin/run.sh ", "content": " #!/bin/sh\n", "mode": 493}),
        call("homeRoot", "ensure_host_file", {"path": "~/top.txt", "content": "", "scope": " USER "}),
        call("absolute", "ensure_host_file", {"path": "${SANDBOX}/home/abs/x.txt", "content": "x"}),
        call("inspectExec", "inspect_host_file", {"path": "bin/run.sh"}),
        task_call("removeRelative", "remove_host_file", {"path": "bin/run.sh", "confirm": True}),
        STOP], FILE),
    # Typed refusals: every one must leave the home tree unchanged.
    scenario("files.refusals", [
        serve(),
        {"as": "outside", "mkdir": {"path": "${SANDBOX}/outside"}},
        {"as": "link", "symlink": {"path": "${SANDBOX}/home/linked", "target": "${SANDBOX}/outside"}},
        {"as": "dir", "mkdir": {"path": "${SANDBOX}/home/adir"}},
        call("scope", "ensure_host_file", {"path": "x", "content": "x", "scope": "bogus"}),
        call("empty", "ensure_host_file", {"path": "  ", "content": "x"}),
        call("escape", "ensure_host_file", {"path": "../escape.txt", "content": "x"}),
        call("absoluteOutside", "ensure_host_file", {"path": "/etc/hostname", "content": "x"}),
        call("throughLink", "ensure_host_file", {"path": "linked/x.txt", "content": "x"}),
        task_call("linkItself", "remove_host_file", {"path": "linked", "confirm": True}),
        call("modeLow", "ensure_host_file", {"path": "m.txt", "content": "x", "mode": 256}),
        call("modeHigh", "ensure_host_file", {"path": "m.txt", "content": "x", "mode": 511}),
        call("modeNegative", "ensure_host_file", {"path": "m.txt", "content": "x", "mode": -1}),
        call("modeFraction", "ensure_host_file", {"path": "m.txt", "content": "x", "mode": 420.9}),
        call("modeString", "ensure_host_file", {"path": "s.txt", "content": "x", "mode": "644"}),
        call("modeTypeBits", "ensure_host_file", {"path": "t.txt", "content": "x", "mode": 33188}),
        call("isDirectory", "ensure_host_file", {"path": "adir", "content": "x"}),
        task_call("removeDirectory", "remove_host_file", {"path": "adir", "confirm": True}),
        call("systemNotUnit", "ensure_host_file", {"path": "/etc/systemd/system/x.conf", "content": "x", "scope": "system"}),
        call("systemOutside", "ensure_host_file", {"path": "/etc/x.service", "content": "x", "scope": "system"}),
        task_call("systemBadName", "remove_host_file", {"path": "/etc/systemd/system/a b.service", "confirm": True, "scope": "system"}),
        task_call("systemMissing", "remove_host_file", {"path": "/etc/systemd/system/opute-parity-absent.service",
                                                  "confirm": True, "scope": "system"}),
        STOP], FILE),
    # A configured shared-host owner that is not this instance refuses every
    # mutation before it looks at its arguments; reads still answer.
    scenario("files.shared-owner-refused", [
        serve(OPUTE_SHARED_HOST_OWNER_INSTANCE="parity-other-instance"),
        call("ensure", "ensure_host_file", {"path": "x.txt", "content": "x"}),
        task_call("remove", "remove_host_file", {"path": "x.txt"}),
        call("inspect", "inspect_host_file", {"path": "x.txt"}),
        STOP], FILE + ["internal/hostruntime/shared.go:RequireSharedHostOwner"]),
    scenario("files.shared-owner-matches", [
        serve(OPUTE_HOST_AGENT_INSTANCE="parity-owner", OPUTE_SHARED_HOST_OWNER_INSTANCE=" parity-owner "),
        call("ensure", "ensure_host_file", {"path": "x.txt", "content": "x"}),
        task_call("remove", "remove_host_file", {"path": "x.txt", "confirm": True}),
        STOP], FILE + ["internal/hostruntime/shared.go:RequireSharedHostOwner"]),
]


# ---- host archives -------------------------------------------------------
import base64, io, tarfile
from compression import zstd

ARCHIVE = ["internal/domain/host/archive.go:ExtractHostArchive", "internal/tools/dispatch_host.go"]


def ti(name, kind=tarfile.REGTYPE, data=b"", mode=0o644, link=""):
    info = tarfile.TarInfo(name)
    info.type, info.mode, info.linkname = kind, mode, link
    info.size = len(data) if kind in (tarfile.REGTYPE, tarfile.AREGTYPE) else 0
    info.mtime, info.uid, info.gid, info.uname, info.gname = 0, 0, 0, "", ""
    return info, data


def tar(entries, fmt=tarfile.PAX_FORMAT):
    out = io.BytesIO()
    with tarfile.open(fileobj=out, mode="w", format=fmt) as t:
        for info, data in entries:
            t.addfile(info, io.BytesIO(data) if data else None)
    return out.getvalue()


def header(name, flag, size=0, mode=0o644, link=""):
    """One raw USTAR header block, for entries tarfile will not write."""
    blk = bytearray(512)
    def put(off, width, value):
        blk[off:off + len(value)] = value
    put(0, 100, name.encode())
    put(100, 8, b"%07o\0" % mode)
    put(108, 8, b"0000000\0"); put(116, 8, b"0000000\0")
    put(124, 12, b"%011o\0" % size); put(136, 12, b"00000000000\0")
    put(156, 1, flag); put(157, 100, link.encode())
    put(257, 6, b"ustar\0"); put(263, 2, b"00")
    blk[148:156] = b" " * 8
    blk[148:156] = b"%06o\0 " % sum(blk)
    return bytes(blk)


def pax(records):
    """PAX extended-header records, each prefixed with its own length."""
    out = b""
    for key, value in records:
        body = b" %s=%s\n" % (key.encode(), value.encode())
        n = len(body) + 1
        while len(str(n)) + len(body) != n:
            n += 1
        out += str(n).encode() + body
    return out


def raw_tar(blocks):
    return b"".join(blocks) + b"\0" * 1024


def pad(data):
    return data + b"\0" * (-len(data) % 512)


def flip(data, at):
    out = bytearray(data)
    out[at] ^= 0x5A
    return bytes(out)


def zst(data, checksum=False):
    options = {zstd.CompressionParameter.compression_level: 3,
               zstd.CompressionParameter.checksum_flag: 1 if checksum else 0}
    return zstd.compress(data, options=options)


BASIC = tar([
    ti("app/", tarfile.DIRTYPE, mode=0o755),
    ti("app/bin/run", data=b"#!/bin/sh\necho ok\n", mode=0o755),
    ti("app/README", data=b"readme\n"),
    ti("app/latest", tarfile.SYMTYPE, link="bin/run"),
    ti("app/suid", data=b"s", mode=0o4755),
    ti("data/deep/x.txt", data=b"x" * 700),
])
LONG = "long/" + "d" * 60 + "/" + "e" * 60 + "/file.txt"
corrupt_crc = bytearray(zst(BASIC, checksum=True)); corrupt_crc[-1] ^= 0xFF
bad_checksum = bytearray(BASIC); bad_checksum[148:156] = b"0000001\0"
truncated_tar = BASIC[:BASIC.index(b"x" * 700) + 300]
SPARSE_PAX = pax([("GNU.sparse.major", "1"), ("GNU.sparse.minor", "0"),
                  ("GNU.sparse.name", "app/sparse.bin"), ("GNU.sparse.realsize", "16")])
# A GNU PAX 1.0 sparse file: 5 data bytes at offset 0 and 3 at 10, real size 16.
SPARSE_DATA = pad(b"2\n0\n5\n10\n3\n") + b"AAAAABBB"
sparse_pax = (header("././@PaxHeader", b"x", size=len(SPARSE_PAX)) + pad(SPARSE_PAX)
              + header("app/GNUSparseFile.0/sparse.bin", b"0", size=len(SPARSE_DATA)) + pad(SPARSE_DATA))
GLOBAL = pax([("comment", "hi")])

ARCHIVES = {
    "basic": zst(BASIC),
    "gnu-long": zst(tar([ti(LONG, data=b"long\n")], tarfile.GNU_FORMAT)),
    "pax-long": zst(tar([ti(LONG, data=b"long\n"), ti("app/" + "l" * 120, tarfile.SYMTYPE, link="../" + LONG)])),
    "legacy-rega": zst(raw_tar([header("old/", b"\0", mode=0o755), header("old/f", b"\0", size=3) + pad(b"abc")])),
    "sparse": zst(raw_tar([sparse_pax])),
    "empty-tar": zst(b"\0" * 1024),
    "empty-frame": zst(b""),
    "zero-bytes": b"",
    "not-zstd": b"plain text, not a zstd frame\n",
    "zstd-garbage": zst(bytes(range(256)) * 3),
    "truncated-zstd": zst(BASIC)[: len(zst(BASIC)) // 2],
    "truncated-tar": zst(truncated_tar),
    "crc-corrupt": bytes(corrupt_crc),
    "bad-header-checksum": zst(bytes(bad_checksum)),
    "partial-magic": zst(BASIC)[:2],
    "header-only": zst(BASIC)[:5],
    "bitflip-early": flip(zst(BASIC), 20),
    "bitflip-middle": flip(zst(BASIC), len(zst(BASIC)) // 2),
    "bitflip-late": flip(zst(BASIC), len(zst(BASIC)) - 12),
    "two-frames": zst(BASIC[:2048]) + zst(BASIC[2048:]),
    "skippable-first": (0x184D2A50).to_bytes(4, "little") + (3).to_bytes(4, "little") + b"abc" + zst(BASIC),
    "skippable-truncated": (0x184D2A50).to_bytes(4, "little") + (30).to_bytes(4, "little") + b"abc",
    "trailing-garbage": zst(BASIC) + b"garbage after the frame",
    "zero-then-header": zst(b"\0" * 512 + BASIC),
    "dotdot": zst(tar([ti("ok.txt", data=b"ok"), ti("../evil.txt", data=b"e")])),
    "absolute": zst(raw_tar([header("/abs.txt", b"0")])),
    "inner-dotdot": zst(tar([ti("a/../../evil.txt", data=b"e")])),
    "backslash": zst(tar([ti("a\\b.txt", data=b"e")])),
    "dot": zst(raw_tar([header(".", b"5", mode=0o755)])),
    "link-escape": zst(tar([ti("l", tarfile.SYMTYPE, link="../../x")])),
    "link-absolute": zst(tar([ti("l", tarfile.SYMTYPE, link="/etc/passwd")])),
    "link-dotdot-inside": zst(tar([ti("a/l", tarfile.SYMTYPE, link="../b")])),
    "hardlink": zst(tar([ti("f", data=b"f"), ti("h", tarfile.LNKTYPE, link="f")])),
    "fifo": zst(tar([ti("p", tarfile.FIFOTYPE)])),
    "chardev": zst(raw_tar([header("c", b"3")])),
    "global-pax": zst(raw_tar([header("pax_global_header", b"g", size=len(GLOBAL)) + pad(GLOBAL)])),
    "overwrite": zst(tar([ti("app/README", data=b"replaced\n", mode=0o600), ti("app/latest", tarfile.SYMTYPE, link="README")])),
    "symlink-over-file": zst(tar([ti("app/README", tarfile.SYMTYPE, link="bin/run")])),
    "file-over-symlink": zst(tar([ti("app/latest", data=b"f")])),
    "through-symlink": zst(tar([ti("via/x.txt", data=b"x")])),
}


def archive_write(name):
    return {"as": "put-" + name, "write": {"path": "${SANDBOX}/home/in/%s.tar.zst" % name,
                                           "base64": base64.b64encode(ARCHIVES[name]).decode()}}


def extract(as_, archive, destination="out", **extra):
    return call(as_, "extract_host_archive", {"archivePath": "in/%s.tar.zst" % archive,
                                               "destination": destination, **extra})


def corpus(id_, names, before=()):
    """Each archive extracts into its own fresh destination."""
    steps = [serve(), *before]
    for name in names:
        steps += [archive_write(name), extract(name, name, "out/" + name)]
    return scenario(id_, steps + [STOP], ARCHIVE)


CORRUPT = {
    "bitflip-early": ("corrupt input: last value not power of two",
                      "zstd: Failed to parse block header: DecompressLiteralsError(HuffmanTableError("
                      "LeftoverIsNotAPowerOf2 { got: 3 }))"),
    "bitflip-late": ("unexpected literal count, want 12 bytes, but only 10 is available",
                     "zstd: Failed to parse block header: DecodeSequenceError(NotEnoughBytesForNumSequences)"),
}


def corrupt_blocks():
    case = corpus("archives.corrupt-blocks", list(CORRUPT))
    case["compare"]["divergences"] = ["D20.corrupt-zstd-block"]
    case["expectations"] = [
        {"implementations": [side], "path": ["steps", name, "body", "result", "content", 0, "text"],
         "equals": "Error: extract host archive: " + texts[i]}
        for name, texts in CORRUPT.items() for i, side in enumerate(("go", "rust"))]
    return case


cases += [
    corpus("archives.valid", ["basic", "gnu-long", "pax-long", "legacy-rega", "sparse"]),
    corpus("archives.stream-faults", ["empty-tar", "empty-frame", "zero-bytes", "not-zstd", "zstd-garbage",
                                      "truncated-zstd", "truncated-tar", "crc-corrupt", "bad-header-checksum",
                                      "zero-then-header", "partial-magic", "header-only", "bitflip-middle",
                                      "two-frames", "skippable-first",
                                      "skippable-truncated", "trailing-garbage"]),
    corpus("archives.unsafe-entries", ["dotdot", "absolute", "inner-dotdot", "backslash", "dot", "link-escape",
                                       "link-absolute", "link-dotdot-inside", "hardlink", "fifo", "chardev",
                                       "global-pax"]),
    # D20: a corrupt compressed block fails with each decoder's own message.
    # The failure, the prefix and the tree are compared; expectations pin
    # each side's exact text.
    corrupt_blocks(),
    # Re-extraction over an earlier tree: overwrite keeps the old mode, a
    # symlink replaces a symlink, and existing symlinks are never followed.
    scenario("archives.over-existing", [
        serve(), archive_write("basic"), archive_write("overwrite"), archive_write("symlink-over-file"),
        archive_write("file-over-symlink"), archive_write("through-symlink"),
        {"as": "outside", "mkdir": {"path": "${SANDBOX}/outside"}},
        extract("first", "basic"), extract("again", "basic"), extract("overwrite", "overwrite"),
        extract("symlinkOverFile", "symlink-over-file"), extract("fileOverSymlink", "file-over-symlink"),
        {"as": "viaLink", "symlink": {"path": "${SANDBOX}/home/out/via", "target": "${SANDBOX}/outside"}},
        extract("throughSymlink", "through-symlink"),
        STOP], ARCHIVE),
    # Tool-level refusals before any entry is read.
    scenario("archives.arguments", [
        serve(), archive_write("basic"),
        {"as": "dirArchive", "mkdir": {"path": "${SANDBOX}/home/in/dir.tar.zst"}},
        {"as": "fileDest", "write": {"path": "${SANDBOX}/home/plain", "content": "x"}},
        {"as": "outside", "mkdir": {"path": "${SANDBOX}/outside"}},
        {"as": "destLink", "symlink": {"path": "${SANDBOX}/home/linkdest", "target": "${SANDBOX}/outside"}},
        extract("zip", "basic", format="zip"),
        extract("formatCase", "basic", "fmt", format=" TAR.ZST "),
        call("missing", "extract_host_archive", {"archivePath": "in/none.tar.zst", "destination": "out"}),
        call("dirArchive", "extract_host_archive", {"archivePath": "in/dir.tar.zst", "destination": "out"}),
        call("archiveOutside", "extract_host_archive", {"archivePath": "/etc/hostname", "destination": "out"}),
        extract("destOutside", "basic", "../escape"),
        extract("destIsFile", "basic", "plain/sub"),
        extract("destThroughLink", "basic", "linkdest/x"),
        call("noArchive", "extract_host_archive", {"destination": "out"}),
        extract("noDestination", "basic", ""),
        STOP], ARCHIVE),
    scenario("archives.shared-owner-refused", [
        serve(OPUTE_SHARED_HOST_OWNER_INSTANCE="parity-other-instance"), archive_write("basic"),
        extract("extract", "basic"), STOP], ARCHIVE + ["internal/hostruntime/shared.go:RequireSharedHostOwner"]),
]


# ---- host artifacts -------------------------------------------------------
import hashlib as _hashlib

ARTIFACT = ["internal/domain/host/artifact.go:EnsureHostArtifact", "internal/tools/dispatch_host.go"]
PAYLOAD = b"#!/bin/sh\necho artifact\n"
PAYLOAD_SHA = _hashlib.sha256(PAYLOAD).hexdigest()
OK = {"status": 200, "base64": base64.b64encode(PAYLOAD).decode()}
URL = "https://127.0.0.1:${STUB_PORT}"


def stub(responses, default=OK):
    return {"as": "stub", "httpStub": {"tls": True, "responses": responses, "default": default}}


def trusted(**env):
    return serve(SSL_CERT_FILE="${SANDBOX}/tls/ca.pem", **env)


def artifact(as_, path="/a.bin", dest="bin/a.bin", sha=PAYLOAD_SHA, **extra):
    return task_call(as_, "ensure_host_artifact", {"uri": URL + path, "destination": dest, "sha256": sha, **extra})


cases += [
    scenario("artifacts.download", [
        stub([]), trusted(),
        artifact("create", executable=True),
        artifact("reuse", executable=True),
        artifact("plain", dest="lib/b.bin", sha="sha256:" + PAYLOAD_SHA.upper()),
        artifact("makeExecutable", dest="lib/b.bin", executable=True),
        artifact("keepExecutable", dest="lib/b.bin"),
        {"as": "hits", "httpRequests": {"stub": "stub"}},
        STOP], ARTIFACT),
    scenario("artifacts.faults", [
        stub([
            OK,                                                     # mismatch: downloaded, then refused
            {"status": 404, "body": "missing"},
            {"status": 302, "headers": {"Location": URL + "/final"}}, OK,
            {"status": 302, "headers": {"Location": "http://127.0.0.1:${STUB_PORT}/plain"}},
            {"status": 200, "contentLength": 5 * 1024 ** 3, "body": "x"},
            {"status": 200, "contentLength": 100, "body": "short"},
            {"status": 204},
        ]), trusted(),
        {"as": "existing", "write": {"path": "${SANDBOX}/home/keep/a.bin", "content": "old"}},
        artifact("mismatch", dest="keep/a.bin", sha="0" * 64),
        artifact("notFound", path="/missing"),
        artifact("redirected", path="/redirect", dest="r/a.bin"),
        artifact("toPlainHttp", path="/downgrade", dest="d/a.bin"),
        artifact("tooLarge", path="/large", dest="l/a.bin"),
        artifact("shortBody", path="/short", dest="s/a.bin"),
        artifact("noContent", path="/empty", dest="e/a.bin"),
        {"as": "hits", "httpRequests": {"stub": "stub"}},
        STOP], ARTIFACT),
    scenario("artifacts.arguments", [
        stub([]), trusted(),
        task_call("plainHttp", "ensure_host_artifact", {"uri": "http://127.0.0.1:${STUB_PORT}/a", "destination": "x", "sha256": PAYLOAD_SHA}),
        task_call("credentials", "ensure_host_artifact", {"uri": "https://u:p@127.0.0.1:${STUB_PORT}/a", "destination": "x", "sha256": PAYLOAD_SHA}),
        task_call("noHost", "ensure_host_artifact", {"uri": "https:///a", "destination": "x", "sha256": PAYLOAD_SHA}),
        task_call("relativeUri", "ensure_host_artifact", {"uri": "/a", "destination": "x", "sha256": PAYLOAD_SHA}),
        task_call("shortSha", "ensure_host_artifact", {"uri": URL + "/a", "destination": "x", "sha256": "abc"}),
        task_call("badSha", "ensure_host_artifact", {"uri": URL + "/a", "destination": "x", "sha256": "sha256:" + "g" * 64}),
        task_call("outside", "ensure_host_artifact", {"uri": URL + "/a", "destination": "../x", "sha256": PAYLOAD_SHA}),
        {"as": "hits", "httpRequests": {"stub": "stub"}},
        STOP], ARTIFACT),
    # Without SSL_CERT_FILE the test CA is not trusted.
    scenario("artifacts.untrusted", [stub([]), serve(), artifact("untrusted"), STOP], ARTIFACT),
    scenario("artifacts.refused", [
        serve(), task_call("refused", "ensure_host_artifact",
                           {"uri": "https://127.0.0.1:1/a.bin", "destination": "x", "sha256": PAYLOAD_SHA}),
        STOP], ARTIFACT),
    scenario("artifacts.shared-owner-refused", [
        stub([]), trusted(OPUTE_SHARED_HOST_OWNER_INSTANCE="parity-other-instance"), artifact("refused"),
        {"as": "hits", "httpRequests": {"stub": "stub"}}, STOP],
        ARTIFACT + ["internal/hostruntime/shared.go:RequireSharedHostOwner"]),
]

# Host tools. PATH is the shim directory alone, so lookups, sudo, apt-get and
# the installer shells are the recorded shims; no real package manager runs.
# prepare_host_agent_artifacts is exercised only up to the step before its
# systemd workload unit (an absolute /usr/bin/systemd-run): the build itself
# is a T2 case.
TOOL = ["internal/domain/host/tool.go:EnsureHostTool", "internal/exec/process.go:RunPrivilegedPackage",
        "internal/tools/dispatch_host.go"]
PREPARE = ["internal/domain/host/artifacts_agent.go:PrepareHostAgentArtifacts",
           "internal/tools/dispatch_host.go"]


def shims_only(**env):
    return serve(PATH="${SANDBOX}/shims", **env)


def tool(as_, name):
    return task_call(as_, "ensure_host_tool", {"tool": name})


def with_fixture(case, fixture):
    case["fixture"] = fixture
    return case


SRC = "${SANDBOX}/home/src"
cases += [
    with_fixture(scenario("tools.package-manager", [
        shims_only(),
        tool("present", "gcc"),
        tool("installed", "cmake"),
        tool("again", "cmake"),
        tool("vanished", "ninja"),
        tool("failed", "podman"),
        tool("silent", "buildah"),
        tool("unknown", "rustc"),
        tool("empty", ""),
        STOP], TOOL), "host-tools"),
    with_fixture(scenario("tools.user-local", [
        shims_only(),
        tool("bun", "bun"),
        tool("bunAgain", "bun"),
        tool("helm", "helm"),
        STOP], TOOL), "host-tools"),
    with_fixture(scenario("tools.update-fails", [
        shims_only(),
        tool("update", "cmake"),
        tool("bunMissing", "bun"),
        tool("helmPath", "helm"),
        STOP], TOOL), "host-tools-update-fails"),
    with_fixture(scenario("tools.no-sudo", [shims_only(), tool("noSudo", "gcc"), STOP], TOOL),
                 "host-tools-nosudo"),
    scenario("tools.no-package-manager", [
        shims_only(), tool("noApt", "go"), tool("noBash", "bun"), STOP], TOOL),
    scenario("tools.shared-owner-refused", [
        shims_only(OPUTE_SHARED_HOST_OWNER_INSTANCE="parity-other-instance"), tool("refused", "gcc"), STOP],
        TOOL + ["internal/hostruntime/shared.go:RequireSharedHostOwner"]),
    scenario("prepare.arguments", [
        shims_only(),
        {"as": "src", "mkdir": {"path": SRC}},
        {"as": "file", "write": {"path": "${SANDBOX}/home/plain", "content": "x"}},
        task_call("noSource", "prepare_host_agent_artifacts", {"sourceDir": " ", "destDir": "d"}),
        task_call("noDest", "prepare_host_agent_artifacts", {"sourceDir": SRC, "destDir": ""}),
        task_call("missing", "prepare_host_agent_artifacts", {"sourceDir": SRC + "/none", "destDir": SRC + "/out"}),
        task_call("notDir", "prepare_host_agent_artifacts", {"sourceDir": "${SANDBOX}/home/plain", "destDir": SRC}),
        task_call("outside", "prepare_host_agent_artifacts", {"sourceDir": SRC, "destDir": "${SANDBOX}/elsewhere"}),
        task_call("noGo", "prepare_host_agent_artifacts", {"sourceDir": SRC, "destDir": SRC + "/../src/out"}),
        task_call("buildDir", "prepare_host_agent_artifacts",
                  {"sourceDir": SRC, "destDir": "${SANDBOX}/home/.opute-host-agent-build"}),
        task_call("homeRoot", "prepare_host_agent_artifacts",
                  {"sourceDir": SRC, "destDir": "${SANDBOX}/home/.opute/host-agent-build/x"}),
        STOP], PREPARE),
    with_fixture(scenario("prepare.unsupported-arch", [
        shims_only(),
        {"as": "src", "mkdir": {"path": SRC}},
        task_call("arch", "prepare_host_agent_artifacts",
                  {"sourceDir": SRC, "destDir": SRC + "/out", "archs": ["riscv64"]}),
        task_call("mixedArchs", "prepare_host_agent_artifacts",
                  {"sourceDir": SRC, "destDir": SRC + "/out2", "archs": [7, "x86"]}),
        STOP], PREPARE), "host-tools"),
    scenario("prepare.shared-owner-refused", [
        shims_only(OPUTE_SHARED_HOST_OWNER_INSTANCE="parity-other-instance"),
        {"as": "src", "mkdir": {"path": SRC}},
        task_call("refused", "prepare_host_agent_artifacts", {"sourceDir": SRC, "destDir": SRC + "/out"}),
        STOP], PREPARE + ["internal/hostruntime/shared.go:RequireSharedHostOwner"]),
]

if __name__ == "__main__":
    out = sys.argv[1] if len(sys.argv) > 1 else "scenarios/host-domain.json"
    with open(out, "w") as f:
        json.dump(cases, f, indent=1)
        f.write("\n")
    print(f"wrote {len(cases)} scenarios to {out}")
