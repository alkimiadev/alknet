# ADR-062: Docker Client and OwnershipStore Injection via Closure Capture

## Status

Accepted

## Context

The docker operation handlers need access to two non-secret, shared
runtime handles:

1. The bollard `Docker` client — a `Clone`-able handle (internally an
   `Arc`) to the docker daemon connection, constructed once at
   assembly-layer startup.
2. The `OwnershipStore` — the ADR-050 write-side trait, used by
   `docker/container/create` (to `record`) and `docker/container/remove`
   (to `revoke`).

An earlier draft of the docker spec
(`docker-operations.md`) showed handlers calling
`ctx.docker_client()` and speculated the handle lived on "a
`DockerOpsExt` extension trait on `OperationContext`, or a
`DockerClient` capability in `ctx.capabilities`." This was an inline
open question with a model conflict:

### Why `Capabilities` is the wrong channel

`Capabilities` (ADR-014, `operation-registry.md` §"Capability
Injection") is the type for **outbound secret material** — decrypted
API keys, signing keys, vault-derived credentials. Its contract
(`core-types.md`): non-`Serialize`, zeroized on drop, populated from
the vault at registration time. A bollard `Docker` handle to a local
unix socket is not secret material; it has no zeroizing semantics; it
must not be smuggled through the secret-injection channel. Putting a
`Docker` handle in `Capabilities` would be a category error against
the one-way `Capabilities` contract (ADR-014).

The same applies to the `OwnershipStore`: it's a shared state handle
(shared `Arc<dyn OwnershipStore>`), not secret material.

### Why `OperationContext` extension is the wrong channel

`OperationContext` (`operation-registry.md:230`) is a concrete struct
with a fixed field set, constructed by the dispatch path per call.
Adding a `docker_client` field (or an extension trait that reads one)
would either (a) make every non-docker handler pay for a field they
don't use, or (b) require a typed-map / downcast pattern that
violates the "context is concrete, not a bag" design. The
established pattern for per-handler-set shared state is not a
context field — it's closure capture.

### The established pattern: closure capture at registration

The `from_openapi` adapter (ADR-017, `client-and-adapters.md`,
`operation-registry.md:819`) captures its `reqwest::Client` in each
forwarding handler's closure at registration time:

```rust
// operation-registry.md:819 (the established pattern)
.with_local(vastai_listMachines_spec(), Arc::new(vastai_handler),
            CompositionAuthority::new("vastai", ["vastai:query"]),
            ScopedOperationEnv::new([...]),
            Capabilities::new().with_api_key("vastai", vastai_token))
```

The `vastai_handler` closure captures its reqwest client (and its
auth token via `Capabilities`, which *is* secret material). The
non-secret shared handle (reqwest client) is captured in the closure;
the secret (the API key) goes through `Capabilities`. This is the
correct split: secret material through `Capabilities`, non-secret
shared state through closure capture.

The docker ops follow the same split. The `Docker` client and the
`OwnershipStore` are non-secret shared state — closure-captured. There
are no secret capabilities (local bollard needs no API key; the
docker daemon is local).

## Decision

### 1. `register_docker_ops` captures the `Docker` client and `OwnershipStore` in each handler closure

The `register_docker_ops` function (or the `DockerOps` builder)
takes `Arc<Docker>` and `Arc<dyn OwnershipStore>` (plus the label
config and the `CompositionAuthority`) as arguments, and constructs
each `Handler` / `StreamingHandler` closure with those handles
captured by reference (`Arc::clone` per handler):

```rust
pub fn register_docker_ops(
    builder: &mut OperationRegistryBuilder,
    docker: Arc<bollard::Docker>,
    ownership_store: Arc<dyn OwnershipStore>,
    labels: &DockerLabels,
    authority: CompositionAuthority,
) {
    let docker_clone = docker.clone();
    let ownership_clone = ownership_store.clone();
    let labels_clone = labels.clone();
    builder.with_local(
        container_inspect_spec(),
        Arc::new(move |input, ctx| {
            let docker = docker_clone.clone();
            Box::pin(async move {
                let container_id = input["containerId"].as_str()
                    .ok_or_else(|| /* INVALID_INPUT */)?;
                match docker.inspect_container(container_id, None::<()>).await {
                    Ok(info) => ResponseEnvelope::ok(to_json_value(info)),
                    Err(e) => /* CONTAINER_NOT_FOUND / DOCKER_ERROR */,
                }
            })
        }),
        authority.clone(),
        ScopedOperationEnv::empty(),
        Capabilities::new(),  // no secret caps — local bollard
    );
    // ... same pattern for each operation ...
}
```

Each handler closure captures the `Docker` client and (where needed)
the `OwnershipStore` by cloning the `Arc`. The handler does not read
these from `OperationContext`; they're baked into the closure at
registration time. The `Capabilities` passed to `with_local` is empty
(`Capabilities::new()`) — there are no secret capabilities for local
bollard operations.

### 2. The `Docker` client is shared, `Clone`-able, and constructed once

bollard's `Docker` is `Clone` (it holds an internal `Arc` to the
connection). The assembly layer constructs one
(`Docker::connect_with_local_defaults()`, ADR-059 §2), wraps it in an
`Arc`, and passes it to `register_docker_ops`. Each handler clones
the `Arc` (cheap — a refcount bump) into its closure. There is no
per-handler docker client; the one client is shared across all
docker operations.

