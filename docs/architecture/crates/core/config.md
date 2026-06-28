---
status: draft
last_updated: 2026-06-27
---

# Configuration

StaticConfig, DynamicConfig, ArcSwap, and ConfigReloadHandle.

## StaticConfig

Immutable configuration resolved at startup. Cannot be changed without restarting the endpoint.

```rust
pub struct StaticConfig {
    /// Bind address for the quinn endpoint (e.g., "0.0.0.0:4433").
    /// None if the quinn endpoint is not configured (iroh-only node).
    pub listen_addr: Option<SocketAddr>,

    /// TLS identity mode for the quinn endpoint.
    /// Required if listen_addr is Some.
    pub tls_identity: Option<TlsIdentity>,

    /// iroh relay URL (e.g., "https://relay.iroh.network/").
    /// None if the iroh endpoint is not configured.
    pub iroh_relay: Option<RelayUrl>,

    /// Drain timeout for graceful shutdown (default: 2 seconds).
    pub drain_timeout: Duration,
}

/// TLS identity configuration for the quinn endpoint.
pub enum TlsIdentity {
    /// X.509 certificate for domain-facing identity.
    /// Required for browser/WebTransport clients.
    X509 {
        cert: PathBuf,
        key: PathBuf,
    },

    /// RFC 7250 raw Ed25519 public key.
    /// No domain, no CA, no cert renewal. Key = identity.
    /// Same model as iroh's NodeId, but for direct QUIC connections.
    /// Uses `Ed25519SecretKey` (alknet-core-owned wrapper over
    /// `ed25519_dalek::SigningKey`) — not coupled to the `iroh` feature.
    /// Available in quinn-only builds. See ADR-027.
    RawKey(Ed25519SecretKey),

    /// Self-signed X.509 cert for development.
    /// Generated on startup, not validated by external clients.
    SelfSigned,

    /// ACME auto-provisioning via Let's Encrypt (rustls-acme).
    /// Produces X.509 certs at runtime; handles TLS-ALPN-01 challenges
    /// and automatic renewal. Feature-gated behind `acme`. See ADR-027.
    Acme {
        domains: Vec<String>,
        cache_dir: PathBuf,
        directory: AcmeDirectory,  // Production, Staging, Custom(url)
        contact: Vec<String>,      // e.g. ["mailto:admin@example.com"]
    },
}
```

### Why `TlsIdentity` instead of `tls_cert`/`tls_key` options

TLS identity in alknet has two distinct use cases, not one. The original `tls_cert: Option<PathBuf>` / `tls_key: Option<PathBuf>` assumed X.509 was the only TLS identity model. RFC 7250 raw public keys (used by iroh, supported by rustls) provide a fundamentally different mode: Ed25519 key as identity, no X.509, no CA, no domain. This is the default for most alknet nodes — it works natively with SSH auth and git. X.509 certs are for domain-hosted services and browser/WebTransport clients, which don't support RFC 7250.

The `TlsIdentity` enum captures all four modes. See OQ-12 for the use-case
rationale and [ADR-027](../../decisions/027-tls-identity-redesign-acme-rawkey-decoupling.md)
for the ACME + RawKey decoupling design.

### `Ed25519SecretKey`

A thin alknet-core-owned wrapper over `ed25519_dalek::SigningKey`. Not
feature-gated — available in all builds. Used by `TlsIdentity::RawKey`
for RFC 7250 raw public key TLS identity. When the `iroh` transport is
configured, `build_iroh_endpoint` converts to `iroh::SecretKey::from_bytes`
(see ADR-027, Decision 4).

### `AcmeDirectory`

```rust
pub enum AcmeDirectory {
    Production,   // Let's Encrypt production
    Staging,      // Let's Encrypt staging
    Custom(String), // custom ACME directory URL
}
```

### Construction examples (updated)

