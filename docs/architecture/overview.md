---
status: draft
last_updated: 2026-07-15
---

# Alknet Overview

## What Alknet Is

Alknet is a **core networking toolkit** for building self-hostable,
p2p-capable, "vpn-like without being a vpn" systems. It is built on
QUIC+TLS with ALPN-based protocol dispatch, plus TCP+TLS for the
web/browser path. A single endpoint accepts connections on one port
per transport, and the ALPN string negotiated during the TLS handshake
routes each connection to the correct protocol handler. Every service —
call, channels, HTTP, TTY, tunnels, SFTP — is an ALPN on a shared
endpoint or a data-channel ALPN inside channels.

This is the core insight: **a service IS an ALPN.** One endpoint, one
port per transport, many protocols — dispatched by the TLS handshake,
not by application-level peeking or separate listeners.

### Scope: core mono-repo vs. consumer repos

The mono-repo is the **core networking toolkit** — the substrate
(core, tls, call, channels), the deployment shapes (hub, worker), the
foundational protocol handlers (tty, http, ssh, tunnel, socks5, fs,
sftp), and vault (foundational to ACL). Crates that build *on top of*
a hub or worker (docker operations, agent, future applications) are
**consumer repos** — they depend on the published core crates, not on
`alknet-core` directly. See [ADR-085](decisions/085-workspace-scope-core-vs-consumer-repos.md)
for the full scope decision.

### Endpoint types and entry points (ADR-086)

A hub composes a subset of three **endpoint types**, each an independent
listener with its own identity model, auth model, and transport(s):

| Endpoint type | Identity | Auth model | Transport(s) | Client class |
|---------------|----------|------------|--------------|--------------|
| **web** | X.509 (ACME or manual) | token-based (Bearer) | TCP+TLS (HTTP, WebSocket), QUIC (WebTransport — deferred) | browsers, curl, registration |
| **native** | RFC 7250 raw key (Ed25519) | key-based (fingerprint) | QUIC (primary), TCP+TLS (fallback) | alknet-native clients, workers |
| **iroh** | RFC 7250 raw key (NodeId) | key-based (fingerprint) | iroh (relay-assisted QUIC) | p2p peers, NAT'd nodes |

A full hub runs all three; a minimal hub runs iroh alone (no public IP
required). The first real use case is web + native. See
[ADR-086](decisions/086-endpoint-types-and-entry-points.md) for the
full model, including the entry-point vs. endpoint ALPN distinction
(entry points are accepted without identity; endpoints require
identity resolution) and the split-by-endpoint-type ALPN list pattern.

## Why ALPN Dispatch

The previous architecture used a three-layer model (StreamInterface/MessageInterface, ListenerConfig, OperationEnv) that required separate listener types, application-level protocol detection via byte-peeking, and complex dispatch paths. ALPN negotiation eliminates all of this:

- Protocol detection happens at the TLS layer — no byte-peeking
- A single endpoint replaces multiple listener types
- Adding a protocol is registering an ALPN string
- Each handler owns its entire wire format

See [ADR-001](decisions/001-alpn-protocol-dispatch.md) for the full rationale.

## The Hub/Worker Model

Alknet's deployment shape is hub-and-spoke. A **hub** is a channels
hub — it accepts inbound connections (over quinn, iroh, TCP+TLS),
runs `ChannelsAdapter` on `alknet/channels`, relays data channels
between legs (ADR-079), aggregates workers' operations, and serves
discovery. A **worker** is a channels worker — it dials out to a hub
via `ChannelClient`, discovers operations via `from_call`, and exposes
its own operations on channel 0. A **hub-worker** does both.

The bidirectionality of call and channels means both sides can be both
hub and worker within a connection. A hub (A) that dials another hub
(B) is, from B's perspective, a worker. This does not require a
separate "hub-as-client" abstraction — `ChannelClient` /
`CallClient` take-over APIs (`from_connection`, `spawn_dispatch`) are
transport-agnostic and work regardless of whether the dialer is a hub,
a worker, or a hub-worker.

See [ADR-029](decisions/029-peer-graph-routing-model.md),
[ADR-034](decisions/034-outgoing-only-x509-and-three-peer-roles.md),
[ADR-079](decisions/079-hub-relay-translate-not-forward.md), and
[crates/hub/README.md](crates/hub/README.md) for the full topology.

