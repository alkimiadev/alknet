# OQ-25: ~~Remote-Safe Marking Shape for CallClient Peer-Scoped Filtering~~ (Dissolved by ADR-029)

- **Origin**: [client-and-adapters.md](crates/call/client-and-adapters.md), ADR-017 (§1 Consequences), ADR-028
- **Status**: **dissolved** (ADR-029)
- **Door type**: ~~Two-way (shape only — existence is one-way, resolved by ADR-028)~~
- **Priority**: ~~medium~~
- **Resolution**: **Dissolved by [ADR-029](decisions/029-peer-graph-routing-model.md).**
  ADR-028's `remote_safe: bool` / `trusted_peer` model is superseded — it was a
  parallel, weaker authorization system that duplicated the existing
  `AccessControl`/`Identity` machinery. ADR-029 retires `remote_safe`/
  `trusted_peer` entirely; peer authorization flows through
  `AccessControl::check(peer_identity)`. The op's `AccessControl` *is* the
  peer-authorization policy — there is no separate marking. Per-peer
  differentiation is via `IdentityProvider` config (different peers get
  different scopes), not a per-op boolean. The "shape" question is moot
  because there is no marking to shape. See ADR-029 §3.
- **Cross-references**: ADR-009, ADR-014, ADR-015, ADR-017, ADR-022, ADR-024,
  ~~ADR-028~~ (superseded), ADR-029, [client-and-adapters.md](crates/call/client-and-adapters.md),
  [operation-registry.md](crates/call/operation-registry.md)
