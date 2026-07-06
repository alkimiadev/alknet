# OQ-28: from_call Namespace Collision Behavior

- **Origin**: [client-and-adapters.md](crates/call/client-and-adapters.md), ADR-017 §3
- **Status**: **resolved** (2026-06-27)
- **Door type**: Two-way
- **Priority**: low
- **Resolution**: ADR-017 §3's `FromCallConfig` namespace prefix is
  **optional, default no prefix, same-peer collision = error**. A node
  importing from a peer that exposes two ops with the same name should fail
  loudly rather than silently overwrite. This matches the default-deny,
  explicit-allow posture (ADR-015). The alternative (last-wins) would
  silently mask one op behind another, which is the kind of surprise the
  default-deny posture exists to avoid.

  **Cross-peer collision dissolved by ADR-029.** Under the peer-keyed
  overlay model, same name on different peers is fine — they live in
  separate peer sub-overlays, no collision, no prefix needed.
  `FromCallConfig::namespace_prefix` is optional local-naming sugar for
  when the importing node wants to expose a peer's ops under a different
  name *locally* — a local-naming concern, not a disambiguation concern.
  See ADR-029 §5.
- **Cross-references**: ADR-015, ADR-017, ~~ADR-028~~ (superseded), ADR-029,
  [client-and-adapters.md](crates/call/client-and-adapters.md)
