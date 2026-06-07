---
status: draft
last_updated: 2026-06-07
---

# Interface (Layer 2)

## What

The Interface layer sits between Transport (Layer 1) and Protocol (Layer 3).
An Interface consumes a `Transport::Stream` and produces call protocol sessions.
SSH is an interface, not a transport — it wraps a byte stream in session
semantics. Raw framing (4-byte length prefix + JSON `EventEnvelope`) is another
interface, one without SSH overhead.

## Why

In the current architecture, SSH is deeply embedded in `ServerHandler`. This
tangling of transport, interface, and protocol makes it impossible to:

- Run the call protocol over DNS queries without wrapping SSH inside DNS
- Use raw framing for local service mesh (no SSH overhead)
- Support WebTransport direct call protocol for browsers
- Separate auth mechanics from channel management

The three-layer model (ADR-026) cleanly separates these concerns. Transport
produces bytes. Interface parses bytes into sessions. Protocol carries
semantics. A connection is always a (Transport, Interface) pair.

## Architecture

### Three-Layer Model

```
Layer 3: Protocol    (Call protocol, Operations, OperationEnv)
Layer 2: Interface   (SSH, raw framing, HTTP/WS, DNS control channel)
Layer 1: Transport   (TCP, TLS, iroh, DNS, WebTransport)
```

- **Layer 1: Transport** — produces byte streams (`AsyncRead + AsyncWrite + Unpin
  + Send`). Unchanged per ADR-001.
- **Layer 2: Interface** — consumes a `Transport::Stream` and produces call
  protocol sessions. SSH does handshake + auth + channel multiplexing. Raw
  framing does length-prefix parsing.
- **Layer 3: Protocol** — carries semantics. Call protocol events, operation
  registry, service calls. Agnostic to both Transport and Interface below it.

### Interface Trait

```rust
#[async_trait]
pub trait Interface: Send + Sync + 'static {
    type Session;
    async fn accept(stream: TransportStream, config: &InterfaceConfig) -> Result<Self::Session>;
}
```

The session produced by an interface is consumed by the call protocol handler.
Different interfaces produce different session types, but the call protocol
handler receives `EventEnvelope` frames from any interface.

### SshInterface

Wraps the existing `ServerHandler` logic. This is the most complex interface
because SSH provides channel multiplexing, auth negotiation, and proxy
management within a single session.

What stays in SshInterface (Layer 2):
- SSH handshake and session management
- Auth delegation to `IdentityProvider` (via `auth_publickey()` callback)
- Channel multiplexing (multiple channels per session)
- `alknet-control:0` channel routing to call protocol

What moves to Layer 3 (call protocol handler):
- Operation registry and dispatch
- Forwarding policy checks (per ADR-031)
- Operation context construction (Identity, scopes)

What moves to per-connection state:
- Port forwarding proxy logic

### RawFramingInterface

Reads 4-byte big-endian length prefix + JSON `EventEnvelope` frames directly
from the transport stream. No SSH wrapping. No channel multiplexing — the
entire stream is a single call protocol channel.

```rust
pub struct RawFramingInterface;

impl Interface for RawFramingInterface {
    type Session = RawFramingSession;
    // Reads length-prefixed EventEnvelope frames from the stream
}
```

Used for:
- DNS control channel (DNS transport + raw framing)
- Local service mesh (TCP + raw framing, no SSH overhead)
- Browser direct call protocol (WebTransport + raw framing, future)

### DNS Control Channel

A (DNS transport, raw framing interface) pair. The DNS transport encodes
`EventEnvelope` frames as DNS query/response pairs. The raw framing interface
parses them directly — **NOT** SSH inside DNS.

```
Client: Encode EventEnvelope as base32 DNS query labels
  → DNS Transport → DNS Server → Raw Framing Interface → Call Protocol Handler

Server: Return EventEnvelope as DNS TXT record response
  ← Raw Framing Interface ← DNS Transport ← Call Protocol Handler
```

### Valid (Transport, Interface) Pairs

