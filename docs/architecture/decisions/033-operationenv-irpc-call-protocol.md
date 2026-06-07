# ADR-033: OperationEnv as Universal Composition Mechanism

## Status

Accepted

## Context

The `@alkdev/operations` TypeScript package defines `OperationEnv` as a
universal composition mechanism. A handler receives `context.env[namespace][op](input)`
and can invoke any registered operation regardless of whether it runs locally, in
an irpc service on the same cluster, or on a remote node via call protocol.

The research documents define three dispatch paths:
1. **Local dispatch** — direct function call through the operation registry
2. **Service dispatch** — irpc protocol call to a service backend
3. **Remote dispatch** — call protocol `EventEnvelope` to a remote node

Without a formal decision, irpc services could be seen as a replacement for
OperationEnv or for the call protocol. They are not — irpc is one dispatch
backend for OperationEnv, not a replacement for anything. The call protocol is
another dispatch backend. OperationEnv unifies them from the handler's
perspective.

The three communication patterns in the system (ADR-032) are:
- Domain events (Honker streams) — internal to the owning service
- irpc service calls — synchronous, in-cluster
- Call protocol events — asynchronous, cross-node

irpc services and call protocol operations serve different scopes but must
compose cleanly through OperationEnv.

## Decision

**OperationEnv is the universal composition mechanism that all operation
handlers receive. It provides namespace + operation name → invoke with input,
return output, regardless of dispatch path.**

### OperationEnv Behavioral Contract

```rust
// The behavioral contract: given a namespace and operation name, invoke the
// operation with the given input and return the output. The handler neither
// knows nor cares whether the dispatch is local, via irpc, or via call protocol.
pub trait OperationEnv: Send + Sync {
    fn invoke(&self, namespace: &str, operation: &str, input: Value) -> ResponseEnvelope;
}
```

The Rust implementation may use typed method dispatch or a registry behind the
scenes, but the handler-facing API must preserve this contract.

### Three Dispatch Paths

OperationEnv resolves each call to one of three dispatch backends:

| Path | Mechanism | Serialization | Scope |
|------|-----------|---------------|-------|
| Local | Direct function call through registry | None (in-process) | Same process |
| Service | irpc protocol enum dispatch | postcard (binary) | Same cluster |
| Remote | Call protocol `EventEnvelope` | JSON | Cross-node |

All three produce the same `ResponseEnvelope`. The handler always calls
`context.env.invoke("secrets", "derive", input)` and gets a `ResponseEnvelope`
back.

### Service Assembly

The deployment topology determines which dispatch path each operation uses:

```rust
// Minimal deployment (single node, all local)
let env = OperationEnv::local(local_registry);

// Production deployment (mix of local and remote)
let env = OperationEnv::new()
    .local("auth", auth_registry)           // Auth runs locally
    .local("config", config_registry)       // Config runs locally
    .service("secrets", secret_irpc_client) // Secret service via irpc
    .remote("worker-1", call_protocol_conn)  // Worker-1 operations via call protocol
```

### irpc Services Are One Dispatch Backend

irpc services (`AuthProtocol`, `SecretProtocol`, `ConfigProtocol`) define the
wire format for in-cluster communication. They are Rust-to-Rust, type-safe,
and efficient. But they are not a replacement for OperationEnv or for the call
protocol. They are one dispatch backend.

An irpc service can be exposed as a call protocol operation:
`/head/auth/verify` receives a call protocol event and internally calls
`AuthProtocol::VerifyPubkey` via irpc. The layers compose:

```
Call Protocol (Layer 3, external, JSON)
    └── irpc Service (Layer 3, internal, postcard)
            └── Honker Streams (Domain events, within service boundary)
```

### Adapters Map to OperationEnv

HTTP (`POST /v1/{namespace}/{op}`), MCP (`tools/call`), DNS
(`{op}.{namespace}.alk.dev TXT?`), and call protocol
(`/call.requested`) all resolve through OperationEnv. This is what makes
operations universally composable across all interfaces.

## Consequences

- **Positive**: Handlers compose through a single interface. Adding a new
  dispatch path (e.g., a new irpc service) doesn't change handler code.
- **Positive**: irpc and call protocol coexist naturally. The handler doesn't
  know which path was taken.
- **Positive**: Adapters (MCP, HTTP, DNS) map to operations through the same
  OperationEnv interface. One handler, multiple dispatch paths.
- **Positive**: Deployment topology determines dispatch, not code. Same handler
  works locally, in-cluster, or cross-node.
- **Negative**: OperationEnv is a new abstraction that must coexist with the
  existing call protocol handler pattern. The registry currently maps paths to
  handlers; OperationEnv adds namespace-aware composition on top.
- **Negative**: The `@alkdev/operations` TypeScript `HashMap<String,
  HashMap<String, fn>>` model needs idiomatic Rust translation. The behavioral
  contract must match, but the implementation can differ.

## References

- [research/services.md](../../research/services.md) — OperationContext, OperationEnv
- [research/integration-plan.md](../../research/integration-plan.md) — Phase 1.5, OperationEnv wiring
- [ADR-032](032-event-boundary-discipline.md) — Event boundary discipline
- [ADR-024](024-bidirectional-call-protocol.md) — Bidirectional call protocol
- [ADR-025](025-handler-spec-separation.md) — Handler/spec separation