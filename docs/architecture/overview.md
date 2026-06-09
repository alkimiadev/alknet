---
status: reviewed
last_updated: 2026-06-07
---

# Alknet Overview

## Purpose

Alknet is a self-hostable SSH-based tunnel tool that provides VPN-like functionality without being a VPN protocol. It enables:

- **Private tunneling** of services (Postgres, Redis, internal APIs) over SSH
- **Censorship circumvention** — SSH over TLS on port 443 looks like HTTPS to DPI
- **NAT traversal** — iroh transport allows peer-to-peer connections without public IPs or port forwarding
- **Service mesh connectivity** — a lightweight transport layer for the pubsub/operations event system

The core insight: SSH tunnels work because SSH is fundamental infrastructure. Blocking it breaks the internet. Alknet makes SSH tunneling accessible through a simple CLI with pluggable transports.

## Crate Structure

Alknet is decomposed into six crates with a strict acyclic dependency graph (ADR-027):

| Crate | Purpose | Exists Now? |
|-------|---------|-------------|
| **alknet-core** | Transport, SSH, call protocol, config, auth types, `OperationSpec`, `Interface` trait | Yes |
| **alknet-napi** | Node.js native addon via napi-rs | Yes |
| **alknet-secret** | BIP39, SLIP-0010 HD key derivation, AES-256-GCM, `SecretProtocol` irpc service | Phase 2+ |
| **alknet-storage** | SQLite-backed metagraph, identity tables, ACL graph, honker, `StorageProtocol` | Phase 2+ |
| **alknet-flowgraph** | `FlowGraph<N,E>` over petgraph, operation graph, call graph | Phase 2+ |
| **alknet** (CLI) | Binary that assembles everything with feature flags | Yes |

The four library crates (core, secret, storage, flowgraph) are independent of each other. Dependencies flow upward only: the CLI binary sits at the top and wires concrete implementations together. alknet-storage implements alknet-core's `IdentityProvider` trait without a crate dependency — the CLI binary provides the bridge.

irpc is behind a feature flag in alknet-core. Nodes that only do SSH tunneling don't need the service layer overhead.

## Three-Layer Model

Alknet uses a three-layer model (ADR-026, ADR-035):

| Layer | Responsibility | Examples |
|-------|---------------|----------|
| **Layer 1: Transport** | Produces byte streams (`AsyncRead + AsyncWrite + Unpin + Send`) | TCP, TLS, iroh, WebTransport (future) |
| **Layer 2: Interface** | Two categories: StreamInterface (consumes transport stream, produces session) and MessageInterface (handles discrete requests, manages own transport) | Stream: SSH, raw framing. Message: HTTP, DNS |
| **Layer 3: Protocol** | Carries semantics — operation registry, service calls, events | Call protocol, OperationEnv, operation dispatch |

SSH is an interface, not a transport. DNS is a message interface, not a transport.
The three-layer model enables HTTP interfaces (stealth mode byte-peek),
DNS control channels, and local service mesh (raw framing) without wrapping SSH
inside those transports.

A stream-based connection is always a (Transport, StreamInterface) pair.
Message-based interfaces manage their own transport. The protocol layer is
agnostic to both.

## Service Layer

The irpc service layer decomposes alknet's core responsibilities into independently testable, deployable, and replaceable components (ADR-033, [services.md](services.md)):

- **Auth** (`AuthProtocol`) — verify identities, check credentials
- **Secret** (`SecretProtocol`) — derive keys, encrypt/decrypt
- **Config** (`ConfigProtocol`) — dynamic config reload
- **Storage** (`StorageProtocol`) — graph CRUD, metagraph operations

**OperationEnv** is the universal composition mechanism. A handler receives `context.env.invoke("secrets", "derive", input)` and doesn't know whether the dispatch is local (direct function call), in-cluster (irpc service), or cross-node (call protocol `EventEnvelope`). Three dispatch paths, one handler-facing API.

**Phase boundary**: Phase 1 ships `ConfigIdentityProvider` (ArcSwap-backed) and `ConfigServiceImpl` (ArcSwap-backed) as the only auth and config implementations. The irpc service protocols (`AuthProtocol`, `SecretProtocol`, etc.) and the production deployment topology (multi-node with `StorageIdentityProvider`) are contracted in the specs but will be implemented in Phase 2+. Application services (DockerService, NodeService, agent services) are downstream concerns that build on top of the call protocol and OperationEnv.

## Identity

`Identity` struct and `IdentityProvider` trait are core types in alknet-core (ADR-029, [identity.md](identity.md)):

```rust
pub struct Identity {
    pub id: String,          // Fingerprint (config auth) or account UUID (database auth)
    pub scopes: Vec<String>, // Authorization scope strings
    pub resources: HashMap<String, Vec<String>>, // Resource-level authorization
}
```

