---
status: draft
last_updated: 2026-06-22
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
  - decisions/001 through 023 (all 23 ADRs)
reviewer: architect
---

# Architecture Gap Review #002

## Purpose

This is the second pre-implementation sanity check. Review #001 caught the
registration-bundle tangle (C1–C4) and the missing error schemas (C5), both
resolved via ADR-022 and ADR-023. This review goes deeper on two axes the
first review under-covered:

1. **Cross-document consistency**: places where an Accepted ADR contradicts a
   Proposed ADR, where a spec doc adopted a Proposed ADR's types without the
   ADR being advanced, and where a later ADR narrowed a "two-way door"
   deferral without updating the OQ classification.
2. **Two-way-door deferral audit**: the user's concern that the one-way/two-way
   door framing from ADR-009 has been applied too loosely. Some deferrals are
   legitimate, but several are questionable or misleading — either because a
   later ADR quietly narrowed the door, because the "path of least resistance"
   implementation forecloses a better option, or because multiple
   individually-reversible deferrals compound to foreclose a path that no
   single deferral would block on its own.

Review #001 resolved 5 critical, 4 warning, and 4 suggestion findings. This
review finds a different set: the registration-bundle work surfaced new
inconsistencies between ADR-015 (Accepted) and ADR-022 (Proposed), the abort
policy (ADR-016) was never reflected in the OperationContext struct, the
OperationEnv trait was never defined, and several vault deferrals have
security dimensions that the two-way-door framing doesn't surface.

## Severity Definitions

| Severity | Meaning |
|----------|---------|
| **Critical** | Must resolve before implementation — would cause divergent implementations or security issues |
| **Warning** | Should resolve — missing specifications that could cause confusion or rework |
| **Suggestion** | Consider — improvements for clarity or completeness |

---

## Critical Findings

### C1. ADR-015 Is Not Reconciled With ADR-022 — Direct Contradiction in the Document Set

**Documents**: ADR-015 (Status: Accepted, Decision 3 L114–136, Assumption 6
L275–280), ADR-022 (Status: Proposed, Decision 2 L145–186), operation-registry.md
(L115, L133, L172), call-protocol.md (L296)

**Problem**: ADR-015 is marked **Accepted** and defines
`handler_identity: Option<Identity>` in its Decision 3 (L124) — a peer `Identity`
type resolved via `IdentityProvider`. Assumption 6 (L275–280) explicitly states
"the handler identity is a full `Identity` (with scopes), not a special
principal type… This reuses the existing `Identity` type and `IdentityProvider`
infrastructure (ADR-004)."

ADR-022 is marked **Proposed** and explicitly supersedes ADR-015's Assumption 6
(L147–149: "This ADR refines that: composition authority is a declared authority
bundle, not a peer `Identity`"; L183–185: "This supersedes ADR-015's Assumption
6"). The spec docs now use `handler_identity: Option<CompositionAuthority>` —
a different, non-peer type.

This is a **direct contradiction**. Per the ADR lifecycle, a superseded
decision should be marked `Superseded` or carry an inline note pointing to the
superseding ADR. ADR-015 has neither. Worse, ADR-015 has **higher status**
(Accepted) than the ADR that supersedes it (Proposed). An implementer reading
ADR-015 in isolation would build `Identity`-based handler identity; an
implementer reading the specs would build `CompositionAuthority`-based. The
Accepted-overrides-Proposed governance rule means the old `Identity` definition
is, by the document set's own lifecycle, currently authoritative — yet the
specs have already adopted the proposed change.

This is the same shape of bug as review #001's W1 (ADR-020's stale example
path contradicting the locked ADR-021 scheme), but more severe: it's a
security-model type, not a path string.

**Suggestion**: Two things must happen before implementation:

1. Advance ADR-022 and ADR-023 to **Accepted**. The specs already depend on
   them as the source of truth for types that appear throughout the code.
   Keeping them Proposed while the specs adopt their types is incoherent
   governance. (See C2.)
2. Reconcile ADR-015: either update its Decision 3 and Assumption 6 to point
   to ADR-022 (inline amendment note + change `Identity` to
   `CompositionAuthority` in the struct), or mark the relevant sections
   `Superseded by ADR-022 (Decision 2)`.

The cleanest path is to do both in one pass: advance ADR-022/023 to Accepted,
amend ADR-015 to reference the supersession. This resolves C1, C2, and C6 in
one move.

---

### C2. ADR-022 and ADR-023 Are "Proposed" but the Specs Treat Them as Authoritative

**Documents**: ADR-022 (Status: Proposed), ADR-023 (Status: Proposed),
operation-registry.md (L40, L115, L133, L149, L158, L160–168, L286, L372–379,
L404–406), call-protocol.md (L296, L305, L369–373), README.md (ADR table
L36–60)

**Problem**: Both ADR-022 and ADR-023 are marked **Proposed**. Yet the spec
docs depend on them pervasively as the source of truth:

- `OperationSpec.error_schemas` (ADR-023) — operation-registry.md L40, L286.
- `ErrorDefinition` (ADR-023) — operation-registry.md L56–61.
- `HandlerRegistration` with `provenance`, `composition_authority`,
  `scoped_env`, `capabilities` (ADR-022) — operation-registry.md L160–168,
  the central registration type.
- `OperationContext.handler_identity: Option<CompositionAuthority>` (ADR-022
  Decision 2) — operation-registry.md L115, L133; call-protocol.md L296.
- `CompositionAuthority`, `ScopedOperationEnv`, `OperationProvenance`
  (ADR-022) — used throughout.

The README's ADR table (L36–60) shows ADR-022 and ADR-023 as "Proposed," but
the call-crate README's ADR table has **no Status column** at all — a reader
can't tell which ADRs are binding. Per the ADR lifecycle, "Proposed" means
under review, not yet a commitment.

Combined with C1 (ADR-015 Accepted still contradicts ADR-022 Proposed), the
governance state is incoherent: the Accepted ADR says `Identity`, the Proposed
ADR says `CompositionAuthority`, and the specs have picked the Proposed one.

**Suggestion**: Advance ADR-022 and ADR-023 to **Accepted** before
implementation. They are clearly intended to be binding — the specs already
implement them. Add a Status column to the crate README ADR tables so the
binding/non-binding distinction is visible. This is the governance fix that
unblocks C1.

---

### C3. The Abort Policy (ADR-016 Decision 6) Is Missing from `OperationContext` and `OperationEnv::invoke()`

**Documents**: ADR-016 (Decision 6 L120–164, Accepted), operation-registry.md
(`OperationContext` struct L111–128, `LocalOperationEnv::invoke()` L211–249),
call-protocol.md (L124, L345)

**Problem**: ADR-016 Decision 6 states unambiguously (L122–125): "The abort
policy (`abort-dependents` vs `continue-running`) is set on `OperationContext`
and propagated to children via `OperationEnv::invoke()`." It shows
`OperationContext` carrying the policy (L154) and an opt-in
`invoke_with_policy(...)` method (L149–151).

The `OperationContext` struct in operation-registry.md (L111–128) has **no
`abort_policy` field**. The fields shown are: `request_id`,
`parent_request_id`, `identity`, `handler_identity`, `capabilities`,
`metadata`, `env`, `internal`. No policy. The `LocalOperationEnv::invoke()`
implementation (L211–249) likewise constructs a child `OperationContext` with
no policy field and takes no policy parameter.

call-protocol.md L124 and L345 correctly *state* that the policy is on
`OperationContext` and propagated via `OperationEnv::invoke()`, but the actual
struct definition in operation-registry.md — the one an implementer would copy
— omits it. This is a gap between ADR-016 (Accepted) and the spec. An
implementer copying the spec would simply omit the abort policy, and the
`continue-running` opt-in would have no wiring point.

**Suggestion**: Add `pub abort_policy: AbortPolicy` (with a default of
`AbortPolicy::AbortDependents`) to the `OperationContext` struct in
operation-registry.md, and define the `AbortPolicy` enum
(`AbortDependents | ContinueRunning`). The `build_root_context` in
call-protocol.md should set `abort_policy: AbortPolicy::AbortDependents` (the
default per ADR-016 L140). See C4 for the `invoke()` side.

---

### C4. `OperationEnv` Trait Is Never Defined; `invoke_with_policy()` Absent from Spec

**Documents**: operation-registry.md (L196–259), ADR-016 (Decision 6 L144–164)

**Problem**: Two related gaps:

1. **The `OperationEnv` trait is never defined.** operation-registry.md
   repeatedly references `OperationEnv` as a **trait** that "must remain a
   trait" (L259, also OQ-19), and shows
   `#[async_trait] impl OperationEnv for LocalOperationEnv` (L211–212), but
   **the trait definition itself is never shown**. Only the `LocalOperationEnv`
   struct (L207–209) and its impl block (L211–249) appear. An implementer
   cannot know the trait's method signatures, return types, or whether
   `invoke()` is the sole method.

2. **The `invoke_with_policy()` opt-in is absent.** ADR-016 Decision 6
   (L144–152) shows the policy entering through `OperationEnv::invoke()` via
   `invoke_with_policy("fs", "readFile", input, &context,
   AbortPolicy::ContinueRunning)`. The spec's `OperationEnv::invoke()`
   signature (L213) takes no policy parameter, and there is no
   `invoke_with_policy` method, no `InvokeOptions` struct, no builder
   pattern. The spec never defines the trait surface, so the policy opt-in
   mechanism ADR-016 mandates is unspecified.

This is load-bearing because OQ-19 makes the trait-ness of `OperationEnv` a
**constraint**: session-scoped registries depend on wrapping the global env
via trait layering. If the trait surface is unspecified, two implementers
could define it differently, breaking the wrapping pattern.

**Suggestion**: Define the `OperationEnv` trait explicitly in
operation-registry.md, showing at minimum:

```rust
#[async_trait]
pub trait OperationEnv: Send + Sync {
    async fn invoke(
        &self,
        namespace: &str,
        operation: &str,
        input: Value,
        parent: &OperationContext,
    ) -> ResponseEnvelope;

    /// Opt-in: compose a child with a non-default abort policy.
    async fn invoke_with_policy(
        &self,
        namespace: &str,
        operation: &str,
        input: Value,
        parent: &OperationContext,
        policy: AbortPolicy,
    ) -> ResponseEnvelope {
        // default implementation: set policy on child context, delegate to invoke
    }
}
```

This satisfies both the "must remain a trait" constraint and ADR-016's
policy parameter. See C7 for a related type ambiguity on the same struct.

---

### C5. ADR-017's `from_call` Mirror List Does Not Include `error_schemas` — Inconsistency with ADR-023

**Documents**: ADR-017 (Decision 3 L113–122, Assumption 4 L274–279, Accepted),
ADR-023 (Decision 1 L112–147, L42–47, L257, Proposed)