| Transport | Interface | Use case |
|-----------|-----------|----------|
| TLS | SSH | Standard alknet tunnel |
| TCP | SSH | Plain SSH tunnel |
| iroh | SSH | P2P SSH tunnel |
| DNS | raw framing | DNS control channel |
| WebTransport | SSH | Browser SSH tunnel (future) |
| WebTransport | raw framing | Browser call protocol (future) |
| TCP | raw framing | Direct call protocol, local mesh |

### InterfaceConfig

Different interfaces require different configuration:

```rust
pub enum InterfaceConfig {
    Ssh(SshInterfaceConfig),
    RawFraming(RawFramingConfig),
}

pub struct SshInterfaceConfig {
    pub auth: Arc<dyn IdentityProvider>,
    pub forwarding: Arc<ArcSwap<DynamicConfig>>, // for ForwardingPolicy
    pub host_key: Arc<PrivateKey>,
}

pub struct RawFramingConfig {
    // No SSH-specific config needed
    // Auth is handled by the transport layer (e.g., token auth for WebTransport)
    // or by the call protocol layer
}
```

### Auth Across Interfaces

- **SshInterface**: Auth happens during SSH handshake via
  `IdentityProvider::resolve_from_fingerprint()`. The authenticated `Identity`
  is attached to the session.
- **RawFramingInterface**: Auth is handled by the transport (e.g., token auth
  for WebTransport via `IdentityProvider::resolve_from_token()`) or by the call
  protocol layer (operation-level ACL).

Both paths produce the same `Identity` type (ADR-029).

### Server Accept Loop

With the Interface trait, the accept loop becomes:

```rust
for listener in listeners {
    let (transport, interface) = listener;
    tokio::spawn(async move {
        loop {
            let stream = transport.accept().await?;
            let session = interface.accept(stream, &config).await?;
            // session produces call protocol events
            // call protocol handler is interface-agnostic
        }
    });
}
```

## Constraints

- The Interface trait must accommodate both SSH's channel multiplexing and raw
  framing's single-stream model through the same abstraction.
- `SshInterface` is the most invasive refactoring in Phase 1. The existing
  `ServerHandler` owns auth, channel management, and proxy logic — extracting
  these cleanly requires careful design (integration-plan, Phase 1.8).
- DNS transport implementation is Phase 4 work. The `TransportKind::Dns` variant
  and `RawFramingInterface` are defined now; implementation is deferred.
- WebTransport is Phase 4 work. The `TransportKind::WebTransport` variant is a
  tag only for now.

## Open Questions

- **OQ-IF-01**: How does the `Interface` session type relate to the call
  protocol's `EventEnvelope` stream? Does every session implement
  `Stream<Item=EventEnvelope>`? This needs design during Phase 1.8.

- **OQ-IF-02**: Should `SshInterface` own the `ForwardingPolicy` check for
  `channel_open_direct_tcpip`, or should that move to Layer 3? Current thinking:
  the forwarding check is a Layer 3 concern (it's policy, not session mechanics),
  but the channel open/close lifecycle is Layer 2. The Interface reports channel
  open requests to Layer 3; Layer 3 applies `ForwardingPolicy` and tells
  Layer 2 whether to proxy.

## Design Decisions

| ADR | Decision | Summary |
|-----|----------|---------|
| [026](decisions/026-transport-interface-separation.md) | Three-layer model | SSH is Layer 2, not Layer 1 |
| [033](decisions/033-operationenv-irpc-call-protocol.md) | OperationEnv | Protocol is interface-agnostic |
| [029](decisions/029-identity-core-type.md) | Identity as core type | Auth resolution across interfaces |
| [031](decisions/031-forwarding-policy.md) | Forwarding policy | Layer 3 policy applied to Layer 2 channel requests |

## References

- [research/integration-plan.md](../research/integration-plan.md) — Phase 1.8, valid (Transport, Interface) pairs
- [research/core.md](../research/core.md) — DNS transport, three-layer model
- [ADR-026](decisions/026-transport-interface-separation.md) — Transport/interface separation
- [transport.md](transport.md) — Transport trait (unchanged at Layer 1)
- [server.md](server.md) — Current ServerHandler (will become SshInterface)
- [identity.md](identity.md) — IdentityProvider, auth across interfaces