```rust
// P2P / key-based identity (default for most nodes) — no iroh dep needed
let p2p_config = StaticConfig {
    listen_addr: Some("0.0.0.0:4433".parse()?),
    tls_identity: Some(TlsIdentity::RawKey(Ed25519SecretKey::generate())),
    iroh_relay: None,
    drain_timeout: Duration::from_secs(2),
};

// Domain-hosted service with ACME auto-provisioning
let acme_config = StaticConfig {
    listen_addr: Some("0.0.0.0:443".parse()?),
    tls_identity: Some(TlsIdentity::Acme {
        domains: vec!["relay.alk.dev".to_string()],
        cache_dir: "/var/lib/alknet/acme".into(),
        directory: AcmeDirectory::Production,
        contact: vec!["mailto:admin@alk.dev".to_string()],
    }),
    iroh_relay: None,
    drain_timeout: Duration::from_secs(2),
};
```

### Key differences from reference implementation

The reference `StaticConfig` (in `alknet-main/crates/alknet-core/src/config/static_config.rs`) is SSH-centric: it holds `host_key`, `host_key_algorithm`, `proxy_config`, `stealth`, `transport_mode`, and `listeners`. The new model removes all of these:

- **No `host_key`/`host_key_algorithm`**: SSH host keys are managed by the SSH handler, not by core config. The endpoint uses TLS certs, not SSH host keys.
- **No `proxy_config`**: Outbound proxy is an SSH-specific concern (SOCKS5/HTTP CONNECT forwarding). Not in core config.
- **No `stealth`**: ALPN eliminates the need for stealth/byte-peeking. See [ADR-001](../../decisions/001-alpn-protocol-dispatch.md).
- **No `transport_mode`/`listeners`**: The old `ServeTransportMode` and `ListenerConfig` enum are replaced by `listen_addr` (quinn) and `iroh_relay` (iroh). Both are optional — a node can use either or both. See [ADR-010](../../decisions/010-alpn-router-and-endpoint.md).

### Construction

`StaticConfig` is constructed by the CLI binary from CLI arguments or a config file. The exact shape of `StartupOptions` (or whatever the CLI uses) is a CLI concern, not a core concern. alknet-core provides `StaticConfig` as a data structure; the CLI is responsible for populating it.

```rust
// The CLI binary constructs StaticConfig from its own options/config.
// StartupOptions is NOT a core type — it belongs to the alknet CLI binary.
// alknet-core receives a fully populated StaticConfig.

// P2P / key-based identity (default for most nodes)
let p2p_config = StaticConfig {
    listen_addr: Some("0.0.0.0:4433".parse()?),
    tls_identity: Some(TlsIdentity::RawKey(Ed25519SecretKey::generate())),
    iroh_relay: None,
    drain_timeout: Duration::from_secs(2),
};

// Domain-hosted service (relays, public services, browsers) — manual certs
let domain_config = StaticConfig {
    listen_addr: Some("0.0.0.0:4433".parse()?),
    tls_identity: Some(TlsIdentity::X509 {
        cert: "/path/to/cert.pem".into(),
        key: "/path/to/key.pem".into(),
    }),
    iroh_relay: None,
    drain_timeout: Duration::from_secs(2),
};
```

## DynamicConfig

Runtime-reloadable configuration. Hot-reloaded via `ArcSwap` without restarting the endpoint.

```rust
#[derive(Debug, Clone)]
pub struct DynamicConfig {
    pub auth: AuthPolicy,
    pub rate_limits: RateLimitConfig,
}
```

### AuthPolicy

Authorization policy derived from peer entries and API keys.

```rust
pub struct AuthPolicy {
    /// Peer entries: each maps a stable logical peer_id to its current
    /// fingerprint, scopes, resources, and enabled state. Replaces the
    /// pre-ADR-030 `authorized_fingerprints: HashSet<String>`. The list
    /// is keyed by `peer_id`; resolution looks up by `fingerprint`.
    /// See ADR-030.
    pub peers: Vec<PeerEntry>,

    /// API keys for token-based auth. Unchanged by ADR-030 — API keys
    /// don't get the PeerEntry treatment (rotation = new identity is the
    /// correct semantics for bearer tokens). See ADR-030 §"API keys".
    pub api_keys: Vec<ApiKeyEntry>,
}
```

### PeerEntry