**Problem**: ADR-017 Decision 3, step 3 (L118–119) says `from_call` constructs
a spec that "mirrors the remote operation's name, namespace, type, schemas,
and access control." Assumption 4 (L274–275) repeats: "the imported
`OperationSpec` has the same name, namespace, type, schemas, and access
control as the remote operation." The list is: name, namespace, type,
schemas, access control. There is **no `error_schemas`** in either list.

ADR-023 adds `error_schemas: Vec<ErrorDefinition>` to `OperationSpec` (L123)
and Decision 5 (L236–261) makes adapter fidelity explicit for
`from_openapi`/`to_openapi`/`from_mcp`/`to_mcp`. But ADR-023 does **not**
mention `from_call` in Decision 5 — it covers OpenAPI and MCP adapters only.
Meanwhile ADR-017's `from_call` mirror list predates `error_schemas` existing
as a field.

The result: it's ambiguous whether `from_call` should mirror `error_schemas`.
ADR-017's step 2 (L116–117) says `from_call` calls `services/schema` "for each
→ gets the input/output JSON Schemas" — again, no mention of error schemas.
ADR-023 Decision 6 (L264–286) confirms `services/schema` *returns*
`error_schemas`, so the data is available, but ADR-017 doesn't say to consume
it.

An implementer following ADR-017 literally would import operations with empty
`error_schemas`, silently dropping the remote error contract — the exact
lossy-import problem ADR-023 was written to solve for OpenAPI/MCP, now
recurring for `from_call`.

**Suggestion**: Update ADR-017 Decision 3 step 3 and Assumption 4 to include
`error_schemas` in the mirror list (name, namespace, type, schemas,
**error_schemas**, access control), and update step 2 to note that
`services/schema` now also returns error schemas. Add a cross-reference to
ADR-023. This is an inconsistency between an Accepted ADR (017) and a later
ADR (023) that must be reconciled — same shape as review #001's W3
(`from_mcp` missing from adapter lists), but for a new field.

---

### C6. `OperationContext.env` Has a Type Identity Crisis — Reachability Set vs Dispatch Trait

**Documents**: ADR-015 (L127: `pub env: OperationEnv`), ADR-022 (L208–215
`ScopedOperationEnv`, L286–287 `env: registration.scoped_env…`),
operation-registry.md (L118: `pub env: OperationEnv`; L220:
`parent.env.allows(&name)`; L243–244: `env: registration.scoped_env.clone()…`;
L198: `context.env.invoke(...)`), call-protocol.md (L298–299)

**Problem**: There is a type identity problem for the `env` field on
`OperationContext`:

- ADR-015 L127 and operation-registry.md L118 declare `pub env: OperationEnv`
  (a trait object / the trait type).
- ADR-022 and operation-registry.md L243–244 and call-protocol.md L298–299
  *populate* `env` with a `ScopedOperationEnv` (a concrete struct:
  `allowed_operations: HashSet<String>`, ADR-022 L208–214).

`ScopedOperationEnv` (a reachability set — ADR-022's reachability control)
and `OperationEnv` (the composition dispatch trait — the thing with
`invoke()`) are **different concepts**. ADR-015 conflated them by putting
`OperationEnv` on the context; ADR-022 introduced `ScopedOperationEnv` as the
reachability *input* but the dispatch path assigns the scoped set directly to
`env:`.

operation-registry.md L220 calls `parent.env.allows(&name)` — implying `env`
is the `ScopedOperationEnv` (has `.allows()`). But the same field is named
`OperationEnv` in the struct and described (L136) as "the operation environment
for composing calls… `OperationEnv`" with `invoke()`. A handler calls
`context.env.invoke(...)` (L198, L200) — implying `env` is the dispatch
trait. One field cannot be both a `HashSet`-based reachability gate and a
trait with an async `invoke()` method, unless they are the *same type* (they
are not — ADR-022 treats `ScopedOperationEnv` as data and the dispatch
`OperationEnv`/`LocalOperationEnv` as the trait impl that *consults* the
scoped env).

This is a genuine type-system ambiguity that would force an implementer to
guess: is `context.env` the dispatch trait (then where does the scoped
reachability set live?), or is it the reachability set (then how does the
handler call `invoke()` on it?), or is it a composite type that holds both?

This interacts with OQ-19's constraint that `OperationEnv` must remain a trait:
if `env` is `ScopedOperationEnv` (a concrete struct), the session-overlay
pattern is closed because the field can't hold a trait object.

**Suggestion**: Separate the two concepts explicitly. Likely:
`OperationContext` carries `scoped_env: ScopedOperationEnv` (reachability
data, used for the `allows()` check) and a separate `env: Box<dyn
OperationEnv>` (or `Arc<dyn OperationEnv>`) that is the dispatch trait (used
for `invoke()`). The reachability check in `invoke()` consults
`parent.scoped_env.allows(&name)`, and the actual dispatch goes through the
`OperationEnv` trait impl. The current field naming (`env` for both) is the
root of the confusion and should be disambiguated. This may warrant an ADR-022
clarification, since it changes the struct shape that ADR-022 Decision 5 shows.

---

### C7. OQ-21's "Non-Breaking" Deferral Conflicts With ADR-019 and Hides a Crate-Decomposition Decision

**Documents**: open-questions.md (OQ-21 L252–285), ADR-019 (L133–136),
crates/vault/protocol.md (L187–216, L278–291), crates/vault/service.md
(L300–322)

**Problem**: Three interlocking problems:

1. **ADR-019 directly contradicts OQ-21's deferral framing.** ADR-019 L133–136
   states: *"Remote vault administration (unlock a running node's vault over
   the network) is not supported. If needed in the future, it requires a
   separate, heavily restricted mechanism (admin scope, mTLS-only, never
   expose the mnemonic over an unauthenticated channel) and **its own ADR**."*
   OQ-21 defers remote vault *access* (derive/encrypt/decrypt) as
   "non-breaking" with no ADR required. The boundary between ADR-019's
   "remote vault administration" (needs an ADR) and OQ-21's "remote vault
   access" (deferred, no ADR) is undefined. An implementer reading ADR-019
   could reasonably conclude that *any* remote vault exposure requires an
   ADR; an implementer reading OQ-21 could conclude none is needed.

2. **The "non-breaking" claim is only true for the wire format, not for
   secure enablement.** OQ-21 and protocol.md L278–286 frame enabling remote
   access as a "server-setup change." But the auth-wrapping handler that makes
   remote access *safe* cannot live in the vault crate (ADR-018) and requires
   either (a) assembly-layer integration or (b) **a new dedicated vault-server
   crate depending on both alknet-core and alknet-vault** (protocol.md
   L201–204, open-questions.md L269). Option (b) is a crate-decomposition
   change (ADR-003 territory), not "server setup." OQ-21's "two-way (enabling
   is non-breaking)" door-type classification (L256) is misleading: once
   workers build dependencies on the remote vault API, disabling it breaks
   them — the door is operationally one-way even if the wire format is
   additive.

