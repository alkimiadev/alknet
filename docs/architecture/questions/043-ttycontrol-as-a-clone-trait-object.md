# OQ-43: `TtyControl` as a `Clone` trait object

- **Origin**: [crates/tty/tty-backend.md](crates/tty/tty-backend.md)
  (the `TtyHandle.control` field shape);
  `docs/research/alknet-tty/phase-0-findings.md` OQ-TTY-01 (the trait-
  shape open question the local-PTY POC resolved).
- **Status**: resolved
- **Door type**: One-way (the `TtyControl` trait shape is part of the
  `TtyBackend` API surface — ADR-053), two-way (the concrete
  `TtyControlHandle` newtype mechanism)
- **Priority**: medium
- **Resolution**: `TtyHandle.control` is `Option<TtyControlHandle>`.
  `TtyControlHandle` is a concrete `#[derive(Clone)]` newtype wrapping
  `Arc<dyn TtyControl + Send + Sync>`; the adapter clones the `Arc` to
  hand a handle to the spawned control-chunk dispatcher. The
  `TtyControl` trait itself is NOT `Clone` — `Clone` is not object-safe
  (`fn clone(&self) -> Self` returns `Self`, which forbids `dyn`
  dispatch), so `Box<dyn TtyControl + Clone>` does not compile. The
  design splits the concerns: the trait stays object-safe (`Send +
  Sync`, no `Clone`); the newtype carries the `Clone`-ability. The
  local-PTY POC used a concrete `PtyControl` struct (inherently `Clone`
  — it held `Arc<Mutex<...>>` fields); the newtype generalizes the POC's
  shape so a backend produces its own control type via
  `TtyControlHandle::new(Arc::new(MyControl))` without the adapter
  knowing the concrete shape. The `Clone` constraint exists because the
  adapter's control-chunk dispatcher needs to be handed off to the
  spawned pump task (the POC's `session::drive_session` clones
  `pty.control` for the client→backend pump). See ADR-053 and
  `tty-backend.md`.
- **Cross-references**: ADR-053, [tty-backend.md](crates/tty/tty-backend.md),
  `/workspace/alknet-tty-poc/src/local_pty.rs` (`PtyControl`)
