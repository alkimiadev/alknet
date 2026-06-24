---
id: core/auth-apikey-resources
name: Reconcile ApiKeyEntry.resources — add field to type and populate in resolve_api_key, or drop from spec
status: completed
depends_on: []
scope: narrow
risk: low
impact: component
level: research
---

## Description

Three-way mismatch between spec, type, and implementation for
resource-scoped ACLs on API-key-authenticated identities:

- **Spec** (`docs/architecture/crates/core/auth.md:153`):
  > "Token: ... return `Identity { id: prefix, scopes: entry.scopes,
  > resources: entry.resources }`."
  The spec references `entry.resources`.

- **Type** (`crates/alknet-core/src/config.rs:55–62`): `ApiKeyEntry` has
  fields `prefix, hash, scopes, description, expires_at` — there is no
  `resources` field. So `entry.resources` in the spec cannot be
  implemented as written.

- **Implementation** (`config.rs:113–117`): `resolve_api_key` constructs
  the resolved `Identity` with `resources: std::collections::HashMap::new()`
  — resources are always empty, regardless of what the API key grants.

The same gap exists in `resolve_identity_from_fingerprint`
(config.rs:69–79), which also returns `resources: HashMap::new()`.

### Impact

Latent today: no operation in the workspace uses resource-based ACLs
against a token- or fingerprint-resolved identity. The
`AccessControl::resource_type` / `resource_action` fields exist in
`OperationSpec` (spec.rs:32–37) and are tested (spec.rs:284–303), but
those tests always hand-construct `Identity.resources` directly —
never via the resolver path. The moment an operation declares a
resource-scoped ACL and a caller authenticates via API key, the ACL
check will fail with "missing resource" even if the key was granted
that resource in config — because `resources` is always empty.

### This is a research/decision task, not an implementation task

The decomposer rule applies: **the architecture is ambiguous** on
whether API keys should grant resource-scoped access. Two valid designs
exist; pick one and document it before implementing. Do not implement
until the decision is made.

**Option A — add `resources` to `ApiKeyEntry`** (matches current spec):
- Add `pub resources: HashMap<String, Vec<String>>` to `ApiKeyEntry`.
- Update `resolve_api_key` to populate `Identity.resources` from
  `entry.resources`.
- Update `resolve_identity_from_fingerprint` similarly — either add a
  `resources` field to the fingerprint config path, or document that
  fingerprint auth grants scopes only (resources empty).
- Update `auth.md`'s token resolution example to match the new field.
- Define the TOML schema for `resources` in `AuthPolicy` (when a TOML
  schema is added — currently config is built in code, not parsed).
- Resource-scoped ACLs then work for both auth paths.

**Option B — drop `resources` from the spec for API keys**:
- Remove `entry.resources` from `auth.md:153`.
- Document that API keys grant scopes only; resource-scoped access
  requires a different identity source (e.g., a future OAuth/JWT
  provider that carries resource claims).
- `Identity.resources` stays in the type (it's used by hand-constructed
  identities in tests and by `CompositionAuthority::as_identity` for
  internal calls) but token/fingerprint resolvers always return empty.
- Resource-scoped ACLs against token identities return `Forbidden` —
  this becomes a documented limitation, not a bug.

### Deliverable

Produce a short decision note (a paragraph in `auth.md` under
"Identity Resolution" — or a new ADR if the decision feels
consequential enough) that picks A or B and justifies it. Then either
implement the chosen option in the same task (if small) or split a
follow-up `level: implementation` task gated on this one.

The decision should consider: do any planned operations (in the
upcoming alknet-ssh, alknet-fs, alknet-git crates) need resource-scoped
ACLs on API-key identities? If yes, A. If resource ACLs are only ever
applied to handler-internal composition identities
(`CompositionAuthority`), B is fine and simpler.

## Acceptance Criteria

- [ ] Decision made: Option A or Option B
- [ ] Decision documented in `auth.md` (or a new ADR if consequential)
- [ ] If Option A: `ApiKeyEntry.resources` added, `resolve_api_key` populates `Identity.resources`, `resolve_identity_from_fingerprint` handling decided and documented, `auth.md:153` matches the new shape
- [ ] If Option B: `auth.md:153` corrected to drop `entry.resources`, limitation documented
- [ ] Either way: a test covering the chosen behavior (token resolves with resources, or token resolves with empty resources + documented limitation)
- [ ] `cargo test -p alknet-core` succeeds
- [ ] `cargo clippy -p alknet-core --all-targets` succeeds with no warnings

## References

- docs/reviews/004-post-implementation-sanity-check.md — W4 (full finding)
- docs/architecture/crates/core/auth.md:152–153 — spec text referencing `entry.resources`
- crates/alknet-core/src/config.rs:55–62 — `ApiKeyEntry` (missing `resources`)
- crates/alknet-core/src/config.rs:69–118 — both resolvers returning empty `resources`
- crates/alknet-call/src/registry/spec.rs:77–103 — `AccessControl::check` resource path (the consumer that would fail)
- crates/alknet-call/src/registry/context.rs:58–65 — `CompositionAuthority::as_identity` (the internal-call path that does populate `resources`)

## Notes

> This is a `level: research` task because the fix is small but the
> decision is not. The decomposer principle: if architecture is
> ambiguous, do not proceed with implementation — escalate. Make the
> decision first, then implement. If the decision is A and the
> implementation is more than ~30 lines, split a follow-up
> `level: implementation` task (`core/auth-apikey-resources-impl`)
> depending on this one.

## Summary

Decision: **Option B** — dropped `entry.resources` from the spec.
Rationale: `Identity.resources` is populated only by
`CompositionAuthority::as_identity` (the composition path, ADR-015/022).
All architecture examples use scope-based ACLs for external identities
(`fs:read`, `vastai:query`, `llm:call`). Adding a second
resource-population path for API keys would muddy the external/internal
separation without a demonstrated downstream need. `auth.md:153`
corrected to `resources: {}`; documented limitation added. Two tests
confirm both `resolve_api_key` and `resolve_identity_from_fingerprint`
return empty resources. `cargo test -p alknet-core` and clippy clean.