# OQ-30: PeerRef::Any Routing Policy

- **Origin**: [ADR-029](decisions/029-peer-graph-routing-model.md) §2, [client-and-adapters.md](crates/call/client-and-adapters.md), `docs/research/alknet-call-peer-routing/findings.md` §3.2
- **Status**: **resolved** (2026-06-27)
- **Door type**: Two-way
- **Priority**: low
- **Resolution**: `PeerRef::Any` uses **insertion-order first-match** —
  deterministic but order-dependent (worker A connects before worker B →
  `Any` routes to A until A disconnects). This is the simplest routing
  policy and is correct for the immediate use case (the head picks the
  first worker that serves the op). A richer `RoutingPolicy` (round-robin,
  least-loaded, affinity) is a feature extension — the `PeerRef` enum is
  designed to compose with a `Route { selector, policy }` struct without
  breaking the `invoke_peer` signature. Adding a routing policy is
  non-breaking; it's a feature addition when a fan-out use case needs it,
  not an unmade architectural decision.
- **Cross-references**: ADR-029, [client-and-adapters.md](crates/call/client-and-adapters.md)
