---
id: setup/project-init
name: Initialize Cargo workspace with alknet, alknet-core, and alknet-napi crates
status: pending
depends_on: []
scope: moderate
risk: low
impact: project
level: implementation
---

## Description

Set up the Rust workspace from scratch. The repo currently has only `docs/` and `.git/`. Initialize a Cargo workspace with three crate directories following the architecture spec:

- **`alknet-core`** — library crate with feature flags (`tls`, `iroh`, `acme`). All core logic lives here.
- **`alknet`** — binary crate depending on `alknet-core`. CLI entry point.
- **`alknet-napi`** — napi-rs crate for the Node.js native addon (skeleton only at this stage).

Per overview.md: `russh`, `tokio`, `clap`, `tracing`, `anyhow`/`thiserror` are core dependencies. `tokio-rustls`, `rustls`, `rustls-acme`, `iroh` are feature-gated.

## Acceptance Criteria

- [ ] `Cargo.toml` workspace root with `[workspace]` members: `crates/alknet-core`, `crates/alknet`, `crates/alknet-napi`
- [ ] `crates/alknet-core/Cargo.toml` with library crate, feature flags: `tls` (tokio-rustls + rustls), `iroh` (iroh), `acme` (rustls-acme, implies `tls`)
- [ ] Core dependencies listed: `russh`, `tokio` (full), `tracing`, `anyhow`, `thiserror`, `tokio-util`
- [ ] `crates/alknet/Cargo.toml` with binary crate, depends on `alknet-core` with default features, `clap` with `derive` feature
- [ ] `crates/alknet-napi/Cargo.toml` with `cdylib` crate type, depends on `alknet-core`, `napi` and `napi-derive`
- [ ] `crates/alknet-core/src/lib.rs` with module skeleton: `pub mod transport; pub mod client; pub mod server; pub mod auth; pub mod socks5; pub mod error;`
- [ ] `crates/alknet/src/main.rs` with minimal `fn main()` skeleton
- [ ] `crates/alknet-napi/src/lib.rs` with `#[macro_use] extern crate napi_derive;` and empty skeleton
- [ ] `.gitignore` covers `target/`, `node_modules/`
- [ ] `cargo check` succeeds for all workspace members
- [ ] Feature flags resolve correctly: `cargo check -p alknet-core --features tls`, `--features iroh`, `--features acme`

## References

- docs/architecture/overview.md — package structure, dependencies, feature flags
- docs/architecture/napi-and-pubsub.md — alknet-napi crate purpose

## Notes

> To be filled by implementation agent

## Summary

> To be filled on completion