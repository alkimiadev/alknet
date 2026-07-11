# OQ-54: Inbound Worker on_worker_connected Hook Placement

- **Origin**: [crates/hub/README.md](crates/hub/README.md)
- **Status**: resolved (2026-07-09)
- **Door type**: Two-way
- **Priority**: low
- **Resolution**: The hook is a **callback** (`WorkerConnectedCallback`) that
  fires inside `CallAdapter::handle()` between identity resolution and
  dispatch start. `handle()` blocks until disconnect — there is no "after
  handle accepts" point for the assembly layer to hook into. The callback
  carries both `on_connected` (runs `from_call`, registers bundles, attaches
  peer to aggregated env) and `on_disconnected` (detaches peer, drops channel
  tracking on `run_loop` exit). The callback is wired via
  `CallAdapter::with_worker_connected_callback(callback)`.

  The earlier "explicit post-hoc call" design (option a) was based on the
  incorrect assumption that `CallAdapter::handle` returns after accepting
  the connection. It doesn't — it runs the dispatch loop internally and
  returns only on disconnect. The callback approach is the correct design.

- **Cross-references**: ADR-067, ADR-068, [crates/hub/README.md](crates/hub/README.md)
