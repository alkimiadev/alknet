---
id: tty/local-feature-reexport
name: Wire alknet-tty `local` feature gate and re-export LocalTtyBackend
status: pending
depends_on: [tty/review-tty, tty-local/review-tty-local]
scope: narrow
risk: low
impact: component
level: implementation
---

## Description

Wire the `local` feature gate on `alknet-tty` (declared in `tty/crate-init`)
to re-export `LocalTtyBackend` from `alknet-tty-local`. This is the seam
between the two crates (ADR-054): a consumer that wants the local backend
enables `features = ["local"]` and gets `alknet_tty::local::LocalTtyBackend`;
a consumer that only wants docker/ssh uses the default features and depends
on the backend crate directly — no `portable_pty` in the dependency tree.

### Cargo.toml (alknet-tty)

The `local` feature was declared in `tty/crate-init` but the
`alknet-tty-local` dependency was not yet wired. Add it as an optional
dependency activated by the `local` feature:

```toml
[features]
default = []
local = ["dep:alknet-tty-local"]

[dependencies]
alknet-tty-local = { path = "../alknet-tty-local", optional = true }
```

### Re-export module (alknet-tty/src/lib.rs)

Add a `local` module gated on the feature, re-exporting `LocalTtyBackend`:

```rust
#[cfg(feature = "local")]
pub mod local {
    pub use alknet_tty_local::LocalTtyBackend;
}
```

A consumer with `features = ["local"]` accesses it as
`alknet_tty::local::LocalTtyBackend`.

### Verification

- `cargo check -p alknet-tty` (default features) — succeeds, no
  `portable_pty` in the tree.
- `cargo check -p alknet-tty --features local` — succeeds,
  `alknet_tty::local::LocalTtyBackend` is accessible.
- `cargo tree -p alknet-tty --features local` shows `portable_pty` pulled in
  via `alknet-tty-local`; `cargo tree -p alknet-tty` (default) does NOT show
  `portable_pty`.
- `cargo clippy -p alknet-tty --features local` succeeds with no warnings.

### Assembly-layer example

Document (in a doc comment or the crate root) the assembly pattern:

```rust
let mut backends = HashMap::new();
backends.insert("local".into(),
    Arc::new(alknet_tty::local::LocalTtyBackend::new()) as Arc<dyn alknet_tty::TtyBackend>);
let tty_adapter = alknet_tty::adapter::TtyAdapter::new(backends);
```

This is the pattern the assembly layer (the CLI binary) uses to construct and
register the local backend. A docker-only deployment registers `docker`
instead; a mixed deployment registers both.

## Acceptance Criteria

- [ ] `alknet-tty` Cargo.toml has `alknet-tty-local` as an optional dependency, `path = "../alknet-tty-local"`
- [ ] `local` feature activates `dep:alknet-tty-local`
- [ ] `alknet-tty/src/lib.rs` has `#[cfg(feature = "local")] pub mod local` re-exporting `LocalTtyBackend`
- [ ] `cargo check -p alknet-tty` (default features) succeeds
- [ ] `cargo check -p alknet-tty --features local` succeeds
- [ ] `cargo tree -p alknet-tty` (default) does NOT include `portable_pty`
- [ ] `cargo tree -p alknet-tty --features local` includes `portable_pty` via `alknet-tty-local`
- [ ] `cargo clippy -p alknet-tty --features local` succeeds with no warnings
- [ ] `cargo test -p alknet-tty --features local` succeeds
- [ ] Assembly-layer pattern documented (doc comment or crate root)

## References

- docs/architecture/crates/tty/overview.md — §"Feature Gates", §"Backend Location Map"
- docs/architecture/crates/tty/tty-backend.md — §"Backend registration and the assembly layer"
- docs/architecture/decisions/054-local-tty-backend-sibling-crate.md — ADR-054 (sibling crate behind `local` feature)

## Notes

> This is the seam between the two crates. The feature gate ensures a
> docker-only or ssh-only deployment doesn't pull in `portable_pty`. The
> `cargo tree` check is the verification: `portable_pty` appears only with
> `--features local`. This task depends on both review tasks
> (`tty/review-tty`, `tty-local/review-tty-local`) because it wires the two
> reviewed crates together.

## Summary

> To be filled on completion