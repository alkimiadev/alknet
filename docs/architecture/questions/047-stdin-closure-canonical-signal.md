# OQ-47: Stdin Closure Canonical Signal

- **Origin**: `docs/research/alknet-docker/poc-summary.md` §"Open
  Unknowns" #4 (stdin closure semantics for raw carriage);
  [crates/tty/tty-wire.md](crates/tty/tty-wire.md) §"Stdin Closure".
- **Status**: resolved
- **Door type**: Two-way
- **Priority**: low
- **Resolution**: Either a zero-length stdin chunk (stream_type 0,
  length 0 — the docker POC's sentinel) or a `{"type":"eof"}`
  control chunk (stream_type 3 — the tty POC's explicit signal) closes
  the client's stdin. Both are accepted by the adapter; the spec
  recommends `eof` for explicitness (it's a control message, not a
  data-length hack). The adapter handles both identically: signal EOF
  to the backend's stdin (`ChildStdin::drop` / PTY writer close) and
  keep pumping stdout until the exit resolves — the client may still
  want to receive remaining output + the exit code. A third path
  (client closes the write half of the bidi stream) is also accepted
  and handled the same way. See ADR-052 and `tty-wire.md`.
- **Cross-references**: ADR-052, [tty-wire.md](crates/tty/tty-wire.md),
  [tty-adapter.md](crates/tty/tty-adapter.md)
