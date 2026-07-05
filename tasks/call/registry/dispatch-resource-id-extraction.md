---
id: call/registry/dispatch-resource-id-extraction
name: Wire dispatch path to extract resource_id from input and thread OwnershipProvider to AccessControl::check (ADR-050 §2a, §4a)
status: pending
depends_on: [call/registry/operation-spec-resource-id-path, call/registry/access-control-ownership-check]
scope: moderate
risk: medium
impact: component
level: implementation
---

## Description

Wire the dispatch path to: (a) extract `resource_id` from the operation
input using `spec.resource_id_path`, and (b) thread an `OwnershipProvider`
to `AccessControl::check`. This is the task that makes the dynamic
ownership model actually work end-to-end. Per ADR-050 §2a and §4a.

### Where the wiring happens

The dispatch flow is:

```
dispatch()                           // protocol/dispatch.rs:220
  → extract input from payload       // line 238
  → build_root_context(...)          // line 246
  → registry.invoke(name, input, context)  // line 261
      → look up registration         // registration.rs:123
      → check visibility             // registration.rs:128
      → check ACL: acl.check(...)   // registration.rs:142  ← THIS IS WHERE resource_id + ownership go
      → call handler
```

`registry.invoke()` (in `registration.rs:116`) has access to both `input`
and `registration.spec` (which now has `resource_id_path`). The resource ID
extraction and ownership provider threading happen here.

### resource_id extraction (ADR-050 §2a)

In `invoke()` and `invoke_streaming()`, before the `check()` call:

```rust
// Extract resource_id from input using spec.resource_id_path
let resource_id = registration.spec.resource_id_path
    .as_ref()
    .and_then(|path| extract_json_pointer(&input, path));
```

`extract_json_pointer` parses a JSON pointer like `"$.containerId"` and
extracts the value at that path from the input `Value`. If the pointer is
`None`, or the path doesn't exist in the input, `resource_id` is `None`.

**JSON pointer parsing:** the `resource_id_path` uses the `$.field` syntax
(JSON Pointer with a leading `$`). Use the `serde_json` pointer API or a
small helper. `serde_json::Value::pointer()` takes a `/field` path — strip
the leading `$.` and replace with `/`, or use a simple helper that handles
the `$.` prefix. If the path is malformed or the field is missing,
`resource_id` is `None` (graceful — the check will deny if the spec
requires a resource_type but no ID is available).

### OwnershipProvider threading

The `OwnershipProvider` needs to be available in `invoke()`. Two options:

**Option A: thread it through `OperationContext`.** Add an
`ownership: Option<Arc<dyn OwnershipProvider>>` field to
`OperationContext`. `build_root_context` populates it from the
`CallAdapter` (which holds `Arc<dyn OwnershipProvider>`). `invoke()` reads
`context.ownership.as_deref()` and passes it to `check()`.

**Option B: hold it on the `OperationRegistry`.** The registry holds
`Option<Arc<dyn OwnershipProvider>>` and `invoke()` reads it directly.

**Recommendation: Option A (OperationContext).** It's consistent with how
`identity`, `capabilities`, and `env` are threaded — all on
`OperationContext`. The registry stays stateless (no new field); the
context carries the ownership provider the same way it carries the
identity provider's output. The assembly layer wires the ownership
provider into the `CallAdapter` at construction, and
`build_root_context` threads it onto the context.

### What changes

1. **`OperationContext` gains `ownership: Option<Arc<dyn OwnershipProvider>>`**
   (in `operation-registry.md` / `operation-context.md` / the struct in
   `registry/operation_context.rs` or wherever `OperationContext` is
   defined). `None` when no ownership provider is wired (backward compat —
   `check` falls back to static path).

2. **`build_root_context`** (in `protocol/dispatch.rs:148`) populates
   `ownership` from `self.ownership_provider` (a new field on
   `Dispatcher`/`CallAdapter`, set at construction by the assembly layer).

3. **`invoke()` and `invoke_streaming()`** (in `registration.rs`) extract
   `resource_id` from `input` via `spec.resource_id_path`, then call
   `acl.check(identity.as_ref(), resource_id.as_deref(),
   context.ownership.as_deref())`.

4. **`invoke_with_policy()` in `connection.rs:311`** — the composition path
   (internal calls via `OperationEnv::invoke`). Same pattern: extract
   `resource_id` from `input` using the child op's `resource_id_path`,
   thread the ownership provider from the parent context.