A peer entry maps a stable logical peer identity to its current
cryptographic material and authorization scopes. The `peer_id` is stable
across key rotation; the `fingerprint` changes when the node rotates its
TLS key. `ConfigIdentityProvider::resolve_from_fingerprint` resolves
fingerprint → `PeerEntry` → `Identity { id: peer_id, ... }`, so
`Identity.id` is the stable `peer_id`, not the rotating fingerprint.

```rust
pub struct PeerEntry {
    /// Stable logical peer id ("worker-a", "alice"). Does NOT change on
    /// key rotation. This becomes Identity.id on resolution, regardless of
    /// which credential path resolved the identity.
    pub peer_id: String,

    /// TLS fingerprints for this peer — one or more. A peer may have
    /// multiple keys (e.g., an Ed25519 raw key for P2P and an X.509 cert
    /// for domain-facing). Resolution matches against any entry.
    /// Format: "ed25519:<hex of 32-byte pub key>" for RFC 7250 raw keys
    /// (normalized across quinn and iroh — ADR-030 §6), "SHA256:<hex>" for
    /// X.509 certs (DER hash). Changes on key rotation.
    pub fingerprints: Vec<String>,

    /// Optional: bearer-token authentication for this peer. A peer that
    /// also authenticates via auth_token (e.g., HTTP clients that can't
    /// do TLS client-auth) stores the SHA-256 hash of the token here.
    /// Resolution via resolve_from_token matches this field and returns
    /// the same Identity { id: peer_id, ... } as the fingerprint path.
    pub auth_token_hash: Option<String>,

    /// Authorization scopes granted to this peer. Resolved into
    /// Identity.scopes.
    pub scopes: Vec<String>,

    /// Named resource lists granted to this peer. Resolved into
    /// Identity.resources.
    pub resources: HashMap<String, Vec<String>>,

    /// Human-readable display name for logs / UIs. Optional.
    pub display_name: Option<String>,

    /// Whether this peer is authorized at all. false = recognized but
    /// disabled (revoked). Resolution returns None.
    pub enabled: bool,
}
```

See [ADR-030](../../decisions/030-peerentry-and-identity-id-decoupling.md)
for the `PeerEntry` model, the multi-credential resolution path, the
fingerprint normalization rationale, and the key-rotation story (vault
rotates locally; the remote side updates the `PeerEntry.fingerprints` or
`auth_token_hash` field; the `peer_id` and all ACL / routing references
stay stable).

Certificate authority entries for cert-based auth are omitted from
`AuthPolicy` until alknet-ssh is implemented, to avoid referencing an
undefined type. Adding the `cert_authorities` field is additive (a new
field on `AuthPolicy` is non-breaking for existing config files that don't
use it). alknet-ssh will define `CertAuthorityEntry` with the necessary
fields (public key, principals, options).

This replaces the reference implementation's `AuthPolicy` which depended on `russh::keys::PublicKey`. The new version stores fingerprints as strings (in `PeerEntry.fingerprint`), not russh types. This removes the russh dependency from alknet-core.

### ApiKeyEntry

```rust
pub struct ApiKeyEntry {
    /// Key prefix (first 8 chars of the key). Used for O(1) lookup.
    pub prefix: String,

    /// SHA-256 hash of the full key. Used for verification.
    pub hash: String,

    /// Authorization scopes granted by this key.
    pub scopes: Vec<String>,

    /// Human-readable description.
    pub description: String,

    /// Unix timestamp when the key expires. None = never expires.
    pub expires_at: Option<u64>,
}
```

Carries forward from the reference implementation with no changes.

### RateLimitConfig

```rust
pub struct RateLimitConfig {
    pub max_connections_per_ip: usize,
    pub max_auth_attempts: usize,
}
```

Carries forward from the reference implementation. Rate limits are entirely dynamic — `StaticConfig` does not contain rate limit fields. The CLI binary sets initial `RateLimitConfig` values when constructing the initial `DynamicConfig`. Hot-reloading via `ConfigReloadHandle` replaces rate limits immediately without restart.

## ArcSwap Pattern

`DynamicConfig` is wrapped in `Arc<ArcSwap<DynamicConfig>>` for lock-free reads and atomic swaps.

