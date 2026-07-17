---
id: endpoint/registry
name: Extract HandlerRegistry from alknet-core/endpoint.rs into alknet-endpoint
status: pending
depends_on: [endpoint/crate-init]
scope: narrow
risk: low
impact: component
level: implementation
---

## Description

Phase 2, Task 2 of the crate extraction. Extract the `HandlerRegistry` type from
`crates/alknet-core/src/endpoint.rs` (lines 66-116) into `crates/alknet-endpoint/src/registry.rs`.

The old code **stays** in `endpoint.rs` (duplicated) — no breakage. The new crate is
self-contained and builds standalone.

### Types to extract

From `endpoint.rs` lines 66-116:

| Type/Function | Lines | Destination |
|---------------|-------|-------------|
| `HandlerRegistry` struct | 66-68 | `registry.rs` |
| `HandlerRegistry::new()` | 71-75 | `registry.rs` |
| `HandlerRegistry::register()` | 77-86 | `registry.rs` |
| `HandlerRegistry::get()` | 88-90 | `registry.rs` |
| `HandlerRegistry::alpn_strings()` | 92-94 | `registry.rs` |
| `impl Default for HandlerRegistry` | 97-101 | `registry.rs` |
| `impl Debug for HandlerRegistry` | 103-116 | `registry.rs` |

### Adaptations

1. **Imports**: Update `crate::types::ProtocolHandler` to `alknet_core::types::ProtocolHandler`.
2. **No other changes**: The `HandlerRegistry` is a pure data structure with no transport deps.
   It maps ALPN byte strings to `ProtocolHandler` instances. No feature gates needed — it's
   always available regardless of which transports are enabled.

### Public API

```rust
// registry.rs

/// Maps ALPN byte strings to `ProtocolHandler` instances.
/// Registered statically at startup by the assembly layer; the endpoint
/// dispatches by looking up the negotiated ALPN.
pub struct HandlerRegistry {
    handlers: HashMap<&'static [u8], Arc<dyn ProtocolHandler>>,
}

impl HandlerRegistry {
    pub fn new() -> Self;
    /// Insert a handler. Panics if the ALPN is already registered.
    pub fn register(&mut self, handler: Arc<dyn ProtocolHandler>);
    /// Look up a handler by ALPN string.
    pub fn get(&self, alpn: &[u8]) -> Option<&Arc<dyn ProtocolHandler>>;
    /// Return all registered ALPN strings.
    pub fn alpn_strings(&self) -> Vec<Vec<u8>>;
}
```

### What stays in core

The old `HandlerRegistry` in `endpoint.rs` lines 66-116 is **not deleted** — it stays as a
duplicate. The prune happens in Phase 4. This task only adds code to `alknet-endpoint`.

## Acceptance Criteria

- [ ] `crates/alknet-endpoint/src/registry.rs` contains `HandlerRegistry` with all methods
- [ ] `HandlerRegistry::new()` creates an empty registry
- [ ] `HandlerRegistry::register()` inserts a handler, panics on duplicate ALPN
- [ ] `HandlerRegistry::get()` looks up by ALPN, returns `None` for unknown
- [ ] `HandlerRegistry::alpn_strings()` returns all registered ALPNs
- [ ] `Default` impl delegates to `new()`
- [ ] `Debug` impl lists ALPNs without exposing handler internals
- [ ] All imports use `alknet_core::` (not `crate::`)
- [ ] No feature gates (always available)
- [ ] `cargo check -p alknet-endpoint` succeeds
- [ ] `cargo clippy -p alknet-endpoint` succeeds with no warnings
- [ ] `cargo test -p alknet-core` still passes (old code untouched)

## References

- docs/research/alknet-crate-extraction/findings.md — Phase 2, registry module
- docs/architecture/crates/endpoint/README.md — HandlerRegistry spec
- crates/alknet-core/src/endpoint.rs — lines 66-116 (source code to extract)

## Notes

> This is the simplest extraction in Phase 2 — ~50 lines of pure data structure.
> `HandlerRegistry` has no transport deps and no feature gates. It's extracted
> first because `AlknetEndpoint::new()` takes it as a parameter. The old code in
> `endpoint.rs` is NOT deleted — that's Phase 4.

## Summary

> To be filled on completion
