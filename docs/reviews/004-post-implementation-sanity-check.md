---
status: resolved
last_updated: 2026-06-24
reviewed_artifacts:
  - crates/alknet-vault/src/{lib,cache,derivation,encryption,ethereum,mnemonic,protocol,service}.rs
  - crates/alknet-core/src/{lib,auth,config,endpoint,types}.rs
  - crates/alknet-call/src/{lib,registry/*,protocol/*}.rs
  - docs/architecture/{overview,open-questions}.md
  - docs/architecture/crates/{vault,core,call}/*.md
  - docs/architecture/decisions/001–026
  - docs/reviews/001,002,003-*.md
  - tasks/{vault,core,call}/**/*.md (28 completed tasks)
reviewer: code-reviewer (post-implementation)
---

# Post-Implementation Sanity Check #004

## Purpose

Sanity check after the 28-task implementation round across the three greenfield
crates (`alknet-vault`, `alknet-core`, `alknet-call`). Reviews #001–#003 were
*pre*-implementation and caught spec-level gaps (type-surface completeness,
registration-bundle tangle, abort-policy wiring, vault deferral audit). This
review is *post*-implementation: it asks whether the code matches the now-stable
specs, and whether anything obvious (or not so obvious) slipped through.

Headline result: **the implementation is in remarkably good shape.** It builds
clean (`cargo build --workspace --all-features`), all 332 tests pass, and
`cargo clippy --workspace --all-features --all-targets` reports zero default-
level warnings. The spec drift that remains is concentrated in two places:
(a) one wiring gap where implemented-and-tested code is never called, and (b)
two minor behaviors the spec describes but the implementation shortcuts. None
of the findings block forward progress; all are local fixes.

## Severity Definitions

| Severity | Meaning |
|----------|---------|
| **Critical** | Security or correctness defect that will misbehave in production |
| **Warning** | Spec drift that works today but will surprise a future implementer or breaks an invariant |
| **Suggestion** | Hardening, ergonomics, or test-coverage gaps |

## Methodology

- Read every source file in the three crates (~10.3k LOC).
- Cross-referenced each task's acceptance criteria against the implemented code.
- Cross-referenced each ADR's binding decisions against the implementation.
- Ran `cargo build`, `cargo test`, `cargo clippy` (default + `-W clippy::pedantic`).
- Walked the dispatch paths end-to-end: ALPN handshake → endpoint dispatch →
  handler → registry invoke → env compose → child invoke → wire response.
- Traced the secret-material flow: vault unlock → derive → cache → encrypt →
  `DerivedKey`/`EncryptionKey` zeroization on drop.

## Summary Statistics

| Severity | Count |
|----------|-------|
| Critical | 0 |
| Warning  | 4 (W1–W4) |
| Suggestion | 5 (S1–S5) |

---

## Warning Findings

### W1. `AbortCascade` is implemented and tested but never invoked by `CallAdapter`

**Files**: `crates/alknet-call/src/protocol/abort.rs` (full impl + 20 tests),
`crates/alknet-call/src/protocol/adapter.rs:200–228` (`handle_stream`),
`crates/alknet-call/src/protocol/adapter.rs:261–301` (`handle`)

**Problem**: `AbortCascade::cascade_abort` exists with full tree-walking logic
for both `AbortDependents` and `ContinueRunning`, and 20 unit tests cover depth-
3 cascades, mixed Call/Subscribe entries, and the "unknown root silently
discarded" rule from ADR-016. But no caller invokes it.

In `CallAdapter::handle_stream` (adapter.rs:210–213), the only inbound event
type dispatched is `EVENT_REQUESTED`:

```rust
if envelope.r#type != EVENT_REQUESTED {
    debug!(event_type = %envelope.r#type, id = %envelope.id, "ignoring non-requested event on inbound stream");
    continue;
}
```

In `CallAdapter::handle` (adapter.rs:261–301), the only abort-related behavior
is `pending.lock().fail_all(...)` on connection close — which fails every
pending wire entry with `INTERNAL` but does not walk the call tree. There is no
path where an inbound `call.aborted` event for a parent request triggers
`AbortCascade::cascade_abort`.

**Spec contradiction**: `docs/architecture/crates/call/call-protocol.md:497`:

> "When `call.aborted` arrives for a parent request, the protocol cascades the
> abort to all non-terminal descendants in the tree. The CallAdapter walks the
> tree (indexed by `parent_request_id` in `PendingRequestMap`) and sends
> `call.aborted` for each descendant."

The implementation does not do this. A wire client that sends `call.aborted`
to cancel its in-flight root request gets the event silently dropped
server-side; the root's composed descendants continue running to completion
under the default `AbortDependents` policy — exactly the wasted-work / unwanted-
side-effects case ADR-016 was written to prevent.

**Why the tests pass anyway**: `AbortCascade`'s unit tests construct a
`PendingRequestMap` directly and call `cascade_abort` on it. They never go
through `CallAdapter`. The call-adapter task's acceptance criteria (tasks/call/
protocol/call-adapter.md:213–238) did not include "inbound `call.aborted`
triggers cascade" — only outgoing aborts via `CallConnection::abort()` and the
abort-cascade logic itself. The abort-cascade task's acceptance criteria
(tasks/call/protocol/abort-cascade.md:154–172) likewise test `AbortCascade` in
isolation. The integration step — wiring the cascade into the adapter's inbound
event path — is in neither task's acceptance criteria, and so was never done.

