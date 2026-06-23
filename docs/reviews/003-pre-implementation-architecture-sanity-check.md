---
status: resolved
last_updated: 2026-06-23
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
  - decisions/001 through 026 (all 26 ADRs)
reviewer: architect
---

# Architecture Gap Review #003

## Purpose

This is the third pre-implementation sanity check. Reviews #001 and #002
caught the registration-bundle tangle (C1–C4), the missing error schemas (C5),
the governance reconciliation (Proposed ADRs treated as binding, Accepted ADRs
contradicting later refinements), the abort-policy wiring gap, and the vault
deferral audit. All of those are resolved.

This review goes deeper on a different axis: **type and API surface
completeness**. The prior reviews focused on cross-document *decisions*
(contradictions between ADRs, stale framing). This review asks: could two
implementers, working from these specs alone, produce compatible code? The
findings are mostly places where a type or API is *referenced* but never
*defined*, or where a code sketch in an ADR went stale after the spec docs
were updated.

The two themes:

1. **Stale ADR sketches**: ADR-022's `build_root_context` and `invoke()`
   sketches were written before ADR-024 split `env` into `scoped_env` + `env`
   and before ADR-016's abort policy was added to `OperationContext`. The spec
   docs (call-protocol.md, operation-registry.md) were updated; the ADR
   sketches were not. An implementer copying the ADR verbatim won't compile.

2. **Referenced-but-undefined types**: `Capabilities`, `SessionOverlaySource`,
   `CachedKey`, `CallConnection`, `Secp256k1ExtendedPrivKey` — all referenced
   in specs, none defined. These are exactly the boundary types where two
   implementers must agree, and the spec leaves them as exercises.

## Severity Definitions

| Severity | Meaning |
|----------|---------|
| **Critical** | Must resolve before implementation — would cause divergent implementations or security issues |
| **Warning** | Should resolve — missing specifications that could cause confusion or rework |
| **Suggestion** | Consider — improvements for clarity or completeness |

---

## Critical Findings

### C1. `DerivedKey`: `#[derive(Deserialize)]` contradicts "custom Deserialize rejects `[REDACTED]`"

**Documents**: crates/vault/protocol.md (L29, L68),
decisions/025-vault-local-only-dispatch.md (L137)

**Problem**: `protocol.md:29` shows `#[derive(Zeroize, Deserialize)]` on
`DerivedKey`. But `protocol.md:68` and ADR-025:137 both require a **custom**
`Deserialize` that explicitly rejects `private_key == "[REDACTED]"` with an
error. These are mutually exclusive: `#[derive(Deserialize)]` generates a
default impl, and you cannot also write a manual `impl Deserialize` (conflicting
impls). With the derived impl, a redacted string would fail only incidentally
(serde type mismatch: string vs sequence), producing a generic error — not the
explicit redaction-rejection the spec describes.

**Resolution**: Dropped `Deserialize` from the derive list in protocol.md and
showed a separate manual `impl<'de> Deserialize<'de> for DerivedKey` that
checks for `"[REDACTED]"` and returns a clear error.

---

### C2. `encrypt` prose says "derived at `PATHS::ENCRYPTION`" but signature takes `key_version`

**Documents**: crates/vault/service.md (L175),
decisions/021-key-rotation-via-version-indexed-paths.md (L115–122)

**Problem**: `service.md:172` signature: `encrypt(&self, plaintext, key_version)`.
`service.md:175` prose: "Encrypt plaintext using the encryption key derived at
`PATHS::ENCRYPTION`." `PATHS::ENCRYPTION` is the v2 path only. ADR-021 makes
`encrypt` derive at `encryption_path_for_version(key_version)`. An implementer
following the prose stamps `key_version: 3` but uses the v2 key — a rotation bug
producing mislabeled blobs that decrypt fine locally but are cryptographically
mislabeled.

**Resolution**: Updated service.md:175 to "Encrypt plaintext using the
encryption key derived at `encryption_path_for_version(key_version)` (ADR-021);
the same `key_version` is stamped on the resulting `EncryptedData`."

---

### C3. `derive_encryption_key(path)` returns `DerivedKey` but `derive_encryption_key_for_version(version)` returns `EncryptionKey`

