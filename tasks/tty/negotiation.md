---
id: tty/negotiation
name: Implement negotiation frame (NegotiateRequest, length-prefixed framing, error response)
status: completed
depends_on: [tty/wire-codec]
scope: narrow
risk: medium
impact: component
level: implementation
---

## Description

Implement the Phase 1 "JSON carriage" negotiation frame in
`src/negotiation.rs`. The client opens a bidi stream and writes a single
length-prefixed JSON frame carrying the terminal parameters, backend selector,
command, and environment. After this frame, the stream switches to raw chunks
(task `tty/wire-codec`). The framing is self-contained in alknet-tty (ADR-057)
— a 4-byte big-endian length prefix + UTF-8 JSON body.

### NegotiateRequest struct

```rust
#[derive(Deserialize)]
pub struct NegotiateRequest {
    pub carriage: String,          // "raw" in v1; any other value → malformed_negotiation
    pub backend: String,           // backend selector key ("local", "docker", "ssh")
    pub tty: Option<TerminalParamsWire>,  // None = pipe mode (ADR-054)
    pub cmd: Vec<String>,           // argv[0] + args; non-empty
    #[serde(default)]
    pub cwd: Option<PathBuf>,       // None = inherit/default
    #[serde(default)]
    pub env: HashMap<String, String>,  // empty = inherit
    #[serde(default)]
    pub backend_params: serde_json::Map<String, serde_json::Value>,  // opaque; backend-deserialized
    // plus backend-specific fields, captured into backend_params via serde(flatten)
}

#[derive(Deserialize)]
pub struct TerminalParamsWire {
    pub term: Option<String>,       // None = backend default
    pub cols: u16,
    pub rows: u16,
    #[serde(default)]
    pub pixel_width: u16,
    #[serde(default)]
    pub pixel_height: u16,
    #[serde(default)]
    pub modes: serde_json::Value,   // reserved — OQ-44; backends MUST ignore content in v1
}
```

The `serde(flatten)` for backend-specific fields means the negotiation frame's
top-level JSON object carries both the shared fields (`carriage`, `backend`,
`tty`, `cmd`, `cwd`, `env`) and the backend-specific fields (e.g.,
`"container": "abc123"` for docker); the latter land in `backend_params`.

### Validation

- `carriage` MUST be `"raw"` (else `malformed_negotiation`).
- `cmd` MUST be non-empty (else `malformed_negotiation`).
- `backend` MUST be a registered backend key (else `unknown_backend`) — this
  check happens in the adapter (task `tty/adapter`), not here; this module
  only parses.
- Backend-specific params validation is the backend's job (in `allocate()`).

### Length-prefixed framing

A self-contained ~30-line reader/writer on tokio's `AsyncRead`/`AsyncWrite`:

```rust
pub struct NegotiationReader<R: AsyncRead + Unpin> { /* ... */ }
impl<R: AsyncRead + Unpin> NegotiationReader<R> {
    pub fn new(reader: R) -> Self;
    pub fn into_inner(self) -> R;
    /// Read a 4-byte BE length prefix, bounds-check, read N bytes.
    pub async fn read_frame(&mut self) -> Result<Bytes, NegotiationError>;
}

pub struct NegotiationWriter<W: AsyncWrite + Unpin> { /* ... */ }
impl<W: AsyncWrite + Unpin> NegotiationWriter<W> {
    pub fn new(writer: W) -> Self;
    pub fn into_inner(self) -> W;
    /// Write a 4-byte BE length prefix + the JSON body.
    pub async fn write_frame(&mut self, body: &[u8]) -> Result<(), NegotiationError>;
}
```

The reader bounds-checks the length against `MAX_CHUNK_LEN` (from `wire.rs`)
so a malformed length prefix can't trigger an oversized allocation. The POC
used a 1 MiB cap; the crate uses `MAX_CHUNK_LEN` (16 MiB) to match the raw
chunk limit and to make the framing-disambiguation trick sound (see below).

### Error response shape

If the server cannot allocate the session, it sends a JSON error response in
the same length-prefixed framing and closes the stream without entering raw
mode:

```json
{ "error": "unknown_backend", "backend": "kubernetes" }
```

| Error | When | Shape |
|-------|------|------|
| `unknown_backend` | the `backend` string is not in the adapter's backend map | `{"error":"unknown_backend","backend":"..."}` |
| `malformed_negotiation` | the negotiation frame failed to parse or failed validation | `{"error":"malformed_negotiation","message":"..."}` |
| `allocate_failed` | `backend.allocate()` returned a `TtyError` | `{"error":"allocate_failed","message":"..."}` |

Provide a helper to serialize an error response:

