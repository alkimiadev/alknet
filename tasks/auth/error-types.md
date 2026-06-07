---
id: auth/error-types
name: Define error types for transport, auth, channel, and configuration layers
status: completed
depends_on:
  - setup/project-init
scope: narrow
risk: trivial
impact: phase
level: implementation
---

## Description

Define the error hierarchy per the overview.md layered error pattern:
- **Transport errors** — connection failures, TLS handshake failures, iroh endpoint errors
- **Auth errors** — key rejection, certificate validation failures, missing keys
- **Channel errors** — target unreachable, proxy failure
- **Config errors** — invalid flags, key file not found, bind failure

Use `thiserror` for structured error types propagated via `anyhow::Result` in the public API. The key design: transport/auth errors cause reconnection (client) or rejection (server). Channel-level errors close that channel without killing the session.

## Acceptance Criteria

- [ ] `crates/alknet-core/src/error.rs` exports error types
- [ ] `TransportError` enum: `ConnectionFailed`, `HandshakeFailed`, `Timeout`, `ProxyFailed`
- [ ] `AuthError` enum: `KeyRejected`, `CertInvalid`, `CertExpired`, `CertPrincipalMismatch`, `NoMatchingKey`
- [ ] `ChannelError` enum: `TargetUnreachable`, `ProxyConnectFailed`, `ChannelClosed`
- [ ] `ConfigError` enum: `InvalidFlag`, `KeyFileNotFound`, `BindFailed`, `IncompatibleOptions`
- [ ] All error types implement `std::error::Error` via `thiserror`, `Display`, and `Debug`
- [ ] Error types have `source` chaining where appropriate (e.g., `TransportError::HandshakeFailed { source: std::io::Error }`)
- [ ] Re-exported from `crates/alknet-core/src/lib.rs`
- [ ] Unit tests for Display output of each error variant

## References

- docs/architecture/overview.md — "Error handling follows a consistent layered pattern"
- docs/architecture/client.md — error handling section (transport → reconnect, channel → close)
- docs/architecture/server.md — error handling section

## Notes

> To be filled by implementation agent

## Summary

> To be filled on completion