# A typed stable endpoint for an HA K3s cluster (proposed)

Status: proposed. Owner question Q19
([rust-spec-authority/questions.md](../rust-spec-authority/questions.md)).
Nothing here is implemented.

## Why

e2e-3a journey 1 (`e2e3a.build`, run `e2e-3a-r5`) calls
`opute.capability.kubernetes.ensure-ha-endpoint`, as the goal asks:

- before the first server exists, the call is refused with the typed
  `resource_binding` error for the unknown cluster URI. That is correct;
- after three servers have joined, the only endpoint the VM can offer is
  one server's own K3s API. The probe fails closed with `UnknownIssuer`,
  because the K3s CA is not in the host's certificate roots.

`ensure-ha-endpoint` checks a caller's stable endpoint; it creates nothing.
No typed operation provides an address in front of the servers that
survives the loss of one of them (a load balancer, a VIP, or a registration
address with a host-trusted certificate). The page's test record also
reported "no external HA endpoint". Today a client that loses the server it
addresses has to choose another server itself.

## What Changes

If the owner accepts Q19, a typed operation provides a stable endpoint for
an embedded-etcd K3s cluster. One option is the provider configuring a
registration address in each server's TLS SANs, with its trust material
returned by reference, so `ensure-ha-endpoint` can verify it. Which design,
and whether it belongs in the provider or in a network capability, is the
owner's decision.

## Impact

- The K3s provider (`crates/host-agent/src/providers/k3s/`) and the catalog;
  a new capability needs its own catalog revision.
- Rerun e2e3a.build with `ensure-ha-endpoint` passing against that endpoint.
