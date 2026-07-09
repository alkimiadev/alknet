---
status: draft
last_updated: 2026-07-09
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
├── alknet-call       (depends on alknet-core)
│   ├── CallAdapter (server: ProtocolHandler for alknet/call)
│   ├── Call client (send/receive over QUIC)
│   ├── OperationSpec, OperationRegistry, AccessControl
│   └── Adapter traits (from_*, to_*)
│
├── alknet-agent      (depends on alknet-call)
│   ├── LLM execution loop (forked aisdk, simplified)
│   ├── Tool dispatch via call protocol
│   └── Provider credentials via capabilities (no env vars, no vault on the wire)
│
├── alknet-git        (depends on alknet-core, gix)
├── alknet-sftp       (depends on alknet-core, russh-sftp)
├── alknet-msg        (depends on alknet-core)
├── alknet-http       (depends on alknet-core, alknet-call, axum, reqwest, wtransport, rmcp)
├── alknet-dns        (depends on alknet-core, hickory-proto)
│
├── alknet-napi       (depends on alknet-call, napi-rs)
│   └── Thin NAPI projection of call protocol client to Node.js
│
└── alknet            (CLI binary, depends on all handler crates + alknet-vault)
```

Dependency rules:
- No handler crate depends on another handler crate
- All handler crates depend on alknet-core
- alknet-vault has zero alknet crate dependencies
- alknet-agent depends on alknet-call (not alknet-core) — it uses the call protocol client for tool dispatch
- alknet-napi depends only on alknet-call — thin NAPI projection, no business logic
- alknet (CLI) is the only crate that depends on all handler crates and alknet-vault
- Rust is the canonical implementation language — TypeScript is a reference/browser adaptation, not a parallel implementation (see ADR-013)

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
| `alknet/call` | CallAdapter | JSON-RPC via hand-rolled EventEnvelope framing: operations, streaming, pub/sub |
| `alknet/git` | GitAdapter | Git smart protocol over QUIC (gix, pkt-line) |
| `alknet/sftp` | SftpAdapter | SFTP protocol (russh-sftp core) |
| `alknet/msg` | MessageAdapter | E2E encrypted messaging, mixnet |
| `alknet/http` | HttpAdapter | axum REST API, dashboard, MCP endpoint |
| `alknet/dns` | DnsAdapter | DNS over QUIC/TLS, pkrr service discovery |
| `h3` | HttpAdapter (HTTP/3 + WebTransport) | Browser-compatible WebTransport + HTTP/3 (first-class, ADR-038) |
| `h2` / `http/1.1` | HttpAdapter | Standard HTTP for browsers, curl |

> **Note**: `alknet/agent` is not in the ALPN registry. The agent service is a future consumer that builds on top of `alknet-call` (it depends on `alknet-call`, not `alknet-core` directly — see ADR-003). It uses the call protocol for tool dispatch and exposes agent operations (e.g., `/agent/chat`) as call-protocol operations in the `OperationRegistry`, not as a separate ALPN. The agent is a mental model that informed the core architecture (capabilities, scoped env, abort cascade) but is not specced yet — its design will change as it's built out against the implemented core crates.

> **Note**: `alknet/vault` is not in the ALPN registry. alknet-vault is a standalone local key vault with no alknet-core dependency and no remote dispatch capability (ADR-025). The CLI binary embeds it and accesses it at the assembly layer — unlocking the vault at startup, deriving and decrypting credentials, and injecting them into handler capabilities. The vault is not exposed over the call protocol. No vault operations are registered in the operation registry. See ADR-008, ADR-014, and ADR-025.

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

## Security Model: Secret Material Flow

Authentication (above) handles inbound identity — who is calling me. Secret material flow handles outbound credentials — what secrets a handler uses for its own outbound calls (LLM provider API keys, HTTP service tokens, signing keys). These are orthogonal concerns with different sources and lifetimes:

| Axis | Question | Source | Lifetime |
|------|----------|--------|----------|
| Identity (inbound) | Who is the caller? | AuthContext, per-request (TLS cert, auth token) | Per-request |
| Capabilities (outbound) | What secrets can I use outbound? | Assembly layer, from vault, injected at construction | Handler lifetime |

The vault (alknet-vault) holds the master seed and derives keys and decrypts credentials. It is accessed **only at the assembly layer** — the CLI binary unlocks it at startup, derives/decrypts what each handler needs, and injects the results into handler capabilities. The vault is not exposed over the call protocol. No vault operations are registered in the operation registry. The master seed and derived private keys never cross the network.

This replaces the industry default of environment variables and plaintext config files for storing credentials. There is no `std::env::var("API_KEY")` path — the only way a handler gets a credential is through a capability, and the only way a capability is populated is through the assembly layer from the vault.

The call protocol carries no secret material — not in request payloads, not in response payloads, not in operation metadata. Operations that need to share public key material use a dedicated operation that returns only the public component.

See [ADR-008](decisions/008-secret-service-integration.md) and [ADR-014](decisions/014-secret-material-flow-and-capability-injection.md) for the full rationale.

## Call Protocol

alknet-call uses hand-rolled `EventEnvelope` framing (length-prefixed JSON). The wire format, operation registry, and dispatch are all hand-rolled in alknet-call — irpc was never integrated (ADR-064 supersedes ADR-005, which had accepted "irpc as the call protocol foundation" based on the previous architecture but was never implemented as stated). Operations are registered in a hand-rolled registry with JSON Schema discovery. The call protocol supports request/response, streaming subscriptions, and pub/sub.

The call protocol's adapter contract (from_openapi, from_mcp, from_call, to_openapi, to_mcp) enables bidirectional composition — operations can be imported from external sources and exported to external protocols. These adapter traits are defined in Rust in alknet-call. The existing TypeScript `@alkdev/operations` library informed the design and may be adapted for browser use (see ADR-013).

See [ADR-064](decisions/064-irpc-never-integrated-hand-rolled-framing.md) for the full rationale (supersedes [ADR-005](decisions/005-irpc-as-call-protocol-foundation.md)).

## WASM Compatibility

WASM is not an implementation target. It is a design constraint on one-way doors (see ADR-009): core types must not assume tokio or quinn, and protocol parsers that are pure data transformations remain transport-agnostic. The cost of keeping this door open is low (trait vs concrete type, abstracted I/O); the cost of closing it is irreversibly high. The browser path is through a JavaScript SDK adapted from the existing TypeScript `@alkdev/operations` library, speaking the EventEnvelope wire format over WebTransport streams — not through Rust-to-WASM compilation of the full stack (see ADR-013). Specific WASM targeting decisions are deferred to individual crate specs. See OQ-09.

## Shared Types

The following types live in alknet-core and are used across handler crates:

| Type | Purpose |
|------|---------|
| `ProtocolHandler` | The trait every handler implements |
| `Connection` | Transport connection (QUIC via quinn/iroh, or a generic single stream via `from_stream` — ADR-065) — handlers open/accept streams on it |
| `BiStream` | Trait: `AsyncRead + AsyncWrite + Send + Unpin` — bidirectional byte stream |
| `AuthContext` | Resolved identity for a connection (may be partial) |
| `Identity` | Authenticated peer identity (inbound) |
| `IdentityProvider` | Trait for resolving credentials to identity |
| `AuthToken` | Opaque authentication token |
| `Capabilities` | Outbound credentials injected by the assembly layer (non-serializable, zeroized, immutable after construction) — defined in [core-types.md](crates/core/core-types.md#capabilities) |
| `Visibility` | Operation visibility — External (wire-callable) or Internal (composition-only) |
| `StaticConfig` | Immutable configuration loaded at startup |
| `DynamicConfig` | Hot-reloadable configuration (`ArcSwap`) |
| `ConfigReloadHandle` | Handle for triggering config reloads |

## Design Principles

### One-Way and Two-Way Doors

Not all decisions carry the same reversal cost. One-way door decisions (BiStream type, crate independence, secret material flow) require ADRs and possibly POCs before commitment. Two-way door decisions (single vs multi-transport) can be decided during implementation — start simple, add complexity when needed. The static-vs-dynamic registration question is now resolved: the `HandlerRegistry` (ALPN-level) is static at startup (ADR-010, OQ-04), while the `OperationRegistry` (call-protocol-level) is layered — curated ops static, session/imported ops dynamic at their trust-boundary scopes (ADR-024). WASM compatibility is a design constraint within this framework, not a separate principle: decisions that would permanently close the WASM door require explicit justification. See [ADR-009](decisions/009-one-way-door-decision-framework.md).

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
| [005](decisions/005-irpc-as-call-protocol-foundation.md) | irpc as Call Protocol Foundation | ~~Accepted~~ → **Superseded** by [ADR-064](decisions/064-irpc-never-integrated-hand-rolled-framing.md) (irpc was never integrated) |
| [006](decisions/006-alpn-convention-and-connection-model.md) | ALPN String Convention and Connection Model | `alknet/` prefix, one ALPN per connection |
| [007](decisions/007-bistream-type-definition.md) | BiStream Type Definition | BiStream is a trait, handlers receive Connection not BiStream |
| [008](decisions/008-secret-service-integration.md) | Vault Integration Point | CLI-embedded, vault is a capability source accessed at assembly time |
| [009](decisions/009-one-way-door-decision-framework.md) | One-Way Door Decision Framework | Classify decisions by reversal cost; one-way doors need ADRs |
| [010](decisions/010-alpn-router-and-endpoint.md) | ALPN Router and Endpoint | HandlerRegistry, accept loop, static registration |
| [011](decisions/011-authcontext-structure.md) | AuthContext Structure and Resolution Flow | AuthContext fields, hybrid resolution |
| [012](decisions/012-call-protocol-stream-model.md) | Call Protocol Stream Model | Bidirectional streams, EventEnvelope, ID-based correlation |
| [013](decisions/013-rust-canonical-implementation.md) | Rust as Canonical Implementation Language | Rust canonical, TypeScript reference adaptation |
| [014](decisions/014-secret-material-flow-and-capability-injection.md) | Secret Material Flow and Capability Injection | Capabilities carry outbound credentials; call protocol carries no secret material |
| [015](decisions/015-privilege-model-and-authority-context.md) | Privilege Model and Authority Context | `internal` = authority switch not ACL skip; External/Internal visibility; handler identity + scoped env |
| [016](decisions/016-abort-cascade-for-nested-calls.md) | Abort Cascade for Nested Calls | `call.aborted` cascades to descendants; default `abort-dependents`, `continue-running` opt-in |
| [017](decisions/017-call-protocol-client-and-adapter-contract.md) | Call Protocol Client and Adapter Contract | `CallClient` opens connections; `from_call` imports remote ops; connection direction independent of call direction |
| [018](decisions/018-vault-standalone-crate.md) | Vault as Standalone Crate | Zero alknet crate dependencies; vault defines own types and errors |
| [019](decisions/019-vault-assembly-layer-only.md) | Vault Assembly-Layer-Only Access | The assembly layer (CLI binary) is the sole direct caller; handlers never hold a vault reference |
| [020](decisions/020-hd-derivation-for-encryption-keys.md) | HD Derivation for Encryption Keys | SLIP-0010 derivation from seed, not PBKDF2; salt field unused in v2 |
| [021](decisions/021-key-rotation-via-version-indexed-paths.md) | Key Rotation via Version-Indexed Paths | Version-indexed derivation paths; `rotate` re-encrypts between versions |
| [022](decisions/022-handler-registration-provenance-and-composition-authority.md) | Handler Registration, Provenance, and Composition Authority | Registration bundle carries provenance, composition authority, scoped env, capabilities; dispatch path reads from bundle |
| [023](decisions/023-operation-error-schemas.md) | Operation Error Schemas | Operations declare domain errors; `call.error` carries typed `details`; adapter fidelity for `from_openapi`/`to_openapi` |
| [024](decisions/024-operation-registry-layering.md) | Operation Registry Layering | Curated (static) + session/connection overlays (dynamic); `OperationEnv` as trait-object integration point |
| [025](decisions/025-vault-local-only-dispatch.md) | Vault Local-Only Dispatch | Dropped irpc from vault; direct method calls; local-only by construction |
| [026](decisions/026-vault-key-model-hd-derivation.md) | Vault Key Model — HD Derivation | HD derivation from BIP39 seed; `74'` coin type; SLIP-0010/Ed25519 default; AES-256-GCM for credentials |
| [027](decisions/027-tls-identity-redesign-acme-rawkey-decoupling.md) | TLS Identity Redesign — ACME + RawKey Decoupling | `TlsIdentity::Acme` variant + two-phase server config; `RawKey` uses `ed25519-dalek` (not `iroh::SecretKey`); `acme` feature gate |

