---
status: draft
last_updated: 2026-06-04
---

# Open Questions

## Transport

### OQ-01: TLS certificate management strategy
- **Origin**: [server.md](server.md)
- **Status**: ~~resolved~~
- **Priority**: ~~medium~~ —
- **Resolution**: ADR-008 — Support both domain-based and IP-based ACME/Let's Encrypt auto-provisioning, plus manual certs. Domain-based uses standard certbot-style flow with HTTP-01/TLS-ALPN-01 challenges. IP-based uses short-lived certs via TLS-ALPN-01 on port 443. Manual certs via `--tls-cert`/`--tls-key` always supported. Implementation uses `rustls-acme` or similar pure-Rust ACME client.
- **Cross-references**: [ADR-008](decisions/008-acme-lets-encrypt.md), Server spec, TlsTransport implementation

### OQ-02: iroh relay configuration defaults
- **Origin**: [transport.md](transport.md)
- **Status**: ~~resolved~~
- **Priority**: ~~low~~ —
- **Resolution**: ADR-009 — Default to n0's free relay servers. Allow override via `--iroh-relay <url>`. Document self-hosted relay setup. This matches iroh's own defaults and minimizes friction for testing/development.
- **Cross-references**: [ADR-009](decisions/009-default-iroh-relay.md), Transport spec

### OQ-05: Transport chaining support in CLI
- **Origin**: [transport.md](transport.md)
- **Status**: ~~resolved~~
- **Priority**: ~~low~~ —
- **Resolution**: ADR-010 — Support `--transport iroh --proxy socks5://...` natively in the CLI. iroh's endpoint builder accepts proxy configuration directly, so the implementation is minimal. Other transport combinations (TCP+TLS) are already implicit.
- **Cross-references**: [ADR-010](decisions/010-transport-chaining-cli.md), Transport spec

## Client

### OQ-06: SSH config file parsing
- **Origin**: [client.md](client.md)
- **Status**: ~~resolved~~
- **Priority**: ~~low~~ —
- **Resolution**: ADR-011 — No `~/.ssh/config` parsing, no custom config file. Configuration is programmatic-first: CLI flags, library API structs (`ConnectOptions`, `ServeOptions`), and environment variables. Cross-platform path issues (`~` expansion) are avoided. The library API is the primary interface; if config files are needed later, they can be a separate layer.
- **Cross-references**: [ADR-011](decisions/011-no-ssh-config-programmatic-api.md), Client spec

## Server

### OQ-07: ACME/Let's Encrypt support
- **Origin**: [server.md](server.md)
- **Status**: ~~resolved~~
- **Priority**: ~~medium~~ —
- **Resolution**: ADR-008 — Same resolution as OQ-01. Both domain-based (standard, domain-bound, auto-renewing) and IP-based (short-lived, no domain required) ACME flows are supported. The domain-based path requires port 80 or DNS access for challenges. The IP-based path uses TLS-ALPN-01 on port 443 and requires the ACME client to run continuously.
- **Cross-references**: [ADR-008](decisions/008-acme-lets-encrypt.md), Server spec, TlsTransport

### OQ-08: Connection limits and rate limiting
- **Origin**: [server.md](server.md)
- **Status**: ~~resolved~~
- **Priority**: ~~low~~ —
- **Resolution**: ADR-013 — Two-layer approach: (1) Structured logging of auth attempts and connections at INFO level for fail2ban integration on Linux — matches our production fail2ban setup with nftables and systemd journal. (2) Built-in rate limiting: `--max-connections-per-ip` and `--max-auth-attempts` flags providing platform-independent abuse protection.
- **Cross-references**: [ADR-013](decisions/013-fail2ban-friendly-logging.md), Server spec, Production fail2ban docs

### OQ-04: Authentication beyond Ed25519 keys
- **Origin**: [client.md](client.md), [server.md](server.md)
- **Status**: ~~resolved~~
- **Priority**: ~~low~~ —
- **Resolution**: ADR-012 — Ed25519 public key (default, unchanged) + OpenSSH certificate authority support (new, important for multi-user). No password authentication over SSH channels. If a local SOCKS5 proxy needs its own auth, that's a separate concern. Cert-authority makes multi-user management practical: one CA entry in `authorized_keys` instead of N individual keys. Certificates support expiry and restrictions.
- **Cross-references**: [ADR-012](decisions/012-auth-ed25519-and-cert-authority.md), Client spec, Server spec

## TUN

### OQ-03: Windows TUN support scope
- **Origin**: [tun-shim.md](tun-shim.md)
- **Status**: ~~resolved~~
- **Priority**: ~~low~~ —
- **Resolution**: ADR-014 — TUN is deferred entirely from the alknet project. For VPN-like behavior, users run `tun2proxy --proxy socks5://127.0.0.1:1080` alongside alknet. This eliminates all TUN-related scope questions (Windows, TCP reconstruction, etc.).
- **Cross-references**: [ADR-014](decisions/014-defer-tun-recommend-socks5-proxy.md)

### OQ-09: TCP reconstruction approach for TUN
- **Origin**: [tun-shim.md](tun-shim.md)
- **Status**: ~~resolved~~
- **Priority**: ~~medium~~ —
- **Resolution**: ADR-014 — TUN is deferred from alknet. tun2proxy (external tool) handles this if users need VPN-like behavior.
- **Cross-references**: [ADR-014](decisions/014-defer-tun-recommend-socks5-proxy.md)

## NAPI / PubSub

