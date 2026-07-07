---
status: draft
last_updated: 2026-07-07
---

# alknet-tty

Terminal session protocol handler for the ALPN-as-service architecture:
a `ProtocolHandler` on `alknet/tty` that pumps a bidirectional byte stream
(stdin/stdout/stderr) with a JSON control channel (resize, signal, eof,
exit) over a framed bidi stream, decoupled from the backend that allocates
the PTY (docker, SSH, local process) via a `TtyBackend` trait.

## Documents

| Document | Status | Description |
|----------|--------|-------------|
| [overview.md](overview.md) | draft | Crate purpose, the two-carriage model in brief, dependencies, ALPN, backend location map, feature gates |
| [tty-wire.md](tty-wire.md) | draft | The wire format: negotiation frame (JSON carriage), raw chunk codec (`[stream_type: u8][length: u32 be][payload]`), control channel (stream_type 3, JSON control messages), sentinels |
| [tty-backend.md](tty-backend.md) | draft | `TtyBackend` trait, `TtyParams`, `TtyHandle`, `TtyControl` — the inversion point between the wire-format adapter and the backends. Carries REQ-TTY-01 (backends need not be natively async) |
| [tty-adapter.md](tty-adapter.md) | draft | `TtyAdapter` (`ProtocolHandler` on `alknet/tty`): session lifecycle, three-pump bidirectional driver, negotiation errors, exit-chunk ordering (ADR-055), access control |
| [tty-local.md](tty-local.md) | draft | `alknet-tty-local` sibling crate: `LocalTtyBackend` via `portable_pty` (PTY) and `std::process::Command` (pipe/runner). Carries REQ-TTY-02 (signal forwarding to the process group) |

## Applicable ADRs

| ADR | Title | Relevance |
|-----|-------|-----------|
| [001](../../decisions/001-alpn-protocol-dispatch.md) | ALPN-Based Protocol Dispatch | `TtyAdapter` registers on `alknet/tty` |
| [002](../../decisions/002-protocol-handler-trait.md) | ProtocolHandler Trait | `TtyAdapter` implements `ProtocolHandler` |
| [003](../../decisions/003-crate-decomposition.md) | Crate Decomposition | alknet-tty depends on alknet-core; backends depend on alknet-tty (Amendment 1: alknet-call as protocol-foundation, framing utility reuse) |
| [006](../../decisions/006-alpn-convention-and-connection-model.md) | ALPN String Convention and Connection Model | `alknet/tty` is the custom ALPN; one ALPN per connection; new ALPN for incompatible versions |
| [007](../../decisions/007-bistream-type-definition.md) | BiStream Type Definition | `TtyAdapter` receives a `Connection`, accepts bidi streams, pumps per-session |
| [009](../../decisions/009-one-way-door-decision-framework.md) | One-Way Door Decision Framework | Wire format is one-way; local backend placement is two-way (decided, not deferred) |
| [012](../../decisions/012-call-protocol-stream-model.md) | Call Protocol Stream Model | The call protocol's stream model — which tty's raw carriage is *not* using for the body, by design |
| [032](../../decisions/032-forwarded-for-identity.md) | Forwarded-For Identity | `forwarded_for` for proxied terminal sessions (hub→worker) |
| [040](../../decisions/040-webtransport-alpn-stream-proxy.md) | WebTransport ALPN-Stream-Proxy | **Parked** per ADR-044; the `alknet/tty` ALPN is reachable over WebTransport's stream proxy when WebTransport revives |
| [044](../../decisions/044-defer-webtransport-browsers-use-websocket.md) | Defer h3/WebTransport; Browsers Use WebSocket | WebTransport deferred; the browser terminal case revives with WebTransport |
| [049](../../decisions/049-streaming-handler-for-subscriptions.md) | Streaming Handler for Subscriptions | The `StreamingHandler` path tty explicitly does *not* use for the byte body — raw carriage, not `call.responded` events |
| [050](../../decisions/050-dynamic-resource-ownership-for-runtime-spawned-resources.md) | Dynamic Resource Ownership | Terminal sessions are runtime-spawned resources; `AccessControl` shape declares against this model |
| [052](../../decisions/052-alknet-tty-wire-format-and-two-carriage.md) | alknet-tty Wire Format and Two-Carriage Model | The chunk codec, control channel, negotiation frame, fixed channel set |
| [053](../../decisions/053-ttybackend-trait-and-ttyhandle.md) | TtyBackend Trait and TtyHandle | The backend trait, handle shape, blocking-backend accommodation (REQ-TTY-01) |
| [054](../../decisions/054-local-tty-backend-sibling-crate.md) | Local TTY Backend as a Sibling Crate | `alknet-tty-local` behind a `local` feature re-export; PTY vs pipe per-session |
| [055](../../decisions/055-exit-code-on-control-chunk.md) | Exit Code on a Control Chunk | Exit code on stream_type 3; "exit chunk is last" invariant; adapter owns the ordering |
| [056](../../decisions/056-backend-cleanup-on-session-cancel.md) | Backend Cleanup on Session Cancel | Dropping the `exit_code` future (session cancel) MUST kill the session target; behavioral contract on the `TtyBackend` trait |

