---
id: client/review-client
name: Review alknet-client implementation for spec conformance, API shape, and test coverage
status: completed
depends_on: [client/tests]
scope: moderate
risk: low
impact: phase
level: review
---

## Description

Phase 3 review checkpoint. Verify the `alknet-client` crate is spec-conformant,
self-contained, and ready for downstream consumption by the assembly layer (hub/worker).
The crate must match the ADR-089/090/091 shape: three dial methods unified on
`&ConnectionCredentials`, pre-built transports via builder methods, `ClientDialError`
with five variants, and optional SOCKS5 proxy support.

### Review Checklist

1. **Crate structure**:
   - Module layout matches spec: `error.rs`, `client.rs`, `dial/{mod,quinn,tcp_tls,iroh}.rs`, `socks5.rs`
   - Public API types: `AlknetClient`, `ClientDialError`, `Socks5ProxyConfig`, `Socks5Credentials`
   - Re-exports in `lib.rs` are correct and minimal
   - No dependency on `alknet-call` (dial is below the protocol)

2. **`AlknetClient` API shape (ADR-089/090)**:
   - `new()` takes no parameters (no `StaticConfig`, no credentials)
   - `with_quinn(endpoint: quinn::Endpoint)` builder (feature-gated on `quinn`)
   - `with_tcp_tls(connector: TlsConnector)` builder (feature-gated on `tcp`)
   - `with_iroh(endpoint: iroh::Endpoint)` builder (feature-gated on `iroh`)
   - `with_socks5_proxy(proxy: Socks5ProxyConfig)` builder (feature-gated on `socks5`)
   - `Default` impl delegates to `new()`
   - `Debug` impl lists configured transports (no transport internals)
   - No `connect()` method (the old welded dial is not replicated)

3. **`ClientDialError` (ADR-089)**:
   - Five variants: `TlsConfig`, `Connect`, `Handshake`, `NoTransport`, `Proxy`
   - `TlsConfig` wraps `alknet_tls::TlsError` via `#[from]`
   - `Connect` and `Handshake` take `String` (not concrete transport error types)
   - `NoTransport` has `transport: &'static str` field
   - `Proxy` is feature-gated on `socks5`
   - `#[non_exhaustive]` attribute
   - All variants have descriptive `#[error("...")]` messages

