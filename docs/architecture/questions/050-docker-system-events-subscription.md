# OQ-50: Docker System Events Subscription

- **Origin**:
  [crates/docker/docker-operations.md](crates/docker/docker-operations.md)
  §"Out of scope for v1";
  [ADR-060](decisions/060-container-resource-model-and-label-namespace.md)
  §4 (autonomous-death tolerance).
- **Status**: deferred(scope)
- **Door type**: Two-way
- **Priority**: low
- **Blocked on**: a concrete use case for prompt stale-ownership
  cleanup. The base model (ADR-060 §4) tolerates stale ownership
  entries from autonomous container death (a `--rm` exit, external
  `docker rm`, daemon restart) — they're inert (a reused container ID
  gets a fresh `record` on its next `create`). A reaper that
  subscribes to docker daemon events would clean them promptly, but
  the promptness gain is marginal for the current use cases.
- **Resolution**: Not yet decidable. bollard's `events()`
  (system.rs:128) returns a `Stream<SystemEventsMessage>` of daemon
  events (container start/stop/die/destroy, image pull, etc.). A
  `docker/system/events` `Subscription` operation would surface these
  as `call.responded` frames; the ownership store could subscribe
  internally to revoke on `destroy` events. The deferral is scope:
  the base model works without it; the events subscription is a
  refinement for when prompt cleanup matters (e.g., a high-churn
  coordinator that spawns/removes many containers and wants the
  ownership store to stay tight). Adding it is additive (a new
  operation + an internal store subscription) and does not break the
  existing surface.
- **Cross-references**:
  [ADR-060](decisions/060-container-resource-model-and-label-namespace.md)
  §4 (teardown coupling — stale-entry tolerance is the base model
  this subscription would refine),
  [crates/docker/docker-operations.md](crates/docker/docker-operations.md)