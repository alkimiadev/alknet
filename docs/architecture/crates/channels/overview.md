---
status: draft
last_updated: 2026-07-12
---

# alknet-channels — Overview

## What

`alknet-channels` is a multiplexing proxy crate. It implements
`ProtocolHandler` for the `alknet/channels` ALPN: it receives one
bidirectional transport stream, reads 9-byte chunk headers, and routes each
chunk's payload to the right logical channel. Each channel is reassembled
into an `AsyncRead + AsyncWrite` pair and presented to its handler as a
`Connection` — the handler doesn't know it's inside a channels connection.

Channel 0 is pre-negotiated as `alknet/call` (ADR-072). Every other channel
is opened dynamically via `channel/open` on channel 0 (ADR-073) and routed
through the same `HandlerRegistry` as top-level connections. The channels
layer does no protocol work itself — it is a re-framing proxy that converts
between "one transport stream carrying N channels" (the wire) and "N
independent stream handles" (what handlers see).

## Why

### The problem: three multiplexing models that don't compose

Before channels, alknet had three multiplexing models:

| Model | Where | Mechanism |
|-------|-------|-----------|
| Connection-level | ALPN router | One ALPN per QUIC connection |
| Stream-level | QUIC native | Many bidi streams per connection |
| Sub-stream-level | TTY chunk format | 4 logical channels within one bidi stream |

A docker client needing both JSON call operations and raw TTY sessions
required **two separate QUIC connections** with different ALPNs. The call
protocol can't say "for this operation, open a TTY stream." The hub,
bridging browsers and spokes over multiple transports, faced an
O(protocols × transports × spokes) matrix of per-protocol framing parsers
and per-ALPN connection management.

### The collapse: one multiplexing model, one connection per leg

With `alknet/channels`, one connection carries everything:

```
Browser ──WebTransport──► Hub ──QUIC──► Spoke
          alknet/channels         alknet/channels
          ┌─────────────┐         ┌─────────────┐
          │ ch0: call   │         │ ch0: call   │
          │ ch1: tty    │  relay  │ ch1: tty    │
          │ ch2: ssh    │ ◄─────► │ ch2: ssh    │
          │ ch3: tunnel │         │ ch3: tunnel │
          └─────────────┘         └─────────────┘
```

The hub's relay is channel-by-channel byte forwarding (with `channel_id`
rewrite — ADR-079), not per-protocol framing parsers. The hub's complexity
collapses from O(protocols × transports × spokes) to O(channels).

The collapse is at three levels:

1. **One connection per leg, not one per protocol.** All needs (call, TTY,
   SSH, tunnel) ride as channels on one connection per leg.
2. **One multiplexing model, not three.** Connection-level, stream-level,
   and sub-stream-level all become channels chunks.
3. **The call protocol orchestrates from inside.** Channel 0 is
   `alknet/call` on both legs. The call protocol's `OperationRegistry`,
   `AccessControl`, and `forwarded_for` machinery govern channel lifecycle
   with no new auth.

## Architecture

The crate has two internal components (ADR-075):

- **`ChannelsAdapter`** — implements `ProtocolHandler` for
  `alknet/channels`. Its `handle()` receives one `Connection`, reads 9-byte
  chunk headers, and routes chunks to the `ChannelManager`. The read/demux
  half.
- **`ChannelManager`** — the shared state. Holds `channel_id →
  ChannelState`, the `HandlerRegistry` reference, and the
  `OperationRegistry` reference. The reassemble/allocate half. What the
  `channel/open` operation handler closes over.

Each channel is presented to its handler as a `Connection` constructed via
`Connection::from_source(ChannelBidiStreamSource, alpn)` (ADR-070/074). The
handler calls `accept_bi()` once (yield-once per channel) and drives its
session — identical to how it works on a top-level QUIC connection.

See [channels-adapter.md](channels-adapter.md) for the full adapter/manager
design.

## Crate dependencies

```
alknet-channels-core
├── alknet-core (ProtocolHandler, Connection, HandlerRegistry,
│                BidiStreamSource, SendStream, RecvStream, AuthContext)
├── tokio (spawn, mpsc, io)
├── bytes (Bytes for chunk payloads)
├── async-trait
├── thiserror
└── tracing

alknet-channels-call
├── alknet-channels-core (ChannelManager, ChannelsAdapter,
│                         ChannelBidiStreamSource, ChannelClient)
├── alknet-call (OperationRegistry, HandlerKind, make_handler,
│                make_streaming_handler, CallError, ResponseEnvelope)
└── tokio

alknet-hub (the existing hub crate — consumes channels)
├── alknet-channels-call
└── alknet-call (from_call, CallAdapter, forwarded_for — ADR-079)

worker crates (any crate that dials a hub — consumes channels)
└── alknet-channels-call (ChannelClient — ADR-080)
```

