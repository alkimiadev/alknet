---
id: axum-http-router-scaffold
name: Axum HTTP router scaffold with auth middleware and stealth handoff
status: pending
depends_on: [api-keys-dynamic-config, listenconfig-http-dns-stubs]
scope: moderate
risk: low
impact: component
level: implementation
---

## Description

Create an axum HTTP router scaffold behind the `http` feature flag, with auth middleware that extracts `Authorization: Bearer <token>` and calls `IdentityProvider::resolve_from_token()`, and a stealth mode handoff that replaces `send_fake_nginx_404` with routing detected HTTP traffic to the axum router.

Per the integration plan section 2.7 and research/phase2/tls-transport.md:

This task creates the structural scaffold for HTTP — auth middleware and stealth handoff only. No operational routes (no `POST /v1/{namespace}/{op}` handlers). The question of how HTTP paths map to operation invocations is intentionally deferred to Phase 5.

**Key components**:
1. **Auth middleware**: Extract `Authorization: Bearer <token>` from HTTP request headers. Call `IdentityProvider::resolve_from_token()`. Attach resolved `Identity` to request extensions. Reject with 401 if token is missing or invalid. Both AuthTokens (Ed25519 signed) and API keys (hash-verified) go through this path.
2. **Stealth handoff**: When `ListenerConfig::Http { stealth: true }`, replace `send_fake_nginx_404` with routing the detected-HTTP `BufReader<TlsStream>` to the axum router. The existing `ProtocolDetection` enum already has `Ssh` vs `Http` — the `Http` path currently sends a fake 404 and disconnects.
3. **Default 404 handler**: Any unmatched route returns 404. No `/v1/*` routes are registered yet.
4. **Dependency**: Add `axum` dependency behind `http` feature flag in `Cargo.toml`.

**Current state**:
- `stealth.rs` has `detect_protocol()` returning `ProtocolDetection::Ssh` or `ProtocolDetection::Http`
- `send_fake_nginx_404()` currently sends a fake nginx 404 response
- No `axum` dependency exists yet
- `IdentityProvider::resolve_from_token()` exists (will be extended with API keys by task 2.6)

## Acceptance Criteria

- [ ] `axum` dependency added to `Cargo.toml` behind `http` feature flag
- [ ] `crates/alknet-core/src/http/` module created (behind `http` feature flag)
- [ ] Auth middleware function: extracts `Authorization: Bearer <token>`, calls `IdentityProvider::resolve_from_token()`, attaches `Identity` to axum request extensions, returns 401 on missing/invalid token
- [ ] Auth middleware supports both AuthTokens and API keys (via `resolve_from_token()` which dispatches based on format/prefix)
- [ ] Stealth handoff: `stealth.rs` `send_fake_nginx_404` replaced with axum router handoff when `http` feature is enabled. When `http` feature is disabled, the fake 404 behavior remains.
- [ ] Default 404 handler for unmatched routes (returns `404 Not Found`)
- [ ] Axum `Router` scaffold constructed with auth middleware layer and default 404 fallback
- [ ] `HttpInterface` struct from task 1 (stream/message interface split) gets its internal `Router` reference and `IdentityProvider` wired
- [ ] `http` feature flag in `Cargo.toml` correctly gates the `axum` dependency and `http` module
- [ ] Unit test: auth middleware extracts bearer token from `Authorization` header
- [ ] Unit test: auth middleware returns 401 for missing token
- [ ] Unit test: auth middleware returns 401 for invalid token
- [ ] Unit test: auth middleware attaches `Identity` to request extensions on valid token
- [ ] Integration test: stealth mode detection routes HTTP traffic to axum (not fake 404)
- [ ] All existing server/stealth tests continue to pass (no behavioral change when `http` feature is disabled)

## References

- docs/research/integration-plan.md — Phase 2.7
- docs/research/phase2/tls-transport.md — Axum integration, stealth handoff, auth middleware
- crates/alknet-core/src/server/stealth.rs — Current ProtocolDetection, send_fake_nginx_404
- crates/alknet-core/src/auth/identity.rs — IdentityProvider::resolve_from_token()

## Notes

> The integration plan explicitly states: "No operational routes yet — the question of how HTTP paths map to operation invocations depends on the from_openapi / spec-generation work and is deferred to Phase 5." This task is a scaffold: auth middleware, stealth handoff, default 404. Full route registrations come later.

> For the stealth handoff, consider a compile-time approach: the `http` feature flag determines whether `send_fake_nginx_404` or the axum handoff is used. When `http` is disabled, the existing fake 404 behavior should remain unchanged.

> The axum router is created per-server (not per-request). It holds references to the `IdentityProvider` and `OperationEnv`/`OperationRegistry`.

> `send_fake_nginx_404` should NOT be deleted — just conditionally bypassed when the `http` feature is enabled and a `ListenerConfig::Http` listener is configured.

## Summary

> To be filled on completion