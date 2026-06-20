---
status: draft
last_updated: 2026-06-20
reviewed_documents:
  - overview.md
  - README.md
  - open-questions.md
  - crates/core/README.md
  - crates/core/core-types.md
  - crates/core/endpoint.md
  - crates/core/auth.md
  - crates/core/config.md
  - crates/call/README.md
  - crates/call/call-protocol.md
  - crates/call/operation-registry.md
  - crates/vault/README.md
  - crates/vault/mnemonic-derivation.md
  - crates/vault/encryption.md
  - crates/vault/service.md
  - crates/vault/protocol.md
  - decisions/001 through 021 (all 21 ADRs)
reviewer: architect
---

# Architecture Gap Review #001

## Purpose

This document captures gaps, ambiguities, and inconsistencies found in the
alknet architecture specifications (core, call, vault crates) before
decomposition into implementation tasks. Each finding is categorized by
severity and structured as **Problem** → **Suggestion** (or **Open Question**
where no solution is yet known).

This is a final sanity check before implementation. The vault crate carries
over source from the POC with documented drift (OsRng, `unwrap()`,
`CURRENT_KEY_VERSION=1`, `spawn` return bug) flagged for correction during
implementation sync; that drift is out of scope for this review. The focus is
cross-boundary ambiguities and inconsistencies that would cause divergent
implementations across crates.

## Severity Definitions

| Severity | Meaning |
|----------|---------|
| **Critical** | Must resolve before implementation — would cause divergent implementations or security issues |
| **Warning** | Should resolve — missing specifications that could cause confusion or rework |
| **Suggestion** | Consider — improvements for clarity or completeness |

---

## Critical Findings

### C1. `handler_identity` Has No Registration Path

**Documents**: ADR-015 (lines 79, 141, 251), operation-registry.md (lines 81,
104, 122, 126, 185), call-protocol.md (lines 265, 279, 288)

**Problem**: ADR-015 and operation-registry.md are emphatic and repeated that
`handler_identity` is "set at registration by the assembly layer":

> ACL check runs against the composing handler's `handler_identity` (**set at
> registration**) — operation-registry.md L81

> `handler_identity`: The identity of the handler processing this call. **Set
> at registration by the assembly layer**. — operation-registry.md L122

> **Handler identity is set at registration by the assembly layer.** — ADR-015
> Assumption 2

But the registration API shown in operation-registry.md has no place for it:

- `OperationSpec` (L33–41) has fields: `name`, `namespace`, `op_type`,
  `visibility`, `input_schema`, `output_schema`, `access_control`. No
  `handler_identity`.
- `OperationRegistry::register(spec, handler)` (L140) takes
  `(OperationSpec, Handler)`.
- `OperationRegistryBuilder::with(spec, handler)` (L148–153, L259–266) takes
  `(OperationSpec, Handler)`.

So the spec says handler_identity is "set at registration" but the registration
API doesn't accept it. Tracing how it would reach `OperationContext` at call
time reveals the gap: `build_root_context` (call-protocol.md L265–286) sets
`handler_identity: None` for wire-originated calls — correct for the root (ACL
runs against caller identity). `OperationEnv::invoke()` propagates
`parent.handler_identity.clone()` to children (L185). But the parent's
`handler_identity` was `None` (root), so every internal call gets
`handler_identity: None`. Under the shown API, ADR-015's "ACL runs against
`handler_identity` for internal calls" is checking against `None` — which is
the privilege-escalation gap ADR-015 was written to close.

The handler's identity must enter the system somewhere. The natural place is
registration: the assembly layer associates an `Identity` (scopes) with each
handler, and the dispatch path attaches it to the `OperationContext` for calls
dispatched to that handler (so it propagates via `parent.handler_identity`
when that handler composes children). The spec shows no such mechanism.

The TypeScript reference (`/workspace/@alkdev/operations/src/registry.ts`) has
the same shape problem at a smaller scale — `OperationContext.trusted`
(types.ts L27) is set by `buildEnv()` (env.ts L37), not by `register()`. The
TypeScript registry never carried a handler identity; it relied on `trusted`
being set by the env builder. The Rust rewrite replaces `trusted` with
`handler_identity` (ADR-015) but doesn't show the equivalent of `buildEnv`'s
wiring point.