4. **`dial_quic` (ADR-089 §3, ADR-091)**:
   - Signature: `(addr, server_name, alpn, creds: &ConnectionCredentials) -> Result<Connection, ClientDialError>`
   - Builds `TlsClientConfig::new(creds, alpn)` — `TlsError` → `ClientDialError::TlsConfig`
   - Converts to `quinn::ClientConfig` via `for_quinn()`
   - Uses pre-built quinn endpoint from `self.quinn`
   - Returns `NoTransport` when `self.quinn` is `None`
   - Returns `Connection::from_quinn_with_alpn(conn, alpn)`
   - Does NOT call `spawn_dispatch` (protocol take-over is caller's concern)
   - Does NOT hardcode `alknet/call` ALPN
   - SOCKS5 proxy path: uses `Socks5UdpSocket` + `new_with_abstract_socket` when proxy configured

5. **`dial_tcp_tls` (ADR-089 §3, ADR-091)**:
   - Signature: `(host, addr, alpn, creds: &ConnectionCredentials) -> Result<Connection, ClientDialError>`
   - Builds `TlsClientConfig::new(creds, alpn)`
   - Uses pre-built `TlsConnector` or builds one from `TlsClientConfig`
   - Connects TCP via `TcpStream::connect(addr)`
   - `Connect` errors → `ClientDialError::Connect`
   - TLS handshake errors → `ClientDialError::Handshake`
   - Returns `Connection::from_bidi(tls_stream, alpn, Some(addr))`
   - SOCKS5 proxy path: SOCKS5 CONNECT handshake before TLS

6. **`dial_iroh` (ADR-089 §3, ADR-091)**:
   - Signature: `(alpn, creds: &ConnectionCredentials) -> Result<Connection, ClientDialError>`
   - Does NOT use `TlsClientConfig` (iroh has its own TLS)
   - Extracts `NodeId` from `creds.remote_identity.fingerprint`
   - Unknown iroh remote (`remote_identity: None`) fails closed
   - Returns `Connection::from_iroh(conn)`
   - No `addr` or `server_name` parameter (iroh handles addressing internally)

7. **SOCKS5 proxy (ADR-090)**:
   - `Socks5ProxyConfig` with `addr` and `credentials` fields
   - `Socks5Credentials` with `username` and `password` fields
   - `Socks5UdpSocket` implements `quinn::AsyncUdpSocket`
   - `dial_quic` routes through UDP ASSOCIATE when proxy configured
   - `dial_tcp_tls` routes through CONNECT when proxy configured
   - No silent fallback to direct connection when proxy configured
   - `Proxy` error variant used for proxy failures
   - Feature-gated on `socks5`

8. **Dependency hygiene**:
   - `alknet-core` and `alknet-tls` are the only alknet dependencies
   - No dependency on `alknet-call` or `alknet-channels-call`
   - `quinn` is optional, gated behind `quinn` feature (pulls `alknet-tls/quinn` + `alknet-core/quinn`)
   - `tokio-rustls` is optional, gated behind `tcp` feature (pulls `alknet-tls/tcp`)
   - `iroh` is optional, gated behind `iroh` feature (pulls `alknet-core/iroh`)
   - `fast-socks5` is optional, gated behind `socks5` feature
   - No unexpected heavy deps

9. **Test coverage**:
   - `AlknetClient` construction tests: `new()`, `Default`, `Send + Sync`, `Debug`
   - `ClientDialError` tests: `#[from]` conversion, display formatting, `Send + Sync`
   - `dial_quic` error path tests: `NoTransport`, `TlsConfig`
   - `dial_tcp_tls` error path tests: `NoTransport`
   - `dial_iroh` error path tests: `NoTransport`, unknown remote fail-closed
   - `Socks5ProxyConfig` / `Socks5Credentials` construction tests
   - Integration test: `tests/dial_and_takeover.rs`
   - Feature-gated tests are correctly annotated

10. **Cross-cutting checks**:
    - `cargo build -p alknet-client` succeeds (all feature combos)
    - `cargo test -p alknet-client` succeeds (all feature combos)
    - `cargo clippy -p alknet-client --all-targets` succeeds with no warnings
    - `cargo fmt --check -p alknet-client` passes
    - `cargo build --workspace` still succeeds (old code untouched)
    - `cargo test --workspace` still succeeds (old tests untouched)

## Acceptance Criteria

- [ ] Crate structure matches spec (7 source files, correct module layout)
- [ ] `AlknetClient` API matches ADR-089/090 shape (builder methods, no `connect()`, no `StaticConfig`)
- [ ] `ClientDialError` has 5 variants, `#[non_exhaustive]`, correct `#[from]` impl
- [ ] `dial_quic` takes `&ConnectionCredentials`, returns `Connection`, ALPN is a parameter
- [ ] `dial_tcp_tls` takes `&ConnectionCredentials`, returns `Connection`, `host` + `addr` separate
- [ ] `dial_iroh` takes `&ConnectionCredentials`, returns `Connection`, no `addr`/`server_name`
- [ ] All three dials unified on `&ConnectionCredentials` (ADR-091)
- [ ] `dial_iroh` does NOT use `TlsClientConfig` (iroh has its own TLS)
- [ ] SOCKS5 proxy: `Socks5ProxyConfig`, `Socks5Credentials`, `Socks5UdpSocket`, proxy integration in dials
- [ ] No silent fallback to direct connection when proxy configured
- [ ] No dependency on `alknet-call`
- [ ] All tests pass (unit + integration)
- [ ] `cargo build -p alknet-client` succeeds (all feature combos)
- [ ] `cargo test -p alknet-client` succeeds (all feature combos)
- [ ] `cargo clippy -p alknet-client --all-targets` succeeds with no warnings
- [ ] `cargo fmt --check -p alknet-client` passes
- [ ] Workspace still green: `cargo build --workspace` + `cargo test --workspace` pass

## References

- docs/research/alknet-crate-extraction/findings.md — Phase 3
- docs/architecture/crates/client/README.md — full architecture spec
- docs/architecture/decisions/089-alknetclient-native-dial-seam.md — ADR-089
- docs/architecture/decisions/090-client-dial-socks5-proxy-seam.md — ADR-090
- docs/architecture/decisions/091-connectioncredentials-decouple-dial-from-call.md — ADR-091
- tasks/client/crate-init.md
- tasks/client/error-type.md
- tasks/client/client-core.md
- tasks/client/dial-quic.md
- tasks/client/dial-tcp-tls.md
- tasks/client/dial-iroh.md
- tasks/client/socks5-proxy.md
- tasks/client/tests.md

## Notes

> This review gates Phase 3 completion. The crate must be self-contained and
> spec-conformant before Phase 4 (core prune) begins, since the prune removes
> the old `endpoint.rs` from core and the assembly layer (which consumes
> `alknet-client`) will wire the dial to the protocol take-overs. The old code
> in `call_client.rs` is intentionally still present (duplicated) — the prune
> happens in Phase 5. If deviations are found, document and fix before
> proceeding to Phase 4.

## Summary

> To be filled on completion
