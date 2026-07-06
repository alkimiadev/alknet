---
id: call/registry/operation-spec-resource-id-path
name: Add resource_id_path field to OperationSpec (ADR-050 §2a)
status: completed
depends_on: []
scope: single
risk: low
impact: component
level: implementation
---

## Description

Add a `resource_id_path: Option<String>` field to `OperationSpec`. This is
a JSON pointer into the operation input that tells the dispatcher where to
find the resource ID for runtime-spawned resource authorization. Per ADR-050
§2a.

### The field

```rust
pub struct OperationSpec {
    pub name: String,
    pub namespace: String,
    pub op_type: OperationType,
    pub visibility: Visibility,
    pub input_schema: Value,
    pub output_schema: Value,
    pub error_schemas: Vec<ErrorDefinition>,
    pub access_control: AccessControl,
    /// JSON pointer into the input for the resource ID, when
    /// `access_control.resource_type` is set and the operation targets a
    /// specific runtime-spawned resource (ADR-050). e.g., `"$.containerId"`
    /// for `docker/container/exec`. Absent for no-specific-resource
    /// operations (the `list` case). `None` for operations with no
    /// `resource_type` or with static resource sets.
    pub resource_id_path: Option<String>,
}
```

### Construction

`OperationSpec::new(...)` currently takes 7 arguments (name, op_type,
visibility, input_schema, output_schema, error_schemas, access_control).
This task adds `resource_id_path` as an 8th argument. Since all
construction sites need to update, consider whether a builder pattern is
worth introducing — but for now, add it as the last positional argument
(defaulting to `None` at call sites that don't need it).

### What this task does NOT do

- Does NOT change `AccessControl::check` — that's
  `call/registry/access-control-ownership-check`, which depends on
  `core/ownership-store-trait`.
- Does NOT extract the resource ID from input or pass it to `check` —
  that's `call/registry/dispatch-resource-id-extraction`, which depends on
  this task.
- This task only adds the field to the struct, updates `new()`, and updates
  all existing construction sites + tests.

### Existing construction sites

Search for `OperationSpec::new(` across the codebase — every call site
needs the new argument added. Most will pass `None` (no runtime-spawned
resources). The existing tests in `spec.rs` construct `OperationSpec`
without `resource_id_path` — they all need the `None` argument added.

## Acceptance Criteria

- [ ] `OperationSpec` struct has `resource_id_path: Option<String>` field
- [ ] `OperationSpec::new(...)` takes `resource_id_path` as the 8th argument
- [ ] All existing `OperationSpec::new(...)` call sites updated (most pass `None`)
- [ ] All existing tests that construct `OperationSpec` updated
- [ ] Existing tests still pass (no semantic change — `None` means "no resource ID extraction")
- [ ] Unit test: `resource_id_path` is `None` by default when not specified
- [ ] `cargo test -p alknet-call` succeeds
- [ ] `cargo clippy -p alknet-call` succeeds with no warnings

## References

- docs/architecture/crates/call/operation-registry.md — OperationSpec (updated with `resource_id_path`)
- docs/architecture/decisions/050-dynamic-resource-ownership-for-runtime-spawned-resources.md — ADR-050 §2a
- crates/alknet-call/src/registry/spec.rs — current OperationSpec struct

## Notes

> The fit with JSON Schema is load-bearing: `input_schema` is already a
> JSON Schema, so `resource_id_path` is a pointer *within* an existing
> schema on the same spec. The `OperationSpec` becomes fully
> self-describing for authorization — what resource type, what action,
> and *which input field* drives the resource lookup. This is a single
> field addition with no semantic change — existing call sites pass
> `None` and behave exactly as before. The field is consumed by the
> dispatch path in `call/registry/dispatch-resource-id-extraction`.