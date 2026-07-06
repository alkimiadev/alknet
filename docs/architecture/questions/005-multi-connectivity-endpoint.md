# OQ-05: Multi-Connectivity Endpoint

- **Origin**: [overview.md](overview.md)
- **Status**: resolved
- **Door type**: One-way
- **Priority**: high
- **Resolution**: `AlknetEndpoint` supports both `quinn::Endpoint` (public QUIC+TLS) and `iroh::Endpoint` (P2P relay-assisted) simultaneously, both optional and feature-gated. Both produce QUIC connections that dispatch through the same `HandlerRegistry` by ALPN string. These are not interchangeable transports — they serve fundamentally different deployment contexts (public IP vs NAT traversal). TCP is not an endpoint concern — bare TCP SSH is handled by the SSH handler directly. See ADR-010.
- **Cross-references**: ADR-001, ADR-010, [endpoint.md](crates/core/endpoint.md)