### OQ-10: NAPI wrapper API surface
- **Origin**: [napi-and-pubsub.md](napi-and-pubsub.md)
- **Status**: ~~resolved~~
- **Priority**: ~~medium~~ —
- **Resolution**: ADR-016 — Expose both `connect()` and `serve()` from the start. Both are fundamental operations needed by the pubsub event target system (spokes use `connect()`, hubs could use `serve()`). The NAPI layer is transport-agnostic — it doesn't know about pubsub's `EventEnvelope`. The pubsub adapter wraps the `Duplex` stream. This ensures the NAPI wrapper is reusable for any stream-based protocol, not tied specifically to pubsub.
- **Cross-references**: [ADR-016](decisions/016-napi-expose-connect-and-serve.md), napi-and-pubsub.md

### OQ-11: napi-rs vs uniffi for FFI bridge
- **Origin**: [napi-and-pubsub.md](napi-and-pubsub.md)
- **Status**: ~~resolved~~
- **Priority**: ~~low~~ —
- **Resolution**: ADR-015 — Use napi-rs. It's the standard for Node.js native addons, matches our primary consumer (TypeScript/Node.js), and has the best ecosystem and documentation. If future Python or mobile consumers are needed, a separate uniffi layer can be added — the Rust core doesn't change.
- **Cross-references**: [ADR-015](decisions/015-napi-rs-for-ffi-bridge.md), napi-and-pubsub.md

## Configuration

### OQ-12: Per-user forwarding scope vs global rules
- **Origin**: [research/configuration.md](../research/configuration.md)
- **Status**: open
- **Priority**: medium
- **Resolution**: (pending)
- **Cross-references**: configuration.md

### OQ-13: Config file auto-reload via file watching
- **Origin**: [research/configuration.md](../research/configuration.md)
- **Status**: resolved
- **Priority**: low
- **Resolution**: No file watching. CLI loads once at startup; NAPI/hub reload explicitly. File watching is a potential attack vector and unnecessary complexity for a security tool.
- **Cross-references**: configuration.md

### OQ-14: ArcSwap vs RwLock for dynamic config
- **Origin**: [research/configuration.md](../research/configuration.md)
- **Status**: resolved
- **Priority**: low
- **Resolution**: ArcSwap. Lock-free reads on the hot path (every auth check, every channel open). `RwLock` adds contention. `arc-swap` is small (~500 lines) and well-maintained.
- **Cross-references**: configuration.md

### OQ-15: TLS + WebTransport + iroh QUIC listener coexistence
- **Origin**: [research/configuration.md](../research/configuration.md)
- **Status**: open
- **Priority**: medium
- **Resolution**: (pending — needs R&D in WebTransport transport session)
- **Cross-references**: [auth.md](auth.md), OQ-19

### OQ-16: Transport-specific forwarding policy (e.g., WebTransport clients restricted to alknet-* channels)
- **Origin**: [research/configuration.md](../research/configuration.md)
- **Status**: open
- **Priority**: low
- **Resolution**: (pending — defer to forwarding policy design)
- **Cross-references**: configuration.md

### OQ-17: Transport-aware auth layer (SSH keys vs API keys for non-SSH transports)
- **Origin**: [research/configuration.md](../research/configuration.md)
- **Status**: ~~resolved~~
- **Priority**: ~~medium~~ —
- **Resolution**: ADR-023 — Unified auth with shared key material. SSH transports use SSH pubkey auth. Non-SSH transports (WebTransport) use Ed25519-signed timestamp tokens. Both verify against the same `authorized_keys` set. The presentation differs per transport, but the identity is unified. `AuthPolicy` holds both `SshAuthConfig` and `TokenAuthConfig`, with `TokenKeySource::Shared` as the default (same keys for both paths). `IdentityProvider` trait decouples alknet-core from identity storage.
- **Cross-references**: [ADR-023](decisions/023-unified-auth-shared-key-material.md), [auth.md](auth.md), OQ-15

## Auth

### OQ-18: Source of Identity.scopes — ForwardingPolicy, IdentityProvider, or both?
- **Origin**: [auth.md](auth.md)
- **Status**: open
- **Priority**: medium
- **Resolution**: (pending)
- **Cross-references**: ADR-023, [call-protocol.md](call-protocol.md)

### OQ-19: Separate TLS identity for WebTransport vs shared with SSH-over-TLS?
- **Origin**: [auth.md](auth.md)
- **Status**: open
- **Priority**: low
- **Resolution**: (pending)
- **Cross-references**: OQ-15

## Call Protocol

### OQ-20: Spoke registration and discovery on connect/disconnect
- **Origin**: [call-protocol.md](call-protocol.md)
- **Status**: open
- **Priority**: medium
- **Resolution**: (pending — registration on connect / cleanup on disconnect is the leading approach)
- **Cross-references**: ADR-024, ADR-025

### OQ-21: Routing calls to specific spokes with same-service operations
- **Origin**: [call-protocol.md](call-protocol.md)
- **Status**: ~~resolved~~
- **Priority**: ~~medium~~ —
- **Resolution**: ADR-024, ADR-025 — Operation paths use `/{spoke}/{service}/{op}` format. The first path segment identifies the spoke and routes the call to the correct connected node. Multiple spokes exposing the same service (e.g., two dev envs both with `/fs/*`) are differentiated by the spoke prefix (`/dev1/fs/readFile` vs `/dev2/fs/readFile`). The hub maintains a routing table mapping spoke identity to connection. This mirrors iroh's ALPN dispatch: first segment = routing key.
- **Cross-references**: [call-protocol.md](call-protocol.md), ADR-024, ADR-025

### OQ-22: Client streaming (streaming inputs) in the call protocol?
- **Origin**: [call-protocol.md](call-protocol.md)
- **Status**: open
- **Priority**: low
- **Resolution**: (pending)
- **Cross-references**: ADR-024