# ADR-015: Privilege Model and Authority Context

## Status

Accepted

## Context

The call protocol allows handlers to compose other operations through
`OperationEnv::invoke()`. This creates a call tree: a parent request spawns
children, which may spawn their own children. The `parent_request_id` field
records this tree.

The previous design had a `trusted: bool` flag on `OperationContext`. When a
handler invoked another operation through `OperationEnv`, the nested call was
marked `trusted: true` and **all ACL checks were skipped**. The intent was to
avoid double-checking: if `/agent/chat` is allowed and it internally calls
`/auth/verify`, the auth check is "trusted" because the caller already passed
ACL on `/agent/chat`.

This is a privilege escalation vector. Two concrete attacks:

**Buggy handler**: a handler accidentally calls an operation it shouldn't. With
`trusted: true`, ACL is skipped entirely. A handler with `read` scope that
accidentally calls an operation requiring `admin` succeeds — the caller's `read`
scope effectively triggered an `admin` operation.

**Parameterized dispatch**: a handler takes caller input that determines which
internal operation to call. This is the core agent use case — an LLM picks which
tool to invoke based on the user's prompt. With `trusted: true`, the LLM (and
therefore the user) can invoke any registered operation without ACL checks,
regardless of the caller's scopes. A caller with `chat` scope can invoke
operations requiring `admin` by choosing the right tool name.

The call protocol is a general-purpose cross-boundary RPC mechanism. Every
consumer — NAPI adapter, Python adapter, agent service, future services —
inherits whatever privilege model the protocol defines. The privilege boundary
between external and internal calls, and the authority context switch for
composition, are core protocol semantics. This is not a feature of any single
consumer; it is the protocol's security model.

The agent service is a useful test case because it exercises every edge case
(parameterized dispatch, deep composition, dynamic operations, role-based
escalation), but the decision belongs to the call protocol.

## Mental Models

Two analogies clarify the model:

**Kernel/user mode**: external operations are syscalls — curated entry points
where an unprivileged caller can enter the kernel. Internal operations are
kernel functions — callable only from composition, not from userspace. The
`internal` flag means "this call is in kernel mode." Kernel mode has access
controls — it runs under a different principal, not with no principal.

**Domain/integration events**: external operations are integration events —
they cross a boundary and are visible to external systems. Internal operations
are domain events — they stay within the bounded context. `services/list` is
the integration contract; it only exposes integration events.

**Principal/agent (legal contracting)**: the caller is the principal; the
handler is the agent. The principal delegates scoped authority to the agent.
The agent acts under its own identity (for attribution) but with the principal's
delegated authority (for scope). Liabilities flow upstream (traceable through
`parent_request_id`); privileges flow downstream (the agent gets a subset of the
principal's authority). Role-based escalation: a lower-privileged role can
escalate through a chain of command (agent requests promotion, architect
performs it), not through direct authority.

## Decision

### 1. The `internal` flag switches authority context, not skips ACL

The `internal` flag on `OperationContext` marks calls that originated from
composition (a handler calling another operation via `OperationEnv`), as opposed
to external calls that arrived as `call.requested` from a wire client.

When `internal: true`:
- The ACL check runs against the **handler's identity** (set at registration by
  the assembly layer), not the caller's identity and not as a blanket skip.
- The handler's identity has scopes scoped to its composition needs (least
  privilege), not blanket root and not the caller's scopes.

When `internal: false` (external call from the wire):
- The ACL check runs against the **caller's identity** (from `AuthContext`,
  resolved per-request).

The `internal` flag is set by `OperationEnv`, not by callers. A handler cannot
mark its own call as internal. The field uses module-private construction; only
`pub fn is_internal(&self) -> bool` is exposed for reads.

### 2. Operations have External/Internal visibility

`OperationSpec` has a `visibility: Visibility` field:

```rust
pub enum Visibility {
    External,  // Callable from the wire (call.requested from a client)
    Internal,  // Composition-only (env.invoke from a handler)
}
```

The assembly layer declares visibility when registering operations.

When a `call.requested` arrives from a wire client:
- An `Internal` operation returns `call.error` with code `NOT_FOUND` (not
  `FORBIDDEN`). This does not leak that the operation exists.
- An `External` operation proceeds to ACL checking.

`services/list` only returns `External` operations to remote callers. Internal
operations are not part of the wire-facing API surface. A remote client cannot
enumerate the internal call tree.

### 3. Handler identity is carried on OperationContext

