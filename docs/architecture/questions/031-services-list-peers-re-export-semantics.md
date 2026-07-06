# OQ-31: services/list-peers Re-Export Semantics

- **Origin**: [ADR-029](decisions/029-peer-graph-routing-model.md) §6, `docs/research/alknet-call-peer-routing/findings.md` §3.5
- **Status**: **resolved** (2026-06-27)
- **Door type**: Two-way
- **Priority**: low
- **Resolution**: `services/list` defaults to **"own ops only"** — it shows
  the head's own Layer 0 `External` ops, filtered by
  `AccessControl::check(calling_peer)`, unchanged from today (minus the
  retired `remote_safe` filter). A `services/list-peers` opt-in (new
  built-in operation) lists the peer overlays with attribution: each
  peer's sub-overlay listed as `{ peer: Option<PeerId>, operations: [...] }`,
  filtered by the calling peer's authorization. The re-export policy is an
  `AccessControl` decision on the listing op. Whether `services/list-peers`
  is built now or as a feature addition is a scheduling question — the
  decision (opt-in, `AccessControl`-filtered) is made.
- **Cross-references**: ADR-029, [client-and-adapters.md](crates/call/client-and-adapters.md)
