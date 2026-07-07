---
id: tty/crate-init
name: Initialize alknet-tty crate with Cargo.toml, dependencies, and module skeleton
status: pending
depends_on: []
scope: moderate
risk: low
impact: project
level: implementation
---

## Description

Initialize the `alknet-tty` crate from scratch. This is the lean core crate for
the `alknet/tty` ALPN: the wire format, the `TtyBackend` trait, and the
`TtyAdapter` (`ProtocolHandler`). It depends on `alknet-core` only — no
`portable_pty`, no `bollard`, no `russh`, no `alknet-call` (ADR-057). The local
backend lives in a sibling crate (`alknet-tty-local`, ADR-054) behind a `local`
feature re-export.

### Crate setup

Create `crates/alknet-tty/` with:

- `Cargo.toml` — package metadata, dependencies, feature flags
- `src/lib.rs` — crate root with module declarations and re-exports
- Module skeleton files (empty or with `// TODO` markers) for:
  - `src/wire.rs` — `ChunkReader`/`ChunkWriter`, `Chunk`, `RawError`, stream-type
    constants, `MAX_CHUNK_LEN`
  - `src/control.rs` — `ControlMessage` tagged enum, `signal_from_name`
  - `src/negotiation.rs` — `NegotiateRequest`, `TerminalParamsWire`,
    length-prefixed framing reader/writer, error response shape
  - `src/backend.rs` — `TtyBackend` trait, `TtyHandle`, `TtyControl`,
    `TtyControlHandle`, `TtyParams`, `TerminalParams`, `TtyError`
  - `src/adapter.rs` — `TtyAdapter` (`ProtocolHandler` on `alknet/tty`),
    `drive_session` three-pump driver

### Dependencies

Per the architecture specs (overview.md, tty-wire.md, tty-backend.md):

| Crate | Purpose |
|-------|---------|
| `alknet-core` | `ProtocolHandler`, `Connection`, `AuthContext`, `Identity`, `HandlerError`, `OwnershipProvider` (workspace path) |
| `tokio` 1 (full) | Async runtime, mpsc/oneshot channels, `AsyncRead`/`AsyncWrite` |
| `bytes` 1 | `Bytes` for chunk payloads and stdout streams |
| `futures-core` | `Stream` trait for `TtyHandle.stdout`/`stderr` |
| `tokio-stream` | `StreamExt` re-export of `futures_core::Stream` for extension methods |
| `serde` 1 | Serialization for `NegotiateRequest`, `ControlMessage` |
| `serde_json` 1 | JSON for negotiation frame, control channel, `backend_params` map |
| `async-trait` 0.1 | `TtyBackend` trait (async fn in trait) |
| `tracing` 0.1 | Structured logging |
| `thiserror` 2 | Error enums (`TtyError`, `RawError`) |

No `portable_pty`, no `bollard`, no `russh`, no `alknet-call`. The negotiation
framing is self-contained (~30 lines, ADR-057).

### Feature flags

```toml
[features]
default = []
local = ["dep:alknet-tty-local"]   # re-export LocalTtyBackend from alknet-tty-local
```

- `default` — the wire format, `TtyAdapter`, and the `TtyBackend` trait. No
  backend implementations; the assembly layer registers backends from their own
  crates.
- `local` — re-export `alknet_tty_local::LocalTtyBackend` as
  `alknet_tty::local::LocalTtyBackend`. Pulls in `alknet-tty-local` (which pulls
  in `portable_pty`). Wired in task `tty/local-feature-reexport`.

### Workspace Cargo.toml

Add `crates/alknet-tty` to the workspace `members` list in the root `Cargo.toml`.

### Module skeleton

```rust
// src/lib.rs
//! alknet-tty: Terminal session protocol handler for the `alknet/tty` ALPN.
//!
//! Two-carriage model (ADR-052): a JSON negotiation frame, then raw chunks
//! (`[stream_type: u8][length: u32 be][payload]`). Backend-agnostic via the
//! `TtyBackend` trait (ADR-053). Depends on alknet-core only (ADR-057).

pub mod wire;
pub mod control;
pub mod negotiation;
pub mod backend;
pub mod adapter;

// Re-exports filled in by subsequent tasks.
```

Each module file gets a doc comment and `// TODO: implement` marker. The
subsequent tasks (wire-codec, control-messages, negotiation, backend-trait,
adapter) fill these in.

## Acceptance Criteria

- [ ] `crates/alknet-tty/Cargo.toml` exists with all dependencies and the `local` feature gate
- [ ] `crates/alknet-tty/src/lib.rs` exists with module declarations
- [ ] Module skeleton files exist: `wire.rs`, `control.rs`, `negotiation.rs`, `backend.rs`, `adapter.rs`
- [ ] Root `Cargo.toml` `members` list includes `crates/alknet-tty`
- [ ] `cargo check -p alknet-tty` succeeds
- [ ] `cargo clippy -p alknet-tty` succeeds with no warnings
- [ ] Dual licensing: `MIT OR Apache-2.0` (workspace-inherited)
- [ ] `alknet-core` dependency uses workspace path (`path = "../alknet-core"`)
- [ ] No `portable_pty`, `bollard`, `russh`, or `alknet-call` dependency present
- [ ] `local` feature gate declared but `alknet-tty-local` dependency not yet wired (added in `tty/local-feature-reexport`)

## References

- docs/architecture/crates/tty/README.md — crate index
- docs/architecture/crates/tty/overview.md — crate overview, dependencies, feature gates, backend location map
- docs/architecture/decisions/003-crate-decomposition.md — ADR-003 (Amendment 2: alknet-tty depends on alknet-core only)
- docs/architecture/decisions/052-alknet-tty-wire-format-and-two-carriage.md — ADR-052
- docs/architecture/decisions/054-local-tty-backend-sibling-crate.md — ADR-054 (sibling crate behind `local` feature)
- docs/architecture/decisions/057-alknet-tty-no-alknet-call-dep.md — ADR-057 (no alknet-call dependency)

## Notes

> This is the foundational setup task for alknet-tty. All subsequent tty tasks
> depend on this one. The crate is intentionally lean — no backend deps. The
> `local` feature gate is declared here but the `alknet-tty-local` dependency
> is wired in a later task (`tty/local-feature-reexport`) once the sibling crate
> exists. The POC at `/workspace/alknet-tty-poc/` is the reference implementation
> for the wire codec, control messages, and the local PTY bridge.

## Summary

> To be filled on completion