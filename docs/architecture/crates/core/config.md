---
status: draft
last_updated: 2026-06-16
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
    RawKey(SecretKey),

    /// Self-signed X.509 cert for development.
    /// Generated on startup, not validated by external clients.
    SelfSigned,
}
```

### Why `TlsIdentity` instead of `tls_cert`/`tls_key` options

The original `tls_cert: Option<PathBuf>` / `tls_key: Option<PathBuf>` assumed X.509 was the only TLS identity model. RFC 7250 raw public keys (used by iroh, supported by rustls) provide an alternative: Ed25519 key as identity, no X.509, no CA, no domain. This is a separate mode, not just "no cert."

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
let static_config = StaticConfig {
    listen_addr: "0.0.0.0:4433".parse()?,
    tls_cert: Some("/path/to/cert.pem".into()),
    tls_key: Some("/path/to/key.pem".into()),
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

Authorization policy derived from authorized keys, certificate authorities, and API keys.

```rust
pub struct AuthPolicy {
    /// SHA-256 fingerprints of authorized keys (SSH keys, TLS client certs).
    /// Stored as strings to avoid russh dependency in core.
    pub authorized_fingerprints: HashSet<String>,

    /// Certificate authorities for certificate-based auth.
    /// The exact structure is TBD — it will be defined when alknet-ssh
    /// is implemented. For now, this is a placeholder that reserves
    /// the field. alknet-ssh will define `CertAuthorityEntry` with
    /// the necessary fields (public key, principals, options).
    pub cert_authorities: Vec<CertAuthorityEntry>,

    /// API keys for token-based auth.
    pub api_keys: Vec<ApiKeyEntry>,
}
```

`CertAuthorityEntry` is a placeholder type. Its fields will be defined when alknet-ssh is implemented and the certificate authority validation requirements are clear. For v1, `cert_authorities` will be an empty vector.

This replaces the reference implementation's `AuthPolicy` which depended on `russh::keys::PublicKey`. The new version stores fingerprints as strings, not russh types. This removes the russh dependency from alknet-core.

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

Carries forward from the reference implementation. Note: `max_connections_per_ip` and `max_auth_attempts` appear in both `StaticConfig` and `RateLimitConfig`. The relationship is:

- `StaticConfig` does NOT contain rate limit fields. Rate limits are entirely dynamic.
- `RateLimitConfig` in `DynamicConfig` is the authoritative source at runtime.
- The CLI binary sets initial `RateLimitConfig` values when creating the initial `DynamicConfig`.
- Hot-reloading `DynamicConfig` via `ConfigReloadHandle` replaces rate limits immediately — no restart needed.

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
| StaticConfig fields | SSH host key, stealth, transport_mode, listeners, proxy | listen_addr, TLS cert/key, drain_timeout, rate limits |
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