---
id: review/complete-system
name: Review complete system — CLI, NAPI, end-to-end integration
status: completed
depends_on:
  - meta/cli-layer
  - meta/napi-layer
  - review/server-and-client
scope: system
risk: low
impact: project
level: review
---

## Description

Final review of the complete alknet system. Verify CLI binary works end-to-end, NAPI wrapper provides correct JavaScript API, and both layers properly wrap the core library.

## Acceptance Criteria

- [x] `alknet serve` + `alknet connect` end-to-end: SSH tunnel established, SOCKS5 proxy routes traffic
- [x] All CLI flags work: transport modes (tcp, tls, iroh), auth options, proxy, stealth, rate limits
- [x] Environment variables (`ALKNET_SERVER`, `ALKNET_IDENTITY`) work as defaults
- [x] `--stealth` validates `--transport tls` requirement
- [x] NAPI `connect()` returns Duplex stream; data flows bidirectionally
- [x] NAPI `serve()` accepts connections; `onConnection` emits Duplex streams
- [x] NAPI key material from Buffer works (not just file paths)
- [x] Feature flags: `tls`, `iroh`, `acme` correctly gate optional functionality
- [x] Base build (`cargo build -p alknet-core` with no features) compiles and works
- [x] All tests pass: `cargo test --workspace`
- [x] NAPI tests pass: `cd crates/alknet-napi && npm test`
- [x] `cargo clippy --workspace` passes
- [x] No logging of tunnel destinations anywhere in the system

## References

- docs/architecture/overview.md, docs/architecture/napi-and-pubsub.md

## Summary

Final review complete. All acceptance criteria verified:
- CLI binary: alknet serve/connect with all flags, env vars, stealth validation
- NAPI: connect() returns AlknetStream, serve() returns AlknetServer with onConnection
- Feature flags: tls, iroh, acme correctly gate optional code; base build compiles
- ADR-006: no server-side logging of tunnel destinations
- 241 tests pass, clippy clean with all features