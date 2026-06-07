---
id: core/config-static-dynamic-split
name: Implement StaticConfig / DynamicConfig split with ArcSwap hot-reload
status: completed
depends_on: []
scope: moderate
risk: medium
impact: component
level: implementation
---

## Description

Split alknet-core's configuration into `StaticConfig` (immutable after startup) and `DynamicConfig` (hot-reloadable at runtime via `ArcSwap`). This is the foundational change for Phase 1 — ForwardingPolicy, IdentityProvider, and ConfigService all depend on `DynamicConfig` being ArcSwap-backed.

Per ADR-030 and configuration.md:

**StaticConfig** (constructed from `ServeOptions`, never changes):
- Transport mode, listen address
- TLS config (cert, key)
- iroh config (relay URL)
- Stealth mode flag
- Host key, host key algorithm
- Max auth attempts, max connections per IP
- Proxy config

**DynamicConfig** (ArcSwap-wrapped, hot-reloadable):
- `AuthPolicy` — authorized keys, certificate authorities (replaces current `Arc<ServerAuthConfig>`)
- `ForwardingPolicy` — allow/deny rules for channel targets (new, Phase 1.3)
- `RateLimitConfig` — rate limiting parameters

**Key changes**:
- `ServerHandler` currently holds `Arc<ServerAuthConfig>`. Replace with `Arc<ArcSwap<DynamicConfig>>` so it can reload auth policy without restarting.
- `ServeOptions` builder pattern is preserved. `ServeOptions` → `StaticConfig` at startup.
- Add `ConfigReloadHandle` with `reload(DynamicConfig)` method, obtained from `Server::run()`.
- `DynamicConfig` starts with what was in `ServerAuthConfig` and gains `ForwardingPolicy` (added in task 1.3).

**What stays the same**: All existing tests should continue to pass. The default behavior is unchanged — `DynamicConfig::default()` should produce `ForwardingPolicy::allow_all()` and equivalent auth to the current `ServerAuthConfig`.

## Acceptance Criteria

- [ ] `StaticConfig` struct defined in `crates/alknet-core/src/config/static_config.rs` with all fields per ADR-030
- [ ] `DynamicConfig` struct defined in `crates/alknet-core/src/config/dynamic_config.rs` with `AuthPolicy` and `ForwardingPolicy` (initially `ForwardingPolicy::allow_all()` — detailed rules in task 1.3)
- [ ] `ArcSwap<DynamicConfig>` used in `ServerHandler` instead of `Arc<ServerAuthConfig>`
- [ ] `ConfigReloadHandle` struct with `reload(&self, DynamicConfig)` method, obtained from `Server::run()`
- [ ] `ServeOptions::build()` (or similar) produces `(StaticConfig, DynamicConfig)` from builder
- [ ] All existing server/auth tests pass with the new config structure
- [ ] `AuthPolicy` struct defined with `authorized_keys` and `cert_authorities` fields (migrated from `ServerAuthConfig`)
- [ ] `RateLimitConfig` struct defined with rate limiting parameters
- [ ] No behavioral changes — default config produces identical behavior to current code

## References

- docs/architecture/configuration.md — StaticConfig, DynamicConfig, ConfigReloadHandle
- docs/architecture/decisions/030-static-dynamic-config-split.md — ADR-030
- docs/architecture/decisions/031-forwarding-policy.md — ForwardingPolicy in DynamicConfig
- docs/architecture/identity.md — DynamicConfig.auth consumed by IdentityProvider
- crates/alknet-core/src/server/handler.rs — current ServerHandler with Arc<ServerAuthConfig>
- crates/alknet-core/src/server/serve.rs — current ServeOptions and Server

## Notes

> To be filled by implementation agent

## Summary

> To be filled on completion