```rust
pub fn error_response_bytes(error: &str, fields: &[(&str, &str)]) -> serde_json::Result<Vec<u8>>;
```

### Framing disambiguation (success vs error)

Both a successful allocation (raw chunks) and a failed allocation (JSON error
frame) begin with bytes the client must read before knowing which framing
applies. The disambiguation is by the first byte:

- A JSON error frame's 4-byte big-endian length prefix always starts with
  `0x00` (error frames MUST be under 16 MiB — `MAX_CHUNK_LEN` — so the high
  byte is zero; this is a wire-format invariant, not an assumption).
- A raw chunk's first byte is a `stream_type` in `{0, 1, 2, 3}`. A stream_type
  of `0` (stdin from server) is invalid — the server never sends stdin chunks.

So the client distinguishes: read the first byte; if it is `0x00`, interpret
the next 4 bytes as a big-endian length prefix and read that many bytes as a
JSON error frame; otherwise interpret it as a `stream_type` byte and continue
reading the raw chunk header. This is a one-way-door wire-format invariant
(ADR-052). The `NegotiationWriter::write_frame` for an error MUST ensure the
body is under 16 MiB (the high byte of the length is `0x00`).

### NegotiationError

```rust
#[derive(Debug, thiserror::Error)]
pub enum NegotiationError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("connection closed")]
    ConnectionClosed,
    #[error("frame too large: {0}")]
    FrameTooLarge(u32),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
}
```

### Tests

- Round-trip: write a `NegotiateRequest` as JSON, read the frame, parse, assert
  fields match.
- `serde(flatten)` test: a frame with `{"carriage":"raw","backend":"local",...,"container":"abc"}`
  parses `container` into `backend_params`.
- Validation: `carriage != "raw"` → adapter rejects (this module parses; the
  adapter checks the value). Test that the struct deserializes regardless and
  the adapter-side check is a string comparison.
- `FrameTooLarge` on length > MAX_CHUNK_LEN.
- `ConnectionClosed` on truncated frame.
- Error response serialization produces the expected JSON shape.
- Framing disambiguation: an error frame's first byte is `0x00` (write a
  frame, read the first byte, assert `0x00`).

## Acceptance Criteria

- [ ] `NegotiateRequest` struct with all fields, `serde(flatten)` for `backend_params`
- [ ] `TerminalParamsWire` struct with `term`, `cols`, `rows`, `pixel_width`, `pixel_height`, `modes`
- [ ] `NegotiationReader::read_frame` reads 4-byte BE length, bounds-checks against `MAX_CHUNK_LEN`, reads body
- [ ] `NegotiationWriter::write_frame` writes 4-byte BE length + body
- [ ] `NegotiationError` with `Io`, `ConnectionClosed`, `FrameTooLarge`, `Json`
- [ ] `error_response_bytes` helper produces `{"error":"...","field":"..."}`
- [ ] Error frames are under 16 MiB (high byte of length prefix is `0x00`)
- [ ] `into_inner` on reader/writer reclaims the underlying stream for raw-chunk use
- [ ] Round-trip unit test for `NegotiateRequest` (all fields)
- [ ] Unit test: `serde(flatten)` captures backend-specific fields into `backend_params`
- [ ] Unit test: `FrameTooLarge` on length > MAX_CHUNK_LEN
- [ ] Unit test: `ConnectionClosed` on truncated frame
- [ ] Unit test: error response first byte is `0x00` (framing disambiguation)
- [ ] `cargo test -p alknet-tty` succeeds
- [ ] `cargo clippy -p alknet-tty` succeeds with no warnings

## References

- docs/architecture/crates/tty/tty-wire.md — §"Phase 1: Negotiation Frame", §"Constraints" (negotiation errors)
- docs/architecture/crates/tty/tty-adapter.md — §"Negotiation Errors" (framing disambiguation)
- docs/architecture/decisions/052-alknet-tty-wire-format-and-two-carriage.md — ADR-052
- docs/architecture/decisions/057-alknet-tty-no-alknet-call-dep.md — ADR-057 (self-contained framing)
- /workspace/alknet-tty-poc/src/session.rs — `NegotiationReader` (the ~30-line reference)

## Notes

> The framing is self-contained (ADR-057) — do NOT depend on alknet-call's
> `FrameFramedReader`. The `MAX_CHUNK_LEN` constant from `wire.rs` is the
> bounds-check ceiling for both the negotiation frame and the raw chunks,
> which is what makes the `0x00`-as-length-prefix vs `0x00`-as-invalid-stream_type
> disambiguation sound. The POC's `NegotiationReader` used a 1 MiB cap; the
> crate uses `MAX_CHUNK_LEN` (16 MiB) per the spec.

## Summary

> To be filled on completion