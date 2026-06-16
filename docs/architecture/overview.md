---
status: draft
last_updated: 2026-06-16
---

# Alknet Overview

## What Alknet Is

Alknet is a self-hostable networking toolkit built on QUIC+TLS with ALPN-based protocol dispatch. A single endpoint accepts connections on one port, and the ALPN string negotiated during the TLS handshake routes each connection to the correct protocol handler. Every service — SSH, SFTP, Git, HTTP, DNS, messaging, RPC — is an ALPN on a shared endpoint.

This is the core insight: **a service IS an ALPN.** One endpoint, one port, many protocols — dispatched by the TLS handshake, not by application-level peeking or separate listeners.

## Why ALPN Dispatch

The previous architecture used a three-layer model (StreamInterface/MessageInterface, ListenerConfig, OperationEnv) that required separate listener types, application-level protocol detection via byte-peeking, and complex dispatch paths. ALPN negotiation eliminates all of this:

- Protocol detection happens at the TLS layer — no byte-peeking
- A single endpoint replaces multiple listener types
- Adding a protocol is registering an ALPN string
- Each handler owns its entire wire format

See [ADR-001](decisions/001-alpn-protocol-dispatch.md) for the full rationale.

## Crate Graph

```
alknet-vault (standalone, no alknet-core dependency)
│
alknet-core
│   ├── ProtocolHandler trait
│   ├── ALPN router / endpoint
│   ├── BiStream trait, Connection type
│   ├── AuthContext, IdentityProvider
│   └── StaticConfig, DynamicConfig (ArcSwap)
│
├── alknet-ssh        (depends on alknet-core, russh)
├── alknet-call       (depends on alknet-core, irpc)
├── alknet-git        (depends on alknet-core, gix)
├── alknet-sftp       (depends on alknet-core, russh-sftp)
├── alknet-msg        (depends on alknet-core)
├── alknet-http       (depends on alknet-core, axum)
├── alknet-dns        (depends on alknet-core, hickory-proto)
│
├── alknet-napi       (depends on alknet-call, napi-rs)
│
└── alknet            (CLI binary, depends on all handler crates + alknet-vault)
```

Dependency rules:
- No handler crate depends on another handler crate
- All handler crates depend on alknet-core
- alknet-vault has zero alknet crate dependencies
- alknet-napi depends only on alknet-call (call protocol client)
- alknet (CLI) is the only crate that depends on all handler crates and alknet-vault

See [ADR-003](decisions/003-crate-decomposition.md) for the full decomposition rationale.

## ProtocolHandler Trait

The central abstraction. Every handler implements one trait:

```rust
#[async_trait]
pub trait ProtocolHandler: Send + Sync + 'static {
    fn alpn(&self) -> &'static [u8];
    async fn handle(&self, connection: Connection, auth: &AuthContext) -> Result<(), HandlerError>;
}
```

- `alpn()` returns the handler's ALPN identifier (e.g., `b"alknet/ssh"`, `b"alknet/call"`)
- `handle()` receives a `Connection` (not a single stream) and an `AuthContext` (which may be partial — see authentication section), returning `HandlerError` on failure
- Handlers that need a single stream call `connection.accept_bi()` once; handlers that multiplex (SSH, call) open/accept streams as needed
- Each handler manages its own wire format

This differs from the original ADR-002 signature which passed `BiStream`. See ADR-007 for the rationale: handlers like SSH and call need connection-level ownership to manage multiple streams.

See [ADR-002](decisions/002-protocol-handler-trait.md) and [ADR-007](decisions/007-bistream-type-definition.md) for the full rationale.

## ALPN Registry

| ALPN | Handler | Description |
|------|---------|-------------|
| `alknet/ssh` | SshAdapter | SSH-2 handshake, channel multiplexing, SOCKS5, port forwarding |
| `alknet/call` | CallAdapter | JSON-RPC via irpc: operations, streaming, pub/sub |
| `alknet/git` | GitAdapter | Git smart protocol over QUIC (gix, pkt-line) |
| `alknet/sftp` | SftpAdapter | SFTP protocol (russh-sftp core) |
| `alknet/msg` | MessageAdapter | E2E encrypted messaging, mixnet |
| `alknet/http` | HttpAdapter | axum REST API, dashboard, MCP endpoint |
| `alknet/dns` | DnsAdapter | DNS over QUIC/TLS, pkarr service discovery |
| `h3` | HttpAdapter (WebTransport upgrade) | Browser-compatible WebTransport, then ALPN upgrade |
| `h2` / `http/1.1` | HttpAdapter | Standard HTTP for browsers, curl |

