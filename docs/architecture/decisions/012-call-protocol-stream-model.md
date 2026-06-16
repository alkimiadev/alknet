# ADR-012: Call Protocol Stream Model

## Status

Accepted

## Context

The call protocol (alknet-call) operates on a QUIC connection with ALPN `alknet/call`. Within that connection, QUIC provides bidirectional streams. The question is how the call protocol uses those streams and how it correlates requests with responses — especially when both sides can initiate calls.

The reference implementation used `EventEnvelope` framing with a `PendingRequestMap` that correlates `call.requested` events to `call.responded` events by request ID, regardless of which stream carries them. This works well but the relationship between streams and operations was underspecified.

OQ-07 asked: "What is the scope of the call protocol within a connection? Should operations be multiplexed within a single stream, or should each operation get its own stream?"

## Decision

The call protocol uses **bidirectional QUIC streams with EventEnvelope framing and ID-based correlation**. The protocol does not prescribe a stream usage pattern — it works with any arrangement:

1. **EventEnvelope on every stream** — every bidirectional stream opened on the `alknet/call` connection carries length-prefixed JSON `EventEnvelope` messages. The five event types (`call.requested`, `call.responded`, `call.completed`, `call.aborted`, `call.error`) are the protocol primitives.

2. **PendingRequestMap correlates by ID, not by stream** — the `id` field in `EventEnvelope` correlates requests with responses. A response on stream 5 can fulfill a request sent on stream 3. The PendingRequestMap is keyed by request ID.

3. **Protocol is symmetric** — both sides of the connection can `open_bi()` to initiate calls and `accept_bi()` to receive them. The server calling a client operation uses the same EventEnvelope format and the same correlation mechanism.

4. **Top-level protocol operations** — the call protocol defines four operations that map to EventEnvelope event patterns:
   - **call**: `call.requested` → `call.responded` (one response) or `call.error`
   - **subscribe**: `call.requested` → one or more `call.responded` → `call.completed` or `call.aborted`
   - **batch**: multiple `call.requested` events (with correlated IDs) → multiple `call.responded` events
   - **schema**: `call.requested` (name `/services/list` or `/services/schema`) → `call.responded`

5. **Stream usage is the client's choice** — a client may open one stream per operation, one stream for all operations, or any mix. The protocol is stream-agnostic. The server accepts streams and processes EventEnvelopes regardless of which stream they arrive on.

This resolves OQ-07: the call protocol's scope within a connection is the full operation registry. One `alknet/call` connection gives access to all operations (call, subscribe, batch, schema). QUIC's built-in stream multiplexing handles concurrency — the protocol doesn't need to impose additional multiplexing.

## Consequences

**Positive:**
- Simple mental model: one connection, full access, stream-agnostic correlation
- The protocol works the same way regardless of stream usage — no "right" way to use streams
- Bidirectional calls are natural — either side can open a stream and send `call.requested`
- PendingRequestMap from the reference implementation carries forward without modification
- QUIC's stream multiplexing provides natural flow control and head-of-line blocking avoidance
- The top-level operations (call, subscribe, batch, schema) are protocol primitives, not separate ALPNs

**Negative:**
- Clients that multiplex many operations on one stream must manage request IDs carefully — but this is standard RPC practice
- The PendingRequestMap requires timeout-based cleanup to prevent memory leaks from abandoned requests — but this is already implemented and tested in the reference
- No built-in stream-level backpressure per operation when multiple operations share a stream — but QUIC provides connection-level and stream-level flow control

## References

- ADR-005: irpc as call protocol foundation
- ADR-006: ALPN string convention and connection model
- ADR-007: BiStream type definition
- OQ-07: Call protocol scope within a connection (resolved by this ADR)
- Reference implementation: `/workspace/@alkdev/alknet-main/crates/alknet-core/src/call/`