**Documents**: crates/vault/service.md (L133, L143),
decisions/021-... (L104–122)

**Problem**: Same cache, different return types. No conversion between them is
specified, and `CachedKey` (the cache value type) is never defined. An
implementer cannot reconcile the cache.

**Resolution**: Made `derive_encryption_key_for_version` return `DerivedKey`
(consistent with `derive_encryption_key`), and `encrypt`/`decrypt` extract the
`EncryptionKey` from the cached `DerivedKey` via `EncryptionKey::from_derived_bytes`.
Defined `CachedKey` as a `#[derive(Zeroize, ZeroizeOnDrop)]` struct wrapping a
`DerivedKey` with cache metadata.

---

### C4. `tokio` vs `std::sync::RwLock` contradiction

**Documents**: decisions/018 (L69, L155–156), decisions/025 (L166–171),
crates/vault/service.md (L27–33)

**Problem**: ADR-018:155 says "`std::sync::RwLock` works"; ADR-018:69 and
ADR-025:166 list `tokio` as a retained dep "for RwLock sync primitives";
service.md shows `RwLock` without specifying which. All vault methods are sync
(no `async`), implying `std::sync::RwLock` — which means tokio isn't needed,
contradicting both ADR dependency lists. ADR-018:155 still references the
removed actor's `tokio::sync::mpsc`.

**Resolution**: Specified `std::sync::RwLock` in service.md. Dropped `tokio`
from retained-deps lists in ADR-018 and ADR-025. Removed the stale actor/mpsc
sentence in ADR-018.

---

### C5. Missing drift rows in vault README drift table

**Documents**: crates/vault/README.md (L121–130),
decisions/021-... (L126–128), crates/vault/encryption.md (L185–188)

**Problem**: The README drift table (the stated "single source of truth") omits
two drift items: (a) `decrypt`/`encrypt` ignore `key_version` and always derive
at `PATHS::ENCRYPTION`; (b) `rotate` is not implemented. An implementer using
only the table would miss these.

**Resolution**: Added drift rows #9 (key_version ignored) and #10 (rotate not
implemented) to the README table.

---

### C6. ADR-022's `build_root_context` and `invoke()` omit `abort_policy`

**Documents**: decisions/022 (L344–358, L382–394),
crates/call/call-protocol.md (L296–318),
crates/call/operation-registry.md (L309–336)

**Problem**: ADR-022's sketches have 9 fields; the spec docs have 10 (including
`abort_policy`). An implementer copying ADR-022 verbatim won't compile against
the `OperationContext` struct.

**Resolution**: Added `abort_policy: AbortPolicy::default(),` to ADR-022's
`build_root_context` and `abort_policy: parent.abort_policy.clone(),` to its
`invoke()` sketch. Restated the `env` type as `Arc<dyn OperationEnv + Send +
Sync>` in both sketches.

---

### C7. `Capabilities` type referenced widely but defined nowhere

**Documents**: overview.md (L168), crates/call/operation-registry.md (L116,
L206, L455, L473, L507–513), crates/call/call-protocol.md (L305)

**Problem**: `Capabilities` appears in `OperationContext`, `HandlerRegistration`,
`build_root_context`, and the capability injection section — but has no struct
definition, no trait bounds, no builder API, no module location anywhere. The
spec asserts `Clone`, "non-serializable", "zeroized", "immutable after
construction" in prose only. The core and call implementers will produce
incompatible types.

**Resolution**: Added a `Capabilities` struct definition to core-types.md with
`Clone + Send + Sync`, `Zeroize`/`ZeroizeOnDrop`, a sealed builder API
(`new()`, `with_api_key()`, `with_http_token()`), private fields (immutability
guard), and no `Serialize` derive. Cross-referenced from operation-registry.md.

---

### C8. `SessionOverlaySource` referenced on `CallAdapter` but never defined; crate violation

**Documents**: crates/call/call-protocol.md (L41),
decisions/024-... (L447–456)

