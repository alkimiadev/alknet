# ADR-009: Default iroh Relay with Override

## Status
Accepted

## Context
iroh requires a relay server for NAT traversal and initial connection establishment. The n0 project provides free relay servers (`https://relay.iroh.network/`) that work out of the box. However, relying on a third-party service creates a dependency:

- n0's relay could change terms, rate-limit, or go down
- Production deployments may want self-hosted relays for reliability and privacy
- The relay URL is a configuration point that should be explicit

Conversely, requiring users to set up a relay server before they can use iroh transport is a significant friction point for testing and quick starts.

## Decision
Default to n0's relay servers. Allow override via `--iroh-relay <url>` CLI flag. Document self-hosted relay setup in project documentation.

This matches iroh's own defaults — n0's relay is the standard starting point. Users who need production reliability self-host.

## Consequences
- **Positive**: Zero-config iroh transport for testing and development. `alknet serve --transport iroh` just works.
- **Positive**: Self-hosting is a single flag override, not a complex setup requirement.
- **Negative**: Default depends on n0's infrastructure. If n0's relay is down, default iroh connections fail (but this is the same experience as every iroh user).
- **Negative**: Privacy-conscious users must remember to `--iroh-relay` to avoid n0. Mitigated by documentation.

## References
- [transport.md](../transport.md)
- [OQ-02](../open-questions.md) — resolved by this ADR