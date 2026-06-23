---
id: vault/spec-sync-remove-drift
name: Update vault specs to remove drift table and security-constraint drift prose, bump doc status
status: completed
depends_on: [vault/review-vault-sync]
scope: narrow
risk: low
impact: component
level: implementation
---

## Description

After the vault review confirms all drift is resolved, update the vault
architecture docs to remove the drift tracking artifacts and reflect the
completed state. The drift table and the "known drift" prose in the security
constraints sections were tracking tools during the spec-to-implementation
sync — now that the sync is complete, they should be cleaned up.

### What to update

1. **vault/README.md**:
   - Remove the "Known Source Drift" section (the entire table and its intro
     paragraph). The drift is resolved; the table is no longer needed.
   - Remove the "Security Constraints" drift prose — the items that said
     "The current source uses `rand::random()` — this is a known drift" etc.
     Keep the constraint statements themselves (OsRng for IVs, zeroized drop,
     no unwrap, etc.) — those are permanent implementation requirements. Remove
     only the "current source uses X, this is a known drift" sentences.
   - Bump `status: draft` → `status: stable` in the frontmatter (per the
     Document Lifecycle in the architecture README: stable = implementation
     complete and verified).

2. **vault/encryption.md**:
   - In Security Constraints, remove the "The current source uses
     `rand::random()` for IV generation (`encryption.rs` line 133) — this is a
     known drift from the spec and must be corrected during implementation
     sync." sentence. Keep the "OsRng for IVs" constraint.
   - In Key Versioning, remove the "The current source uses
     `CURRENT_KEY_VERSION = 1` with HD derivation and does not implement
     version-indexed paths or `rotate`. These are drift items to be corrected
     during implementation sync." paragraph.
   - Bump `status: draft` → `status: stable`.

3. **vault/service.md**:
   - In Security Constraints, remove the drift prose about `rand::random()`,
     `unwrap()` on RwLock, and `KeyCache::clear()` verification. Keep the
     constraint statements.
   - Bump `status: draft` → `status: stable`.

4. **vault/protocol.md**:
   - Remove the "to be updated per ADR-025 — remove `VaultProtocol` enum and
     irpc usage" note in References.
   - Remove the "postcard tests to be removed" note in References.
   - Bump `status: draft` → `status: stable`.

5. **vault/mnemonic-derivation.md**:
   - Bump `status: draft` → `status: stable` (no drift prose to remove here,
     but the doc should reflect stable status).

6. **architecture/README.md**:
   - Update the vault crate doc status entries in the Architecture Documents
     table from `draft` to `stable`.
   - Update the Current State paragraph to reflect vault implementation is
     complete (remove "pending ADR-025/026 refactor" language).

### What NOT to change

- Do not remove the Security Constraints sections themselves — they are
  permanent implementation requirements, not drift tracking.
- Do not change the ADRs — they record decisions, not implementation status.
- Do not remove the Public API section — it's a living reference.

### Scope

This task touches only documentation files — no source code changes. It
depends on the review task (which depends on all drift fixes).

## Acceptance Criteria

- [ ] "Known Source Drift" table removed from vault/README.md
- [ ] Drift prose removed from Security Constraints sections (constraint statements kept)
- [ ] All vault doc frontmatter bumped from `status: draft` to `status: stable`
- [ ] architecture/README.md vault doc statuses updated to `stable`
- [ ] architecture/README.md Current State updated (no "pending refactor" language)
- [ ] No drift-tracking language remains anywhere in vault docs
- [ ] Security constraint statements (OsRng, zeroize, no unwrap, etc.) preserved
- [ ] Public API section preserved in vault/README.md

## References

- docs/architecture/crates/vault/README.md — Known Source Drift, Security Constraints, Public API
- docs/architecture/crates/vault/encryption.md — Security Constraints, Key Versioning
- docs/architecture/crates/vault/service.md — Security Constraints
- docs/architecture/crates/vault/protocol.md — References
- docs/architecture/README.md — Document Lifecycle, Architecture Documents table, Current State

## Notes

> This is the doc cleanup that closes out the vault phase. The drift table and
> "known drift" prose were tracking tools during spec-to-implementation sync;
> now that the sync is complete, they're noise. Keep the permanent constraint
> statements — they guide future implementation agents who touch the vault.

## Summary

Removed the Known Source Drift table from vault/README.md, removed all "known
drift"/"current source uses X" prose from Security Constraints in README,
encryption.md, and service.md (constraint statements preserved), removed the
drift paragraph in encryption.md Key Versioning, removed stale ADR-025/postcard
notes in protocol.md References. Bumped all 5 vault doc frontmatter to
`status: stable`. Updated architecture/README.md vault doc statuses to stable
and Current State to remove "pending ADR-025/026 refactor" language. Merged to
develop.