## Crate Graph

The mono-repo contains the substrate, the deployment shapes, and the
foundational handlers. Consumer repos (docker, agent) are not in this
graph — they depend on the published core crates from their own repos
(ADR-085).

```
alknet-vault (standalone — foundational to ACL: key derivation, identity)
│
├── Substrate
│   alknet-core        ProtocolHandler, Connection, BidiStreamSource, AuthContext,
│   │                  IdentityProvider, StaticConfig, DynamicConfig, fingerprint,
│   │                  ConnectionCredentials, RemoteIdentity
│   │                  (endpoint extracted to alknet-endpoint; core is now lightweight
│   │                  types+auth+config — no quinn/iroh/rcgen deps; ConnectionCredentials
│   │                  + RemoteIdentity moved here from alknet-call per ADR-091;
│   │                  CallCredentials removed per ADR-091 Am. 2026-07-17)
│   ├── alknet-tls     TlsServerConfig + TlsClientConfig + FingerprintPinVerifier — shared TLS config across quinn + TCP+TLS + iroh (ADR-082/087; FingerprintPinVerifier moved from alknet-call per ADR-089 §5)
│   ├── alknet-call    CallAdapter on alknet/call, CallClient (spawn_dispatch only — connect removed per ADR-089 §5), OperationRegistry, adapters (no TLS/transport deps)
│   ├── alknet-channels
│   │   ├── alknet-channels-core  pure multiplexer (wire format, demux/mux) — ADR-081
│   │   └── alknet-channels-call  channel 0 pre-negotiation + lifecycle ops — ADR-081
│   ├── alknet-client  AlknetClient — native client dial seam (QUIC + TCP+TLS + iroh); produces Connection for CallClient/ChannelClient take-over (ADR-089; CallClient::connect/ChannelClient::connect_quic removed — dial centralized here)
│   └── alknet-endpoint  AlknetEndpoint — multi-transport accept-loop runner, extracted from core (ADR-083 Am. 2026-07-15); takes pre-built transports; public dispatch for SSH/WT
│
├── Deployment shapes
│   ├── alknet-hub     channels hub — accepts workers, relays, aggregates (ADR-079); dials workers via AlknetClient
│   └── alknet-worker  channels worker — dials out to a hub via AlknetClient [not yet specced]
│
├── Foundational handlers (inside channels as data-channel ALPNs, or on the endpoint)
│   ├── alknet-tty          alknet/tty — specced (ADR-052–057), implemented
│   ├── alknet-tty-local    PTY/pipe backend — sibling crate (ADR-054)
│   ├── alknet-http         h2/http1.1 + WebSocket — the web endpoint edge case (registration, browser, MCP)
│   ├── alknet-ssh          russh server — endpoint ALPN wrapping channels (channels-over-SSH); RFC 7250 keys; legacy compat [not yet specced]
│   ├── alknet-tunnel       alknet/tunnel — channels data-channel ALPN; POC-validated, minimal spec needed [not yet specced]
│   ├── alknet-socks5       SOCKS5 proxy — channels data-channel ALPN [not yet specced]
│   ├── alknet-fs           filesystem access — channels data-channel ALPN [not yet specced]
│   └── alknet-sftp         SFTP — channels data-channel ALPN [not yet specced]
│
└── Consumer repos (separate repos, depend on the published core crates)
    alknet-docker      docker operations — a docker host is a worker
    alknet-agent       LLM agent — builds on alknet-call for tool dispatch
```

Dependency rules:
- The substrate crates form a clean DAG: `channels` → `call` → `core`; `tls` → `core`. No cycles.
- `alknet-hub` and `alknet-worker` depend on the substrate (channels, call, core) and on the handlers they wire. They are consumers of the substrate, not part of it.
- No handler crate depends on another handler crate — cross-handler communication goes through `alknet/call` on channel 0.
- `alknet-call` is a protocol-foundation crate (ADR-003 Am. 1): `alknet-http` depends on it for `OperationSpec`/`Handler`/`OperationAdapter` types, not as a peer-handler dep.
- `alknet-vault` has zero alknet crate dependencies (ADR-018). It is foundational to ACL: the hub/worker identity model derives from vault-managed keys. Vault is accessed only at the assembly layer (ADR-019); handlers receive derived credentials via capabilities (ADR-014).
- Consumer repos (docker, agent) depend on the published core crates, not on `alknet-core` directly.
- Rust is the canonical implementation language (ADR-013).