**Problem**: `CallAdapter` (alknet-call) has a field of type
`Arc<dyn SessionOverlaySource>`, but ADR-024 says the trait is "an agent-crate
concern." That's a crate-graph violation: alknet-call can't hold a field whose
type is defined in alknet-agent (agent depends on call, not reverse).

**Resolution**: Defined `SessionOverlaySource` as a trait in alknet-call
(alongside `OperationEnv`), since the `CallAdapter` must name it. alknet-agent
implements it; alknet-call defines the integration point. This matches the
`IdentityProvider` pattern (ADR-004: core defines the trait, handlers implement
it).

---

### C9. `CompositeOperationEnv` dispatch fall-through semantics unspecified

**Documents**: crates/call/operation-registry.md (L348–387)

**Problem**: The `OperationEnv::invoke` signature returns `ResponseEnvelope` —
no "not in this overlay" sentinel. The sketch says "returns a sentinel or the
composite continues" and "the exact mechanism is a two-way door" — but this
affects cross-impl interop. Two implementers picking different mechanisms
(sentinel vs `contains` check) will produce `OperationEnv` impls that don't
compose.

**Resolution**: Added a `contains(&self, name: &str) -> bool` method to the
`OperationEnv` trait (with a default impl that returns `true` for backward
compat). The composite env probes `contains()` before dispatching, eliminating
the sentinel ambiguity. `CompositeOperationEnv::invoke` now checks
`session.contains()` → `connection.contains()` → base, dispatching to the first
overlay that contains the op.

---

### C10. No API for Layer 2 (connection overlay) registration; `CallConnection` undefined

**Documents**: crates/call/operation-registry.md (L214–232, L456–480),
decisions/024-... (L252–263)

**Problem**: `OperationRegistryBuilder` only builds Layer 0. `from_call` imports
go into the connection's overlay at runtime, but there's no registration API, no
`CallConnection` struct definition, and no spec for how the overlay becomes part
of `CompositeOperationEnv.connection`.

**Resolution**: Added a `CallConnection` struct definition (overlay-related
fields) and a `register_imported(registration)` / `imported_operations` API to
call-protocol.md. Clarified that `OperationRegistryBuilder` is Layer-0-only;
runtime overlay registration uses `CallConnection` methods.

---

### C11. `with_local` signature diverges between two examples in operation-registry.md

**Documents**: crates/call/operation-registry.md (L217–230, L458–478)

**Problem**: The first example shows `with_local(spec, handler, authority,
scoped_env)` — 4 args, no capabilities. The second uses
`.with(HandlerRegistration { ... capabilities })`. `HandlerRegistration.capabilities`
is required (not `Option`). The first example silently drops capabilities for
the agent handler.

**Resolution**: Added `capabilities` as the 5th arg to `with_local` in the first
example, making both examples consistent. Documented the `with_local` and
`with_leaf` signatures explicitly.

---

## Warning Findings

### W1. `invoke_with_policy` has no default impl and can't delegate to `invoke()`

**Documents**: crates/call/operation-registry.md (L265–273, L340–342)

**Problem**: `invoke_with_policy` is a required trait method with no default
impl. `invoke()` hardcodes `abort_policy: parent.abort_policy.clone()`, so
`invoke_with_policy` can't delegate to it — every `OperationEnv` impl must
duplicate the full body.

**Resolution**: Restructured the trait: `invoke_with_policy` is the required
method; `invoke` has a default impl that calls `invoke_with_policy` with
`parent.abort_policy.clone()`. Impls only write `invoke_with_policy`. Added the
`CompositeOperationEnv::invoke_with_policy` body.

---

### W2. `CachedKey` referenced 7+ times but never defined

**Documents**: crates/vault/service.md (L216, L231, L330–336, L351–352),
crates/vault/protocol.md (L114), crates/vault/README.md (L99, L128)

**Resolution**: Added a `CachedKey` struct definition to service.md's Cache
section, showing fields (`key: DerivedKey`, `cached_at: Instant`,
`last_accessed: Instant`) and `Zeroize`/`ZeroizeOnDrop` derives.

---

### W3. `EncryptionKey` constructor + derivation glue unspecified; missing from re-export

**Documents**: crates/vault/encryption.md (L60–78),
crates/vault/service.md (L140–155), crates/vault/README.md (L144)