> **Note**: `alknet/vault` is not in the ALPN registry. alknet-vault is a standalone local key vault with no alknet-core dependency. The CLI binary embeds it and exposes its operations through `alknet/call`. The vault is a capability source — derived keys and decrypted credentials are injected into operation contexts at the assembly layer, not passed as vault references to handlers. See ADR-008 for the integration rationale.

## Authentication

All handlers resolve identity through the shared `IdentityProvider` in alknet-core:

```rust
pub trait IdentityProvider: Send + Sync + 'static {
    fn resolve_from_fingerprint(&self, fingerprint: &str) -> Option<Identity>;
    fn resolve_from_token(&self, token: &AuthToken) -> Option<Identity>;
}
```

Each handler extracts credentials differently (SSH key fingerprint, AuthToken, Bearer header) but resolves through the same provider. Auth resolution is **hybrid**: the endpoint resolves what it can (e.g., TLS client certificate → fingerprint), and the handler resolves what it must (e.g., AuthToken in the first call frame). The `AuthContext` passed to `handle()` may be partial — handlers complete authentication inside `handle()`.

See [ADR-004](decisions/004-auth-as-shared-core.md) for the full rationale.

## Call Protocol

alknet-call uses irpc as its foundation. The wire format is length-prefixed JSON (EventEnvelope framing). Operations are registered in an irpc registry with JSON Schema discovery. The call protocol supports request/response, streaming subscriptions, and pub/sub.

The call protocol's TypeScript predecessor can import OpenAPI schemas and expose operations as HTTP endpoints or MCP tools. This bidirectional capability carries forward.

See [ADR-005](decisions/005-irpc-as-call-protocol-foundation.md) for the full rationale.

## WASM Compatibility

WASM is not an immediate implementation target, but it is a **design constraint on one-way doors** (see ADR-009). Decisions that would permanently prevent WASM targets from participating as peers require explicit justification.

This means:
- Core types (BiStream, Connection, ProtocolHandler, AuthContext) must not assume tokio or quinn
- Protocol parsers that are pure data transformations remain transport-agnostic
- The cost of keeping the WASM door open is low (trait vs concrete type, abstracted I/O) and the cost of closing it is high

Handlers with transport-agnostic cores are particularly WASM-friendly:
- russh-sftp's protocol core is already transport-agnostic
- hickory-proto is `#![no_std]` with `wasm-bindgen` feature
- The call protocol's JSON framing is inherently cross-language
- Git's pkt-line is simple enough to implement anywhere

A browser gets a WebTransport stream and speaks SFTP, Git, or call protocol directly — but only if we haven't closed that door with concrete type choices.

## Shared Types

The following types live in alknet-core and are used across handler crates:

| Type | Purpose |
|------|---------|
| `ProtocolHandler` | The trait every handler implements |
| `Connection` | QUIC connection (or mock) — handlers open/accept streams on it |
| `BiStream` | Trait: `AsyncRead + AsyncWrite + Send + Unpin` — bidirectional byte stream |
| `AuthContext` | Resolved identity for a connection (may be partial) |
| `Identity` | Authenticated peer identity |
| `IdentityProvider` | Trait for resolving credentials to identity |
| `AuthToken` | Opaque authentication token |
| `StaticConfig` | Immutable configuration loaded at startup |
| `DynamicConfig` | Hot-reloadable configuration (`ArcSwap`) |
| `ConfigReloadHandle` | Handle for triggering config reloads |

## Design Principles

### One-Way and Two-Way Doors

Not all decisions carry the same reversal cost. One-way door decisions (BiStream type, crate independence) require ADRs and possibly POCs before commitment. Two-way door decisions (static vs dynamic registration, single vs multi-transport) can be decided during implementation — start simple, add complexity when needed. See [ADR-009](decisions/009-one-way-door-decision-framework.md).

### WASM Door Preservation

WASM compatibility is not an active deliverable, but it is a design constraint. Decisions that would permanently close the WASM door (e.g., concrete quinn types in public APIs) require explicit justification. The cost of keeping the door open is low; the cost of closing it is irreversibly high.

### One ALPN, One Connection, One Handler

Each ALPN gets its own QUIC connection. The handler owns the entire connection lifecycle. Handlers that need multiple streams (SSH, call) call `connection.accept_bi()` or `connection.open_bi()` as needed. There is no multiplexing layer between connections.

### Handler Independence

