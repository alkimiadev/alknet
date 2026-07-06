# OQ-43: `TtyControl` as a `Clone` trait object

- **Origin**: [crates/tty/tty-backend.md](crates/tty/tty-backend.md)
  (the `TtyHandle.control` field shape);
  `docs/research/alknet-tty/phase-0-findings.md` OQ-TTY-01 (the trait-
  shape open question the local-PTY POC resolved).
- **Status**: resolved
- **Door type**: One-way (the `TtyControl` trait shape is part of the
  `TtyBackend` API surface — ADR-053), two-way (the concrete `Clone`
  newtype mechanism)
- **Priority**: medium
- **Resolution**: `TtyHandle.control` is
  `Option<Box<dyn TtyControl + Send + Unpin + Clone>>`. The `Clone`
  trait-object bound is satisfied via an `Arc`-backed `Clone` newtype: a
  small struct holding `Arc<dyn TtyControlInner>` where `TtyControlInner:
  Send + Sync` has the `resize`/`signal` methods, and the public
  `TtyControl` newtype implements `Clone` by cloning the `Arc`. The
  local-PTY POC used a concrete `PtyControl` struct (inherently `Clone` —
  it held `Arc<Mutex<...>>` fields); the trait-object form generalizes it
  so a backend can produce its own control type without the adapter
  knowing the concrete shape. The `Clone` constraint exists because the
  adapter's control-chunk dispatcher needs to be handed off to the
  spawned pump task (the POC's `session::drive_session` clones
  `pty.control` for the client→backend pump). See ADR-053 and
  `tty-backend.md`.
- **Cross-references**: ADR-053, [tty-backend.md](crates/tty/tty-backend.md),
  `/workspace/alknet-tty-poc/src/local_pty.rs` (`PtyControl`)
