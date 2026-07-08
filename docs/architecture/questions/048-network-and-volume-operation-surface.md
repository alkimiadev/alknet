# OQ-48: Network and Volume Operation Surface

- **Origin**:
  [crates/docker/docker-operations.md](crates/docker/docker-operations.md)
  §"Operation Surface (v1 scope)" (out-of-scope list).
- **Status**: deferred(scope)
- **Door type**: Two-way
- **Priority**: low
- **Blocked on**: a concrete use case for network or volume management
  over the call protocol. The two container use cases (disposable dev
  containers, hosted services) don't currently require
  network/volume CRUD over the call protocol — dev containers use
  the default bridge network; hosted services are configured via
  `docker compose` with networks/volumes declared in the compose file.
- **Resolution**: Not yet decidable. bollard has the API surface
  (`network.rs`: `create_network`, `remove_network`, `inspect_network`,
  `list_networks`, `connect_network`, `disconnect_network`;
  `volume.rs`: `list_volumes`, `create_volume`, `inspect_volume`,
  `remove_volume`, `prune_volumes`) — the mapping is mechanical, the
  same shape as the container lifecycle ops (`Query`/`Mutation`,
  single `call.responded`). The deferral is scope, not feasibility:
  v1 is containers + images; networks and volumes are added when a
  use case forces them (e.g., a coordinator that needs to create
  isolated networks for dev containers, or a fleet layer that manages
  volumes across hosts). Adding them is additive (new operations in
  the registry) and does not break the existing surface.
- **Cross-references**:
  [ADR-058](decisions/058-alknet-docker-on-alknet-call.md) (the shared
  `alknet/call` registration model new ops would follow),
  [crates/docker/docker-operations.md](crates/docker/docker-operations.md)