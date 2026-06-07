---
id: review/core-foundation
name: Review core foundation — transport traits, auth, error types, key loading
status: completed
depends_on:
  - meta/transport-layer
  - meta/auth-layer
  - setup/test-infrastructure
scope: broad
risk: low
impact: phase
level: review
---

## Description

Review the core foundation layer before proceeding to server/client implementation. Verify that transport abstractions match architecture, auth logic is correct, errors follow the layered pattern, and key loading handles all spec'd formats.

This is the critical review before building the higher-level server and client components on top of these foundations.

## Acceptance Criteria

- [ ] Transport trait matches transport.md: correct bounds, object-safety, describe() method
- [ ] TransportAcceptor matches transport.md: returns TransportInfo with correct metadata
- [ ] TCP, TLS, iroh transports all produce correct stream types per implementations table
- [ ] ACME integration with TLS works (or feature gates correctly prevent compilation without it)
- [ ] Key loading handles file paths and in-memory data, rejects PEM format
- [ ] authorized_keys parsing handles cert-authority entries with options
- [ ] Server auth: Ed25519 key matching (constant-time), cert-authority validation (signature, expiry, principal)
- [ ] Client auth: key pair presentation, Handler implementation
- [ ] Error types cover all four layers (transport, auth, channel, config)
- [ ] All tests pass: `cargo test --workspace`
- [ ] `cargo clippy --workspace` passes with no warnings

## References

- docs/architecture/transport.md, docs/architecture/client.md, docs/architecture/server.md

## Notes

> To be filled by implementation agent

## Summary

> To be filled on completion