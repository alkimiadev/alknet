# OQ-44: Terminal Modes (TTY modes)

- **Origin**: `docs/research/alknet-tty/phase-0-findings.md` OQ-TTY-02;
  [crates/tty/tty-backend.md](crates/tty/tty-backend.md)
  (`TerminalParams.modes` field).
- **Status**: deferred(scope)
- **Door type**: Two-way
- **Priority**: low
- **Impacts**: None — the `modes` field is reserved as `{}` and backends
  use their defaults, which work for the common terminal case.
- **Blocked on**: a concrete mode-control use case (a deployment that
  needs to set echo/raw/canonical/etc. modes on a PTY, beyond the backend's
  defaults).
- **Resolution**: Not yet decidable. SSH's `pty_request` carries TTY
  modes (echo, raw, canonical, etc.) as a packed bitmask. The common
  case is "default terminal modes" — the `modes` field in
  `TerminalParams` is `serde_json::Value` (reserved as `{}` in v1) for
  when a concrete use case requires mode control. The backends
  (`portable_pty`, docker `tty: true`, russh `pty_request`) all have
  defaults that work for the common terminal case. Adding mode control
  is additive (extend the `modes` JSON shape) and does not break
  downstream; the decision is deferred until a use case forces it.
- **Cross-references**: ADR-053, [tty-backend.md](crates/tty/tty-backend.md),
  [tty-wire.md](crates/tty/tty-wire.md)
