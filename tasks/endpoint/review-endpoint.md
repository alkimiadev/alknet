---
id: endpoint/review-endpoint
name: Review alknet-endpoint implementation for spec conformance, API shape, and test coverage
status: completed
depends_on: [endpoint/tests]
scope: moderate
risk: low
impact: phase
level: review
---

## Description

Phase 2 review checkpoint. Verify the `alknet-endpoint` crate is spec-conformant,
self-contained, and ready for downstream consumption by the assembly layer (Phase 4+
and beyond). The crate must match the ADR-083 shape: `new()` takes no `StaticConfig`,
transports are injected via builder methods, `dispatch` is public, `shutdown()` is
infallible, and `EndpointError` is removed.

### Review Checklist

1. **Crate structure**:
   - Module layout matches spec: `registry.rs`, `endpoint.rs`, `dispatch.rs`, `accept/{quinn,iroh,tcp_tls}.rs`
   - Public API types: `AlknetEndpoint`, `HandlerRegistry`
   - Re-exports in `lib.rs` are correct and minimal
   - No dependency on `alknet-tls` (endpoint takes pre-built transports)

2. **`AlknetEndpoint` API shape (ADR-083)**:
   - `new(handlers, dynamic, identity_provider, drain_timeout)` — no `StaticConfig`, no TLS config
   - `with_quinn(endpoint: quinn::Endpoint)` builder (feature-gated on `quinn`)
   - `with_iroh(endpoint: iroh::Endpoint)` builder (feature-gated on `iroh`)
   - `with_tcp_tls(listener, acceptor)` builder (feature-gated on `tcp`)
   - `TcpTlsListener` type alias (feature-gated on `tcp`)
   - `shutdown_sender()` returns `watch::Sender<bool>`
   - `run(self: Arc<Self>)` spawns accept loops for each active transport
   - `shutdown(&self)` is infallible (`async fn shutdown(&self)`, no `Result`)
   - `Debug` impl lists handlers and drain_timeout (no transport internals)

3. **`HandlerRegistry`**:
   - `new()`, `register()`, `get()`, `alpn_strings()` methods
   - `register()` panics on duplicate ALPN
   - `Default` impl delegates to `new()`
   - `Debug` impl lists ALPNs without exposing handler internals
   - No feature gates (always available)

4. **`dispatch` (public)**:
   - Takes `&self`, `Connection`, `alpn`, `fingerprint`, `remote_addr`
   - Synchronous (non-async) — spawns handler and returns immediately
   - ACME guard (`acme-tls/1` → close + return) when `acme` feature enabled
   - Handler-not-found is swallowed (close + log, no error)
   - Calls `build_auth_context` and `tokio::spawn`s the handler

5. **`build_auth_context`**:
   - Resolves identity from fingerprint via `IdentityProvider`
   - Returns `AuthContext` with all fields populated
   - Feature-gated on `#[cfg(any(feature = "quinn", feature = "iroh"))]`

6. **Accept loops**:
   - Quinn: `run_accept_loop` spawns per-connection tasks, extracts ALPN + fingerprint, calls dispatch
   - Iroh: `run_accept_loop` negotiates ALPN, extracts fingerprint, calls dispatch
   - TCP+TLS: `run_accept_loop` accepts TCP, performs TLS handshake, extracts ALPN + fingerprint, calls dispatch
   - All three feed the same `dispatch_connection` free function
   - All three handle shutdown signal via `watch::Receiver`

7. **What's NOT present (correctly absent)**:
   - No `EndpointError` type (removed — `BindFailed` vestigial, `HandlerNotFound` swallowed)
   - No `StaticConfig` parameter on `new()` (assembly layer reads it)
   - No `TlsSetup`, `build_rustls_server_config`, `build_quinn_server_config_from_rustls` (in `alknet-tls`)
   - No `has_iroh_identity` function (transport-building decision moved to assembly layer)
   - No `acme_state_handle` field (ACME state lives in `alknet-tls`)
   - No dependency on `alknet-tls`

