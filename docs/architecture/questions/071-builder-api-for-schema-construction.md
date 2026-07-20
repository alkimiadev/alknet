# OQ-071: Builder API for schema construction

- **Origin**: [crates/typedef/schema-layer.md](crates/typedef/schema-layer.md),
  [crates/typedef/overview.md](crates/typedef/overview.md);
  `docs/research/alknet-typedef/findings.md` (the builder API was noted
  as the one detail not covered by the POCs)
- **Status**: deferred(scope)
- **Door type**: Two-way (additive — a builder API can be added without
  changing the existing JSON-consumption path)
- **Priority**: medium
- **Impacts**: No current consumer. Schemas are authored in TypeBox (JS)
  or hand-written JSON for v1. A Rust builder API would enable
  programmatic schema construction in Rust without depending on a JS
  toolchain, but no current consumer needs this.
- **Blocked on**: A concrete need for programmatic schema construction
  in Rust. The current consumers (SFTP, metatensor, binary call frames,
  TTY negotiation) all have schemas that can be hand-written or generated
  from TypeBox.
- **Resolution**: Not yet decidable. The builder API is important but
  not needed for the initial consumers. The engine's JSON-consumption
  path is the primary interface for v1. A builder API would be a fluent
  Rust API that produces the same JSON Schema structure — it would sit
  on top of the engine, not inside it.
- **Cross-references**: ADR-095, [schema-layer.md](crates/typedef/schema-layer.md)
