---
status: draft
last_updated: 2026-06-09
---

# Interface (Layer 2)

## What

The Interface layer sits between Transport (Layer 1) and Protocol (Layer 3).
Interfaces consume byte streams from Transports or manage their own transports,
and produce call protocol sessions or handle discrete requests. SSH is an
interface, not a transport — it wraps a byte stream in session semantics. Raw
framing (4-byte length prefix + JSON `EventEnvelope`) is another interface.
HTTP and DNS are message-based interfaces that handle individual request/response
pairs without persistent sessions.

## Why

In the original architecture, SSH was deeply embedded in `ServerHandler`. This
tangling of transport, interface, and protocol made it impossible to:

- Run the call protocol over DNS queries without wrapping SSH inside DNS
- Use raw framing for local service mesh (no SSH overhead)
- Support WebTransport direct call protocol for browsers
- Separate auth mechanics from channel management
- Accept HTTP requests and map them to call protocol operations

The three-layer model (ADR-026) cleanly separates these concerns. Transport
produces bytes. Interface parses bytes into sessions or handles requests.
Protocol carries semantics. A connection is always a (Transport, Interface)
pair for stream-based interfaces, or a standalone message-based interface.

Phase 2 research identified that HTTP and DNS don't fit the persistent session
model — they're stateless per-request. This led to the StreamInterface /
MessageInterface split (ADR-035), which gives each interface category its own
trait with the right lifecycle and ownership model.

## Architecture

### Three-Layer Model

```
Layer 3: Protocol    (Call protocol, Operations, OperationEnv)
Layer 2: Interface   (StreamInterface: SSH, raw framing | MessageInterface: HTTP, DNS)
Layer 1: Transport   (TCP, TLS, iroh, WebTransport)
```

- **Layer 1: Transport** — produces byte streams (`AsyncRead + AsyncWrite + Unpin
  + Send`). Unchanged per ADR-001. DNS is NOT a transport.
- **Layer 2: Interface** — two categories:
  - **StreamInterface**: consumes a `TransportStream` and produces a long-lived
    session that yields `InterfaceEvent` frames.
  - **MessageInterface**: handles individual `InterfaceRequest` →
    `InterfaceResponse` pairs. Manages its own transport.
- **Layer 3: Protocol** — carries semantics. Call protocol events, operation
  registry, service calls. Agnostic to both Transport and Interface below it.

### StreamInterface Trait

```rust
#[async_trait]
pub trait StreamInterface: Send + Sync + 'static {
    type Session: InterfaceSession;

    async fn accept(
        &self,
        stream: Box<dyn TransportStream>,
        config: &InterfaceConfig,
    ) -> Result<Self::Session>;
}
```

The session produced by a `StreamInterface` is consumed by the call protocol
handler. Different stream interfaces produce different session types, but the
call protocol handler receives `InterfaceEvent` frames from any stream
interface.

### MessageInterface Trait

```rust
#[async_trait]
pub trait MessageInterface: Send + Sync + 'static {
    async fn handle_request(&self, request: InterfaceRequest) -> Result<InterfaceResponse>;
}
```

Message-based interfaces handle individual requests without persistent sessions.
They manage their own transport (HTTP server, DNS server) and normalize requests
into `InterfaceRequest` / `InterfaceResponse`.

### InterfaceRequest / InterfaceResponse

```rust
pub struct InterfaceRequest {
    pub operation_path: String,         // e.g., "/head/auth/verify"
    pub input: Value,                   // JSON input payload
    pub auth_token: Option<AuthToken>,  // Extracted from wire format
    pub metadata: HashMap<String, String>,
}

pub struct InterfaceResponse {
    pub result: Result<Value, CallError>,
    pub status: u16,                    // HTTP status, DNS result code, etc.
    pub headers: HashMap<String, String>,
}
```

