# ADR-008: ACME/Let's Encrypt Certificate Provisioning

## Status
Accepted

## Context
TLS transport mode requires certificates. Manual certificate management is error-prone — users need to obtain, install, and renew certificates. Our production setup uses certbot with Let's Encrypt (documented in `/workspace/system/dev1/certbot.md`), which automates this via the ACME protocol.

There are two ACME flows:
1. **Domain-based**: Standard flow with DNS-01 or HTTP-01 challenge. Certificate is tied to a domain name, auto-renews via certbot/systemd timer. Requires port 80 or DNS access for challenges.
2. **IP-based**: Short-lived certificates via TLS-ALPN-01 challenge on port 443. No domain needed, but cert is short-lived (days, not months). Simpler for quick setups but requires the ACME client to run continuously.

Both flows are important for alknet's usability. Without ACME, TLS mode requires manual cert setup — a significant barrier for users who want "SSH over port 443" for censorship resistance.

## Decision
Support both ACME certificate provisioning paths:

1. **Domain-based ACME** (`--acme-domain <domain>`): Standard certbot-style flow. Certificate is domain-bound, auto-renews. The server runs a challenge responder (HTTP-01 on port 80 or TLS-ALPN-01 on port 443) during certificate issuance/renewal.

2. **IP-based ACME**: Short-lived certs for servers without a domain. Uses TLS-ALPN-01 challenge on port 443. Lower burden but certs expire frequently.

3. **Manual certs** (`--tls-cert` / `--tls-key`): Always supported for users with existing certificates or specific PKI setups.

The implementation should use the `rustls-acme` crate (or similar pure-Rust ACME client) to avoid an external certbot dependency. This keeps alknet self-contained as a single binary.

## Consequences
- **Positive**: Users can run `alknet serve --transport tls --acme-domain example.com` and get working TLS with zero manual cert management.
- **Positive**: IP-based ACME covers the quick-setup case without requiring a domain.
- **Positive**: Consistent with our production infrastructure (certbot + Let's Encrypt is already our standard).
- **Negative**: ACME adds complexity to the server binary (challenge responder, cert store, renewal timer).
- **Negative**: IP-based short-lived certs require more frequent renewal handling.
- **Negative**: Binary size increases with ACME support (rustls-acme dependency). Consider making ACME a feature flag (`acme`).

## References
- [server.md](../server.md)
- [OQ-01](../open-questions.md) — resolved by this ADR
- [OQ-07](../open-questions.md) — resolved by this ADR
- Production certbot setup: `/workspace/system/dev1/certbot.md`