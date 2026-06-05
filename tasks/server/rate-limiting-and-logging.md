---
id: server/rate-limiting-and-logging
name: Implement server rate limiting and fail2ban-friendly structured logging
status: pending
depends_on:
  - server/handler
scope: narrow
risk: low
impact: component
level: implementation
---

## Description

Implement the two-layer abuse protection per ADR-013:

1. **Structured logging** at INFO level for fail2ban integration: auth attempts (remote_addr, user, key_fingerprint, accept/reject), connection opened/closed (remote_addr, transport, duration)
2. **Built-in rate limiting**: `--max-connections-per-ip` (reject new connections from IPs with N active connections), `--max-auth-attempts` (disconnect after N failed auth attempts per connection)

No logging of tunnel destinations, DNS resolutions, or bytes transferred (ADR-006).

## Acceptance Criteria

- [ ] `crates/alknet-core/src/server/rate_limit.rs` exports connection rate limiter
- [ ] `ConnectionRateLimiter` tracks active connections per IP using `HashMap<IpAddr, usize>`
- [ ] `ConnectionRateLimiter::check(ip) -> bool` — returns `true` if connection allowed, `false` if over limit
- [ ] `ConnectionRateLimiter::on_connect(ip)` — increment counter
- [ ] `ConnectionRateLimiter::on_disconnect(ip)` — decrement counter
- [ ] `AuthAttemptLimiter` tracks failed auth attempts per connection
- [ ] `AuthAttemptLimiter::check() -> bool` — returns `true` if under limit
- [ ] `AuthAttemptLimiter::on_failure()` — increment failure counter
- [ ] Structured `tracing::info!` logging on: auth attempt, connection opened, connection closed
- [ ] Log format includes key-value pairs: `remote_addr`, `user`, `key_fingerprint`, `result`, `transport`, `duration`
- [ ] No logging of: channel open targets, DNS resolutions, bytes transferred
- [ ] Integration with `ServerHandler`: rate limiter checked before auth, auth attempt limiter checked during auth
- [ ] Unit tests: connection limit enforced, auth attempt limit enforced, log format verification

## References

- docs/architecture/server.md — Logging and Rate Limiting section
- docs/architecture/decisions/013-fail2ban-friendly-logging.md — logging format, rate limiting flags
- docs/architecture/decisions/006-no-logging-of-tunnel-destinations.md — no destination logging

## Notes

> To be filled by implementation agent

## Summary

> To be filled on completion