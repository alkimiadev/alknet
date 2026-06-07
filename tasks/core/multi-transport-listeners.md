---
id: core/multi-transport-listeners
name: Implement multi-transport listeners with Vec<ListenerConfig>
status: completed
depends_on:
  - core/config-static-dynamic-split
scope: moderate
risk: medium
impact: component
level: implementation
---

## Description

Change `ServeTransportMode` from a single enum to `Vec<ListenerConfig>`, allowing a server to accept connections on multiple transports simultaneously. Per configuration.md and ADR-026.

Currently, `Server::run()` accepts a single transport mode. After this change, `Server::run()` spawns one accept loop per listener, sharing `DynamicConfig`, `ConnectionRateLimiter`, sessions, and shutdown signal.

**Key changes**:
- `ListenerConfig` struct: `{ transport_kind: TransportKind, listen_addr: SocketAddr, ... per-transport config }`
- `ServeOptions` gains a `listeners()` method (builder) that accepts `Vec<ListenerConfig>`
- Backwards compatibility: `ServeOptions.transport_mode()` still works (creates a single-element listeners vec)
- `Server::run()` iterates over listeners, spawning one accept loop per transport
- `TransportKind` enum gains `Dns` and `WebTransport` variants (initially tags only, no acceptor implementation)
- `DynamicConfig` and `IdentityProvider` are Arc'd and shared across all listeners

**What stays the same**: Single-transport usage via `ServeOptions.transport_mode()` continues to work unchanged.

## Acceptance Criteria

- [ ] `ListenerConfig` struct defined with `transport_kind`, `listen_addr`, and per-transport configuration
- [ ] `TransportKind` gains `Dns` and `WebTransport` variants (tag only, no behavior)
- [ ] `ServeOptions` has both `.transport_mode()` (single, backwards compat) and `.listeners()` (multi) builder methods
- [ ] `Server::run()` spawns one accept loop per `ListenerConfig`, sharing `DynamicConfig`, `ConnectionRateLimiter`, and `IdentityProvider`
- [ ] All listeners share the same `Arc<ArcSwap<DynamicConfig>>` and `Arc<dyn IdentityProvider>`
- [ ] Graceful shutdown terminates all listener accept loops
- [ ] TOML config file support: `[[listeners]]` array-of-tables syntax (added to `StaticConfig`)
- [ ] All existing tests pass (single-transport behavior unchanged)
- [ ] New tests: multi-transport server with TCP + TLS listeners simultaneously

## References

- docs/architecture/configuration.md — Multi-Transport Listeners, ListenerConfig
- docs/architecture/decisions/026-transport-interface-separation.md — TransportKind includes all Layer 1 types
- crates/alknet-core/src/server/serve.rs — current ServeOptions and Server::run()
- crates/alknet-core/src/server/handler.rs — current TransportKind

## Notes

> To be filled by implementation agent

## Summary

> To be filled on completion