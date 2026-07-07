---
status: draft
last_updated: 2026-07-07
---

# alknet-tty — Overview

The terminal session protocol handler: a `ProtocolHandler` on `alknet/tty`
that pumps a bidirectional byte stream (stdin/stdout/stderr) with a JSON
control channel (resize, signal, eof, exit) over a framed bidi stream,
decoupled from the backend that allocates the PTY via a `TtyBackend`
trait. This document covers the crate's purpose, the two-carriage model
in brief, its dependency edges, the ALPN, and the backend location map.
Component details are in the sibling documents.

## What

`alknet-tty` is the terminal session protocol handler for the
ALPN-as-service architecture (ADR-001). It registers the `alknet/tty`
ALPN on the shared `AlknetEndpoint` and implements the `ProtocolHandler`
trait (ADR-002, ADR-007). The `TtyAdapter` receives a `Connection`,
accepts one bidi stream per terminal session, reads a single JSON
negotiation frame, switches to a raw chunk format, and pumps bytes
bidirectionally for the life of the session — backend-agnostic.

The guiding insight that shapes the crate:

> A terminal session is not an SSH concern, or a Docker concern — it is
> a terminal concern. SSH and Docker are just two backends that can
> allocate a PTY.

The alknet-docker POC (`docs/research/alknet-docker/poc-summary.md`)
proved that the hard part of interactive attach — bidirectional byte
pumping over a framed stream with a 1-byte stream-type multiplexer — is
the same problem regardless of whether the backend is
`bollard::attach_container()` or russh's `pty_request`. The POC's raw
chunk format is the seed of alknet-tty's wire format. alknet-tty
extracts that pattern into its own crate and ALPN; the backends (Docker,
SSH, local process) implement a `TtyBackend` trait; the `alknet/tty`
handler is backend-agnostic. This dissolves the PTY hedge in the
alknet-ssh research (DP-5): PTY is not an SSH feature delegated to a
separate crate, it's a tty feature that SSH happens to be able to
provide.

## Why

The crate's purpose is to be the terminal session library for downstream
consumers. A hub that runs agent workspaces in containers wires
`DockerTtyBackend` into the `TtyAdapter` and gets interactive terminal
sessions over `alknet/tty`. A coordinator that runs `cargo test`
remotely wires `LocalTtyBackend` (pipe mode) and gets the runner pattern
(a process whose stdin/stdout/stderr/exit-code stream over a framed bidi
connection) — the same shape as GitHub/Gitea Actions runners, just over
alknet's transport instead of HTTP polling. A browser terminal (xterm.js
over WebTransport, when WebTransport revives) connects to `alknet/tty`
directly and gets raw bytes without implementing SSH or the call
protocol.

The key architectural insight: **the wire format and the backends
invert at the `TtyBackend` trait.** alknet-tty owns the wire format, the
negotiation frame, the chunk codec, the control channel, and the
session lifecycle; the backends own the PTY allocation (docker exec
with `tty: true`, russh `pty_request` + `shell_request`,
`portable_pty::openpty`). The adapter is backend-agnostic and testable
with a mock backend (in-memory pipes). See
[ADR-053](../../decisions/053-ttybackend-trait-and-ttyhandle.md).

## The Two-Carriage Model in Brief

A `alknet/tty` bidi stream has two phases (full detail in
[tty-wire.md](tty-wire.md), decided in
[ADR-052](../../decisions/052-alknet-tty-wire-format-and-two-carriage.md)):

1. **Negotiation (JSON carriage).** The client writes a single
   length-prefixed JSON frame carrying the terminal parameters, backend
   selector, command, and environment. The framing is byte-identical to
   alknet-call's `FrameFramedReader`/`FrameFramedWriter` (the utility is
   reused, not the `EventEnvelope` type — see Dependencies below).

2. **Raw carriage.** After the negotiation frame, the stream switches to
   the chunk format (`[stream_type: u8][length: u32 be][payload]`) for
   the life of the session. Four stream types: 0=stdin (client→server),
   1=stdout (server→client), 2=stderr (server→client), 3=control
   (bidirectional, JSON control messages: resize, signal, eof, exit).
   There is no `call.responded`/`call.completed` — this is not the call
   protocol; the raw-carriage byte pump is its own wire format after the
   single JSON negotiation frame.

