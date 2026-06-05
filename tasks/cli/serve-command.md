---
id: cli/serve-command
name: Implement `alknet serve` CLI subcommand with clap
status: completed
depends_on:
  - server/serve-loop
scope: moderate
risk: low
impact: component
level: implementation
---

## Description

Implement the `alknet serve` CLI subcommand using `clap` with derive macros. This translates `ServeOptions` into CLI flags and runs the server. All options from server.md CLI interface must be supported.

Environment variable defaults: none mandated for serve, but consistent with programmatic-first API.

The binary is the `alknet` crate at `crates/alknet/src/main.rs`.

## Acceptance Criteria

- [x] `crates/alknet/src/main.rs` defines CLI with clap derive: `alknet` with `serve` and `connect` subcommands (connect stub for now)
- [x] `alknet serve` subcommand flags match server.md CLI interface exactly: `--key`, `--authorized-keys`, `--cert-authority`, `--transport`, `--listen`, `--tls-cert`, `--tls-key`, `--acme-domain`, `--stealth`, `--proxy`, `--iroh-relay`, `--max-connections-per-ip`, `--max-auth-attempts`
- [x] `--key` is required (no default)
- [x] `--transport` defaults to `tcp`
- [x] `--listen` defaults to `0.0.0.0:22`
- [x] `--stealth` validates that `--transport tls` is set; error otherwise
- [x] `--transport iroh` prints endpoint ID on startup
- [x] `--acme-domain` requires `acme` feature (compile-time or runtime error if missing)
- [x] Key inputs accept file paths (strings); in-memory key data is a library/API concern, not CLI
- [x] CLI translates args into `ServeOptions` and calls `Server::new(opts).run().await`
- [x] Errors reported to stderr with non-zero exit code
- [x] `cargo run -p alknet -- serve --help` shows all flags with descriptions

## References

- docs/architecture/server.md — CLI Interface section with all flags
- docs/architecture/overview.md — "A single binary with subcommands"

## Notes

All 12 CLI flags implemented. ServeTransportModeArg ValueEnum maps to ServeTransportMode. Stealth validation checks transport==tls. ACME feature-gated at compile time. iroh prints endpoint ID on startup.

## Summary

Implemented alknet serve CLI subcommand with all server.md flags. Clap derive with ServeTransportModeArg, stealth validation, ACME feature gate, iroh endpoint ID printing. Build/clippy/test pass across all feature combinations.