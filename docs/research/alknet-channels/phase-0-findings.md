---
status: draft
last_updated: 2026-07-12
---

# alknet-channels — Phase 0 Research Findings

This document captures Phase 0 (Exploration) findings for the `alknet-channels`
crate. The objective of Phase 0 per `docs/sdd_process.md` is: *"Capture vision
and guiding principles; research options; validate approaches; converge on a
recommended approach."* It is the input to Phase 1 (Architecture), where the
Architect will produce `docs/architecture/crates/channels/*.md` specs, ADRs,
and open questions.

This document was drafted 2026-07-10, emerging from a discussion about the
conceptual tangle between the call protocol, the TTY crate, and the coming
docker and SSH crates. The core issue: most "call clients" will use more than
just the call protocol, and not everything is well described by JSON. The TTY
crate's chunk format already solves sub-stream multiplexing for terminal
sessions — this document generalizes that pattern into a universal channel
multiplexer that the call protocol can orchestrate.

The 2026-07-11 revision adds the hub motivation (§Hub Motivation), the
concrete channel-open negotiation protocol (§Channel Open Negotiation), and
the channel-manager/connection internals (§Channel Manager and Connection
Internals) — driven by the realization that the hub crate's multi-transport
complexity is what channels specifically collapses, and that the call
protocol's auth/operation-overlay model carries over to channel lifecycle
with no new machinery.

## Vision Recap

`alknet-channels` is a **multiplexing proxy** — a `ProtocolHandler` on
`alknet/channels` that decomposes a single bidirectional stream into multiple
logical channels, each carrying a different protocol (ALPN). It is the
generalization of two existing patterns:

1. **SSH's channel multiplexer** (RFC 4254): `ChannelId(u32)` with
   string-named types negotiated per channel, all traffic interleaved on one
   encrypted transport stream.
2. **alknet-tty's chunk format**: `[stream_type: u8][length: u32 be][payload]`
   with a fixed set of four sub-streams (stdin/stdout/stderr/control).

The generalization: a chunk header of `(channel_id: u32, stream_type: u8,
length: u32)` — 9 bytes — that multiplexes an arbitrary number of channels,
each with up to 256 sub-stream types, over a single transport. Channel 0 is
pre-negotiated as `alknet/call` — the call protocol. Every other channel is
opened dynamically via call operations on channel 0, and channel open is
**bidirectional**: either side can open a channel to the other, just like
the call protocol's operation overlay where each side populates what
operations they can call.

The guiding insight:

> **Streams are streams.** A TTY session, an SSH channel, a forwarded TCP
> connection, a QUIC bidi stream — they're all just `AsyncRead + AsyncWrite`
> handles. The differences are only in how they're *opened* (negotiation) and
> what *multiplexing layer* carries them. Once you normalize to
> `AsyncRead + AsyncWrite`, the architecture collapses into a simple shape:
> every channel is just an ALPN routed through the same `HandlerRegistry`,
> channel 0 is just `alknet/call` pre-negotiated, and ACL governs everything.

## The Problem This Solves

### The conceptual tangle

Today, alknet's architecture has three different multiplexing models that
don't compose well:

| Model | Where | Mechanism |
|-------|-------|-----------|
| Connection-level | ALPN router | One ALPN per QUIC connection |
| Stream-level | QUIC native | Many bidi streams per connection |
| Sub-stream-level | TTY chunk format | 4 logical channels within one bidi stream |

A docker client needs both JSON call operations (`docker/container/list`,
`docker/container/create`) and raw byte streams (interactive exec, container
attach). Today that means **two separate QUIC connections** with different
ALPNs (`alknet/call` and `alknet/tty`). The call protocol has no way to say
"for this operation, open a TTY stream." There's no cross-ALPN coordination.

### The deeper issue

The call protocol is positioned as the universal RPC layer, but it's
fundamentally **JSON-only**. The `EventEnvelope` framing can't carry raw
bytes. So anything non-JSON (TTY sessions, SSH channels, file transfers, git
pack protocols) needs its own ALPN and its own connection. The call protocol
can't even *reference* these other streams — there's no "stream token" or
"channel open" mechanism.

### What channels provides

A single `alknet/channels` connection carries:
- Channel 0: `alknet/call` (pre-negotiated — both sides route it to the `CallAdapter`)
- Channel N: `alknet/tty` (opened dynamically via `channel/open`)
- Channel M: `alknet/ssh` (opened dynamically via `channel/open`)
- Channel K: `alknet/tunnel` (opened dynamically via `channel/open`)

All on one QUIC connection (or one TCP connection, or one WebTransport
session). Channel 0 is not a special "control plane" — it's just the
`alknet/call` ALPN, pre-negotiated so both sides know to route it to the
`CallAdapter` without an explicit `channel/open` exchange. Every other
channel works exactly the same way: reassemble chunks into a stream, look up
the ALPN in the `HandlerRegistry`, hand off to the handler. Channel open is
bidirectional — either side can open a channel to the other, just like the
call protocol's operation overlay where each side populates what operations
they can call.

## Hub Motivation: The Multi-Transport Collapse

This design was forced by the hub crate. A hub is the architectural role
(ADR-029, ADR-034) that bridges peers and browsers: it terminates QUIC from
spokes, terminates WebTransport/WSS from browsers, and may itself be a spoke
of an upstream hub. The hub's job is to **route and relay**, not to
re-implement every protocol's framing per transport.

Without channels, the hub quickly becomes a mess:

### The mess, concretely

A hub serving a browser over WebTransport and a spoke over QUIC wants to
offer the browser three things on the same logical session:

1. Call operations on the spoke (e.g., `docker/container/list` — JSON).
2. A TTY session on the spoke (interactive exec — raw bytes, ADR-052 chunk
   format).
3. An SSH session to the spoke (raw bytes, SSH binary protocol).

Today each of these is a **separate ALPN** on a **separate connection**, and
each transport multiplexes differently:

| Need | Today's mechanism | Transport | Multiplexing |
|------|-------------------|-----------|--------------|
| JSON ops | `alknet/call` ALPN | QUIC stream or WSS | one op per bidi stream |
| TTY | `alknet/tty` ALPN | QUIC stream or WSS | TTY chunk format *within* one bidi stream |
| SSH | `alknet/ssh` ALPN (future) | QUIC stream or WSS | SSH multiplexes *within* one bidi stream |

So the hub faces a matrix: **3 needs × 2 transports × N spokes**, and the
multiplexing layer is different in each cell. The hub would have to:

- Maintain a `quinn::Connection` per spoke AND a WebTransport session per
  browser, AND decide which one carries which ALPN.
- For a browser→spoke TTY, the hub can't just "forward a stream" — the
  browser's WSS stream carries `alknet/tty` chunks, and the spoke's QUIC
  stream also carries `alknet/tty` chunks, so the hub *can* relay bytes —
  but it had to negotiate two separate `alknet/tty` connections (one per
  leg) and correlate them by out-of-band state. There's no "one session that
  spans the relay."
- For a browser→spoke SSH, same problem with a different ALPN and a
  different internal multiplexer.
- The call protocol that *orchestrates* all this lives on a **third** ALPN
  (`alknet/call`), so the hub is juggling three connections per (browser,
  spoke) pair, each with its own framing, and the correlation across them is
  implicit.
- Every new protocol (tunnel, future file-transfer, future git-pack) adds
  **another column** to the matrix and **another ALPN** the hub must
  register, dispatch, and relay.

This is the "PITA to maintain a bunch of connections" problem: the hub's
complexity is **O(protocols × transports × spokes)**, when it should be
**O(spokes)**.

### What channels collapses

With `alknet/channels`, the hub's per-spoke and per-browser state is **one
channels connection each**, and everything else is routing:

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

The hub's relay logic is **channel-by-channel byte forwarding**: read chunks
for `(channel_id)` off the browser's channels connection, write the same
payload (with the spoke's `channel_id` substituted) onto the spoke's
channels connection. The hub does **not** parse `alknet/tty` chunks, does
**not** understand SSH's internal multiplexer, does **not** run a separate
`CallAdapter` per leg — it forwards opaque `(channel_id, stream_type,
payload)` tuples and lets the endpoints at each end do the protocol work.

The collapse is at three levels:

1. **One connection per leg, not one per protocol.** The browser holds one
   WebTransport session to the hub; the hub holds one QUIC connection to the
   spoke. All three needs (call, TTY, SSH) ride as channels on those two
   connections. The hub correlates by channel, not by ALPN-per-connection.
2. **One multiplexing model, not three.** Connection-level (ALPN router),
   stream-level (QUIC native), and sub-stream-level (TTY chunks) collapse
   into one: channels chunks. The hub's relay code is one loop, not three.
3. **The call protocol orchestrates from inside.** Channel 0 is `alknet/call`
   on both legs. The browser calls `channel/open` on its channel 0; the hub
   forwards that call to the spoke's channel 0 (via `from_call`); the spoke
   allocates the channel and returns the ID; the hub opens a matching channel
   on the browser's side and bridges them. The hub never runs a handler for
   `alknet/tty` or `alknet/ssh` — it only runs `alknet/channels` (the relay)
   and `alknet/call` (for its own hub-level operations like routing and
   resource lookup).

### Why the auth model reuses cleanly

The call protocol's `OperationContext` (identity, scopes, capabilities,
ownership) already gates every operation. `channel/open` is just another
operation on channel 0, so the same `OperationContext` flows through:

- The browser's `channel/open` carries the browser's identity (bearer token
  resolved by the hub per ADR-043).
- The hub forwards via `from_call`, which populates `forwarded_for` from the
  hub's `OperationContext.identity` (ADR-032 §3) — exactly the kernel/user-
  land + forwarded-for model from ADR-050.
- The spoke's `AccessControl::check` sees the hub as the caller and the
  browser as `forwarded_for`. The spoke authorizes the hub (its direct
  peer); the hub's "who is this for" is its own app state, carried as
  `forwarded_for`.

No new auth machinery. The hub doesn't authenticate channels — it
authenticates the call operations that *open* channels, which it already
does. The channels layer inherits the call protocol's ACL by being one of
its operation types.

### What the hub *does* still own

Channels does not eliminate the hub's responsibilities; it relocates them:

- **Routing**: which spoke serves `container:abc123`? That's the hub's
  resource registry / ownership store (ADR-050), queried via call operations
  on channel 0. Channels doesn't touch this.
- **ACL at the hub**: does this browser's identity have `channel:open` scope
  for `alknet/ssh` to `spoke-X`? That's the call protocol's
  `AccessControl::check` on the `channel/open` operation, run by the hub's
  `CallAdapter` before it forwards. Channels doesn't touch this either.
- **Relay lifecycle**: when a browser disconnects, the hub tears down the
  spoke-side channels (and vice versa). This is `channel/close` on each
  channel, or a transport-level close that the channels layer observes.

What the hub *no longer* owns: per-protocol framing parsers, per-protocol
relay loops, per-ALPN connection management, and the correlation state
across multiple connections per (browser, spoke) pair. Those move into the
channels layer, which is transport-agnostic and protocol-agnostic.

## The Wire Format

### Chunk header

```
[channel_id: u32 be][stream_type: u8][length: u32 be][payload bytes]
```

