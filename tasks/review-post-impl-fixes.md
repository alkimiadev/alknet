---
id: review-post-impl-fixes
name: Review the four post-implementation sanity-check #004 fixes for spec conformance
status: completed
depends_on: [call/protocol/abort-cascade-wiring, core/endpoint-client-fingerprint, vault/mnemonic-debug-redaction, core/auth-apikey-resources]
scope: moderate
risk: low
impact: phase
level: review
---

## Description

Review the four fixes produced from review #004's findings (W1–W4)
before they are considered closed. Confirm each fix matches the
resolution described in `docs/reviews/004-post-implementation-sanity-
check.md`, does not introduce new spec drift, and is adequately tested.

### Per-fix review checklist

**W1 — `call/protocol/abort-cascade-wiring`**:
- `CallAdapter::handle_stream` handles `EVENT_ABORTED` (not just
  `EVENT_REQUESTED`).
- Cascade uses `AbortPolicy::AbortDependents` (the wire caller does not
  choose the policy — ADR-016 Decision 6).
- No `call.aborted` frames are sent back to the wire for descendant IDs
  (ADR-016 Decision 2: server-side cascade; composed child request_ids
  are internal).
- Root entry is also removed (cascade_abort skips the root by design).
- Integration test exercises the full path: inbound abort frame →
  `PendingRequestMap` entries gone for parent + child.

**W2 — `core/endpoint-client-fingerprint`**:
- `dispatch_quinn` and `dispatch_iroh` extract a fingerprint when one
  is presented (not hard-coded `None`).
- `AuthContext.identity` is populated via `resolve_from_fingerprint`
  when the fingerprint resolves.
- Fingerprint string format is documented in `auth.md` and consistent
  with `AuthPolicy::authorized_fingerprints`.
- No regression: no-client-cert case still produces
  `tls_client_fingerprint: None` and `identity: None`.
- Server-config decision (request-but-don't-require vs. no-client-auth)
  is documented.

**W3 — `vault/mnemonic-debug-redaction`**:
- `Mnemonic` has a manual redacting `Debug` impl; `#[derive(Debug)]`
  is gone.
- `format!("{:?}", mnemonic)` does not contain any phrase word.
- `Seed` checked — no `Debug` impl leaks `bytes`.

**W4 — `core/auth-apikey-resources`**:
- Decision (Option A or B) is documented in `auth.md` or a new ADR.
- Implementation (if any) matches the decision.
- `auth.md:153` no longer references `entry.resources` if Option B was
  chosen; or `ApiKeyEntry.resources` exists and is populated if Option
  A was chosen.
- Test covers the chosen behavior.

### Cross-cutting checks

- `cargo build --workspace --all-features` succeeds.
- `cargo test --workspace --all-features` succeeds (no regressions).
- `cargo clippy --workspace --all-features --all-targets` clean.
- No new spec/code drift introduced (reconcile any spec text touched
  against the implementation).
- Update `docs/reviews/004-post-implementation-sanity-check.md`'s
  status from `open` to `resolved` once all four findings are confirmed
  fixed.

## Acceptance Criteria

- [ ] W1 fix confirmed: inbound `call.aborted` cascades to descendants
- [ ] W2 fix confirmed: endpoint extracts TLS client fingerprint
- [ ] W3 fix confirmed: `Mnemonic` `Debug` redacts the phrase
- [ ] W4 fix confirmed: `ApiKeyEntry.resources` reconciled with spec (or spec corrected)
- [ ] `cargo build --workspace --all-features` succeeds
- [ ] `cargo test --workspace --all-features` succeeds
- [ ] `cargo clippy --workspace --all-features --all-targets` succeeds with no warnings
- [ ] Review #004 status updated to `resolved` in its frontmatter

## References

- docs/reviews/004-post-implementation-sanity-check.md — the review being closed
- tasks/call/protocol/abort-cascade-wiring.md — W1 fix task
- tasks/core/endpoint-client-fingerprint.md — W2 fix task
- tasks/vault/mnemonic-debug-redaction.md — W3 fix task
- tasks/core/auth-apikey-resources.md — W4 fix task

## Notes

> This review task mirrors the pattern of `vault/review-vault-sync`,
> `core/review-core`, and `call/review-call`: a `level: review` gate
> at the end of a fix batch, with `scope: moderate`, `risk: low`,
> `impact: phase`. It does not need to re-derive the findings — review
> #004 already did that work. It only needs to confirm the fixes land
> correctly and the workspace stays green.

## Summary

All four fixes verified against acceptance criteria:
- W1: `handle_stream` handles `EVENT_ABORTED`, cascades with
  `AbortDependents`, no descendant frames on wire, root removed, two
  integration tests pass.
- W2: both dispatch paths extract fingerprints, format documented in
  `auth.md`, no-cert case returns `None` (no regression), server-config
  change deferred to `core/endpoint-request-client-cert`.
- W3: `Mnemonic` has manual redacting `Debug`, `Seed` has no `Debug`,
  redaction test passes.
- W4: Option B — spec corrected, limitation documented, both resolvers
  return empty resources, tests pass.

Workspace green: `cargo build --workspace --all-features` ✓, `cargo test
--workspace --all-features` (326 tests, 0 failures) ✓, `cargo clippy
--workspace --all-features --all-targets` (0 warnings) ✓. Review #004
status updated to `resolved`.