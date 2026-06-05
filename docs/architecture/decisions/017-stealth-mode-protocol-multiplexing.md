# ADR-017: Stealth Mode — Protocol Multiplexing on Port 443

## Status
Accepted

## Context
When running a alknet server with TLS transport on port 443, the server should be indistinguishable from a regular HTTPS web server to port scanners and deep packet inspection (DPI) systems. This is important for censorship circumvention — if SSH traffic on port 443 is detectable, it can be blocked.

After the TLS handshake completes, the server sees a raw byte stream. SSH protocol identification starts with `SSH-2.0-`, while HTTP starts with HTTP method verbs (GET, POST, etc.). The server can inspect the first bytes to determine the protocol.

## Decision
When `--stealth` is enabled with TLS transport:

1. After completing the TLS handshake, peek at the first few bytes of the connection
2. If the connection starts with `SSH-2.0-`, proceed with SSH session via `server::run_stream()`
3. If the connection starts with anything else (HTTP, random data), respond with `HTTP/1.1 404 Not Found\r\nServer: nginx\r\n\r\n` and close the connection

This makes the server appear as an nginx web server returning 404 errors to all non-SSH connections. Scanners and DPI systems see a typical HTTPS site with no SSH exposure.

The fake response uses `Server: nginx` headers to match the most common web server profile.

## Consequences
- **Positive**: TLS+alknet servers on port 443 are indistinguishable from ordinary HTTPS sites to automated scanners.
- **Positive**: Simple implementation — just peek at the first bytes and branch.
- **Positive**: Consistent with censorship circumvention best practices.
- **Negative**: Legitimate HTTPS traffic to the same port gets a 404. If the same IP needs to serve real web content, use a reverse proxy (nginx/haproxy) in front that routes by SNI or path.
- **Negative**: The `--stealth` flag only applies to TLS transport. It has no effect on TCP or iroh transports.

## References
- [server.md](../server.md)