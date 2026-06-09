---
status: draft
last_updated: 2026-06-07
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
- **Status**: ~~resolved~~
- **Priority**: ~~medium~~ —
- **Resolution**: ADR-031 — Start with global rules + principal matching from `Identity.scopes`. Per-user scope from `peer_credentials.metadata.scopes` via `IdentityProvider`. The `ForwardingPolicy` evaluates rules against `Identity.id` and `Identity.scopes` from the authenticated identity.
- **Cross-references**: [ADR-031](decisions/031-forwarding-policy.md), [configuration.md](configuration.md)

### OQ-13: Config file auto-reload via file watching
- **Origin**: [research/configuration.md](../research/configuration.md)
- **Status**: resolved
- **Priority**: low
- **Resolution**: No file watching. CLI loads once at startup; NAPI/head reload explicitly. File watching is a potential attack vector and unnecessary complexity for a security tool.
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
- **Resolution**: (deferred to Phase 4 — needs R&D in WebTransport transport session)
- **Cross-references**: [auth.md](auth.md), OQ-19, [interface.md](interface.md)

### OQ-16: Transport-specific forwarding policy (e.g., WebTransport clients restricted to alknet-* channels)
- **Origin**: [research/configuration.md](../research/configuration.md)
- **Status**: ~~resolved~~
- **Priority**: ~~low~~ —
- **Resolution**: ADR-031 — Add `TransportKind` match in `ForwardingRule`. WebTransport clients can be restricted to `alknet-*` channels via `TargetPattern::AlknetPrefix` combined with a `TransportKind::WebTransport` filter.
- **Cross-references**: [ADR-031](decisions/031-forwarding-policy.md), [configuration.md](configuration.md)

### OQ-17: Transport-aware auth layer (SSH keys vs API keys for non-SSH transports)
- **Origin**: [research/configuration.md](../research/configuration.md)
- **Status**: ~~resolved~~
- **Priority**: ~~medium~~ —
- **Resolution**: ADR-023 — Unified auth with shared key material. SSH transports use SSH pubkey auth. Non-SSH transports (WebTransport) use Ed25519-signed timestamp tokens. Both verify against the same `authorized_keys` set. The presentation differs per transport, but the identity is unified. `AuthPolicy` holds both `SshAuthConfig` and `TokenAuthConfig`, with `TokenKeySource::Shared` as the default (same keys for both paths). `IdentityProvider` trait decouples alknet-core from identity storage.
- **Cross-references**: [ADR-023](decisions/023-unified-auth-shared-key-material.md), [identity.md](identity.md), OQ-15

### OQ-23: irpc dependency — always or behind feature flag?
- **Origin**: [research/integration-plan.md](../research/integration-plan.md)
- **Status**: ~~resolved~~
- **Priority**: medium —
- **Resolution**: ADR-027 — Feature flag. Nodes that only do SSH tunneling don't need the service layer. irpc is behind a feature flag in alknet-core and an independent dependency in alknet-secret and alknet-storage.
- **Cross-references**: [ADR-027](decisions/027-crate-decomposition.md)

### OQ-24: DNS control channel scope for initial implementation?
- **Origin**: [research/integration-plan.md](../research/integration-plan.md)
- **Status**: ~~resolved~~
- **Priority**: medium —
- **Resolution**: ADR-026 — DNS control channel carries call protocol frames only (no SSH tunneling over DNS). The (DNS transport, raw framing interface) pair sends `EventEnvelope` directly. SSH-over-DNS is a future possibility but out of scope.
- **Cross-references**: [ADR-026](decisions/026-transport-interface-separation.md), [interface.md](interface.md)

### OQ-25: alknet-storage and alknet-secret irpc dependency
- **Origin**: [research/integration-plan.md](../research/integration-plan.md)
- **Status**: ~~resolved~~
- **Priority**: low —
- **Resolution**: ADR-027 — Independently. They're separate crates. irpc is a shared library they both use as an independent dependency.
- **Cross-references**: [ADR-027](decisions/027-crate-decomposition.md)

