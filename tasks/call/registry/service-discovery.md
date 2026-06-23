---
id: call/registry/service-discovery
name: Implement services/list and services/schema built-in operations
status: pending
depends_on: [call/registry/handler-registration]
scope: narrow
risk: low
impact: isolated
level: implementation
---

## Description

Implement the two built-in service discovery operations in
`src/registry/discovery.rs`. These are read-only operations that expose what
the node offers.

### Operations

| Operation name | Display path | Type | Description |
|---------------|-------------|------|-------------|
| `services/list` | `/services/list` | Query | List registered operation names and metadata |
| `services/schema` | `/services/schema` | Query | Get the OperationSpec for a specific operation |

### services/list

Returns `External` operations only. `Internal` operations are not part of the
wire-facing API surface — they're implementation details of composition. A
remote client cannot enumerate the internal call tree (ADR-015).

```json
{
  "operations": [
    { "name": "fs/readFile", "namespace": "fs", "op_type": "query" },
    { "name": "agent/chat", "namespace": "agent", "op_type": "subscription" },
    { "name": "events/subscribe", "namespace": "events", "op_type": "subscription" }
  ]
}
```

The handler queries the registry's `list_operations()` (which returns External
specs only) and serializes to the above format.

### services/schema

Accepts `{ "name": "fs/readFile" }` (no leading slash — registry form, same as
`OperationSpec.name`) and returns the full `OperationSpec` including
input/output JSON Schemas and declared `error_schemas` (ADR-023).

The CallAdapter normalizes the leading slash from wire `operationId`s before
lookup, so `services/schema` accepts both `fs/readFile` and `/fs/readFile`.

This enables client code generation: a client reading the schema can produce
typed error enums instead of generic error handling.

### Registration

These are registered as `Local` provenance with empty composition authority,
empty scoped env, and empty capabilities (they don't compose, don't need
credentials):

```rust
.with_local(services_list_spec(), Arc::new(services_list_handler),
            CompositionAuthority::none(), ScopedOperationEnv::empty(), Capabilities::new())
.with_local(services_schema_spec(), Arc::new(schema_handler),
            CompositionAuthority::none(), ScopedOperationEnv::empty(), Capabilities::new())
```

### Specs

```rust
fn services_list_spec() -> OperationSpec {
    OperationSpec {
        name: "services/list".into(),
        namespace: "services".into(),
        op_type: OperationType::Query,
        visibility: Visibility::External,
        input_schema: json!({}),  // no input
        output_schema: json!({
            "type": "object",
            "properties": {
                "operations": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "name": { "type": "string" },
                            "namespace": { "type": "string" },
                            "op_type": { "type": "string", "enum": ["query", "mutation", "subscription"] }
                        }
                    }
                }
            }
        }),
        error_schemas: vec![],
        access_control: AccessControl::default(),  // no restrictions — callable by all
    }
}

fn services_schema_spec() -> OperationSpec {
    OperationSpec {
        name: "services/schema".into(),
        namespace: "services".into(),
        op_type: OperationType::Query,
        visibility: Visibility::External,
        input_schema: json!({
            "type": "object",
            "properties": { "name": { "type": "string" } },
            "required": ["name"]
        }),
        output_schema: json!({ /* full OperationSpec schema */ }),
        error_schemas: vec![],
        access_control: AccessControl::default(),
    }
}
```

### Handlers

The handlers need access to the registry. Since handlers are `Arc<dyn Fn>`,
the registry reference is captured in the closure. Use `Arc<OperationRegistry>`
cloned into the closure.

```rust
fn services_list_handler(registry: Arc<OperationRegistry>) -> Handler {
    Arc::new(move |input: Value, ctx: OperationContext| {
        let registry = registry.clone();
        Box::pin(async move {
            let ops: Vec<_> = registry.list_operations()
                .into_iter()
                .filter(|s| s.visibility == Visibility::External)
                .map(|s| json!({
                    "name": s.name,
                    "namespace": s.namespace,
                    "op_type": match s.op_type {
                        OperationType::Query => "query",
                        OperationType::Mutation => "mutation",
                        OperationType::Subscription => "subscription",
                    }
                }))
                .collect();
            ResponseEnvelope::ok(ctx.request_id, json!({ "operations": ops }))
        })
    })
}
```

## Acceptance Criteria

- [ ] `services/list` spec with correct fields (Query, External, no input, output schema)
- [ ] `services/schema` spec with correct fields (Query, External, name input, full spec output)
- [ ] `services/list` handler returns External operations only (Internal excluded)
- [ ] `services/list` output format matches spec (operations array with name, namespace, op_type)
- [ ] `services/schema` handler accepts name with or without leading slash
- [ ] `services/schema` returns full OperationSpec (input_schema, output_schema, error_schemas)
- [ ] `services/schema` returns NOT_FOUND for unknown operation name
- [ ] Both registered as Local provenance, empty authority/env/caps
- [ ] Both have empty AccessControl (callable by all, including unauthenticated)
- [ ] Unit test: services/list returns only External ops
- [ ] Unit test: services/schema returns spec for known op
- [ ] Unit test: services/schema returns NOT_FOUND for unknown op
- [ ] Unit test: services/schema accepts both "fs/readFile" and "/fs/readFile"
- [ ] `cargo test -p alknet-call` succeeds
- [ ] `cargo clippy -p alknet-call` succeeds with no warnings

## References

- docs/architecture/crates/call/operation-registry.md — Service Discovery section
- docs/architecture/decisions/015-privilege-model-and-authority-context.md — ADR-015 (Internal not in services/list)

## Notes

> services/list returns External ops only — Internal ops are implementation
> details of composition and must not be enumerable from the wire. The
> CallAdapter normalizes leading slashes, so services/schema accepts both
> forms. These are the only built-in operations; no admin operations are
> exposed through the call protocol itself.

## Summary

> To be filled on completion