This is the pattern the docker POC validated and the SSH research
independently arrived at: JSON for the structured request, raw bytes for
the body, which is the part that is actually bytes. The full rationale
(why not JSON for everything; the two-carriage decision) is in
[ADR-052](../../decisions/052-alknet-tty-wire-format-and-two-carriage.md)
§Context.

## Dependencies

```
alknet-tty
├── alknet-core   (ProtocolHandler, Connection, AuthContext, Identity, AccessControl,
│                  OwnershipProvider — ADR-050 for terminal sessions as resources)
├── alknet-call   (FrameFramedReader/FrameFramedWriter — framing utility reuse only,
│                  NOT EventEnvelope or the call protocol types; ADR-003 Amendment 1)
└── (no backend deps — portable_pty, bollard, russh are in the backend crates)
```

alknet-tty is dependency-light: alknet-core (the handler interface and
auth) and alknet-call's framing codec (the length-prefix utility for the
negotiation frame). The heavy backend dependencies (`portable_pty`,
`bollard`, `russh`) live in the backend crates, not here.

### The `alknet-call` dependency (ADR-003 Amendment 1)

alknet-tty depends on alknet-call for the `FrameFramedReader`/
`FrameFramedWriter` utility — the 4-byte length prefix + JSON body
framing the negotiation frame uses. This is a *framing utility* reuse,
not a dependency on the call protocol's type system: the negotiation
payload is a tty-specific struct (`NegotiateRequest`), not a
`call.requested` `EventEnvelope`. ADR-003's rule is "no handler crate
depends on another handler crate," but `alknet-call` is both a handler
(it implements `ProtocolHandler` on `alknet/call`) *and* the
protocol-foundation crate. alknet-tty depending on alknet-call is "tty
uses the call protocol's framing codec," not "tty depends on SSH." See
[ADR-003 Amendment 1](../../decisions/003-crate-decomposition.md).

alknet-call stays lean — it has no `portable_pty`, no `bollard`, no
backend deps. The `TtyBackend` implementations are opaque
`Arc<dyn TtyBackend>` from the adapter's perspective: constructed by
the assembly layer at startup, stored in the adapter's backend map,
dispatched by the `backend` field of the negotiation frame.

## ALPN

| ALPN | Handler | Transport | Browser? |
|------|---------|-----------|----------|
| `alknet/tty` | `TtyAdapter` | QUIC bidi stream | Yes (when WebTransport revives — ADR-040 parked) |

`alknet/tty` is a custom ALPN per the ADR-006 `alknet/<name>` convention.
The `TtyAdapter` registers for it; the endpoint's `HandlerRegistry` maps
`alknet/tty` to the adapter instance. One ALPN per connection (ADR-006);
within a connection, multiple bidi streams carry independent sessions (one
session per stream — see [tty-adapter.md](tty-adapter.md)).

The browser terminal case: a browser (xterm.js) connects via WebTransport
to `alknet/tty` and gets raw bytes. The browser doesn't need to implement
SSH or the call protocol for the terminal use case — only if it wants
SSH-specific features (port forwarding, SFTP). This is a cleaner browser
story than "run a WASM SSH client." WebTransport is deferred per
[ADR-044](../../decisions/044-defer-webtransport-browsers-use-websocket.md);
when it revives, the `alknet/tty` ALPN is reachable over WebTransport's
ALPN-stream-proxy (ADR-040, parked). See OQ-38 for the WebTransport
relay scope question (unrelated to tty's own scope).

## Backend Location Map

The decomposition principle (the same as `alknet-http`'s adapter location
map): the trait lives where the types live (`alknet-tty`); the
implementations live where their transport dependencies live.

```
alknet-tty (lean — no portable_pty, no bollard, no russh)
├── TtyBackend trait             (the contract — ADR-053)
├── TtyHandle, TtyControl        (the handle shape backends produce)
├── TtyParams, TerminalParams    (the allocation request)
├── TtyAdapter                   (ProtocolHandler on alknet/tty — session lifecycle)
├── wire format                  (ChunkReader/ChunkWriter, ControlMessage — ADR-052)
└── negotiation framing          (reuses alknet-call's FrameFramedReader/Writer)

alknet-tty-local (sibling crate — ADR-054; behind alknet-tty's `local` feature re-export)
├── LocalTtyBackend              (impl TtyBackend — portable_pty for PTY, std::process for pipe)
├── portable_pty dependency      (PTY allocation — the heavy dep, here not in alknet-tty)
└── libc                         (signal forwarding — REQ-TTY-02, Unix only)

alknet-docker (or alknet-tty-docker adapter — future crate, out of scope here)
└── DockerTtyBackend             (impl TtyBackend — wraps bollard::attach_container / exec with tty:true)

alknet-ssh (future crate — out of scope here)
└── SshTtyBackend                (impl TtyBackend — wraps russh pty_request + shell_request/exec_request)
```

alknet-tty never sees `portable_pty`, `bollard`, or `russh`. The backend
implementations are opaque `Arc<dyn TtyBackend>` from the adapter's
perspective. alknet-tty stays lean; the backend crates own their
transport dependencies. The local backend's crate placement (sibling
crate behind a feature re-export) is decided in
[ADR-054](../../decisions/054-local-tty-backend-sibling-crate.md); the
docker and SSH backends are future crates (out of scope for this spec
set — see [tty-backend.md](tty-backend.md) §"Backend implementations"
for where they live).