> **Note**: This decision's `handler_identity: Option<Identity>` type was
> superseded by ADR-022, which replaced `Identity` with
> `CompositionAuthority` — a declared authority bundle that is not a peer
> identity and is not resolvable through `IdentityProvider`. The core
> decision (authority switch, not ACL skip) holds unchanged. See ADR-022
> Decision 2 for the current type.

`OperationContext` carries both the caller's identity (who invoked me) and
the handler's identity (who am I acting as):

```rust
pub struct OperationContext {
    pub request_id: String,
    pub parent_request_id: Option<String>,
    pub identity: Option<Identity>,            // Caller's identity (inbound)
    // Type changed to Option<CompositionAuthority> by ADR-022:
    pub handler_identity: Option<CompositionAuthority>,  // Handler's composition authority
    pub capabilities: Capabilities,
    pub metadata: HashMap<String, Value>,
    pub env: OperationEnv,
    /// Module-private for writes; read via `is_internal()`. Set only by
    /// `OperationEnv::invoke()` (true) or `CallAdapter` dispatch (false).
    pub(crate) internal: bool,
}

impl OperationContext {
    pub fn is_internal(&self) -> bool { self.internal }
}
```

- `identity`: the authenticated caller (from `AuthContext`). For external calls,
  this is who sent the `call.requested`. For internal calls, this is the
  *parent handler's* identity (propagated through `OperationEnv::invoke()`).
- `handler_identity`: the identity of the handler processing this call. Set at
  registration by the assembly layer. For external calls, this is the handler's
  own identity. For internal calls, the ACL check runs against this identity.

The distinction is the principal/agent model: `identity` is the principal (who
delegated), `handler_identity` is the agent (who is acting). Attribution traces
through both — any action can be attributed to the handler that performed it and
the caller that initiated the chain.

### 4. Scoped composition env

The `OperationEnv` given to a handler is scoped — it can only invoke a declared
set of operations. This bounds the parameterized-dispatch attack surface: a
caller (or an LLM) picking which operation to invoke picks from the declared
set, not from the entire registry.

Scoping happens at two levels:

**Static scoping at registration**: the assembly layer declares which operations
a handler may compose. The `OperationEnv` given to that handler is pre-filtered
— `invoke("fs", "readFile", ...)` works, `invoke("admin", "deleteUser", ...)`
returns `NOT_FOUND`. This is the reachability control.

**Dynamic scoping at sandbox creation**: when a handler spawns a sandbox
(quickjs), it passes a *further scoped* env to the sandbox — a subset of what
the handler itself can reach. The handler might have `fs:read` and `bash:exec`,
but it only gives the sandbox `fs:read` (not `bash:exec`), because the sandbox
runs untrusted LLM-generated code. This is the "privileges flow downstream"
principle: the principal delegates a subset.

The specific API for declaring the scoped operation set is specified in
ADR-022: `ScopedOperationEnv { allowed_operations: HashSet<String> }`,
operation-level granularity (not just namespace-level). This is finer-grained
than the TypeScript `@alkdev/operations` `buildEnv()` which used
`allowedNamespaces` — operation-level scoping is safer for the
parameterized-dispatch use case.

### 5. The three controls together

The three controls are independent and all are needed:

| Control | What it gates | Without it |
|---------|--------------|-----------|
| Operation visibility | Whether an operation is callable from the wire | Internal operations exposed to external callers |
| Handler identity | What authority composition runs under | ACL skipped or caller's scopes propagated (escalation) |
| Scoped composition env | What operations a handler can reach | Handler can call anything in the registry |

- Visibility alone: internal operations are hidden from the wire, but
  composition skips ACL (escalation through buggy handler).
- Handler identity alone: ACL checks against handler scopes, but the handler can
  reach any operation (parameterized dispatch unbounded).
- Scoped env alone: handler can only reach declared operations, but ACL is
  skipped (if a declared operation requires a scope the handler doesn't have, it
  still runs).

All three together: the handler can only reach declared operations (scoped env),
those operations are ACL-checked against the handler's scoped identity (handler
identity), and internal operations are never exposed to the wire (visibility).
Principle of least privilege.

## Consequences

**Positive:**
- No privilege escalation through composition. A handler can only compose
  operations its own identity is authorized for, and only from its declared
  scope.
- Parameterized dispatch is safe. The agent/LLM tool selection case is bounded
  by the scoped env — the LLM picks from the declared tool set, not from the
  entire registry. The ACL checks against the handler's identity, not the
  caller's.