This is the classic "two correctly-implemented halves with no bolt between
them" pattern. Each half passes its own tests; the integration is untested and
absent.

**Impact**: Today, with no `from_call` ops and no LLM agent driving nested
composition, the gap is mostly latent — most call trees are depth-1 (single
`call.requested` → single handler → response). The cascade becomes load-
bearing the moment a handler composes other operations and a client aborts the
root mid-flight (e.g., user cancels an agent chat → `/agent/chat` handler has
already invoked `/fs/readFile` and `/bash/exec` → those keep running, possibly
with side effects, after the user-visible cancellation). That is the precise
scenario ADR-016's "aborted parent work has no consumer waiting for results —
continuing is wasted work at best and unwanted side effects at worst" calls
out.

**Resolution**: In `CallAdapter::handle_stream`, add a branch for
`EVENT_ABORTED` (and possibly `EVENT_COMPLETED`/`EVENT_ERROR` for client-
initiated close on subscriptions) that:

1. Looks up the request_id in the connection's `PendingRequestMap`.
2. If present, constructs `AbortCascade::new(&mut pending)` and calls
   `cascade_abort(&request_id, AbortPolicy::AbortDependents)` (the wire caller
   doesn't get to choose the policy — the root's policy is `AbortDependents`
   per ADR-016 Decision 6; `ContinueRunning` is a handler-level opt-in for
   children, not a wire field).
3. Optionally sends `call.aborted` back for each cascaded descendant ID, if the
   protocol intends the cascade to be wire-visible (the spec is ambiguous
   here — call-protocol.md:497 says "sends `call.aborted` for each descendant";
   abort-cascade.md:68–74 says "Composed child request_ids are internal — the
   client only sees `call.aborted` for the root ID it sent". The latter is the
   stronger statement and matches ADR-016. Pick one and document it.)

Then add an integration test that opens a stub connection, registers a parent
op whose handler invokes a child op via `env.invoke()`, sends `call.aborted`
for the parent's request_id on an inbound stream, and asserts the child's
`PendingRequestMap` entry is removed. This is the test that would have caught
the gap.

---

### W2. Endpoint never extracts TLS client certificate fingerprint

**Files**: `crates/alknet-core/src/endpoint.rs:306` (`dispatch_quinn`),
`crates/alknet-core/src/endpoint.rs:396` (`dispatch_iroh`),
`crates/alknet-core/src/endpoint.rs:405–421` (`build_auth_context`)

**Problem**: Both dispatch functions hard-code `tls_client_fingerprint: None`
when calling `build_auth_context`:

```rust
let auth = build_auth_context(&alpn, remote_addr, None, identity_provider);
//                                                           ^^^^
```

