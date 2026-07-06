# OQ-46: Runner API Surface

- **Origin**: `docs/research/alknet-tty/phase-0-findings.md` OQ-TTY-05;
  [crates/tty/tty-local.md](crates/tty/tty-local.md) (the runner pattern
  in pipe mode).
- **Status**: deferred(scope)
- **Door type**: Two-way
- **Priority**: low
- **Blocked on**: a concrete runner-policy use case that forces the API
  surface (job management, log persistence, task graph integration).
- **Resolution**: Not yet decidable. The runner *mechanism* (pipe mode —
  `TtyParams.terminal = None` → `std::process::Command` with piped stdio
  → framed byte stream + exit code) is in alknet-tty (ADR-054). The
  runner *policy* (job management, log persistence, task graph
  integration) is a downstream crate's job, not in scope for alknet-tty.
  This OQ tracks whether a runner-policy crate (e.g., an
  `alknet-runner` crate that builds on the pipe mode + the wire format
  to provide job management) is needed, and what its API surface would
  be. The decision is deferred until a concrete use case forces it; the
  mechanism is preserved regardless. See ADR-054 and `tty-local.md`
  §"The Runner Pattern".
- **Cross-references**: ADR-054, [tty-local.md](crates/tty/tty-local.md)