The call protocol handler processes `InterfaceRequest` the same way it processes
`InterfaceEvent` frames — both resolve to operation invocations through
`OperationEnv`. The difference is framing: stream interfaces produce `InterfaceEvent`
frames from a continuous byte stream, message interfaces construct `InterfaceRequest`
from their wire format.

### InterfaceSession

Every stream interface session implements `InterfaceSession`:

```rust
pub struct InterfaceEvent {
    pub envelope: EventEnvelope,
    pub identity: Option<Identity>,
}

#[async_trait]
pub trait InterfaceSession: Send {
    async fn recv(&mut self) -> Option<InterfaceEvent>;
    async fn send(&mut self, envelope: EventEnvelope) -> Result<()>;
}
```

`InterfaceEvent` carries an `EventEnvelope` and the authenticated `Identity`.
The call protocol handler (Layer 3) receives `InterfaceEvent` frames and
processes them uniformly, regardless of whether they arrived over SSH or raw
framing.

### SshInterface (StreamInterface)

Wraps the existing `ServerHandler` logic. This is the most complex stream
interface because SSH provides channel multiplexing, auth negotiation, and
proxy management within a single session.

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

**Current implementation note**: `SshSession::recv()` and `SshSession::send()`
are stubs. The bridge from SSH channels to `InterfaceEvent` frames is
scheduled for Phase 2 implementation (see integration-plan.md Phase 2.1).

### RawFramingInterface (StreamInterface)

Reads 4-byte big-endian length prefix + JSON `EventEnvelope` frames directly
from the transport stream. No SSH wrapping. No channel multiplexing — the
entire stream is a single call protocol channel.

```rust
pub struct RawFramingInterface;

impl StreamInterface for RawFramingInterface {
    type Session = RawFramingSession;
    // Reads length-prefixed EventEnvelope frames from the stream
}
```

Used for:
- Local service mesh (TCP + raw framing, no SSH overhead)
- Secure mesh (TLS + raw framing)
- WebTransport direct call protocol (future: WebTransport + raw framing)

Auth for raw framing: `AuthToken` in frame header, resolved via
`IdentityProvider::resolve_from_token()`.

**Current implementation note**: `RawFramingInterface::accept()` returns an
error. Frame reading/writing is scheduled for Phase 2 implementation (see
integration-plan.md Phase 2.2).

### HttpInterface (MessageInterface)

Accepts standard HTTP requests and maps them to call protocol operations:

```
POST /v1/{namespace}/{op}     → registry.invoke(namespace, op, input)  (mutation)
GET  /v1/{namespace}/{op}     → registry.invoke(namespace, op, input)  (query)
GET  /v1/{namespace}/{op} SSE → registry.subscribe(namespace, op, input) (subscription)
GET  /v1/schema               → registry.list_operations()
```

Auth: `Authorization: Bearer <token>` header, resolved via
`IdentityProvider::resolve_from_token()`. Both AuthTokens and API keys are
accepted.

The HTTP interface runs inside the existing stealth mode byte-peek architecture:
after a TLS handshake, the server peeks at the first bytes. If they're
`SSH-2.0-`, the stream goes to `SshInterface`. Otherwise, the stream goes to
the axum HTTP router.

**Phase 2 scope**: Auth middleware, stealth handoff, and default 404 handler
only. Specific operation routes and path conventions are Phase 5+. The
`ListenerConfig::Http` variant spawns an axum router that reaches auth context;
routing inside axum is a later concern.

### DnsInterface (MessageInterface)

A DNS server that encodes/decodes `EventEnvelope` frames as DNS query/response
pairs. AuthToken is embedded in DNS query labels. Resolution via
`IdentityProvider::resolve_from_token()`.

This is a `MessageInterface` — it manages its own transport (UDP/TCP port 53)
and handles individual DNS queries as request/response pairs. DNS is NOT a
transport.

**Phase**: DNS interface implementation is Phase 5+. The `ListenerConfig::Dns`
variant and `DnsInterface` stub are defined now; implementation is deferred.

