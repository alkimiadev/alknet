# Interface Model: Stream and Message Interfaces

> Status: Research / Draft
> Last updated: 2026-06-08
> Part of: Phase 2 planning

## Overview

The current three-layer model (ADR-026, [interface.md](../../architecture/interface.md)) defines Transport (Layer 1), Interface (Layer 2), and Protocol (Layer 3). The `Interface` trait assumes a persistent byte stream from a `Transport`, which works for SSH and raw framing. However, two important interface types — HTTP and DNS — don't fit this model: they handle individual requests, not persistent sessions. This document proposes splitting the interface model into `StreamInterface` and `MessageInterface`, adding HTTP as a first-class interface, and reclassifying DNS from a transport to a message-based interface.

## Problem Statement

### DNS is not a transport

The current `TransportKind` enum includes `Dns { domain: String }` alongside `Tcp`, `Tls`, and `Iroh`. But DNS doesn't produce a `AsyncRead + AsyncWrite + Unpin + Send` byte stream. It's a request/response protocol. Listing it as a transport conflates different abstractions. DNS encodes/decodes `EventEnvelope` frames as DNS query/response pairs — that's an interface behavior, not a transport behavior.

### HTTP is missing as an interface

The current valid (Transport, Interface) pairs are all stream-based:

| Transport | Interface |
|---|---|
| TLS | SSH |
| TCP | SSH |
| iroh | SSH |
| DNS | raw framing |
| WebTransport | SSH |
| WebTransport | raw framing |
| TCP | raw framing |

But there's no HTTP interface — the (TCP/TLS, HTTP) pair that accepts standard HTTP requests and maps them to call protocol operations. This is the **server-side** equivalent of `OpenAPIServiceRegistry` (which does client-side: consuming OpenAPI specs to make outbound HTTP calls). Without it, external clients (browsers, curl, monitoring) can only reach alknet through SSH.

### Auth across all interfaces

Different interfaces authenticate differently, but all resolve to the same `Identity` through `IdentityProvider`:

| (Transport, Interface) | Auth mechanism | Resolves via |
|---|---|---|
| (TLS, SSH) | SSH public key handshake | `IdentityProvider::resolve_from_fingerprint()` |
| (TCP, SSH) | SSH public key handshake | `IdentityProvider::resolve_from_fingerprint()` |
| (iroh, SSH) | SSH public key handshake | `IdentityProvider::resolve_from_fingerprint()` |
| (TLS, raw framing) | Token in frame header | `IdentityProvider::resolve_from_token()` |
| (TCP, raw framing) | Token in frame header | `IdentityProvider::resolve_from_token()` |
| (WebTransport, raw framing) | Token in CONNECT request | `IdentityProvider::resolve_from_token()` |
| (TLS, HTTP) | HTTP Authorization header | `IdentityProvider::resolve_from_token()` |
| (—, DNS) | Token embedded in DNS query | `IdentityProvider::resolve_from_token()` |

All token-based paths use the same `AuthToken` format (Ed25519-signed timestamp, defined in [auth.md](../../architecture/auth.md)). The `IdentityProvider` trait doesn't change — `resolve_from_token()` already covers all of these. The difference is just how the token gets extracted from the wire format.

## Design

### StreamInterface and MessageInterface

The current `Interface` trait has this signature:

```rust
#[async_trait]
pub trait Interface: Send + Sync + 'static {
    type Session;
    async fn accept(stream: TransportStream, config: &InterfaceConfig) -> Result<Self::Session>;
}
```

This works for SSH and raw framing — both run over a duplex stream. But HTTP and DNS are **message-based**: they receive isolated requests, not persistent sessions. The interface model needs to accommodate both patterns.

**Rename `Interface` to `StreamInterface`** for stream-based connections:

```rust
#[async_trait]
pub trait StreamInterface: Send + Sync + 'static {
    type Session;
    async fn accept(stream: TransportStream, config: &InterfaceConfig) -> Result<Self::Session>;
}
```

**Add `MessageInterface`** for message-based request/response interfaces:

```rust
#[async_trait]
pub trait MessageInterface: Send + Sync + 'static {
    async fn handle_request(&self, request: InterfaceRequest) -> Result<InterfaceResponse>;
}
```

Why separate traits instead of one:
- Different signatures: `StreamInterface` produces a session from a stream. `MessageInterface` handles an individual request.
- Different lifecycles: Stream sessions are long-lived (SSH channels persist). Message handlers are stateless per-request (each HTTP request is independent).
- Different transport ownership: `StreamInterface` receives a `TransportStream` from elsewhere. `MessageInterface` manages its own transport (HTTP server, DNS server).

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

