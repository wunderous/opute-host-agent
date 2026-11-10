## ADDED Requirements

### Requirement: Core HTTP clients honour the proxy environment as Go's default transport does
The Host Agent's HTTP probe, MCP probe, bridge health check, host artifact
download, remote recipe source fetch and local LLM clients SHALL send a
request for a non-loopback host through the proxy that `HTTP_PROXY`,
`HTTPS_PROXY` and `NO_PROXY` (or their lower-case forms) select, and SHALL
never proxy `localhost` or a loopback address. The OAuth client metadata
fetch and the HTTP probe's IPv4 retry SHALL NOT use a proxy.

#### Scenario: A probe behind a proxy
- **WHEN** `HTTP_PROXY` names a proxy and a client probes `http://proxy-probe.invalid/health`
- **THEN** the request goes to the proxy as `GET http://proxy-probe.invalid/health`

#### Scenario: A loopback endpoint
- **WHEN** `HTTP_PROXY` is set and a client probes `http://127.0.0.1:<port>/`
- **THEN** the request goes directly to the endpoint

#### Scenario: A NO_PROXY name
- **WHEN** `NO_PROXY=example.com` and a client requests `example.com` or `a.example.com`
- **THEN** the request goes directly, and a request for `notexample.com` goes through the proxy

#### Scenario: A NO_PROXY subdomain pattern
- **WHEN** `NO_PROXY` is `.example.com` or `*.example.com`
- **THEN** a request for `a.example.com` goes directly, and a request for `example.com` itself goes through the proxy

#### Scenario: A NO_PROXY address
- **WHEN** `NO_PROXY` lists an IP address, an `[IPv6]:port` or a CIDR block
- **THEN** a request to an IP host goes directly only when its address equals the entry's or is inside the block and any entry port is the request's, and a name entry never matches an IP host

#### Scenario: A NO_PROXY port
- **WHEN** `NO_PROXY=example.com:8080`
- **THEN** only a request to port 8080 of that name goes directly

