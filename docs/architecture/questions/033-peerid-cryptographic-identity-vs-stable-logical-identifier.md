# OQ-33: PeerId — Cryptographic Identity vs Stable Logical Identifier

- **Origin**: [ADR-029](decisions/029-peer-graph-routing-model.md) Assumption 1, `docs/research/alknet-call-peer-routing/findings.md` §6.1
- **Status**: **resolved** (2026-06-27 by ADR-030)
- **Door type**: One-way (composition semantics), two-way (id source)
- **Priority**: high
- **Resolution**: `PeerId` is a **logical identifier, decoupled from the
  cryptographic identity**. It is *not* the raw fingerprint or API-key
  prefix — those change on key rotation, which would break every
  in-flight `PeerRef::Specific` and every ACL entry referencing that peer.

  ADR-029 established the one-way door (`PeerId` is logical, not crypto)
  with a v1 UUID source as a no-storage workaround. **ADR-030 supersedes
  the UUID source**: `Identity.id` becomes `PeerEntry.peer_id` (stable
  across key rotation) on the fingerprint path, and `PeerId =
  Identity.id` from `IdentityProvider` resolution. The UUID workaround is
  removed — the stable logical id is the real thing, sourced from the auth
  system, not an ephemeral connection-assigned value.

  The `PeerEntry` config model (`peer_id`, `fingerprint`, `scopes`,
  `resources`, `display_name`, `enabled`) lives in `AuthPolicy`. Key
  rotation is a single `PeerEntry.fingerprint` update — the `peer_id`,
  ACL entries, and `PeerRef::Specific` references stay stable. The
  no-DB posture is preserved (core has the trait + the in-memory
  `ConfigIdentityProvider` adapter; persistence adapters are additive
  separate crates, ADR-033).

  **The one-way door (preserved from ADR-029):** `PeerId` is a logical id,
  not `Identity.id` (the fingerprint). This determines the
  `PeerCompositeEnv` key type, the `PeerRef::Specific` payload type, and
  the `ScopedPeerEnv.peer_pinned` entry shape. The *source* of the logical
  id (ADR-029's UUID → ADR-030's `PeerEntry.peer_id`) was the two-way-door
  remainder; it is now resolved.
- **Cross-references**: ADR-009, ADR-014, ADR-015, ADR-017, ADR-021, ADR-027,
  ADR-029, ADR-030, OQ-34, OQ-35, [client-and-adapters.md](crates/call/client-and-adapters.md),
  [operation-registry.md](crates/call/operation-registry.md),
  [auth.md](crates/core/auth.md)
