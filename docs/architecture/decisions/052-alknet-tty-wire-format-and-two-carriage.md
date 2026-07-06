# ADR-052: alknet-tty Wire Format and Two-Carriage Model

## Status

Accepted

## Context

The alknet-docker POC (`docs/research/alknet-docker/poc-summary.md`)
validated that interactive attach — bidirectional byte pumping over a framed
bidi stream with a 1-byte stream-type multiplexer — is the same problem
regardless of whether the backend is `bollard::attach_container()` or
russh's `pty_request`. The POC's raw chunk format
(`[stream_type: u8][length: u32 be][payload bytes]`, stream_type 0=stdin,
1=stdout, 2=stderr) is a deliberately impoverished version of SSH's channel
multiplexer: fixed set of channel types, no negotiation, no open/close
handshake, no windowing (QUIC provides flow control on the bidi stream).
That impoverishment is the feature — a terminal session needs exactly those
channels and no more.

The alknet-tty POC (`/workspace/alknet-tty-poc`, built 2026-07-05) extended
that format with a 4th stream_type (3 = control) carrying JSON control
messages (resize, signal, eof, exit) and validated the full round-trip
against a real `portable_pty` PTY: negotiate → PTY alloc → bidirectional
echo → mid-session resize → EOF → exit code, plus SIGINT forwarding to a
child process group. The wire format this ADR commits is the POC's
`src/raw.rs` + `src/control.rs`, generalized from one backend to the
backend-agnostic crate.

Three load-bearing questions are decided here:

1. **Separate ALPN with raw carriage, not call-protocol operations.** A
   terminal session could be modeled as a call-protocol `Subscription`
   operation (`tty/open`) streaming `call.responded` events. That is
   rejected: JSON-encoding every byte chunk is wasteful (base64 for binary,
   per-chunk `EventEnvelope` overhead) and lossy (a TTY streams partial
   bytes with no message boundary that maps to a JSON object). The
   two-carriage model — a single JSON negotiation frame, then raw chunks —
   keeps the call protocol's JSON-RPC shape for the *request* and switches
   to bytes for the *body*, which is the part that is actually bytes. This
   is the pattern the docker POC validated and the SSH research
   (`docs/research/alknet-ssh/phase-0-findings.md`) independently arrived
   at for PTY.

2. **Fixed channel set, not extensible.** SSH multiplexes arbitrary
   services (forwarding, SFTP, agent, X11) over `ChannelId(u32)` with
   string-named types negotiated per channel. alknet-tty multiplexes one
   service — a terminal session — with a fixed `u8` set and no negotiation.
   Adding a 5th channel type is a wire-format change (one-way door). The
   ALPN model handles extensibility at the protocol level: a genuinely new
   sideband (e.g., file transfer alongside the terminal) is a different
   ALPN, not a 5th tty channel type. A new ALPN is cheap; a wire-format
   change is not.

3. **Control messages as JSON, not binary.** A binary control format
   (`[control_type: u8][params...]`) would be faster but harder to extend
   and inconsistent with the negotiation layer. Control messages are rare
   (resize on window drag, signal on Ctrl-C, one eof, one exit per
   session) — serialization cost is negligible against the data chunks. If
   a hot control path appears, a binary `control_type` can be added without
   breaking the chunk format (additive within the control channel).

## Decision

### 1. alknet-tty is a `ProtocolHandler` on ALPN `alknet/tty`

`alknet/tty` is a custom ALPN per the ADR-006 `alknet/<name>` convention.
The `TtyAdapter` implements `ProtocolHandler` (ADR-002, revised by ADR-007
to receive a `Connection`). The handler owns the entire connection
lifecycle and accepts one bidi stream per terminal session. This is a
separate ALPN, not a set of operations in the call protocol's
`OperationRegistry` — the raw-carriage byte pump is not a `StreamingHandler`
(ADR-049); it is its own wire format after a single JSON negotiation frame.