This is a normalized interface-agnostic request/response. The `MessageInterface` implementation extracts the operation path, input, and auth token from its wire format (HTTP, DNS, etc.) and constructs an `InterfaceRequest`. The call protocol handler processes it and returns an `InterfaceResponse` that the implementation serializes back to its wire format.

### HTTP Interface

The HTTP interface accepts standard HTTP requests and maps them to call protocol operations:

```
POST /v1/{namespace}/{op}     → registry.invoke(namespace, op, input)  (mutation)
GET  /v1/{namespace}/{op}     → registry.invoke(namespace, op, input)  (query, params as input)
GET  /v1/{namespace}/{op} SSE → registry.subscribe(namespace, op, input) (subscription)
```

This is how external clients invoke alknet operations without SSH. Use cases:
- Dashboard UI calling operations via fetch()
- Third-party service integration via REST API
- Health checks and monitoring endpoints
- Other alknet nodes using `OpenAPIServiceRegistry` to register against this API

```rust
pub struct HttpInterface {
    identity_provider: Arc<dyn IdentityProvider>,
    registry: Arc<OperationRegistry>,
    env: OperationEnv,
}
```

Auth: Extract `Authorization: Bearer <token>` header, pass to `IdentityProvider::resolve_from_token()`. The token is the same `AuthToken` format used by WebTransport and raw framing.

The HTTP interface manages its own transport layer (hyper/axum/actix). It doesn't need a `Transport` from Layer 1 — HTTP IS the transport. This is the same pattern as the DNS interface.

### DNS Interface

DNS is not a transport. It's a **message-based interface** that encodes `EventEnvelope` frames as DNS query/response pairs:

```
DNS query: "_alknet.request.{base64url(payload)}.alk.dev TXT?"
   → decoded as EventEnvelope (call.requested)
   → call protocol handler processes it
   → encoded as EventEnvelope (call.responded)
   → returned as DNS TXT record response
```

```rust
pub struct DnsInterface {
    domain: String,
    identity_provider: Arc<dyn IdentityProvider>,
    registry: Arc<OperationRegistry>,
    env: OperationEnv,
}
```

Auth: Token embedded in the DNS query. Same `AuthToken` format.

The DNS interface runs its own DNS server. It doesn't need a separate `Transport` — DNS is both the transport and the interface combined.

### Remove TransportKind::Dns

Since DNS is a `MessageInterface` (not a transport), `TransportKind::Dns` should be removed from the enum. The `ListenerConfig` enum should be updated to cover both stream and message listeners:

```rust
pub enum ListenerConfig {
    Stream {
        transport: TransportKind,
        interface: StreamInterfaceKind,
    },
    Message {
        interface: MessageInterfaceKind,
        bind_addr: SocketAddr,
    },
}
```

This cleanly separates "listen for byte streams" from "listen for messages."

### Revised Interface Pairs

**Stream-based connections** (persistent session, `StreamInterface`):

| Transport | StreamInterface | Auth | Use case |
|---|---|---|---|
| TLS | SshInterface | SSH pubkey handshake | Standard alknet tunnel |
| TCP | SshInterface | SSH pubkey handshake | Plain SSH tunnel |
| iroh | SshInterface | SSH pubkey handshake | P2P SSH tunnel |
| TCP | RawFramingInterface | Token in frame header | Local service mesh |
| TLS | RawFramingInterface | Token in frame header | Secure mesh |
| WebTransport | SshInterface | SSH pubkey handshake | Browser SSH tunnel (future) |
| WebTransport | RawFramingInterface | Token in CONNECT request | Browser call protocol (future) |

**Message-based interfaces** (stateless per-request, `MessageInterface`):

| MessageInterface | Auth | Owns transport? | Use case |
|---|---|---|---|
| HttpInterface | Authorization header (Bearer token) | Yes (hyper/axum) | REST API, dashboard, integrations |
| DnsInterface | Token embedded in query labels | Yes (DNS server) | Censorship-resistant control channel |
| WebSocketInterface | Token in handshake or first message | Yes (WS server) | Browser persistent connection (future) |

The `MessageInterface` implementations manage their own transport. They don't need the `Transport` trait because they're not wrapping a generic byte stream — they ARE the transport+interface combined.

### Unified auth across all interfaces

Every interface resolves to the same `Identity` through `IdentityProvider`:

```
SSH fingerprint      → IdentityProvider::resolve_from_fingerprint → Identity
Bearer token          → IdentityProvider::resolve_from_token       → Identity
HTTP Authorization    → IdentityProvider::resolve_from_token       → Identity
DNS embedded token    → IdentityProvider::resolve_from_token       → Identity
WebSocket token       → IdentityProvider::resolve_from_token       → Identity
```