3. **Nested, undecided deferral.** OQ-21 L269 says the auth handler "lives in
   the assembly layer **or** a dedicated vault-server crate." This "or" is
   itself an undecided architectural choice (affects the dependency graph and
   ADR-003's crate decomposition) deferred *inside* the OQ-21 deferral. It is
   not tracked as a separate question.

4. **The RemoteService path is unvalidated.** The deferral means
   `VaultProtocol`'s `RemoteService` trait impl, `DerivedKey`'s dual
   serialization (JSON redacts, postcard preserves), and
   `VaultServiceActor`'s transport-agnosticism have never been exercised over
   a real network. The spec *asserts* these work ("by construction"), but
   there are concrete failure modes that only surface at enablement —
   e.g., `DerivedKey`'s custom `Serialize` redacts `private_key` for
   "human-readable formats"; whether postcard correctly bypasses the
   redaction is a known source of subtle bugs. If it redacts under postcard
   too, remote dispatch silently corrupts private keys. ADR-009:33 warns
   "if the right choice is unclear, validate with a POC." The deferral skips
   the POC and asserts the door is open by construction.

5. **Safety footgun.** The default `IrohProtocol` handler forwards all
   message types without auth (protocol.md L189–191). If someone naively
   registers the ALPN without the wrapping handler, `Unlock` (mnemonic over
   the wire) and `Lock` (DoS) become remotely callable. The operation access
   policy is documented in prose only; nothing in the vault crate prevents
   the insecure configuration.

**Suggestion**: Promote OQ-21 from "deferred" to "open." Reconcile with
ADR-019 by explicitly defining the administration-vs-access boundary. Decide
now whether a vault-server crate is part of the architecture (this is a
crate-decomposition one-way door per ADR-003/ADR-018 and should be decided,
not deferred). If remote access is genuinely deferred, require that the
*enablement* (not just the protocol) be covered by a future ADR — matching
ADR-019's "requires its own ADR" language. Correct the door-type
classification: enabling remote access is a one-way door operationally (workers
depend on it), even if the wire format is additive. At minimum, add a guard
clause: "The RemoteService path is unvalidated. The `DerivedKey` postcard
serialization and the actor's transport-agnosticism must be verified by a
POC before claiming the door is open."

---

### C8. The Operation Access Policy Table Is Incomplete — Security-Sensitive Methods Are Unclassified

**Documents**: crates/vault/protocol.md (L218–236), crates/vault/service.md
(L104–212)

**Problem**: The "Operation access policy" table (protocol.md L223–232)
covers exactly the 8 `VaultProtocol` enum variants. But `VaultServiceHandle`
exposes additional methods not in the protocol enum: `rotate`, `unlock_new`,
`derive_encryption_key_for_version`, `derive_password_string`, `is_unlocked`.
These methods have no access classification — an implementer cannot determine
from the spec whether they are local-only, remote-capable, or simply not
protocol-exposed. This is security-relevant:

- **`rotate`** re-encrypts blobs (security-sensitive operational action;
  ADR-021 L168–170 says "rotation is an operational action, not a runtime
  behavior"). If it were ever exposed remotely, a compromised worker could
  trigger mass re-encryption. It is not in the protocol enum and not in the
  access policy table — its status is undefined.
- **`unlock_new`** generates and returns the mnemonic (root of trust) as a
  plaintext `String`. Not in the protocol enum; not in the policy table. Its
  local-only status is implied but not stated.
- **`derive_encryption_key_for_version`** returns an `EncryptionKey` directly
  (raw key material), unlike `Encrypt`/`Decrypt` which use the key internally.
  If exposed remotely, a worker could extract raw encryption keys. Not
  classified.

The spec never states the rule that "service methods not in `VaultProtocol`
are local-only by construction." An implementer could add protocol variants
for these methods without recognizing the security implication, since the
policy table only constrains existing variants.

**Suggestion**: Add an explicit rule: "Service methods absent from
`VaultProtocol` are local-only by default; adding a protocol variant requires
updating this policy table and an ADR if the method is security-sensitive."
Add `rotate`, `unlock_new`, and `derive_encryption_key_for_version` to the
policy table (or an explicit "local-only" list) with rationale. Document the
mapping between service methods and protocol variants.

---

### C9. `site_password_path(site_hash: &str)` Is Underspecified — String-to-Index Mapping Breaks Determinism

**Documents**: crates/vault/mnemonic-derivation.md (L202–204, L213, L219–222)

**Problem**: The helper `site_password_path(site_hash: &str) -> String`
produces path `m/74'/1'/0'/{site_hash}'`, but SLIP-0010 hardened derivation
indices are `u32` (0 to 2³¹−1). A `&str` "site hash" cannot be a derivation
index directly — it must be converted to a `u32`. The conversion (hash
algorithm, truncation, endianness, collision handling) is completely
unspecified. Two implementers could produce different, incompatible mappings:
SHA-256(site) → first 4 bytes → u32; or last 4 bytes; or BLAKE3; or a string
hash. Since the path semantics table (L213) says these are "Ed25519 bytes"
used for "Per-site passwords (not cached)," and determinism is a stated design
principle (L234–237: "the same mnemonic + passphrase + path always produces
the same key"), the string→index mapping must be deterministic and specified.
An underspecified mapping means the same site + mnemonic produces different
passwords across implementations — breaking the reproducibility guarantee.

Additionally, "Ed25519 bytes" (L213) as a password source is an undefined
concept. The path produces a 32-byte Ed25519 private key; using those raw
bytes as a password is a non-standard construction with no ADR explaining why
Ed25519-derived material is suitable as password entropy (vs HKDF or a
dedicated KDF).

**Suggestion**: Specify the exact string→u32 mapping (e.g.,
`u32::from_be_bytes(SHA-256(site_hash)[0..4]) & 0x7FFFFFFF` for hardened
index). Add a design note or ADR explaining why Ed25519-derived bytes are
acceptable as password material. Specify the password encoding (raw bytes vs
base64url — service.md L166 says base64url for the string variant, but the
bytes variant's encoding is the caller's concern).

---

### C10. `HandlerError::StreamError(io::Error)` vs `StreamError` Variant — Conversion Gap

**Documents**: crates/core/core-types.md (L33–38, L118–139),
crates/core/endpoint.md (L248–253), crates/core/auth.md (L55, L59), ADR-010

**Problem**: `core-types.md` and `endpoint.md` (citing ADR-010) both define
`HandlerError::StreamError(io::Error)`. But `auth.md:59` shows handler code
calling
`self.identity_provider.resolve_from_token(&token).ok_or(HandlerError::AuthRequired)?`
after `let stream = connection.accept_bi().await?;` (L55). The `.await?` on
`accept_bi()` returns `StreamError` (per `core-types.md:60`), yet there is no
`From<StreamError> for HandlerError` impl specified. The mapping table in
`core-types.md:132–137` is descriptive prose, not a trait impl. Without a
specified `From<StreamError> for HandlerError` (or `#[from]`), the `?` in the
example will not compile. Two implementers following these docs literally
will diverge: one will hand-map per the table, another will expect an auto-
`From`.

Additionally, `HandlerError::StreamError(io::Error)` holds `io::Error`, but
`StreamError` itself holds `Internal(io::Error)` (L118–124). So
`StreamError::Internal(io::Error)` → `HandlerError::StreamError(io::Error)`
is a lossy wrap (the distinction between stream-level and connection-level is
flattened). The spec should state whether this `From` impl exists and its
exact behavior, because handler code (`?` operator) depends on it.

**Suggestion**: Specify the exact conversion mechanism. Either (a) declare
`HandlerError::StreamError(#[from] StreamError)` — but then the variant must
hold `StreamError`, not `io::Error`, contradicting ADR-010/core-types; or (b)
explicitly state handlers must use `.map_err(HandlerError::from)` or
`.map_err(|e| e.into())` and define the `From` impl; or (c) keep `io::Error`
and define `From<StreamError> for HandlerError` matching the mapping table.
Resolve the variant payload (`io::Error` vs `StreamError`) and specify the
`From` impl. Update `auth.md:55` example to use the canonical conversion
pattern.

---

### C11. ADR-004 Says Handlers "Enrich or Replace" AuthContext; ADR-011 Says AuthContext Is Immutable in `handle()`

**Documents**: ADR-004 (L39, Accepted), ADR-011 (L131–133, Accepted),
crates/core/auth.md (L82–85)

**Problem**: ADR-004 line 39 states the handler "enriches or replaces the
partial `AuthContext` with the fully resolved `Identity`." This describes
**mutation** of the shared AuthContext. ADR-011 explicitly contradicts this
at lines 131–133: "`handle()` signature passes `&AuthContext` (immutable
reference). Handlers that resolve identity create a local variable… they
don't mutate the AuthContext." `auth.md:85` reiterates immutability. The
immutability decision is the later, more refined one, but ADR-004 was never
amended. An implementer reading ADR-004 alone would mutate `AuthContext` —
causing cross-stream contamination (one stream's token resolution leaking
into another stream's AuthContext on the same connection), exactly what
ADR-011 prevents.

This is security-relevant — mutation across streams is a contamination
vector. Same shape as C1 (Accepted ADR contradicts a later refinement that
was never recorded as a supersession).

**Suggestion**: Amend ADR-004 to note that the "enrich/replace" language was
superseded by ADR-011's immutability model (handlers resolve into a local
variable, store on `Connection` via `set_identity`). Add an inline note at
ADR-004 L39 pointing to ADR-011. Same pattern as the C1 fix for ADR-015.

---

### C12. ADR-002's `ProtocolHandler::handle` Signature Is Stale — Not Marked as Superseded by ADR-007

**Documents**: ADR-002 (L19–27, Accepted), ADR-007 (L44–53, Accepted),
overview.md (L89), crates/core/core-types.md (L14–19)

**Problem**: ADR-002 (Status: Accepted) still shows
`async fn handle(&self, stream: BiStream, auth: &AuthContext)` at L26.
ADR-007 revised the signature to
`handle(&self, connection: Connection, auth: &AuthContext)` and its References
note "ADR-002: ProtocolHandler trait (signature revised by this ADR)."
However, ADR-002 itself has **no superseded/amended marker** — it reads as
the current authoritative definition. `overview.md:89` even says "This differs
from the original ADR-002 signature which passed `BiStream`," confirming the
discrepancy is known but ADR-002 was never annotated. An implementer reading
only ADR-002 (as the README's "Applicable ADRs" table encourages) gets the
wrong trait.

**Suggestion**: Add a "Superseded by ADR-007" amendment note at the top of
ADR-002's Decision section (or a `## Amendments` section) stating the
signature was changed by ADR-007. Alternatively, update the ADR-002 code block
to the `Connection` signature with a note "Revised by ADR-007." ADRs must
reflect current state or explicitly mark supersession — this is the same
lifecycle rule that C1 and C11 violate.

---

### C13. `Connection::set_identity` Observability Read Path Is Under-Specified

**Documents**: crates/core/core-types.md (L53–67), crates/core/auth.md (L80),
open-questions.md (OQ-11 L138), crates/core/endpoint.md (L122–136)

**Problem**: `Connection` has private `identity: OnceLock<Identity>`.
`set_identity(&self,…)` takes `&self` (interior mutability) and is
"write-once-read-many." But the endpoint "holds a reference for logging"
(OQ-11:138) and the dispatch code in `endpoint.md:122–136` **moves** `conn`
into the spawned task (`Connection::new(connection)` then
`tokio::spawn(async move { ... handler.handle(conn, &auth) })`). After the
move, the endpoint no longer has the `Connection` to read `identity()` for
observability. OQ-11 says "the endpoint may also hold a reference for
logging" — but there is no `Arc<Connection>` or shared handle shown anywhere;
the struct definition in `core-types.md:53–57` has no sharing mechanism, and
`AlknetEndpoint` in `endpoint.md:17–27` does not hold a registry of live
connections. So the stated observability reader (the endpoint) has no defined
way to access the connection after spawn.

**Suggestion**: Specify the observability read path precisely. Either (a)
the endpoint holds a side table of live connections keyed by connection ID
with `Weak<Connection>` or an `Arc<Connection>` shared with the handler task,
and `Connection` becomes `Arc`-shareable; (b) the handler emits the resolved
identity via a channel/observer callback and the endpoint records it; or (c)
drop the claim that the endpoint reads it and state observability is
handler-side logging only. The current spec is internally inconsistent on
who reads `identity()` and how they obtain the reference.

---

## Warning Findings

### W1. ADR-017's `OperationAdapter` Trait Is Stale Relative to ADR-022 — And the Sync/Async "Two-Way Door" Is Already Decided

**Documents**: ADR-017 (L155–158, L171–174, Accepted), ADR-022 (L225–255,
Proposed)

**Problem**: ADR-017 L171–172 states: "The specific trait signatures (async
vs sync, error types, configuration parameters) are two-way doors for
implementation." Two problems:

1. **The bundle shape is already constrained.** ADR-022 changes registration
   from `(OperationSpec, Handler)` to `HandlerRegistration` (spec + handler +
   provenance + composition_authority + scoped_env + capabilities).
   ADR-022 L257–261 says adapter convenience methods construct
   `HandlerRegistration` with `composition_authority: None` and
   `scoped_env: None`. So the adapter trait must now produce
   `Vec<HandlerRegistration>`, not `Vec<(OperationSpec, Handler)>`. The trait
   signature shown in ADR-017 (`fn import(&self) -> Vec<(OperationSpec,
   Handler)>`) is wrong per ADR-022. The "two-way door for implementation" on
   the signature has already been narrowed by ADR-022 — the return type is
   committed to the bundle, not the pair. ADR-017 has not been updated.

2. **The async/sync question is not fully two-way.** `from_call` is inherently
   async — it discovers operations via `services/list` + `services/schema`
   over a QUIC connection. A synchronous `fn import(&self) -> Vec<…>` trait
   cannot accommodate `from_call` without a separate async pre-step that
   populates a cache, then a sync import that reads the cache. This means the
   trait shape is constrained: either the trait is async (`async fn import`),
   or `from_call` is not an `OperationAdapter` impl and uses a different path.
   The "two-way door" framing suggests you can pick sync now and switch to
   async later; in reality, `from_call` forces at least an async-capable path
   from day one.

This is a "two-way door" label that ADR-022 has silently narrowed without
updating the OQ classification. (See the two-way-door audit, item 7.)

**Suggestion**: Update ADR-017 to: (1) return `Vec<HandlerRegistration>` (not
`(OperationSpec, Handler)`), (2) acknowledge that `from_call` requires async
discovery and the trait must be async or `from_call` must be special-cased, (3)
reclassify the signature as one-way (bundle shape) with a narrow two-way door
on ergonomic details (error types, config params). The current framing in
ADR-017 is inconsistent with ADR-022 and should be reconciled — same
reconciliation pass as C5.

---

### W2. `Capabilities` Concrete Shape and Clone Semantics — "Two-Way Door" With a Hidden One-Way Door Inside

**Documents**: operation-registry.md (L364, L369), ADR-014 (L74–76, L123–124,
L163–165)

**Problem**: The spec says "the concrete shape of the type (a typed map, a
struct with named fields, a trait object) is a two-way door for implementation"
and "the concrete cloning semantics (reference-counted `Arc` vs deep copy of
zeroized material) is a two-way door for implementation, but
`Capabilities: Clone` is required."

