---
id: transport/tcp-transport
name: Implement TcpTransport and TcpAcceptor
status: completed
depends_on:
  - transport/trait-and-types
scope: narrow
risk: low
impact: component
level: implementation
---

## Description

Implement the simplest transport: plain TCP. `TcpTransport` connects via `TcpStream::connect(addr)` on the client side. `TcpAcceptor` accepts via `TcpListener::accept()` on the server side. This is the baseline transport that all others build upon conceptually.

## Acceptance Criteria

- [ ] `crates/alknet-core/src/transport/tcp.rs` exports `TcpTransport` and `TcpAcceptor`
- [ ] `TcpTransport` holds a `SocketAddr` target address
- [ ] `TcpTransport::connect()` calls `TcpStream::connect(addr)` and returns the stream
- [ ] `TcpTransport::describe()` returns e.g. `"tcp://1.2.3.4:22"`
- [ ] `TcpAcceptor` holds a `TcpListener` and accept address
- [ ] `TcpAcceptor::accept()` calls `listener.accept()`, returns `(stream, TransportInfo)` with `remote_addr` set and `TransportKind::Tcp`
- [ ] `TcpAcceptor` constructor binds the listener: `TcpAcceptor::bind(addr)` async factory
- [ ] Connection timeout handling (tokio default connect timeout is reasonable; document behavior)
- [ ] Unit tests: connect creates a stream, accept receives a connection, describe format
- [ ] Integration test: client connects to server via TCP, stream is duplex

## References

- docs/architecture/transport.md — TcpTransport row in implementations table
- docs/architecture/overview.md — "TCP on port 22 for basic use"

## Notes

> To be filled by implementation agent

## Summary

> To be filled on completion