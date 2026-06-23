---
id: call/protocol/wire-types
name: Implement EventEnvelope, ResponseEnvelope, CallError, and length-prefixed JSON framing
status: completed
depends_on: [call/crate-init]
scope: moderate
risk: medium
impact: component
level: implementation
---

## Description

Implement the wire protocol types and framing in `src/protocol/wire.rs`. Every
message on the wire is a length-prefixed JSON `EventEnvelope`.

### EventEnvelope

```rust
pub struct EventEnvelope {
    pub r#type: String,    // Event type
    pub id: String,        // Correlation key (request ID, subscription ID)
    pub payload: Value,    // serde_json::Value — schema depends on event type
}

// Frame: 4-byte big-endian length prefix + UTF-8 JSON body
```

The envelope is JSON because it must be consumable from JavaScript, Python, and
any language. The `Value` type is `serde_json::Value`.

Binary payloads (postcard, protobuf) are base64-encoded as a JSON string within
the `payload` field. The envelope itself does not interpret the payload — this
is a handler-level concern, not a protocol-level concern.

### Event Types

Five event types:

| Event | Direction | Purpose |
|-------|-----------|---------|
| `call.requested` | Caller → Handler | Initiate a call or subscription |
| `call.responded` | Handler → Caller | Deliver a result (one for calls, many for subscriptions) |
| `call.completed` | Handler → Caller | Signal end of subscription stream |
| `call.aborted` | Either side | Cancel the call/subscription |
| `call.error` | Handler → Caller | Signal an error |

### Wire Payload Schemas

| Event | `payload` shape |
|-------|----------------|
| `call.requested` | `{ "operationId": "/fs/readFile", "input": {...}, "auth_token": "alk_..." (optional) }` |
| `call.responded` | `{ "output": <Value> }` |
| `call.completed` | `{}` — empty object |
| `call.aborted` | `{}` — empty object |
| `call.error` | `{ "code": "...", "message": "...", "retryable": bool, "details": {...} (optional) }` |

### call.requested payload

```json
{
  "operationId": "/fs/readFile",
  "input": { ... },
  "auth_token": "alk_..."    // optional
}
```

- `operationId` — the operation to invoke, **with a leading slash** on the wire.
  The registry stores names without the leading slash; the wire format adds it.
  The CallAdapter strips the leading slash before registry lookup.
- `input` — the operation input, matching the operation's `input_schema`.
- `auth_token` — optional. If present, CallAdapter resolves via
  `IdentityProvider::resolve_from_token()`. Resulting Identity takes precedence
  over connection-level identity for this request.

The `call.requested` payload does **not** carry an abort policy field. The abort
policy is set on `OperationContext` and propagated through
`OperationEnv::invoke()` — the composing handler decides, not the wire caller.

### call.error payload

```json
{
  "code": "FILE_NOT_FOUND",
  "message": "file not found: /etc/nonexistent",
  "retryable": false,
  "details": { "path": "/etc/nonexistent", "errno": 2 }
}
```

Protocol-level codes (emitted by dispatch machinery):
- `NOT_FOUND` — operation not in registry (or Internal op called from wire)
- `FORBIDDEN` — access denied
- `INVALID_INPUT` — input doesn't match JSON Schema
- `INTERNAL` — handler error, panic, connection failure
- `TIMEOUT` — request timed out (retryable: true)

Operation-level domain codes (emitted by handlers, ADR-023): e.g.,
`FILE_NOT_FOUND`, `RATE_LIMITED`. These carry a `details` payload conforming to
the declared `ErrorDefinition.schema`.

New error codes may be added in future. Clients should treat unknown codes as
`INTERNAL` with `retryable: false`.

### ResponseEnvelope

```rust
pub struct ResponseEnvelope {
    pub request_id: String,
    pub result: Result<Value, CallError>,
}

pub struct CallError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
    pub details: Option<Value>,
}
```

Local dispatch produces `ResponseEnvelope` with no serialization overhead. The
CallAdapter converts it to `EventEnvelope` for the wire.

### ResponseEnvelope → EventEnvelope conversion

| `ResponseEnvelope` | `EventEnvelope` |
|--------------------|-----------------|
| `Ok(value)` | `{ type: "call.responded", id: request_id, payload: { output: value } }` |
| `Err(call_error)` | `{ type: "call.error", id: request_id, payload: <serialized CallError> }` |