The `Clone` requirement is accurately identified as a one-way constraint. The
problem is the *clone semantics*, which the framing treats as two-way but
which contain a hidden one-way door:

- **Arc-based clone (cheap, shared).** If `Capabilities` is
  `Arc<Map<String, Secret>>`, `clone()` bumps the refcount. All calls in a
  composition tree share the same secret material. If a handler can mutate
  capabilities (e.g., add a derived key for a child operation), that mutation
  is visible to siblings and the parent — shared mutable state across the
  call tree. This is a security-relevant behavior.
- **Deep-copy clone (expensive, isolated).** Each call gets its own copy.
  Mutations are isolated. But cloning zeroized secret material on every
  `invoke()` duplicates secrets N times for a depth-N tree, increasing the
  attack surface.

The "path of least resistance" is Arc-based (cheap, the obvious choice for a
`Clone` type holding secrets). Once shipped, handlers may come to depend on
shared-mutation. Switching from Arc-shared to deep-copy-isolated later is a
behavior change that breaks those handlers.

Worse, ADR-022's composition model *assumes* cheap clone — it clones
unconditionally on every `invoke()`. A depth-5 composition tree with Arc-based
clone does 5 refcount bumps (cheap). With deep-copy, it does 5 full copies of
all secret material. The ADR-022 design bakes in the assumption that clone is
cheap — which means it bakes in the Arc-based choice, making the "two-way door"
effectively decided by the composition model's performance assumptions.

**Suggestion**: Add a guard clause: `Capabilities` must be *immutable after
construction* (no interior mutability, no `Mutex<Map>`, no `RefCell`). This
makes Arc-based clone safe (shared immutable state is fine) and makes the
two-way door genuinely two-way: you can switch between `Arc<Map>` and
deep-copy without behavior change, because neither supports mutation. Without
this guard, the "two-way door" is a future one-way door waiting for the first
handler that mutates capabilities.

---

### W3. `CallClient` Registry Mechanism — "Two-Way Door" With a Security Dimension ADR-022 Introduced

**Documents**: ADR-017 (L236–237, Accepted), ADR-022 (L225–242, Proposed)

**Problem**: ADR-017 L236–237: "The specific mechanism (sharing the global
registry, a peer-scoped subset, or a separate registry) is a two-way door."

The three options are mechanically possible with the `HandlerRegistration`
bundle. But ADR-022 introduced a security dimension the framing doesn't flag:

- **Sharing the global registry exposes local capabilities to the remote
  peer.** Each `HandlerRegistration` carries `Capabilities` with secret
  material. If the `CallClient` shares the global registry, the remote peer
  can invoke any External operation — and those operations' handlers use the
  *local* capabilities. A remote peer calling `/llm/generate` uses the local
  node's Google API key. The remote peer effectively gains access to all
  local secret material that backs External operations.
- **A peer-scoped subset must filter by capability remote-safety, not just
  operation name.** The "path of least resistance" for a peer-scoped subset is
  to filter by operation name/namespace. But the real filter must be: "is this
  operation's capability safe to expose to this peer?" An operation backed by
  a signing key that should never leave the node is not remote-safe,
  regardless of its name.

The framing was written before ADR-022 put capabilities in the bundle.
Post-ADR-022, the "two-way door" on the registry mechanism has a security
constraint attached that the framing doesn't mention.

**Suggestion**: Append to ADR-017's negative consequences: "Sharing the
global registry with a `CallClient` exposes all local capabilities (API keys,
signing keys) to the remote peer, because the dispatch path populates
`OperationContext.capabilities` from the registration bundle. A peer-scoped
subset must filter by capability remote-safety, not just operation name. The
registry-mechanism choice is two-way mechanically but has a security dimension
post-ADR-022: the 'share global' option is a capability-exposure decision, not
just a dispatch decision."

---

### W4. `from_call` Hot-Swapping and Registry Mutability — Two "Two-Way Doors" That Gate Each Other

**Documents**: ADR-017 (L279), operation-registry.md (L383)

**Problem**: ADR-017 L279: "Hot-swapping imported specs is a two-way door."
operation-registry.md L383: "The registry is immutable after construction. No
runtime registration or deregistration. Two-way door —
`ArcSwap<OperationRegistry>` can be added later."

These are presented as independent two-way doors. They are not — one gates the
other. If the initial implementation registers `from_call`-imported specs
immutably (per the immutable-registry constraint), and handlers/clients
compile/generated-code against those specs, then "hot-swapping" later means:
re-importing on reconnect, diffing specs, invalidating generated clients,
handling in-flight calls whose schema changed. If the registry is
`ArcSwap`-able, hot-swap is mechanically possible but semantically fraught (a
handler mid-composition doesn't expect the child's schema to change).

Deciding "hot-swap is a two-way door" without resolving the registry-mutability
question leaves an implementer to guess whether to build for immutability or
hot-swap. The deferral hides a coupling: the *immutability* of the registry (a
one-way door per ADR-010 and operation-registry.md L14) and the *mutability*
of remote specs (inherent to `from_call`) are in tension.

Additionally, hot-swapping with ADR-022 bundles requires re-injecting
capabilities (cloning from the old registry or re-deriving from the vault) and
rebuilding the TLS `ServerConfig` (ALPN strings come from the registry).
The "simple ArcSwap" now touches the secret-material flow and the TLS config.
This isn't closing the door, but it raises the cost and security sensitivity
of the swap.

**Suggestion**: Resolve the registry-mutability question (immutable vs
`ArcSwap`) *before* deciding hot-swap, since hot-swap depends on it — don't
let both be independent "two-way doors" when one gates the other. Add a guard
clause to OQ-04: "Hot-swapping the registry with ADR-022 bundles requires
re-injecting capabilities (cloning from the old registry or re-deriving from
the vault) and rebuilding the TLS ServerConfig. The `ArcSwap` upgrade is
mechanical but not trivial — it is a secret-material-handling operation, not
a pure pointer swap."

---

### W5. `to_*` Adapter "Best-Effort" Mappings Become Compatibility Contracts Once Published

**Documents**: ADR-017 (L244–246, L281–286, Accepted), ADR-023 (error schemas
projection)

**Problem**: ADR-017 L244–246: "Some semantics don't map cleanly (e.g.,
subscriptions in OpenAPI, bidirectional calls in MCP). The adapters handle
these with best-effort mappings and document the gaps."

The "best-effort" framing implies the mappings are malleable — two-way,
adjustable. This is misleading once the generated spec is published:

- **A published spec is a compatibility contract.** `to_openapi` generates an
  OpenAPI spec. Once external HTTP clients consume that spec and build against
  it, the mapping semantics (e.g., subscriptions → SSE long-poll, or
  subscriptions → a polling endpoint) become a de facto contract. Changing the
  mapping later breaks every client that built against the original spec. The
  "best-effort" mapping, once published, is a one-way door for the clients
  that depend on it.
- **`from_openapi` round-trip lossiness compounds.** If `to_openapi` maps
  subscriptions to SSE, and later `from_openapi` imports a spec where
  subscriptions are modeled as webhooks, the two adapters disagree on what a
  "subscription" means in OpenAPI. The `to_*` mapping choices constrain the
  `from_*` parsing expectations.
- **With ADR-023**, `to_openapi` now maps `ErrorDefinition.http_status` to
  OpenAPI response codes. Once published, those status code mappings are a
  contract. Changing `FILE_NOT_FOUND` from 404 to 422 in a later `to_openapi`
  revision breaks clients.

This is the "informal spec becomes formal spec" problem. The generated
artifact is the contract; the "best-effort" label is internal framing that
external consumers never see. ADR-009's framework classifies doors by
reversal cost in the codebase, not by compatibility cost for external
consumers — a systematic blind spot.

**Suggestion**: Add a guard clause to ADR-017: "The `to_*` mapping choices,
once published as a generated spec, become a compatibility contract for
external consumers. Best-effort mappings are two-way before first publication
but one-way after. Version the generated specs (e.g., OpenAPI spec version
tied to the registry's External operation set version) and document that
mapping changes are breaking for consumers. The `to_*` adapters should emit a
spec version marker so consumers can detect mapping changes."

---

### W6. The Salt Field's "Reserved for Future KDF" Framing Is Misleading — Semantics Can't Change for Existing Data

**Documents**: ADR-020 (L102–108, L151–157, L204–207, Accepted),
crates/vault/encryption.md (L112–128)

**Problem**: ADR-020 L108: "If a future KDF-based derivation is needed (see
'Future KDF' below), the field is already present." ADR-020 L156: "This is
additive — it doesn't change v2 data."

The "reserved" framing creates a false sense of flexibility:

- **The salt in v2 is cryptographically meaningless and cannot be
  retroactively made load-bearing for v2 data.** v2 encrypts with an
  HD-derived key at `m/74'/2'/0'/0'`. The salt is random, stored, and unused.
  If v3 introduces a KDF that uses the salt, v3's key derivation is different
  from v2's. v3 cannot decrypt v2 data using the salt — v2 was never encrypted
  with a salt-derived key. The salt is only usable for NEW v3+ data. So "the
  field is already present" is technically true but misleading: the field is
  present in v2 blobs, but its presence doesn't make v2 data upgradeable to a
  KDF. It only means v3 blobs can reuse the field without a wire-format
  change.
- **The wire-format-compat framing is the real two-way door, and it's
  accurate.** Keeping the field means the `EncryptedData` struct doesn't
  change shape across v2→v3. That IS a wire-format two-way door — you can add
  a KDF in v3 without changing the struct. But the *semantics* of the salt
  field ("unused" in v2, "load-bearing" in v3) cannot be changed for existing
  v2 data.
- **A KDF doesn't fit the rotation scheme.** Rotation is version-indexed
  paths (`m/74'/2'/0'/{version-2}'`). A KDF-based derivation is a different
  derivation *method*, not a different path. If a KDF is introduced, the
  rotation mechanism (version-indexed paths) and the KDF mechanism
  (salt-based) coexist or conflict. The "reserved salt" framing doesn't
  acknowledge that a KDF is a different derivation family, not just another
  version index.

**Suggestion**: Update ADR-020: "The salt field is reserved for FUTURE
versions' use. v2 data's salt is permanently unused — it was random, never
participated in key derivation, and cannot be retroactively made
load-bearing. Introducing a KDF in v3 is a new derivation method (not a
version-indexed path), requiring its own design and a v2→v3 migration
(re-encrypt with the new KDF, using a newly-generated v3 salt — the v2 salt
is not reused). The field's presence saves a wire-format struct change only;
it does not make the KDF design or migration trivial." Drop the speculative
"reserved for future KDF" framing or explicitly mark it as speculative, not a
planned feature.

---

### W7. `unlock_new` Returns the Mnemonic as a Non-Zeroizing `String` — Security Gap