**Suggestion**: Decide and document where `handler_identity` is supplied at
registration. Likely a third parameter to `register`/`with` or part of a
registration bundle (see C4). The dispatch path needs to look up the handler's
registered identity by operation name and attach it to the
`OperationContext` before invoking the handler, so that handler's children
inherit it via `parent.handler_identity`. This should be its own ADR
(ADR-022: Handler Registration and Dispatch Context) before any call-crate
implementation begins — see C4 for the synthesis.

---

### C2. Scoped Composition Env Has No Registration/Construction Path

**Documents**: ADR-015 (lines 150–169, 256–260), operation-registry.md (lines
126, 168, 174, 307), call-protocol.md (line 285)

**Problem**: Same shape as C1. ADR-015 and operation-registry.md say the
scoped composition env is "set at registration by the assembly layer":

> The `OperationEnv` given to a handler is scoped — it can only invoke a
> declared set of operations, **set at registration by the assembly layer**
> — operation-registry.md L307

> **Static scoping at registration**: the assembly layer declares which
> operations a handler may compose — ADR-015 L159

> The scoped env is **declared at registration** (static) — ADR-015
> Assumption 3

But:

- `register(spec, handler)` / `with(spec, handler)` take no scoped-env
  declaration. `OperationSpec` has no field for it.
- The only `OperationEnv` implementation shown is `LocalOperationEnv {
  registry: Arc<OperationRegistry> }` (operation-registry.md L168) — the
  full registry, not a scoped one.
- `LocalOperationEnv::invoke` (L174) does `self.registry.invoke(&name, ...)`
  against the unfiltered registry. There's no scoping layer shown.
- `build_root_context` sets `env: self.env.clone()` (call-protocol.md L285) —
  one env cloned to every root context. That's a single global env, not
  per-handler scoping.

For scoping to work, each handler must receive a different scoped env, but the
dispatch path shown gives every root context the same `self.env`. ADR-015
explicitly calls the scoped env one of the three independent controls that are
"all needed" (L177–198), and says without it "the handler can call anything in
the registry" — the parameterized-dispatch attack surface. The spec locks the
requirement but not the mechanism, and the mechanism is non-obvious (it
changes the registration API and the per-handler context construction).

The TypeScript reference (`/workspace/@alkdev/operations/src/env.ts`) implements
scoping via `buildEnv({ registry, context, allowedNamespaces })` — the env
builder takes an `allowedNamespaces` filter and returns a pre-filtered env
(L14–46). The Rust equivalent needs the same: a per-handler env built from a
per-handler scope declaration. The declaration point (registration) and the
construction point (dispatch) both need specifying.

**Suggestion**: Specify how a scoped env is declared at registration and how it
reaches the handler's `OperationContext.env`. Likely the same bundle that
resolves C1 — see C4.

---

### C3. `Capabilities` Lives in Two Places and the Two Models Aren't Reconciled

**Documents**: ADR-014 (lines 82–83, 114–124), operation-registry.md (lines 105,
253–256, 274–297), call-protocol.md (line 274)

**Problem**: ADR-014 and operation-registry.md show two different models for how
a handler gets outbound credentials:

- **Model A — construction-time capture in the handler closure.**
  operation-registry.md L253–256:
  `agent_chat_handler(Capabilities::new().with_api_key("google", google_api_key))`.
  The handler is constructed with capabilities baked in. ADR-014 L82:
  "injected at handler construction (the common case: a static decrypted API
  key held for the handler's lifetime)."

- **Model B — per-request on `OperationContext.capabilities`, propagated
  through composition.** operation-registry.md L105:
  `OperationContext { pub capabilities: Capabilities }`. L186:
  `capabilities: parent.capabilities.clone()` in `invoke()`. ADR-014 L83: "or
  scoped per-request for internal-only flows." call-protocol.md L274:
  `build_root_context(..., capabilities: Capabilities)` takes a capabilities
  parameter.

