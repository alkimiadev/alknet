---
id: transport/tls-transport
name: Implement TlsTransport and TlsAcceptor (feature-gated tls)
status: completed
depends_on:
  - transport/tcp-transport
  - transport/trait-and-types
scope: moderate
risk: medium
impact: component
level: implementation
---

## Description

Implement TLS transport that wraps TCP with `tokio-rustls`. Client-side: `TlsTransport` establishes a TCP connection and wraps it with a TLS client session. Server-side: `TlsAcceptor` accepts TCP connections and wraps them with a TLS server session.

Supports:
- Manual cert/key configuration (`--tls-cert`, `--tls-key`)
- insecure mode (accept self-signed certs) for development
- `tls_server_name` override for SNI (ADR-010)
- Stealth mode support requires peeking at first bytes post-TLS-handshake (handled in server task, but TLS stream must support this)

Feature-gated behind `tls` feature flag.

## Acceptance Criteria

- [ ] `crates/alknet-core/src/transport/tls.rs` (behind `#[cfg(feature = "tls")]`)
- [ ] `TlsTransport` holds: target addr, optional `tls_server_name`, `insecure` flag, optional root cert for verification
- [ ] `TlsTransport::connect()` does TCP connect then TLS client handshake via `tokio_rustls::TlsConnector`
- [ ] When `insecure`, accepts any certificate (dangerous, `webpki_roots::CertStore` bypass or custom verifier)
- [ ] When not `insecure`, verifies server cert against system roots + optional custom CA
- [ ] `TlsTransport::describe()` returns e.g. `"tls://example.com:443"`
- [ ] `TlsAcceptor` holds: `TcpListener`, `ServerConfig` (from `rustls::ServerConfig`)
- [ ] `TlsAcceptor::accept()` does TCP accept then TLS server handshake via `tokio_rustls::TlsAcceptor`
- [ ] `TlsAcceptor` constructor accepts: `tls_cert` path/data, `tls_key` path/data, optional ACME config (stub for now)
- [ ] `TransportInfo.transport_kind` is `TransportKind::Tls { server_name }`
- [ ] Module re-exported from `transport/mod.rs` behind `#[cfg(feature = "tls")]`
- [ ] Unit tests for connect/accept with self-signed certs (insecure mode)
- [ ] Integration test: full TLS client-to-server connection succeeds

## References

- docs/architecture/transport.md — TlsTransport row, TLS cert provisioning
- docs/architecture/server.md — TLS certificate provisioning modes
- docs/architecture/decisions/008-acme-lets-encrypt.md — ACME cert support (feature-gated)

## Notes

> To be filled by implementation agent

## Summary

> To be filled on completion