### Stream-Based Interface Pairs

| Transport | StreamInterface | Credential Presentation | Use case |
|-----------|---------------|------------------------|----------|
| TLS | SshInterface | SSH key handshake | Standard alknet tunnel |
| TCP | SshInterface | SSH key handshake | Plain SSH tunnel |
| iroh | SshInterface | SSH key handshake | P2P SSH tunnel |
| TCP | RawFramingInterface | AuthToken in frame header | Local service mesh |
| TLS | RawFramingInterface | AuthToken in frame header | Secure mesh |
| WebTransport | RawFramingInterface | AuthToken in CONNECT request | Browser call protocol (future) |

### Message-Based Interface Pairs

| MessageInterface | Credential Presentation | Owns transport? | Use case |
|-----------------|------------------------|----------------|----------|
| HttpInterface | `Authorization: Bearer` header | Yes (axum) | REST API, dashboard, integrations |
| DnsInterface | AuthToken in query labels | Yes (DNS server) | Censorship-resistant control channel |
| WebSocketInterface | AuthToken in handshake | Yes (WS server) | Browser persistent connection (future) |

Message-based interfaces manage their own transport. They don't need a
`Transport` from Layer 1 — they ARE the transport+interface combined.

### ListenerConfig

The server's accept loop configuration covers both stream and message interfaces:

```rust
pub enum ListenerConfig {
    Stream {
        transport: TransportKind,
        interface: StreamInterfaceKind,
    },
    Http {
        bind_addr: SocketAddr,
        tls: bool,
        stealth: bool,    // byte-peek protocol detection on shared port
    },
    Dns {
        bind_addr: SocketAddr,
        tls: bool,
    },
}

pub enum StreamInterfaceKind {
    Ssh,
    RawFraming,
}

pub enum TransportKind {
    Tcp,
    Tls { server_name: Option<String> },
    Iroh { endpoint_id: String },
    WebTransport,  // Phase 5+: tag only, no acceptor yet
}
```

Note: `TransportKind::Dns` does NOT exist. DNS is a `MessageInterface`, not a
transport. The `ListenerConfig::Dns` variant handles DNS listener configuration
directly.

### Credential Presentation Across Interfaces

Every interface resolves to the same `Identity` through `IdentityProvider`:

```
SSH fingerprint       → IdentityProvider::resolve_from_fingerprint → Identity
AuthToken (Bearer)   → IdentityProvider::resolve_from_token       → Identity
API key (Bearer)     → IdentityProvider::resolve_from_token       → Identity
DNS embedded token    → IdentityProvider::resolve_from_token       → Identity
```

The credential presentation differs per (Transport, Interface) pair, but the
resolution result is always an `Identity`. See [definitions.md](definitions.md)
for the full table and terminology rules.

### Server Accept Loop

With both stream and message interfaces, the accept loop becomes:

```rust
for listener in listeners {
    match listener {
        ListenerConfig::Stream { transport, interface } => {
            // Spawn accept loop: transport.accept() → interface.accept(stream)
        }
        ListenerConfig::Http { bind_addr, tls, stealth } => {
            // Spawn axum HTTP server on bind_addr
            // If stealth: byte-peek after TLS, route SSH vs HTTP
        }
        ListenerConfig::Dns { bind_addr, tls } => {
            // Spawn DNS server on bind_addr
        }
    }
}
```

## Constraints

- `StreamInterface` and `MessageInterface` are independent traits with different
  signatures, lifecycles, and transport ownership. No common super-trait (ADR-035).
- `SshInterface` is the most invasive refactoring. The existing `SshHandler`
  owns auth, channel management, and proxy logic — extracting these cleanly
  requires careful design (integration-plan Phase 1.8, completed in Phase 1).
- DNS interface implementation is Phase 5 work. `DnsInterface` is defined as a
  `MessageInterface` stub; implementation is deferred.
- HTTP interface Phase 2 scope is limited to auth middleware and stealth handoff.
  Specific operation routes are Phase 5+.