`alknet-channels-core` is the pure multiplexer — wire format, demux/mux,
`ChannelBidiStreamSource`, `ChannelManager`. It depends on `alknet-core`
only. No `alknet-call` dependency. ALPN-blind, call-protocol-blind,
transport-blind. This is where the "streams are streams" insight lives.

`alknet-channels-call` is the call-protocol coupling — channel 0
pre-negotiation as `alknet/call` (ADR-072), the four lifecycle operations
(ADR-073) registered on the call protocol's `OperationRegistry`, and
`ChannelClient` (ADR-080). This is where the call-protocol coupling lives,
isolated from the pure multiplexer.

The hub and worker are **consumers**, not sub-crates. The existing
`alknet-hub` crate IS the channels hub — it depends on `channels-call` and
uses channels as its substrate, with the relay logic (ADR-079) living in
`alknet-hub` alongside its existing peer lifecycle and service discovery
responsibilities. A worker is any crate that uses `ChannelClient` to dial.
There are no `channels-hub` or `channels-worker` sub-crates.

See ADR-081 for the full decomposition rationale.

## ALPN

`alknet/channels` — the ALPN the `ChannelsAdapter` registers on. One ALPN
per channels connection; the connection carries N logical channels, each
with its own ALPN (negotiated via `channel/open`).

## Transport agnosticism

The channels wire format works over any ordered, reliable bidirectional byte
stream:

| Transport | How |
|-----------|-----|
| QUIC bidi stream | `alknet/channels` ALPN on a QUIC connection; one bidi stream carries all channels |
| TCP+TLS | `alknet/channels` ALPN on a TLS connection; the TCP stream carries all channels |
| WebTransport | `alknet/channels` session (deferred per ADR-044; the browser path uses WebSocket carrying `alknet/channels`) |
| SSH channel | channels connection riding inside an SSH `direct-tcpip` channel (channels-over-SSH) |
| Another channels connection | recursive composition (channel type `alknet/channels` inside `alknet/channels`) |

The same wire format, the same chunk reassembly, the same `Connection`
abstraction. The transport is a parameter, not a design constraint.
`Connection::from_stream` / `from_source` (ADR-065/070) handles the
transport-agnostic `Connection` construction.

## WASM compatibility

The wire format's core is pure byte manipulation — `parse_header` /
`write_header` are pure functions with no platform dependencies. The de-risk
POC validated the sync core compiles under `wasm32-unknown-unknown`. The
async shell (demux/mux) wraps this core with `read_exact`/`write_all` and
`mpsc` routing.

The `ChannelManager` is ALPN-blind, auth-blind, and transport-blind (ADR-
075) — pure byte routing with no platform or protocol dependencies. A WASM
build can read chunks from a WebTransport `BiStream`, reassemble them, and
present `AsyncRead + AsyncWrite` handles to WASM-compatible handlers. The
handlers themselves may or may not be WASM-compatible (russh's client is;
`portable_pty` is not), but the channels layer is WASM-compatible by
construction.

The async shell and `alknet-core` dep graph are not fully WASM-clean yet
(transitive `getrandom`/`rand` deps) — this is an implementation concern,
not an architecture concern. The sync core's WASM compatibility is validated.

## Relationship to existing crates

### alknet-call

Unchanged. The call protocol remains JSON-only, `EventEnvelope`-based. It
runs on channel 0 exactly as on a top-level `alknet/call` connection. The
`CallAdapter` receives a `Connection` backed by channel-0 chunk reassembly
and dispatches operations — it doesn't know it's inside channels.

What changes: the call protocol gains a new class of operations — channel
lifecycle (ADR-073). These are registered on the `OperationRegistry` at
assembly time and dispatched through the existing `OperationContext` /
`AccessControl::check` path.

### alknet-tty

The TTY crate gains a `channels` feature (ADR-077) that enables
inside-channels mode. In direct mode (`alknet/tty` ALPN on a top-level
connection), the TTY adapter uses its own 5-byte wire format (ADR-052,
unchanged). In channels mode (`channel/open` with ALPN `alknet/tty`), the
adapter receives `ChannelSubStreams` (ADR-074) — four named
`SendStream`/`RecvStream` pairs for stream_types 0-3 — and pumps without
chunk parsing. The `TtyBackend` trait and `TtyHandle` are unchanged;
backends don't know which mode the adapter is in.

