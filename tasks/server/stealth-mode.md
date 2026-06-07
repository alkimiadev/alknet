---
id: server/stealth-mode
name: Implement stealth mode — protocol multiplexing on port 443 (ADR-017)
status: completed
depends_on:
  - transport/tls-transport
  - server/handler
scope: narrow
risk: medium
impact: component
level: implementation
---

## Description

Implement stealth mode per ADR-017. When `--stealth` is enabled alongside TLS transport on port 443:

1. After completing the TLS handshake, peek at the first bytes of the connection
2. If the connection starts with `SSH-2.0-`, proceed with `russh::server::run_stream()`
3. If the connection starts with anything else (HTTP, random data), respond with `HTTP/1.1 404 Not Found\r\nServer: nginx\r\n\r\n` and close

This makes the server appear as an nginx web server returning 404 errors to non-SSH connections, making it indistinguishable from a regular HTTPS site to port scanners and DPI systems.

Stealth mode requires TLS transport. The CLI should reject or warn if `--stealth` is used without `--transport tls`.

## Acceptance Criteria

- [ ] `crates/alknet-core/src/server/stealth.rs` exports stealth mode protocol detection
- [ ] `detect_protocol(stream: TlsStream) -> ProtocolDetection` — peeks at first bytes to determine SSH vs HTTP
- [ ] `ProtocolDetection` enum: `Ssh`, `Http` (or `Unknown`)
- [ ] If SSH detected: pass stream to `russh::server::run_stream()`
- [ ] If HTTP/unknown detected: write `HTTP/1.1 404 Not Found\r\nServer: nginx\r\n\r\n` then close
- [ ] Peek uses `tokio::io::BufReader` or similar buffered read to avoid consuming the SSH banner bytes
- [ ] Integration with `TlsAcceptor` flow: after accept + TLS handshake, optionally run protocol detection before passing to russh
- [ ] Stealth mode flag validated: requires TLS transport, warn/reject otherwise
- [ ] Unit tests: SSH banner detection, HTTP request detection, random data → fake nginx 404
- [ ] Integration test: stealth server responds to HTTP scanner with 404, SSH client connects successfully

## References

- docs/architecture/server.md — Stealth Mode section
- docs/architecture/decisions/017-stealth-mode-protocol-multiplexing.md — protocol multiplexing design

## Notes

> To be filled by implementation agent

## Summary

> To be filled on completion