**Resolution**: Added `from_derived_bytes` pseudocode wiring SLIP-0010 output to
`EncryptionKey`. Added `EncryptionKey` to README re-export list. Specified
trait derives (custom redacting `Debug`, no `Clone`, `Zeroize`/`ZeroizeOnDrop`).

---

### W4. `Secp256k1ExtendedPrivKey` never defined; `derive_ethereum_key` glue unspecified

**Documents**: crates/vault/service.md (L157–165),
crates/vault/mnemonic-derivation.md (L152–164)

**Resolution**: Added `Secp256k1ExtendedPrivKey` struct definition to
mnemonic-derivation.md (parallel to `ExtendedPrivKey`). Documented
`derive_ethereum_key` → `derive_secp256k1_path` → wrap into `DerivedKey` glue.

---

### W5. `encryption_path_for_version(v<2)` collides onto v2 path

**Documents**: decisions/021-... (L78–84),
crates/vault/mnemonic-derivation.md (L203), crates/vault/service.md (L147–148)

**Problem**: `saturating_sub(2)` maps v0/v1/v2 to the same path. v1 is TS PBKDF2
(undecryptable by design), but `derive_encryption_key_for_version(1)` silently
returns the v2 key.

**Resolution**: Added a guard: `encryption_path_for_version` and
`derive_encryption_key_for_version` return `VaultServiceError::InvalidPath` for
`version < 2`. Documented in mnemonic-derivation.md and service.md.

---

### W6. `ResponseEnvelope` → `EventEnvelope` conversion unspecified; payload schemas missing

**Documents**: crates/call/call-protocol.md (L80–85, L331–341)

**Resolution**: Added a "Wire Payload Schemas" section to call-protocol.md
showing the `payload` shape for `call.responded`, `call.completed`, and
`call.aborted`. Documented the `ResponseEnvelope` → `EventEnvelope` mapping.

---

### W7. Timeout location and composition propagation unspecified

**Documents**: crates/call/call-protocol.md (L352)

**Resolution**: Added a timeout section to call-protocol.md specifying: timeout
duration is on `CallAdapter` (constructor param); composed calls inherit the
parent's remaining deadline via a `deadline: Option<Instant>` on
`OperationContext`; composed calls that exceed the deadline are cancelled (future
dropped). Added a cross-reference from `OperationContext`.

---

### W8. `generate_request_id()` unspecified; wire ID vs internal ID relationship undefined

**Documents**: crates/call/operation-registry.md (L315, L167),
crates/call/call-protocol.md (L291)

**Resolution**: Added a "Request ID Generation" subsection to
operation-registry.md specifying UUID v4 generation for composed calls, that wire
`call.requested.id` becomes the root `request_id`, and that composed child IDs
are internal (appear in `PendingRequestMap` for abort indexing but are not sent
as `call.requested` unless the child is a `from_call` op).

---

### W9. `unlock_new` already-unlocked behavior unspecified

**Documents**: crates/vault/service.md (L74–93)

**Resolution**: Added: "`unlock_new` returns `AlreadyUnlocked` if the vault is
already unlocked, matching `unlock`'s behavior."

---

### W10. `KeyType`'s `Serialize`/`Deserialize` justification stale post-ADR-025

**Documents**: crates/vault/protocol.md (L115)

**Resolution**: Removed the stale "part of the irpc protocol" justification.
`KeyType` derives `Serialize`/`Deserialize` for `EncryptedData` interop (future
use) and `Clone` because it's a non-secret tag; the derives are retained but the
justification is corrected.

---

### W11. `OperationProvenance` and `CompositionAuthority` defined only in ADR-022

**Documents**: crates/call/operation-registry.md (L202, L209, L468),
decisions/022-... (L116–131, L165–181)

**Resolution**: Added inline definitions of `OperationProvenance` and
`CompositionAuthority` to operation-registry.md, with "See ADR-022 for
rationale" cross-references.

---

### W12. `encrypt`/`decrypt` free functions vs `VaultServiceHandle` methods relationship unstated

**Documents**: crates/vault/encryption.md (L132–146),
crates/vault/service.md (L169–190)

