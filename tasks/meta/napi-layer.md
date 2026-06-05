---
id: meta/napi-layer
name: Complete NAPI layer — project setup, connect(), serve()
status: completed
depends_on:
  - napi/project-setup
  - napi/connect-function
  - napi/serve-function
scope: moderate
risk: high
impact: phase
level: planning
---

## Description

Meta task that clusters NAPI tasks. Once complete, the `@alkdev/alknet` Node.js native addon provides `connect()` and `serve()` returning duplex streams for TypeScript consumers.

## Acceptance Criteria

- [x] All NAPI tasks completed
- [x] `connect()` returns Duplex stream, no SOCKS5, no port forwarding
- [x] `serve()` returns AlknetServer with close() and onConnection events
- [x] Key material from Buffer (in-memory) and file paths both work
- [x] JS-to-Rust and Rust-to-JS error marshalling works correctly

## References

- docs/architecture/napi-and-pubsub.md

## Summary

NAPI layer complete. connect() returns AlknetStream (read/write/close), serve() returns AlknetServer with close()/onConnection(). Key material works from both file paths and in-memory Buffers. TCP transport fully supported; TLS/iroh return helpful errors in NAPI layer.