```rust
let dynamic = Arc::new(ArcSwap::new(Arc::new(DynamicConfig::default())));
```

- **Reads**: `dynamic.load()` returns `Arc<DynamicConfig>`. Multiple readers can hold references simultaneously without blocking.
- **Writes**: `dynamic.store(Arc::new(new_config))` atomically replaces the config. All subsequent reads see the new config.
- **No locks**: `ArcSwap` uses atomic operations. No reader is ever blocked by a writer.

This pattern carries forward directly from the reference implementation (`alknet-main/crates/alknet-core/src/config/dynamic_config.rs`).

## ConfigReloadHandle

```rust
pub struct ConfigReloadHandle {
    dynamic: Arc<ArcSwap<DynamicConfig>>,
}

impl ConfigReloadHandle {
    pub fn reload(&self, new_config: DynamicConfig);
    pub fn dynamic(&self) -> Arc<DynamicConfig>;
}
```

- `reload()`: Atomically replaces the dynamic config. All subsequent reads (including in-flight `IdentityProvider` calls) see the new config.
- `dynamic()`: Returns the current config as `Arc<DynamicConfig>`.

The CLI binary creates a `ConfigReloadHandle` and passes it to a config watcher (file watcher, SIGHUP handler, or call protocol operation) that calls `reload()` when config changes are detected.

**Config reload is a privilege-escalation path.** `ConfigIdentityProvider` reads from `ArcSwap<DynamicConfig>`, so a reload that adds an authorized fingerprint or API key grants access immediately. A malicious reload is equivalent to root-level privilege grant. The reload trigger **must be authenticated/local-only**: SIGHUP (local signal), local file watch, or an admin call protocol operation with the same auth treatment as any other mutation (requires `admin` scope, ADR-015). The implementation must not ship a reload endpoint with no auth "for convenience."

## ConfigError

```rust
pub enum ConfigError {
    InvalidFlag { name: String },
    KeyFileNotFound { path: String },
    BindFailed(io::Error),
    TlsConfig(io::Error),
    IncompatibleOptions,
}
```

Simplified from the reference implementation. Removes proxy-specific errors (now an SSH concern) and listener validation errors (no more `ListenerConfig` enum).

## Key Differences from Reference Implementation

| Aspect | Reference | New Model |
|--------|-----------|-----------|
| StaticConfig fields | SSH host key, stealth, transport_mode, listeners, proxy | listen_addr, TLS cert/key, drain_timeout |
| DynamicConfig.auth | `HashSet<PublicKey>` (russh types) | `HashSet<String>` (fingerprint strings) |
| ListenerConfig | Enum with Stream/Http/Dns variants | Eliminated — single endpoint, ALPN dispatch |
| TransportMode | Tcp/Tls/Iroh | Eliminated — always QUIC+TLS |
| Stealth mode | Byte-peeking HTTP/SSH detection | Eliminated — ALPN handles protocol detection |
| ForwardingPolicy | In DynamicConfig | Moved to handler-specific config (SSH) |

## Design Decisions

| Decision | ADR | Summary |
|----------|-----|---------|
| No russh dependency in core | [ADR-003](../../decisions/003-crate-decomposition.md) | Core is ALPN-agnostic; russh is an alknet-ssh dependency |
| ArcSwap for dynamic config | Carry-forward from reference | Lock-free reads, atomic swaps |
| No ListenerConfig | [ADR-001](../../decisions/001-alpn-protocol-dispatch.md) | Single endpoint, ALPN replaces multiple listener types |
| PeerEntry and Identity.id decoupling | [ADR-030](../../decisions/030-peerentry-and-identity-id-decoupling.md) | `authorized_fingerprints: HashSet<String>` → `peers: Vec<PeerEntry>`; `Identity.id` = `peer_id` (stable), not fingerprint |
| Storage boundary and repo/adapter pattern | [ADR-033](../../decisions/033-storage-boundary-and-repo-adapter-pattern.md) | Core defines repo traits + in-memory defaults; `AuthPolicy.peers` is the config model for the in-memory `ConfigIdentityProvider` adapter; persistence adapters are separate crates |