The token format is the same `AuthToken = base64url(key_id || timestamp || signature)` defined in [auth.md](../../architecture/auth.md). The interface just extracts the credential from its wire format. `IdentityProvider` resolves it to an `Identity`. The call protocol handler receives `OperationContext` with that identity.

In database-backed deployments (`StorageIdentityProvider`), `Identity.id` is the account UUID — so the same person connecting via SSH, HTTP, or DNS resolves to the same identity. No separate `account_id` field needed.

### ConfigIdentityProvider: Token auth without a database

The config-based (minimal) deployment gains API key / bearer token support through `DynamicConfig.auth`:

```toml
[auth.ssh]
authorized_keys = [...]

[auth.token]
enabled = true
max_token_age = "5m"
# key_source = "shared"  (default: same keys as SSH)

[[auth.api_keys]]
prefix = "alk_"
hash = "sha256:xyz..."
scopes = ["relay:connect"]
description = "dashboard service account"
```

`ConfigIdentityProvider::resolve_from_token()` already exists in the current spec. It verifies the `AuthToken` format (Ed25519 signed timestamp) against the same `authorized_keys` set used for SSH. The `api_keys` section adds an alternative: simple bearer tokens (hash-verified, with optional TTL) that don't require Ed25519 key pairs. This is useful for service accounts and automation.

Both token types produce the same `Identity`. Config-based `Identity.id` is the key fingerprint (for `AuthToken`) or the key prefix (for simple bearer tokens). In database-backed deployments, both resolve to the account UUID.

## Service Decomposition

### AuthService (existing — ADR-028)

Resolves **inbound** credentials to an `Identity`. Already defined. Works across all interfaces — SSH interface calls `resolve_from_fingerprint()`, HTTP/DNS interfaces call `resolve_from_token()`. No changes needed.

### CredentialService (new — see credential-provider.md)

Resolves **outbound** credentials for external service access. Defined in [credential-provider.md](credential-provider.md).

### AccountService (new — storage layer)

Manages accounts and credential associations. This is a storage-layer irpc service, not a core concern:

- `AccountProtocol::CreateAccount { display_name, default_scopes }`
- `AccountProtocol::GetAccount { account_id }`
- `AccountProtocol::AddCredential { account_id, credential }` (SSH key, API key)
- `AccountProtocol::RemoveCredential { account_id, credential_id }`
- `AccountProtocol::ListCredentials { account_id }`

This is the CRUD layer. `StorageIdentityProvider` uses it internally. External management (admin UI) goes through `AccountService`. Analogous to how `ConfigService` provides `ConfigReloadHandle` — core has the read trait, storage has the management service.

Core doesn't need `AccountService` for operation. `IdentityProvider` is the read-only contract. Account management is additive.

## Impact on Existing Specs

### interface.md

Needs revision:

1. **Rename `Interface` to `StreamInterface`** — the current trait becomes the stream-specific variant.
2. **Add `MessageInterface` trait** — for HTTP, DNS, WebSocket.
3. **Add `HttpInterface`** as a `MessageInterface` implementation.
4. **Clarify DNS** — DNS is a `MessageInterface`, not a (DNS transport, raw framing) pair. Remove `TransportKind::Dns` from the transport enum.
5. **Add valid message-based interface pairs** table alongside the stream-based pairs table.
6. **Add `InterfaceRequest` / `InterfaceResponse`** types that normalize calls across message interfaces.

### auth.md

Needs revision:

1. **Add HTTP interface auth** — `Authorization: Bearer <token>` extraction.
2. **Add DNS interface auth** — token embedded in DNS query labels.
3. **Add auth presentation table** showing all interface/auth combos.
4. **Add simple API keys** — bearer tokens (hash-verified, with optional TTL) for service accounts. Not all token auth needs Ed25519 key pairs.

### transport.md

Minor: **Remove `TransportKind::Dns`** from the enum. Add note that DNS is handled as a `MessageInterface`.

### call-protocol.md

Minor update: the call protocol handler should accept `EventEnvelope` frames from both `StreamInterface::Session` and `MessageInterface::handle_request()`. The dispatch logic is the same — only the framing differs.

### ADR-026

Needs update: the three-layer model is correct, but the (Transport, Interface) pair enumeration in ADR-026 lists DNS as a transport. This should be revised to show `StreamInterface` and `MessageInterface` as two interface categories at Layer 2.

## Phasing Considerations