No handler crate depends on another handler crate. Cross-handler communication goes through the call protocol (`alknet/call`) or through alknet-core's endpoint. The only crate that depends on all handlers is the CLI binary.

## Design Decisions

All design decisions are documented as ADRs in [decisions/](decisions/).

| ADR | Decision | Summary |
|-----|----------|---------|
| [001](decisions/001-alpn-protocol-dispatch.md) | ALPN-Based Protocol Dispatch | Single endpoint, ALPN negotiation routes to handlers |
| [002](decisions/002-protocol-handler-trait.md) | ProtocolHandler Trait | One trait replaces StreamInterface/MessageInterface |
| [003](decisions/003-crate-decomposition.md) | Crate Decomposition | One crate per protocol handler, core provides shared infra |
| [004](decisions/004-auth-as-shared-core.md) | Auth as Shared Core | IdentityProvider in core, handlers extract credentials |
| [005](decisions/005-irpc-as-call-protocol-foundation.md) | irpc as Call Protocol Foundation | Call protocol uses irpc for registry, framing, dispatch |
| [006](decisions/006-alpn-convention-and-connection-model.md) | ALPN String Convention and Connection Model | `alknet/` prefix, one ALPN per connection |
| [007](decisions/007-bistream-type-definition.md) | BiStream Type Definition | BiStream is a trait, handlers receive Connection not BiStream |
| [008](decisions/008-secret-service-integration.md) | Vault Integration Point | CLI-embedded, exposed via call protocol, vault is a capability source |
| [009](decisions/009-one-way-door-decision-framework.md) | One-Way Door Decision Framework | Classify decisions by reversal cost; one-way doors need ADRs |

## Open Questions

Open questions are tracked in [open-questions.md](open-questions.md). Key questions affecting this document:

- **OQ-01**: BiStream type definition (resolved: trait, Connection parameter — see ADR-007)
- **OQ-02**: AuthContext resolution timing (resolved: hybrid — see ADR-004)
- **OQ-03**: ALPN string naming convention (resolved: see ADR-006)
- **OQ-04**: Dynamic handler registration at runtime vs static at startup (two-way door, defer to implementation)
- **OQ-08**: Vault integration point (resolved: CLI-embedded via call protocol — see ADR-008)

## Failure Modes

| Failure | Behavior |
|---------|----------|
| ALPN negotiation fails (no intersection) | TLS handshake fails — correct behavior, the client and server have no protocol in common |
| Handler `handle()` returns `HandlerError` | Endpoint logs the error, closes the QUIC stream. Other streams on the same connection are unaffected |
| Handler panics | The handler's task is caught by tokio's panic handling. The stream is closed. Other streams and connections are unaffected |
| `IdentityProvider` returns `None` | AuthContext is partial. If the handler requires authentication and cannot extract credentials from the stream, it closes the stream with an auth error |
| Config reload fails | `ArcSwap<DynamicConfig>` keeps the previous valid config. Error is logged. No service interruption |
| BiStream read/write error | QUIC stream-level error. The handler detects this as an I/O error and returns from `handle()`. The connection itself may remain open for other streams |

## What Stays from the Previous Implementation

The reference implementation at `/workspace/@alkdev/alknet-main/` contains working code that carries forward, adapted to the new model:

| Module | Lines | Destination | Notes |
|--------|-------|-------------|-------|
| `src/auth/*` | ~1450 | alknet-core | Identity, IdentityProvider, keys — simplified per ADR-004 |
| `src/config/*` | ~950 | alknet-core | StaticConfig, DynamicConfig, ArcSwap — adapted for ALPN handler config |
| `src/transport/*` | ~1500 | alknet-core | Transport trait, TCP/TLS/iroh — becomes endpoint connection acceptors |
| `src/call/*` | ~1200 | alknet-call | EventEnvelope, registry, framing — becomes ProtocolHandler on alknet/call |
| `src/interface/ssh.rs` | 982 | alknet-ssh | SSH channel handling |
| `src/server/handler.rs` | 974 | alknet-ssh | SSH server handler |
| `src/server/channel_proxy.rs` | 555 | alknet-ssh | Channel proxy |
| `src/server/serve.rs` | 1526 | alknet-core (reference) | Accept loop pattern informs ALPN router, but gets rewritten |
| `src/client/*` | ~1900 | alknet-ssh | SOCKS5 client, connect logic |
| `src/socks5/*` | ~800 | alknet-ssh | SOCKS5 protocol |

The old code is reference, not constraint. Understand what it did and why, then implement against the new ProtocolHandler trait and ALPN router.