## Relevant Open Questions

| OQ | Title | Status | Relevance |
|----|-------|--------|-----------|
| OQ-43 | `TtyControl` trait object `Clone` constraint | resolved | `control: Option<TtyControlHandle>` via a `#[derive(Clone)]` newtype wrapping `Arc<dyn TtyControl + Send + Sync>`; the trait is NOT `Clone` (not object-safe) — the newtype carries `Clone`-ability; confirmed by the POC's concrete `PtyControl` |
| OQ-44 | Terminal modes (TTY modes) | deferred(scope) | `TerminalParams.modes` reserved; default terminal modes suffice for current scope; blocked on a concrete mode-control use case |
| OQ-45 | Flow control for high-throughput stdout | resolved | QUIC per-stream flow control is the backpressure mechanism (chain complete by construction); no application-level windowing. Reversal is an additive `ControlMessage` variant, not a wire-format change |
| OQ-46 | Runner API surface | deferred(scope) | The runner mechanism (pipe mode) is in alknet-tty; runner policy (job management, log persistence, task graph) is a downstream crate, not in scope here |
| OQ-47 | Stdin closure canonical signal | resolved | Either a zero-length stdin chunk or a `{"type":"eof"}` control chunk; both are accepted; the spec recommends `eof` for explicitness |

## Key Design Principles

1. **A terminal session is a terminal concern, not an SSH or Docker
   concern.** SSH and Docker are two backends that can allocate a PTY.
   alknet-tty owns the terminal session lifecycle; the backends
   (`DockerTtyBackend`, `SshTtyBackend`, `LocalTtyBackend`) implement a
   `TtyBackend` trait. This dissolves the PTY hedge in the alknet-ssh
   research (DP-5): PTY is not an SSH feature delegated to a separate
   crate, it's a tty feature that SSH happens to be able to provide. See
   [overview.md](overview.md) and [ADR-053](../../decisions/053-ttybackend-trait-and-ttyhandle.md).

2. **Two-carriage model: JSON negotiation, then raw chunks.** The bidi
   stream opens with a single length-prefixed JSON negotiation frame
   (terminal params, backend selector, command), then switches to a raw
   chunk format (`[stream_type: u8][length: u32 be][payload]`) for the
   life of the session. The call protocol's JSON-RPC shape handles the
   structured request; raw bytes handle the body, which is what a
   terminal actually is. No per-chunk `EventEnvelope` overhead, no
   base64. See [tty-wire.md](tty-wire.md) and
   [ADR-052](../../decisions/052-alknet-tty-wire-format-and-two-carriage.md).