**Resolution**: Added a note to encryption.md that the free `encrypt`/`decrypt`
functions are module-internal crypto helpers; `VaultServiceHandle::encrypt`/
`decrypt` are the public API and call through to them.

---

### W13. `rotate` placement in encryption.md (method signature in crypto doc)

**Documents**: crates/vault/encryption.md (L177–179)

**Resolution**: Replaced the `rotate` signature block in encryption.md with a
prose pointer to service.md and ADR-021 (rotate is a `VaultServiceHandle`
method, not a free function).

---

### W14. `CallAdapter::new()` missing session_source param

**Documents**: crates/call/operation-registry.md (L480),
crates/call/call-protocol.md (L35–42)

**Resolution**: Added `CallAdapter::new(registry, identity_provider)` (defaults
`session_source` to `None`) and `CallAdapter::with_session_source(source)`
builder method to call-protocol.md.

---

## Suggestions

### S1. `Seed: Clone` duplicates root of trust

**Documents**: crates/vault/mnemonic-derivation.md (L84–88)

**Resolution**: Kept `Clone` on `Seed` (derivation functions take `&[u8]`, so
`Clone` is not strictly needed, but the `to_seed` path and cache rebuild may
benefit). Added a note that callers should prefer `&Seed` and avoid cloning.

---

### S2. `VaultServiceInner.unlocked: bool` redundant with `seed: Option<Seed>`

**Documents**: crates/vault/service.md (L35–41)

**Resolution**: Added an invariant note: `unlocked` is `true` iff
`seed.is_some()`; `is_unlocked()` reads `unlocked` for cheap checks, but the
ground truth is `seed.is_some()`.

---

### S3. `ExtendedPrivKey`/`Mnemonic` accessor signatures incomplete

**Documents**: crates/vault/mnemonic-derivation.md (L58–63, L138–150)

**Resolution**: Added accessor signatures for `ExtendedPrivKey`
(`private_key()`, `public_key()`, `chain_code()`, `path()`).

---

### S4. `CURRENT_KEY_VERSION` module location unspecified

**Documents**: decisions/021-... (L159), crates/vault/encryption.md (L154)

**Resolution**: Added: `CURRENT_KEY_VERSION` lives in `encryption.rs` and is
re-exported from the crate root.

---

### S5. ADR-018 stale actor reference

**Documents**: decisions/018 (L155–156)

**Resolution**: Removed the stale actor/mpsc sentence (addressed in C4).

---

### S6. `device_path`/`encryption_path_for_version`/`derive_path_from_seed` not in re-export list

**Documents**: crates/vault/README.md (L136–154)

**Resolution**: Added a note that derivation helpers are accessible as
`alknet_vault::derivation::*`.

---

## Summary Statistics

| Severity | Count | Status |
|----------|-------|--------|
| Critical | 11 | All resolved (C1–C11) |
| Warning | 14 | All resolved (W1–W14) |
| Suggestion | 6 | All resolved (S1–S6) |

## Recommended Resolution Order

All findings resolved. Resolution summary:

1. **Vault type/reconciliation pass** (C1–C5, W2–W5, W9–W13, S1–S4): Fixed
   `DerivedKey` derive contradiction, `encrypt` prose, return-type divergence,
   RwLock contradiction, drift table, `CachedKey`/`EncryptionKey`/
   `Secp256k1ExtendedPrivKey` definitions, `encryption_path_for_version`
   guard, `unlock_new` behavior, `KeyType` justification, free-vs-method
   relationship, `rotate` placement.
2. **Core+call type/API pass** (C6–C11, W1, W6–W8, W14, S5–S6): Fixed
   ADR-022 stale sketches, defined `Capabilities`, `SessionOverlaySource`,
   `CallConnection`, Layer 2 registration API, `CompositeOperationEnv`
   dispatch, `with_local` signature, `invoke_with_policy` default impl,
   payload schemas, timeout, request ID generation, `CallAdapter` constructor.
3. **Inline definitions** (W11): Added `OperationProvenance` and
   `CompositionAuthority` inline in operation-registry.md.

Review #003 is complete. All 11 critical, 14 warning, and 6 suggestion findings
resolved.