8. **Dependency hygiene**:
   - `alknet-core` is the only alknet dependency
   - `quinn` is optional, gated behind `quinn` feature (pulls `alknet-core/quinn`)
   - `iroh` is optional, gated behind `iroh` feature (pulls `alknet-core/iroh`)
   - `tokio-rustls` is optional, gated behind `tcp` feature
   - No unexpected heavy deps

9. **Test coverage**:
   - All 7 registry tests pass
   - All 3 `build_auth_context` tests pass
   - `dispatch_decision_logic_lookup_and_auth` passes
   - 3 `has_iroh_identity` replacement tests pass (builder pattern)
   - `endpoint_constructs_with_iroh_raw_key_identity` (adapted) passes
   - `iroh_endpoint_runs_accept_loop_and_shutdown` (adapted) passes
   - `debug_for_alknet_endpoint_is_implemented_without_panicking` (adapted) passes
   - Tests exercise error paths (unknown ALPN, missing fingerprint, etc.)
   - Feature-gated tests are correctly annotated

10. **Cross-cutting checks**:
    - `cargo build -p alknet-endpoint` succeeds (all feature combos)
    - `cargo test -p alknet-endpoint` succeeds (all feature combos)
    - `cargo clippy -p alknet-endpoint --all-targets` succeeds with no warnings
    - `cargo fmt --check -p alknet-endpoint` passes
    - `cargo build --workspace` still succeeds (old code untouched)
    - `cargo test --workspace` still succeeds (old tests untouched)

## Acceptance Criteria

- [ ] Crate structure matches spec (7 source files, correct module layout)
- [ ] `AlknetEndpoint` API matches ADR-083 shape (no `StaticConfig`, builder methods, infallible shutdown)
- [ ] `HandlerRegistry` API correct and complete
- [ ] `dispatch` is public, synchronous, transport-agnostic
- [ ] `build_auth_context` resolves identity correctly
- [ ] Quinn accept loop extracts ALPN + fingerprint, calls dispatch
- [ ] Iroh accept loop negotiates ALPN, extracts fingerprint, calls dispatch
- [ ] TCP+TLS accept loop performs TLS handshake, extracts ALPN + fingerprint, calls dispatch
- [ ] No `EndpointError` type present
- [ ] No `StaticConfig` in `new()` signature
- [ ] No `has_iroh_identity` function
- [ ] No dependency on `alknet-tls`
- [ ] All 17 tests pass
- [ ] `cargo build -p alknet-endpoint` succeeds (all feature combos)
- [ ] `cargo test -p alknet-endpoint` succeeds (all feature combos)
- [ ] `cargo clippy -p alknet-endpoint --all-targets` succeeds with no warnings
- [ ] `cargo fmt --check -p alknet-endpoint` passes
- [ ] Workspace still green: `cargo build --workspace` + `cargo test --workspace` pass

## References

- docs/research/alknet-crate-extraction/findings.md — Phase 2
- docs/architecture/crates/endpoint/README.md — full architecture spec
- docs/architecture/decisions/083-endpoint-as-accept-loop-runner.md — ADR-083
- tasks/endpoint/crate-init.md
- tasks/endpoint/registry.md
- tasks/endpoint/endpoint-core.md
- tasks/endpoint/dispatch.md
- tasks/endpoint/accept-quinn.md
- tasks/endpoint/accept-iroh.md
- tasks/endpoint/accept-tcp-tls.md
- tasks/endpoint/tests.md

## Notes

> This review gates Phase 2 completion. The crate must be self-contained and
> spec-conformant before Phase 3 (`alknet-client`) begins, since the assembly layer
> (which consumes both) will wire them together. The old code in core's `endpoint.rs`
> is intentionally still present (duplicated) — the prune happens in Phase 4.
> If deviations are found, document and fix before proceeding to Phase 3.

## Summary

> To be filled on completion