So `AuthContext.tls_client_fingerprint` is always `None`, and
`AuthContext.identity` (the endpoint-resolved identity) is always `None` too —
because `build_auth_context` only attempts `resolve_from_fingerprint` when the
fingerprint is `Some`. The endpoint-level auth resolution path described in
`docs/architecture/crates/core/auth.md:159–171` is non-functional:

> "QUIC connection arrives → TLS handshake (ALPN negotiation) → Extract TLS
> client certificate fingerprint (if presented) → If fingerprint present:
> IdentityProvider::resolve_from_fingerprint() → Some(identity):
> auth.identity = Some(identity) → Construct AuthContext { identity, alpn,
> remote_addr, tls_client_fingerprint }"

The implementation constructs the AuthContext but with `identity: None` and
`tls_client_fingerprint: None` regardless of whether the TLS peer presented a
client certificate. All identity resolution is deferred to handler-level code
(the `auth_token` path in `CallAdapter::resolve_identity`, or the future SSH
handler's key-fingerprint path).

**Task summary mismatch**: `tasks/core/endpoint.md:252` claims "AuthContext
construction from connection (alpn/remote_addr/fingerprint/identity)" as a
completed deliverable. The fingerprint step is not actually implemented —
`None` is passed in both dispatch sites. The task's acceptance criteria
(tasks/core/endpoint.md:213–238 — not directly quoted above, but the same
file) likewise lists fingerprint extraction as an acceptance criterion.

**Why this matters**: For SSH and git handlers, this is fine — they do their
own key-fingerprint extraction inside `handle()`. For P2P nodes using RFC 7250
raw Ed25519 keys (the "default for most alknet nodes" per OQ-12), the
connection-level identity *is* the TLS client cert — there's no separate
protocol-level credential to extract. Without endpoint-level fingerprint
extraction, a raw-key peer connecting to an `alknet/call` endpoint cannot be
identified by fingerprint at the endpoint layer; the call protocol's
`auth_token` field becomes the *only* identity path, which defeats the
"hybrid resolution" design from ADR-004/OQ-02.

**Resolution**: In `dispatch_quinn`, extract the client cert from the rustls
session via `connection.handshake_data()` →
`quinn::crypto::rustls::HandshakeData` (which already has `downcast::<...>()`
in `extract_quinn_alpn` at endpoint.rs:316–326). The `HandshakeData` struct
exposes `peer_subjective_name`/`client_cert_chain`-equivalent fields; with
`with_no_client_auth()` currently set, no client cert is requested, so this
also requires deciding whether the server config should request client certs
(two-way door — `with_client_auth()` vs `with_no_client_auth()`). For iroh,
`connecting.lms()` / the connection's `tls::Lts` exposes the peer's `NodeId`,
which *is* the raw-key fingerprint in iroh's model. At minimum, extract *some*
fingerprint when one is available; passing `None` unconditionally is the gap.

If client cert extraction is deferred (a defensible call — ACME and full
client-auth are also deferred per the task summary), update the task summary
and the spec to say so explicitly. Right now the spec says "extracted", the
summary says "constructed from fingerprint", and the code says `None`. Pick
two.

---

### W3. `Mnemonic: #[derive(Debug)]` prints the seed phrase

**File**: `crates/alknet-vault/src/mnemonic.rs:35`

**Problem**: `Mnemonic` derives `Debug`:

```rust
#[derive(Debug)]
pub struct Mnemonic {
    inner: Bip39Mnemonic,
    phrase: String,
}
```

The derived `Debug` prints both fields, including `phrase: "abandon abandon
abandon ..."`. The crate's own module doc (mnemonic.rs:8) says "Seed material
is protected with `Zeroize`" and the `Mnemonic::phrase()` accessor (line 82)
warns "Handle with care — this is the root of trust for all derived keys." The
`Zeroize + Drop` impls (lines 88–99) wipe the phrase from memory on drop — but
`Debug` will happily hand the phrase to any `tracing::debug!`, `format!("{:?}")`,
panic backtrace, or error-context printer that touches a `Mnemonic` value.

This is inconsistent with the rest of the crate's secret handling:
- `DerivedKey` has a custom redacting `Debug` (protocol.rs:48–56) that prints
  `private_key: "[REDACTED]"`.
- `EncryptionKey` has a custom redacting `Debug` (encryption.rs:132–139).
- `Capabilities` has a redacting `Debug` (types.rs:112–118).
- `Secret<T>` has a redacting `Debug` (types.rs:51–55).
- `Mnemonic` ... derives `Debug` and prints the phrase.

**Impact**: Low today (no `tracing::debug!` site currently formats a
`Mnemonic`), but this is exactly the kind of latent secret-leak that bites
later — a future debugging session adds `tracing::debug!(?mnemonic, ...)`, the
phrase lands in a log file, the log file gets attached to a bug report. The
root of trust should never have a `Debug` impl that can print it.

**Resolution**: Replace `#[derive(Debug)]` with a manual impl that redacts,
matching the pattern used for `DerivedKey`:

```rust
impl std::fmt::Debug for Mnemonic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Mnemonic")
            .field("phrase", &"[REDACTED]")
            .finish()
    }
}
```

Add a test asserting `format!("{:?}", mnemonic)` does not contain any word
from the phrase. Same treatment for `Seed` if it doesn't already redact (it
derives `Zeroize` but I didn't check its `Debug` — verify).

---

### W4. `AuthPolicy::resolve_api_key` does not populate `Identity.resources`

**Files**: `crates/alknet-core/src/config.rs:81–118` (`resolve_api_key`),
`crates/alknet-core/src/auth.rs:15–19` (`Identity`)

**Problem**: `resolve_api_key` constructs the resolved `Identity` with
`resources: std::collections::HashMap::new()` (config.rs:116) — resources are
always empty, regardless of what the API key entry grants. The spec
(docs/architecture/crates/core/auth.md:153) says:

> "Token: Parse as UTF-8. If it starts with `alk_`, look up in
> `DynamicConfig::auth::api_keys` by prefix match + SHA-256 hash. If found and
> not expired, return `Identity { id: prefix, scopes: entry.scopes,
> resources: entry.resources }`."

The spec references `entry.resources`, but `ApiKeyEntry` (config.rs:55–62)
has no `resources` field — only `prefix, hash, scopes, description, expires_at`.
So the spec's `entry.resources` cannot be implemented as written; either the
spec is ahead of the type, or the type is behind the spec.

**Impact**: Today, no operation in the workspace uses resource-based ACLs
against an API-key-resolved identity (the `AccessControl::resource_type`/
`resource_action` fields exist in spec.rs:32–37 and are tested in spec.rs:284–
303, but always with hand-constructed `Identity` resources — never with a
token-resolved identity). So the gap is latent. The moment an operation
declares a resource-scoped ACL and a caller authenticates via API key, the ACL
check will fail with "missing resource" even if the key was granted that
resource in config — because `resources` is always empty.

**Resolution**: Pick one:

(a) **Add `resources: HashMap<String, Vec<String>>` to `ApiKeyEntry`**, update
`resolve_api_key` to populate `Identity.resources` from `entry.resources`, and
update `AuthPolicy`'s TOML schema (when one exists) to parse it. This matches
the spec.

(b) **Drop `entry.resources` from the spec**, document that API keys grant
scopes only (not resources), and update `auth.md:153` accordingly. Resource-
scoped access then requires fingerprint-based identity (which also currently
returns `resources: HashMap::new()` at config.rs:74 — same gap, same fix
needed there).

Either way, the current state — spec says `entry.resources`, code says `HashMap::new()`,
type has no field for it — is a three-way mismatch that will trip the next
implementer who tries to use resource ACLs with token auth.

---

## Suggestions

### S1. `EncryptionKey::from_derived_bytes` panics on short input

**File**: `crates/alknet-vault/src/encryption.rs:112–119`

`bytes[..32]` panics if `bytes.len() < 32`. Today every caller passes a 32-byte
`DerivedKey.private_key` (from SLIP-0010, which always produces 32 bytes), so
this is safe in practice. But the function is `pub`, takes `&[u8]`, and
documents itself as "the bridge from `DerivedKey` (SLIP-0010 output) to
`EncryptionKey`" — a caller passing a short slice (e.g., a future secp256k1
path that accidentally hands a 33-byte compressed pub key where a 32-byte
priv key was expected) gets an index-out-of-bounds panic from a `pub` crypto
API, which is a poor failure mode for a security-sensitive function.

