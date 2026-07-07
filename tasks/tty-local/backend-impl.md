---
id: tty-local/backend-impl
name: Implement LocalTtyBackend (TtyBackend) branching on terminal Some/None
status: pending
depends_on: [tty-local/pty-mode, tty-local/pipe-mode]
scope: narrow
risk: low
impact: component
level: implementation
---

## Description

Implement `LocalTtyBackend` in `src/backend.rs`: the `TtyBackend` implementation
that branches on `TtyParams.terminal` to dispatch to PTY mode (task
`tty-local/pty-mode`) or pipe mode (task `tty-local/pipe-mode`).

### LocalTtyBackend

```rust
pub struct LocalTtyBackend;

impl LocalTtyBackend {
    pub fn new() -> Self;
}

#[async_trait]
impl TtyBackend for LocalTtyBackend {
    async fn allocate(&self, params: &TtyParams) -> Result<TtyHandle, TtyError> {
        match &params.terminal {
            Some(terminal) => pty::allocate_pty(
                terminal.clone(),
                params.cmd.clone(),
                params.cwd.clone(),
                params.env.clone(),
            ).await,
            None => pipe::allocate_pipe(
                params.cmd.clone(),
                params.cwd.clone(),
                params.env.clone(),
            ).await,
        }
    }

    fn resource_id(&self, _params: &TtyParams) -> Option<(&'static str, String)> {
        None  // local backend creates its own resource (process); no pre-existing resource
    }
}
```

`LocalTtyBackend` takes no constructor dependencies (unlike
`DockerTtyBackend` which wraps a `bollard::Docker` client). The
`portable_pty` system is process-global. The assembly layer constructs one
`LocalTtyBackend` and registers it as `"local"`.

### Validation

`allocate()` validates `params.cmd` is non-empty (the adapter already checks
this at negotiation, but the backend should fail gracefully if called
directly with an empty cmd — return `TtyError::AllocFailed`). The
`backend_params` map is ignored by the local backend (it has no
backend-specific selector fields); if a caller passes unexpected
`backend_params`, they are silently ignored (the local backend doesn't
define a typed params struct).

### Default trait methods

`resource_id()` returns `None` (the local backend creates its own resource —
a process — so there's no pre-existing resource for the ownership check). The
default impl on `TtyBackend` already returns `None`, so this can be omitted
unless an explicit impl is preferred for clarity.

### Tests

- **PTY dispatch**: `allocate` with `terminal: Some` → returns a `TtyHandle`
  with `stderr: None` and a `PtyControl`.
- **Pipe dispatch**: `allocate` with `terminal: None` → returns a `TtyHandle`
  with `stderr: Some` and a `PipeControl`.
- **Empty cmd**: `allocate` with empty `cmd` → `TtyError::AllocFailed`.
- **resource_id**: returns `None`.
- These can be lightweight (the heavy tests are in `pty-mode` and `pipe-mode`);
  this task verifies the dispatch wiring.

## Acceptance Criteria

- [ ] `LocalTtyBackend` struct with `new()` constructor (no deps)
- [ ] `impl TtyBackend for LocalTtyBackend` with `allocate()` branching on `params.terminal`
- [ ] `terminal: Some` → dispatches to `pty::allocate_pty`
- [ ] `terminal: None` → dispatches to `pipe::allocate_pipe`
- [ ] `resource_id()` returns `None` (or uses the default)
- [ ] Empty `cmd` → `TtyError::AllocFailed`
- [ ] `LocalTtyBackend` re-exported from `lib.rs` (`pub use backend::LocalTtyBackend`)
- [ ] Unit tests: PTY dispatch, pipe dispatch, empty cmd, resource_id
- [ ] `cargo test -p alknet-tty-local` succeeds
- [ ] `cargo clippy -p alknet-tty-local` succeeds with no warnings

## References

- docs/architecture/crates/tty/tty-local.md — §"PTY Mode", §"Pipe Mode", §"LocalTtyBackend takes no constructor dependencies"
- docs/architecture/crates/tty/tty-backend.md — `TtyBackend` trait, `resource_id()`
- docs/architecture/decisions/054-local-tty-backend-sibling-crate.md — ADR-054 (per-session PTY vs pipe choice)

## Notes

> This is the wiring task — the heavy lifting is in `pty-mode` and
> `pipe-mode`. The branch on `TtyParams.terminal` is the per-session choice
> (ADR-054): one `LocalTtyBackend` serves both terminal and runner use cases.
> The local backend ignores `backend_params` (no backend-specific selector
> fields); a docker/SSH backend would deserialize its own typed params from
> it.

## Summary

> To be filled on completion