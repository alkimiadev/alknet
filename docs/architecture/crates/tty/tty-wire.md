---
status: draft
last_updated: 2026-07-07
---

# alknet-tty — Wire Format

The wire protocol for `alknet/tty`: the negotiation frame (JSON
carriage), the raw chunk codec, the control channel, and the sentinels.
The two-carriage model is decided in
[ADR-052](../../decisions/052-alknet-tty-wire-format-and-two-carriage.md);
this document specifies what an implementer builds.

## What

A `alknet/tty` bidi stream carries one terminal session. The stream has
two phases:

1. **Negotiation (JSON carriage).** A single length-prefixed JSON frame
   from the client carrying the terminal parameters, backend selector,
   command, and environment.
2. **Raw carriage.** After the negotiation frame, the stream switches to
   a chunk format for the life of the session: bidirectional byte pumping
   with a 1-byte stream-type multiplexer and a JSON control channel.

The format is the alknet-docker POC's raw chunk format
(`/workspace/alknet-docker-poc/src/raw.rs`, stream_type 0/1/2) extended
with a 4th stream_type (3 = control) and a JSON control message schema,
both validated by the alknet-tty POC (`/workspace/alknet-tty-poc/src/raw.rs`
+ `src/control.rs`). See ADR-052.

## Why

A terminal session is a byte stream with a small control sideband. The
two-carriage model (JSON negotiation, then raw chunks) keeps the call
protocol's JSON-RPC shape for the structured request and switches to
bytes for the body, which is what a terminal actually is. The fixed
channel set (four stream types, no negotiation) is an impoverishment of
SSH's channel multiplexer that is the feature: alknet-tty multiplexes
*one* service (a terminal session) with a fixed channel structure, not
*arbitrary* services, so the demux is a `match`, not a hash lookup. The
full rationale — why not JSON for everything, why fixed channel set
rather than extensible — is in
[ADR-052](../../decisions/052-alknet-tty-wire-format-and-two-carriage.md)
§Context.

## Architecture

### Phase 1: Negotiation Frame (JSON Carriage)

The client opens a bidi stream (or the server accepts one) and writes a
single length-prefixed JSON frame. The framing is a 4-byte big-endian
length prefix + UTF-8 JSON body — a self-contained ~30-line module in
alknet-tty (read 4-byte length, bounds-check, read N bytes; write the
inverse) on tokio's `AsyncRead`/`AsyncWrite`. The format coincides with
alknet-call's `EventEnvelope` framing by convention (both are
length-prefixed JSON), not by code reuse — alknet-tty does not depend on
alknet-call. The negotiation payload is a tty-specific struct
(`NegotiateRequest`), not a `call.requested` event. See ADR-052 §6 and
ADR-057.

The payload shape:

```json
{
  "carriage": "raw",
  "backend": "local",
  "tty": {
    "term": "xterm-256color",
    "cols": 80,
    "rows": 24,
    "pixel_width": 0,
    "pixel_height": 0,
    "modes": {}
  },
  "cmd": ["/bin/bash"],
  "cwd": null,
  "env": {}
}
```

Fields:

- `carriage` — `"raw"` for terminal sessions (the only carriage in v1).
  Selects the post-negotiation byte format. MUST be `"raw"` in v1; any
  other value (e.g., `"json"`, an unknown carriage, or the field
  absent) is a `malformed_negotiation` error and the adapter closes the
  stream without entering raw mode. A future carriage (e.g., a
  structured JSON-only mode for a non-terminal use case) is a v2
  addition; in v1 the field is required and must be the literal
  `"raw"`.
- `backend` — the backend selector string (`"local"`, `"docker"`,
  `"ssh"`). The adapter dispatches to the registered `TtyBackend` by this
  key (ADR-053 §5).
