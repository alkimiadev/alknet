# ADR-025: Handler/Spec Separation for Downstream Service Registration

## Status
Accepted

## Context

The current control channel (ADR-018) is hardcoded: `alknet-control:0` bridges
to the local pubsub event bus. If NAPI wants to expose `fs.readFile` or
`bash.exec` as callable operations, it has no way to register these with core's
channel routing. The NAPI handler would need to intercept channel data outside
of core.

For the hub/spoke model, spokes register their operations with the hub when
they connect. The hub's registry must include both hub-local operations and
remote operations exposed by spokes.

## Decision

Operation specs and handlers are separated from core. Core provides:

1. `OperationSpec` — describes what an operation does (name, type, input/output
   schemas, access control)
2. `OperationHandler` — implements the operation logic
3. `OperationRegistry` — maps paths to specs + handlers
4. Built-in operations: `/services/list`, `/services/schema`

Downstream consumers register their own operations:

```rust
// NAPI layer registers dev env tools
registry.register(OperationSpec { name: "/fs/readFile", ... }, fs_read_handler);
registry.register(OperationSpec { name: "/bash/exec", ... }, bash_exec_handler);

// Browser client registers a custom UDF
registry.register(OperationSpec { name: "/notify/alert", ... }, notify_handler);
```

Operation names use slash-based paths: `/{spoke}/{service}/{op}`. The first
segment routes to the node. The `namespace` field on `OperationSpec` is
derived from the second path segment (`service`).

When spoke operations are registered with the hub, the hub adds the spoke
prefix: a spoke that registers `/fs/readFile` as "dev1" becomes addressable as
`/dev1/fs/readFile` in the hub's routing table.

The `/services/list` operation returns all registered specs. The
`/services/schema` operation returns the spec for a specific operation. These
are read-only — no admin operations.

## Consequences

- **Positive**: NAPI, Python, and any downstream consumer can register
  operations without modifying core.
- **Positive**: Service discovery is built in. Clients query `/services/list`
  to learn what operations a hub offers.
- **Positive**: Spoke prefix naturally differentiates multiple spokes exposing
  the same service (dev1 vs dev2).
- **Positive**: `AccessControl` on each `OperationSpec` enables per-operation
  authorization. Higher-risk operations (shell, filesystem write) can require
  tighter scopes.
- **Positive**: Schema exposure enables MCP adapter generation. OperationSpec
  maps directly to MCP tool definitions.
- **Negative**: The registry adds complexity. Core now owns `OperationSpec`,
  `OperationRegistry`, and `PendingRequestMap`.
- **Negative**: Namespace collisions between downstream consumers are possible.
  The spoke prefix mitigates this: `/dev1/fs/readFile` vs `/dev2/fs/readFile`.

## References

- [call-protocol.md](../call-protocol.md) — Full call protocol spec
- [ADR-018](018-control-channel-for-pubsub.md) — Control channel (generalized)
- `@alkdev/operations` — TypeScript `OperationSpec`, `CallHandler`, registry