### 3. The `OwnershipStore` is shared the same way

`Arc<dyn OwnershipStore>` is cloned into the `create` and `remove`
handler closures (the two operations that write to the store). The
other operations (inspect, start, stop, logs, exec, list, images)
don't capture the store — they don't write to it. The `create`
closure captures it for `record`; the `remove` closure captures it
for `revoke`.

### 4. No `docker_client()` accessor; no `OperationContext` extension

The handlers do not call `ctx.docker_client()` or any other accessor.
The `Docker` client and `OwnershipStore` are not on `OperationContext`
and are not accessible via an extension trait. They're closure-
captured, full stop. A handler that needs the docker client gets it
from its closure's captured `Arc<Docker>`, not from the context.

This means the handler signature is the standard
`Fn(Value, OperationContext) -> Pin<Box<dyn Future<Output = ResponseEnvelope> + Send>>`
(ADR-049) — no new parameter, no new context field. The docker-
specific state is in the closure, not the context.

## Consequences

**Positive:**

- The injection model matches the established `from_openapi` pattern
  (non-secret shared state via closure capture, secret material via
  `Capabilities`). No new mechanism; no `OperationContext` change; no
  `Capabilities` conflict.
- The `Capabilities` contract (ADR-014) stays clean — it holds secret
  material only. A local bollard handle is not smuggled through the
  secret channel.
- The handlers are self-contained — the `Docker` client and store are
  baked in at registration, not looked up at call time. This is
  testable (construct a `Docker` client in a test, register the ops,
  invoke the handler directly) and composable (the same handler works
  whether the docker client points at a local socket or a test
  daemon).
- No new trait, no extension mechanism, no downcast. The injection is
  plain Rust closure capture — the simplest thing that works.

**Negative:**

- Each handler closure captures the `Docker` and (for create/remove)
  the `OwnershipStore` by `Arc::clone`. This is a refcount bump per
  handler construction (at startup, once) — negligible. The per-call
  cost is zero (the closure already holds the `Arc`; calling the
  handler doesn't re-clone).
- The `register_docker_ops` function signature is longer (takes the
  `Docker` + store + labels + authority). This is the assembly layer's
  wiring concern, not a handler concern — the handlers don't know
  where their `Docker` came from.
- A test that wants to invoke a single docker handler must construct
  the `Docker` client and (if the handler uses the store) the
  `OwnershipStore` to register the op. This is the same as any
  handler test that needs its dependencies — not new.

## Door type

**One-way.** The decision to use closure capture (not a context field,
not a `Capabilities` entry) is the injection model every docker
handler is written against. Reversing to a context field would be a
rewrite of every handler and an `OperationContext` change. The
specific captures (`Docker` + `OwnershipStore`) are the non-secret
state the handlers depend on; changing what's captured is a handler
rewrite.

The choice of `Arc<Docker>` vs a non-`Arc` shared reference is a
two-way-door implementation detail (bollard's `Docker` is already
`Clone`-with-`Arc`-internally; the outer `Arc` is for the trait-object
`OwnershipStore`'s sake, not the `Docker`'s). The capture pattern
(closure capture) is the one-way commitment.

## References

- [ADR-014](014-secret-material-flow-and-capability-injection.md) —
  the `Capabilities` contract this ADR keeps clean (secret material
  only; the `Docker` handle is not secret)
- [ADR-017](017-call-protocol-client-and-adapter-contract.md) — the
  `from_openapi`/`from_call` adapter pattern this ADR follows
  (non-secret shared state via closure capture)
- [ADR-022](022-handler-registration-provenance-and-composition-authority.md)
  — `HandlerRegistration`, the bundle the `register_docker_ops`
  builder populates
- [ADR-050](050-dynamic-resource-ownership-for-runtime-spawned-resources.md)
  — the `OwnershipStore` trait the `create`/`remove` handlers capture
- [ADR-058](058-alknet-docker-on-alknet-call.md) — the operation
  surface this injection model serves
- `crates/call/operation-registry.md` §"Capability Injection"
  (the `Capabilities` semantics this ADR respects) and
  §"OperationContext" (the struct this ADR does *not* extend)
- `crates/call/client-and-adapters.md` §"from_openapi forwarding"
  (the prior art: reqwest client closure-captured, API key in
  `Capabilities`)
- Spec: [docker-operations.md](../crates/docker/docker-operations.md)
  §"Handler injection" (the section this ADR adds, replacing the
  earlier `ctx.docker_client()` sketches)