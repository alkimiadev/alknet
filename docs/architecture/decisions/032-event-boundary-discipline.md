# ADR-032: Event Boundary Discipline

## Status

Accepted

## Context

The research identified three distinct communication patterns in the system, and
conflating them is a known anti-pattern in event-driven architectures:

1. **Domain events** (Honker streams) — Internal to the service that owns that
   data. Used for state reconstruction within the service's own boundaries.
   Examples: `nodes:created`, `edges:deleted`, `accounts:updated`.

2. **irpc service calls** — Synchronous request-response within a node or
   cluster. Internal to the system. Examples: `AuthProtocol::VerifyPubkey`,
   `SecretProtocol::DeriveEd25519`, `ConfigProtocol::ReloadForwarding`.

3. **Call protocol events** (`EventEnvelope`) — Asynchronous integration events
   that cross node boundaries. External to the system. Examples:
   `call.requested`, `call.responded`, `call.completed`, `call.aborted`.

Without a hard constraint, it's tempting to have one service subscribe directly
to another service's Honker streams. This leads to:

- **Leaky event store**: Service A reads Service B's domain events directly,
  coupling A to B's internal state representation. When B changes its schema, A
  breaks.
- **Boomerang coupling**: An integration event is too thin, causing the
  consumer to call back to the source service synchronously to get details. This
  negates the benefit of async communication.
- **Fat notification trap**: A notification event carries full entity state,
  when it should use state transfer instead.

## Decision

**Event boundary discipline is a hard architectural constraint, not a
suggestion.**

1. **Domain events stay within the owning service.** A Honker stream published
   by the storage service (`nodes:created`) is for the storage service's own
   state reconstruction. No other service reads these stream events directly.

2. **irpc service calls are synchronous and internal.** They never cross node
   boundaries. They are request-response, not events. They should not be used
   as a substitute for integration events.

3. **Call protocol events are the only events that cross node boundaries.**
   `EventEnvelope` frames are the integration boundary. When a domain event
   needs to be communicated to another node, it must be projected into a call
   protocol event.

4. **Projection from domain events to integration events is required when
   crossing boundaries.** A service that owns a Honker stream must project
   relevant state changes into `EventEnvelope` frames before they leave the
   node. The projection strips internal details and produces a versioned,
   stable integration event.

This discipline applies at three levels:

```
Call Protocol (Layer 3, external, JSON)
    └── irpc Service (Layer 3, internal, postcard)
            └── Honker Streams (Domain events, within service boundary)
```

A call protocol handler MAY call an irpc service internally (e.g.,
`/head/auth/verify` calls `AuthProtocol::VerifyPubkey`). The irpc service MAY
use Honker streams for its own state management. But domain events never
propagate beyond the service boundary without projection.

## Consequences

- **Positive**: Prevents leaky event stores. Services are independently
  deployable and their internal schemas can evolve without breaking consumers.
- **Positive**: Honker and irpc are implementation details, not cross-boundary
  contracts. The call protocol's `EventEnvelope` is the only stable, versioned
  contract that other nodes depend on.
- **Positive**: Clear ownership. Each service owns its Honker streams and can
  change them freely. Integration events are a deliberate, reviewed contract.
- **Positive**: Makes testing easier. Services can be tested in isolation with
  mock domain events. Integration events are tested against the `EventEnvelope`
  schema.
- **Negative**: Projection code is required. Every domain event that needs to
  cross a boundary must be explicitly projected. This is deliberate — the
  overhead ensures the integration contract is intentional.
- **Negative**: Developers must resist the temptation to subscribe directly to
  Honker streams across services. Code review should catch this pattern.

## References

- [research/services.md](../../research/services.md) — Event boundary discipline section
- [research/storage.md](../../research/storage.md) — Honker integration, event boundaries
- [research/integration-plan.md](../../research/integration-plan.md) — ADR 032 entry
- [event_source_types.md](../../research/event-sourcing/event_source_types.md) — Event-driven architecture patterns