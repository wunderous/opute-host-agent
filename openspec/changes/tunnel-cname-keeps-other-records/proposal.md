# A tunnel's CNAME keeps the hostname's other records

Status: **proposed (m34)**. A defect fix found while moving www.opute.io onto
the dev machine's K3s. It adds no tool, input, output field or default.

## Why

`ensure_cloudflared_tunnel` on the dedicated path points each hostname at its
tunnel with a proxied CNAME. Before it creates the CNAME, it deleted **every**
other record at that name. At a zone apex that includes the MX and TXT
records of the zone's mail: routing `opute.io` through a tunnel would have
deleted Cloudflare Email Routing's three MX records and its SPF record. Only
A, AAAA and CNAME records stand where the tunnel's CNAME goes.

## What Changes

- The CNAME upsert deletes only the hostname's A, AAAA and CNAME records that
  don't already point at the tunnel. Every other record (MX, TXT, CAA, SRV
  and the rest) stays.
- If Cloudflare refuses the CNAME beside one of the kept records, the call
  fails with Cloudflare's error and nothing else is deleted.
- Teardown (`unpublish`) is unchanged: it already deletes only the
  hostname's CNAME record.

## Impact

- `crates/host-agent/src/providers/cloudflare/api.rs`: the CNAME upsert.
- Effect only: `tools/list` and the error codes are unchanged.
