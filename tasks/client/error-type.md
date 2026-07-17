---
id: client/error-type
name: Implement ClientDialError enum with five variants
status: completed
depends_on: [client/crate-init]
scope: narrow
risk: low
impact: component
level: implementation
---

## Description

Phase 3, Task 2. Implement the `ClientDialError` enum in `crates/alknet-client/src/error.rs`.
This is the error type for all three dial methods (`dial_quic`, `dial_tcp_tls`, `dial_iroh`).
A single `#[non_exhaustive]` enum, one variant per failure category.

### Target shape (per ADR-089 / architecture spec)

```rust
use thiserror::Error;

/// Errors produced by `AlknetClient` dial methods.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ClientDialError {
    /// TLS config construction failure — `TlsClientConfig::new` failed
    /// (verifier build, cert load, provider init). Wraps `TlsError`
    /// from alknet-tls.
    #[error("TLS config construction: {0}")]
    TlsConfig(#[from] alknet_tls::TlsError),

    /// Transport connect failure — quinn connect, TcpStream::connect,
    /// or iroh connect. The transport's own error type, stringified.
    #[error("transport connect: {0}")]
    Connect(String),

    /// TLS handshake failure — the handshake started but failed
    /// (rejected cert, ALPN mismatch, unknown raw-key remote
    /// fail-closed). Distinct from TlsConfig (which is pre-handshake).
    #[error("TLS handshake: {0}")]
    Handshake(String),

    /// No transport handle configured for the requested dial — e.g.,
    /// `dial_quic` called but `with_quinn` was not set.
    #[error("no transport handle configured for {transport}")]
    NoTransport { transport: &'static str },

    /// SOCKS5 proxy failure — handshake rejected, UDP ASSOCIATE
    /// unsupported, auth failed, or the proxy closed the control
    /// connection (ADR-090). The dial did not reach the remote; the
    /// caller decides whether to fall back to a direct dial or
    /// surface the error. The dial never silently falls back — that
    /// would defeat the privacy posture.
    #[cfg(feature = "socks5")]
    #[error("SOCKS5 proxy: {0}")]
    Proxy(String),
}
```

### Design rationale

**`TlsConfig` wraps `alknet_tls::TlsError`** (ADR-088) — the config construction errors.
This is the only variant that wraps a concrete error type via `#[from]`, because
`TlsError` has one source crate (rustls + pemfile + rcgen).

**`Connect(String)` and `Handshake(String)` take `String`** rather than wrapping the
concrete transport error types (`quinn::ConnectError`, `io::Error`, `rustls::Error`)
because the three transports' error types are non-unifiable — the dial is
transport-polymorphic, and there is no single source type that covers quinn,
tokio-rustls, and iroh. The category is in the variant (`Connect` vs `Handshake`);
the detail is in the string.

**`Handshake` resolves an ADR-088 §6 deferral.** ADR-088 §6 explicitly scoped
`TlsError` to config-construction errors and deferred the handshake-error surfacing
question to the dial-seam ADR. `ClientDialError::Handshake` is the resolution:
handshake-time errors (rejected cert, ALPN mismatch, unknown-raw-key fail-closed)
surface through the dial's error type as `Handshake(String)`, not through `TlsError`
(which stays config-construction-only).

**`NoTransport`** is a wiring error — calling a dial without the matching `with_*`
builder method. The `transport` field is a `&'static str` like `"quinn"`, `"tcp"`,
or `"iroh"`.

**`Proxy`** (ADR-090) is the SOCKS5 proxy failure category — the dial's transport
never reached the remote because the proxy rejected or dropped the association.
Feature-gated on `socks5`. Takes `String` for the same reason `Connect` and
`Handshake` do — the proxy error source type is an implementation detail.

### What this does NOT include

- No `EndpointError` equivalent — the endpoint's error type was removed per ADR-083.
  `ClientDialError` is the client-side error type, not a shared type.
- No `From<io::Error>` impl — the dials convert `io::Error` to `Connect(String)`
  or `Handshake(String)` at the call site, not via blanket `From`.

## Acceptance Criteria

- [ ] `ClientDialError` enum defined in `crates/alknet-client/src/error.rs`
- [ ] Five variants: `TlsConfig`, `Connect`, `Handshake`, `NoTransport`, `Proxy`
- [ ] `TlsConfig` wraps `alknet_tls::TlsError` via `#[from]`
- [ ] `Connect` and `Handshake` take `String` (not concrete transport error types)
- [ ] `NoTransport` has `transport: &'static str` field
- [ ] `Proxy` is feature-gated on `#[cfg(feature = "socks5")]`
- [ ] All variants have `#[error("...")]` messages
- [ ] `#[non_exhaustive]` attribute on the enum
- [ ] `Debug` derived
- [ ] Re-exported from `lib.rs` (or via `pub mod error; pub use error::ClientDialError;`)
- [ ] `cargo check -p alknet-client` succeeds
- [ ] `cargo clippy -p alknet-client` succeeds with no warnings
- [ ] `cargo build --workspace` still succeeds

## References

- docs/architecture/crates/client/README.md — `ClientDialError` section (lines 460-530)
- docs/architecture/decisions/089-alknetclient-native-dial-seam.md — ADR-089
- docs/architecture/decisions/088-webpki-roots-fallback.md — ADR-088 §6 (handshake error deferral)
- docs/architecture/decisions/090-client-dial-socks5-proxy-seam.md — ADR-090 (Proxy variant)
- crates/alknet-tls/src/lib.rs — `TlsError` definition (reference for the wrapped type)

## Notes

> This is a small, self-contained task. The error type is used by all three dial
> methods (tasks client/dial-quic, client/dial-tcp-tls, client/dial-iroh) and the
> SOCKS5 proxy path (client/socks5-proxy). `Connect(String)` and `Handshake(String)`
> take `String` rather than wrapping concrete transport error types because the
> three transports' error types are non-unifiable. The `Proxy` variant is
> feature-gated on `socks5` per ADR-090.

## Summary

> To be filled on completion
