---
id: cleanup/napi-identity-provider-wiring
name: Fix NapiServerHandler to use IdentityProvider and ForwardingPolicy
status: completed
depends_on:
  - review/phase1-core-modifications
scope: moderate
risk: medium
impact: component
level: implementation
---

## Description

The `NapiServerHandler` in `crates/alknet-napi/src/serve.rs` bypasses the `IdentityProvider` trait contract, calling `config.auth.authenticate_publickey()` directly instead of `IdentityProvider::resolve_from_fingerprint()`. This creates two problems:

1. **No Identity stored on session**: Without resolving through `IdentityProvider`, no `Identity` struct is attached to the connection. Per-identity forwarding rules (`principals` field in `ForwardingRule`) cannot match because there's no identity to match against.

2. **No forwarding policy check**: The NAPI handler's `channel_open_direct_tcpip()` doesn't evaluate `ForwardingPolicy::check()` at all. It only handles `alknet-` prefixed channels and rejects everything else. This means NAPI-served tunnels have no forwarding access control, defeating the purpose of ADR-031.

The core `ServerHandler` and `SshHandler` both correctly use `IdentityProvider` and `ForwardingPolicy`. The NAPI handler should be consistent.

**Fix**:
- `NapiServerHandler` should hold `Arc<dyn IdentityProvider>` and use it in `auth_publickey()`
- `NapiServerHandler.auth_publickey()` should call `identity_provider.resolve_from_fingerprint()` and store the resulting `Identity`
- `NapiServerHandler.channel_open_direct_tcpip()` should evaluate `ForwardingPolicy::check()` with the identity before proxying, matching `ServerHandler` and `SshHandler` behavior

## Acceptance Criteria

- [ ] `NapiServerHandler` holds `Arc<dyn IdentityProvider>` and `Arc<ArcSwap<DynamicConfig>>`
- [ ] `auth_publickey()` delegates through `IdentityProvider::resolve_from_fingerprint()` and stores `Identity` on the session
- [ ] `channel_open_direct_tcpip()` evaluates `ForwardingPolicy::check()` before proxying
- [ ] Per-identity forwarding rules work correctly through NAPI
- [ ] Existing NAPI tests pass
- [ ] New test: forwarding policy deny blocks channel open via NAPI handler

## References

- crates/alknet-napi/src/serve.rs — NapiServerHandler to be fixed
- crates/alknet-core/src/server/handler.rs — correct pattern to follow
- docs/architecture/identity.md — IdentityProvider contract
- docs/architecture/decisions/031-forwarding-policy.md — ForwardingPolicy

## Notes

> Identified during Phase 1 review (W1)

## Summary

> NapiServerHandler now uses ConfigIdentityProvider for auth (resolving Identity via fingerprint) and evaluates ForwardingPolicy::check() in channel_open_direct_tcpip() with the authenticated identity and transport kind, consistent with ServerHandler and SshHandler. TransportKind is properly tracked per connection instead of using a string.