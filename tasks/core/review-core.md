---
id: core/review-core
name: Review alknet-core implementation for spec conformance and pattern consistency
status: pending
depends_on: [core/endpoint]
scope: moderate
risk: low
impact: phase
level: review
---

## Description

Review the alknet-core implementation for spec conformance, pattern
consistency, and correctness before alknet-call (which depends on core)
begins implementation. This is the quality checkpoint at the end of the core
phase.

### Review Checklist

1. **Core types conformance** (core-types.md):
   - `ProtocolHandler` trait signature matches spec (alpn, handle)
   - `HandlerError` has all 4 variants (ConnectionClosed, StreamError, AuthRequired, Internal)
   - `Connection` has all methods, from_quinn/from_iroh feature-gated
   - `Connection::set_identity` is write-once via OnceLock
   - `BiStream` is a trait (AsyncRead + AsyncWrite + Send + Unpin)
   - `SendStream` implements AsyncWrite, `RecvStream` implements AsyncRead
   - `StreamError` has all 4 variants
   - `From<StreamError> for HandlerError` impl matches spec mapping table
   - `Capabilities` is non-serializable, zeroized, immutable, Clone+Send+Sync
   - `Capabilities` has builder API (new, with_api_key, with_http_token, get), private fields

2. **Config conformance** (config.md):
   - `StaticConfig` fields match (listen_addr, tls_identity, iroh_relay, drain_timeout)
   - `TlsIdentity` has X509, RawKey, SelfSigned
   - `DynamicConfig` has auth and rate_limits
   - `AuthPolicy` has authorized_fingerprints (HashSet<String>), api_keys (Vec<ApiKeyEntry>)
   - `ApiKeyEntry` has all 5 fields (prefix, hash, scopes, description, expires_at)
   - `ConfigReloadHandle` has reload() and dynamic()
   - No russh dependency (fingerprints as strings)
   - No removed fields (host_key, stealth, transport_mode, listeners)

3. **Auth conformance** (auth.md):
   - `AuthContext` has all 4 fields, derives Clone
   - `Identity` has id, scopes, resources
   - `AuthToken` has raw field
   - `IdentityProvider` trait with both methods
   - `ConfigIdentityProvider` reads from ArcSwap on every call
   - Fingerprint resolution looks up in authorized_fingerprints
   - Token resolution: alk_ prefix, hash match, expiry check
   - Two identity scopes documented (connection-level vs per-request)

4. **Endpoint conformance** (endpoint.md):
   - `AlknetEndpoint` has quinn/iroh (both Option, both feature-gated)
   - `HandlerRegistry` register/get/alpn_strings, panics on duplicate
   - Quinn accept loop: select on accept + shutdown, dispatch by ALPN
   - iroh accept loop: select on accept + shutdown, dispatch by ALPN
   - Dispatch spawns handler task via tokio::spawn
   - AuthContext constructed from connection (alpn, remote_addr, fingerprint, identity)
   - TLS RawKey: only_raw_public_keys(), on-the-fly cert from Ed25519
   - TLS X509: load from files
   - TLS SelfSigned: generate on startup
   - ALPN list in ServerConfig from registry.alpn_strings()
   - Graceful shutdown: drain timeout, force close
   - EndpointError has all 3 variants
   - No byte-peeking, no per-handler loops, no SSH-specific logic

5. **Pattern consistency**:
   - ArcSwap used consistently for DynamicConfig
   - Feature flags (quinn, iroh) gate transport code correctly
   - Error handling patterns consistent (thiserror, Result propagation)
   - No quinn/iroh types in public API (Connection wraps them)

6. **Security constraints**:
   - Capabilities non-serializable (no Serialize derive)
   - Capabilities zeroized (Zeroize, ZeroizeOnDrop)
   - Capabilities immutable (no mut accessors)
   - Config reload is privilege escalation (no unauthenticated reload endpoint)
   - Token entropy requirement documented

7. **Test coverage**:
   - Unit tests for Capabilities (build, get, clone, zeroize)
   - Unit tests for config types and reload
   - Unit tests for auth resolution (fingerprint, token, expiry)
   - Unit tests for HandlerRegistry
   - Integration test: endpoint dispatch by ALPN

## Acceptance Criteria

- [ ] All core types match core-types.md
- [ ] All config types match config.md
- [ ] All auth types match auth.md
- [ ] Endpoint matches endpoint.md (accept loops, TLS modes, shutdown)
- [ ] Capabilities security constraints satisfied (non-serializable, zeroized, immutable)
- [ ] No russh dependency in core
- [ ] No quinn/iroh types in public API
- [ ] ArcSwap pattern consistent
- [ ] Feature flags gate transport code correctly
- [ ] Test coverage adequate for all functionality
- [ ] `cargo fmt --check -p alknet-core` passes
- [ ] `cargo clippy -p alknet-core` passes with no warnings
- [ ] All tests pass

## References

- docs/architecture/crates/core/README.md
- docs/architecture/crates/core/core-types.md
- docs/architecture/crates/core/config.md
- docs/architecture/crates/core/auth.md
- docs/architecture/crates/core/endpoint.md
- docs/architecture/decisions/ (relevant ADRs: 001-011, 014, 015, 022)

## Notes

> This review verifies core is spec-conformant before alknet-call begins.
> alknet-call depends heavily on core types (ProtocolHandler, Connection,
> AuthContext, Capabilities, IdentityProvider) — any issues here propagate to
> call. If deviations are found, document and fix before proceeding.

## Summary

> To be filled on completion