3. **Fixed channel set, not extensible.** Four stream types (0=stdin,
   1=stdout, 2=stderr, 3=control), no negotiation. A 5th channel type
   is a wire-format change (one-way door); the ALPN model handles
   extensibility at the protocol level (a new ALPN is cheap, a
   wire-format change is not). The impoverishment vs SSH channels is
   the feature: alknet-tty multiplexes *one* service (a terminal
   session) with a fixed channel structure, not *arbitrary* services.
   See [tty-wire.md](tty-wire.md).

4. **The backend trait is the inversion point.** alknet-tty defines
   `TtyBackend`; the backend crates implement it. alknet-tty depends on
   alknet-core; backends depend on alknet-tty for the trait; alknet-tty
   does not depend on any backend. This preserves ADR-003's
   no-handler-depends-on-another-handler rule (Amendment 1 for the
   alknet-call framing utility reuse). See
   [tty-backend.md](tty-backend.md) and
   [ADR-053](../../decisions/053-ttybackend-trait-and-ttyhandle.md).

5. **Backends need not be natively async (REQ-TTY-01).** The trait's
   adapter-facing types (`AsyncWrite`, `Stream<Item = Bytes>`,
   `BoxFuture`, `TtyControl`) are the adapter's contract. A backend may
   expose blocking handles internally and bridge them via std threads +
   tokio mpsc/oneshot (the pattern `portable_pty` requires, and the
   local-PTY POC validated). The bridging pattern is a documented,
   supported implementation strategy. See
   [tty-backend.md](tty-backend.md) and [tty-local.md](tty-local.md).

6. **Exit code on a control chunk, last before stream close (ADR-055).**
   `{"type":"exit","code":N}` rides on the control channel (stream_type
   3) and is the last chunk before the server closes the write half.
   This gives coordinators deterministic completion notification — no
   polling, no plugin state. The adapter owns the ordering; backends
   resolve `exit_code` and the adapter awaits, sends the chunk, closes.
   See [tty-adapter.md](tty-adapter.md) and
   [ADR-055](../../decisions/055-exit-code-on-control-chunk.md).

7. **The runner pattern is preserved, not specialized.** The local
   backend in pipe mode (`terminal: None`) is a process-streaming
   endpoint — the same shape as GitHub/Gitea Actions runners, just over
   alknet's transport instead of HTTP polling. alknet-tty provides the
   *mechanism* (framed byte stream + exit code); runner *policy* (job
   management, log persistence, task graph) is a downstream crate's
   job. See [tty-local.md](tty-local.md) and
   [ADR-054](../../decisions/054-local-tty-backend-sibling-crate.md).

## References

- `docs/research/alknet-tty/phase-0-findings.md` — Phase 0 research
  (wire format, backend trait, REQ-TTY-01/02, decision points DP-1
  through DP-6, open questions OQ-TTY-01 through OQ-TTY-05)
- `/workspace/alknet-tty-poc/` — Phase 0 local-PTY validation POC
  (`src/raw.rs` chunk codec, `src/control.rs` JSON control schema,
  `src/local_pty.rs` blocking→async bridge, `src/session.rs` session
  pump, `tests/integration.rs` + `tests/signal.rs` round-trip tests)
- `/workspace/alknet-docker-poc/src/raw.rs` — the seed codec
  (stream_type 0/1/2) the tty POC extended with stream_type 3
- `docs/research/alknet-docker/poc-summary.md` — the POC that seeded
  this crate (two-carriage model, raw chunk format, validated targets)
- `docs/research/alknet-ssh/phase-0-findings.md` DP-5 — the PTY hedge
  this crate dissolves (PTY is a tty feature, not an SSH feature)
- `/workspace/@alkdev/dispatch/` — the reverse-runner prior art
  (currently requires SSH; `LocalTtyBackend` removes that requirement)
- `portable-pty` 0.9 source — the blocking-API constraint that drives
  REQ-TTY-01 and the signal-delivery contract (REQ-TTY-02)