Consider `u32::try_from(bytes.len())` and returning `Result<Self,
EncryptionError>` with `EncryptionError::InvalidKeyLength`, or at minimum
`bytes.get(..32).ok_or(...)` instead of `bytes[..32]`. Tests should cover the
short-input case.

---

### S2. `CallConnection::call` does not enforce the `OperationContext.deadline`

**File**: `crates/alknet-call/src/protocol/connection.rs:75–117`

`call()` registers the pending entry with `Instant::now() + DEFAULT_CALL_TIMEOUT`
(a fixed 30s), ignoring the caller's `OperationContext.deadline`. The spec
(call-protocol.md:480–489) says composed calls inherit the parent's remaining
deadline — a depth-5 composition should not get 5×30s. The deadline field is
propagated through `OperationContext` (env.rs:83 `deadline: parent.deadline`),
but `CallConnection::call` is the *client*-side path (outgoing wire call) and
doesn't receive an `OperationContext` — it builds its own timeout from a
const. This is fine for top-level client calls; it's a gap for `from_call` ops
(which forward via `CallConnection::call` from inside a handler). When
`from_call` is implemented (currently deferred, see call-connection.md:119),
the forwarding handler should pass `parent.deadline` through to the outgoing
call so a depth-N composition is bounded by the root's deadline, not N×30s.

