---
id: core/config
name: Implement StaticConfig, DynamicConfig, AuthPolicy, ApiKeyEntry, ConfigReloadHandle, TlsIdentity
status: completed
depends_on: [core/core-types]
scope: moderate
risk: low
impact: component
level: implementation
---

## Description

Implement the configuration types in `src/config.rs`. These are the config
structures consumed by the endpoint and the CLI binary. StaticConfig is
immutable at startup; DynamicConfig is hot-reloadable via ArcSwap.

### StaticConfig

```rust
pub struct StaticConfig {
    pub listen_addr: Option<SocketAddr>,
    pub tls_identity: Option<TlsIdentity>,
    pub iroh_relay: Option<RelayUrl>,
    pub drain_timeout: Duration,
}
```

Immutable configuration resolved at startup. `listen_addr` is None for
iroh-only nodes. `tls_identity` is required if `listen_addr` is Some.

### TlsIdentity

```rust
pub enum TlsIdentity {
    X509 { cert: PathBuf, key: PathBuf },
    RawKey(iroh::SecretKey),
    SelfSigned,
}
```

Three modes (OQ-12):
- `X509`: domain certificate for browser/WebTransport clients
- `RawKey`: RFC 7250 raw Ed25519 public key — default for P2P, no domain/CA
- `SelfSigned`: development only

`RawKey` uses `iroh::SecretKey` (Ed25519) — re-exported from iroh, which
alknet-core depends on (feature-gated). The key can be derived from
alknet-vault at the assembly layer or generated fresh.

### DynamicConfig

```rust
#[derive(Debug, Clone)]
pub struct DynamicConfig {
    pub auth: AuthPolicy,
    pub rate_limits: RateLimitConfig,
}
```

Runtime-reloadable via ArcSwap.

### AuthPolicy

```rust
pub struct AuthPolicy {
    pub authorized_fingerprints: HashSet<String>,
    pub api_keys: Vec<ApiKeyEntry>,
}
```

Fingerprints stored as strings (no russh dependency in core — ADR-003).
Certificate authority entries deferred to alknet-ssh (omitted from v1 to avoid
referencing an undefined type; adding back is additive).

### ApiKeyEntry

```rust
pub struct ApiKeyEntry {
    pub prefix: String,
    pub hash: String,
    pub scopes: Vec<String>,
    pub description: String,
    pub expires_at: Option<u64>,
}
```

Carries forward from reference implementation. Prefix (first 8 chars) for O(1)
lookup, SHA-256 hash for verification.

### RateLimitConfig

```rust
pub struct RateLimitConfig {
    pub max_connections_per_ip: usize,
    pub max_auth_attempts: usize,
}
```

### ArcSwap pattern

```rust
let dynamic = Arc::new(ArcSwap::new(Arc::new(DynamicConfig::default())));
```

- Reads: `dynamic.load()` returns `Arc<DynamicConfig>` — lock-free
- Writes: `dynamic.store(Arc::new(new_config))` — atomic swap
- No locks: ArcSwap uses atomic operations

### ConfigReloadHandle

```rust
pub struct ConfigReloadHandle {
    dynamic: Arc<ArcSwap<DynamicConfig>>,
}

impl ConfigReloadHandle {
    pub fn reload(&self, new_config: DynamicConfig);
    pub fn dynamic(&self) -> Arc<DynamicConfig>;
}
```

- `reload()`: atomically replaces the dynamic config
- `dynamic()`: returns current config as `Arc<DynamicConfig>`

**Config reload is a privilege-escalation path.** A reload that adds an
authorized fingerprint or API key grants access immediately. The reload
trigger must be authenticated/local-only (SIGHUP, file watch, or admin call
protocol operation). The implementation must not ship a reload endpoint with
no auth "for convenience."

### ConfigError

```rust
pub enum ConfigError {
    InvalidFlag { name: String },
    KeyFileNotFound { path: String },
    BindFailed(io::Error),
    TlsConfig(io::Error),
    IncompatibleOptions,
}
```

### Defaults

- `drain_timeout`: 2 seconds
- `max_connections_per_ip`: implementation default (reference uses a reasonable value)
- `max_auth_attempts`: implementation default
- `DynamicConfig::default()`: empty auth policy, default rate limits

### What NOT to include

Per the spec, StaticConfig does NOT include: `host_key`, `host_key_algorithm`,
`proxy_config`, `stealth`, `transport_mode`, `listeners`. These are removed in
the new model (ALPN dispatch replaces them — see config.md Key Differences).

## Acceptance Criteria

- [ ] `StaticConfig` struct with all fields per config.md
- [ ] `TlsIdentity` enum with X509, RawKey, SelfSigned variants
- [ ] `DynamicConfig` struct with `auth` and `rate_limits` fields
- [ ] `AuthPolicy` struct with `authorized_fingerprints` and `api_keys`
- [ ] `ApiKeyEntry` struct with all 5 fields
- [ ] `RateLimitConfig` struct with both fields
- [ ] `ConfigReloadHandle` with `reload()` and `dynamic()` methods
- [ ] `ConfigError` enum with all variants
- [ ] `DynamicConfig` derives `Clone`, `Debug` (for ArcSwap)
- [ ] Default values match config.md (drain_timeout = 2s, etc.)
- [ ] No russh dependency (fingerprints as strings)
- [ ] Unit tests for Default impls
- [ ] Unit test: ConfigReloadHandle reload swaps config atomically
- [ ] `cargo test -p alknet-core` succeeds
- [ ] `cargo clippy -p alknet-core` succeeds with no warnings

## References

- docs/architecture/crates/core/config.md — all type definitions
- docs/architecture/decisions/003-crate-decomposition.md — ADR-003 (no russh in core)
- docs/architecture/decisions/010-alpn-router-and-endpoint.md — ADR-010 (no ListenerConfig)

## Notes

> Config reload is a privilege-escalation path — do not ship an unauthenticated
> reload endpoint. The ArcSwap pattern carries forward from the reference
> implementation. StaticConfig removes all SSH-centric fields (host_key,
> stealth, transport_mode, listeners) — those are handler concerns now.

## Summary

Implemented all configuration types in `config.rs`: `StaticConfig`
(`drain_timeout=2s` default), `TlsIdentity` (X509/RawKey[iroh-gated]/SelfSigned),
`DynamicConfig` (Clone/Debug/Default, ArcSwap-reloadable), `AuthPolicy` (String
fingerprints, no russh), `ApiKeyEntry` (5 fields), `RateLimitConfig` (100/5
defaults), `ConfigReloadHandle` (reload/dynamic via ArcSwap), `ConfigError`
(thiserror, all variants). `iroh_relay` and `RawKey` feature-gated to `iroh`.
Preserved `AuthPolicy::resolve_identity_from_fingerprint`/`resolve_api_key`
methods from the parallel `core/auth` task. 41 total tests pass; clippy clean on
default + iroh features. Merged to develop (resolved config.rs conflicts with
core/auth).