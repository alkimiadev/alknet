---
id: call/registry/operation-env
name: Implement OperationEnv trait, LocalOperationEnv, and CompositeOperationEnv
status: pending
depends_on: [call/registry/handler-registration]
scope: broad
risk: high
impact: component
level: implementation
---

## Description

Implement the `OperationEnv` trait and its implementations in
`src/registry/env.rs`. This is the universal composition mechanism — a handler
calls `context.env.invoke(...)` to compose child operations. The trait-object
design is what enables registry layering (ADR-024).

**Read ADR-024 before starting this task.** The trait-object pattern is
load-bearing — making `OperationEnv` concrete would close the session-overlay
and connection-overlay patterns.

### OperationEnv trait

```rust
#[async_trait]
pub trait OperationEnv: Send + Sync {
    /// Compose a child operation. The child's OperationContext is constructed
    /// with internal: true, inheriting the parent's composition authority as
    /// the child's caller identity. Abort policy defaults to parent's.
    async fn invoke(
        &self,
        namespace: &str,
        operation: &str,
        input: Value,
        parent: &OperationContext,
    ) -> ResponseEnvelope {
        self.invoke_with_policy(namespace, operation, input, parent, parent.abort_policy.clone()).await
    }

    /// Compose with explicit abort policy (ADR-016 Decision 6).
    /// This is the required method — invoke() delegates to it.
    async fn invoke_with_policy(
        &self,
        namespace: &str,
        operation: &str,
        input: Value,
        parent: &OperationContext,
        policy: AbortPolicy,
    ) -> ResponseEnvelope;

    /// Does this env contain the named operation? Used by CompositeOperationEnv
    /// to probe overlays before dispatching (ADR-024).
    fn contains(&self, name: &str) -> bool { true }
}
```

`invoke()` has a default impl that delegates to `invoke_with_policy()` with
the parent's abort policy. Implementations only need to implement
`invoke_with_policy()`.

### LocalOperationEnv (Layer 0)

```rust
pub struct LocalOperationEnv {
    registry: Arc<OperationRegistry>,
}

#[async_trait]
impl OperationEnv for LocalOperationEnv {
    async fn invoke_with_policy(&self, namespace: &str, operation: &str, input: Value, parent: &OperationContext, policy: AbortPolicy) -> ResponseEnvelope {
        let name = format!("{namespace}/{operation}");

        // 1. Reachability check (ADR-015, ADR-022): is this op in parent's scoped env?
        if !parent.scoped_env.allows(&name) {
            return ResponseEnvelope::not_found(name);
        }

        // 2. Look up registration
        let registration = self.registry.registration(&name);

        // 3. Construct child OperationContext
        let context = OperationContext {
            request_id: generate_request_id(),  // UUID v4 — NOT deterministic
            parent_request_id: Some(parent.request_id.clone()),
            identity: parent.handler_identity.as_identity(),  // authority switch
            handler_identity: registration.composition_authority.clone(),
            capabilities: parent.capabilities.clone(),  // inherit
            metadata: HashMap::new(),  // fresh — does NOT propagate parent metadata (ADR-014)
            abort_policy: policy,
            deadline: parent.deadline,  // inherit — children don't get fresh 30s
            scoped_env: registration.scoped_env.clone().unwrap_or_else(ScopedOperationEnv::empty),
            env: parent.env.clone(),  // inherit the same composite env
            internal: true,  // nested calls use handler authority
        };

        // 4. Dispatch
        self.registry.invoke(&name, input, context).await
    }

    // contains() uses default (returns true — curated registry contains everything it can dispatch)
}
```

Key points:
- **Reachability check first**: if op not in parent's scoped_env, NOT_FOUND.
  This bounds the parameterized-dispatch attack surface.
