---
id: architecture/adr-028-auth-irpc-service
name: Write ADR-028 — Auth as irpc service
status: pending
depends_on:
  - architecture/adr-029-identity-core-type
scope: narrow
risk: medium
impact: phase
level: implementation
---

## Description

Write ADR-028 establishing that auth verification is provided via an irpc service protocol, with the `IdentityProvider` trait as the interface contract and `ConfigIdentityProvider` (ArcSwap-backed) as the default implementation.

This ADR defines the relationship between the trait-based path and the irpc path:

1. `IdentityProvider` trait in `alknet_core::auth` — the contract that callers depend on
2. `ConfigIdentityProvider` — default impl, reads from `ArcSwap<DynamicConfig>`, no database needed
3. `AuthProtocol` irpc service enum — `VerifyPubkey`, `VerifyToken`, `ReloadKeys`, `CheckAccess` — behind `irpc` feature flag
4. Future: `StorageIdentityProvider` (in alknet-storage) backed by SQLite — additive, not replacing the trait

The critical design point: callers go through `IdentityProvider`. The irpc service is one way to satisfy the trait. Feature-gating (`irpc` feature) means nodes that only do SSH tunneling don't need the service layer overhead. Both paths produce the same result — an `Identity` or rejection.

## Acceptance Criteria

- [ ] `docs/architecture/decisions/028-auth-irpc-service.md` exists
- [ ] ADR follows established format
- [ ] Context explains why a service layer is needed: for head nodes serving many users, in-memory key lookup doesn't scale; irpc provides async boundary for database-backed auth
- [ ] Decision states: IdentityProvider trait is the contract; ConfigIdentityProvider is the default; AuthProtocol irpc service is behind feature flag; irpc path and trait path produce identical Identity results; StorageIdentityProvider in alknet-storage is a future additive impl
- [ ] Shows AuthProtocol enum (`VerifyPubkey`, `VerifyToken`, `ReloadKeys`, `CheckAccess`) and AuthResult type
- [ ] Consequences: minimal deployments use ArcSwap without irpc; production deployments wire SQLite-backed service; feature flag keeps core lean
- [ ] References: research/services.md AuthProtocol, auth.md, research/configuration.md auth service approach, ADR-029

## References

- docs/research/services.md — AuthProtocol definition
- docs/architecture/auth.md — IdentityProvider trait, Identity struct
- docs/research/configuration.md — auth service approach
- docs/research/integration-plan.md — ADR 028 entry, Phase 1.4

## Notes

> To be filled by implementation agent

## Summary

> To be filled on completion