# OQ-51: Container Create Options Surface

- **Origin**:
  [crates/docker/docker-operations.md](crates/docker/docker-operations.md)
  §"Out of scope for v1";
  [crates/docker/docker-operations.md](crates/docker/docker-operations.md)
  §"`docker/container/create`".
- **Status**: deferred(scope)
- **Door type**: Two-way
- **Priority**: medium
- **Blocked on**: v1 implementation. The v1 `create` input schema
  accepts the common fields (image, command, env, labels, name) and
  the full `CreateContainerOptions` surface (mounts, port bindings,
  networks, volumes, capabilities, etc.) is deferred to the
  implementation pass, where the input JSON Schema can be designed
  against bollard's `Config` struct and tested against real create
  calls. This is not an architectural decision (the `create` operation
  is already decided — ADR-060 §5); it's a schema-detail decision
  best made with the bollard types in hand.
- **Resolution**: Not yet decidable. bollard's `create_container`
  takes a `Config` struct (`container.rs:296`) with ~40 fields
  (`Image`, `Cmd`, `Env`, `Labels`, `HostConfig` with mounts/ports/
  networks/etc.). The v1 input schema accepts the high-frequency
  fields and omits the long tail; the full surface is a JSON Schema
  design task (which fields are required, which are optional, how
  `HostConfig` is nested, how the `alknet.*` labels merge with
  caller labels). The deferral is to the implementation pass, not
  past it — the schema is finalized when `register_docker_ops` is
  written and tested. The `OperationSpec.input_schema` is the
  one-way surface; its exact field set is a two-way-door refinement
  within it.
- **Cross-references**:
  [ADR-060](decisions/060-container-resource-model-and-label-namespace.md)
  §5 (`create` records ownership — the architectural decision this
  OQ defers the schema details of),
  [crates/docker/docker-operations.md](crates/docker/docker-operations.md)
  §"`docker/container/create`"