Not a defect today (no `from_call` callers exist), but worth a TODO at the
call site so the next implementer doesn't ship the same N×30s bug.

---

### S3. `AccessControl::check` for `resource_action` without `resource_type` is surprising

**File**: `crates/alknet-call/src/registry/spec.rs:91–99`

When `resource_type` is `None` but `resource_action` is `Some(action)`, the
check scans *every* resource type in `identity.resources` looking for any list
that contains `action`. This is an odd semantics — "the caller has *some*
resource with this action, anywhere" — and is not documented in the spec. The
spec (operation-registry.md) describes resource checks as type-scoped. If the
"any resource" branch is intentional, document it; if not, require
`resource_type` whenever `resource_action` is set (or return `Allowed` when
only `resource_action` is set, matching the principle that restrictions
shouldn't be inferred). The test at spec.rs:284–303 only exercises the type-
scoped case, so the type-less branch is untested and unspecified.

---

### S4. `PendingRequestMap::handle_responded` silently drops a Subscribe entry when the channel is full

**File**: `crates/alknet-call/src/protocol/pending.rs:135–141`

When a Subscribe entry's `mpsc::Sender::try_send` returns `Full`, the entry is
removed from the map and the subscription is silently closed — the subscriber
sees the channel end but no error, just `None`. The `tracing::warn!` at
pending.rs:136 logs it server-side. The subscriber has no way to distinguish
"stream completed normally" from "you were too slow and got dropped". For
subscriptions that carry semantic state (LLM token streams, event tails),
silent drop is a poor failure mode. Consider sending an explicit
`CallError::internal("subscribe channel full")` before closing, or blocking
on `send().await` with a timeout rather than `try_send`. This is a two-way
door (the protocol doesn't forbid either behavior), but the current choice
should be a documented one, not an accident.

---

### S5. `clippy::pedantic` flags a few minor items worth addressing

**Files**: various

`cargo clippy --all-features --all-targets -- -W clippy::pedantic` (run during
this review) reports 11 unique warnings beyond default clippy:

- `clippy::cast_possible_truncation` at `wire.rs:312, 488, 533` — `body.len()
  as u32` in the frame writer. Already guarded by the `len > MAX_FRAME_SIZE`
  check at wire.rs:232, but `u32::try_from(body.len()).expect("guarded by
  MAX_FRAME_SIZE check")` (or `unwrap_or`) would make the invariant explicit
  and silence the lint.
- `clippy::unchecked_time_subtraction` at `pending.rs:556` —
  `Instant::now() - Duration::from_millis(1)` in a test. Test-only, but
  `checked_sub` is the pedantic-clean form.
- `clippy::redundant_closure_for_method_calls` at `registration.rs:307` —
  `scopes.iter().map(|s| s.to_string())` → `.map(ToString::to_string)`.
- A handful of `missing_const_fn` / `needless_pass_by_value` in tests.

None of these are correctness issues. Default `cargo clippy` (the bar the
tasks were held to) is clean. The pedantic set is offered as a one-time
cleanup if the team wants to enable `clippy::pedantic` in CI later — the
fixes are mechanical.

---

## What Was Verified and Found Correct

These are areas I specifically checked because the prior reviews or the ADRs
flagged them as high-risk, and the implementation got them right:

- **`DerivedKey` custom `Deserialize` rejects `"[REDACTED]"`** (protocol.rs:69–
  91) — exactly as review #003 C1 demanded. The `#[derive(Deserialize)]` is
  gone, the manual impl checks for the redaction marker, and the test at
  protocol.rs:136–149 verifies the error message mentions `[REDACTED]` and
  does not leak key bytes.
- **`encrypt`/`decrypt` use `encryption_path_for_version(key_version)`**
  (service.rs:320–340) — review #003 C2's resolution is implemented faithfully.
  The `key_version` is stamped on `EncryptedData` from the same call, not from
  `PATHS::ENCRYPTION`. Test at service.rs:610–621 confirms v2 round trip;
  service.rs:623–638 confirms rotate v2→v3.
- **`encryption_path_for_version(version < 2)` returns `InvalidPath`**
  (derivation.rs:61–68) — review #003 W5's guard is in place. Tests at
  derivation.rs:274–287 and service.rs:654–679 cover v0 and v1 rejection at
  both the path and `encrypt` layers.
- **`std::sync::RwLock` (not tokio) in `VaultServiceHandle`** (service.rs:43)
  — review #003 C4's resolution is correct. All vault methods are sync; no
  `tokio` dependency in `alknet-vault`'s `Cargo.toml`. Poisoned-lock recovery
  via `unwrap_or_else(|e| e.into_inner())` at service.rs:133, 152, 173, 190,
  etc. — tested at service.rs:429–449.
- **`Capabilities` is non-serializable, `Zeroize`/`ZeroizeOnDrop`, `Clone`,
  sealed-builder** (types.rs:57–118) — review #003 C7's resolution is
  implemented: private `entries`, builder via `with_api_key`/`with_http_token`,
  no `Serialize` derive (test at types.rs:589–592 asserts this), redacting
  `Debug`. `Clone` is explicit (not derived) and zeroizes-on-drop via the
  `Secret<T>` wrapper.
- **`SessionOverlaySource` is defined in `alknet-call`** (adapter.rs:38–43) —
  review #003 C8's crate-graph fix is correct. The trait lives alongside
  `OperationEnv`, not in a hypothetical `alknet-agent`. `CallAdapter` holds
  `Option<Arc<dyn SessionOverlaySource + Send + Sync>>`.
- **`CompositeOperationEnv` dispatch probes `contains()` before delegating**
  (env.rs:144–161) — review #003 C9's resolution is implemented: session →
  connection → base, each gated by `contains()`. No sentinel ambiguity. Tests
  at env.rs:397–417 (session overlay wins), 419–448 (falls through to base),
  466–488 (contains aggregates), 568–597 (connection wins when session absent
  or missing the op).
- **`OperationRegistryBuilder` is Layer-0-only; `CallConnection::register_
  imported` is the Layer 2 API** (registration.rs:127–199, connection.rs:57–
  67) — review #003 C10's resolution is correct. The builder has no overlay
  API; runtime overlay registration uses `CallConnection` methods. The
  `OverlayOperationEnv` (connection.rs:232–291) implements `OperationEnv`
  with `contains()` keyed on the overlay map.
- **`with_local` takes 5 args including `capabilities`** (registration.rs:138–
  157) — review #003 C11's resolution is in place. Both examples in the spec
  now agree; the implementation matches.
- **`invoke_with_policy` is the required method; `invoke` has a default impl**
  (env.rs:11–35) — review #003 W1's resolution is correct. Impls only write
  `invoke_with_policy`; `invoke` delegates with `parent.abort_policy`.
- **`OperationContext` has 11 fields including `abort_policy` and `deadline`**
  (context.rs:11–23) — matches the post-review-#003 spec. `internal` is
  `pub(crate)` (set only by `OperationRegistry::invoke` and the env impls,
  not by handlers) — good, prevents handlers from forging internal authority.
- **`CallError.details` is `Option<Value>` and serializes only when present**
  (wire.rs:61–68, 477–481) — ADR-023's typed error payload is wired. The
  `skip_serializing_if = "Option::is_none"` keeps protocol-level errors (no
  `details`) compact on the wire.
- **`OperationProvenance` and `CompositionAuthority` are defined inline in
  the call crate** (registration.rs:19–27, context.rs:38–65) — review #003
  W11's resolution is correct. `CompositionAuthority::as_identity()` produces
  the `Identity` used for internal-call ACL (registration.rs:103–110),
  matching ADR-015's "handler identity, not caller identity, for internal
  calls" rule. Test at registration.rs:473–503 confirms internal call uses
  handler authority; 505–538 confirms insufficient handler authority is
  forbidden; 540–570 confirms external call uses caller identity, not handler
  authority.
- **Internal ops return `NOT_FOUND` (not `FORBIDDEN`) from the wire**
  (registration.rs:98–100) — ADR-015's "internal ops invisible from the wire"
  rule is correct. Tested at registration.rs:331–351.
- **`auth_token` resolution: success overrides connection identity; failure
  falls back** (adapter.rs:87–105) — matches call-protocol.md:403. Tested at
  adapter.rs:490–535 (all four cases: no token, valid token, invalid token
  with connection identity, invalid token without connection identity).
- **`call.requested` payload carries no abort policy field; root gets
  `AbortPolicy::default()`** (adapter.rs:161) — ADR-016 Decision 6 is
  honored. The wire caller cannot set the policy; the composing handler opts
  children into `ContinueRunning` via `invoke_with_policy`.
- **Vault has no `tokio` dependency, no `irpc`, no ALPN, no alknet-core
  dep** (Cargo.toml) — ADR-018, ADR-025 are satisfied. The vault is local-
  only by construction.
- **`VaultServiceHandle` is `Clone` (Arc<RwLock>)** (service.rs:56–59) —
  matches ADR-019's "assembly layer holds the handle, injects derived
  material" model.
- **`rotate` decrypts with old version, re-encrypts with new** (service.rs:350–
  357) — ADR-021's "no new mnemonic" rotation is correct. Partial rotation is
  safe (test at service.rs:640–652 confirms old key still derivable after
  rotation).
- **`KeyCache` zeroizes on drop via `CachedKey: Zeroize + ZeroizeOnDrop`**
  (cache.rs:25–37) — `DerivedKey` inside `CachedKey` is `#[zeroize]` on the
  private key field. `KeyCache::clear` drops all entries, triggering
  zeroization. The drop-tracker tests at cache.rs:215–289 verify HashMap
  `clear`/`remove`/`insert`-replace all invoke `Drop` on the values.
- **`build_root_context` reads `composition_authority`, `capabilities`,
  `scoped_env` from the registration bundle** (adapter.rs:126–166) — ADR-
  022's bundle is the dispatch source of truth, not a closure-captured
  ambient. Capabilities are per-request on `OperationContext`, populated
  from the bundle.
- **`OperationContext.metadata` is fresh per composed call** (env.rs:81,
  connection.rs:277) — ADR-014's "metadata does not propagate through
  composition" security constraint is honored. Test at env.rs:351–370
  confirms child metadata is empty even when parent has entries.
- **`ScopedOperationEnv` bounds composition reachability** (context.rs:67–94,
  env.rs:63–65, connection.rs:248–250) — ADR-015's "handler can only invoke
  declared ops" rule is enforced at every env layer. Test at env.rs:288–299
  confirms disallowed op returns `NOT_FOUND`.
- **Wire framing: 4-byte big-endian length prefix, 64 MiB max, zero-length
  rejected, oversized rejected, EOF → `ConnectionClosed`** (wire.rs:185–214)
  — robust. Tested at wire.rs:297–349.
- **`CallAdapter::handle` spawns a 10s sweeper for expired pending entries**
  (adapter.rs:246–259) and fails all pending on connection close (289–297).
  Correct lifecycle.

---

## Recommended Resolution Order

1. **W1 (abort cascade wiring)** — highest value. The code exists; the bolt
   is missing. ~30 lines in `handle_stream` + one integration test. Without
   this, ADR-016 is a documented property that doesn't hold at runtime.
2. **W3 (Mnemonic Debug redaction)** — smallest fix, eliminates a latent
   root-of-trust leak. ~10 lines + one test.
3. **W2 (fingerprint extraction)** — decide: implement or explicitly defer.
   Either way, reconcile spec / summary / code. If implementing, the quinn
   path is ~20 lines (downcast `HandshakeData`, extract client cert, hash to
   fingerprint); the iroh path uses the peer `NodeId` directly.
4. **W4 (ApiKeyEntry resources)** — decide: add the field or drop it from the
   spec. Whichever way, fix the three-way mismatch.
5. **S1–S5** — opportunistic; bundle into a follow-up "polish" task or address
   as touched.

Review #004 is open. Zero critical findings; four warnings, all local; five
suggestions. The implementation is sound, the spec drift is bounded, and the
one wiring gap (W1) has all the hard logic already written and tested — it
just needs to be called.

---

## Resolution (2026-06-24)

All four warnings (W1–W4) resolved. Workspace green:
`cargo build --workspace --all-features`, `cargo test --workspace
--all-features` (326 tests, 0 failures), `cargo clippy --workspace
--all-features --all-targets` (0 warnings).

- **W1 (abort cascade wiring)**: `CallAdapter::handle_stream` now
  matches `EVENT_ABORTED`, invokes `AbortCascade::cascade_abort` with
  `AbortPolicy::AbortDependents`, and aborts the root. No descendant
  `call.aborted` frames sent on the wire (ADR-016 Decision 2). Two
  integration tests cover the cascade + unknown-id no-op paths.
  (`tasks/call/protocol/abort-cascade-wiring.md` → completed)

- **W2 (fingerprint extraction)**: `dispatch_quinn` extracts the leaf
  client cert DER via `peer_identity()` → `SHA256:<hex>`; `dispatch_iroh`
  extracts the peer `NodeId` → `ed25519:<hex>`. Fingerprint format
  documented in `auth.md`. Server config still uses
  `with_no_client_auth()` — extraction is a safe no-op until the
  follow-up task `core/endpoint-request-client-cert` switches to
  request-but-don't-require. Two unit tests cover fingerprint format +
  determinism.
  (`tasks/core/endpoint-client-fingerprint.md` → completed)

- **W3 (Mnemonic Debug redaction)**: `#[derive(Debug)]` replaced with
  manual redacting impl matching the `DerivedKey` pattern. `Seed`
  confirmed to have no `Debug` impl. Test asserts no phrase word leaks.
  (`tasks/vault/mnemonic-debug-redaction.md` → completed)

- **W4 (ApiKeyEntry resources)**: Option B chosen — spec corrected to
  drop `entry.resources`; `auth.md` now documents that external
  identities (token/fingerprint) grant scopes only, resource-scoped
  ACLs are a composition-internal concern (ADR-015/022). Two tests
  confirm both resolvers return empty resources.
  (`tasks/core/auth-apikey-resources.md` → completed)

S1–S5 (suggestions) remain opportunistic; not gated by this review.