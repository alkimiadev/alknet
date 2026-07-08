# OQ-49: Image Build (buildkit) Scope

- **Origin**:
  [crates/docker/docker-operations.md](crates/docker/docker-operations.md)
  §"Out of scope for v1"; [ADR-059](decisions/059-bollard-021-dependency-and-features.md)
  §3 (no `buildkit` feature).
- **Status**: deferred(scope)
- **Door type**: Two-way
- **Priority**: low
- **Blocked on**: a concrete use case for building images over the
  call protocol. The two container use cases (disposable dev
  containers, hosted services) pull pre-built images (`docker/image/pull`)
  rather than building them. The reverse-proxy and other hosted
  services build via `docker compose build` (operator-side), not via
  alknet.
- **Resolution**: Not yet decidable. bollard's `build_image` (image.rs:655)
  and the `buildkit` feature (which pulls tonic +
  bollard-buildkit-proto) are available but deferred. Build is a large
  feature (build context upload, layer caching, multi-stage, buildkit
  progress streaming) and is not needed for the current scope. When a
  use case forces it, the operation is a `Subscription` (progress
  events → `call.responded`, build complete → `call.completed`) and
  the `buildkit` feature is enabled in `Cargo.toml` (two-way-door
  feature addition, per ADR-059). The v1 surface has `image/pull` +
  `image/list` + `image/inspect`; `image/build` is added when needed.
- **Cross-references**:
  [ADR-059](decisions/059-bollard-021-dependency-and-features.md)
  (feature set decision — `buildkit` not enabled),
  [crates/docker/overview.md](crates/docker/overview.md) §"bollard
  version and features"