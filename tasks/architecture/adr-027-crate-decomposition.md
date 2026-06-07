---
id: architecture/adr-027-crate-decomposition
name: Write ADR-027 — Crate decomposition
status: pending
depends_on:
  - architecture/adr-029-identity-core-type
scope: moderate
risk: medium
impact: project
level: implementation
---

## Description

Write ADR-027 defining the crate decomposition for the alknet project: what crates exist, what each contains, and crucially what the dependency graph looks like (which must be acyclic).

Crate structure:
- **alknet-core**: transport, SSH, call protocol, config, auth types, identity, OperationSpec, Interface trait. Depends on: russh, tokio, irpc (feature-gated), serde. Does NOT depend on: alknet-secret, alknet-storage, alknet-flowgraph.
- **alknet-secret**: BIP39, SLIP-0010 Ed25519 HD key derivation, AES-256-GCM, SecretProtocol irpc service. Depends on: bip39, ed25519-bip32 (or rust-bip32-ed25519), aes-gcm, sha2, irpc. Does NOT depend on: alknet-core, alknet-storage.
- **alknet-storage**: SQLite-backed metagraph, identity tables, ACL graph, honker integration, StorageProtocol irpc service. Depends on: rusqlite, honker, petgraph, jsonschema, irpc. Does NOT depend on alknet-core (but implements alknet-core's IdentityProvider trait via the trait, not a crate dep). Does NOT depend on alknet-secret (but references EncryptedData type format).
- **alknet-flowgraph**: FlowGraph<N,E> over petgraph, operation graph, call graph, type compatibility. Depends on: petgraph, serde, jsonschema. Does NOT depend on: alknet-core, alknet-storage, alknet-secret.
- **alknet-napi**: Node.js native addon. Depends on: alknet-core.
- **alknet** (CLI binary): Assembles everything. Depends on: alknet-core, alknet-secret (feature), alknet-storage (feature), alknet-flowgraph (feature), toml.

The narrow interface points: `Identity` type, `IdentityProvider` trait, and `OperationSpec` are in alknet-core. External crates implement core traits or serialize to formats core understands.

This ADR must also address the irpc feature flag question (OQ: resolved — irpc is behind a feature flag in alknet-core, independent in other crates) and the storage/secret irpc dependency question (resolved — each crate depends on irpc independently).

## Acceptance Criteria

- [ ] `docs/architecture/decisions/027-crate-decomposition.md` exists
- [ ] ADR follows established format
- [ ] Context explains why decomposition is needed: core shouldn't depend on heavy services; different deployment topologies need different subsets; circular dependencies prevent clean builds
- [ ] Decision states: the six crates, their contents, and their dependencies
- [ ] Includes the dependency graph ASCII art from integration-plan.md
- [ ] States the narrow interface points: Identity, IdentityProvider, OperationSpec
- [ ] States that irpc is a feature flag in alknet-core and an independent dep elsewhere
- [ ] States that alknet-storage implements IdentityProvider via the trait (not a crate dependency on alknet-core)
- [ ] States that alknet-storage references alknet-secret's EncryptedData wire format (type-level compatibility, not crate dep)
- [ ] Consequences: core is lean; services are pluggable; no circular deps; deployment topology determines which crates to include
- [ ] References: integration-plan.md dependency graph, ADR-029

## References

- docs/research/integration-plan.md — Phase 2, dependency graph
- docs/research/core.md — alknet-core contents
- docs/research/services.md — service protocols
- docs/research/storage.md — alknet-storage contents
- docs/research/flow.md — alknet-flowgraph contents

## Notes

> To be filled by implementation agent

## Summary

> To be filled on completion