- `tty` — terminal parameters. `null` for the pipe/runner case (no PTY —
  ADR-054). `Some` for the PTY case. The `tty` block maps directly to
  SSH's `pty_request` parameters (term, cols, rows, pixel_width,
  pixel_height, modes) and to docker's `CreateExecOptions { tty: true }`;
  a local backend passes it to `portable_pty::PtySystem::openpty`. The
  `modes` field is reserved (OQ-44 — default terminal modes suffice for
  the current scope).
- `cmd` — command vector (argv[0] + args). Non-empty.
- `cwd` — working directory (`null` = inherit/default).
- `env` — environment variables (empty = inherit).

The Rust struct the adapter parses the frame into:

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

Validation: `carriage` MUST be `"raw"` (else `malformed_negotiation`);
`cmd` MUST be non-empty (else `malformed_negotiation`); `backend` MUST
be a registered backend key (else `unknown_backend`). Backend-specific
params validation is the backend's job (in `allocate()`); the adapter
does not interpret `backend_params`. The struct's `serde(flatten)` for
backend-specific fields means the negotiation frame's top-level JSON
object carries both the shared fields (`carriage`, `backend`, `tty`,
`cmd`, `cwd`, `env`) and the backend-specific fields (e.g.,
`"container": "abc123"` for docker); the latter land in
`backend_params`.

Backend-specific selector fields ride alongside (e.g., `"container":
"abc123"` for docker). The adapter parses the negotiation frame,
extracts the `backend` string, and passes the remaining backend-specific
fields to the selected backend's `allocate()` as an opaque
`serde_json::Map` (ADR-053) — the adapter does not interpret them; the
backend deserializes its own strongly-typed params struct.

After the negotiation frame, the stream switches to raw chunks. There is
no `call.responded`/`call.completed` — this is not the call protocol.

### Phase 2: Raw Chunk Format

```text
[stream_type: u8][length: u32 be][payload bytes]
```

- **`stream_type`** (1 byte) — the channel:

  | stream_type | channel | direction | payload |
  |---|---|---|---|
  | 0 | data-in (stdin) | client→server | raw bytes |
  | 1 | data-out (stdout) | server→client | raw bytes |
  | 2 | data-err (stderr) | server→client | raw bytes |
  | 3 | control | bidirectional | JSON control message |

  `stream_type > 3` is a protocol error (`InvalidStreamType`). There is
  no extension escape hatch in the byte — a 5th channel is a wire-format
  change requiring a new ALPN (`alknet/tty/v2` per ADR-006), not a
  negotiated addition to this format. See ADR-052 §"Fixed channel set,
  not extensible."

- **`length`** (4 bytes, big-endian) — payload length in bytes. Max
  16 MiB (`MAX_CHUNK_LEN = 16 * 1024 * 1024`). A chunk larger than 16 MiB
  is a protocol error (`ChunkTooLarge`).

- **`payload`** (`length` bytes) — the raw bytes (for data channels) or
  UTF-8 JSON (for the control channel).

The codec is `ChunkReader`/`ChunkWriter` (the POC's
`/workspace/alknet-tty-poc/src/raw.rs` generalized):
`ChunkReader::read_chunk()` reads the 5-byte header, validates the
stream_type and length, reads the payload; `ChunkWriter::write_chunk()`
writes the header and payload. See ADR-052.

### Sentinels

Zero-length data chunks are sentinels:

- **Zero-length stdin chunk (stream_type 0, length 0)** — EOF from the
  client. The server closes the backend's stdin (`ChildStdin::drop` /
  PTY writer close). This is one of two canonical "stdin done" signals;
  the other is a `{"type":"eof"}` control chunk — see OQ-47.
- **Zero-length stdout chunk (stream_type 1, length 0)** — "drained"
  from the server. The backend's stdout stream ended (process exited,
  container output stream ended, SSH channel closed). This is an
  implementation sentinel; the deterministic completion signal is the
  exit control chunk (ADR-055), not this sentinel — but the drained
  sentinel is emitted for symmetry with the docker POC's pattern.