| Work | Suggested Phase | Notes |
|---|---|---|
| Rename `Interface` → `StreamInterface` | Phase 1 (now) | Rename only, no behavior change. Existing code already implements the stream pattern. |
| Define `MessageInterface` trait | Phase 1 (now) | Cheap, forward-compatible. Define the trait and `InterfaceRequest`/`InterfaceResponse` types. |
| Define `HttpInterface` stub | Phase 1 (now) | Define the struct and impl signature. Full HTTP server wiring can wait. |
| `TransportKind::Dns` removal | Phase 1 (now) | Clean up the enum before code depends on `TransportKind::Dns`. |
| `ListenerConfig` with Stream/Message variants | Phase 1 (now) | Update the server accept loop to support both interface types. |
| `HttpInterface` implementation | Phase 2 | Full HTTP server with router, auth middleware, SSE. Depends on core being stable. |
| `DnsInterface` implementation | Phase 3+ | DNS protocol is non-trivial. Deferring is fine. |
| `AccountService` irpc protocol | Phase 2 | CRUD for accounts. Lives in alknet-storage. |
| `ApiKeys` in `DynamicConfig.auth` | Phase 1 (now) | Enable bearer token auth in config-based deployments. |

The key observation: defining the traits (`MessageInterface`, `InterfaceRequest`, `HttpInterface` stub) now is cheap and prevents refactoring later. The actual HTTP server implementation can wait for Phase 2. But the trait surface needs to exist in Phase 1 so downstream code can target it.

## Open Questions

### OQ-IF-03: Should `MessageInterface` and `StreamInterface` share a common trait?

Recommendation: Independent traits. Different signatures (`handle_request` vs `accept` + `next_event/send_event`), different lifecycles (stateless vs session-stateful), different transport ownership (self-managed vs provided). A common super-trait adds complexity without clear benefit.

### OQ-IF-04: Should `TransportKind::Dns` be removed from the enum?

Recommendation: Yes. DNS doesn't produce byte streams. Remove it and add `ListenerConfig::Message` variant. This is a cleanup, not a breaking change — `TransportKind::Dns` is currently a tag with no acceptor implementation.

### OQ-IF-05: Should the HTTP interface share a port with the SSH listener?

In production, alknet might run SSH on port 22 and HTTP on port 443. Or both on 443 (TLS with ALPN). The `HttpInterface` could share a TLS listener with `SshInterface` if ALPN negotiation selects SSH vs. HTTP.

Recommendation: Start simple — separate ports. HTTP on its own port (default 8080 or configured via `[[listeners]]`). ALPN multiplexing is a future optimization that doesn't change the interface abstraction.

### OQ-IF-06: Should the HTTP interface auto-generate OpenAPI specs from the OperationRegistry?

If alknet exposes operations as `POST /v1/{namespace}/{op}`, the HTTP interface could auto-generate an OpenAPI spec from the registered `OperationSpec`s. This would provide:
- Interactive API documentation
- Automatic client SDK generation
- Compatibility with `OpenAPIServiceRegistry` (another alknet node's `FromOpenAPI` could register against this spec)

This is the reverse of `OpenAPIServiceRegistry` — instead of consuming an OpenAPI spec to register operations, it produces an OpenAPI spec from registered operations. The `OperationSpec` already has `input_schema`, `output_schema`, `description`, and `tags`.

Recommendation: Yes, but Phase 4+. The HTTP interface needs to exist first.

### OQ-IF-07: How do self-hosted services (rustfs, gitea) authenticate requests from alknet users?

When alknet sits in front of rustfs or gitea (e.g., as a reverse proxy or HTTP interface gateway), how does it map alknet identities to external service identities?

Options:
1. **Shared secret / API key**: Alknet holds a service-level credential. All proxied requests use it. Simple but loses per-user identity on the external service.
2. **Identity-bound credentials**: Each alknet account has a corresponding rustfs/gitea credential, looked up via `Identity.id`. Per-user ACL on the external service.
3. **Alknet as OIDC provider**: Rustfs/gitea trust alknet as their identity provider. No stored credentials — users authenticate directly via OIDC.

Recommendation: Start with Option 1. Add Option 2 when multi-tenant access is needed. Option 3 is the long-term goal (Phase D in [credential-provider.md](credential-provider.md)).

## References

- [interface.md](../../architecture/interface.md) — Current Interface layer spec (needs update for `StreamInterface`/`MessageInterface`)
- [auth.md](../../architecture/auth.md) — Unified auth, IdentityProvider, AuthToken format
- [identity.md](../../architecture/identity.md) — Identity struct, IdentityProvider trait
- [call-protocol.md](../../architecture/call-protocol.md) — Call protocol, OperationEnv
- [services.md](../../architecture/services.md) — irpc service definitions
- [credential-provider.md](credential-provider.md) — CredentialProvider, CredentialSet (Phase 2)
- [ADR-026](../../architecture/decisions/026-transport-interface-separation.md) — Three-layer model (needs update for `MessageInterface`)
- [ADR-023](../../architecture/decisions/023-unified-auth-shared-key-material.md) — Unified auth with shared key material
- [ADR-029](../../architecture/decisions/029-identity-core-type.md) — Identity as core type