**Documents**: crates/vault/service.md (L71–79)

**Problem**: `unlock_new(word_count) -> Result<String, VaultServiceError>`
returns the generated mnemonic phrase as a `String`. The `Mnemonic` type
zeroizes on drop (mnemonic-derivation.md L77–78), but `String` does not
implement `Zeroize` by default. The root of trust is returned to the caller
as a plaintext heap-allocated `String` that lingers in freed memory until the
allocator reuses it. The spec says "Store the returned phrase securely"
(L78–79) but does not address that the returned `String` cannot be zeroized
by the caller without `unsafe` or converting to a zeroizing container.

This contradicts the stated principle "Zeroize everything sensitive"
(vault README L65–67) and the `Mnemonic` type's own zeroization. The Security
Constraints section (service.md L365–399) does not mention this.

**Suggestion**: Either return a zeroizing wrapper (e.g., `Zeroizing<String>`
or a dedicated `Phrase` type that implements `Zeroize`/`ZeroizeOnDrop`), or
document in Security Constraints that the caller must convert the returned
`String` to a zeroizing type and must not let it be cloned/logged. At
minimum, flag this as a known limitation.

---

### W8. `DerivedKey` JSON Deserialization Silently Produces a Corrupted Key

**Documents**: crates/vault/protocol.md (L94–112)

**Problem**: protocol.md L104–107 states: "A JSON-deserialized `DerivedKey`
will have `"[REDACTED]"` as its `private_key` string — this is expected; JSON
round-tripping a `DerivedKey` is not a supported use case (the private key is
gone)."

This is a silent-failure footgun: if a `DerivedKey` is accidentally
serialized to JSON and then deserialized (e.g., in a log pipeline, a debug
tool, or a caching layer that uses JSON), the resulting `DerivedKey` has a
10-byte string (`"[REDACTED]"`) as its `private_key` instead of 32 bytes. Any
code that then uses this `DerivedKey` for signing will either produce garbage
signatures or panic on length mismatch — with no error at deserialization
time. The redaction is a defense-in-depth for *serialization* (logs), but the
*deserialization* side silently accepts the redacted form, producing a
corrupted key.

For the irpc/postcard path this is not an issue (postcard preserves bytes).
But if any code path ever uses JSON for `DerivedKey` transport (the spec says
this is "not a supported use case" but doesn't prevent it), the corruption is
silent.

**Suggestion**: Either (a) make `DerivedKey::deserialize` reject payloads
where `private_key == "[REDACTED]"` (returning an error rather than a
corrupted key), or (b) document explicitly that `private_key` length must be
validated before use and that JSON-deserialized `DerivedKey`s are invalid.
Consider whether the custom `Deserialize` impl should enforce a 32-byte
length check.

---

### W9. No ADR for the HD-Derivation Key Model (Identity/SSH/Signing) — Only Encryption Keys Have ADR-020

**Documents**: crates/vault/mnemonic-derivation.md (L31–46, L248–256),
crates/vault/README.md (L68–70)

**Problem**: ADR-020 covers HD derivation for *encryption* keys. But the
vault's primary use of HD derivation is for identity keys, SSH host keys, and
signing keys (mnemonic-derivation.md L26–29, L184–215). The rationale for "HD
derivation, not stored keys" (L31–46: no key storage, reproducible across
nodes, domain separation, auditable derivation) is inline in the spec's "Why
HD Derivation" section. The Design Decisions table (L250–256) lists this with
no ADR ("HD derivation (not stored keys) | — | One seed, many keys, no key
storage").

This is a foundational architectural decision — the entire vault model
depends on it — yet it has no ADR. The choice of HD derivation vs. stored keys
is a one-way door (switching to stored keys would change the entire trust
model). ADR-018 and ADR-019 both *assume* HD derivation but don't *decide* it.

Similarly, these design choices have inline rationale but no ADR:
- **`74'` coin type reservation** (L186, L254): SLIP-0044 unallocated coin
  type claimed for alknet. This is a namespace allocation that, once keys are
  derived, cannot be changed without re-deriving all keys — effectively
  one-way. No ADR records this allocation.
- **secp256k1 feature-gating** (L166–179): rationale (heavy C dependency, only
  needed for Ethereum) is inline. No ADR.
- **AES-256-GCM cipher/mode choice** (encryption.md L27–37): rationale
  (authenticated, hardware-accelerated) is inline. No ADR.

**Suggestion**: Either (a) create an ADR for "HD derivation as the vault's key
model" covering identity/SSH/signing keys (the encryption case is already
ADR-020), or (b) broaden ADR-020's scope, or (c) create a single "Vault Key
Model" ADR covering the overarching HD-derivation decision with ADR-020 as a
special case for encryption keys. Record the `74'` coin type reservation and
the AES-256-GCM choice in ADRs or a tracked decisions log, since they are
one-way doors with no current ADR.

---

### W10. The `EncryptedData` "Stable Wire Format" Is a One-Way Door Documented Only in Prose

**Documents**: crates/vault/encryption.md (L84–95), ADR-018 (L101–104)

**Problem**: encryption.md L86–95 states: "This is the **stable wire format**
shared with `alknet-storage` (a future crate) by type-level agreement, not by
a crate dependency. Both crates must agree on the serialization format… This
cross-language compatibility is why the wire format must stay stable —
changing it breaks both `alknet-storage` and the TypeScript consumer."
ADR-018 L101–104 repeats this.

This is a one-way door: once `alknet-storage` and TS consumers depend on the
format, changing it breaks them. Yet it is not locked by an ADR. It is a
cross-crate compatibility contract enforced by documentation, not by a decided
constraint. An implementer modifying `EncryptedData` (e.g., removing the
unused `salt` field) would find no ADR saying "this format is frozen." The
"type-level agreement, not a crate dependency" approach (ADR-018) means there
is no compiler enforcement either — the stability contract exists only in
prose.

**Suggestion**: Create an ADR (or add a decision to ADR-018) locking the
`EncryptedData` wire format as stable, with the compatibility surface
explicitly enumerated (fields, base64 encoding, field semantics). Note that
the `salt` field, though unused in v2, is part of the frozen format and cannot
be removed without a format-version migration.

---

### W11. `OperationContext.identity` Conversion from `CompositionAuthority` Is Undefined

**Documents**: operation-registry.md (L236:
`identity: parent.handler_identity.as_identity()`), ADR-022 (L314:
`identity: parent.handler_identity_as_identity()`)

**Problem**: Both spec and ADR-022 populate the child's `identity` by
converting the parent's `handler_identity: Option<CompositionAuthority>` into
an `Option<Identity>` via a method `as_identity()` /
`handler_identity_as_identity()`. This conversion method is **never defined**
and its semantics are unclear: `CompositionAuthority` (label + scopes +
resources) is a declared authority bundle, while `Identity` is a peer identity
resolved via `IdentityProvider`. ADR-022 goes to lengths to stress that
`CompositionAuthority` is *not* a peer `Identity` (L150–160). Yet the dispatch
path needs an `Identity` to run ACL against for the child.

So there's an implicit conversion from a non-peer authority bundle to a peer
identity type. Is it a synthetic `Identity` constructed with the authority's
label as `id` and its scopes? Does it bypass `IdentityProvider`? Is
`as_identity()` returning `None` when `handler_identity` is `None` (the leaf
case)? For leaves, the ACL "checks against the leaf's `AccessControl`" — but
if `identity` is `None` because the parent is a leaf with
`handler_identity: None`, then per operation-registry.md L86–87 "If the
relevant identity is `None` and the operation has restrictions, the adapter
returns `FORBIDDEN`." That would mean a composing handler with no composition
authority cannot compose *any* restricted leaf — which contradicts the model
where leaves are composed under the *parent's* authority.

Actually: a composing parent (Local/Session) *does* have
`handler_identity: Some(...)`, so `as_identity()` returns `Some`. Leaves being
composed *are* the child; their own `handler_identity` is `None` but that's
fine because the child's ACL uses the child's `identity` (which is the parent's
authority). So the conversion is load-bearing and must be specified.

**Suggestion**: Define `CompositionAuthority::as_identity() -> Option<Identity>`
(or `handler_identity_as_identity()` on `OperationContext`) explicitly in the
spec or ADR-022. Specify that it constructs a synthetic
`Identity { id: label, scopes, resources }` that is *not* resolvable via
`IdentityProvider` but is used directly for ACL matching. Note that this
creates a second `Identity` construction path (the first is
`IdentityProvider::resolve_*`), which should be acknowledged.

---

### W12. Source Drift Items Are Tracked Only in Scattered Prose — The `spawn` Return Bug Could Be Lost

**Documents**: crates/vault/README.md (L79–103), crates/vault/encryption.md
(L185–188, L245–250), crates/vault/service.md (L280–286, L374–391)

**Problem**: Four source-vs-spec drift items are documented in prose across
multiple files:

1. **OsRng vs `rand::random()`** — documented in README L87–89, encryption.md
   L245–250, service.md L374–376 (3 locations).
2. **`unwrap()` on RwLock** — documented in README L94–97, service.md
   L384–391 (2 locations).
3. **`CURRENT_KEY_VERSION = 1` (should be 2)** — documented in encryption.md
   L185–188, ADR-020 L120–124 (2 locations, none in README).
4. **`spawn` returns unspawned actor** — documented **only** in service.md
   L280–286 (1 location, inside a method description, not in a Security
   Constraints section).

Problems with this approach:
- The `spawn` bug (item 4) is documented in exactly one place, buried in the
  `spawn` method's bullet description, not in any "Security Constraints" or
  "Known Drift" section. An implementer who reads the Security Constraints
  sections (the natural place to look for implementation-sync corrections)
  would miss it.
- README's Security Constraints (L79–103) lists OsRng and unwrap but **omits**
  `CURRENT_KEY_VERSION` and the `spawn` bug. README is the index document; an
  implementer using it as a checklist would miss two drift items.
- There is no single, structured tracking mechanism. If one prose location is
  updated and others aren't, the docs silently diverge.

**Suggestion**: Add a single "Known Source Drift" table to README.md (or a
dedicated section) listing all drift items with: item, current behavior,
target behavior, location in source, and status. At minimum, add
`CURRENT_KEY_VERSION` and the `spawn` bug to README's Security Constraints.
Move the `spawn` bug note out of the method description and into the drift
tracker.

---

### W13. `CertAuthorityEntry` Is an Undefined Placeholder Type Used in `DynamicConfig`

**Documents**: crates/core/config.md (L119–130)

**Problem**: `AuthPolicy.cert_authorities: Vec<CertAuthorityEntry>` is
declared, but `CertAuthorityEntry` is explicitly "a placeholder type. Its
fields will be defined when alknet-ssh is implemented." This means alknet-core's
public API references a type that does not exist yet. The spec says "for v1,
`cert_authorities` will be an empty vector," implying the field is dead weight.
An implementer cannot compile `DynamicConfig` without defining
`CertAuthorityEntry` somewhere. Where does it live — alknet-core (but it's
SSH-specific per the doc) or alknet-ssh (but then alknet-core can't reference it
without a dependency)? This is a crate-dependency contradiction: core
references an SSH-defined type, but core cannot depend on alknet-ssh (ADR-003
forbids it).