### 2. Two-carriage model: JSON negotiation, then raw chunks

The bidi stream has two phases:

- **Negotiation (JSON carriage).** The client opens a bidi stream and
  writes a single length-prefixed JSON frame — the same 4-byte
  big-endian length prefix + UTF-8 JSON body framing as alknet-call's
  `FrameFramedReader`/`FrameFramedWriter`
  (`crates/alknet-call/src/protocol/wire.rs`). The frame carries the
  terminal parameters and backend selector the server needs to allocate
  the session:

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

  The `carriage` field is `"raw"` for terminal sessions (the only
  carriage in v1). The `tty` block is `null` for the pipe/runner case
  (no PTY — see ADR-054). The `backend` field selects the `TtyBackend`
  (ADR-053); backend-specific fields (e.g., `container` for docker)
  ride alongside.

- **Raw carriage.** After the negotiation frame, the stream switches to
  the chunk format below for the life of the session. There is no
  `call.responded`/`call.completed` — this is not the call protocol.

### 3. Chunk format

```text
[stream_type: u8][length: u32 be][payload bytes]
```

- `stream_type` — fixed set, no negotiation:

  | stream_type | channel | direction | payload |
  |---|---|---|---|
  | 0 | data-in (stdin) | client→server | raw bytes |
  | 1 | data-out (stdout) | server→client | raw bytes |
  | 2 | data-err (stderr) | server→client | raw bytes |
  | 3 | control | bidirectional | JSON control message |

  `stream_type > 3` is a protocol error (`InvalidStreamType`). There is
  no extension escape hatch in the byte — a 5th channel is a wire-format
  change requiring a new ALPN (`alknet/tty/v2` per ADR-006), not a
  negotiated addition to this format.

- `length` — payload length in bytes, u32 big-endian, max 16 MiB. A
  chunk larger than 16 MiB is a protocol error (`ChunkTooLarge`). The
  16 MiB bound accommodates large paste operations and large `env`
  blocks while bounding memory per chunk (a reader can allocate a
  bounded buffer up front); it mirrors the docker POC's limit.

- Zero-length data chunks are sentinels: a zero-length stdin chunk is
  EOF from the client; a zero-length stdout chunk is "drained" from the
  server (output stream ended). Control chunks are never zero-length
  (the JSON payload is at least `{}`).

### 4. Control channel (stream_type 3) carries JSON control messages

Control chunks carry a small JSON payload, tagged by `type`:

| Direction | Message | Shape |
|---|---|---|
| client→server | resize | `{"type":"resize","cols":80,"rows":24,"pixel_width":0,"pixel_height":0}` |
| client→server | signal | `{"type":"signal","name":"INT"}` |
| client→server | eof | `{"type":"eof"}` |
| server→client | exit | `{"type":"exit","code":0}` |

- **resize** — window-size change. Maps to SSH `window-change`, docker
  exec resize, or `ioctl(TIOCSWINSZ)` on a local PTY.
- **signal** — signal forwarding. `name` is an uppercase string
  (`"INT"`, `"TERM"`, `"HUP"`, `"QUIT"`, `"TSTP"`, `"CONT"`, `"KILL"`,
  `"USR1"`, `"USR2"`). Unknown names fall back to the backend's default
  kill (see ADR-053 / `tty-local.md` REQ-TTY-02).