## Auth

### OQ-18: Source of Identity.scopes — ForwardingPolicy, IdentityProvider, or both?
- **Origin**: [auth.md](auth.md)
- **Status**: ~~resolved~~
- **Priority**: ~~medium~~ —
- **Resolution**: ADR-029 and ADR-031 — `IdentityProvider` owns scopes. The `Identity` struct includes `scopes` and `resources` fields populated by the `IdentityProvider` implementation (config-based or database-backed). `ForwardingPolicy` uses scopes from `Identity` — it consumes them, it doesn't produce them.
- **Cross-references**: [ADR-029](decisions/029-identity-core-type.md), [ADR-031](decisions/031-forwarding-policy.md), [identity.md](identity.md)

### OQ-19: Separate TLS identity for WebTransport vs shared with SSH-over-TLS?
- **Origin**: [auth.md](auth.md)
- **Status**: open
- **Priority**: low
- **Resolution**: (deferred to Phase 4 — QUIC is UDP, TLS-over-TCP is TCP, they can share port 443 without conflict)
- **Cross-references**: OQ-15, [interface.md](interface.md)

## Call Protocol

### OQ-20: Worker registration and discovery on connect/disconnect
- **Origin**: [call-protocol.md](call-protocol.md)
- **Status**: open
- **Priority**: medium
- **Resolution**: (pending — registration on connect / cleanup on disconnect is the leading approach but needs spec in call-protocol.md)
- **Cross-references**: ADR-024, ADR-025

### OQ-21: Routing calls to specific workers with same-service operations
- **Origin**: [call-protocol.md](call-protocol.md)
- **Status**: ~~resolved~~
- **Priority**: ~~medium~~ —
- **Resolution**: ADR-024, ADR-025 — Operation paths use `/{node}/{service}/{op}` format. The first path segment identifies the node and routes the call to the correct connected node. Multiple workers exposing the same service are differentiated by the node prefix (`/dev1/fs/readFile` vs `/dev2/fs/readFile`). The head maintains a routing table mapping node identity to connection.
- **Cross-references**: [call-protocol.md](call-protocol.md), ADR-024, ADR-025

### OQ-22: Client streaming (streaming inputs) in the call protocol?
- **Origin**: [call-protocol.md](call-protocol.md)
- **Status**: ~~resolved~~
- **Priority**: ~~low~~ —
- **Resolution**: Deferred. Current model (single request, optional streaming response) covers all identified use cases. Client streaming can be added later if needed.
- **Cross-references**: ADR-024

## Services

### OQ-SVC-01: Should the secret service support multiple seed phrases (one per tenant)?
- **Origin**: [secret-service.md](secret-service.md)
- **Status**: open
- **Priority**: low
- **Resolution**: (deferred — one seed per node is simplest; multi-seed can be added later by indexing `Unlock` with a tenant ID)
- **Cross-references**: [secret-service.md](secret-service.md)

### OQ-SVC-02: Should service protocols use postcard (binary) or JSON for remote calls?
- **Origin**: [research/services.md](../research/services.md)
- **Status**: ~~resolved~~
- **Priority**: low —
- **Resolution**: Postcard for irpc (Rust-to-Rust, efficient). JSON for call protocol (cross-language, universal). The irpc remote path naturally uses postcard.
- **Cross-references**: [services.md](services.md)

### OQ-SVC-03: How does the secret service integrate with the existing EncryptedDataSchema from @alkdev/storage?
- **Origin**: [secret-service.md](secret-service.md)
- **Status**: open
- **Priority**: medium
- **Resolution**: (pending — Rust implementation replaces PBKDF2 password-based encryption with derived AES-256-GCM keys; EncryptedData format is a superset; migration by re-encrypting)
- **Cross-references**: [secret-service.md](secret-service.md), [storage.md](storage.md)

### OQ-SVC-04: Should workers cache derived keys locally?
- **Origin**: [secret-service.md](secret-service.md)
- **Status**: ~~resolved~~
- **Priority**: low —
- **Resolution**: Yes, with a TTL (default: 1 hour). The head can revoke by invalidating the session.
- **Cross-references**: [secret-service.md](secret-service.md)