9 bytes of header, compared to TTY's current 5 bytes (`[stream_type: u8]
[length: u32 be]`). The `channel_id` field is the addition — it's what turns
a fixed 4-channel multiplexer into an arbitrary N-channel multiplexer.

### Channel 0: pre-negotiated as `alknet/call`

Channel 0 is not a special "control plane" with its own framing. It is
simply the `alknet/call` ALPN, pre-negotiated: both sides know that channel
0 is routed to the `CallAdapter` without an explicit `channel/open`
exchange. The call protocol runs on channel 0 exactly as it runs on a
top-level `alknet/call` QUIC connection — `EventEnvelope` frames on a
bidirectional stream.

Channel 0's chunks use `stream_type` to disambiguate framing:

| stream_type | purpose |
|-------------|---------|
| 0 | `EventEnvelope` frame (JSON, length-prefixed — the call protocol wire format) |
| 1-255 | reserved for future sub-streams |

### Data channels (1..N)

Every other channel works exactly the same way as channel 0: reassemble
chunks into a stream, look up the ALPN in the `HandlerRegistry`, hand off to
the handler. The `stream_type` byte decomposes the channel into sub-streams,
following the TTY crate's proven model:

| stream_type | direction | purpose |
|-------------|-----------|---------|
| 0 | write half | data flowing in (stdin equivalent) |
| 1 | read half | data flowing out (stdout equivalent) |
| 2 | read half (optional) | error/diagnostic output (stderr equivalent) |
| 3 | bidirectional | control messages (protocol-specific JSON) |
| 4-255 | reserved | future sub-stream types |

Not all channels use all sub-streams. A TTY session uses 0-3. A raw tunnel
might use only 0 and 1. An SSH connection uses only 0 and 1 (SSH multiplexes
internally). The `stream_type` set is fixed per channel type, declared at
open time.

### Channel lifecycle

1. **Open**: either side sends a `channel/open` call operation on channel 0
   with `{ alpn, params }`. The receiving side validates ACL, looks up the
   ALPN in the `HandlerRegistry`, allocates the handler, and returns
   `{ channel_id, stream_types }`. The `stream_types` field declares which
   sub-streams are active for this channel (e.g., `[0, 1, 2, 3]` for TTY,
   `[0, 1]` for a tunnel). Channel open is **bidirectional** — a hub can
   expose a resource a worker consumes, or a worker can expose a resource
   the hub consumes, just like the call protocol's operation overlay.
2. **Data**: both sides read/write chunks on the assigned `channel_id`. Each
   side reassembles chunks for a given `(channel_id, stream_type)` into a
   stream. The reassembled streams are presented to the handler as
   `AsyncRead + AsyncWrite` handles.
3. **Control**: channel-specific control messages ride on `stream_type 3` as
   JSON. For TTY channels this is resize/signal/eof. For tunnel channels
   this might be keepalive or connection-close. The call protocol on channel
   0 can also send control operations scoped to a specific channel (e.g.,
   `tty/resize` with a `channel_id` parameter).
4. **Close**: either side sends a `channel/close` call operation on channel
   0, or the channel's handler signals completion (e.g., TTY exit). The
   channel's chunks stop; the `channel_id` may be reused.

### Framing disambiguation

The TTY crate's framing disambiguation trick (ADR-052 §5) carries forward.
Channel 0 is just another channel — its chunks have `channel_id=0` in the
header. The disambiguation between channel 0 (call protocol) and data
channels is by `channel_id`, not by a special first-byte trick. Within a
channel, `stream_type` 0 (stdin) from the server is invalid, so `0x00` as
the first byte of a chunk payload from the server is unambiguous.

## The Channel Connection Abstraction

### Reassembling chunks into streams

On each end of a channels connection, chunks for a given `(channel_id,
stream_type)` are reassembled into a byte stream. The reassembly is
order-preserving within a `(channel_id, stream_type)` pair — chunks arrive
in order because they ride on a single ordered transport (QUIC stream, TCP
connection).

Channel 0 is reassembled exactly like any other channel. The reassembled
stream for `(channel_id=0, stream_type=0)` is handed to the `CallAdapter`
as a `Connection`. The `CallAdapter` doesn't know it's inside a channels
connection — it sees a `Connection` yielding bidi streams, same as if it
were a top-level QUIC connection.

For data channels (1..N), the reassembled streams are presented through the
same `Connection` abstraction that alknet-core already defines
(`crates/alknet-core/src/types.rs`):

```rust
// A channels connection presents the same interface as a QUIC connection
impl ChannelConnection {
    async fn accept_bi(&self) -> Result<(SendStream, RecvStream), StreamError>;
    async fn open_bi(&self) -> Result<(SendStream, RecvStream), StreamError>;
}
```

Internally, `accept_bi` / `open_bi` map to `channel/open` call operations on
channel 0, and the returned `SendStream` / `RecvStream` are backed by chunk
reassembly on the assigned `channel_id`.

### ALPN routing inside channels

A `ChannelConnection` holds a reference to the same `HandlerRegistry` that
the outer `AlknetEndpoint` uses. When a `channel/open` arrives with an ALPN
string, the channels handler looks up the ALPN in the registry, validates
ACL, and hands the reassembled stream to the handler. The handler doesn't
know it's inside a channels connection — it sees a `Connection` yielding bidi
streams, same as if it were a top-level QUIC connection.

Channel 0 is not special — it's just `alknet/call` pre-negotiated. Both
sides know to route channel 0 to the `CallAdapter` without an explicit
`channel/open` exchange. Every other channel is opened dynamically via
`channel/open` on channel 0, and the open is bidirectional: either side can
initiate.

This means every existing and future `ProtocolHandler` works inside channels
without modification. The `CallAdapter`, `TtyAdapter`, the future
`SshAdapter`, a tunnel handler — they all just see a `Connection`. The
channels layer is a transparent proxy.

### Recursive composition

A `ChannelConnection` is itself a `Connection`. A channels handler can open a
sub-channels connection on a data channel. This is recursive composition:
`alknet/channels` inside `alknet/channels`. It's powerful (arbitrary nesting
of multiplexing layers) and potentially confusing (infinite recursion). The
recommendation is to allow it but not encourage it — the primary use case is
one level of multiplexing. Recursive composition is a natural consequence of
the `Connection` abstraction, not a feature to design for.

## Channel Open Negotiation

Channel lifecycle is orchestrated by the call protocol on channel 0. The
call protocol is JSON-only — it carries `channel/open`,
`channel/close`, `channel/control`, and `channel/resources` as **new
operation types** in the existing `OperationRegistry`, dispatched through
the existing `OperationContext` (identity, scopes, capabilities, ownership,
`forwarded_for`). No wire-format change to `EventEnvelope`, no new carriage
type. The channels layer registers these operations on the call protocol's
`OperationRegistry` at assembly time.

This is where the call protocol's auth model is reused: `channel/open` is
gated by `AccessControl::check` exactly like any other operation, and the
operation's `AccessControl` can declare `required_scopes` (e.g.
`channel:open:alknet/tty`), `resource_type` + `resource_action` (e.g.
`container` + `tty`), or `required_scopes_any` for multi-scope gates. The
channels layer doesn't re-implement auth — it leans on the
`OperationRegistry::invoke` path that already runs the check before the
handler runs.

### `channel/open` — request a channel

Either side sends a `call.requested` on channel 0 with operation name
`channel/open`:

```json
{
  "type": "call.requested",
  "id": "req-abc",
  "payload": {
    "operation": "channel/open",
    "input": {
      "alpn": "alknet/tty",
      "stream_types": [0, 1, 2, 3],
      "params": {
        "backend": "docker",
        "cmd": ["bash"],
        "container": "abc123"
      },
      "direction": "initiator-to-responder"
    }
  }
}
```

Fields:

| field | type | meaning |
|-------|------|---------|
| `alpn` | string | The ALPN the channel will carry, e.g. `alknet/tty`, `alknet/ssh`, `alknet/tunnel`. The responder looks this up in its `HandlerRegistry`. |
| `stream_types` | `[u8]` | Which sub-stream types this channel will use. Declared at open time so both sides size their reassembly buffers and know which `(channel_id, stream_type)` pairs are valid. E.g. `[0, 1, 2, 3]` for TTY, `[0, 1]` for a raw tunnel. |
| `params` | object | ALPN-specific parameters passed to the handler. For `alknet/tty` this is the `NegotiateRequest` (backend, cmd, env, size). For `alknet/tunnel` this is the target resource (`{"resource": "container:abc123", "port": 5432}`). For `alknet/ssh` this is the SSH auth/method hints. The channels layer does not interpret `params` — it hands the JSON to the handler's allocate entry point. |
| `direction` | string | `initiator-to-responder` (the initiator wants the responder to expose a resource) or `responder-to-initiator` (the initiator wants to expose a resource *to* the responder — the "worker exposes, hub consumes" case). See "Bidirectional open" below. |

The `direction` field is what makes channel open match the call protocol's
operation-overlay symmetry: just as either side can populate operations the
other can call, either side can expose resources the other can open
channels to. `initiator-to-responder` is the common case ("open me a TTY on
your docker container"). `responder-to-initiator` is the reverse ("I'm a
worker exposing my local Postgres as a tunnel target; hub, you can open a
channel to it") — which is how a hub consumes a worker's resources without
the worker initiating.

### `channel/open` — response

The responder validates ACL, looks up the ALPN in `HandlerRegistry`, and —
crucially — **allocates the `channel_id` before returning** (DP-1: server-
assigned). The response is a normal `call.responded`:

```json
{
  "type": "call.responded",
  "id": "req-abc",
  "payload": {
    "output": {
      "channel_id": 7,
      "stream_types": [0, 1, 2, 3]
    }
  }
}
```

| field | type | meaning |
|-------|------|---------|
| `channel_id` | u32 | The server-assigned channel ID. Both sides now know to route chunks with this `channel_id` to the new channel. |
| `stream_types` | `[u8]` | The *negotiated* set — the responder may narrow the initiator's requested set (e.g. refuse stderr for a backend that doesn't produce it). The intersection of requested and supported. |

Once the initiator receives the response, data may flow on
`(channel_id=7, stream_type=*)`. The responder's handler is already
allocated and reading from its reassembled streams.

### `channel/open` — error cases

Errors use the call protocol's existing `CallError` shape, dispatched as
`call.error`. The channels layer defines a small set of error codes:

| code | meaning | retryable |
|------|---------|-----------|
| `channel:unknown_alpn` | The `alpn` is not in the responder's `HandlerRegistry`. | false |
| `channel:forbidden` | `AccessControl::check` denied the open (missing scope, not the resource owner, `forwarded_for` not trusted). | false |
| `channel:allocation_failed` | The handler's allocate failed (e.g. `DockerTtyBackend::allocate` couldn't start the exec). The `details` carry the backend's error message. | true (often transient) |
| `channel:invalid_params` | The `params` JSON didn't satisfy the ALPN's expectations (e.g. missing `cmd` for TTY). | false |
| `channel:too_many_channels` | The responder hit its per-connection channel limit (OQ-CH-06). | false |
| `channel:stream_type_unavailable` | The responder's handler can't provide one of the requested `stream_types`. The `details` carry the supported set. | false |

These are new `CallError.code` strings, not new framing. The call protocol
already carries `code`/`message`/`retryable`/`details`.

### `channel/close` — tear down a channel

Either side sends:

```json
{
  "operation": "channel/close",
  "input": { "channel_id": 7, "reason": "exit" }
}
```

The responder (the side that *didn't* send the close) drains its
reassembled streams for `channel_id`, signals EOF to the handler, and
returns `call.responded` with `{ "closed": true }`. The `channel_id` is now
eligible for reuse (OQ-CH-04). `reason` is a free-form string for
observability — `"exit"`, `"cancel"`, `"error"` — and is not semantically
required.

The ordering invariant (ADR-055 for TTY): the channel's **data chunks** must
be written and flushed before the `channel/close` operation is sent on
channel 0. This is an implementation constraint on the side closing — the
channels layer's close handler must observe the data-channel pump complete
before issuing the call operation. For TTY this is the exit-chunk-is-last
invariant carried forward.

### `channel/control` — out-of-band control on channel 0

For control that doesn't need ordering relative to data (resize, signal,
keepalive), a call operation on channel 0:

```json
{
  "operation": "channel/control",
  "input": {
    "channel_id": 7,
    "stream_type": 3,
    "message": { "type": "resize", "cols": 80, "rows": 24 }
  }
}
```

The channels layer routes `message` to the handler's control handle for
`channel_id`. This is the "call protocol for orchestration" half of DP-4.
The `message` JSON is ALPN-specific; the channels layer doesn't interpret
it — it hands it to the handler's control entry point, same as `params` on
open.

### `stream_type 3` — in-band control on the data channel

For control that **does** need ordering relative to data (EOF before exit,
flush before close), a chunk with `stream_type = 3` on the data channel
itself:

```
[channel_id: u32 be][stream_type: 0x03][length: u32 be][json payload]
```

This is the "stream_type 3 for data-ordered control" half of DP-4. The
handler parses the JSON; the channels layer just reassembles and delivers
it in-order with the data. The TTY crate's `ControlMessage::Eof` is the
canonical example — it must arrive after the last stdin chunk, which is
guaranteed by chunk ordering within `(channel_id, stream_type)`, not by a
call-protocol round-trip.

### `channel/resources` — populate the resource overlay

This is the conceptual gap the doc is closing. The call protocol already
has `services/list` and `services/list-peers` for discovering **operations**
each side exposes. Channels needs the equivalent for **resources** — what
ALPNs each side is willing to open channels for, and with what constraints.

A side calls `channel/resources` on channel 0 to ask the other side what it
exposes:

```json
{
  "operation": "channel/resources",
  "input": {}
}
```

Response:

```json
{
  "output": {
    "resources": [
      {
        "alpn": "alknet/tty",
        "backends": ["docker", "local"],
        "access": { "required_scopes": ["tty:open"] }
      },
      {
        "alpn": "alknet/ssh",
        "access": { "required_scopes": ["ssh:open"] }
      },
      {
        "alpn": "alknet/tunnel",
        "targets": ["container:*", "service:postgres"],
        "access": { "required_scopes_any": ["tunnel:open", "admin"] }
      }
    ]
  }
}
```

| field | type | meaning |
|-------|------|---------|
| `alpn` | string | The ALPN this side will accept `channel/open` for. |
| `backends` / `targets` | `[string]` | ALPN-specific enumeration of what's available — TTY backends, tunnel target resource patterns. The channels layer doesn't interpret these; they're for the *initiator* to know what `params` to send. |
| `access` | object | A preview of the `AccessControl` that `channel/open` will check. Lets the initiator fail fast (e.g. a browser without `tty:open` doesn't bother trying to open a TTY channel). This is advisory — the real check happens on `channel/open`. |

This is the resource-discovery analogue of `services/list`. It's the
mechanism by which both sides populate what resources they expose, matching
the bidirectional symmetry of the operation overlay. The hub, after
`from_call` discovers the spoke's operations, also calls
`channel/resources` to discover what channels the spoke will accept — and
the hub aggregates that into what it exposes to the browser.

### Bidirectional open — who initiates

Channel open is bidirectional. Two cases:

1. **Initiator wants responder's resource** (`direction:
   initiator-to-responder`): the browser opens a TTY channel on a spoke.
   The initiator sends `channel/open`; the responder allocates the handler
   and returns the ID. This is the common case.

2. **Initiator exposes a resource to the responder** (`direction:
   responder-to-initiator`): a worker wants the hub to be able to open a
   tunnel channel *to* the worker's local service. Here the worker sends
   `channel/open` with `direction: responder-to-initiator` — it's saying
   "I'm making myself available; when *you* want to connect, use this
   channel." The semantics: the channel is created, but the worker's handler
   is the *server* side of the ALPN, and the hub is the *client* side. The
   `params` describe what the worker is exposing (e.g.
   `{"target": "service:postgres", "port": 5432}`), not what it's asking
   for.

   This is the mirror of the call protocol's operation overlay: a worker
   registers `bash/exec` (the hub can call it); a worker opens a
   `responder-to-initiator` channel (the hub can use it). In both cases the
   worker is the server, the hub is the client, and the *worker* initiates
   the registration/open because it's the one that knows what it has.

### ACL flow end-to-end

Putting it together, a browser opening a TTY channel to a spoke through a
hub:

1. Browser's channel 0 → hub's channel 0: `channel/open`
   `{ alpn: "alknet/tty", params: { backend: "docker", cmd: ["bash"], container: "abc123" } }`.
   The browser's identity is a bearer token (ADR-043).
2. Hub's `CallAdapter` runs `AccessControl::check` on `channel/open` with
   the browser's identity. If the browser lacks `channel:open:alknet/tty`
   or the hub's policy forbids it → `channel:forbidden`.
3. Hub forwards to spoke via `from_call`: the hub's `forwarded_for` handler
   constructs a `call.requested` with the hub as caller and the browser as
   `forwarded_for` (ADR-032 §3). The spoke receives
   `channel/open` with `caller = hub`, `forwarded_for = browser`.
4. Spoke's `CallAdapter` runs `AccessControl::check` with the hub as caller
   (the spoke authorizes the hub, its direct peer — ADR-050). The spoke's
   ownership store (ADR-050) verifies the hub (or the `forwarded_for`
   browser, if the spoke's policy says so) owns `container:abc123`.
5. Spoke allocates the channel via `TtyAdapter` / `DockerTtyBackend`, returns
   `channel_id`.
6. Hub opens a matching channel on the browser's side (it's now the
   *responder* for the browser leg, *initiator* for the spoke leg) and
   bridges them: read chunks off browser channel, write onto spoke channel
   (with `channel_id` remapped), and vice versa.

The hub ran **zero** protocol-specific auth. It ran `channel/open`'s
`AccessControl::check` (call-protocol machinery) and forwarded. The channels
layer inherited the auth model by being a call-protocol operation.

## Channel Manager and Connection Internals

This section fills the second conceptual gap: what state the channels layer
holds, how `ChannelsAdapter::handle` is wired, and how the `channel/open`
handler threads back into the `CallAdapter`'s `OperationRegistry`. The
guiding constraint is that **the channels layer is a re-framing proxy** — it
converts between "one transport stream carrying N channels" (the wire) and
"N independent `AsyncRead + AsyncWrite` handles" (what handlers see) — and
it does **no protocol work itself**.

### The two halves: ChannelsAdapter and ChannelManager

The channels crate has two internal components, split by responsibility:

1. **`ChannelsAdapter`** — implements `ProtocolHandler` for
   `alknet/channels`. Its `handle()` receives one `Connection` (the
   transport), reads 9-byte chunk headers off the single bidi stream, and
   routes each chunk to the `ChannelManager`. It is the *read/demux* half.
   It does not know what ALPNs exist or what a handler is — it just splits
   streams.

2. **`ChannelManager`** — the shared state both halves touch. It holds the
   map of `channel_id → ChannelState`, the `HandlerRegistry` reference, and
   the `OperationRegistry` reference. It is the *reassemble/allocate* half.
   It is what the `channel/open` operation handler closes over.

The split mirrors the TTY crate's `ChunkReader`/`ChunkWriter` + adapter
pattern, generalized: the adapter no longer drives one session — it drives
N channels, and the channel-0 session is special only in that it's pre-
allocated.

### ChannelManager state

```rust
pub struct ChannelManager {
    /// channel_id → per-channel state. Channel 0 is pre-inserted at construction.
    channels: Mutex<HashMap<u32, ChannelState>>,
    /// The handler registry for looking up ALPNs on channel/open.
    handlers: Arc<HandlerRegistry>,
    /// The call protocol's operation registry, so channel/open etc. can be
    /// registered at assembly time. The ChannelsAdapter holds a clone.
    call_ops: Arc<OperationRegistry>,
    /// Next server-assigned channel_id. Monotonic; wraps at u32::MAX.
    next_id: AtomicU32,
    /// Per-channel reassembly buffer cap (DP-5). Default 1 MiB.
    buffer_cap: usize,
    /// Per-connection channel limit (OQ-CH-06). Default 256.
    max_channels: usize,
}