- **eof** — client signals no more stdin. Maps to SSH channel EOF,
  docker stdin close, or `ChildStdin::drop` / PTY writer close. This is
  one of two canonical "stdin done" signals; the other is a zero-length
  stdin chunk (the docker POC's sentinel). Both are accepted as
  equivalent — `eof` is recommended for explicitness (it's a control
  message, not a data-length hack); the zero-length stdin chunk is kept
  for compatibility with the docker POC's pattern. See OQ-47.
- **exit** — server signals process exit with code. This is the last
  control chunk before stream close (see ADR-055).

The `type` tag is the extensibility seam: new control message types are
added by extending the tagged enum. Unknown `type` values are ignored
(not a protocol error) so that a newer client sending a control message
an older server doesn't recognize degrades gracefully rather than
tearing down the session. This is a two-way-door extension point within
the one-way-door wire format — adding a control message type is
additive; changing the chunk header is not.

### 5. Negotiation errors use the JSON framing, not the raw chunk format

If the server cannot allocate the session (unknown backend, PTY
allocation failed, the command is invalid), it sends a JSON error
response in the same 4-byte length-prefixed framing as the negotiation
frame and closes the stream without entering raw mode. The error
response shape is `{"error":"<code>","message":"..."}` (see
[tty-adapter.md](../crates/tty/tty-adapter.md) §"Negotiation errors" for
the codes). This is not `call.error` — this is not the call protocol;
the error is a JSON response in the negotiation framing, and the stream
closes after it.

**Framing disambiguation (success vs error).** Both a successful
allocation (raw chunks) and a failed allocation (JSON error frame) begin
with bytes the client must read before knowing which framing applies.
The disambiguation is by the first byte: a JSON error frame's 4-byte
big-endian length prefix always starts with `0x00` (error frames are
small — under 16 MiB, so the high byte is zero), while a raw chunk's
first byte is a `stream_type` in `{0, 1, 2, 3}`. A stream_type of `0`
(stdin from server) is invalid — the server never sends stdin chunks —
so the client distinguishes: read the first byte; if it is `0x00`,
interpret the next 4 bytes as a big-endian length prefix and read that
many bytes as a JSON error frame; otherwise interpret it as a
`stream_type` byte and continue reading the raw chunk header. This is a
one-way-door wire-format invariant: error frames use the negotiation
framing (length prefix), success uses the raw chunk framing
(stream_type byte first); the `0x00`-as-length-prefix vs
`0x00`-as-invalid-stream_type disambiguation is what makes the two
distinguishable on the wire.

### 6. Negotiation frame reuses alknet-call's framing, not its types

The 4-byte length prefix + JSON body is byte-identical to
`alknet_call::protocol::wire::FrameFramedReader`/`FrameFramedWriter`.
alknet-tty reuses the *framing utility* (the length-prefix read/write),
not the `EventEnvelope` type — the negotiation payload is a
tty-specific struct (`NegotiateRequest`), not a `call.requested` event.
This keeps alknet-tty's dependency on alknet-call limited to the framing
codec, not the call protocol's type system. (See ADR-053 for the
dependency edge and ADR-003 Amendment 1 for the protocol-foundation
exception.)

## Consequences

**Positive:**

- The wire format is POC-validated twice (docker POC for stream_type 0/1/2
  + bidirectional pump; tty POC for stream_type 3 + control messages +
  local PTY). No new wire-format invention in Phase 1.
- The fixed channel set is a `match`, not a hash lookup — fast on the hot
  path where every chunk is data.
- The two-carriage model keeps the call protocol's JSON-RPC shape for the
  structured request while letting the body be raw bytes, which is what a
  terminal actually is. No base64, no per-chunk `EventEnvelope` overhead.
- Control messages as JSON are consistent with the negotiation layer and
  trivially extensible (tagged enum), at negligible cost for rare messages.
- A separate ALPN composes with the ALPN dispatch model (ADR-001/006):
  the endpoint routes `alknet/tty` to the `TtyAdapter`; the call protocol
  is unaffected. Browser terminals (xterm.js over WebTransport, when
  WebTransport revives) connect to `alknet/tty` directly without
  implementing SSH or the call protocol.

**Negative:**

- A 5th channel type is a wire-format change (one-way door). The
  justification is that the use cases are bounded — a terminal session
  has stdin, stdout, stderr, and control. New sideband needs are
  different ALPNs, not 5th channels. If this proves wrong, the reversal
  is a new ALPN string (`alknet/tty/v2`), which coexists with the old one
  rather than replacing it — but every client and server implementing
  the old format would need updating to speak the new one.