**Suggestion**: Either define a minimal `CertAuthorityEntry` in alknet-core
(as an opaque/newtype or a generic struct with the fields that are
ALPN-agnostic), or remove `cert_authorities` from v1 `AuthPolicy` entirely and
add it back when alknet-ssh lands (it's a two-way door — adding a field is
additive). Leaving an undefined type in a public struct blocks compilation.

---

### W14. `SecretKey` Type in `TlsIdentity::RawKey(SecretKey)` Is Undefined and Its Origin Ambiguous

**Documents**: crates/core/config.md (L44, L79)

**Problem**: `TlsIdentity::RawKey(SecretKey)` and `SecretKey::generate()` are
used but `SecretKey` is never imported or attributed. It could be
`ed25519_dalek::SecretKey`, `iroh::SecretKey`, `alknet_vault::SecretKey`, or a
core-defined type. Since config.md says the key "can be derived from
alknet-vault" (per endpoint.md:181), and alknet-core must not depend on
alknet-vault (ADR-003, ADR-018), this matters: if `SecretKey` is from iroh,
then alknet-core depends on iroh even for quinn-only nodes; if it's a
re-export/newtype, that needs stating. The crate dependency table in ADR-003
L27 lists alknet-core depending on "tokio, quinn, rustls, irpc" — **not iroh**
— yet `config.md` and `endpoint.md` show iroh types in core's public API.

**Suggestion**: Specify the exact type and crate for `SecretKey`. If it's
`iroh::SecretKey`, update ADR-003's dependency list (iroh is already used for
`iroh::Endpoint`, so this is consistent, but ADR-003 omits iroh from
alknet-core's deps — a separate inconsistency). If it's `ed25519_dalek`, say
so. Resolve whether alknet-core publicly depends on iroh types.

---

### W15. ADR-003 Omits iroh From alknet-core's Dependency List; Spec Docs Assume It

**Documents**: ADR-003 (L27), crates/core/endpoint.md (L20), config.md (L44)

**Problem**: ADR-003 L27 lists alknet-core as depending on "tokio, quinn,
rustls, irpc" — no iroh. But `AlknetEndpoint` holds
`iroh: Option<iroh::Endpoint>` (endpoint.md L20, ADR-010 L67),
`SendStream`/`RecvStream` wrap "iroh::SendStream or iroh::RecvStream"
(core-types.md L103), and `TlsIdentity::RawKey(SecretKey)` likely uses iroh's
key type. ADR-010 L184 says this is "mitigated: both are feature-gated." But
ADR-003's dependency table is now stale — it doesn't reflect the iroh
dependency that ADR-010 introduced.

**Suggestion**: Update ADR-003's dependency table to include iroh
(feature-gated) for alknet-core, or add an amendment note that ADR-010 added
the iroh dependency.

---

### W16. Overview Failure-Mode Table Says Handler Error Closes "QUIC Stream"; Spec Says Connection

**Documents**: overview.md (L237–238), crates/core/core-types.md (L30, L46),
crates/core/endpoint.md (L255–257, L262)

**Problem**: `overview.md:237` says "Handler `handle()` returns `HandlerError`
| Endpoint logs the error, closes the QUIC **stream**." But
`core-types.md:30` says "The endpoint catches these, logs them, and closes the
**connection**." `endpoint.md:256` says HandlerError is "Non-fatal errors
within a handler" and the connection closes. The whole architecture is
one-ALPN-per-connection (ADR-006), and `handle()` receives the whole
`Connection`. Closing a "stream" doesn't make sense when the handler owns the
connection. This is a stale leftover from the pre-ADR-007 model where
`handle()` received a `BiStream`.

**Suggestion**: Change `overview.md:237–238` from "closes the QUIC stream" to
"closes the QUIC connection."

---

### W17. OQ-09's "BiStream Trait Preserves the WASM Door" Is Only True for the Client Side

**Documents**: open-questions.md (OQ-09 L100–107), ADR-007, ADR-009 (L41, L44),
crates/core/core-types.md, crates/core/endpoint.md

**Problem**: The resolution claims "The BiStream trait decision (ADR-007) has
already preserved the most important WASM door." This is accurate for the
*client* path (a browser can implement `BiStream` over WebTransport) but
oversells what is preserved for a *server-side* WASM peer:

- **`Connection` is a concrete struct, not a trait** (ADR-007). Its
  `accept_bi()`/`open_bi()` return `SendStream`/`RecvStream` described as
  wrapping "the underlying QUIC stream types." A WASM server-side peer cannot
  implement this `Connection` without a QUIC library that compiles to WASM.
  BiStream being a trait does not help — handlers receive `Connection`, and
  `Connection` is the binding point.
- **The accept loop uses `tokio::spawn`** (ADR-010). Tokio does not run on
  WASM. The entire endpoint runtime is tokio-bound.
- **`PendingRequestMap` and the `CallAdapter` use tokio `oneshot`/`mpsc`
  channels** (call-protocol.md L218–240). These are tokio-specific.

The OQ framing implies BiStream was the binding constraint and the rest is
detail. In reality, BiStream preserves the *client stream* door, but
`Connection`, the accept loop, and the call-protocol dispatch internals
quietly close the *server-side dispatch* door.

**Suggestion**: Update OQ-09's resolution to state: "BiStream being a trait
preserves the client-side stream door. The server-side dispatch door
(`Connection` as a concrete quinn-bound struct, tokio-based accept loop, tokio
channel dispatch) is NOT preserved by ADR-007 and would require a `Connection`
trait and a runtime-abstracted accept loop to open. This is a known, accepted
closure — the browser path is client-side via a JS SDK, not server-side
Rust-to-WASM." This converts the misleading framing into an explicit, accepted
one-way door.

---

### W18. OQ-10's "Start With Smart Protocol" Deferral Forecloses Git Composability

**Documents**: open-questions.md (OQ-10 L109–116)

**Problem**: The deferral says "start with git smart protocol over QUIC
streams, ERC721 and full server capabilities are additive." The ALPN/endpoint
design does not constrain the git adapter's *wire format* — the handler owns
its format and `alknet/git` is a dedicated connection. That part is genuinely
two-way.

However, the deferral hides a fork that the "path of least resistance" will
silently resolve in a way that forecloses composability:

- **Raw smart protocol vs. call-protocol projection.** If git ships as a
  `ProtocolHandler` speaking raw git smart protocol over QUIC streams (the
  simplest path), git operations are NOT callable via `env.invoke()`. They
  exist only on the `alknet/git` ALPN, not in the `OperationRegistry`. An
  agent handler that wants to compose `git/clone` cannot — there's no
  `OperationSpec`, no `Handler`, no registration. The agent would have to
  open a raw QUIC connection to `alknet/git` and speak pkt-line, bypassing
  the entire composition/security model (ADR-015, ADR-022).
- To make git composable later, you'd need a *separate* call-protocol
  projection — a set of `OperationSpec`/`Handler` pairs that wrap git
  operations behind the registry. That's additive, but it means the "simple"
  choice (raw smart protocol only) produces a git adapter that is permanently
  non-composable unless a second handler layer is built.

The deferral frames ERC721/full-server as the additive part. But the real
additive question is composability, and the simplest implementation forecloses
it.

**Suggestion**: Append to OQ-10's resolution: "The composability of git
operations (whether git ops are registered in the `OperationRegistry` and
callable via `env.invoke()`, or only available as raw smart protocol on
`alknet/git`) is a separate decision from ERC721 scope. The path of least
resistance (raw smart protocol only) forecloses agent composition of git
operations. If agent tool dispatch needs to compose git ops, a call-protocol
projection must be built alongside or instead of the raw handler. Resolve
this when speccing alknet-git, not deferred past it."

---

### W19. ADR-016 Self-Referential Citation and Under-Specified Grandchild Propagation

**Documents**: ADR-016 (L134, L154–158)

**Problem**: Two minor issues in ADR-016:

1. **Self-referential citation** (L134): ADR-016 cites itself ("ADR-016
   Assumption 5 says the policy is per-call, not per-operation") in its own
   Decision 6 rationale. This should just be "Assumption 5" (it's the same
   document).

2. **Grandchild propagation under-specified** (L154–157): "The child's
   `OperationContext` carries the policy. If the child itself composes
   grandchildren, the policy propagates unless the child explicitly overrides
   it." This is the only statement of propagation-vs-override semantics. It
   doesn't specify: does the grandchild *inherit* the child's policy by
   default (so if the child got `ContinueRunning`, the grandchild is
   `ContinueRunning` unless overridden)? Or does each `invoke()` reset to
   the default `AbortDependents` unless explicitly set? The phrase
   "propagates unless overridden" implies inheritance, which is reasonable,
   but the `invoke()` default (operation-registry.md L213, ADR-016 L146) shows
   `context.env.invoke(...)` with no policy = default. So is "no policy arg"
   = "inherit parent's" or "reset to AbortDependents"? These conflict.

**Suggestion**: (1) Change "ADR-016 Assumption 5" to "Assumption 5" within
ADR-016. (2) Clarify: does `invoke()` with no policy argument (a) inherit the
parent's current policy, or (b) reset to `AbortDependents`? Recommend (b)
reset-to-default for predictability (each composition is a fresh decision)
unless the parent opts the child into `ContinueRunning`, and document that
`ContinueRunning` does *not* auto-propagate to grandchildren (the child must
re-affirm it for each grandchild). Or pick (a) and update the `invoke()`
examples. Either is defensible; pick one.

---

### W20. ADR-023's `from_openapi` Error Code Example Collides With Protocol-Level `NOT_FOUND`

**Documents**: ADR-023 (Assumption 3 L366–373, Decision 5 L241)

**Problem**: ADR-023 Assumption 3 acknowledges the collision risk: an operation
declaring `NOT_FOUND` as a domain code collides with the protocol-level
`NOT_FOUND` (operation not registered). The resolution (L367–368): "the
protocol code takes precedence in the dispatch machinery." And (L372–373):
"The assumption is that operations use domain-specific codes (`FILE_NOT_FOUND`)
rather than reusing protocol codes (`NOT_FOUND`). This is a naming convention,
not a type-system enforcement."

But Decision 5's example (L241) shows `from_openapi` mapping a 404 to
`ErrorDefinition { code: "NOT_FOUND", http_status: Some(404), … }` — i.e.,
`from_openapi` *does* produce a domain code that collides with a protocol
code. So the "naming convention" is violated by the very adapter the ADR
specifies. An implementer following Decision 5's example would create a
`FILE_NOT_FOUND`-vs-`NOT_FOUND` ambiguity: when `call.error` carries
`code: "NOT_FOUND"`, is it "operation not registered" (protocol) or "file not
found" (operation-level, from a 404)? The `details` field disambiguates
(present for operation-level, absent for protocol-level), but the `code` alone
is ambiguous, and ADR-023 L171 says "Clients should switch on `code`, not
parse `message`."

**Suggestion**: Either (a) make `from_openapi` prefix/namespace the imported
error codes (e.g., `HTTP_404` or `<op>_NOT_FOUND`) to avoid collision, or (b)
update Decision 5's example to use a non-colliding code and add a normative
rule that `from_openapi` must not produce codes that match the five
protocol-level codes. Assumption 3's "naming convention" should be elevated to
a requirement for adapters, since the adapter example breaks it.

---

### W21. `ScopedOperationEnv` Public Field Leaks the `HashSet` Representation — Future Subgraph Refactor Is Breaking

**Documents**: ADR-022 (L208–214, L97–105), operation-registry.md (L188, L328)

**Problem**: ADR-022 L97–105: "For v1, the scoped env can be a
`HashSet<String>` of reachable operation names. A dedicated
`alknet-flowgraph` crate … is a future enhancement — not a prerequisite for
the security model."

The `HashSet<String>` representation is fine *internally*. The problem is that
it leaks into the public API in a way that makes the future refactor a
breaking change:

- **`ScopedOperationEnv` is a public struct with a public field.** ADR-022
  L208–214 shows `pub struct ScopedOperationEnv { pub allowed_operations:
  HashSet<String> }`. The assembly layer and agent crate construct this
  directly: `ScopedOperationEnv { allowed_operations: HashSet::from(…) }`
  (operation-registry.md L188, L328). If the representation later changes to a
  subgraph type (with type-compatibility edges, as the flowgraph model
  requires), every construction site breaks — the field type changes from
  `HashSet<String>` to `Subgraph` or `Vec<Edge>`. This is a public-API
  breaking change, not an internal refactor.

The framing says the flowgraph crate is "not a prerequisite for the security
model." That's true — the security model works with a flat set. But the
*refactor* from flat set to subgraph is not the trivial "two-way door" the
framing implies. It's a breaking change to construction sites.

Additionally, this interacts with OQ-19 (session-scoped registries): session
ops compose under a scoped env. If the scoped env is a flat `HashSet<String>`,
session ops get reachability control (can this op be reached?) but NOT
type-compatibility validation (does the output of op A feed validly into op
B?). A session op (untrusted, LLM-written) that composes an op whose output
type doesn't match the next op's input gets a runtime error, not a static
rejection. The flowgraph deferral means session ops ship without static type
validation — a security-relevant gap for untrusted code.

**Suggestion**: Make `allowed_operations` private (not `pub`), with a
constructor `ScopedOperationEnv::new(ops: impl IntoIterator<Item = String>)`
and a query method `allows(&self, name: &str) -> bool`. This keeps the
representation internal and makes the future subgraph refactor a non-breaking
change. Add to the flowgraph deferral: "The `HashSet<String>` representation
does not support type-compatibility validation. Session-scoped ops (OQ-19,
untrusted code) compose without static type checking until the flowgraph
crate is built. This is a known gap for the untrusted-code path, not just a
visualization feature."

---

## Suggestions

### S1. Inline Decision Rationale in Spec Docs That Should Be in ADRs

**Documents**: call-protocol.md (L341, L343–344, L124), operation-registry.md
(L202, L364–369, L377), crates/vault/encryption.md (L27–37),
crates/vault/mnemonic-derivation.md (L31–46)

**Problem**: Per the architect's documentation principles, spec docs should
reference ADRs by number, not contain the rationale. Several spec sections
contain *why* rationale that duplicates or extends ADR content:

- call-protocol.md L341 duplicates ADR-016 Decision 2's rationale ("the
  correct default because aborted parent work has no consumer…") verbatim.
- operation-registry.md L202 explains *why* metadata doesn't propagate (the
  secret-leak scenario) — this is ADR-014's rationale, restated inline.
- operation-registry.md L364–369 explains the `Clone` semantics of
  `Capabilities` and the security implication — this is a design rationale,
  not in any ADR. It should be in ADR-014 or ADR-022.
- operation-registry.md L377 ("`from_call` trust is transitive… a compromised
  remote node can do anything") is a threat-model statement that belongs in
  ADR-017.
- crates/vault/encryption.md L27–37 (AES-256-GCM rationale) and
  mnemonic-derivation.md L31–46 (HD derivation rationale) have no ADRs (see
  W9).

**Suggestion**: Move the unique rationale to the relevant ADRs (ADR-016
already has the abort-dependents rationale — just cite it; add
capabilities-clone rationale to ADR-014 or ADR-022; add `from_call` transitive
trust to ADR-017) and replace the inline rationale with ADR references. Keep
only the *constraint statement* in the spec.

---

### S2. Untracked "Two-Way Door" Deferrals Scattered Through Specs

**Documents**: operation-registry.md (L383, L257, L364, L369),
call-protocol.md (L333), ADR-017 (L236–237, L279)

**Problem**: Per the documentation principles, inline open questions should be
in `open-questions.md`. Several "two-way door" / "future work" notes are
scattered through the specs without being tracked as open questions:

- operation-registry.md L383: "Two-way door — `ArcSwap<OperationRegistry>`
  can be added later" (contradicts the "immutable after construction"
  constraint at L14, L194, L340).
- operation-registry.md L257: "Future work may add irpc service dispatch and
  remote call protocol dispatch as additional backends."
- operation-registry.md L364: "The concrete shape of the type… is a two-way
  door for implementation."
- operation-registry.md L369: "The concrete cloning semantics… is a two-way
  door for implementation."
- call-protocol.md L333: timeout policy (30s default, adapter-level config) —
  no ADR governs this.
- ADR-017 L236–237, L279: CallClient registry mechanism and `from_call`
  hot-swapping.

**Suggestion**: Add the genuinely-open items (Capabilities concrete shape,
Capabilities clone semantics, `OperationAdapter` trait signatures, CallClient
registry mechanism, `to_*` adapter gap semantics, `from_call` hot-swap,
registry hot-swap via ArcSwap, timeout policy) to `open-questions.md` with
explicit status. For items that are *decided* deferrals, record the deferral
in the relevant ADR's Consequences rather than as inline spec commentary.

---

### S3. Missing ADR References for Design Choices ("Why X Over Y?")

**Documents**: call-protocol.md (L86, L116, L262–272, L333),
operation-registry.md (L64–67, L120–127, L139)

**Problem**: Several design choices are stated in the spec with no ADR
backing:

- **Binary payload convention** (call-protocol.md L86): base64-in-JSON. Why
  not a separate binary event type? No ADR.
- **`auth_token` in payload** (call-protocol.md L116–122, L262–272): per-
  request identity upgrade via an optional `auth_token` field. Significant
  security-model decision with no ADR.
- **Per-request vs per-connection identity** (L272): "Identity is resolved
  per-request, not per-connection." No ADR.
- **Timeout policy** (L333): 30s default, adapter-level config. No ADR.
- **Module-private `internal` writes** (operation-registry.md L120–127):
  the enforcement mechanism for "handlers can't set `internal`." The
  requirement is from ADR-015; the mechanism (Rust visibility) has no ADR.
- **Namespace derivation** (operation-registry.md L66): "`namespace` is
  derived from the name." Why derived rather than independently declared? No
  ADR.

**Suggestion**: For load-bearing choices (binary convention, auth_token /
per-request identity, timeout policy), either add an ADR or extend an
existing one. For minor choices (namespace derivation, visibility pattern), a
one-line note suffices, but they shouldn't be silently decided in the spec.

---

### S4. README ADR Tables Missing Entries and Status Column

**Documents**: crates/call/README.md (L19–36), crates/core/README.md
(L21–31), crates/vault/README.md (L38–47)

**Problem**: Three issues across the crate READMEs:

1. **call README** omits ADR-013 (Rust canonical implementation), which both
   call spec docs reference (call-protocol.md L88, operation-registry.md L24).
2. **core README** omits ADR-015, referenced by auth.md L78, L200 and
   config.md L200.
3. **vault README** omits ADR-010 and ADR-006, referenced by
   mnemonic-derivation.md L26 and protocol.md L290.
4. **None of the crate READMEs have a Status column** in their ADR tables,
   which contributes to C2 (reader can't tell which ADRs are binding).

**Suggestion**: Add the missing ADRs to each crate README's ADR table. Add a
Status column (Accepted/Proposed/Superseded) to make the governance state
visible — this directly supports resolving C1/C2.

---

### S5. `Connection::new()` Referenced but Never Defined

**Documents**: crates/core/core-types.md (L113), crates/core/endpoint.md
(L127), ADR-010 (L117)

**Problem**: `Connection::new(connection)` is called in dispatch pseudocode
(endpoint.md L127, ADR-010 L117) but the `impl Connection` block in
`core-types.md:59–67` does not include a `new()` constructor, nor specify its
signature/parameter. `core-types.md:113` says "`Connection::new()` wraps the
appropriate stream source based on where the connection came from" — but
there's no formal signature, and the argument type differs by source (a
`quinn::Connection` vs an iroh `Accepting`/`Connecting`). An implementer
doesn't know the constructor's parameter type.

**Suggestion**: Add `pub fn new(source: ConnectionSource) -> Connection` (or
two constructors `from_quinn` / `from_iroh`, or a private enum) to the
`impl Connection` block. Define `ConnectionSource` or the variant
constructors.

---

### S6. `services/schema` Input Shape Under-Specified (Leading Slash)

**Documents**: operation-registry.md (L286), call-protocol.md (L120, L126)

**Problem**: The input shape for `services/schema` is shown as
`{ "name": "fs/readFile" }` (no leading slash) but the wire `operationId`
convention (call-protocol.md L120, L126) is *with* a leading slash
(`/fs/readFile`). So does `services/schema`'s `name` input use the wire form
(leading slash) or the registry form (no slash)? The spec shows no slash,
implying registry form, but a client calling `services/schema` over the wire
would naturally use the same `operationId` form it uses elsewhere. This is a
small ambiguity but could cause a `NOT_FOUND` if the adapter doesn't
normalize.