See [ADR-003](decisions/003-crate-decomposition.md) (as amended by
[ADR-085](decisions/085-workspace-scope-core-vs-consumer-repos.md)) for
the decomposition rationale.

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

ALPNs are split into two layers: **endpoint ALPNs** (negotiated in the
TLS handshake, dispatched by the endpoint) and **channels data-channel
ALPNs** (negotiated via `channel/open` inside a channels connection,
dispatched by the channels substrate). See ADR-071 and ADR-073.

Within the endpoint ALPNs, there is a further distinction (ADR-086 §2):
**entry points** (connections accepted without an established peer
identity; per-request auth inside the handler) vs. **endpoints** in the
narrow sense (connections that require identity resolution before the
handler runs). This distinction determines which `TlsServerConfig`
advertises which ALPNs — each endpoint type (web, native, iroh)
advertises only the ALPNs its client class can negotiate (ADR-086 §3).

### Endpoint ALPNs

#### Entry points (no identity required at the TLS layer)

| ALPN | Handler | Endpoint type | Description |
|------|---------|---------------|-------------|
| `h2` / `http/1.1` | `HttpAdapter` | web | HTTP registration, browser API routes, stealth decoy, WebSocket upgrade (ADR-048) |
| `alknet/register` (future) | (registration handler) | native, web | Worker registration over QUIC/TCP without HTTP — a direct ALPN for enrollment. Not yet specced. |

#### Endpoints (identity required before dispatch)

| ALPN | Handler | Endpoint type | Description |
|------|---------|---------------|-------------|
| `alknet/channels` | `ChannelsAdapter` | native, web (for WS-channels), iroh | Multiplexing substrate: N channels over one transport stream (ADR-071); channel 0 = `alknet/call` (ADR-072). Identity resolved on channel 0 before dispatch. |
| `alknet/call` | `CallAdapter` | native, iroh | Call protocol: operations, streaming, pub/sub (hand-rolled EventEnvelope — ADR-064). When used as a top-level ALPN; as channel 0 inside channels, identity is resolved before dispatch. |
| `alknet/ssh` (future) | (ssh handler) | native | SSH server wrapping channels (channels-over-SSH); RFC 7250 keys; legacy-client entry point for git/sftp compat. Not yet specced. |

### Channels data-channel ALPNs

These ride inside a `alknet/channels` connection as data channels,
opened via `channel/open` (ADR-073). They get the ACL and
bidirectionality of channels + call for free. They are NOT in any
`TlsServerConfig`'s ALPN list — they are negotiated inside channels,
not at the TLS layer.

| ALPN | Handler | Status |
|------|---------|--------|
| `alknet/tty` | `TtyAdapter` | specced (ADR-052–057), implemented |
| `alknet/tunnel` | (tunnel handler) | POC-validated, minimal spec needed [not yet specced] |
| `alknet/socks5` | (SOCKS5 handler) | not yet specced |
| `alknet/fs` | (fs handler) | not yet specced |
| `alknet/sftp` | (sftp handler) | not yet specced |
| (future) | any ALPN a consumer registers | channels supports any ALPN — ADR-071 |

### SSH — an endpoint ALPN that wraps channels (ADR-086 §4)

SSH is structurally different from the channels data-channel ALPNs
above. It is an **endpoint ALPN** (negotiated at the TLS layer on the
native config), and it runs channels *inside* it
(channels-over-SSH): the SSH server accepts a connection, and each SSH
channel becomes a channels data-channel ALPN. SSH uses the same RFC
7250 keys as the native endpoint — it is a legacy-client entry point
for git/sftp compatibility, not a new identity model. SSH is gated by
channels (the channels run inside it) but is itself an endpoint ALPN,
not a data-channel ALPN. It comes later in the roadmap — tunnels,
sftp, and other data-channel ALPNs are prioritized first.

### Notes

