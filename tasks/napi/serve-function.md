---
id: napi/serve-function
name: Implement NAPI serve() — server with connection events returning Duplex streams
status: completed
depends_on:
  - napi/project-setup
  - server/serve-loop
scope: moderate
risk: high
impact: component
level: implementation
---

## Description

Implement the NAPI `serve()` function per ADR-016. Returns a `AlknetServer` object with a `close()` method and `onConnection` event emitter. Each incoming SSH connection produces a `Duplex` stream.

The function accepts `AlknetServeOptions` and returns `Promise<AlknetServer>`. The NAPI layer handles transport binding, SSH server setup, and connection handling.

## Acceptance Criteria

- [x] `#[napi]` function `serve(options: AlknetServeOptions) -> Result<AlknetServer>` in `crates/alknet-napi/src/serve.rs`
- [x] `AlknetServeOptions` struct with napi fields: `transport`, `hostKey`, `authorizedKeys`, `certAuthority`, `tlsCert`, `tlsKey`, `acmeDomain`, `listen`, `irohRelay`
- [x] `AlknetServer` napi class with `close() -> Promise<void>` and `onConnection(callback)` event registration
- [x] Each incoming connection produces a `Duplex` stream via the `onConnection` callback
- [x] `ConnectionInfo` struct passed with each connection: `remoteAddr`, `transportKind`
- [x] Key material: `hostKey`, `authorizedKeys` accept file path (string) or `Buffer` (in-memory)
- [x] Server starts transport acceptor, authenticates connections, emits stream events
- [x] `close()` triggers graceful shutdown
- [x] TypeScript type matches napi-and-pubsub.md spec
- [x] Integration test: JS serve() + connect() round-trip works

## References

- docs/architecture/napi-and-pubsub.md — NAPI serve() spec, AlknetServer interface
- docs/architecture/decisions/016-napi-expose-connect-and-serve.md — both connect() and serve()
- docs/architecture/server.md — server configuration

## Notes

TCP transport fully implemented. TLS/iroh transports return helpful "not yet supported" errors. AlknetServerStream provides read/write/close. ConnectionInfo includes remoteAddr and transportKind.

## Summary

Implemented NAPI serve() in crates/alknet-napi/src/serve.rs: AlknetServeOptions, AlknetServer with close()/onConnection(), AlknetServerStream (Duplex read/write/close), ConnectionInfo. TCP transport works end-to-end. 241 tests pass, clippy clean.