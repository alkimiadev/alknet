---
id: tty-local/crate-init
name: Initialize alknet-tty-local crate with Cargo.toml, dependencies, and module skeleton
status: completed
depends_on: [tty/backend-trait]
scope: moderate
risk: low
impact: project
level: implementation
---

## Description

Initialize the `alknet-tty-local` sibling crate (ADR-054). This crate
implements `TtyBackend` for `LocalTtyBackend` via `portable_pty` (PTY mode)
and `std::process::Command` (pipe/runner mode). It depends on `alknet-tty`
for the trait and types; the heavy `portable_pty` dependency lives here, not
in the core crate.

### Crate setup

Create `crates/alknet-tty-local/` with:

- `Cargo.toml` — package metadata, dependencies
- `src/lib.rs` — crate root with module declarations and re-exports
- Module skeleton files for:
  - `src/pty.rs` — PTY mode: three-thread bridge, `PtyControl`, REQ-TTY-02
    signal forwarding, ADR-056 kill guard
  - `src/pipe.rs` — pipe mode: `tokio::process`, `PipeControl`, ADR-056 kill
    guard
  - `src/backend.rs` — `LocalTtyBackend` implementing `TtyBackend`, branching
    on `TtyParams.terminal`

### Dependencies

Per the architecture spec (tty-local.md §"Dependencies"):

| Crate | Purpose |
|-------|---------|
| `alknet-tty` | `TtyBackend` trait, `TtyHandle`, `TtyControl`, `TtyControlHandle`, `TtyParams`, `TerminalParams`, `TtyError`, `ControlMessage`, `signal_from_name` (workspace path) |
| `portable_pty` | PTY allocation — Unix `openpty` + Windows ConPTY (the heavy dep) |
| `libc` | Signal forwarding — REQ-TTY-02, Unix only |
| `tokio` 1 (full) | mpsc, oneshot, `AsyncRead`/`AsyncWrite` for the pipe case |
| `bytes` 1 | `Bytes` for stdout/stderr streams |
| `futures-core` | `Stream` trait for `TtyHandle.stdout`/`stderr` |
| `tracing` 0.1 | Structured logging |
| `thiserror` 2 | Error conversion (if needed beyond `TtyError`) |

`alknet-tty-local` does NOT depend on `alknet-core` directly (it accesses core
types via `alknet-tty`'s re-exports if needed). `portable_pty` and `libc` are
the heavy deps that motivate the sibling-crate split (ADR-054).

### Feature flags

No feature flags — the crate is unconditional. The `local` feature gate lives
on `alknet-tty` (the consumer enables it to pull this crate in).

### Workspace Cargo.toml

Add `crates/alknet-tty-local` to the workspace `members` list in the root
`Cargo.toml`.

### Module skeleton

```rust
// src/lib.rs
//! alknet-tty-local: Local TTY backend for alknet-tty.
//!
//! `LocalTtyBackend` implements `alknet_tty::TtyBackend` via `portable_pty`
//! (PTY mode, terminal semantics) and `std::process::Command` (pipe mode,
//! the runner case). The blocking→async bridge for PTY mode uses three
//! dedicated std threads feeding tokio mpsc/oneshot channels (REQ-TTY-01).
//! Signal forwarding targets the foreground process group (REQ-TTY-02).

pub mod pty;
pub mod pipe;
pub mod backend;

pub use backend::LocalTtyBackend;
```

Each module file gets a doc comment and `// TODO: implement` marker.

## Acceptance Criteria

- [ ] `crates/alknet-tty-local/Cargo.toml` exists with all dependencies
- [ ] `crates/alknet-tty-local/src/lib.rs` exists with module declarations and `pub use backend::LocalTtyBackend`
- [ ] Module skeleton files exist: `pty.rs`, `pipe.rs`, `backend.rs`
- [ ] Root `Cargo.toml` `members` list includes `crates/alknet-tty-local`
- [ ] `cargo check -p alknet-tty-local` succeeds
- [ ] `cargo clippy -p alknet-tty-local` succeeds with no warnings
- [ ] Dual licensing: `MIT OR Apache-2.0` (workspace-inherited)
- [ ] `alknet-tty` dependency uses workspace path (`path = "../alknet-tty"`)
- [ ] `portable_pty` and `libc` dependencies present
- [ ] No `alknet-core` direct dependency (accessed via `alknet-tty` if needed)

## References

- docs/architecture/crates/tty/tty-local.md — the authoritative spec for this crate
- docs/architecture/decisions/054-local-tty-backend-sibling-crate.md — ADR-054 (sibling crate placement)
- docs/architecture/decisions/053-ttybackend-trait-and-ttyhandle.md — ADR-053 (the trait this crate implements)
- /workspace/alknet-tty-poc/src/local_pty.rs — the reference PTY implementation

## Notes

> This crate can be initialized in parallel with the remaining core crate
> tasks (`tty/control-messages`, `tty/negotiation`, `tty/adapter`) since it
> only depends on `tty/backend-trait` for the trait and types. The
> `portable_pty` dependency is the heavy dep that motivates the sibling-crate
> split — a docker-only deployment doesn't pull it in. The POC's
> `local_pty.rs` is the reference for the PTY mode; the pipe mode is simpler
> (tokio's `Child` is natively async).

## Summary

> To be filled on completion