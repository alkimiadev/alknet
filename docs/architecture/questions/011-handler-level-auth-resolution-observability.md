# OQ-11: Handler-Level Auth Resolution Observability

- **Origin**: [auth.md](crates/core/auth.md)
- **Status**: resolved
- **Door type**: Two-way
- **Priority**: medium
- **Resolution**: **Option B — handlers store resolved identity on the Connection.** When a handler resolves identity inside `handle()` (the handler-level auth phase), it calls `connection.set_identity(identity)` to store the resolved `Identity` on the connection object. The endpoint and observability layers can read it later for connection logging, audit trails, and metrics.

  Why not Option A (return identity from `handle()`): it changes the `ProtocolHandler` trait signature for all handlers, even those that don't do auth resolution (DNS, health check). It also assumes one identity per connection — but the call protocol can have different identities per request on the same connection (one connection, multiple `call.requested` events with different auth tokens). Returning a single identity from `handle()` would be misleading for the call protocol.

  Why not Option C (identity stays local): the resolved identity is useful beyond the handler. The endpoint may want to log "connection from X authenticated as Y." A connection-level observability layer needs the identity. If it stays local, every handler that resolves identity would need to duplicate logging logic, and the endpoint can't correlate connections to identities.

  **Two identity scopes exist and must not be conflated:**
  - **Connection-level identity** (this decision): set once by the handler in `handle()`, stored on `Connection`, read by the endpoint for logging/observability. This is the "connection owner" — who opened this QUIC connection.
  - **Per-request identity** (already in the call protocol spec): set per `call.requested` by the `CallAdapter`, stored on `OperationContext.identity`. This is the "call caller" — who is making this specific call, which may upgrade mid-session (different auth tokens on the same connection).

  Both exist. The connection-level identity is the stable "who is this connection from"; the per-request identity is the dynamic "who is this specific call from." The call protocol's per-request resolution (which may produce a different identity than the connection-level resolution) takes precedence for ACL on `OperationContext` — the connection-level identity is for observability only, not for ACL.

  **C13 resolution (review #002)**: the endpoint does **not** read
  `identity()` after `handle()` returns. The `Connection` is moved into the
  spawned handler task (endpoint.md), so the endpoint no longer has a
  reference to it. Connection-level observability (remote addr, ALPN,
  connection ID) is logged by the endpoint *before* the move. Identity-level
  observability is logged by the handler (the handler knows which identity
  it resolved and can log it). There is no `Arc<Connection>` sharing or
  channel-based identity-reporting mechanism — the simplest honest answer
  that avoids over-engineering the observability path before there's a
  demonstrated need. If a future use case requires the endpoint to
  correlate connections to identities, an `Arc<Connection>` or a
  side-channel can be added then.
- **Cross-references**: ADR-004, ADR-011, ADR-015 (per-request identity on OperationContext), [auth.md](crates/core/auth.md)