- Control as JSON means a `serde_json` deserialize per control chunk.
  Control chunks are rare (one per resize, one per signal, one eof, one
  exit), so this is negligible. A hot control path would warrant a
  binary format — the `type`-tagged enum leaves that door open without a
  wire-format change.
- The negotiation frame is a custom JSON shape, not a `call.requested`
  event, so a client library can't reuse its call-protocol client to open
  a tty session — it speaks the tty wire format directly. This is
  intentional (the tty session is not a call-protocol operation) but
  means the tty client is a separate small client, not a `CallClient`
  method.

## Door type

**One-way.** The chunk header (5 bytes: 1 type + 4 length), the fixed
stream_type set (0-3), and the two-carriage sequence (JSON frame → raw
chunks) are bytes clients and servers parse. Changing any of them breaks
every client and server implementing the format. The reversal path is a
new ALPN (`alknet/tty/v2`), which coexists rather than replaces — but
the cost of migrating every consumer is the one-way-door cost.

The control message `type` enum is a two-way-door extension point within
the one-way wire format: adding a control message type is additive
(unknown types are ignored), changing the meaning of an existing type is
not.

## Assumptions

1. **QUIC per-stream flow control is sufficient for terminal output.**
   The chunk format has no windowing — QUIC's bidi-stream flow control
   handles backpressure. High-throughput stdout (e.g., `cargo build`
   output) is expected to work; a concrete high-volume use case that
   surfaces a flow-control problem would require revisiting. See OQ-45.

2. **The negotiation frame fits in one chunk of the underlying stream's
   initial flow-control window.** The frame is small (terminal params +
   command + env, typically < 4 KiB). QUIC's default initial bidi-window
   (quinn defaults are tens of KiB) accommodates it without a
   flow-control round-trip. A pathological `env` block larger than the
   window would stall until the window opens; the 16 MiB chunk limit is
   the hard cap.

3. **Control messages are rare enough that JSON serialization cost is
   negligible.** Validated by the tty POC: resize on window drag, one
   signal per Ctrl-C, one eof, one exit per session. No measurable cost
   observed.

## References

- `docs/research/alknet-tty/phase-0-findings.md` — Phase 0 research; the
  wire format section is the seed of this ADR
- `docs/research/alknet-docker/poc-summary.md` — the POC that validated
  the raw chunk format (stream_type 0/1/2) and the two-carriage model
- `/workspace/alknet-tty-poc/src/raw.rs` — the chunk codec
  (`ChunkReader`/`ChunkWriter`, stream_type 0-3) this ADR commits
- `/workspace/alknet-tty-poc/src/control.rs` — the JSON control schema
  (`ControlMessage` tagged enum) this ADR commits
- `/workspace/alknet-docker-poc/src/raw.rs` — the seed codec
  (stream_type 0/1/2) the tty POC extended
- [ADR-001](001-alpn-protocol-dispatch.md) — ALPN-based dispatch
- [ADR-002](002-protocol-handler-trait.md) — ProtocolHandler trait
- [ADR-006](006-alpn-convention-and-connection-model.md) — `alknet/<name>`
  ALPN convention; one ALPN per connection; new ALPN for incompatible
  versions
- [ADR-007](007-bistream-type-definition.md) — handler receives a
  `Connection`, accepts bidi streams
- [ADR-003](003-crate-decomposition.md) Amendment 1 — alknet-call as
  protocol-foundation crate (framing utility reuse)
- [ADR-012](012-call-protocol-stream-model.md) — the call protocol's
  stream model (which tty is *not* using for the body, by design)
- [ADR-049](049-streaming-handler-for-subscriptions.md) — the
  `StreamingHandler` path tty explicitly does not use for the byte body
- Spec: [crates/tty/tty-wire.md](../crates/tty/tty-wire.md)