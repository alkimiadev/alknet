---
id: core/napi-reload-api
name: Add NAPI reload API for DynamicConfig and ForwardingPolicy
status: pending
depends_on:
  - core/config-identity-provider-into-handler
  - core/multi-transport-listeners
scope: narrow
risk: low
impact: component
level: implementation
---

## Description

Expose `reloadAuth()`, `reloadForwarding()`, and `reloadAll()` on the NAPI `AlknetServer` object, per configuration.md and napi-and-pubsub.md updates.

**Key changes**:
- `AlknetServer` NAPI object holds a `ConfigReloadHandle` (from task 1.5)
- `reloadAuth(config)`: creates new `AuthPolicy` and atomically swaps it via ArcSwap
- `reloadForwarding(config)`: creates new `ForwardingPolicy` and atomically swaps it
- `reloadAll(config)`: swaps the entire `DynamicConfig`
- Call protocol integration: expose operation registry for NAPI consumers to register handlers (depends on operation-context-registry)

## Acceptance Criteria

- [ ] `AlknetServer` NAPI object has `reloadAuth()`, `reloadForwarding()`, `reloadAll()` methods
- [ ] `reloadAuth()` creates new `AuthPolicy` from provided config and swaps via `ConfigReloadHandle`
- [ ] `reloadForwarding()` creates new `ForwardingPolicy` and swaps via `ConfigReloadHandle`
- [ ] `reloadAll()` swaps the entire `DynamicConfig`
- [ ] All swaps are atomic via ArcSwap — existing connections continue, new connections get new config
- [ ] NAPI type definitions for `ForwardingPolicyConfig` and auth config types
- [ ] Existing NAPI tests pass

## References

- docs/architecture/configuration.md — NAPI Reload API
- docs/architecture/napi-and-pubsub.md — NAPI layer additions

## Notes

> To be filled by implementation agent

## Summary

> To be filled on completion