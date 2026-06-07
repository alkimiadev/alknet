---
id: cli/connect-command
name: Implement `alknet connect` CLI subcommand with clap
status: completed
depends_on:
  - client/connect-options
scope: moderate
risk: low
impact: component
level: implementation
---

## Description

Implement the `alknet connect` CLI subcommand using `clap` with derive macros. Translates `ConnectOptions` into CLI flags and runs the client session. All options from client.md CLI interface must be supported.

Environment variable defaults: `ALKNET_SERVER`, `ALKNET_IDENTITY` as convenience defaults per ADR-011.

`--proxy` with `--transport tcp` should warn or be a no-op (ADR-019: client proxy wraps transport, and TCP transport is already direct).

## Acceptance Criteria

- [ ] `alknet connect` subcommand flags match client.md CLI interface: `--server`, `--peer`, `--transport`, `--identity`, `--socks5`, `--forward`, `--remote-forward`, `--proxy`, `--iroh-relay`, `--tls-server-name`, `--insecure`
- [ ] `--server` required for tcp/tls transport (validated at parse time or runtime)
- [ ] `--peer` required for iroh transport (validated)
- [ ] `--identity` required for all transports
- [ ] `--transport` defaults to `tcp`
- [ ] `--socks5` defaults to `127.0.0.1:1080`
- [ ] `--forward` is repeatable (clap `multiple_occurrences`)
- [ ] `--remote-forward` is repeatable
- [ ] Environment variable defaults: `ALKNET_SERVER` for `--server`, `ALKNET_IDENTITY` for `--identity`
- [ ] `--proxy` with `--transport tcp` prints warning (ADR-019: effectively no-op)
- [ ] CLI translates args into `ConnectOptions` and calls `ClientSession::new(opts).run().await`
- [ ] Errors reported to stderr with non-zero exit code
- [ ] `cargo run -p alknet -- connect --help` shows all flags with descriptions

## References

- docs/architecture/client.md — CLI Interface section with all flags
- docs/architecture/decisions/011-no-ssh-config-programmatic-api.md — env var defaults
- docs/architecture/decisions/019-proxy-dual-semantics.md — client proxy semantics

## Notes

> To be filled by implementation agent

## Summary

> To be filled on completion