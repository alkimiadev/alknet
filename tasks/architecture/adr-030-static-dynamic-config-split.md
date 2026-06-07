---
id: architecture/adr-030-static-dynamic-config-split
name: Write ADR-030 — Static/dynamic config split
status: pending
depends_on: []
scope: narrow
risk: low
impact: phase
level: implementation
---

## Description

Write ADR-030 establishing the split between `StaticConfig` (immutable after startup) and `DynamicConfig` (hot-reloadable at runtime) in alknet-core.

This is largely a promotion from the well-analyzed research in `docs/research/configuration.md`. The ADR records why this split matters, what goes in each config, and how reload works.

Key points:
- StaticConfig: transport mode, listen addr, TLS config, iroh config, host key, stealth mode, max auth attempts, max connections per IP — everything that requires socket/TLS renegotation to change
- DynamicConfig: auth policy (authorized keys, cert authorities), forwarding policy, rate limits — everything checked per-connection or per-channel
- ArcSwap for lock-free hot reload of DynamicConfig
- ServeOptions builder pattern is preserved; StaticConfig is constructed from ServeOptions
- TOML config file is an optional convenience input format (amends ADR-011, doesn't replace programmatic API)
- ConfigReloadHandle with `reload(DynamicConfig)` method
- NAPI exposes `reloadAuth()`, `reloadForwarding()`, `reloadAll()` on AlknetServer

## Acceptance Criteria

- [ ] `docs/architecture/decisions/030-static-dynamic-config-split.md` exists
- [ ] ADR follows established format
- [ ] Context explains the three failures: no hot reload of auth, no forwarding policy, no structured config beyond CLI flags
- [ ] Decision states: StaticConfig vs DynamicConfig split; ArcSwap for DynamicConfig; ServeOptions preserved as builder; TOML as optional convenience; ConfigService wraps reloads; amends ADR-011
- [ ] Lists what's in StaticConfig and what's in DynamicConfig
- [ ] Consequences: auth and forwarding can be reloaded without restart; config file users get TOML format; programmatic-first API preserved
- [ ] References: research/configuration.md, ADR-011

## References

- docs/research/configuration.md — full analysis, nearly spec-ready
- docs/architecture/decisions/011-no-ssh-config-programmatic-api.md — programmatic-first decision (amended, not superseded)
- docs/research/integration-plan.md — ADR 030 entry, Phase 1.1

## Notes

> To be filled by implementation agent

## Summary

> To be filled on completion