> **`alknet/vault`** is not in the ALPN registry. alknet-vault is a
> standalone local key vault with no alknet-core dependency and no
> remote dispatch capability (ADR-025). The assembly layer (hub or
> worker binary) embeds it, unlocks it at startup, derives/decrypts
> credentials, and injects them into handler capabilities (ADR-014).
> The vault is foundational to ACL — the hub/worker identity model
> (`IdentityProvider`, `PeerEntry`, fingerprint resolution) derives
> from vault-managed keys. See ADR-008, ADR-014, ADR-018, ADR-019.

> **`alknet/http`** is the web endpoint edge case. It is an
> entry-point ALPN (`h2`/`http/1.1`), not a channels data-channel ALPN
> — it wraps the call protocol for browser/curl access (registration,
> MCP/OpenAPI adapters, WebSocket bidirectional path). It is advertised
> on the web endpoint's `TlsServerConfig` (X.509/ACME), not the native
> config. See [crates/http/README.md](crates/http/README.md) and
> ADR-086 §2 (entry points vs. endpoints).

> **Consumer-repo ALPNs** (e.g., docker operations) are not listed
> here. A consumer that builds on top of a hub or worker registers its
> operations on the call protocol (channel 0), not as a separate ALPN.
> Docker, for example, registers its operations as call-protocol ops
> (ADR-058), not as `alknet/docker`.

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

The call protocol's adapter contract (from_openapi, from_jsonschema, from_mcp, from_call, to_openapi, to_mcp) enables bidirectional composition — operations can be imported from external sources and exported to external protocols. The adapter *trait* is defined in `alknet-call`; HTTP-backed adapter implementations (`from_openapi`, `from_jsonschema`, `from_mcp`, `to_openapi`, `to_mcp`) live in `alknet-http` (`from_jsonschema` moved there per ADR-066; the QUIC-backed `from_call` stays in `alknet-call`). The existing TypeScript `@alkdev/operations` library informed the design and may be adapted for browser use (see ADR-013).

See [ADR-064](decisions/064-irpc-never-integrated-hand-rolled-framing.md) for the full rationale (supersedes [ADR-005](decisions/005-irpc-as-call-protocol-foundation.md)).

## WASM Compatibility

WASM is not an implementation target. It is a design constraint on one-way doors (see ADR-009): core types must not assume tokio or quinn, and protocol parsers that are pure data transformations remain transport-agnostic. The cost of keeping this door open is low (trait vs concrete type, abstracted I/O); the cost of closing it is irreversibly high. The browser path is through a JavaScript SDK adapted from the existing TypeScript `@alkdev/operations` library, speaking the EventEnvelope wire format over WebTransport streams — not through Rust-to-WASM compilation of the full stack (see ADR-013). Specific WASM targeting decisions are deferred to individual crate specs. See OQ-09.

## Shared Types

The following types live in alknet-core and are used across handler crates:

| Type | Purpose |
|------|---------|
| `ProtocolHandler` | The trait every handler implements |
| `Connection` | Transport connection (QUIC via quinn/iroh, a generic single stream via `from_stream` — ADR-065, or any `BidiStreamSource` impl — ADR-070) — handlers open/accept streams on it |
| `BidiStreamSource` | The trait `Connection` holds; downstream crates implement it to add connection shapes (channels, future transports) without editing core — ADR-070 |
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

### One ALPN, One Connection, One Handler (endpoint layer)

Each endpoint ALPN gets its own connection. The handler owns the
entire connection lifecycle. Handlers that need multiple streams (call,
channels) open/accept streams as needed. At the channels layer, the
model extends: one `alknet/channels` connection carries many
data-channel ALPNs, each dispatched via `channel/open` (ADR-073) — a
multiplexing power QUIC's per-connection ALPN doesn't provide natively.

### Handler Independence

No handler crate depends on another handler crate. Cross-handler
communication goes through the call protocol (`alknet/call` on channel
0) or through the channels substrate. The assembly layer (hub or
worker binary) is the only place that depends on all handlers.

## Design Decisions

All design decisions are documented as ADRs in [decisions/](decisions/).