**Suggestion**: Specify whether `services/schema`'s `name` input has a
leading slash and whether the adapter strips it (consistent with
`call.requested`'s `operationId` handling per call-protocol.md L120).

---

### S7. `call.completed` For Non-Subscription Calls Not Addressed

**Documents**: call-protocol.md (L96–106)

**Problem**: The event table lists `call.completed` as "Signal end of
subscription stream" — subscription-only. But for a plain `call()`
(request/response, one result), the lifecycle is `call.requested` →
`call.responded` (one) and then… nothing? Is the request considered complete
on the first `call.responded`? Is a `call.completed` ever sent for
non-subscription calls? The spec says (L103) "call(): Sends `call.requested`,
resolves on first `call.responded`" — so no `call.completed` for calls. But
then what signals to the server that the client is done listening? This is
consistent but the asymmetry (subscriptions get `completed`, calls don't)
could be stated explicitly to avoid an implementer sending a spurious
`call.completed` after a `call.responded`.

**Suggestion**: Add a note: "`call.completed` is sent only for subscriptions.
A plain `call()` is complete after its single `call.responded`; no
`call.completed` follows."

---

### S8. `OperationProvenance::FromCall` Comment Is Ambiguous ("Leaf Locally")

**Documents**: ADR-022 (L123–124)

**Problem**: `FromCall` => "QUIC forwarding stub (from_call), leaf locally —
cannot compose." The "locally" qualifier is ambiguous. Does it mean: (a)
`from_call` ops are leaves in the *local* registry but could compose on the
*remote* node? Or (b) they're leaves, full stop, and "locally" is just
emphasis? Assumption 5 (L518–524) clarifies, but the phrasing invites
misreading.

**Suggestion**: Rephrase to "leaf in the local registry — forwards calls to
a remote node; cannot compose locally" or similar to remove the ambiguity.

---

### S9. `ScopedOperationEnv::empty()` and `CompositionAuthority::none()` Referenced but Undefined

**Documents**: operation-registry.md (L182–184, L243–244, L298–299, L318–320),
ADR-022 (L287)

**Problem**: The spec and ADR-022 use `ScopedOperationEnv::empty()` and
`CompositionAuthority::none()` (constructor methods) but neither type's
definition (ADR-022 L164–180 for `CompositionAuthority`, L208–214 for
`ScopedOperationEnv`) lists these methods. They're implied (empty set /
None-equivalent) but not shown.

**Suggestion**: Add `impl ScopedOperationEnv { pub fn empty() -> Self { … }
pub fn allows(&self, name: &str) -> bool { … } }` and
`impl CompositionAuthority { pub fn none() -> Option<Self> { None } }` (or
similar) to the spec or ADR-022.

---

### S10. Overview OQ-04 Entry Is Stale

**Documents**: overview.md (L225)

**Problem**: `overview.md:225` lists "OQ-04: Dynamic handler registration at
runtime vs static at startup (two-way door, defer to implementation)." But
OQ-04 in `open-questions.md:47–51` is **resolved** ("Static registration at
startup"). The overview shows a stale framing ("defer to implementation")
while the OQ is resolved. Compare to OQ-01/OQ-02/OQ-03 which are correctly
marked "resolved" in the same overview list.

**Suggestion**: Update `overview.md:225` to "OQ-04: Dynamic handler
registration (resolved: static at startup — see ADR-010)" to match the
resolved state.

---

### S11. ADR-021 References "W1 Drift Issue from the Vault Review" — Ambiguous Cross-Reference

**Documents**: ADR-021 (L127–129)

**Problem**: ADR-021 L127–129 states: "This is the fix for the W1 drift issue
from the vault review: the current source ignores `key_version` for key
selection; the spec now makes it functional." But review 001's W1 (L355–371)
is about *ADR-020's stale example path* (`m/74'/2'/1'/0'` vs
`m/74'/2'/0'/1'`), not about the source ignoring `key_version`. The
source-ignoring-`key_version` drift is not tracked in review 001 at all. The
"W1" label in ADR-021 doesn't match review 001's W1.

**Suggestion**: Clarify the reference — either cite review 001 with the
correct finding number, or remove the "W1" label and describe the drift
directly (which the text already does).

---

### S12. Duplicate "Helper Functions" Block in mnemonic-derivation.md

**Documents**: crates/vault/mnemonic-derivation.md (L199–204, L217–223)

**Problem**: The helper functions `device_path` and `site_password_path` are
documented twice: L199–204 (missing `encryption_path_for_version`) and
L217–223 (complete). An implementer reading the first block might not realize
`encryption_path_for_version` exists, or might think there are two conflicting
definitions. The duplication is a maintenance hazard.

**Suggestion**: Remove the first block (L199–204); keep the complete list at
L217–223.

---

## Cross-Cutting Theme: ADR-022 Narrowed Several "Two-Way Doors" Without Updating the OQ Classifications

The two-way-door audit found a systematic pattern: ADR-022 (Proposed, not yet
Accepted) is the single largest source of door-type staleness. It narrowed
several doors without updating the OQ classifications that were written
pre-ADR-022:

| Door | Pre-ADR-022 classification | Post-ADR-022 reality |
|------|---------------------------|----------------------|
| Capabilities concrete shape (W2) | Two-way (implementation detail) | Clone semantics become a behavioral contract once handlers depend on shared mutation; ADR-022's composition model bakes in the cheap-clone assumption |
| OperationAdapter trait signatures (W1) | Two-way (async vs sync) | ADR-022 committed the return type to `HandlerRegistration`; `from_call` forces async — both are already decided |
| CallClient registry mechanism (W3) | Two-way (share global vs subset vs separate) | ADR-022 put capabilities (secrets) in the bundle — "share global" is now a capability-exposure decision, not just dispatch |
| Dynamic registration (W4) | Two-way (add ArcSwap later) | Hot-swap now requires re-injecting capabilities (secret-material handling) + TLS config rebuild, not a pointer swap |
| Session-scoped registries (OQ-19) | Two-way (protocol doesn't need changes) | `OperationContext.env` field type is a current commitment — if it's a concrete struct, the session-overlay pattern is closed (see C6) |
| Graph structures (W21) | Two-way (HashSet now, flowgraph later) | `ScopedOperationEnv`'s public field leaks the representation; the refactor is a public-API break, not an internal change |

**Pattern**: ADR-022 was written to resolve review #001's C1–C4 (the
registration-bundle tangle), and in doing so it quietly narrowed several
previously-two-way doors. The OQ tracker and ADR-017 should be reconciled
with ADR-022 before the "two-way door" labels can be trusted. ADR-009's
framework is sound; its application is drifting because later ADRs don't
update earlier OQ classifications.

The fix is not to re-litigate each door — most of ADR-022's narrowing is
correct (the bundle *should* carry capabilities, the adapter *should* produce
`HandlerRegistration`). The fix is to add guard clauses to the affected OQs
and ADRs documenting the narrowed scope, so an implementer reading the OQ
tracker sees the post-ADR-022 reality, not the pre-ADR-022 optimism.

---

## Cross-Cutting Theme: The "Published Artifact Is a Contract" Blind Spot

The audit found a systematic blind spot in ADR-009's framework: it
classifies doors by reversal cost in the codebase, not by compatibility cost
for external consumers. Three deferrals share this pattern:

- **`to_*` best-effort mappings (W5)**: "best-effort" is true before first
  publication but one-way after — external clients depend on the generated
  spec.
- **Error schema `http_status` mappings (W20)**: once `to_openapi` publishes
  `FILE_NOT_FOUND` → 404, changing it breaks clients.
- **The salt field's wire format (W6)**: the field is "reserved" (wire-format-
  stable), but its *semantics* cannot change for existing data — "reserved"
  implies more flexibility than exists.

The framework treats these as two-way because the *format* is additive; it
misses that the *semantics*, once published, are frozen. This is the same
class of issue as a public API: adding a method is additive, but changing an
existing method's behavior is breaking. The spec should acknowledge that
published artifacts (generated OpenAPI specs, published error code mappings,
shipped wire formats) are compatibility contracts, not internal
implementation details.

---

## Summary Statistics

| Severity | Count |
|----------|-------|
| Critical | 13 |
| Warning | 21 |
| Suggestion | 12 |

## Recommended Resolution Order

The findings cluster into a few resolution passes. Recommended order:

### Pass 1: Governance reconciliation (resolves C1, C2, C12, C11)

1. Advance ADR-022 and ADR-023 to **Accepted**.
2. Amend ADR-015: mark Decision 3 and Assumption 6 as superseded by ADR-022;
   update the struct to `CompositionAuthority`.
3. Amend ADR-002: note the signature was revised by ADR-007.
4. Amend ADR-004: note the "enrich/replace" language was superseded by
   ADR-011's immutability model.
5. Add a Status column to all crate README ADR tables.

This is the highest-value pass — it resolves the direct contradictions in the
document set and makes the governance state coherent.

### Pass 2: Spec-ADR consistency (resolves C3, C4, C5, C6, C10, C13, W1)

6. Add `abort_policy` to `OperationContext`; define `AbortPolicy` enum (C3).
7. Define the `OperationEnv` trait explicitly, including
   `invoke_with_policy()` (C4).
8. Separate `OperationContext.env` (dispatch trait) from
   `OperationContext.scoped_env` (reachability set) — resolve the type
   identity crisis (C6).
9. Update ADR-017's `from_call` mirror list and `OperationAdapter` trait to
   include `error_schemas` and `HandlerRegistration` (C5, W1).
10. Specify `From<StreamError> for HandlerError` and resolve the variant
    payload (C10).
11. Specify the `Connection::set_identity` observability read path (C13).

### Pass 3: Vault security and deferral reconciliation (resolves C7, C8, C9, W6, W7, W8, W9, W10, W12)

12. Promote OQ-21 from "deferred" to "open"; reconcile with ADR-019; decide
    the vault-server-crate question (C7).
13. Complete the operation access policy table (C8).
14. Specify the `site_password_path` string→u32 mapping (C9).
15. Add an ADR for the HD-derivation key model (identity/SSH/signing) (W9).
16. Add an ADR locking the `EncryptedData` wire format (W10).
17. Add a "Known Source Drift" table to the vault README (W12).
18. Address `unlock_new` returning non-zeroizing `String` (W7) and
    `DerivedKey` JSON deserialization (W8).
19. Clarify the salt field's "reserved" semantics (W6).

### Pass 4: Two-way-door guard clauses (resolves W2, W3, W4, W5, W17, W18, W21)

20. Add guard clauses to the OQs and ADRs that ADR-022 narrowed (W2, W3, W4,
    W21).
21. Add the "published artifact is a contract" guard to `to_*` adapters (W5).
22. Correct OQ-09 (WASM server-side) and OQ-10 (git composability) framings
    (W17, W18).

### Pass 5: Minor consistency (resolves remaining Warnings and Suggestions)

23. Fix the overview.md "stream" → "connection" wording (W16).
24. Fix ADR-016's self-referential citation and grandchild propagation (W19).
25. Fix ADR-023's `from_openapi` collision example (W20).
26. Address the remaining suggestions (S1–S12).

---

## Note on Effort

This review found more issues than review #001 (13 critical vs 5, 21 warning
vs 4), but that's expected: review #001 caught the registration-bundle tangle
and the missing error schemas, which were the highest-density gaps. This
review goes wider — cross-document consistency, the two-way-door framing, and
the vault deferrals — and surfaces issues that were always there but not yet
audited. The critical findings here are individually smaller than review
#001's C1–C5 (no single tangle of four interlocking gaps), but they are
collectively important because several are governance issues (Proposed ADRs
treated as binding, Accepted ADRs contradicting later refinements) that
undermine the document set's authority.

The two-way-door audit's core finding: the framework is sound, but its
application is drifting because later ADRs don't update earlier OQ
classifications. The fix is not to re-litigate each door — it's to add guard
clauses documenting the narrowed scope, so the OQ tracker reflects
post-ADR-022 reality.