struct ChannelState {
    /// The ALPN this channel carries, for routing and observability.
    alpn: String,
    /// Reassembly buffers per active stream_type. Each is a bounded
    /// channel feeding a `SendStream`/`RecvStream` returned to the handler.
    streams: HashMap<u8, ReassemblyBuffer>,
    /// The handler task driving this channel (TtyAdapter::drive_session,
    /// SshAdapter::handle, etc.). Dropping this aborts the channel.
    handler_task: JoinHandle<()>,
    /// Which stream_types are active (from the open negotiation).
    stream_types: Vec<u8>,
}
```

The `ChannelManager` is `Clone` (cheap — it's an `Arc` internally) so that
the `ChannelsAdapter`, the `channel/open` operation handler, and any relay
logic can all hold a handle to it.

### ChannelsAdapter::handle — the read/demux loop

```rust
#[async_trait]
impl ProtocolHandler for ChannelsAdapter {
    fn alpn(&self) -> &'static [u8] { b"alknet/channels" }

    async fn handle(&self, connection: Connection, auth: &AuthContext) -> Result<(), HandlerError> {
        // One bidi stream carries all channels.
        let (send, recv) = connection.accept_bi().await?;
        // Channel 0 is pre-negotiated as alknet/call. Construct its
        // reassembly buffers and hand the reassembled Connection to the
        // CallAdapter (looked up in the registry, same as every other ALPN).
        self.manager.preinstall_channel_0(send, recv, auth).await?;
        // Now read chunks off the transport and route them.
        self.manager.run_demux_loop().await
    }
}
```

The `preinstall_channel_0` step is the only special case: it constructs the
reassembly buffers for `channel_id = 0`, wraps them as a `Connection` (via
`Connection::from_stream`, which already exists in `crates/alknet-core/src/
types.rs`), and hands that `Connection` to the `CallAdapter` — exactly as if
`alknet/call` had been the top-level ALPN. The `CallAdapter` is none the
wiser: it calls `accept_bi()` on its `Connection`, gets one bidi stream (the
channel-0 reassembled stream), and runs its dispatch loop. The call
protocol's `EventEnvelope` frames ride on `stream_type = 0` of channel 0.

The `run_demux_loop` reads 9-byte headers, looks up `channel_id` in
`channels`, and pushes the payload into the right `ReassemblyBuffer` for
`(channel_id, stream_type)`. If the buffer is full (DP-5), the loop
**stops reading that channel's chunks** until the consumer drains — this is
the backpressure mechanism. Other channels keep flowing. This is the one
place the channels layer does flow control, and it's deliberately simple.

### The channel/open handler — threading into OperationRegistry

The `channel/open` (and `channel/close`, `channel/control`,
`channel/resources`) operations are registered on the call protocol's
`OperationRegistry` at **assembly time**, before the endpoint starts. The
handler closures close over a `ChannelManager` clone:

```rust
// At assembly time, after both adapters are constructed:
let channel_ops = ChannelOperations::new(manager.clone());
channel_ops.register_on(&mut call_registry)?;
```

Where `ChannelOperations::register_on` inserts four
`HandlerRegistration`s (one per operation) into the `OperationRegistry`.
The `channel/open` handler is the interesting one:

```rust
fn channel_open_handler(mgr: ChannelManager) -> Handler {
    Arc::new(move |input: Value, ctx: OperationContext| {
        let mgr = mgr.clone();
        Box::pin(async move {
            let req: ChannelOpenRequest = serde_json::from_value(input)?;
            // 1. ACL is already checked by OperationRegistry::invoke before
            //    this handler runs — AccessControl::check on the operation.
            // 2. Look up the ALPN in the HandlerRegistry.
            let handler = mgr.handlers.get(req.alpn.as_bytes())
                .ok_or_else(|| channel_error("channel:unknown_alpn"))?;
            // 3. Allocate the channel_id (server-assigned, DP-1).
            let channel_id = mgr.next_id.fetch_add(1, Relaxed);
            // 4. Construct the reassembly buffers for the negotiated
            //    stream_types and wrap as a Connection.
            let conn = mgr.build_channel_connection(channel_id, &req.stream_types);
            // 5. Spawn the handler task — same as TtyAdapter::handle does
            //    today, but on a ChannelConnection instead of a QUIC conn.
            let task = tokio::spawn(async move {
                let _ = handler.handle(conn, &auth_from_ctx(&ctx)).await;
            });
            // 6. Record the channel state.
            mgr.insert_channel(channel_id, ChannelState { /* ... */ });
            // 7. Return the channel_id.
            ResponseEnvelope::ok(ctx.request_id, json!({
                "channel_id": channel_id,
                "stream_types": req.stream_types,
            }))
        })
    })
}
```

The key insight: **step 5 is identical to what `TtyAdapter::handle` does
today** (`crates/alknet-tty/src/adapter.rs:130`) — `tokio::spawn` a
`drive_session` task per bidi stream. The only difference is the `Connection`
passed in is a `ChannelConnection` (backed by reassembly) rather than a
quinn connection. The handler doesn't care.

### ChannelConnection as a Connection

`ChannelConnection` is a new `ConnectionKind` variant, or — to avoid
touching `alknet-core` — a `Connection` constructed via the existing
`Connection::from_stream` path, where the "stream" is a reassembly buffer
pair. The reassembly buffer exposes `AsyncRead` (drained by the handler's
`RecvStream`) and the write side exposes `AsyncWrite` (the handler's
`SendStream`, which the channels layer reads from and re-chunks onto the
transport with the right `channel_id`).

The flow for a handler writing to its `SendStream`:

```
handler writes bytes
  → SendStream::poll_write (AsyncWrite on the reassembly write-half)
  → channels layer's per-channel pump reads from the write-half
  → frames as [channel_id][stream_type][length][payload] onto the transport