### alknet-ssh (future)

SSH as a channel type: an `alknet/ssh` channel carries the SSH binary
protocol over stream_types 0 and 1. The channels layer hands the
reassembled stream to `SshAdapter`, which feeds it to russh. SSH as a
channels transport: an SSH `direct-tcpip` channel could carry a channels
connection (channels-over-SSH). The SSH crate doesn't need to know about
channels — it implements `ProtocolHandler` for `alknet/ssh` and accepts a
`Connection`.

### alknet-docker

Docker lifecycle operations are call operations on channel 0 (unchanged
from ADR-058). Interactive exec/attach opens a TTY channel via
`channel/open` with ALPN `alknet/tty` and backend `docker`. No separate
`alknet/tty` connection needed — one `alknet/channels` connection handles
both JSON operations and raw TTY sessions.

### alknet-hub

The hub is the primary consumer. With channels, the hub holds one channels
connection per leg (browser↔hub, hub↔spoke) and relays channels between
them. The hub translates `channel/open` on channel 0 (re-issues on the
spoke leg with `forwarded_for` — ADR-079) and byte-forwards data channels
with `channel_id` rewrite. The hub's complexity collapses from
O(protocols × transports × spokes) to O(channels).

## Design Decisions

All design decisions are documented as ADRs in [decisions/](../../decisions/).

| ADR | Decision | Summary |
|-----|----------|---------|
| [071](../../decisions/071-channels-wire-format.md) | channels Wire Format | 9-byte chunk header; unidirectional stream_types in groups of 3; one-way door |
| [072](../../decisions/072-channel-0-pre-negotiated-call.md) | Channel 0 Pre-Negotiated | Channel 0 = `alknet/call`, stream_types [0,1] |
| [073](../../decisions/073-channel-lifecycle-operations.md) | Channel Lifecycle Operations | `channel/open`/`close`/`control`/`resources/subscribe`; subscribe not poll; `direction` pinned |
| [074](../../decisions/074-channelconnection-bidistreamsource.md) | ChannelConnection | Per-channel `BidiStreamSource`; `into_sub_streams()` with `SubStreamHandle` enum |
| [075](../../decisions/075-channelsadapter-and-channelmanager.md) | ChannelsAdapter and ChannelManager | Substrate-agnostic demux loop; REQ-CH-01..04 |
| [076](../../decisions/076-backpressure-channel-limits-id-reuse.md) | Backpressure, Limits, ID Reuse | Bounded-buffer (1 MiB), 256-channel cap, monotonic IDs |
| [077](../../decisions/077-tty-inside-channels.md) | TTY Inside Channels | Two modes (direct vs channels); 5 sub-streams; control bidirectional via 3/4 |
| [078](../../decisions/078-two-pump-shutdown-on-completion.md) | Two-Pump Pattern | Shutdown-on-completion contract; handler-level |
| [079](../../decisions/079-hub-relay-translate-not-forward.md) | Hub Relay | Translate channel 0, byte-forward data channels with ID rewrite |
| [080](../../decisions/080-channelclient.md) | ChannelClient | Client side; transport-agnostic `from_connection` primary, `connect_quic` convenience; `AlknetClient` dial-seam deferred (OQ-55) |
| [081](../../decisions/081-channels-subcrate-decomposition.md) | Sub-Crate Decomposition | `channels-core` (pure multiplexer) / `channels-call` (call coupling + ChannelClient); hub and worker are consumers |

## Open Questions

Open questions are tracked in [open-questions.md](../../open-questions.md).
Key questions affecting this crate:

- **OQ-55** (deferred(scope)): `AlknetClient` core **dial+TLS seam**
  extraction — blocked on a second *transport's* dial. `ChannelClient`'s
  API is transport-agnostic (`from_connection`); `AlknetClient` is the
  shared *dial* across transports, not the channels protocol.
- **OQ-56** (deferred(scope)): Full channel-level flow-control windowing —
  bounded-buffer is decided (ADR-076); full windowing is an extension
  blocked on a real HOL-blocking deployment observation.
- **OQ-57** (deferred(scope)): Two-pump helper extraction to alknet-core —
  the *contract* is decided (ADR-078); the *helper* is blocked on a second
  two-pump handler existing.