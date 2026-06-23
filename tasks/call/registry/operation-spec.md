---
id: call/registry/operation-spec
name: Implement OperationSpec, OperationType, Visibility, ErrorDefinition, and AccessControl
status: pending
depends_on: [call/crate-init]
scope: moderate
risk: medium
impact: component
level: implementation
---

## Description

Implement the operation specification types in `src/registry/spec.rs`. These
types declare what an operation is, its schemas, and its access control policy.

### OperationSpec

```rust
pub struct OperationSpec {
    pub name: String,              // e.g., "fs/readFile", "agent/chat" (no leading slash)
    pub namespace: String,         // e.g., "fs", "agent"
    pub op_type: OperationType,    // Query, Mutation, Subscription
    pub visibility: Visibility,    // External (wire-callable) or Internal (composition-only)
    pub input_schema: Value,       // JSON Schema for input
    pub output_schema: Value,      // JSON Schema for output
    pub error_schemas: Vec<ErrorDefinition>,  // Declared domain errors (ADR-023)
    pub access_control: AccessControl,
}
```

Operation names use slash-based paths **without a leading slash**, aligned with
URL path conventions: `fs/readFile`, `agent/chat`, `services/list`. The leading
slash is added for display (`spec.path()` returns `/fs/readFile`) and wire
format. The registry stores names without the leading slash.

The `namespace` field is derived from the name: for `fs/readFile` it's `fs`,
for `agent/chat` it's `agent`. It's a convenience accessor for ACL matching and
service grouping.

Implement `OperationSpec::path(&self) -> String` that returns `/{name}` (the
wire/display form with leading slash).

### OperationType

```rust
pub enum OperationType {
    Query,         // Read-only, idempotent (e.g., "fs/readFile", "services/list")
    Mutation,      // Side effects (e.g., "bash/exec", "github/authenticate")
    Subscription,  // Streaming (e.g., "agent/chat", "events/subscribe")
}
```

### Visibility

```rust
pub enum Visibility {
    External,  // Callable from the wire (call.requested from a client)
    Internal,  // Composition-only (env.invoke from a handler)
}
```

`External` operations appear in `services/list` and accept `call.requested`.
`Internal` operations return `NOT_FOUND` when called from the wire and do not
appear in `services/list`. The assembly layer declares visibility at
registration. All import adapters register operations as `Internal` by default
(they're composition material); the handler that composes them is `External`.

### ErrorDefinition

```rust
pub struct ErrorDefinition {
    pub code: String,           // e.g., "FILE_NOT_FOUND", "RATE_LIMITED"
    pub description: String,    // Human-readable description
    pub schema: Value,           // JSON Schema for the error detail payload
    pub http_status: Option<u16>,  // HTTP status for adapter projection (from_openapi/to_openapi)
}
```

A declared operation-level error (ADR-023). When a handler returns a `CallError`
whose `code` matches a declared `ErrorDefinition`, the `call.error` event
carries that code and the error's detail payload. If it doesn't match, the
`call.error` carries `INTERNAL`.

### AccessControl

```rust
pub struct AccessControl {
    pub required_scopes: Vec<String>,          // AND-checked: caller must have ALL
    pub required_scopes_any: Option<Vec<String>>, // OR-checked: caller must have at LEAST ONE
    pub resource_type: Option<String>,          // e.g., "service"
    pub resource_action: Option<String>,        // e.g., "read"
}
```

### ACL check flow

When a `call.requested` event arrives:
1. Registry checks **visibility** — if `Internal`, returns `NOT_FOUND` (does
   not leak existence)
2. Registry checks `access_control.check(identity)`:
   - For external calls (`internal: false`): ACL against the **caller's identity**
   - For internal calls (`internal: true`): ACL against the **handler's
     composition authority** (ADR-015)
3. If denied: `FORBIDDEN`
4. If identity is `None` and operation has restrictions: `FORBIDDEN` with
   message `"authentication required"`

Operations with empty `AccessControl` (no required scopes, no resource checks)
are accessible to all callers, including unauthenticated ones.

### Implement AccessControl::check

```rust
impl AccessControl {
    pub fn check(&self, identity: Option<&Identity>) -> AccessResult;
}

pub enum AccessResult {
    Allowed,
    Forbidden(String),  // reason
}
```

The check logic:
- `required_scopes`: caller must have ALL (subset check)
- `required_scopes_any`: caller must have at LEAST ONE (if present)
- `resource_type` / `resource_action`: check against `identity.resources`
- If `identity` is `None` and any scope/resource is required: `Forbidden("authentication required")`

## Acceptance Criteria

- [ ] `OperationSpec` struct with all 8 fields
- [ ] `OperationSpec::path()` returns `/{name}` (leading slash for wire/display)
- [ ] `OperationSpec::namespace` derived from name (split on `/`)
- [ ] `OperationType` enum with Query, Mutation, Subscription
- [ ] `Visibility` enum with External, Internal
- [ ] `ErrorDefinition` struct with all 4 fields
- [ ] `AccessControl` struct with all 4 fields
- [ ] `AccessControl::check(identity)` returns `AccessResult`
- [ ] `required_scopes` is AND-checked (caller must have all)
- [ ] `required_scopes_any` is OR-checked (caller must have at least one)
- [ ] `None` identity with restrictions → `Forbidden("authentication required")`
- [ ] Empty AccessControl → `Allowed` for all callers
- [ ] Unit tests for AccessControl::check (all combinations)
- [ ] Unit test: OperationSpec::path() produces leading slash
- [ ] Unit test: namespace derived correctly from name
- [ ] `cargo test -p alknet-call` succeeds
- [ ] `cargo clippy -p alknet-call` succeeds with no warnings

## References

- docs/architecture/crates/call/operation-registry.md — OperationSpec, AccessControl, Visibility
- docs/architecture/decisions/015-privilege-model-and-authority-context.md — ADR-015 (visibility, ACL)
- docs/architecture/decisions/023-operation-error-schemas.md — ADR-023 (ErrorDefinition)

## Notes

> Operation names have NO leading slash in the registry (`fs/readFile`). The
> leading slash is added for wire format and display (`/fs/readFile`). This is
> a single rule applied consistently — do not mix the two forms. Visibility
> controls wire-callability: Internal ops return NOT_FOUND from the wire (don't
> leak existence). AccessControl.check is the ACL gate — read it carefully
> against ADR-015 for the internal vs external authority distinction.

## Summary

> To be filled on completion