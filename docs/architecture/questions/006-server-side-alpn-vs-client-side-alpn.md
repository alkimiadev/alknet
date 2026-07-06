# OQ-06: Server-Side ALPN vs Client-Side ALPN

- **Origin**: ADR-001
- **Status**: resolved
- **Door type**: One-way
- **Priority**: low
- **Resolution**: One ALPN per connection. Clients open one QUIC connection per ALPN. QUIC connections are cheap (multiplexed over the same UDP flow). See ADR-006.
- **Cross-references**: ADR-001, ADR-006