## Open Questions

Open questions are tracked in [open-questions.md](open-questions.md). Key questions affecting this document:

- **OQ-01**: BiStream type definition (resolved: trait, Connection parameter — see ADR-007)
- **OQ-02**: AuthContext resolution timing (resolved: hybrid — see ADR-004)
- **OQ-03**: ALPN string naming convention (resolved: see ADR-006)
- **OQ-04**: Dynamic handler registration (resolved: static at startup for the `HandlerRegistry` — see ADR-010; the `OperationRegistry` is layered by ADR-024: curated ops static, session/imported ops dynamic at their trust-boundary scopes)
- **OQ-08**: Vault integration point (resolved: CLI-embedded, assembly-layer only — see ADR-008, ADR-014, ADR-018, ADR-019)
- **OQ-16**: Safe vault operations for call protocol exposure (resolved: none for now — see ADR-014)
- **OQ-20**: Encryption key derivation (resolved: HD derivation, not PBKDF2 — see ADR-020)
- **OQ-21**: Remote vault access (resolved: vault is local-only by construction — see ADR-025; remote access requires a separate vault-server crate with its own ADR)
- **OQ-22**: Key rotation (resolved: version-indexed paths, `rotate` method — see ADR-021)

## Failure Modes

| Failure | Behavior |
|---------|----------|
| ALPN negotiation fails (no intersection) | TLS handshake fails — correct behavior, the client and server have no protocol in common |
| Handler `handle()` returns `HandlerError` | Endpoint logs the error, closes the QUIC connection. Other connections are unaffected |
| Handler panics | The handler's task is caught by tokio's panic handling. The connection is dropped. Other connections are unaffected |
| `IdentityProvider` returns `None` | AuthContext is partial. If the handler requires authentication and cannot extract credentials from the stream, it closes the connection with an auth error |
| Config reload fails | `ArcSwap<DynamicConfig>` keeps the previous valid config. Error is logged. No service interruption |
| BiStream read/write error | QUIC stream-level error. The handler detects this as an I/O error and returns from `handle()`. The connection itself may remain open for other streams — but since each handler owns a full `Connection` (one ALPN per connection, ADR-006), a stream error typically causes the handler to return, closing the connection |

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