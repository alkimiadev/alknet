---
id: cleanup/non-exhaustive-public-api
name: Add #[non_exhaustive] to public API enums and structs likely to evolve
status: completed
depends_on:
  - review/phase1-core-modifications
scope: narrow
risk: low
impact: component
level: implementation
---

## Description

Several public API types introduced in Phase 1 are likely to gain variants or fields in future phases. Adding `#[non_exhaustive]` now prevents downstream breakage when new variants/fields are added.

**Types to annotate**:
- `ForwardingAction` — may gain new actions beyond Allow/Deny
- `TargetPattern` — may gain new pattern types
- `OperationType` — may gain new operation kinds
- `InterfaceConfig` — will gain new interface types (HTTP/WS, DNS)
- `ForwardingRule` — may gain new matching fields
- `DynamicConfig` — will gain new sections
- `CallError` — may gain new fields

**Note**: `TransportKind` already has `Dns` and `WebTransport` as tags-only and is likely to gain variants too, but it may already be exhaustively matched in some code. Check first before annotating.

## Acceptance Criteria

- [ ] `#[non_exhaustive]` added to enums/structs listed above
- [ ] All match statements on these types updated with wildcard arms where needed
- [ ] All existing tests pass
- [ ] No new warnings from clippy

## References

- crates/alknet-core/src/config/forwarding.rs — ForwardingAction, TargetPattern, ForwardingRule
- crates/alknet-core/src/call/spec.rs — OperationType
- crates/alknet-core/src/interface/mod.rs — InterfaceConfig
- crates/alknet-core/src/config/dynamic_config.rs — DynamicConfig
- crates/alknet-core/src/call/response.rs — CallError

## Notes

> Suggested during Phase 1 review (S1)

## Summary

> Added #[non_exhaustive] to ForwardingAction, TargetPattern, ForwardingRule, OperationType, InterfaceConfig, InterfaceKind, DynamicConfig, and CallError. Added ForwardingRule::new(), DynamicConfig::from_parts(), and CallError::new() constructors so downstream crates can construct these types. Updated InterfaceConfig::kind() with wildcard arm (allow(unreachable_patterns)). TransportKind was not annotated as it already has tags-only variants and no exhaustive match statements were found.