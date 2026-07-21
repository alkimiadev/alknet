# OQ-070: `no_std` + `alloc` support

- **Origin**: [crates/typedef/overview.md](crates/typedef/overview.md);
  `docs/research/alknet-typedef/findings.md` §"Open Questions" (OQ 6)
- **Status**: deferred(scope)
- **Door type**: Two-way (additive — can be added as a feature gate
  without changing the existing `std` API)
- **Priority**: low
- **Impacts**: Blocks bare-metal embedded targets (microcontrollers
  running Rust without `std`). Does NOT block any current deployment
  target. Does NOT block WASM — `wasm32-unknown-unknown` has `std`
  available via `wasm-bindgen`; the crate is WASM-clean by construction
  (no tokio, no platform deps, `jsonschema` builds for WASM with
  `default-features = false`).
- **Blocked on**: An embedded use case that requires `no_std` + `alloc`
  (e.g., a microcontroller running Rust without `std`).
- **Resolution**: Not yet decidable. Target `std` for v1. If embedded
  use cases emerge, `no_std` + `alloc` can be added as a feature gate
  later. The engine's core (offset computation, read/write) is already
  allocation-free — it operates on `&[u8]` slices. The `jsonschema`
  dependency is the only `alloc` consumer.
- **Cross-references**: ADR-095
