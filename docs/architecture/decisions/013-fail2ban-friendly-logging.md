# ADR-013: Fail2ban-Friendly Server Logging

## Status
Accepted

## Context
The server needs to handle abuse on public-facing deployments. Our production infrastructure uses fail2ban on Linux (documented in `/workspace/system/dev1/fail2ban.md`) with nftables and systemd journal backend. fail2ban needs structured, parseable logs to identify abusive IP addresses.

However, fail2ban is Linux-specific. On other platforms (macOS, Windows, BSD), users need a different approach to reject abusive connections. The server should provide enough logging for fail2ban on Linux and enough built-in protection for other platforms.

## Decision
The server logs connection and authentication events at `INFO` level with structured fields, and provides a configurable connection rate limiter as a built-in defense.

**Logging** (for fail2ban integration on Linux):
- Log auth attempts: `level=INFO, msg="auth attempt", remote_addr=<ip>, user=<user>, key_fingerprint=<sha256>, result=<accept|reject>`
- Log new connections: `level=INFO, msg="connection opened", remote_addr=<ip>, transport=<tcp|tls|iroh>`
- Log disconnections: `level=INFO, msg="connection closed", remote_addr=<ip>, duration=<secs>`
- Do NOT log: channel open targets, DNS resolutions, bytes transferred

This matches what fail2ban needs: source IP + failure indicator. Our existing fail2ban setup filters on similar fields for SSH and nginx.

**Built-in rate limiting** (for all platforms):
- `--max-connections-per-ip <n>` (default: 0 = unlimited) — reject new connections from an IP that already has N active connections
- `--max-auth-attempts <n>` (default: 10) — disconnect after N failed auth attempts from one connection
- Rate limiting happens at the SSH layer, before channels are opened

This ensures that even without fail2ban, the server rejects obviously abusive connections.

## Consequences
- **Positive**: fail2ban can parse alknet logs the same way it parses SSH and nginx logs on our production systems.
- **Positive**: Built-in rate limiting provides protection on platforms without fail2ban.
- **Positive**: No privacy-sensitive data in logs (no tunnel destinations).
- **Negative**: Slightly more code in the server for connection tracking per IP.
- **Negative**: Users with custom fail2ban filters need to write regex for alknet's log format (documented examples provided).

## References
- [server.md](../server.md)
- [OQ-08](../open-questions.md) — resolved by this ADR
- Production fail2ban setup: `/workspace/system/dev1/fail2ban.md`