For subscriptions, each `call.responded` is a separate `EventEnvelope` with the
same `id`; `call.completed` is `{ type: "call.completed", id, payload: {} }`.

### Framing

Length-prefixed JSON: 4-byte big-endian length prefix + UTF-8 JSON body.

Implement:
- `FrameFramedReader` — reads length-prefixed frames from an async reader
  (RecvStream)
- `FrameFramedWriter` — writes length-prefixed frames to an async writer
  (SendStream)

```rust
pub struct FrameFramedReader<R: AsyncRead + Unpin> { /* ... */ }
impl<R: AsyncRead + Unpin> FrameFramedReader<R> {
    pub fn new(reader: R) -> Self;
    pub async fn read_frame(&mut self) -> Result<EventEnvelope, FrameError>;
}

pub struct FrameFramedWriter<W: AsyncWrite + Unpin> { /* ... */ }
impl<W: AsyncWrite + Unpin> FrameFramedWriter<W> {
    pub fn new(writer: W) -> Self;
    pub async fn write_frame(&mut self, envelope: &EventEnvelope) -> Result<(), FrameError>;
}
```

This is the same framing used by irpc. The Rust implementation in alknet-call is
canonical (ADR-005, ADR-013).

### ResponseEnvelope helper methods

```rust
impl ResponseEnvelope {
    pub fn ok(request_id: String, output: Value) -> Self;
    pub fn error(request_id: String, error: CallError) -> Self;
    pub fn not_found(request_id: String, op_name: &str) -> Self;
    pub fn forbidden(request_id: String, message: &str) -> Self;
}
```

### FrameError

```rust
pub enum FrameError {
    Io(io::Error),
    Json(serde_json::Error),
    ConnectionClosed,
    InvalidFrame,
}
```

## Acceptance Criteria

- [ ] `EventEnvelope` struct with type, id, payload fields
- [ ] `ResponseEnvelope` struct with request_id, result fields
- [ ] `CallError` struct with code, message, retryable, details fields
- [ ] `FrameError` enum with Io, Json, ConnectionClosed, InvalidFrame
- [ ] `FrameFramedReader` reads length-prefixed JSON frames
- [ ] `FrameFramedWriter` writes length-prefixed JSON frames
- [ ] 4-byte big-endian length prefix + UTF-8 JSON body
- [ ] `ResponseEnvelope::ok()`, `error()`, `not_found()`, `forbidden()` helpers
- [ ] `ResponseEnvelope` → `EventEnvelope` conversion (Ok → call.responded, Err → call.error)
- [ ] Unit test: write frame, read frame, round-trip EventEnvelope
- [ ] Unit test: ResponseEnvelope::ok produces correct EventEnvelope
- [ ] Unit test: ResponseEnvelope::error produces correct call.error EventEnvelope
- [ ] Unit test: framing handles large payloads
- [ ] Unit test: framing detects truncated frames (ConnectionClosed error)
- [ ] `cargo test -p alknet-call` succeeds
- [ ] `cargo clippy -p alknet-call` succeeds with no warnings

## References

- docs/architecture/crates/call/call-protocol.md — EventEnvelope, wire format, event types
- docs/architecture/decisions/005-irpc-as-call-protocol-foundation.md — ADR-005
- docs/architecture/decisions/012-call-protocol-stream-model.md — ADR-012
- docs/architecture/decisions/023-operation-error-schemas.md — ADR-023 (CallError, details)

## Notes

> The envelope is always JSON for cross-language compatibility. Binary
> payloads are base64-encoded within the payload field (handler concern, not
> protocol concern). The 4-byte big-endian length prefix is the same framing
> irpc uses. operationId on the wire has a leading slash; the registry stores
> names without it — the CallAdapter strips it before lookup.

## Summary

Implemented `EventEnvelope`, `ResponseEnvelope`, `CallError`, `FrameError`, and
`FrameFramedReader`/`FrameFramedWriter` with 4-byte big-endian length-prefixed
JSON framing in `protocol/wire.rs`. Added `ResponseEnvelope` helpers (ok/error/
not_found/forbidden) and `ResponseEnvelope`→`EventEnvelope` conversion. 20 unit
tests pass; clippy clean. Merged to develop.