## Feature Gates

```toml
# alknet-tty Cargo.toml
[features]
default = []
local = ["dep:alknet-tty-local"]   # re-export LocalTtyBackend from alknet-tty-local
```

- `default` — the wire format, `TtyAdapter`, and the `TtyBackend` trait.
  No backend implementations; the assembly layer registers backends
  from their own crates. A docker-only or ssh-only deployment uses the
  default features and depends on `alknet-docker` / `alknet-ssh` (or
  their own backend crate) directly.
- `local` — re-export `alknet_tty_local::LocalTtyBackend` as
  `alknet_tty::local::LocalTtyBackend`. Pulls in `alknet-tty-local`
  (which pulls in `portable_pty`). A consumer that wants the local
  backend (terminal or runner) enables this feature.

The local backend's `portable_pty` dependency is the heavy dep that
motivates the feature gate — a docker-only deployment should not pull in
PTY allocation code. See [ADR-054](../../decisions/054-local-tty-backend-sibling-crate.md).

## Architecture (component pointers)

- **[tty-wire.md](tty-wire.md)** — the wire format: the negotiation
  frame (JSON carriage, reusing alknet-call's framing), the raw chunk
  codec (`[stream_type: u8][length: u32 be][payload]`), the four
  stream types, the control channel (stream_type 3, JSON control
  messages), sentinels, and the fixed-channel-set rationale.
- **[tty-backend.md](tty-backend.md)** — the `TtyBackend` trait,
  `TtyParams`, `TtyHandle`, `TtyControl`. The inversion point between
  the wire-format adapter and the backends. Carries REQ-TTY-01 (backends
  need not be natively async; the bridging pattern is a documented
  strategy). Notes where the docker/SSH backend crates live (future,
  out of scope here).
- **[tty-adapter.md](tty-adapter.md)** — the `TtyAdapter`
  (`ProtocolHandler` on `alknet/tty`): the session lifecycle, the
  three-pump bidirectional driver (stdout→client, client→backend,
  exit→exit-chunk), negotiation errors, the exit-chunk ordering
  (ADR-055), access control (terminal sessions as runtime-spawned
  resources per ADR-050).
- **[tty-local.md](tty-local.md)** — the `alknet-tty-local` sibling
  crate: `LocalTtyBackend` via `portable_pty` (PTY mode) and
  `std::process::Command` (pipe/runner mode). Carries REQ-TTY-02
  (signal forwarding to the foreground process group). The
  blocking→async bridge pattern (the three std threads feeding tokio
  mpsc/oneshot) is the reference for any future blocking-API backend.

## Design Decisions

| Decision | ADR | Summary |
|----------|-----|---------|
| Wire format and two-carriage model | [ADR-052](../../decisions/052-alknet-tty-wire-format-and-two-carriage.md) | `alknet/tty` ALPN; JSON negotiation frame then raw chunks; fixed channel set 0-3; control as JSON |
| `TtyBackend` trait and `TtyHandle` | [ADR-053](../../decisions/053-ttybackend-trait-and-ttyhandle.md) | The backend inversion point; `exit_code` as `Future`; backends need not be natively async (REQ-TTY-01) |
| Local backend as a sibling crate | [ADR-054](../../decisions/054-local-tty-backend-sibling-crate.md) | `alknet-tty-local` behind a `local` feature re-export; PTY vs pipe per-session |
| Exit code on a control chunk | [ADR-055](../../decisions/055-exit-code-on-control-chunk.md) | `{"type":"exit","code":N}` on stream_type 3; "exit chunk is last" invariant; adapter owns the ordering |
| Backend cleanup on session cancel | [ADR-056](../../decisions/056-backend-cleanup-on-session-cancel.md) | Dropping `exit_code` future (cancel) kills the session target; the adapter triggers it by dropping the `TtyHandle` |
| ALPN-based protocol dispatch | [ADR-001](../../decisions/001-alpn-protocol-dispatch.md) | `TtyAdapter` registers on `alknet/tty` |
| ProtocolHandler trait | [ADR-002](../../decisions/002-protocol-handler-trait.md) | `TtyAdapter` implements `ProtocolHandler` |
| Crate decomposition | [ADR-003](../../decisions/003-crate-decomposition.md) Am. 1 | alknet-tty depends on alknet-core + alknet-call (framing utility); backends depend on alknet-tty for the trait |
| ALPN string convention | [ADR-006](../../decisions/006-alpn-convention-and-connection-model.md) | `alknet/tty` is the custom ALPN; new ALPN for incompatible versions |
| BiStream type definition | [ADR-007](../../decisions/007-bistream-type-definition.md) | `TtyAdapter` receives a `Connection`, accepts bidi streams |
| Call protocol stream model (not used for body) | [ADR-012](../../decisions/012-call-protocol-stream-model.md) | The raw carriage is *not* the call protocol's `EventEnvelope` streaming — by design |
| Forwarded-for identity | [ADR-032](../../decisions/032-forwarded-for-identity.md) | `forwarded_for` for proxied terminal sessions (hub→worker) |
| WebTransport ALPN-stream-proxy (parked) | [ADR-040](../../decisions/040-webtransport-alpn-stream-proxy.md) | **Parked** per ADR-044; `alknet/tty` reachable over WebTransport's stream proxy when WebTransport revives |
| Defer h3/WebTransport | [ADR-044](../../decisions/044-defer-webtransport-browsers-use-websocket.md) | WebTransport deferred; the browser terminal case revives with WebTransport |
| Streaming handler (not used for body) | [ADR-049](../../decisions/049-streaming-handler-for-subscriptions.md) | The `StreamingHandler` path tty explicitly does *not* use for the byte body |
| Dynamic resource ownership | [ADR-050](../../decisions/050-dynamic-resource-ownership-for-runtime-spawned-resources.md) | Terminal sessions are runtime-spawned resources; `AccessControl` shape declares against this model |

## Open Questions

See [open-questions.md](../../open-questions.md) for full details.

- **OQ-43** (resolved): `TtyControl` as a `Clone` trait object.
- **OQ-44** (deferred(scope)): Terminal modes (TTY modes).
- **OQ-45** (resolved): Flow control for high-throughput stdout — no application-level windowing; QUIC per-stream flow control is the backpressure mechanism.
- **OQ-46** (deferred(scope)): Runner API surface.
- **OQ-47** (resolved): Stdin closure canonical signal.

## References

- `docs/research/alknet-tty/phase-0-findings.md` — Phase 0 research
- `/workspace/alknet-tty-poc/` — Phase 0 local-PTY validation POC
  (the reference implementation for REQ-TTY-01 and REQ-TTY-02)
- `/workspace/alknet-docker-poc/src/raw.rs` — the seed codec
  (stream_type 0/1/2) the tty POC extended with stream_type 3
- `docs/research/alknet-docker/poc-summary.md` — the POC that seeded
  this crate
- `docs/research/alknet-ssh/phase-0-findings.md` DP-5 — the PTY hedge
  this crate dissolves
- `/workspace/@alkdev/dispatch/` — the reverse-runner prior art
  (currently requires SSH; `LocalTtyBackend` removes that requirement)
- `portable-pty` 0.9 source — the blocking-API constraint that drives
  REQ-TTY-01 and the signal-delivery contract (REQ-TTY-02)