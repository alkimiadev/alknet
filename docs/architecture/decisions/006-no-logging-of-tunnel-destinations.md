# ADR-006: No Logging of Tunnel Destinations

## Status
Accepted

## Context
An SSH tunnel server sees every destination that clients connect to — hostnames, IP addresses, port numbers. This is extremely sensitive information. Logging it creates:

- **Privacy risks**: Tunnel destinations reveal what services users access (internal databases, APIs, etc.)
- **Legal concerns**: Server operators may be pressured to produce logs showing what clients accessed
- **Data retention liability**: Stored destination logs are an attack surface (data breaches, subpoenas)

However, the server does need to log some information for operational purposes — particularly for fail2ban integration to detect and block abusive connections.

## Decision
The server does NOT log:
- `channel_open_direct_tcpip` destinations (host, port)
- DNS resolutions performed by the server on behalf of clients
- Bytes transferred through tunnel channels
- Connection duration or throughput

The server DOES log (ADR-013):
- Auth attempts (remote_addr, user, key_fingerprint, accept/reject)
- Connection opened (remote_addr, transport kind)
- Connection closed (remote_addr, duration)

This separation ensures fail2ban has enough data to detect abusive IPs while destination privacy is maintained.

## Consequences
- **Positive**: Tunnel destinations are never written to disk or any observable log. This is the same guarantee OpenSSH makes with `LogLevel VERBOSE` or below.
- **Positive**: Reduces legal and privacy exposure for server operators.
- **Positive**: fail2ban can still work — it needs source IPs and auth failures, not destinations.
- **Negative**: Server operators cannot audit what destinations clients are accessing. If an operator needs this for compliance, they must implement it outside alknet (e.g., network-level logging at the target host).
- **Negative**: Debugging connectivity issues is harder without destination logs. Mitigated by client-side logging (the client knows what it's connecting to).

## References
- [server.md](../server.md)
- [ADR-013](013-fail2ban-friendly-logging.md) — what the server does log