These two models don't connect. If the handler closure captured the
capabilities at construction (Model A), then `OperationContext.capabilities`
(Model B) is either redundant or must be populated from the closure — but the
closure isn't passed the context, the closure receives the context. There's a
circular dependency: the handler needs capabilities to do its work, the
capabilities are on the context the handler receives, but the context is built
by the CallAdapter which doesn't have the handler's construction-time
capabilities (the `CallAdapter` struct at call-protocol.md L35–38 has no
`capabilities` field).

`build_root_context`'s comment (call-protocol.md L274) says "the CallAdapter's
own capabilities (if any)" — hedged with "if any," suggesting even the spec
author wasn't sure. For composition, `invoke()` clones `parent.capabilities` —
so children inherit the parent's capabilities. But if the parent's
`context.capabilities` was empty (because the parent handler used its captured
capabilities, not the context's), children get empty capabilities too. An agent
handler composing `/fs/readFile` with an outbound LLM call would lose the LLM
API key at the composition boundary.

**Suggestion**: Pick one of:

- (a) Capabilities are only per-request on `OperationContext`, populated by the
  dispatch path from a per-handler capabilities table held by the
  registry/adapter (the construction-time "baking" just populates that table).
- (b) Capabilities are captured in the handler closure and
  `OperationContext.capabilities` is removed, with composition passing
  capabilities explicitly as an `invoke()` parameter.
- (c) Keep both but document the bridge (e.g., the handler closure captures an
  `Arc<Capabilities>` and the registry also holds a clone that the CallAdapter
  uses to populate `build_root_context`).

Option (a) is closest to the shape that resolves C1 and C2 cleanly — see C4.

---

### C4. C1–C3 Are One Tangle: Registration Bundle Is Missing

**Documents**: operation-registry.md (lines 130–153, 168–194), call-protocol.md
(lines 265–288)

**Problem**: C1, C2, C3 are the same underlying gap: the registration/dispatch
API surface in operation-registry.md doesn't carry the three things ADR-014
and ADR-015 say are "set at registration" — handler identity, scoped env, and
capabilities. All three are load-bearing for the security model, all three are
locked in ADRs, and none are reflected in the `register(spec, handler)`
signature or the `OperationSpec` struct.

The three controls in ADR-015 (visibility, handler identity, scoped env) plus
the capability injection in ADR-014 all enter the system at the same boundary:
the assembly layer hands the registry a `(spec, handler)` pair plus the
handler's runtime context material. The natural shape is a registration bundle
that carries all three:

```rust
// Illustrative — the exact shape is the decision to make.
pub struct HandlerRegistration {
    pub spec: OperationSpec,
    pub handler: Handler,
    pub identity: Identity,               // C1: handler identity for ACL
    pub scoped_env: ScopedOperationEnv,  // C2: reachable operations
    pub capabilities: Capabilities,      // C3: outbound credentials
}
```

And the dispatch path (`build_root_context` + `OperationEnv::invoke`) reads
from this bundle: the CallAdapter looks up the handler's registration by
operation name, attaches `registration.identity` to
`OperationContext.handler_identity` (so children inherit it), uses
`registration.scoped_env` as `OperationContext.env` (so the handler can only
reach declared ops), and populates `OperationContext.capabilities` from
`registration.capabilities` (so composition propagates them via
`parent.capabilities.clone()`).

This resolves all three cleanly and matches the prior art: the TypeScript
`buildEnv({ registry, context, allowedNamespaces })` already bundles the
scoping decision with the env construction; the Rust version generalizes this
to a per-handler registration rather than a per-call build.

**Suggestion**: Write ADR-022 (Handler Registration and Dispatch Context) that:

1. Defines the registration bundle shape.
2. Updates `OperationRegistry::register` / `OperationRegistryBuilder::with`
   signatures.
3. Shows how `build_root_context` and `OperationEnv::invoke()` read from the
   bundle.
4. Reconciles Model A and Model B for capabilities (recommend option (a): the
   bundle carries capabilities, the closure captures nothing, the dispatch
   path populates `OperationContext.capabilities` from the bundle).
5. Updates operation-registry.md and call-protocol.md to match.

This is the single highest-value piece of work before implementation. It
closes the gap between ADR-014/015's locked security model and the API surface
in one place, before any implementer has to guess.

---

### C5. Operation Registry Has No Error Schema Concept

**Documents**: operation-registry.md (lines 33–41, OperationSpec), call-protocol.md
(lines 127–143, call.error payload), ADR-017 (lines 113–124, from_call mirrors
remote spec)

**Problem**: The `OperationSpec` has `input_schema` and `output_schema` but no
`error_schemas`. The `call.error` payload (call-protocol.md L128–134) carries a
`code` (string enum: `NOT_FOUND`, `FORBIDDEN`, `INVALID_INPUT`, `INTERNAL`,
`TIMEOUT`) and a `message`, but there's no way for an operation to declare the
specific errors it can return beyond these five infrastructure codes.

This matters for two reasons:

1. **OpenAPI/HTTP adapter fidelity.** OpenAPI specs naturally include error
   information (response status codes with schemas). The `from_openapi` adapter
   (ADR-017 L113–124) imports operations and mirrors "the remote operation's
   name, namespace, type, schemas, and access control" — but with no error
   schema field, error responses from the OpenAPI source are dropped on
   import. `to_openapi` has nowhere to project error information to. The same
   gap applies to `from_mcp`/`to_mcp`. An OpenAPI operation that returns
   `404: { schema: NotFoundError }` and `422: { schema: ValidationError }`
   cannot be faithfully represented in alknet's `OperationSpec`.

2. **Client ergonomics and type safety.** A client calling `/fs/readFile` can't
   know from the schema that it might return `FILE_NOT_FOUND` vs
   `PERMISSION_DENIED` vs `INVALID_PATH`. The five infrastructure codes cover
   protocol-level failures; they don't cover operation-level domain errors. A
   caller has to parse `message` strings — the exact anti-pattern that typed RPC
   is meant to avoid.

The TypeScript reference (`/workspace/@alkdev/operations/src/types.ts` L38–47,
L94, L112) defines `ErrorDefinitionSchema` and an optional
`errorSchemas?: ErrorDefinition[]` on `OperationSpec`. Each error definition
has `code`, `description`, `schema`, and optional `httpStatus`. The `mapError()`
function (`error.ts` L25–51) matches thrown errors against the declared error
schemas by code prefix. This is a proven pattern; the Rust implementation
should carry it forward. The translator agent likely omitted it because it's
`Optional` in the TS schema (so dropping it doesn't break the happy path) and
because error schemas are semantically different from input/output schemas
(an operation returns one output but could return any of several errors).
That's a reasonable judgment call for a first translation pass, but it leaves a
real gap for adapters and clients.

Without error schemas, `from_openapi` and `to_openapi` are lossy on the error
axis — an OpenAPI operation's error contract is silently dropped. This
undermines the "discoverable, type-safe RPC" goal in operation-registry.md
L18–24. It's also a cross-boundary issue: the call protocol, the operation
registry, and the adapters all need to agree on the error representation, and
right now only the call protocol has one (the five infrastructure codes).

**Suggestion**: Add an optional `error_schemas: Vec<ErrorDefinition>` (or
similar) to `OperationSpec`, where `ErrorDefinition` carries a `code`, optional
`http_status` (for adapter projection), and a JSON Schema for the error detail
payload. Extend the `call.error` payload to optionally carry an error detail
matching the declared schema. Document that the five infrastructure codes
(`NOT_FOUND` etc.) are protocol-level and distinct from operation-level error
codes. This should be its own ADR (ADR-023: Operation Error Schemas) — the
error representation is a one-way door (once the wire format carries error
codes, changing them breaks clients), and it must be decided before
implementing `from_openapi`/`to_openapi`, otherwise the adapter contract in
ADR-017 can't be faithfully implemented.

---

## Warning Findings

### W1. ADR-020 Contains a Stale Example Path That Contradicts ADR-021

**Documents**: ADR-020 (line 70)

**Problem**: ADR-020 L70 says "different derivation paths
(`m/74'/2'/0'/0'` for v1, `m/74'/2'/1'/0'` for a future v2)". This puts the
version index at the third level (`2'/1'/0'`). ADR-021 (which resolves the
actual rotation scheme) and all the vault specs put the version index at the
last level: v2 = `m/74'/2'/0'/0'`, v3 = `m/74'/2'/0'/1'`
(mnemonic-derivation.md L226, encryption.md L160–164, ADR-021 L68–73).
ADR-020's example is a pre-ADR-021 leftover and directly contradicts the locked
decision. An implementer reading ADR-020 in isolation would derive at the wrong
path.

**Suggestion**: Update ADR-020 L70's example to `m/74'/2'/0'/0'` for v2,
`m/74'/2'/0'/1'` for v3, or remove the illustrative example and point to
ADR-021 for the scheme.

---

### W2. `EncryptionError::KeyVersionMismatch` Is Stale After OQ-22 Resolved

**Documents**: encryption.md (lines 197, 205–210)

**Problem**: `EncryptionError::KeyVersionMismatch { expected, actual }` is
defined with comment "reserved for future rotation (OQ-22)". encryption.md
L205 says it's "defined but unused in v2." But OQ-22 is now resolved by
ADR-021, and ADR-021's rotation mechanism (version-indexed paths + `decrypt`
derives at the version's path + `rotate` re-encrypts) never emits
`KeyVersionMismatch` — there's no version-mismatch to detect, because
`decrypt` always derives the key the blob says to use. So after ADR-021,
`KeyVersionMismatch` isn't "reserved for future rotation" (rotation is
implemented, without it) — it's effectively dead. The "reserved for future
(OQ-22)" comment is stale and could mislead an implementer into thinking they
need to wire it up for rotation.

**Suggestion**: Update the encryption.md comment to state that ADR-021
implements rotation without this variant, and clarify whether it's retained
for a different future use or should be removed.

---

### W3. `from_mcp` Is Missing from Adapter Lists in operation-registry.md and ADR-014

**Documents**: operation-registry.md (lines 59, 301), ADR-014 (line 105)

**Problem**: ADR-017 L160–165 lists four import adapters: `FromOpenAPI`,
`FromMCP`, `FromCall`, `FromJsonSchema`. But:

- operation-registry.md L59 says "`from_openapi` and `from_jsonschema`
  adapters register operations as `Internal` by default" — omits `from_mcp`
  and `from_call`.
- operation-registry.md L301 says "The `from_openapi`, `from_jsonschema`, and
  `from_call` adapter patterns" — omits `from_mcp`.
- ADR-014 L105 says "The `from_openapi` and `from_jsonschema` adapter patterns"
  — omits `from_mcp` and `from_call`.

`from_mcp` is a real import adapter (in ADR-017's trait list and ADR-013's
adapter contract). Its absence from the prose in operation-registry.md/ADR-014
is likely an oversight, but it could make an implementer wonder whether
`from_mcp` is real or whether it follows the Internal-by-default rule.

**Suggestion**: Standardize on listing all four import adapters (or say "all
import adapters" / "all `OperationAdapter` implementations") when stating the
Internal-by-default rule, so the rule clearly applies to `from_mcp` too.

---

### W4. `derive_encryption_key_for_version` (ADR-021) Not in service.md Method List

**Documents**: service.md (lines 119–127, 161–179), ADR-021 (lines 104–122)

**Problem**: ADR-021 L104–122 introduces `derive_encryption_key_for_version(version)`
and shows `decrypt` using it. But service.md's "Derive Methods" section (L119–127)
only shows `derive_encryption_key(path)`, and its `encrypt`/`decrypt` (L161, L172)
reference the version-indexed path scheme in prose but don't show the
version-aware method. An implementer whose primary reference is service.md
wouldn't see `derive_encryption_key_for_version` and might implement `decrypt`
as "always derive at `PATHS::ENCRYPTION`" (the pre-ADR-021 behavior, which
ADR-021 explicitly calls out as a drift bug to fix). The ADR is authoritative,
but the spec doc is what the implementer reads first.

**Suggestion**: Add `derive_encryption_key_for_version` to service.md's method
list and show `decrypt` calling it, mirroring ADR-021. Clarify whether
`derive_encryption_key(path)` (the path-based method) is still public API or
subsumed by the version-based one.

---

## Suggestions

### S1. Abort Policy Field Absent from `call.requested` Payload

**Documents**: call-protocol.md (lines 112–118, call.requested payload),
ADR-016 (lines 86–89, 177–181)

**Suggestion**: ADR-016 says the `abort-dependents` vs `continue-running` policy
is per-call and "the specific mechanism (a field in the `call.requested`
payload, a field on `OperationContext`, or a per-operation declaration) is a
two-way door for implementation." call-protocol.md's `call.requested` payload
(L112–118) shows `operationId`, `input`, `auth_token` — no policy field. Since
ADR-016 explicitly leaves the mechanism open, this isn't an inconsistency, but
a one-line note in call-protocol.md ("the abort policy is conveyed via [TBD —
two-way door, ADR-016]") would prevent an implementer from assuming the payload
is final and forgetting the policy exists.

---

### S2. `build_root_context` "CallAdapter's Own Capabilities (If Any)" Hedge

**Documents**: call-protocol.md (line 274)

**Suggestion**: The "(if any)" hedge reflects the C3 ambiguity. Once C3/C4 is
resolved, this comment should be made definitive — either "the handler's
capabilities, looked up from the registry by operation name" (if option (a))
or removed (if capabilities are closure-captured only). Leaving it hedged
propagates the ambiguity into the one place an implementer looks for "how does
the root context get built."

---

### S3. Example Style: `Arc::new(handler)` Inline vs. Pre-wrapped

**Documents**: operation-registry.md (lines 149, 264)

**Suggestion**: L149 uses `.with(spec, Arc::new(services_list_handler))` while
L264 uses `.with(spec, agent_handler)` where `agent_handler` was already
`Arc::new`'d at L253. Both are valid; pick one style in the examples to avoid a
moment of "wait, does `with` take an `Arc` or a `Handler`?" confusion. Trivial.

---

### S4. `last_updated` Dates Scattered; README Index Older Than Sub-docs

**Documents**: frontmatter across all docs

**Suggestion**: The README index date (2026-06-19) is older than several
sub-documents (core/README 06-21, call/operation-registry 06-22, etc.). If
frontmatter dates are meant to mean something, bump README/overview when
sub-docs change. Not worth a cycle on its own.

---

## Summary Statistics

| Severity | Count | Status |
|----------|-------|--------|
| Critical | 5 | Needs resolution before implementation |
| Warning | 4 | Should resolve — mostly quick spec clarifications |
| Suggestion | 4 | Consider for clarity |

## Recommended Resolution Order

1. **Critical findings C1–C4** — These are one tangle (the registration
   bundle). Resolve together via ADR-022 (Handler Registration and Dispatch
   Context) before any call-crate implementation begins. C1, C2, C3 are kept as
   separate findings for traceability (each cites specific lines) but C4
   synthesizes them: the registration bundle carries handler identity, scoped
   env, and capabilities, and the dispatch path reads from it. This is the
   highest-value work before implementation.
2. **Critical finding C5** — Error schemas. Resolve via ADR-023 (Operation
   Error Schemas) before implementing `from_openapi`/`to_openapi`, otherwise
   the adapter contract in ADR-017 can't be faithfully implemented.
3. **Warning findings W1–W4** — Small doc edits. W1 (stale ADR-020 path) and W4
   (missing service.md method) should go in the same pass as the ADR-022 spec
   updates since they touch the same files. W2 and W3 are one-line fixes.
4. **Suggestions S1–S4** — Address opportunistically during implementation.