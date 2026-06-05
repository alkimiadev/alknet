---
id: review/server-and-client
name: Review server and client implementation — full SSH tunnel functionality
status: completed
depends_on:
  - meta/server-layer
  - meta/client-layer
  - review/core-foundation
scope: broad
risk: low
impact: phase
level: review
---

## Description

Review the server and client implementations after the core foundation review. This is a critical checkpoint before the CLI and NAPI layers — the server and client must work correctly as a unit before wrapping them in CLI flags or NAPI bindings.

Verify end-to-end SSH tunnel flow: client connects → SOCKS5 proxy works → port forwards work → reconnection works → server handles channels → proxy modes work → stealth mode works.

## Acceptance Criteria

- [x] Server accepts SSH connections over TCP, TLS, iroh (via integration tests)
- [x] Client establishes SSH sessions and runs SOCKS5 proxy
- [x] Channel proxy: direct TCP, SOCKS5 proxy, HTTP CONNECT proxy all work
- [x] Stealth mode: non-SSH gets nginx 404, SSH connects normally
- [x] Rate limiting: connection limits enforced, auth attempt limits enforced
- [x] Logging: structured `tracing::info!` events match ADR-013 format
- [x] No logging of tunnel destinations (ADR-006)
- [x] Reconnection: transport failure → exponential backoff → reconnect → port forwards re-registered
- [x] Reserved `alknet-` destinations routed to control channel, not TCP proxy
- [x] Graceful shutdown works for both server and client
- [x] All tests pass: `cargo test --workspace`
- [x] `cargo clippy --workspace` passes

## References

- docs/architecture/server.md, docs/architecture/client.md

## Summary

Server and client review passed with fixes. Key issues found and resolved:
- wired channel proxy into handler (was dropping all non-alknet channels)
- added client reconnection with exponential backoff + remote forward re-registration
- fixed ADR-006 violations (removed server-side destination logging)
- 241 tests pass, clippy clean