# Artifact downloads time out on no progress, not on size

Status: **proposed (m32)**. A defect fix found by journey `e2e8.local-llm`. It
adds no tool, input, output field or default path.

## Why

`ensure_host_artifact` downloads a caller-declared HTTPS artifact and checks
its SHA-256. Up to 0.5.3 the whole request, headers and body together, had to
finish within 300 seconds, as Go's `http.Client{Timeout: 5 * time.Minute}` did.
So the largest artifact a host can fetch depends on its link speed: about 1.5
GB at 5 MB/s, 300 MB at 1 MB/s. Nothing in the tool's schema or description
says so.

The m32 local-LLM recipe downloads Ollama v0.40.17
(`ollama-linux-amd64.tar.zst`, 1,545,470,208 bytes). On the dev VM the link
gives about 5.3 MB/s: `curl` took 281 s, and the agent failed after 300 s with

    read host artifact: timed out reading response

The tool already caps the size at 4 GiB. A fixed deadline for the whole
transfer adds nothing to safety that a no-progress limit and a longer total
limit don't also give. It only refuses large artifacts on ordinary links.

## What Changes

- A download fails when no bytes arrive for 300 seconds while connecting,
  awaiting the response headers or reading the body. That is the existing
  five-minute constant, now applied to progress instead of to the whole
  transfer.
- A download also fails when it hasn't finished within one hour, so a server
  that trickles bytes can't hold a call open indefinitely. At the 4 GiB cap
  that still allows about 1.2 MB/s.
- Existing error texts are unchanged: a stall while reading the body reports
  `read host artifact: timed out reading response`, and a stall before the
  headers reports Go's `Client.Timeout exceeded while awaiting headers` cause.
  Passing the total limit reports
  `read host artifact: download did not finish within 3600 seconds`.
- The same limits apply to the cloudflared connector download
  (`hostpublic.rs`), which shares the download path.

## Impact

- `crates/host-agent/src/hostartifact.rs`: the agent's timeouts.
- No catalog, schema or wire change: `tools/list` is unchanged.
- Journey `e2e8.local-llm` downloads the 1.5 GB Ollama archive through
  `ensure_host_artifact` on the dev VM.