Control chunks are never zero-length (the JSON payload is at least
`{}`).

### Control Channel (stream_type 3)

Control chunks carry a JSON payload tagged by `type`. The schema is the
POC's `ControlMessage` (`/workspace/alknet-tty-poc/src/control.rs`):

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlMessage {
    Resize {
        cols: u16,
        rows: u16,
        #[serde(default)]
        pixel_width: u16,
        #[serde(default)]
        pixel_height: u16,
    },
    Signal { name: String },
    Eof,
    Exit { code: i32 },
}
```

| Direction | Message | Shape | Maps to |
|---|---|---|---|
| client→server | resize | `{"type":"resize","cols":80,"rows":24,"pixel_width":0,"pixel_height":0}` | SSH `window-change`, docker exec resize, `ioctl(TIOCSWINSZ)` |
| client→server | signal | `{"type":"signal","name":"INT"}` | SSH `signal`, docker exec signal, `kill(-pgid, sig)` (REQ-TTY-02) |
| client→server | eof | `{"type":"eof"}` | SSH channel EOF, docker stdin close, `ChildStdin::drop` |
| server→client | exit | `{"type":"exit","code":0}` | the terminal/completion signal (ADR-055) |

**Signal names.** `name` is an uppercase string. The supported set (per
the POC's `signal_from_name`): `HUP`, `INT`, `QUIT`, `TERM`, `KILL`,
`USR1`, `USR2`, `TSTP`, `CONT`. Unknown names fall back to the backend's
default kill (see [tty-local.md](tty-local.md) REQ-TTY-02 —
`portable_pty`'s `ChildKiller::kill` sends SIGHUP).

**Exit code.** `code` is `i32` (matches `std::process::ExitStatus::code()`;
negative values are signal-terminated, e.g., -9 for SIGKILL on Unix). The
exit chunk is the last control chunk before stream close (ADR-055).

**Extensibility.** The `type` tag is the extension seam: new control
message types are added by extending the tagged enum. Unknown `type`
values are **ignored** (not a protocol error) so that a newer client
sending a control message an older server doesn't recognize degrades
gracefully rather than tearing down the session. This is a two-way-door
extension point within the one-way-door wire format (ADR-052) — adding a
control message type is additive; changing the meaning of an existing
type is not.

### Stdin Closure

Two signals both close the client's stdin:

1. **`{"type":"eof"}` control chunk** (stream_type 3) — explicit,
   recommended. Tells the server to close the backend's stdin
   (`ChildStdin::drop` / PTY writer close). The client may still want to
   receive remaining stdout + the exit code, so the server does not tear
   down the session on eof — it just closes stdin and keeps pumping
   output.
2. **Zero-length stdin chunk** (stream_type 0, length 0) — the docker
   POC's sentinel. Accepted for compatibility with that pattern.

The spec recommends `eof` for explicitness (it's a control message, not
a data-length hack), but both are accepted. See OQ-47.

### Connection vs Stream

A `Connection` (ADR-007) can open/accept multiple bidi streams. One
`alknet/tty` connection hosts multiple terminal sessions — one session
per bidi stream (DP-6, decided in the research). This matches the call
protocol's model (one operation per stream, multiple operations per
connection) and is the natural fit for QUIC's stream multiplexing. A
coordinator opens one connection to an endpoint and launches multiple
sessions (one stream each) for parallel tasks. The `TtyAdapter::handle`
accepts the connection and loops `accept_bi`, dispatching each stream to
a session — see [tty-adapter.md](tty-adapter.md).

## Constraints

- **The wire format is one-way (ADR-052).** The 5-byte header, the fixed
  stream_type set (0-3), and the two-carriage sequence are bytes clients
  and servers parse. A 5th channel type requires a new ALPN
  (`alknet/tty/v2` per ADR-006), not a negotiated addition.
- **No windowing.** The chunk format has no flow-control window; QUIC's
  per-stream flow control is the backpressure mechanism (OQ-45 resolved:
  the backpressure chain is complete by construction — QUIC flow control
  → bounded drainer channel → bounded stdout channel → OS pipe/PTY
  buffer → process `write()` blocks; no unbounded buffer breaks the
  chain). The reversal path, if ever needed, is an additive
  `ControlMessage` variant on stream_type 3, not a wire-format header
  change.
- **No negotiation round-trip.** The client writes the negotiation frame
  and starts sending chunks; the server reads the frame and starts
  pumping. There is no "the server acknowledges the negotiation before
  the client sends data" step — QUIC's stream reliability handles
  in-order delivery, and the negotiation frame is small (fits in the
  initial flow-control window — ADR-052 assumption 2).
- **Negotiation errors are JSON, not chunks.** If the server cannot
  allocate the session (unknown backend, PTY allocation failed, the
  command is invalid), it sends a JSON error response in the same
  length-prefixed framing as the negotiation frame and closes the stream
  without entering raw mode. The error response MUST be under 16 MiB
  (`MAX_CHUNK_LEN`) so the 4-byte big-endian length prefix's high byte
  is `0x00` — this is what makes the framing-disambiguation trick
  (first byte `0x00` = error frame, first byte `1`/`2`/`3` = raw chunk)
  sound; it is a wire-format invariant, not an empirical observation.
  See [tty-adapter.md](tty-adapter.md) §"Negotiation errors".

## Design Decisions

| Decision | ADR | Summary |
|----------|-----|---------|
| Wire format and two-carriage model | [ADR-052](../../decisions/052-alknet-tty-wire-format-and-two-carriage.md) | `alknet/tty` ALPN; JSON negotiation frame then raw chunks; fixed channel set 0-3; control as JSON |
| No alknet-call dependency (self-contained framing) | [ADR-057](../../decisions/057-alknet-tty-no-alknet-call-dep.md) | alknet-tty implements its own length-prefixed framing; format coincides with alknet-call's by convention, not by code reuse |
| Exit code on a control chunk | [ADR-055](../../decisions/055-exit-code-on-control-chunk.md) | `{"type":"exit","code":N}` on stream_type 3; "exit chunk is last" invariant |
| Stdin closure canonical signal | OQ-47 | Either `eof` control chunk or zero-length stdin chunk; `eof` recommended |

## Open Questions

See [open-questions.md](../../open-questions.md) for full details.

- **OQ-44** (deferred(scope)): Terminal modes.
- **OQ-45** (resolved): Flow control for high-throughput stdout — no
  application-level windowing; QUIC per-stream flow control is the
  backpressure mechanism.
- **OQ-47** (resolved): Stdin closure canonical signal.

## References

- [ADR-052](../../decisions/052-alknet-tty-wire-format-and-two-carriage.md)
  — the wire format decision
- [ADR-055](../../decisions/055-exit-code-on-control-chunk.md) — the
  exit-chunk ordering the control channel carries
- [ADR-003](../../decisions/003-crate-decomposition.md) Amendment 2 —
  alknet-tty does not depend on alknet-call (self-contained framing)
- [ADR-057](../../decisions/057-alknet-tty-no-alknet-call-dep.md) — the
  dependency-edge decision (negotiation framing is self-contained in
  alknet-tty)
- `/workspace/alknet-tty-poc/src/raw.rs` — the chunk codec
  (`ChunkReader`/`ChunkWriter`, stream_type 0-3) this spec commits
- `/workspace/alknet-tty-poc/src/control.rs` — the JSON control schema
  (`ControlMessage` tagged enum) this spec commits
- `/workspace/alknet-docker-poc/src/raw.rs` — the seed codec
  (stream_type 0/1/2) the tty POC extended
- `docs/research/alknet-docker/poc-summary.md` — the POC that validated
  the raw chunk format and the two-carriage model
- [tty-adapter.md](tty-adapter.md) — the session lifecycle that consumes
  this wire format