- Buggy handlers can't accidentally escalate. A handler that tries to call an
  operation outside its scoped env gets `NOT_FOUND`; one that calls an operation
  its identity lacks scopes for gets `FORBIDDEN`.
- Attribution is complete. Every call carries both the caller's identity (who
  initiated the chain) and the handler's identity (who is acting). The
  `parent_request_id` chain traces the full agency chain. This supports the
  gitea-per-agent pattern where each agent (human or LLM) has its own account.
- Session-scoped operations (OQ-19) are safe by construction. They're always
  `Internal`, run under the handler's identity, through the scoped env, in a
  locked-down sandbox. The self-improving workflow (agents writing tools) is
  bounded.
- Role-based escalation is explicit. An agent requesting promotion (session →
  core) is a lower-privileged role asking a higher-privileged role (architect
  with `promote` scope) to perform an action. The escalation goes through the
  chain of command, not through direct authority.

**Negative:**
- `OperationContext` has two identity fields (`identity` and
  `handler_identity`), which is more complex than a single identity. This is
  necessary — the principal/agent distinction is real and both are needed for
  attribution and ACL.
- The assembly layer has more responsibility: it must declare each handler's
  identity (scopes), its scoped composition env (which operations it may
  compose), and operation visibility. This is expected — the assembly layer
  assembles everything (ADR-008), and forcing explicit declaration of privilege
  is a feature, not a bug.
- Adding a new composition to a handler requires updating the assembly layer
  (declare the new operation in the scoped env), not just the handler code.
  This prevents accidental composition of unauthorized operations.
- The scoped env API is not fully specified here. The one-way constraint
  (scoped env exists, is declared at registration, can be further scoped at
  runtime) is fixed; the concrete API is a two-way door for implementation.

## Assumptions

1. **Internal calls should run under a different authority than external calls,
   not skip ACL entirely.** If internal calls should skip ACL (the old `trusted`
   model), this entire ADR is wrong. The assumption is that the escalation
   vectors (buggy handler, parameterized dispatch) are real and must be
   prevented.

2. **Handler identity is set at registration by the assembly layer.** The
   assembly layer is the trust boundary (ADR-008, ADR-014). If the assembly
   layer is compromised, all handler identities are compromised. This is the
   same trust boundary as capabilities.

3. **The scoped env is declared at registration (static) and can be further
   scoped at runtime (dynamic, for sandbox creation).** The static scoping is
   the reachability control; the dynamic scoping is the sandbox boundary. If a
   use case requires fully dynamic scoping (handler discovers at call time what
   it can compose), the model needs extension — but the assumption is that
   composition reachability is knowable at registration time.

4. **`services/list` hides internal operations.** If internal operations should
   be discoverable by remote callers (e.g., for debugging), the visibility model
   needs a third state. The assumption is that internal operations are
   implementation details, not part of the external API surface.

5. **Internal operations return `NOT_FOUND`, not `FORBIDDEN`.** This prevents
   existence leakage. If a use case requires distinguishing "you can't call
   this" from "this doesn't exist" (e.g., for debugging), the error model needs
   refinement. The assumption is that not leaking internal operation existence
   is more important than debuggability from the wire.

6. **The handler identity is a full `Identity` (with scopes), not a special
   principal type.** ~~This reuses the existing `Identity` type and
   `IdentityProvider` infrastructure (ADR-004).~~ **Superseded by ADR-022
   Decision 2**: composition authority is a declared authority bundle
   (`CompositionAuthority`), not a peer `Identity`. It is not resolvable
   through `IdentityProvider` and does not represent an inbound caller. The
   distinction is necessary because a handler is not a network peer — its
   authority is declared by the assembly layer at registration, not resolved
   from credentials.

## References

- ADR-004: Auth as shared core (`IdentityProvider`, `Identity`)
- ADR-008: Vault integration (assembly layer is the trust boundary)
- ADR-014: Secret material flow and capability injection (capabilities are
  orthogonal — both are set at registration by the assembly layer)
- OQ-15: Call protocol client and adapter contract (adapters produce scoped envs)
- OQ-17: Abort cascade (the call tree is the agency chain — `parent_request_id`
  traces principal → agent)
- OQ-19: Session-scoped registries (session operations are always `Internal`)
- [operation-registry.md](../crates/call/operation-registry.md)
- [call-protocol.md](../crates/call/call-protocol.md)
- TypeScript `@alkdev/operations` `buildEnv()` with `allowedNamespaces` — prior
  art for scoped composition env
- POC at `/workspace/toolEnv` — demonstrated the sandbox-to-registry bridge with
  the full-registry exposure gap