`IdentityProvider` decouples alknet-core from identity storage. Phase 1 ships `ConfigIdentityProvider` (reads from `ArcSwap<DynamicConfig.auth>`). `StorageIdentityProvider` (Phase 2+, backed by SQLite) replaces it for production deployments. Both produce the same `Identity` result.

## Exports

### Binary: `alknet`

A single binary with subcommands:

```
alknet serve     — Start the server (accepts SSH connections)
alknet connect  — Start the client (opens SSH session, exposes SOCKS5/port-forwards)
```

### Library: `alknet-core`

The `alknet-core` crate exports the pluggable components for embedding or programmatic use:

- `Transport` trait — produces a duplex stream for SSH to run over
- `TcpTransport` — direct TCP connection
- `TlsTransport` — TCP + tokio-rustls TLS
- `IrohTransport` — iroh QUIC P2P connection
- `Interface` trait → `StreamInterface` trait and `MessageInterface` trait (ADR-035)
- `InterfaceSession` trait — `recv()`/`send()` producing/consuming `InterfaceEvent` frames
- `InterfaceRequest` / `InterfaceResponse` — normalized request/response for message interfaces
- `Socks5Server` — local SOCKS5 proxy that forwards through SSH channels
- `PortForwarder` — manages local/remote port forwards
- `ServerHandler` → `SshInterface` — russh server handler with configurable auth and channel policies
- `Identity` / `IdentityProvider` — core identity types (ADR-029)
- `CredentialProvider` / `CredentialSet` — outbound credential types (ADR-036)
- `OperationSpec` — operation registration for call protocol (ADR-025)
- `OperationEnv` / `OperationContext` — universal composition and operation context
- `ConnectOptions` / `ServeOptions` — programmatic configuration structs
- `StaticConfig` / `DynamicConfig` — static/immutable vs, hot-reloadable config (ADR-030)
- `ConfigReloadHandle` — programmatic reload of dynamic config
- `ForwardingPolicy` — rule-based allow/deny for channel targets (ADR-031)
- `ListenerConfig` — stream and message listener configuration

## Dependencies

| Dependency | Purpose | Crate | Feature-gated |
|------------|---------|-------|---------------|
| `russh` | SSH client & server | core | No (core) |
| `tokio` | Async runtime | core | No (core) |
| `tokio-rustls` | TLS wrapping | core | Yes (`tls`) |
| `rustls` | TLS implementation | core | Yes (`tls`) |
| `rustls-acme` | ACME/Let's Encrypt auto-cert | core | Yes (`acme`) |
| `iroh` | P2P QUIC transport | core | Yes (`iroh`) |
| `irpc` | Streaming RPC service layer | core | Yes (`irpc`) |
| `arc-swap` | Lock-free dynamic config | core | No (core) |
| `serde` | Serialization | core | No (core) |
| `clap` | CLI argument parsing | CLI | No (CLI) |
| `toml` | TOML config file | CLI | No (CLI) |
| `tracing` | Structured logging | core | No (core) |
| `anyhow` / `thiserror` | Error handling | core | No (core) |
| `bip39` | Mnemonic generation | secret | No (secret) |
| `ed25519-bip32` | HD key derivation | secret | No (secret) |
| `aes-gcm` | AES-256-GCM encryption | secret | No (secret) |
| `rusqlite` | SQLite (via honker) | storage | No (storage) |
| `honker` | Event-sourced storage | storage | No (storage) |
| `petgraph` | Graph data structure | storage, flowgraph | No |
| `jsonschema` | JSON Schema validation | storage, flowgraph | No |

> Note: `tun-rs` is no longer a dependency. TUN support is deferred in favor of the external `tun2proxy` tool (ADR-014).

## Architecture Constraints

1. **SSH runs over transport, not alongside** — The transport layer produces a single `AsyncRead+AsyncWrite+Unpin+Send` stream. SSH runs over that stream via `russh::client::connect_stream()` / `russh::server::run_stream()`. The SSH layer never knows what transport it's on. (ADR-001, ADR-004)

2. **Three-layer model: Transport, Interface, Protocol** — SSH is a StreamInterface (Layer 2), not a transport (Layer 1). HTTP and DNS are MessageInterfaces (Layer 2). A connection is always a (Transport, StreamInterface) pair for stream-based interfaces, or a standalone MessageInterface for message-based ones. The call protocol (Layer 3) is agnostic to both. This enables HTTP interfaces, DNS control channels, and local service mesh without wrapping SSH. (ADR-026, ADR-035)

