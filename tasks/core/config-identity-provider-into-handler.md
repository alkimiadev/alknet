---
id: core/config-identity-provider-into-handler
name: Wire IdentityProvider and ForwardingPolicy into ServerHandler
status: completed
depends_on:
  - core/forwarding-policy
scope: narrow
risk: low
impact: component
level: implementation
---

## Description

Wire the `IdentityProvider` and `ForwardingPolicy` into `ServerHandler` and the server accept loop. This is the integration task that connects the config split, identity trait, and forwarding policy to the actual runtime behavior.

**Key changes**:
- `Server::run()` (or `serve()`) constructs `ConfigIdentityProvider` from `ArcSwap<DynamicConfig>` and passes it to `ServerHandler`
- `ServerHandler` holds `Arc<dyn IdentityProvider>` instead of `Arc<ServerAuthConfig>`
- `auth_publickey()` calls `identity_provider.resolve_from_fingerprint()` and stores the resulting `Identity` on the session
- `channel_open_direct_tcpip()` evaluates `ForwardingPolicy::check()` using the session's `Identity`
- `ConfigReloadHandle` is threaded through from `Server::run()` so callers can reload `DynamicConfig`
- The `ServerHandler::new()` API takes `IdentityProvider` + `DynamicConfig` instead of `ServerAuthConfig`

**This is a wiring/integration task** — the pieces exist from tasks 1.1-1.3, this connects them.

## Acceptance Criteria

- [ ] `ServerHandler` holds `Arc<dyn IdentityProvider>` and `Arc<ArcSwap<DynamicConfig>>` instead of `Arc<ServerAuthConfig>`
- [ ] `auth_publickey()` delegates to `IdentityProvider::resolve_from_fingerprint()` and stores `Identity` on the session
- [ ] `channel_open_direct_tcpip()` evaluates `ForwardingPolicy::check()` before proxying; logs rejection with principal and target
- [ ] `ServeOptions` produces `(StaticConfig, DynamicConfig)` at startup
- [ ] `ConfigReloadHandle` returned from `Server::run()` for external reload
- [ ] `ConfigIdentityProvider` constructed at startup from initial `DynamicConfig`
- [ ] All existing integration tests pass
- [ ] New integration test: reload DynamicConfig → new auth keys take effect on next connection
- [ ] New integration test: ForwardingPolicy deny rule blocks channel open

## References

- docs/architecture/identity.md — IdentityProvider wiring into ServerHandler
- docs/architecture/configuration.md — ConfigReloadHandle, ConfigIdentityProvider
- crates/alknet-core/src/server/handler.rs — current handler to be refactored
- crates/alknet-core/src/server/serve.rs — ServeOptions and Server::run()

## Notes

> To be filled by implementation agent

## Summary

> To be filled on completion