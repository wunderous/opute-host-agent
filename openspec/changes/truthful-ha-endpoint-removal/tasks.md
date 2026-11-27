## 1. Tests first

- [x] 1.1 `removing_an_unknown_ha_endpoint_names_no_target` fails against 0.5.2 (`endpointRef` absent, `targetUri` the placeholder).

## 2. Fix

- [x] 2.1 `ha_endpoint_removed` takes an optional target; the fake and live backends pass none for an unknown reference.

## 3. Release

- [ ] 3.1 Shipped in the m30 release candidate with `reclaim-provider-host-service`.
