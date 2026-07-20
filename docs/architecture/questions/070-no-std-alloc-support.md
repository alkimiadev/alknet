# OQ-070: `no_std` + `alloc` support

- **Origin**: [crates/typedef/overview.md](crates/typedef/overview.md);
  `docs/research/alknet-typedef/findings.md` §"Open Questions" (OQ 6)
- **Status**: deferred(scope)
- **Door type**: Two-way (additive — can be added as a feature gate
  without changing the existing `std` API)
- **Priority**: low
- **Impacts**: Blocks embedded/WASM-bare-metal deployment targets (microcontrollers, `no_std` environments). Does NOT block WASM-browser (has `std` via `wasm-bindgen`). Does NOT block any current deployment target.
- **Blocked on**: An embedded use case that requires `no_std` + `alloc`
  (e.g., a microcontroller running Rust without `std`).
- **Resolution**: Not yet decidable. Target `std` for v1. If embedded
  use cases emerge, `no_std` + `alloc` can be added as a feature gate
  later. The engine's core is already allocation-free; the `jsonschema`
  dependency is the only `alloc` consumer.
- **Cross-references**: ADR-095