3. **SOCKS5 is the primary client interface** — Port forwarding is built on top of SOCKS5-like channel management. For VPN-like "route all traffic" behavior, users run `tun2proxy` alongside alknet's SOCKS5 proxy. TUN is not in the project scope. (ADR-005, ADR-014)

4. **No logging of tunnel destinations** — The server logs auth attempts and connections (for fail2ban) but does not log `channel_open_direct_tcpip` destinations, DNS lookups, or bytes transferred. (ADR-006, ADR-013)

5. **Programmatic-first API** — Configuration via CLI flags, library API structs (`ConnectOptions`, `ServeOptions`), and environment variables. No `~/.ssh/config` parsing. Optional `--config` TOML file for reproducible deployments. (ADR-011, ADR-030)

6. **Feature flags control transport inclusion** — `tls`, `iroh`, `acme`, `irpc` are feature-gated so the base install is lean. Users opt in to heavier dependencies.

7. **Authentication is key-based and unified** — Ed25519 public key (default) and OpenSSH certificate authority. Same key material for SSH and token auth. Identity resolves through `IdentityProvider` trait, decoupling core from identity storage. (ADR-012, ADR-023, ADR-029)

8. **NAPI exposes both connect() and serve()** — The napi-rs wrapper provides client and server functionality, using napi-rs as the FFI bridge. The NAPI layer is transport-agnostic and not tied to pubsub. (ADR-015, ADR-016)

9. **Static/dynamic config split** — Transport-level settings (listen address, TLS certs) are immutable after startup. Auth, forwarding policy, and rate limits are hot-reloadable via `ArcSwap<DynamicConfig>`. (ADR-030)

10. **Forwarding policy enforced before proxy spawn** — Each `channel_open_direct_tcpip` is checked against `ForwardingPolicy` before a TCP connection is made. Default-allow preserves current behavior. (ADR-031)

11. **OperationEnv as universal composition mechanism** — Handlers call `context.env.invoke(namespace, op, input)` regardless of dispatch path (local, irpc service, remote call protocol). (ADR-033)

12. **Event boundary discipline** — Domain events (Honker streams) stay within the owning service. irpc calls are synchronous and in-cluster. Call protocol `EventEnvelope` is the only thing that crosses node boundaries. (ADR-032)

13. **Error handling follows a consistent layered pattern** — Transport and auth errors cause reconnection (client, with exponential backoff) or connection rejection (server). Channel-level errors (target unreachable, proxy failure) close the individual channel without killing the session. Library API errors propagate via `anyhow::Result` / `thiserror` types. CLI reports errors to stderr with appropriate exit codes. NAPI errors are marshalled as JavaScript exceptions.

## Design Decisions