- **Authority propagation**: child's `identity` = parent's `handler_identity`
  (the parent's composition authority becomes the caller). This is the
  authority switch from ADR-015.
- **Fresh metadata**: `HashMap::new()`, NOT parent's metadata. Security
  constraint (ADR-014) — prevents secret leakage through composition.
- **Inherited deadline**: children don't get a fresh 30s — the root call's
  deadline bounds the entire call tree.
- **Inherited env**: child gets `parent.env.clone()` (the same composite of
  curated base + active overlays).
- **internal: true**: this is the flag that switches ACL authority.

### CompositeOperationEnv (per-call, ADR-024)

```rust
pub struct CompositeOperationEnv {
    session: Option<Arc<dyn OperationEnv + Send + Sync>>,    // Layer 1
    connection: Option<Arc<dyn OperationEnv + Send + Sync>>, // Layer 2
    base: Arc<dyn OperationEnv + Send + Sync>,               // Layer 0 (LocalOperationEnv)
}

#[async_trait]
impl OperationEnv for CompositeOperationEnv {
    async fn invoke_with_policy(&self, namespace: &str, operation: &str, input: Value, parent: &OperationContext, policy: AbortPolicy) -> ResponseEnvelope {
        let name = format!("{namespace}/{operation}");

        // Reachability check (same as LocalOperationEnv)
        if !parent.scoped_env.allows(&name) {
            return ResponseEnvelope::not_found(name);
        }

        // Dispatch in overlay order: session → connection → curated base
        // First overlay that *contains* the op wins
        if let Some(session) = &self.session {
            if session.contains(&name) {
                return session.invoke_with_policy(namespace, operation, input, parent, policy).await;
            }
        }
        if let Some(connection) = &self.connection {
            if connection.contains(&name) {
                return connection.invoke_with_policy(namespace, operation, input, parent, policy).await;
            }
        }
        self.base.invoke_with_policy(namespace, operation, input, parent, policy).await
    }

    fn contains(&self, name: &str) -> bool {
        self.session.as_ref().map_or(false, |s| s.contains(name))
            || self.connection.as_ref().map_or(false, |c| c.contains(name))
            || self.base.contains(name)
    }
}
```

The `contains()` method (review #003 C9) is the overlay-dispatch contract. It
replaces the previous ambiguous "sentinel or contains check" framing. The
structural decision (composite trait object, overlay order, Arc::clone
inheritance) is locked by ADR-024; the dispatch contract (contains probe before
invoke_with_policy) is locked too.

### Why OperationEnv must remain a trait

The trait-based design enables registry layering (ADR-024):
- The CallAdapter composes the root env per call from curated base + active
  connection/session overlays
- Overlays wrap the base via trait layering
- Session-scoped registries (OQ-19) and connection-scoped remote imports
  (ADR-017 `from_call`) are both overlays on the same base

Making `OperationEnv` concrete or hardcoding the global registry into the
dispatch path would close both patterns. This is the same integration-point
pattern as `IdentityProvider` (ADR-004).

## Acceptance Criteria

- [ ] `OperationEnv` trait with `invoke()`, `invoke_with_policy()`, `contains()`
- [ ] `invoke()` has default impl delegating to `invoke_with_policy()` with parent's policy
- [ ] `contains()` has default impl returning `true`
- [ ] `LocalOperationEnv` struct holding `Arc<OperationRegistry>`
- [ ] `LocalOperationEnv::invoke_with_policy` checks reachability (scoped_env.allows)
- [ ] `LocalOperationEnv` constructs child context with internal: true, authority switch
- [ ] `LocalOperationEnv` fresh metadata (HashMap::new(), not parent's)
- [ ] `LocalOperationEnv` inherited deadline (parent.deadline, not fresh 30s)
- [ ] `LocalOperationEnv` inherited env (parent.env.clone())
- [ ] `CompositeOperationEnv` with session, connection, base fields
- [ ] `CompositeOperationEnv::invoke_with_policy` dispatches in overlay order (session → connection → base)
- [ ] `CompositeOperationEnv` uses `contains()` probe before dispatching to overlay
- [ ] `CompositeOperationEnv::contains` returns true if any layer contains the op
- [ ] Reachability check returns NOT_FOUND if op not in scoped_env
- [ ] Unit test: LocalOperationEnv invoke with allowed op → dispatches
- [ ] Unit test: LocalOperationEnv invoke with disallowed op → NOT_FOUND
- [ ] Unit test: child context has internal: true
- [ ] Unit test: child context identity = parent's handler_identity
- [ ] Unit test: child metadata is fresh (empty), not parent's
- [ ] Unit test: CompositeOperationEnv dispatches to session overlay if contains
- [ ] Unit test: CompositeOperationEnv falls through to base if no overlay contains
- [ ] `cargo test -p alknet-call` succeeds
- [ ] `cargo clippy -p alknet-call` succeeds with no warnings

## References

- docs/architecture/crates/call/operation-registry.md — OperationEnv, LocalOperationEnv, CompositeOperationEnv
- docs/architecture/decisions/015-privilege-model-and-authority-context.md — ADR-015 (authority switch)
- docs/architecture/decisions/016-abort-cascade-for-nested-calls.md — ADR-016 (abort policy propagation)
- docs/architecture/decisions/024-operation-registry-layering.md — ADR-024 (layering, contains contract)

## Notes

> **Read ADR-024 before starting.** The trait-object design is load-bearing —
> OperationEnv MUST remain a trait, not a concrete type. The authority switch
> (child identity = parent handler_identity) is the ADR-015 privilege model.
> Metadata does NOT propagate (ADR-014 security constraint). Deadline
> inherits (children don't get fresh 30s). The `contains()` probe is the
> overlay-dispatch contract from review #003 C9 — any OperationEnv impl that
> correctly reports contains works with the composite.

## Summary

> To be filled on completion