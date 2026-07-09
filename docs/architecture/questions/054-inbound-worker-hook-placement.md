# OQ-54: Inbound Worker on_worker_connected Hook Placement

- **Origin**: [crates/hub/README.md](crates/hub/README.md)
- **Status**: open
- **Door type**: Two-way
- **Priority**: low
- **Resolution**: Not yet decided.

When a worker connects inbound (via `CallAdapter::handle`), the hub needs to
call `from_call`, register the bundles, and `attach_peer`. Two options:

- **(a) Explicit**: The assembly layer calls `hub.on_worker_connected()` after
  `CallAdapter::handle` accepts the connection. The hub provides the method;
  the assembly layer wires it.
- **(b) Wrapper**: The hub provides a `HubCallAdapter` wrapper that calls the
  hook automatically inside `handle()`. The assembly layer uses the wrapper
  instead of the raw `CallAdapter`.

Option (a) is simpler and more flexible — the assembly layer controls when
the hook fires and can add its own logic (authorization checks, logging)
before discovery. Option (b) is more convenient but hides the hook from the
assembly layer.

The explicit approach is the committed interim. A wrapper is additive.

- **Cross-references**: ADR-067, ADR-068, [crates/hub/README.md](crates/hub/README.md)
