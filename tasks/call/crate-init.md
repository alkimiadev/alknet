---
id: call/crate-init
name: Initialize alknet-call crate with Cargo.toml, dependencies, and module skeleton
status: pending
depends_on: [core/core-types]
scope: moderate
risk: low
impact: project
level: implementation
---

## Description

Initialize the `alknet-call` crate from scratch. This crate implements the call
protocol (structured RPC over QUIC) on ALPN `alknet/call`. It depends on
alknet-core (for ProtocolHandler, Connection, AuthContext, Capabilities,
IdentityProvider) and irpc (for framing).

### Crate setup

Create `crates/alknet-call/` with:

- `Cargo.toml` — package metadata, dependencies
- `src/lib.rs` — crate root with module declarations and re-exports
- Module skeleton files for:
  - `src/registry/mod.rs` — registry module root
  - `src/registry/spec.rs` — OperationSpec, OperationType, Visibility, ErrorDefinition, AccessControl
  - `src/registry/context.rs` — OperationContext, AbortPolicy, CompositionAuthority, ScopedOperationEnv
  - `src/registry/registration.rs` — Handler, HandlerRegistration, OperationProvenance, OperationRegistry, OperationRegistryBuilder
  - `src/registry/env.rs` — OperationEnv trait, LocalOperationEnv, CompositeOperationEnv
  - `src/registry/discovery.rs` — services/list, services/schema handlers
  - `src/protocol/mod.rs` — protocol module root
  - `src/protocol/wire.rs` — EventEnvelope, ResponseEnvelope, CallError, framing
  - `src/protocol/pending.rs` — PendingRequestMap, PendingEntry
  - `src/protocol/connection.rs` — CallConnection
  - `src/protocol/adapter.rs` — CallAdapter (ProtocolHandler impl)
  - `src/protocol/abort.rs` — abort cascade logic

### Dependencies

| Crate | Purpose |
|-------|---------|
| `alknet-core` | ProtocolHandler, Connection, AuthContext, Capabilities, IdentityProvider, Identity, HandlerError (workspace path) |
| `irpc` | Framing, service dispatch (workspace dep) |
| `tokio` 1 (full) | Async runtime, sync primitives (oneshot, mpsc, watch) |
| `serde` 1 | Serialization for wire types |
| `serde_json` 1 | JSON wire format, JSON Schema values |
| `async-trait` 0.1 | OperationEnv trait (async fn in trait) |
| `tracing` 0.1 | Structured logging |
| `thiserror` 2 | Error enums |
| `uuid` 1 | Request ID generation (UUID v4) |
| `futures` | Stream trait for subscribe |

### Workspace Cargo.toml

Add `crates/alknet-call` to the workspace `members` list in the root
`Cargo.toml`.

### Module skeleton

```rust
// src/lib.rs
//! alknet-call: Structured RPC over QUIC — operations, streaming, service discovery.
//! Implements ProtocolHandler on ALPN `alknet/call`.

pub mod registry;
pub mod protocol;

// Re-exports (filled in by subsequent tasks)
```

Each module file gets a doc comment and `// TODO: implement` marker.

## Acceptance Criteria

- [ ] `crates/alknet-call/Cargo.toml` exists with all dependencies
- [ ] `crates/alknet-call/src/lib.rs` exists with module declarations
- [ ] All module skeleton files exist (registry/*, protocol/*)
- [ ] Root `Cargo.toml` `members` list includes `crates/alknet-call`
- [ ] `cargo check -p alknet-call` succeeds
- [ ] `cargo clippy -p alknet-call` succeeds with no warnings
- [ ] Dual licensing: `MIT OR Apache-2.0` (workspace-inherited)
- [ ] alknet-core dependency uses workspace path (`path = "../alknet-core"`)

## References

- docs/architecture/crates/call/README.md — crate index
- docs/architecture/crates/call/call-protocol.md — CallAdapter, wire format
- docs/architecture/crates/call/operation-registry.md — registry, OperationEnv
- docs/architecture/decisions/003-crate-decomposition.md — ADR-003
- docs/architecture/decisions/005-irpc-as-call-protocol-foundation.md — ADR-005

## Notes

> alknet-call depends on alknet-core (for ProtocolHandler, Connection,
> AuthContext, Capabilities, IdentityProvider) and irpc (for framing). The
> crate has two subsystems: registry (operation specs, context, dispatch) and
> protocol (wire format, streams, adapter). The module structure reflects
> this split.

## Summary

> To be filled on completion