| ADR | Decision | Summary |
|-----|----------|---------|
| [001](decisions/001-alpn-protocol-dispatch.md) | ALPN-Based Protocol Dispatch | Single endpoint, ALPN negotiation routes to handlers |
| [002](decisions/002-protocol-handler-trait.md) | ProtocolHandler Trait | One trait replaces StreamInterface/MessageInterface |
| [003](decisions/003-crate-decomposition.md) | Crate Decomposition | One crate per protocol handler, core provides shared infra (crate list superseded by [ADR-085](decisions/085-workspace-scope-core-vs-consumer-repos.md) — core mono-repo vs. consumer repos) |
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
| [017](decisions/017-call-protocol-client-and-adapter-contract.md) | Call Protocol Client and Adapter Contract | `CallClient` takes over connections (`spawn_dispatch` transport-agnostic primary; `connect` removed per ADR-089 §5 — dial extracted to `AlknetClient`); `from_call` imports remote ops; connection direction independent of call direction |
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
| [065](decisions/065-connection-from-stream-generic-single-stream.md) | `Connection::from_stream` | Generic single-stream connections — unblocks TCP+TLS, SSH channels, WebTransport, wasm |
| [070](decisions/070-bidistreamsource-trait.md) | BidiStreamSource Trait | Open `Connection` for extension — downstream crates add connection shapes without editing core |
| [071](decisions/071-channels-wire-format.md) | alknet-channels Wire Format | 9-byte chunk header; N channels over one transport stream |
| [079](decisions/079-hub-relay-translate-not-forward.md) | Hub Relay | Translate `channel/open`, byte-forward data channels; the hub never runs protocol-specific handlers |
| [082](decisions/082-alknet-tls-extraction.md) | alknet-tls Crate Extraction | Shared `TlsServerConfig` across quinn + TCP+TLS + iroh; one ACME state machine |
| [083](decisions/083-endpoint-as-accept-loop-runner.md) | Endpoint as Multi-Transport Accept-Loop Runner | Endpoint takes no TLS config; TCP+TLS is an owned transport; public `dispatch` for SSH/WT; endpoint extracted from `alknet-core` into `alknet-endpoint` (Am. 2026-07-15) |
| [085](decisions/085-workspace-scope-core-vs-consumer-repos.md) | Workspace Scope — Core vs. Consumer Repos | Core mono-repo (substrate + deployment shapes + foundational handlers + vault) vs. consumer repos (docker, agent) |
| [086](decisions/086-endpoint-types-and-entry-points.md) | Endpoint Types and Entry Points | Three endpoint types (web/native/iroh); entry-point vs. endpoint ALPN distinction; split ALPN lists per endpoint type (resolves OQ-62) |
| [087](decisions/087-tlsclientconfig-not-blocked-on-dial.md) | `TlsClientConfig` Not Blocked on Dial Seam | `alknet-tls` provides client-side TLS config; not deferred behind OQ-55; breaks the circular hedge; hub-as-client is a first-class use case |
| [089](decisions/089-alknetclient-native-dial-seam.md) | AlknetClient — Native Client Dial Seam | New crate `alknet-client`; client-side analogue of `AlknetEndpoint`; three dials (QUIC + TCP+TLS via `TlsClientConfig`, iroh via key); resolves OQ-55; `alknet/register` named (wire protocol deferred, OQ-66) |

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

## Reference Implementation

The reference implementation at `/workspace/@alkdev/alknet-main/` contains
working code that informed the new architecture. It is reference, not
constraint — understand what it did and why, then implement against
the new `ProtocolHandler` trait, ALPN router, and channels substrate.

| Module | Destination | Notes |
|--------|-------------|-------|
| `src/auth/*` | alknet-core | Identity, IdentityProvider, keys — simplified per ADR-004 |
| `src/config/*` | alknet-core | StaticConfig, DynamicConfig, ArcSwap |
| `src/transport/*` | alknet-core + alknet-tls | Transport construction → alknet-tls (ADR-082); accept loops → alknet-core (ADR-083) |
| `src/call/*` | alknet-call | EventEnvelope, registry, framing — becomes `ProtocolHandler` on `alknet/call` |
| `src/server/serve.rs` | alknet-core (reference) | Accept loop pattern informs the ALPN router; rewritten as multi-transport accept-loop runner (ADR-083) |
| `src/interface/ssh.rs`, `src/server/*` | alknet-ssh [not yet specced] | SSH channel handling — future russh server channels wrapper for git/sftp compat |
| `src/socks5/*`, `src/client/*` | alknet-socks5 [not yet specced] | SOCKS5 protocol — future channels data-channel ALPN |