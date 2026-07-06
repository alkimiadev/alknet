# OQ-12: TLS Identity Provisioning in AlknetEndpoint

- **Origin**: [endpoint.md](crates/core/endpoint.md), [config.md](crates/core/config.md)
- **Status**: resolved
- **Door type**: One-way
- **Priority**: high
- **Resolution**: TLS identity in alknet has two distinct use cases, not one:

  **Use case 1 — P2P / key-based identity (default for most alknet nodes):** RFC 7250 raw Ed25519 public keys. No domain, no CA, no cert renewal. The Ed25519 public key IS the node's identity. This is the same model iroh uses with its `NodeId`. It works natively with SSH auth (same key type) and git (SSH key-based auth). `TlsIdentity::RawKey(Ed25519SecretKey)` in `StaticConfig` covers this. As of [ADR-027](decisions/027-tls-identity-redesign-acme-rawkey-decoupling.md), `RawKey` uses `ed25519_dalek::SigningKey` (via an alknet-core wrapper), **not** `iroh::SecretKey` — so raw-key TLS identity is available in quinn-only builds without the `iroh` feature.

  **Use case 2 — Domain-hosted services (relays, public-facing nodes):** X.509 certificates with domain names. Required for browser/WebTransport clients, which don't support RFC 7250. This has two sub-cases:
  - **Manual**: Provide cert/key file paths via `TlsIdentity::X509`. Already specified in `StaticConfig`.
  - **ACME auto-provisioning**: Let's Encrypt via `rustls-acme`. `TlsIdentity::Acme { domains, cache_dir, directory, contact }` carries static config; the endpoint constructs the `AcmeState` async state machine at setup time. Feature-gated behind `acme`. Designed in [ADR-027](decisions/027-tls-identity-redesign-acme-rawkey-decoupling.md). The reverse-proxy project (`/workspace/@alkdev/reverse-proxy`) demonstrates the proven pattern: `AcmeConfig`, `ResolvesServerCertAcme`, TLS-ALPN-01 challenge handling, automatic renewal.

  **Browser constraint**: Browsers require X.509 and don't support RFC 7250. For browser/WebTransport clients, domain-hosted nodes with X.509 certs are mandatory. All other clients (SSH, git, alknet-native) work with raw keys by default.

  The `TlsIdentity` enum in `StaticConfig` captures all four modes (`X509`, `RawKey`, `SelfSigned`, `Acme`). [ADR-027](decisions/027-tls-identity-redesign-acme-rawkey-decoupling.md) records the design decisions for ACME integration and RawKey decoupling.
- **Cross-references**: ADR-010, ADR-027, [config.md](crates/core/config.md), [endpoint.md](crates/core/endpoint.md)
