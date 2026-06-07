# ADR-026: Transport/Interface Separation (Three-Layer Model)

## Status

Accepted

## Context

In the current architecture, SSH is deeply embedded in the server handler. The
`ServerHandler` owns auth, channel management, and proxy logic — all mixed
together. This makes it impossible to run the call protocol over any transport
that doesn't speak SSH, such as:

- **DNS** — encoding call protocol frames as DNS TXT queries/responses for
  censorship resistance
- **Raw framing** — 4-byte length prefix + JSON `EventEnvelope` without SSH
  wrapping, for local service mesh or browser-to-head direct communication
- **WebTransport** — running call protocol over QUIC streams (browsers can't do
  SSH key exchange)

The DNS control channel concept from research (`core.md`) currently conflates
"DNS as a transport that moves bytes" with "SSH sessions over those bytes." But
SSH is not a transport — it's a protocol layer that sits *on top of* a
transport. Separating them enables the DNS control channel to carry call
protocol events directly, without wrapping SSH inside DNS queries.

The same separation enables raw framing (no SSH overhead) for trusted local
networks, and WebTransport direct call protocol for browser clients.

## Decision

**Establish a three-layer model:**

### Layer 1: Transport

Produces byte streams. A `Transport` still produces
`AsyncRead + AsyncWrite + Unpin + Send`. This layer is unchanged from ADR-001.

```rust
#[async_trait]
pub trait Transport: Send + Sync + 'static {
    type Stream: AsyncRead + AsyncWrite + Unpin + Send + 'static;
    async fn connect(&self) -> Result<Self::Stream>;
    fn describe(&self) -> String;
}
```

Transports: TCP, TLS, iroh, DNS (as byte carrier), WebTransport (future).

### Layer 2: Interface

Consumes a `Transport::Stream` and produces call protocol sessions. An
interface is what SSH currently does: wrap a byte stream in session semantics.

```rust
#[async_trait]
pub trait Interface: Send + Sync + 'static {
    type Session;
    async fn accept(stream: TransportStream, config: &InterfaceConfig) -> Result<Self::Session>;
}
```

Interfaces:

- **SSH interface** — wraps existing `ServerHandler` logic. SSH handshake, auth,
  channel multiplexing. The call protocol runs over a reserved SSH channel
  (`alknet-control:0`).
- **Raw framing interface** — 4-byte big-endian length prefix + JSON
  `EventEnvelope`. No SSH overhead. Direct call protocol over the transport
  stream.
- **DNS control channel** — a (DNS transport, raw framing interface) pair that
  encodes/decodes `EventEnvelope` frames as DNS query/response pairs.

### Layer 3: Protocol

Carries semantics. Call protocol events, operation registry, service calls.
The protocol is agnostic to both the transport and the interface below it. It
receives `EventEnvelope` frames from whatever interface produced them.

### Connection Model

A **connection** is always a (Transport, Interface) pair. The valid combinations are enumerated:

| Transport | Interface | Use case |
|-----------|-----------|----------|
| TLS | SSH | Standard alknet tunnel |
| TCP | SSH | Plain SSH tunnel |
| iroh | SSH | P2P SSH tunnel |
| DNS | raw framing | DNS control channel |
| WebTransport | SSH | Browser SSH tunnel (future) |
| WebTransport | raw framing | Browser call protocol (future) |
| TCP | raw framing | Direct call protocol, local mesh |

**The DNS control channel carries call protocol frames directly — it does NOT
wrap SSH inside DNS.** This is explicit because the research originally
conflated "SSH tunneling over DNS" with "DNS as a transport for call protocol."
The (DNS, raw framing) pair sends `EventEnvelope` frames as DNS TXT
queries/responses — no SSH involved.

### `TransportKind` Enum

The `TransportKind` enum (currently `Tcp | Tls | Iroh`) gains `Dns` and
`WebTransport` variants. Initially these are tags only — no acceptor
implementation. The full DNS and WebTransport implementations are Phase 4 work
per the integration plan.

```rust
pub enum TransportKind {
    Tcp,
    Tls { server_name: Option<String> },
    Iroh { endpoint_id: String },
    Dns { domain: String },
    WebTransport { host: String },
}
```

### ServerHandler Refactor

The existing `ServerHandler` is refactored into `SshInterface`. The interface
abstraction means the server's accept loop becomes:

```rust
// Pseudocode
let (transport, interface) = listener_config;
let stream = transport.accept().await?;
let session = interface.accept(stream, &config).await?;
// session produces call protocol events
```

The call protocol handler is interface-agnostic — it receives `EventEnvelope`
frames from any interface. Auth, forwarding policy, and operation routing happen
at Layer 3, not inside the SSH handler.

## Consequences

- **Positive**: Enables DNS control channel without SSH wrapping. The (DNS,
  raw framing) pair is a clean (Transport, Interface) combination.
- **Positive**: Enables raw framing for local service mesh. No SSH overhead for
  trusted networks.
- **Positive**: SSH becomes pluggable. The same call protocol handler works with
  any interface.
- **Positive**: `ServerHandler` is refactored into `SshInterface` — a smaller,
  more focused component that only handles SSH session management.
- **Positive**: Future WebTransport and WebSocket interfaces are additive — they
  implement the `Interface` trait without touching SSH code.
- **Negative**: This is the most invasive code change in Phase 1
  (integration-plan, Phase 1.8). SSH auth, channel management, and proxy logic
  are currently tangled in `ServerHandler`. Extracting them requires careful
  refactoring to maintain existing behavior.
- **Negative**: The `Interface` trait is new and untested. The design must
  accommodate both SSH's channel multiplexing and raw framing's single-stream
  model through the same abstraction.

## References

- [research/core.md](../../research/core.md) — Transport layer, DNS transport section
- [research/integration-plan.md](../../research/integration-plan.md) — Phase 1.8, three-layer model
- [transport.md](../transport.md) — Current Transport trait (unchanged at Layer 1)
- [server.md](../server.md) — Current ServerHandler (will become SshInterface)
- [ADR-001](001-pluggable-transport.md) — Transport trait produces stream (unchanged)
- [ADR-004](004-ssh-over-transport.md) — SSH runs over transport (reinforced by Layer 2)
- [ADR-024](024-bidirectional-call-protocol.md) — Bidirectional call protocol (Layer 3)