- WebTransport is Phase 5 work. `TransportKind::WebTransport` and
  `StreamInterfaceKind::WebTransport` are tags only for now.
- `TransportKind::Dns` does not exist. DNS is a `MessageInterface`, not a
  transport. This was `TransportKind` enum pollution from an earlier design.
- The `Interface` trait (singular) in the current codebase needs to be renamed
  to `StreamInterface`. This is a rename, not a semantic change.

## Open Questions

- **OQ-IF-02**: ~~Should `SshInterface` own the `ForwardingPolicy` check for
  `channel_open_direct_tcpip`, or should that move to Layer 3?~~ **Resolved**:
  ForwardingPolicy is Layer 3, but channel open/close lifecycle is Layer 2.
  SshInterface reports channel requests to Layer 3; Layer 3 applies policy.

- **OQ-P2-01**: Should `MessageInterface` and `StreamInterface` share a common
  trait? **Recommendation**: No. Independent traits with different signatures,
  lifecycles, and transport ownership. A common super-trait adds complexity
  without clear benefit. (See ADR-035.)

- **OQ-P2-02**: Should the HTTP interface share a port with the SSH listener?
  **Recommendation**: Start with separate ports. ALPN multiplexing on port 443
  is a future optimization that doesn't change the interface abstraction.
  Stealth mode byte-peek already handles shared-port detection for the common
  case.

## Design Decisions

| ADR | Decision | Summary |
|-----|----------|---------|
| [026](decisions/026-transport-interface-separation.md) | Three-layer model | SSH is Layer 2, not Layer 1 |
| [035](decisions/035-streaminterface-messageinterface-split.md) | StreamInterface / MessageInterface | Two trait categories at Layer 2 |
| [033](decisions/033-operationenv-irpc-call-protocol.md) | OperationEnv | Protocol is interface-agnostic |
| [029](decisions/029-identity-core-type.md) | Identity as core type | Auth resolution across interfaces |
| [031](decisions/031-forwarding-policy.md) | Forwarding policy | Layer 3 policy applied to Layer 2 channel requests |

## Phase 2 Implementation Notes

- `Interface` trait renamed to `StreamInterface` throughout alknet-core (ADR-035 implemented)
- `MessageInterface` trait added with `handle_request(InterfaceRequest) -> Result<InterfaceResponse>` (ADR-035 implemented)
- `InterfaceRequest` and `InterfaceResponse` types implemented
- `HttpInterface` and `DnsInterface` stub structs added (Phase 5 for full implementation)
- `InterfaceConfig` split into `StreamInterfaceConfig` and `MessageInterfaceConfig`
- `StreamInterfaceKind` and `MessageInterfaceKind` enums added
- `ListenerConfig` restructured from flat struct to enum with `Stream`, `Http`, `Dns` variants
- `TransportKind::Dns` removed from the enum (DNS is a MessageInterface, not a transport)
- `TransportKind::WebTransport` updated from `{ host: String }` to `{ server_name: Option<String> }`
- `RawFramingInterface` fully implemented with first-frame auth
- `SshSession::recv()`/`send()` bridge to call protocol via `alknet-control:0` channel implemented, using `ControlChannelBridge` with mpsc channels

## References

- [definitions.md](definitions.md) — Terminology disambiguation, credential presentation
- [research/phase2/interface-model.md](../research/phase2/interface-model.md) — Full StreamInterface/MessageInterface analysis
- [research/phase2/tls-transport.md](../research/phase2/tls-transport.md) — HTTP interface, stealth handoff, ListenerConfig
- [research/integration-plan.md](../research/integration-plan.md) — Phase 1.8, Phase 2.1-2.7
- [transport.md](transport.md) — Transport trait (unchanged at Layer 1)
- [auth.md](auth.md) — Credential presentation per (Transport, Interface) pair
- [identity.md](identity.md) — IdentityProvider, auth across interfaces