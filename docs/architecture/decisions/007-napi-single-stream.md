# ADR-007: NAPI Exposes Single Duplex Stream

## Status
Accepted

## Context
The NAPI wrapper for alknet could expose different granularity levels:

1. **Full SSH API**: Expose channel multiplexing, `open_direct_tcpip`, `tcpip_forward`, session management. The TypeScript layer would manage channels.
2. **Single duplex stream**: The NAPI wrapper establishes one SSH channel and returns it as a Node.js `Duplex` stream. TypeScript multiplexing (if needed) happens at the pubsub layer.

## Decision
Option 2: NAPI exposes a single duplex stream.

The NAPI wrapper's job is to get a reliable, authenticated byte stream from A to B. It handles transport (TCP/TLS/iroh), SSH authentication, and channel setup, then hands the caller a single `Duplex` stream that just works.

If the TypeScript consumer needs multiplexing (e.g., multiple concurrent tool calls over operations), pubsub handles that at the `EventEnvelope` level. Multiple `call.requested` / `call.responded` events flow over the same stream, distinguished by their `id` fields. This is how the existing WebSocket adapter works.

## Consequences
- **Positive**: Minimal NAPI surface — one function, one return type. Small binary, small FFI boundary.
- **Positive**: The TypeScript side doesn't need to understand SSH at all. It gets a stream and sends/receives `EventEnvelope` JSON.
- **Positive**: No need to expose russh types in NAPI. The SSH complexity stays in Rust.
- **Negative**: If a consumer wants multiple isolated channels (e.g., one for events, one for file transfer), they'd need multiple `connect()` calls (multiple SSH sessions). This is acceptable for the expected use case (pubsub events over a single stream).

## References
- [napi-and-pubsub.md](../napi-and-pubsub.md)