| ADR | Decision | Summary |
|-----|----------|---------|
| [001](decisions/001-pluggable-transport.md) | Pluggable transport | Transport trait produces `AsyncRead+AsyncWrite+Unpin+Send`, SSH consumes it |
| [002](decisions/002-tun-separate-process.md) | TUN shim separate | Superseded — TUN is deferred, use tun2proxy (ADR-014) |
| [003](decisions/003-iroh-stream-join.md) | iroh stream join | `tokio::io::join(recv, send)` combines QUIC halves |
| [004](decisions/004-ssh-over-transport.md) | SSH over transport | SSH never accesses TCP/iroh/TLS directly |
| [005](decisions/005-socks5-before-tun.md) | SOCKS5 first | SOCKS5 is the primary interface; TUN is external (tun2proxy) |
| [006](decisions/006-no-logging-of-tunnel-destinations.md) | No logging of tunnel destinations | Server logs auth and connections, not destinations |
| [007](decisions/007-napi-single-stream.md) | NAPI single stream | NAPI exposes duplex streams, not SSH multiplexing |
| [008](decisions/008-acme-lets-encrypt.md) | ACME/Let's Encrypt | Auto-provision TLS certs, domain and IP paths |
| [009](decisions/009-default-iroh-relay.md) | Default iroh relay | n0 relay by default, `--iroh-relay` override |
| [010](decisions/010-transport-chaining-cli.md) | Transport chaining | `--proxy` works with all transports natively |
| [011](decisions/011-no-ssh-config-programmatic-api.md) | Programmatic-first | No SSH config files; options are structs, env vars, CLI flags (amended by ADR-030 for optional TOML) |
| [012](decisions/012-auth-ed25519-and-cert-authority.md) | Key + cert-authority | Ed25519 keys + OpenSSH CA; no password auth |
| [013](decisions/013-fail2ban-friendly-logging.md) | Fail2ban-friendly | Structured auth logs + built-in rate limiting |
| [014](decisions/014-defer-tun-recommend-socks5-proxy.md) | Defer TUN | Use tun2proxy for VPN-like behavior; no alknet-tun binary |
| [015](decisions/015-napi-rs-for-ffi-bridge.md) | napi-rs | Standard Node.js native addon tooling |
| [016](decisions/016-napi-expose-connect-and-serve.md) | connect + serve | NAPI exposes both client and server from the start |
| [017](decisions/017-stealth-mode-protocol-multiplexing.md) | Stealth mode | Protocol multiplexing on port 443 |
| [018](decisions/018-control-channel-for-pubsub.md) | Control channel | Reserved `alknet-control` destination for pubsub |
| [019](decisions/019-proxy-dual-semantics.md) | Proxy dual semantics | `--proxy` routes transport on client, data on server |
| [023](decisions/023-unified-auth-shared-key-material.md) | Unified auth | Same key material for SSH and token auth |
| [024](decisions/024-bidirectional-call-protocol.md) | Bidirectional call protocol | Both sides can initiate calls |
| [025](decisions/025-handler-spec-separation.md) | Handler/spec separation | Downstream registers operations without modifying core |
| [026](decisions/026-transport-interface-separation.md) | Three-layer model | SSH is Layer 2, not Layer 1 |
| [027](decisions/027-crate-decomposition.md) | Crate decomposition | Six crates, acyclic deps, feature-gated irpc |
| [028](decisions/028-auth-irpc-service.md) | Auth as irpc service | IdentityProvider is the contract, irpc is one backend |
| [029](decisions/029-identity-core-type.md) | Identity as core type | `Identity` and `IdentityProvider` in alknet-core |
| [030](decisions/030-static-dynamic-config-split.md) | Static/dynamic config | ArcSwap for hot-reloadable auth and forwarding |
| [031](decisions/031-forwarding-policy.md) | Forwarding policy | Per-identity, per-destination, per-transport rules |
| [032](decisions/032-event-boundary-discipline.md) | Event boundary | Domain events never cross service boundaries |
| [033](decisions/033-operationenv-irpc-call-protocol.md) | OperationEnv | Universal composition, three dispatch paths |
| [034](decisions/034-head-worker-terminology.md) | Head/worker | Replaces hub/spoke terminology |
| [035](decisions/035-streaminterface-messageinterface-split.md) | StreamInterface/MessageInterface | Two Layer 2 trait categories for stream vs message |
| [036](decisions/036-credentialprovider-core-type.md) | CredentialProvider as core type | Outbound credentials in `alknet_core::credentials` |
| [037](decisions/037-api-keys-dynamic-config.md) | API keys in DynamicConfig | Hash-verified bearer tokens for service accounts |

## Open Questions

See [open-questions.md](open-questions.md) for all open and resolved questions.
Key open questions: OQ-15 (QUIC coexistence), OQ-19 (WebTransport TLS),
OQ-20 (worker registration), OQ-IF-01 (Interface session / EventEnvelope
relationship).

## References

- [transport.md](transport.md) — Transport abstraction (Layer 1)
- [interface.md](interface.md) — StreamInterface and MessageInterface (Layer 2)
- [call-protocol.md](call-protocol.md) — Call protocol (Layer 3)
- [auth.md](auth.md) — Unified authentication, API keys, credential presentation
- [identity.md](identity.md) — Identity and IdentityProvider
- [credentials.md](credentials.md) — CredentialProvider and CredentialSet (outbound auth)
- [definitions.md](definitions.md) — Terminology disambiguation
- [configuration.md](configuration.md) — StaticConfig, DynamicConfig, ForwardingPolicy
- [services.md](services.md) — irpc service layer, OperationEnv
- [server.md](server.md) — Server acceptance, channel handling
- [client.md](client.md) — Client connection, SOCKS5, port forwarding
- [napi-and-pubsub.md](napi-and-pubsub.md) — NAPI wrapper and pubsub adapter
- [storage.md](storage.md) — alknet-storage: metagraph, identity, ACL
- [flowgraph.md](flowgraph.md) — alknet-flowgraph: call graph, operation graph
- [secret-service.md](secret-service.md) — alknet-secret: BIP39, SLIP-0010, AES-GCM
- [Feasibility Assessment](../research/feasibility/ssh-tunnel-vpn-alternative-feasibility.md)
- [russh API](/workspace/russh) — SSH client/server library
- [Dispatch](/workspace/@alkdev/dispatch) — Reference implementation of russh port forwarding
- [iroh](/workspace/iroh) — P2P QUIC connections
- [tun2proxy](https://github.com/tun2proxy/tun2proxy) — Recommended external TUN-to-SOCKS5 tool
- [irpc](/workspace/irpc) — iroh streaming RPC
- [Production certbot setup](../research/ops/certbot.md) — Let's Encrypt on our infrastructure
- [Production fail2ban setup](../research/ops/fail2ban.md) — fail2ban with nftables on our infrastructure