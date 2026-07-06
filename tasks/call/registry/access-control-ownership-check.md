---
id: call/registry/access-control-ownership-check
name: Update AccessControl::check to consult OwnershipProvider for dynamic resource ownership (ADR-050 §2)
status: completed
depends_on: [core/ownership-store-trait]
scope: moderate
risk: medium
impact: component
level: implementation
---

## Description

Update `AccessControl::check` to accept `resource_id` and an optional
`OwnershipProvider` reference, consulting the provider for runtime-spawned
resource ownership checks. Per ADR-050 §2.

### The signature change

Current (in `crates/alknet-call/src/registry/spec.rs`):

```rust
pub fn check(&self, identity: Option<&Identity>) -> AccessResult
```

Updated:

```rust
pub fn check(
    &self,
    identity: Option<&Identity>,
    resource_id: Option<&str>,
    ownership: Option<&dyn OwnershipProvider>,
) -> AccessResult
```

### Check logic

```
1. Scope check (unchanged): identity.scopes ⊇ required_scopes.
   If identity is None and scopes are required, deny here.
   (Same as current — no change to the scope-check path.)

2. Resource check (only if self.resource_type is Some):
   a. resource_id Some + ownership Some:
        → identity must be Some (owns takes &Identity, not Option);
          if identity is None, deny.
        → p.owns(identity, resource_type, resource_id, resource_action)
        → if false, Forbidden("not owner of resource")
   b. resource_id None + ownership Some (the `list` case, ADR-050 §4a):
        → if identity is None, deny.
        → p.owns_any(identity, resource_type)  [scope-gate]
        → if false, Forbidden("no owned resources of type")
   c. ownership None → fall back to static
        → identity.resources[resource_type] ∋ resource_action
        (backward compat for non-runtime resources — the current logic)
```

### Backward compatibility

When `ownership` is `None`, `check` falls back to the static
`Identity.resources` path — the existing resource-check logic (lines 83-105
of `spec.rs`) runs unchanged. This means operations with static resource
sets work without wiring an ownership provider. The ownership provider is
an additional check, not a replacement.

When `resource_id` is `None` and `ownership` is `None`, the existing logic
also runs unchanged (the `resource_action` without a specific resource ID
path, lines 97-105). This covers operations that have `resource_action` but
no `resource_type` — those stay on the static path.

### Call sites to update

There are **6 call sites** for `check()` that need the signature update:

1. `registry/registration.rs:142` — `acl.check(identity.as_ref())` in `invoke()`
2. `registry/registration.rs:194` — `acl.check(identity.as_ref())` in `invoke_streaming()`
3. `protocol/connection.rs:351` — `access_control.check(caller_identity.as_ref())` in `invoke_with_policy()`
4. `registry/discovery.rs:224` — `spec.access_control.check(calling_identity)` in `services/list`
5. `registry/discovery.rs:247` — `spec.access_control.check(calling_identity)` in `services/schema`
6. `registry/discovery.rs:269` — `reg.spec.access_control.check(calling_identity)` in `services/list-peers`

For sites 1-3 (dispatch path): pass `resource_id: None` and
`ownership: None` for now — the dispatch-path wiring (extracting
`resource_id` from input via `spec.resource_id_path` and threading the
ownership provider) is `call/registry/dispatch-resource-id-extraction`,
which depends on this task. This task changes the signature and the check
logic; the dispatch task wires the new parameters.

For sites 4-6 (service discovery): these are `services/list` /
`services/schema` / `services/list-peers` filtering — they check whether a
calling peer can see an operation at all. Pass `resource_id: None` and
`ownership: None` — service discovery doesn't do per-resource ownership
checks (it's scope-gating only). The `list` result-filter (ADR-050 §4a) is
a handler-level concern, not a service-discovery concern.

### What this task does NOT do

- Does NOT extract `resource_id` from the input or thread the ownership
  provider into the dispatch path — that's
  `call/registry/dispatch-resource-id-extraction`, which depends on this
  task. This task changes the signature and the check logic; all call sites
  pass `None, None` and behave exactly as before.
- Does NOT add `resource_id_path` to `OperationSpec` — that's
  `call/registry/operation-spec-resource-id-path`, which is independent
  (no dependency).

### Tests

The existing tests in `spec.rs` (lines 157-327) all call `check(identity)`
with one argument. They need to be updated to `check(identity, None, None)`
— the behavior should be identical (backward compat). Add new tests for
the ownership-provider path:

- `resource_id Some + ownership Some + provider says owns → Allowed`
- `resource_id Some + ownership Some + provider says not owns → Forbidden`
- `resource_id Some + ownership Some + identity None → Forbidden`
- `resource_id None + ownership Some + provider says owns_any → Allowed` (the `list` scope-gate)
- `resource_id None + ownership Some + provider says not owns_any → Forbidden`
- `ownership None + resource_type Some → static fallback (existing behavior)`
- A mock `OwnershipProvider` impl for testing

## Acceptance Criteria

- [ ] `AccessControl::check` signature updated: `(identity, resource_id, ownership) -> AccessResult`
- [ ] Scope check path unchanged (required_scopes, required_scopes_any)
- [ ] Resource check path: when `ownership` is Some, consults the provider
- [ ] Resource check path: when `ownership` is None, falls back to static `Identity.resources` (backward compat)
- [ ] `resource_id None + ownership Some` → calls `owns_any` (the `list` scope-gate)
- [ ] `resource_id Some + ownership Some` → calls `owns`
- [ ] `identity None + ownership Some + resource_type Some` → Forbidden (owns needs &Identity)
- [ ] All 6 existing call sites updated to pass `(None, None)` — behavior unchanged
- [ ] All existing tests updated to `check(identity, None, None)` — behavior unchanged
- [ ] New tests with a mock `OwnershipProvider` for the dynamic path
- [ ] `cargo test -p alknet-call` succeeds
- [ ] `cargo clippy -p alknet-call` succeeds with no warnings

## References

- docs/architecture/crates/call/operation-registry.md — AccessControl (updated with the new check signature)
- docs/architecture/decisions/050-dynamic-resource-ownership-for-runtime-spawned-resources.md — ADR-050 §2, §4a
- crates/alknet-call/src/registry/spec.rs — current AccessControl::check implementation
- tasks/core/ownership-store-trait.md — the OwnershipProvider trait (dependency)

## Notes

> This is the one-way door — the `check` signature change touches every
> call site and test. The key design property: backward compatibility when
> `ownership` is `None`. All existing call sites pass `(None, None)` in
> this task and behave exactly as before. The dispatch-path wiring
> (extracting `resource_id` from input and threading a real ownership
> provider) is the next task. This task is the signature + logic change;
> the next task is the wiring.