### OQ-SVC-05: How does the NFT-based ACL smart contract interact with the secret service?
- **Origin**: [storage.md](storage.md)
- **Status**: open
- **Priority**: low
- **Resolution**: The Ethereum signing key (`m/44'/60'/0'/0/0`) is derived from the same seed as the secret service. The smart contract is a separate concern — it reads on-chain ACL state, it doesn't call the secret service.
- **Cross-references**: [storage.md](storage.md), [secret-service.md](secret-service.md)

## Interface

### OQ-IF-01: How does the Interface session type relate to the call protocol's EventEnvelope stream?
- **Origin**: [interface.md](interface.md)
- **Status**: ~~resolved~~
- **Priority**: ~~high~~ —
- **Resolution**: `InterfaceSession::recv()` returns `Option<InterfaceEvent>` where `InterfaceEvent` carries `EventEnvelope` + `Identity`. `InterfaceSession::send()` accepts `EventEnvelope`. The `SshSession` bridge implements this over the `alknet-control:0` channel. For `MessageInterface`, `InterfaceRequest`/`InterfaceResponse` normalize request/response pairs. See [interface.md](interface.md) and ADR-035.
- **Cross-references**: [ADR-035](decisions/035-streaminterface-messageinterface-split.md), [interface.md](interface.md)