```

And for reading:

```
transport arrives with a chunk for (channel_id, stream_type)
  → ChannelsAdapter::run_demux_loop routes payload to ReassemblyBuffer
  → handler's RecvStream::poll_read (AsyncRead on the reassembly read-half) yields bytes
```

The `stream_type` is chosen by *which* `SendStream`/`RecvStream` the handler
writes to. A TTY handler gets four handles (stdin=0, stdout=1, stderr=2,
control=3); a tunnel handler gets two (data-in=0, data-out=1). The
`ChannelConnection::accept_bi()` call returns the pair for the *next*
expected stream_type, or — more concretely — the handler is handed a typed
struct (`TtyChannel { stdin, stdout, stderr, control }`) rather than a
generic `Connection`, because the channel's stream_types are known at open
time.

This is a slight divergence from "every handler sees a `Connection`": TTY
wants four named handles, not a `Connection` you call `accept_bi` on four
times. The resolution (carried into Phase 1): the `ChannelConnection`
*implements* the `Connection` interface (for recursion and generic
handlers), **and** can be destructured into typed sub-stream handles for
handlers that know their ALPN's shape. The typed destructure is a
convenience layer over the same reassembly buffers; it's not a separate
abstraction.

### The hub relay: ChannelManager-to-ChannelManager

For the hub use case (§Hub Motivation), the hub holds *two* `ChannelManager`
instances per (browser, spoke) pair — one for the browser leg, one for the
spoke leg — and a relay task per channel that bridges them:

```rust
// For channel_id=7 on browser side, channel_id=12 on spoke side:
tokio::spawn(async move {
    let (b_send, b_recv) = browser_mgr.open_channel_stream(7, stream_type).await;
    let (s_send, s_recv) = spoke_mgr.open_channel_stream(12, stream_type).await;
    tokio::join!(
        pump(b_recv, s_send),  // browser → spoke
        pump(s_recv, b_send),  // spoke → browser
    );
});
```

The relay reads opaque bytes off one `ChannelManager`'s reassembled stream
and writes them onto the other's write-half, which re-chunks them with the
other leg's `channel_id`. The relay does not parse the bytes — it doesn't
know if they're TTY chunks, SSH frames, or tunnel data. The channels layer
on each end does the chunk↔stream conversion; the relay just moves bytes
between two `AsyncRead + AsyncWrite` pairs.

This is why the hub's complexity collapses: the relay is **one pump
function** applied per channel, not per (protocol × transport) cell. The
`ChannelManager` is the uniform interface on both ends.

### What is NOT in the ChannelManager

To keep the boundary clean, the `ChannelManager` deliberately does **not**
hold:

- **No `ProtocolHandler` implementations.** It holds a `HandlerRegistry`
  reference for ALPN lookup, but it doesn't *be* a handler. The handlers
  live in their crates and register on the same registry.
- **No ALPN-specific parsing.** It does not parse `NegotiateRequest` JSON,
  SSH binary frames, or tunnel target strings. It hands `params` JSON to
  the handler and gets back a handler task; it hands `stream_type 3` JSON
  to the handler's control handle. The `ChannelManager` is ALPN-blind.
- **No auth state.** Auth lives in the `OperationContext` that the call
  protocol passes to `channel/open`. The `ChannelManager` doesn't check
  scopes or ownership — that's `AccessControl::check` in
  `OperationRegistry::invoke`, run before the `channel/open` handler is
  called.
- **No transport coupling.** The `ChannelManager` talks to the transport
  only through the `ChannelsAdapter`'s read loop and the per-channel write
  pumps, both of which use `AsyncRead + AsyncWrite`. QUIC, TCP+TLS,
  WebTransport, SSH channel — all look the same.

This is what makes the channels layer WASM-compatible (§WASM Compatibility)
and transport-agnostic (§Transport Agnosticism): the `ChannelManager` is
pure byte routing with no platform or protocol dependencies.

## Relationship to Existing Crates

### alknet-tty

The TTY crate's chunk format becomes a special case of the channels format
(single-channel, no `channel_id` header). Two paths forward:

**Path A (recommended): TTY becomes a channel consumer.** The `TtyAdapter`
no longer owns its own chunk format or its own ALPN. It receives a
`Connection` from the channels layer and drives sessions the same way it does
today. The chunk format becomes the channels layer's concern, not TTY's. The
`alknet/tty` ALPN remains for direct connections (no channels layer), but the
primary path is through channels.

**Path B: TTY keeps its own ALPN and chunk format.** The channels layer and
TTY coexist. TTY's chunk format is a fixed 4-channel subset of the channels
format. A future migration could unify them. This is lower-risk but leaves
two chunk formats in the codebase.

The `TtyBackend` trait and `TtyHandle` are unchanged. The `DockerTtyBackend`
and `LocalTtyBackend` don't need to know about channels — they still
implement `TtyBackend::allocate()` and return a `TtyHandle`. The channels
layer handles the wire format; the TTY adapter handles the session
lifecycle.

### alknet-call

The call protocol is unchanged. It remains JSON-only, `EventEnvelope`-based.
It runs on channel 0 exactly as it runs on a top-level `alknet/call` QUIC
connection — the `CallAdapter` receives a `Connection` and dispatches
operations. The only difference is that the `Connection` is backed by chunk
reassembly on channel 0 rather than by a QUIC bidi stream.

What changes is that the call protocol gains a new class of operations:
channel lifecycle operations.

New call operations:
- `channel/open` — open a data channel with a given ALPN and params.
  Bidirectional: either side can initiate.
- `channel/close` — close a data channel
- `channel/control` — send a control message on a channel's stream_type 3

Existing call operations are unchanged. The call protocol doesn't need to
know about raw bytes — it just hands out channel tokens and dispatches
control operations. The `from_call` operation overlay (where each side
populates what operations they can call) extends naturally to channels:
either side can expose resources the other can open channels to.

### alknet-ssh

The SSH crate's relationship to channels is twofold:

1. **SSH as a channel type**: an `alknet/ssh` channel carries the SSH binary
   protocol over stream_types 0 and 1. The channels layer hands the
   reassembled stream to `SshAdapter`, which feeds it to russh. Russh
   multiplexes SSH channels inside. This is SSH-over-channels — one level of
   multiplexing (channels) carrying another (SSH).

2. **SSH as a channels transport**: an SSH `direct-tcpip` channel could
   carry a channels connection. This is channels-over-SSH — the reverse
   composition. Useful for tunneling alknet through an SSH bastion.

The SSH crate doesn't need to know about channels. It implements
`ProtocolHandler` for `alknet/ssh` and accepts a `Connection`. Whether that
`Connection` is a top-level QUIC connection or a channels data channel is
transparent to the handler.

### alknet-docker

The docker crate registers call operations on the call protocol
(`docker/container/list`, `docker/container/create`, etc.) and a
`DockerTtyBackend` for interactive sessions. With channels:

- Docker lifecycle operations are call operations on channel 0 (unchanged
  from ADR-058).
- Interactive exec/attach opens a TTY channel via `channel/open` with ALPN
  `alknet/tty` and backend `docker`. The channels layer routes to the
  `TtyAdapter`, which dispatches to `DockerTtyBackend`.
- No separate `alknet/tty` connection needed. One `alknet/channels`
  connection handles both JSON operations and raw TTY sessions.

## Transport Agnosticism

The channels wire format works over any ordered, reliable bidirectional byte
stream:

| Transport | How |
|-----------|-----|
| QUIC bidi stream | `alknet/channels` ALPN on a QUIC connection; one bidi stream carries all channels |
| TCP+TLS | `alknet/channels` ALPN on a TLS connection; the TCP stream carries all channels |
| WebTransport | `alknet/channels` session proxied through the h3 handler (ADR-040) |
| SSH channel | channels connection riding inside an SSH `direct-tcpip` channel |
| Another channels connection | recursive composition (channel type `alknet/channels`) |

The same wire format, the same chunk reassembly, the same `Connection`
abstraction. The transport is a parameter, not a design constraint.

## WASM Compatibility

The channels wire format is pure byte manipulation: read a 9-byte header,
split the payload, route to the right reassembly buffer. No platform
dependencies, no tokio-specific types in the core splitting/recombining
logic. A WASM build can:

1. Read chunks from a WebTransport `BiStream`
2. Reassemble `(channel_id, stream_type)` streams
3. Present them as `AsyncRead + AsyncWrite` handles to WASM-compatible
   handlers

The handlers themselves may or may not be WASM-compatible (russh's client is;
`portable_pty` is not). But the channels layer — the multiplexing proxy —
is WASM-compatible by construction. A browser can run a channels client in
WASM over WebTransport, opening TTY channels, SSH channels, and tunnel
channels, with the call protocol on channel 0 orchestrating everything.

## ACL and Security Model

### Channel open is gated by the call protocol's ACL

Every `channel/open` operation goes through the call protocol's existing
`OperationContext` (identity, scopes, capabilities, ownership). No channel
opens without authorization. The ACL check happens before the handler is
invoked.

### No open ports by default

A core design principle carried from the SSH research (DP-5: default-deny
baseline): alknet endpoints never expose ports, sockets, or listeners unless
explicitly configured. Tunnels are channels, not ports. A `tunnel/open`
operation targets a resource (`container:abc123`, `service:postgres`), not a
port number. The hub resolves the resource to a stream. No listener, no
socket, no port — just a channel.

This is the "VPN-like without being a VPN" property: bidirectional stream
proxying through channels, gated by ACL, with no network-level exposure.

### Resources, not addresses

Forwarding destinations and tunnel targets are expressed as resources in the
alknet identity/ownership model, not as IP:port pairs. A docker container
hosting a service is a resource (`container:abc123`). Access is granted
through ownership or scopes, not through firewall rules. This avoids the
global trend of VPN restrictions — there are no VPN ports to block because
there are no VPN ports.

## Decision Points

### DP-1: Channel ID allocation — server-assigned vs client-assigned vs negotiated

*(Recommended: server-assigned, returned in the `channel/open` response)*

Three options:
- **(a) Client-assigned**: client picks a `channel_id` in the open request.
  Risk of collision if two clients open channels simultaneously.
- **(b) Server-assigned**: server returns the `channel_id` in the open
  response. No collision risk. Slight latency (one round-trip before data
  can flow).
- **(c) Negotiated ranges**: client and server each own half the ID space
  (e.g., client uses odd IDs, server uses even). SSH uses this. More
  complex, but zero-round-trip channel open.

**Recommendation**: **(b) server-assigned** for simplicity. The round-trip
for channel open is acceptable — it's the same round-trip the call protocol
already makes for every operation. If zero-round-trip channel open becomes a
performance requirement, option (c) can be added as an extension (two-way
door).

### DP-2: Channel 0 — pre-negotiated `alknet/call`, not a special control plane

*(Recommended: channel 0 is just `alknet/call` pre-negotiated — no special framing)*

Channel 0 is not a "control plane" with its own wire format. It is simply
the `alknet/call` ALPN, pre-negotiated: both sides know that channel 0 is
routed to the `CallAdapter` without an explicit `channel/open` exchange. The
call protocol runs on channel 0 exactly as it runs on a top-level
`alknet/call` QUIC connection — `EventEnvelope` frames on a bidirectional
stream backed by chunk reassembly.

This means:
- Channel 0 uses the same chunk format as every other channel
  (`channel_id=0` in the header)
- The `CallAdapter` receives a `Connection` and dispatches operations —
  unchanged from today
- Channel lifecycle operations (`channel/open`, `channel/close`) are just
  call operations on channel 0, dispatched through the `OperationRegistry`
- No special framing disambiguation is needed — `channel_id` already
  distinguishes channel 0 from data channels

**Recommendation**: channel 0 is `alknet/call` pre-negotiated. No special
framing, no separate control-plane wire format. The channels layer is a
transparent proxy: every channel (including channel 0) is just an ALPN
routed through the `HandlerRegistry`.

### DP-3: TTY chunk format — absorb into channels vs keep separate

*(Recommended: absorb into channels, keep `alknet/tty` ALPN as a direct-connect shortcut)*

The TTY crate's chunk format (`[stream_type: u8][length: u32 be]`) is a
subset of the channels format (it's a channels chunk with `channel_id`
implicitly 0 and only stream_types 0-3). Two paths:

- **(a) Absorb**: TTY sessions use the channels format. The `TtyAdapter`
  becomes a channel consumer. The `alknet/tty` ALPN remains as a
  direct-connect shortcut (no channels layer needed for simple cases), but
  the primary path is through channels.
- **(b) Keep separate**: TTY keeps its own chunk format and ALPN. Channels
  and TTY coexist as separate multiplexing strategies. Two chunk formats in
  the codebase.

**Recommendation**: **(a) absorb**. The TTY crate's chunk format was always
described as "a deliberately impoverished version of SSH's channel
multiplexer" (alknet-tty phase-0-findings.md). Channels is the
non-impoverished version. The `alknet/tty` ALPN stays for direct connections
(useful for simple deployments, browser terminals), but the chunk format
unifies under channels.

### DP-4: Control messages — call protocol operations vs stream_type 3 chunks

*(Recommended: both, with clear division of responsibility)*

Channel-specific control can flow through two paths:
- **(a) Call protocol operations**: `tty/resize`, `tty/signal`,
  `channel/close` — dispatched through the `OperationRegistry` on channel 0.
  Benefits from the call protocol's existing auth, routing, and
  observability.
- **(b) stream_type 3 chunks**: inline control messages on the data
  channel. Lower latency (no channel 0 round-trip), but requires the data
  channel handler to parse and dispatch them.

**Recommendation**: **both**. Use call protocol operations for
channel lifecycle (open, close) and infrequent control (resize, signal). Use
stream_type 3 for control that must be ordered relative to data (EOF before
exit). The TTY crate's exit-chunk-is-last invariant (ADR-055) is an example
of ordering-sensitive control that belongs on the data channel. The division
is: call protocol for orchestration, stream_type 3 for data-ordered control.

### DP-5: Stream reassembly — backpressure and flow control

*(Recommended: rely on the transport's flow control; add channel-level
windowing only if needed)*

QUIC provides per-stream flow control. TCP provides connection-level flow
control. The channels layer adds a third multiplexing level. If one data
channel's consumer is slow, it could block all other channels on the same
transport (head-of-line blocking).

Options:
- **(a) No channel-level flow control**: rely on the transport's flow
  control. Simple, but susceptible to head-of-line blocking.
- **(b) Channel-level windowing**: each channel has its own flow-control
  window (like SSH's `ChannelParams.window_size`). Prevents head-of-line
  blocking but adds complexity.
- **(c) Bounded channels with backpressure**: each channel's reassembly
  buffer has a maximum size; when full, the sender stops reading chunks for
  that channel. Simpler than full windowing, prevents unbounded memory
  growth.

**Recommendation**: **(c) bounded channels** for v1. A configurable maximum
buffer size per channel (default 1 MiB). When a channel's buffer is full, the
channels layer stops reading chunks for that `channel_id` until the consumer
drains it. This prevents memory exhaustion without the complexity of SSH's
sliding-window protocol. If head-of-line blocking becomes a real problem
(which it likely won't for the intended use cases — TTY sessions and tunnels
are not high-throughput), full windowing can be added as an extension
(two-way door).

### DP-6: Crate structure — new crate vs extension of alknet-tty

*(Recommended: new `alknet-channels` crate)*

The channels layer is a generalization of TTY's chunk format, but it's a
different thing: a multiplexing proxy, not a terminal session handler. Two
options:
- **(a) New crate `alknet-channels`**: depends on `alknet-core` (for
  `ProtocolHandler`, `Connection`, `HandlerRegistry`). Defines the chunk
  format, the `ChannelConnection`, and the `ChannelsAdapter`. TTY becomes a
  consumer.
- **(b) Extend `alknet-tty`**: add `channel_id` to the chunk format, add
  the `ChannelConnection` abstraction. TTY is the base; channels is the
  generalization. Simpler crate graph, but blurs the conceptual boundary.

**Recommendation**: **(a) new crate**. The channels layer is a distinct
concept (multiplexing proxy) from TTY (terminal session handler). Keeping
them separate follows the existing pattern (ADR-003: no-handler-depends-on-
another-handler) and makes the dependency graph clear: `alknet-channels`
depends on `alknet-core`; `alknet-tty` depends on `alknet-core` (unchanged);
both register on the same `HandlerRegistry`.

## Straightforward Parts

### 1. The chunk format is a thin extension of a proven design

The TTY crate's chunk format (`[stream_type: u8][length: u32 be]`) is
validated by two POCs (alknet-docker-poc, alknet-tty-poc) and implemented in
production code (`crates/alknet-tty/src/wire.rs`). Adding a `channel_id: u32`
prefix is a 4-byte extension to a proven format. The `ChunkReader`/
`ChunkWriter` pattern, the framing disambiguation trick, and the
bidirectional pump pattern all carry forward.

### 2. The `Connection` abstraction already supports this

`alknet-core`'s `Connection` type already wraps multiple backends (QUIC,
iroh, raw stream) behind a uniform `accept_bi()` / `open_bi()` interface. A
`ChannelConnection` is a new backend for the same interface. The
`HandlerRegistry` and all `ProtocolHandler` implementations work unchanged.

### 3. The call protocol needs no wire-format changes

The call protocol stays JSON-only, `EventEnvelope`-based. The new
`channel/open`, `channel/close`, and channel-scoped control operations are
just new operation types in the `OperationRegistry`. No new framing, no new
carriage types, no protocol version bump.

### 4. WASM compatibility is by construction

Chunk splitting and reassembly is pure byte manipulation. The channels layer
has no platform dependencies. A WASM build can read chunks from a
WebTransport stream, reassemble them, and present `AsyncRead + AsyncWrite`
handles to WASM-compatible handlers. The browser case (xterm.js over
WebTransport to a TTY channel) works without the browser implementing SSH.

### 5. ACL is inherited from the call protocol

Channel open goes through the call protocol's existing `OperationContext`.
Identity, scopes, capabilities, and ownership checks apply to channel open
exactly as they apply to any other call operation. No new auth machinery.

## Less Straightforward Parts

### The TTY control channel is currently not bidirectional

The TTY crate's `ControlMessage` enum has server→client variants (`Exit`)
and client→server variants (`Resize`, `Signal`, `Eof`), but the adapter
ignores `Exit` from the client. The control channel *could* be fully
bidirectional — there's no wire-format reason it isn't. With channels, the
call protocol on channel 0 handles bidirectional control, and stream_type 3
on the data channel handles data-ordered control. The TTY crate's
`ControlMessage` enum may need to be split or generalized.

### The TTY adapter's session lifecycle assumes it owns the stream

The `TtyAdapter::handle()` currently loops `accept_bi()`, spawning a
`drive_session` task for each stream. Inside a channels connection, the
adapter receives a `Connection` that yields bidi streams — but those streams
are already channel-demuxed. The adapter doesn't need to know this. The
question is whether the adapter's current "one session per bidi stream"
model maps cleanly to "one session per channel." It should — a channel is
just a bidi stream with extra metadata.

### The exit-chunk-is-last invariant (ADR-055) with channel close

The TTY crate guarantees the exit chunk is the last chunk before stream
close. With channels, the exit chunk is the last chunk on that channel, and
then a `channel/close` operation on channel 0 signals completion. The
ordering between the exit chunk and the close operation must be preserved:
the exit chunk must be written and flushed before the close operation is
sent. This is an implementation constraint, not a design change.

### Single-stream throughput ceiling and the recursive escape hatch

All channels on one `alknet/channels` connection share one transport
stream's flow-control window. On QUIC that's one bidi stream's window; on
TCP it's the connection's. A slow consumer on one channel can backpressure
others (DP-5's bounded-buffer mitigation handles memory exhaustion but not
throughput). For high-throughput bulk transfer (e.g., a large file
download), parallel QUIC bidi streams or parallel connections will beat one
multiplexed stream — same trade-off as HTTP/2-over-TLS vs HTTP/3-over-QUIC.

This is **not a channels concern**. The 9-byte chunk overhead is
negligible; the cost is the shared transport window, which is a property of
the underlying transport, not the channels multiplexer. The channels layer
is deliberately not a throughput optimizer — it's a multiplexing proxy.

The **recursive composition** (§Recursive Composition) is the escape hatch,
but not in the "cross-connection token" sense. The answer is simpler: a
client that wants parallel throughput opens **N independent
`alknet/channels` connections** and load-balances across them (e.g., a file
transfer splits byte ranges across channels on different connections).
Each connection is self-contained — its own channel 0, its own flow-control
window, its own backpressure. No token, no cross-wiring between
connections, no shared state across the connections. The `Connection`
abstraction already treats each one uniformly.

What the channels layer deliberately does **not** do:

- **No cross-connection channels.** A channel on connection B is not
  openable from connection A's channel 0. Each channels connection is
  self-contained; its channel 0 orchestrates only its own channels.
- **No cross-connection coordination primitives.** No token exchange, no
  shared `channel_id` namespace, no "open a channel on that other
  connection" operation. That coordination belongs at the call-protocol /
  application layer ("open another channels connection, here's its
  address"), not inside the channels multiplexer.
- **No parallel-transfer optimization in the channels layer.** Splitting a
  download across N connections and N channels is a **client optimization for
  a specific edge case** (fast file transfer). It's a real and useful
  concern, but it's a downstream client concern — the fs/storage client
  decides to open N connections and chunk the byte ranges. The channels
  layer's job is to carry one channel's bytes well; the client's job is to
  decide how many channels connections it needs.

For the intended use cases (TTY sessions, SSH, tunnels, call operations)
the single-stream ceiling will not bite — network RTT and handler work
dominate, and SSH tunneling (which has the same single-stream property) works
fine even for port-forwarding Redis. The throughput ceiling only matters for
bulk file transfer, and there the application uses multiple connections —
same as any multiplexer.

## Open Questions to Carry into Phase 1

- **OQ-CH-01 (channel open round-trip)**: server-assigned channel IDs mean
  one round-trip before data can flow. Is this acceptable for all channel
  types? For TTY sessions the negotiation frame already requires a
  round-trip. For tunnels it adds latency to connection establishment. If
  zero-round-trip open is needed, negotiated ID ranges (DP-1 option c) can
  be added.

- **OQ-CH-02 (TTY migration path)**: if the TTY chunk format is absorbed
  into channels, what's the migration path for existing `alknet/tty`
  deployments? The `alknet/tty` ALPN stays as a direct-connect shortcut.
  Does the TTY adapter's wire format change, or does it keep its current
  chunk format for direct connections and use the channels format when
  inside a channels connection?

- **OQ-CH-03 (channel-level flow control)**: is bounded-buffer backpressure
  (DP-5 option c) sufficient, or will head-of-line blocking be a real
  problem? The intended use cases (TTY sessions, tunnels, SSH connections)
  are not high-throughput. If a use case emerges that saturates a channel
  (e.g., file transfer over a tunnel), channel-level windowing may be
  needed.

- **OQ-CH-04 (channel ID reuse)**: after a channel is closed, can its
  `channel_id` be reused? SSH allows reuse. The channels format should
  allow it too, but the reassembly buffers must be fully drained before
  reuse to prevent data from the old channel leaking into the new one.

- **OQ-CH-05 (maximum channels per connection)**: is there a limit? SSH
  uses `u32` channel IDs, effectively unlimited. The channels format should
  match. The practical limit is memory (reassembly buffers per channel) and
  the transport's flow control.

- **OQ-CH-06 (channel open denial-of-service)**: an authenticated peer
  could open many channels and never read from them, exhausting memory.
  Bounded per-channel buffers (DP-5) limit the damage. A per-connection
  channel limit (configurable, default e.g. 256) provides defense in depth.

- **OQ-CH-07 (POC scope)**: what should a Phase 0 POC validate? Minimum:
  chunk format round-trip with multiple channels, channel open/close via
  call protocol, TTY session inside a channel. Stretch: SSH connection
  inside a channel, tunnel inside a channel, recursive composition.

- **OQ-CH-08 (channel/resources staleness)**: `channel/resources` is a
  snapshot of what a side exposes at call time. Resources can appear and
  disappear (a docker container starts/stops, a worker connects/disconnects).
  Does the channels layer need a `channel/resources/subscribe` streaming
  operation (analogous to a subscription), or is polling `channel/resources`
  sufficient for v1? The call protocol already has streaming
  `OperationType::Subscription`; reusing it here is natural but adds a
  long-lived stream on channel 0 per interested peer. Recommendation for
  Phase 1: poll for v1, add subscription if staleness bites.

- **OQ-CH-09 (responder-to-initiator channel lifecycle)**: in the
  `responder-to-initiator` open case (worker exposes, hub consumes), who
  drives the data first? The channel is allocated on the worker's side, but
  the hub is the client of the ALPN — does the hub's handler start pumping
  immediately, or does the worker's handler wait for the hub to write first?
  For TTY the server writes the negotiation response first; for a tunnel the
  client connects first. This is ALPN-specific and probably doesn't need a
  channels-layer rule, but the `direction` field's effect on which side's
  `ChannelConnection` is the "server" vs "client" of the ALPN needs to be
  pinned down in Phase 1.

- **OQ-CH-10 (ChannelConnection: typed destructure vs generic Connection)**:
  the doc proposes that `ChannelConnection` implements the `Connection`
  trait (for recursion and generic handlers) **and** can be destructured
  into typed sub-stream handles (e.g. `TtyChannel { stdin, stdout, stderr,
  control }`). Is the typed destructure a `ChannelConnection` method, a
  separate trait, or an ALPN-specific constructor in the handler crate?
  This affects whether `alknet-channels` needs to know about TTY's
  `stream_type` semantics or whether the TTY crate does the destructure
  itself given a `ChannelConnection`. Recommendation: the TTY crate
  destructures — `alknet-channels` exposes `(channel_id, stream_type) →
  (SendStream, RecvStream)` accessors and the handler crate maps those to
  its typed names.

- **OQ-CH-11 (hub relay channel_id remapping)**: when the hub bridges a
  browser channel (`channel_id=7`) to a spoke channel (`channel_id=12`),
  the relay rewrites the `channel_id` field in each chunk header. But
  `channel/control` and `channel/close` operations on channel 0 carry
  `channel_id` in their *JSON payload*, not in the chunk header. Does the
  hub's call-protocol forwarding (via `from_call`) need to rewrite
  `channel_id` inside the `call.requested` payload too? This is a
  relay-level concern: the hub is terminating channel 0 on both legs (it
  runs its own `CallAdapter`), so it likely translates `channel/open` from
  the browser into a *new* `channel/open` on the spoke (not a raw forward),
  and the spoke's returned `channel_id` is mapped back to the browser's
  side. Phase 1 must specify whether the hub translates or transparently
  forwards, and how the `channel_id` mapping is maintained.

- **OQ-CH-12 (unknown channel_id on demux)**: when the demux receives a
  chunk with a `channel_id` it has not allocated, what does it do? Options:
  (a) drop with a debug log (lenient — survives transient mis-ordering
  during channel teardown), (b) return a protocol error and close the
  transport (strict — catches bugs but is fragile during teardown). SSH
  is lenient. Recommendation: lenient for v1, with an error counter for
  observability. See `poc-plan.md` §Step 1.

- **OQ-CH-13 (core trait for bidi-stream sources)**: the POC
  (`poc-plan.md`) uses `Connection::from_stream` per channel, which is
  yield-once. A Phase 1 refactor may add a `BidiStreamSource` trait to
  `alknet-core` so `ChannelConnection` (many channels, each a bidi stream)
  is a first-class peer of QUIC (many bidi streams) rather than a bag of
  yield-once Connections:

  ```rust
  trait BidiStreamSource: Send + Sync {
      async fn accept_bi(&self) -> Result<(SendStream, RecvStream), StreamError>;
      async fn open_bi(&self) -> Result<(SendStream, RecvStream), StreamError>;
  }
  ```

  with `Connection` holding `Box<dyn BidiStreamSource>`, and QUIC/Iroh/
  Stream/Channels all implementing it. This is **additive** (existing
  callers keep working via a blanket impl or a `from_stream`-backed
  default) and not a major/pita break — it mostly adds a new variant and
  trait-ifies the existing `accept_bi`/`open_bi` methods behind a trait
  object. The POC does NOT need this — it validates the yield-once path is
  sufficient — but Phase 1 should evaluate whether the trait makes the
  channels layer and the client-side endpoint (OQ-CH-14) cleaner. The
  expectation is that this route is +EV: it's not a massive change and
  will make things a lot easier downstream.

- **OQ-CH-14 (client-side channels endpoint)**: both sides of a channels
  connection do the demux/mux work. The server side is a `ProtocolHandler`
  (`ChannelsAdapter::handle`). The client side needs a symmetric type —
  something like `ChannelClient` that opens a transport, runs the demux/
  mux, and exposes `open_channel(alpn, params) -> Channel` to the
  application. This is the channels analogue of `AlknetEndpoint` (server)
  vs `CallClient` (client) in the call protocol. The POC uses a POC-local
  client type; Phase 1 must decide whether this lives in `alknet-channels`
  or is a thin wrapper over `alknet-core`'s endpoint types. The
  `BidiStreamSource` trait (OQ-CH-13) may factor into this — if
  `AlknetEndpoint` and `ChannelClient` both produce `BidiStreamSource`s,
  the client/server symmetry is cleaner. After the POC we're probably
  going to need some light refactoring to the core to make these easier;
  the good news is it should mostly be additive and not breaking in
  major/pita ways.

## Recommended Approach

### Crate

`alknet-channels`, depends on `alknet-core` (for `ProtocolHandler`,
`Connection`, `HandlerRegistry`, `SendStream`, `RecvStream`). Defines:

- The chunk format (`[channel_id: u32 be][stream_type: u8][length: u32 be]`)
- `ChunkReader` / `ChunkWriter` (generalized from TTY's wire module)
- `ChannelConnection` (implements the `Connection` interface over chunk
  reassembly)
- `ChannelsAdapter` (`ProtocolHandler` for `alknet/channels`)
- Channel open/close call operations

Does not depend on `alknet-tty`, `alknet-call`, `alknet-ssh`, or any other
handler crate. The dependency direction is: handlers depend on
`alknet-core`; `alknet-channels` depends on `alknet-core`; nothing depends
on `alknet-channels` except the assembly layer that registers it on the
`HandlerRegistry`.

### Build order

**Step 1: Wire format + mock channels.**
- Implement the 9-byte chunk format with `channel_id`.
- Implement `ChunkReader` / `ChunkWriter` (generalized from TTY's
  `wire.rs`).
- Implement `ChannelConnection` with mock channels (in-memory pipes).
- Validate chunk round-trip with multiple channels, concurrent reads/writes.
- **Result**: a working chunk multiplexer with no real handlers.

**Step 2: Channel open/close via call protocol.**
- Implement `channel/open` and `channel/close` call operations.
- Integrate with `HandlerRegistry` — `channel/open` looks up the ALPN,
  validates ACL, invokes the handler.
- Validate: open a channel, pump data, close the channel.
- **Result**: call protocol orchestrating channel lifecycle.

**Step 3: TTY inside channels.**
- Route `alknet/tty` channel opens to the `TtyAdapter`.
- Validate: `channel/open` with ALPN `alknet/tty` → TTY session inside a
  channel.
- **Result**: TTY sessions work through channels. The docker use case (call
  operations + TTY sessions on one connection) is validated.

**Step 4: Tunnel channels.**
- Implement a simple tunnel handler: `channel/open` with ALPN
  `alknet/tunnel` and a target resource → bidirectional byte proxy.
- Validate: tunnel a TCP connection through a channel.
- **Result**: the "VPN-like without being a VPN" property is proven.

**Step 5: SSH inside channels (stretch).**
- Route `alknet/ssh` channel opens to the `SshAdapter`.
- Validate: SSH connection inside a channel.
- **Result**: SSH-over-channels works. The full stack (call + TTY + SSH +
  tunnel on one connection) is validated.

### De-risk POC

A detailed POC plan lives in `docs/research/alknet-channels/poc-plan.md`.
Summary: a standalone POC at `/workspace/@alkdev/alknet-channels-poc/` that
validates the three highest-leverage unknowns in three independently-runnable
steps:

1. **Chunk format + N-channel demux/mux** — generalize TTY's 5-byte
   `ChunkReader`/`ChunkWriter` to the 9-byte format, build a demux that
   routes chunks to per-channel `mpsc` channels and a mux that frames
   per-channel bytes back onto the transport. Validates the
   decompose→stream→recompose round-trip for 3+ concurrent channels.
2. **Per-channel `Connection` presentation** — wrap each reassembled
   channel as `Connection::from_stream` and run a minimal `ProtocolHandler`
   (echo) through the full demux→Connection→handler→mux path. Validates
   that the existing `Connection` abstraction (from transport-generalization)
   is sufficient; no core changes needed for the POC.
3. **Tunnel handler** — a `channel/open` with a target address opens a
   `TcpStream` and pumps bidirectionally. Validates that the same concepts
   behind the TTY crate work as a generic port proxy.

Stretch goals: WASM build of the sync core; two different channel types
(TTY-shaped 4-stream + tunnel 2-stream) on one connection; hub relay sketch
with `channel_id` remapping (OQ-CH-11).

The POC deliberately does NOT do: the call protocol (channel/open etc. are
Phase 1's concern, already in production), real transport (uses
`tokio::io::duplex`), ACL, real adapters, or recursive composition.

The POC surfaces three open questions carried into Phase 1: OQ-CH-12
(unknown channel_id on demux), OQ-CH-13 (core `BidiStreamSource` trait —
likely +EV additive refactor after the POC), and OQ-CH-14 (client-side
channels endpoint — the symmetric `ChannelClient` type).

The POC lives at `/workspace/@alkdev/alknet-channels-poc/` (mirroring the
`alknet-tty-poc` convention), depends on `alknet-core` only.

## References

- `docs/research/alknet-channels/poc-plan.md` — the detailed de-risk POC
  plan (Step 1: chunk format + demux/mux, Step 2: per-channel Connection,
  Step 3: tunnel handler).
- `docs/research/alknet-tty/phase-0-findings.md` — the TTY crate's chunk
  format, control channel, and backend trait. The seed of the channels
  generalization.
- `docs/research/alknet-ssh/phase-0-findings.md` — SSH's channel
  multiplexer, the channel decomposition (Layers 1-7), the default-deny
  baseline.
- `docs/research/alknet-docker/poc-summary.md` — the docker POC that
  validated the raw chunk format and the two-carriage model.
- `crates/alknet-tty/src/wire.rs` — the current chunk format implementation
  (`ChunkReader`, `ChunkWriter`, `[stream_type: u8][length: u32 be]`).
- `crates/alknet-tty/src/control.rs` — the `ControlMessage` enum (resize,
  signal, eof, exit).
- `crates/alknet-tty/src/adapter.rs` — the `TtyAdapter` and `drive_session`
  bidirectional pump.
- `crates/alknet-core/src/types.rs` — `ProtocolHandler`, `Connection`,
  `SendStream`, `RecvStream`, `HandlerRegistry`.
- `crates/alknet-call/src/protocol/wire.rs` — `EventEnvelope`,
  `FrameFramedReader` / `FrameFramedWriter`.
- `crates/alknet-call/src/protocol/dispatch.rs` — `Dispatcher::run_loop`,
  `handle_stream`, `pump_stream`.
- `/workspace/russh/russh/src/channels/` — russh's channel implementation
  (`ChannelMsg`, `Channel`, `ChannelReadHalf`, `ChannelWriteHalf`,
  `WindowSizeRef`).
- `docs/architecture/decisions/001-alpn-protocol-dispatch.md` — ALPN
  dispatch.
- `docs/architecture/decisions/002-protocol-handler-trait.md` —
  ProtocolHandler.
- `docs/architecture/decisions/003-crate-decomposition.md` —
  no-handler-depends-on-another-handler.
- `docs/architecture/decisions/040-webtransport-alpn-stream-proxy.md` —
  WebTransport stream proxy (the browser path for channels).
- `docs/architecture/decisions/052-tty-wire-format.md` — TTY wire format
  (ADR-052).
- `docs/architecture/decisions/055-tty-exit-chunk-ordering.md` — exit-chunk-
  is-last invariant (ADR-055).
- `docs/architecture/decisions/058-docker-call-alpn.md` — docker on
  `alknet/call` (ADR-058).
