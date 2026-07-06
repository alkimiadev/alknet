# OQ-34: Persistent Peer Registry (Cross-Node State Storage)

- **Origin**: OQ-33 (the storage dimension it surfaced), the no-DB posture of ADR-008/018/025
- **Status**: **resolved** (2026-06-27 by ADR-030 + ADR-031 + ADR-033)
- **Door type**: One-way (storage boundary), two-way (backend choice)
- **Priority**: ~~medium (not a v1 blocker)~~ → resolved
- **Resolution**: The storage boundary is: **core defines repo traits +
  in-memory default adapters; persistence adapters are separate crates;
  the assembly layer wires the adapter.** This is the repo/adapter
  pattern (ADR-033), already established by `IdentityProvider` (ADR-004)
  and now extended to `CredentialStore` (ADR-031).

  - `IdentityProvider` (ADR-004) — the auth repo trait, in core.
    `ConfigIdentityProvider` is the in-memory default, backed by
    `AuthPolicy.peers` (ADR-030). A future `alknet-peer-store-sqlite`
    adapter that persists `PeerEntry` records in a `peers` table is
    additive — it implements the same trait.
  - `CredentialStore` (ADR-031) — the credential repo trait, in core.
    `InMemoryCredentialStore` is the in-memory default. A future
    persistence adapter is additive.

  The no-DB posture of the core crates is preserved in the sense that
  matters: core has **no backend dependency** (no SQLite, no honker). The
  in-memory default adapters carry no persistence. The persistence
  adapters are additive crates, built when a concrete use case forces
  them, wired by the assembly layer.

  The concrete adapter shapes (table schemas, backend choice, indexing,
  caching) were the two-way-door remainder, tracked as OQ-36 — **now
  resolved by [ADR-035](decisions/035-concrete-persistence-adapter-shapes.md)**
  (read/write split, honker+SQLite, `alknet-store-sqlite` crate). The
  trait shapes are the one-way door, committed by ADR-030, ADR-031, and
  ADR-033; ADR-035 builds on them.
- **Cross-references**: ADR-008, ADR-018, ADR-021, ADR-025, ADR-029,
  ADR-030, ADR-031, ADR-033, ADR-035, OQ-33, OQ-36,
  [auth.md](crates/core/auth.md), [config.md](crates/core/config.md)