### OQ-IF-02: Should SshInterface own ForwardingPolicy checks or should they move to Layer 3?
- **Origin**: [interface.md](interface.md)
- **Status**: ~~resolved~~
- **Priority**: ~~medium~~ —
- **Resolution**: ForwardingPolicy is Layer 3 (it's policy, not session mechanics). Channel open/close lifecycle is Layer 2. The Interface reports channel open requests to Layer 3; Layer 3 applies ForwardingPolicy. The current `SshHandler` implementation checks policy in `channel_open_direct_tcpip`, which already delegates to `Identity.scopes` from the authenticated identity — this is consistent with the resolution.
- **Cross-references**: [ADR-031](decisions/031-forwarding-policy.md), [interface.md](interface.md)

### OQ-P2-01: Should MessageInterface and StreamInterface share a common trait?
- **Origin**: [research/phase2/interface-model.md](../research/phase2/interface-model.md)
- **Status**: resolved
- **Priority**: medium
- **Resolution**: Independent traits. Different signatures (`handle_request` vs `accept` + session lifecycle), different transport ownership (self-managed vs provided), different lifecycles (stateless per-request vs long-lived session). A common super-trait adds complexity without benefit. See ADR-035.
- **Cross-references**: [ADR-035](decisions/035-streaminterface-messageinterface-split.md), [interface.md](interface.md)

### OQ-P2-02: Should the HTTP interface share a port with the SSH listener?
- **Origin**: [research/phase2/interface-model.md](../research/phase2/interface-model.md)
- **Status**: resolved
- **Priority**: low
- **Resolution**: Start with separate ports. Stealth mode byte-peek on a shared port is already implemented for SSH vs HTTP detection. `ListenerConfig::Http { stealth: true }` enables the existing peek pattern. ALPN multiplexing on port 443 is a future optimization that doesn't change the interface abstraction.
- **Cross-references**: [interface.md](interface.md), [research/phase2/tls-transport.md](../research/phase2/tls-transport.md)

### OQ-P2-03: Should the HTTP interface auto-generate OpenAPI specs from OperationRegistry?
- **Origin**: [research/phase2/interface-model.md](../research/phase2/interface-model.md)
- **Status**: resolved
- **Priority**: low
- **Resolution**: Yes, but Phase 5+. The HTTP interface needs to exist first (Phase 5.3 in the integration plan). `GET /v1/schema` producing an OpenAPI spec from registered `OperationSpec`s is the natural end state. This creates symmetry with `FromOpenAPI` (inbound spec consumption).
- **Cross-references**: [call-protocol.md](call-protocol.md), [interface.md](interface.md)

### OQ-P2-04: How do self-hosted services authenticate via alknet?
- **Origin**: [research/phase2/credential-provider.md](../research/phase2/credential-provider.md), [research/phase2/definitions.md](../research/phase2/definitions.md)
- **Status**: resolved
- **Priority**: medium
- **Resolution**: Three-phase approach. Phase A: shared secret (`CredentialSet::Bearer` or `S3AccessKey`). Phase C: identity-bound credentials via `ManagedCredentialProvider`. Phase D: alknet as OIDC provider. The `CredentialProvider` trait in core enables Phase A immediately; Phases C and D are additive.
- **Cross-references**: [ADR-036](decisions/036-credentialprovider-core-type.md), [credentials.md](credentials.md)

## Credentials

### OQ-CP-01: Should CredentialProvider support per-identity credentials?
- **Origin**: [credentials.md](credentials.md)
- **Status**: open
- **Priority**: low
- **Resolution**: Start with service-level credentials (`get_credentials(service)`). Add identity-level resolution (`get_credentials_for(service, identity_id)`) when the need is concrete. `Identity.id` already serves as the account UUID in database-backed mode.
- **Cross-references**: [credentials.md](credentials.md), [ADR-036](decisions/036-credentialprovider-core-type.md)

### OQ-CP-02: Where should OIDC provider operations live?
- **Origin**: [credentials.md](credentials.md)
- **Status**: open
- **Priority**: low
- **Resolution**: Application service (Phase D). OIDC is an application concern, not a core concern. The call protocol and OperationRegistry provide the transport; OIDC is just another set of operations.
- **Cross-references**: [credentials.md](credentials.md)

### OQ-CP-03: How do credential rotations propagate across a cluster?
- **Origin**: [credentials.md](credentials.md)
- **Status**: open
- **Priority**: low
- **Resolution**: TBD. Likely TTL-based caching with a refresh threshold. Workers call `CredentialProvider::get_credentials()` which checks `is_expired()` and calls `refresh_credentials()` if needed.
- **Cross-references**: [credentials.md](credentials.md)

### OQ-CP-04: Should CredentialSet include request-signing capability?
- **Origin**: [credentials.md](credentials.md)
- **Status**: resolved
- **Priority**: low
- **Resolution**: No. `CredentialSet` is pure data. Request signing (e.g., AWS Signature V4) is a separate utility function in the service wrapper or a shared `alknet-s3` crate. Credentials are data; signing is protocol behavior.
- **Cross-references**: [credentials.md](credentials.md)

## Definitions

### OQ-DEF-01: Should alknet adopt a "Service Catalog" concept like Keystone?
- **Origin**: [research/phase2/definitions.md](../research/phase2/definitions.md)
- **Status**: resolved
- **Priority**: low
- **Resolution**: Keep `OperationRegistry` global, check scope at invocation time. Add scope-filtered discovery (`GET /v1/schema?scope=...`) when multi-tenant deployment requires it. The unfiltered registry is sufficient for current needs.
- **Cross-references**: [call-protocol.md](call-protocol.md)

### OQ-DEF-03: Should Identity.scopes be hierarchical or stay flat?
- **Origin**: [research/phase2/definitions.md](../research/phase2/definitions.md)
- **Status**: resolved
- **Priority**: low
- **Resolution**: Stay flat. Add implied scope resolution in alknet-storage when multi-tenant deployment requires it. A full policy language (like Rustfs IAM JSON policies) is Phase D territory.
- **Cross-references**: [identity.md](identity.md)

### OQ-DEF-08: Should "credential presentation" replace "auth interface" in terminology?
- **Origin**: [research/phase2/definitions.md](../research/phase2/definitions.md)
- **Status**: resolved
- **Priority**: medium
- **Resolution**: Yes. Adopted in [definitions.md](definitions.md). Use "credential presentation" for the mechanism of presenting credentials on a (Transport, Interface) pair. Never use "auth interface" (overloads "Interface").
- **Cross-references**: [definitions.md](definitions.md), [auth.md](auth.md)