5. **`Dispatcher` / `CallAdapter`** gains an
   `ownership_provider: Option<Arc<dyn OwnershipProvider>>` field, set at
   construction. `None` by default (backward compat — deployments without
   runtime-spawned resources don't wire one).

### What this task does NOT do

- Does NOT build a persistence adapter — the in-memory
  `InMemoryOwnershipStore` from `core/ownership-store-trait` is the default.
- Does NOT build any handler that calls `record`/`revoke` — that's
  downstream crate work (alknet-docker, etc.).
- Does NOT change `services/list` / `services/schema` / `services/list-peers`
  — those stay on the `(None, None)` path (scope-gating only, no per-resource
  ownership check at discovery time).

### Tests

- Integration test: an operation with `resource_type` + `resource_id_path`
  + an `OwnershipProvider` wired → `check` consults the provider → Allowed
  when the provider says owns.
- Integration test: same setup but provider says not owns → Forbidden.
- Integration test: `resource_id_path` points to a field missing from input
  → `resource_id` is None → `list` path (owns_any) or Forbidden (if
  resource_type requires a specific ID).
- Integration test: no ownership provider wired (`ownership: None` on
  context) → static `Identity.resources` fallback (backward compat).
- Unit test: `extract_json_pointer` extracts `$.containerId` from
  `{"containerId": "abc123"}` → `Some("abc123")`.
- Unit test: `extract_json_pointer` on missing field → `None`.
- Unit test: `resource_id_path: None` → `resource_id: None`.

## Acceptance Criteria

- [ ] `OperationContext` has `ownership: Option<Arc<dyn OwnershipProvider>>` field
- [ ] `Dispatcher` / `CallAdapter` holds `ownership_provider: Option<Arc<dyn OwnershipProvider>>`, set at construction
- [ ] `build_root_context` populates `context.ownership` from the dispatcher's provider
- [ ] `invoke()` extracts `resource_id` from `input` via `spec.resource_id_path`, passes `(resource_id, context.ownership.as_deref())` to `check()`
- [ ] `invoke_streaming()` does the same
- [ ] `invoke_with_policy()` in `connection.rs` does the same for the composition path
- [ ] JSON pointer extraction helper handles `$.field` syntax
- [ ] Missing field in input → `resource_id: None` (graceful, not a panic)
- [ ] `resource_id_path: None` → `resource_id: None`
- [ ] No ownership provider wired → `check` falls back to static path (backward compat)
- [ ] Integration test: provider says owns → Allowed
- [ ] Integration test: provider says not owns → Forbidden
- [ ] Integration test: missing field → `list` path or Forbidden
- [ ] `cargo test -p alknet-call` succeeds
- [ ] `cargo clippy -p alknet-call` succeeds with no warnings

## References

- docs/architecture/crates/call/operation-registry.md — AccessControl (dispatch flow with resource_id extraction)
- docs/architecture/decisions/050-dynamic-resource-ownership-for-runtime-spawned-resources.md — ADR-050 §2a, §4a, §4d
- crates/alknet-call/src/protocol/dispatch.rs — dispatch() and build_root_context()
- crates/alknet-call/src/registry/registration.rs — invoke() and invoke_streaming() (the check() call sites)
- crates/alknet-call/src/protocol/connection.rs — invoke_with_policy() (composition path)
- tasks/call/registry/operation-spec-resource-id-path.md — the resource_id_path field (dependency)
- tasks/call/registry/access-control-ownership-check.md — the check() signature change (dependency)
- tasks/core/ownership-store-trait.md — the OwnershipProvider trait (dependency)

## Notes

> This is the wiring task — it connects the pieces from the three
> dependency tasks into a working end-to-end flow. The key design choice is
> threading the ownership provider via `OperationContext` (consistent with
> how identity, capabilities, and env are threaded), not via the registry
> (which stays stateless). The assembly layer wires
> `Option<Arc<dyn OwnershipProvider>>` into the `CallAdapter` at
> construction; `None` is the default for deployments without
> runtime-spawned resources (backward compat). The JSON pointer extraction
> is a small helper — `$.containerId` → look up `containerId` in the input
> Value. Missing fields are graceful (None), not errors — the check denies
> if the spec requires a resource_type but no ID is available.