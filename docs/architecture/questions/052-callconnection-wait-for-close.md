# OQ-52: CallConnection::wait_for_close() for Supervision Loop

- **Origin**: [crates/hub/README.md](crates/hub/README.md)
- **Status**: open
- **Door type**: Two-way
- **Priority**: medium
- **Resolution**: Not yet decided.

The hub's worker supervision loop needs a way to await connection close so it
can call `detach_peer` and retry. Today `CallConnection` exposes `connection()`
(the underlying `Connection`) but not a "wait for run_loop exit" future.

Options:
- **(a)** Add a `closed()` method to `CallConnection` that returns a
  `Future<Output = ()>` resolving when `run_loop` exits. The dispatcher
  signals a `tokio::sync::Notify` on exit; `closed()` awaits it.
- **(b)** Use a `tokio::sync::oneshot` channel created by the caller and
  passed to the dispatcher, signaled on `run_loop` exit.
- **(c)** Poll `connection().accept_bi()` in a loop until it returns
  `ConnectionClosed` — works but is polling, not event-driven.

Option (a) is the cleanest: a method on `CallConnection` that any caller
(not just the hub) can use to await connection close. It is a small additive
change to `alknet-call`.

- **Cross-references**: ADR-067, [crates/hub/README.md](crates/hub/README.md)
