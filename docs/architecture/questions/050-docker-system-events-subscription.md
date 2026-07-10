# OQ-50: Docker System Events Subscription

- **Origin**:
  [crates/docker/docker-operations.md](crates/docker/docker-operations.md)
  §"Operation Surface";
  [ADR-060](decisions/060-container-resource-model-and-label-namespace.md)
  §4 (autonomous-death tolerance).
- **Status**: resolved
- **Door type**: Two-way
- **Priority**: low
- **Resolution**: `docker/system/events` is included in v1 as a
  `Subscription` operation. bollard's `events()` returns
  `Stream<SystemEventsMessage>` — the same `StreamingHandler` pattern
  already wired for `logs`, `exec`, and `image/pull`. The operation
  surfaces daemon events (container start/stop/die/destroy, image
  pull/tag/delete, etc.) as `call.responded` frames. The internal
  ownership-store subscription for stale-entry cleanup on `destroy`
  events is a follow-up refinement — the operation itself is the
  architecture decision. The use case is already documented in
  ADR-060 §4 (autonomous container death leaves stale ownership
  entries); the operation is cheap to add (same mechanical
  `StreamingHandler` mapping as the three existing streaming ops) and
  generally useful beyond ownership cleanup (any consumer may want
  daemon events).
- **Cross-references**:
  [ADR-060](decisions/060-container-resource-model-and-label-namespace.md)
  §4 (teardown coupling — stale-entry tolerance is the base model;
  the events subscription provides the prompt-cleanup path),
  [crates/docker/docker-operations.md](crates/docker/docker-operations.md)
