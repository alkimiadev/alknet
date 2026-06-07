---
id: core/config-service-irpc
name: Implement ConfigProtocol irpc service and ConfigServiceImpl
status: completed
depends_on:
  - core/config-static-dynamic-split
scope: narrow
risk: low
impact: component
level: implementation
---

## Description

Define `ConfigProtocol` irpc service enum and `ConfigServiceImpl` behind the `irpc` feature flag, per ADR-030 and configuration.md.

`ConfigServiceImpl` wraps `ArcSwap<DynamicConfig>` and provides access to forwarding policy, rate limits, and reload capability. In Phase 1, direct `ConfigReloadHandle::reload()` is sufficient for minimal deployments. The irpc service provides the same functionality for production deployments.

**Key additions**:
- `ConfigServiceImpl` struct with `forwarding_policy()`, `rate_limits()`, `reload()` methods (always available, not feature-gated)
- `ConfigProtocol` irpc enum behind `irpc` feature: `GetForwardingPolicy`, `GetRateLimits`, `ReloadForwarding`, `ReloadRateLimits`

**What stays the same**: Direct `ConfigReloadHandle::reload()` is the primary API. `ConfigServiceImpl` is a thin wrapper over ArcSwap, also always available. The irpc service is additive.

## Acceptance Criteria

- [ ] `ConfigServiceImpl` struct defined in `crates/alknet-core/src/config/config_service.rs` with methods per configuration.md
- [ ] `ConfigServiceImpl` reads from `ArcSwap<DynamicConfig>` and returns `Arc<ForwardingPolicy>`, `Arc<RateLimitConfig>`
- [ ] `ConfigProtocol` enum defined behind `irpc` feature flag
- [ ] Without `irpc` feature, `ConfigServiceImpl` is available for direct use
- [ ] With `irpc` feature, `ConfigProtocol` wraps `ConfigServiceImpl`

## References

- docs/architecture/configuration.md — ConfigServiceImpl, ConfigProtocol
- docs/architecture/decisions/030-static-dynamic-config-split.md — ADR-030

## Notes

> To be filled by implementation agent

## Summary

> To be filled on completion