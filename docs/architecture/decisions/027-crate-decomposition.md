# ADR-027: Crate Decomposition

## Status

Accepted

## Context

alknet-core currently contains everything: transport, SSH, auth, config, the
call protocol handler, and the server accept loop. As the project grows to
include SQLite-backed identity, HD key derivation, and metagraph storage, core
would need to depend on rusqlite, bip39, petgraph, and other heavy dependencies
— unacceptable for a library crate that CLI users embed.

Different deployment topologies need different subsets:
- A minimal CLI tunnel only needs core, transport, and auth types
- A head node needs SQLite-backed identity and the secret service
- A flowgraph visualization tool only needs petgraph operations

Circular dependencies must be avoided. alknet-storage implements
alknet-core's `IdentityProvider` trait, so alknet-core cannot depend on
alknet-storage. alknet-storage references alknet-secret's `EncryptedData` wire
format, but not as a crate dependency.

## Decision

**Decompose the project into six crates with a strict acyclic dependency graph.**

### Crate Structure

1. **alknet-core** — Transport, SSH, call protocol, config, auth types, identity,
   `OperationSpec`, `Interface` trait. The foundational crate that everything
   else depends on (by type, not by crate dep in some cases).
   - *Depends on*: russh, tokio, irpc (feature-gated), serde, arc-swap
   - *Does NOT depend on*: alknet-secret, alknet-storage, alknet-flowgraph

2. **alknet-secret** — BIP39 mnemonic generation, SLIP-0010 Ed25519 HD key
   derivation, AES-256-GCM encryption, `SecretProtocol` irpc service.
   - *Depends on*: bip39, ed25519-bip32 (or rust-bip32-ed25519), aes-gcm, sha2,
     irpc
   - *Does NOT depend on*: alknet-core, alknet-storage

3. **alknet-storage** — SQLite-backed metagraph, identity tables, ACL graph,
   honker integration, `StorageProtocol` irpc service.
   - *Depends on*: rusqlite (via honker), honker, petgraph, jsonschema, irpc
   - *Does NOT depend on alknet-core* (but implements alknet-core's
     `IdentityProvider` trait via the trait, not a crate dep)
   - *Does NOT depend on alknet-secret* (but references `EncryptedData` type
     format for wire compatibility)

4. **alknet-flowgraph** — `FlowGraph<N,E>` over petgraph, operation graph, call
   graph, type compatibility checking.
   - *Depends on*: petgraph, serde, jsonschema, thiserror
   - *Does NOT depend on*: alknet-core, alknet-storage, alknet-secret

5. **alknet-napi** — Node.js native addon. Exposes alknet-core to Node.js.
   - *Depends on*: alknet-core
   - *Does NOT depend on*: alknet-secret, alknet-storage, alknet-flowgraph

6. **alknet** (CLI binary) — Assembles everything.
   - *Depends on*: alknet-core, alknet-secret (feature), alknet-storage (feature),
     alknet-flowgraph (feature), toml

### Dependency Graph

```
alknet-secret       alknet-storage      alknet-flowgraph
   (standalone)        (standalone)        (standalone)
        │                   │                  │
        │  (feature flags   │   (trait impl    │  (type compat
        │   in CLI binary)  │    via CLI wire)  │   via JSON)
        ▼                   ▼                  ▼
                 ┌─────────────────────┐
                 │    alknet-core       │
                 │  (transport, SSH,     │
                 │   call protocol,     │
                 │   Identity, Config)  │
                 └─────────┬───────────┘
                           │
              ┌────────────┼────────────┐
              ▼            ▼            ▼
        alknet-napi    alknet (CLI binary — assembles everything)
```

All four library crates (core, secret, storage, flowgraph) are independent of
each other. Dependencies flow **upward** only. The CLI binary sits at the top
and wires concrete implementations together. alknet-storage implements
alknet-core's `IdentityProvider` trait without a crate dependency — the CLI
binary provides the bridge.

### Narrow Interface Points

Three types serve as the narrow interface points between crates:

1. **`Identity`** — Defined in `alknet_core::auth`. Used by auth handler,
   forwarding policy, and call protocol. alknet-storage implements
   `IdentityProvider` to produce instances.

2. **`IdentityProvider`** — Trait defined in `alknet_core::auth`. Implemented by
   `ConfigIdentityProvider` (in core) and `StorageIdentityProvider` (in
   alknet-storage). The CLI/NAPI layer wires the concrete implementation.

3. **`OperationSpec`** — Defined in `alknet_core::call`. Used by the operation
   registry and by alknet-flowgraph for type compatibility checking. The bridge
   is serialization — flowgraph serializes to JSON, storage persists it.

### irpc Feature Flag

irpc is a feature flag in alknet-core. When disabled, auth and config go through
`IdentityProvider` and `ConfigReloadHandle` directly — no irpc overhead. Nodes
that only do SSH tunneling don't need the service layer.

In alknet-secret and alknet-storage, irpc is an independent dependency, not
feature-gated. These crates always define irpc service protocols because they
are used in production deployments where the service layer is active.

### alknet-storage's Relationship to alknet-core

alknet-storage does NOT depend on alknet-core as a crate. Instead:

- alknet-storage defines its own `IdentityProvider` impl that matches
  alknet-core's trait signature. The trait is re-exported or defined locally
  with `#[cfg(feature = "alknet-core")]` interop.
- In practice, the CLI binary crate depends on both and wires them together.
  alknet-storage provides `StorageIdentityProvider`; alknet-core takes
  `impl IdentityProvider`.

### alknet-storage's Relationship to alknet-secret

alknet-storage does NOT depend on alknet-secret as a crate. Instead:

- alknet-storage and alknet-secret share the `EncryptedData` wire format (key
  version, salt, IV, ciphertext). This is a type-level compatibility, not a
  crate dependency.
- alknet-secret encrypts; alknet-storage stores the encrypted blob in a
  `SecretNode` in the metagraph. The bridge is serialization.

## Consequences

- **Positive**: Core is lean. No database, no crypto, no petgraph. CLI users
  get a small binary.
- **Positive**: Services are pluggable. alknet-secret and alknet-storage can be
  swapped for alternative implementations.
- **Positive**: No circular dependencies. The dependency graph is a DAG.
- **Positive**: Deployment topology determines which crates to include. A CLI
  tunnel uses only alknet-core. A head node uses everything.
- **Positive**: irpc is feature-gated in core. Minimal deployments don't pay for
  service layer overhead.
- **Negative**: `IdentityProvider` trait interop between alknet-core and
  alknet-storage requires careful versioning. If the trait signature changes,
  both crates must update.
- **Negative**: `EncryptedData` wire format compatibility between alknet-secret
  and alknet-storage is implicit (not enforced by the type system). A shared
  types crate could be extracted if needed, but adds another crate dependency.

## References

- [research/integration-plan.md](../../research/integration-plan.md) — Phase 2, dependency graph
- [research/core.md](../../research/core.md) — alknet-core contents
- [research/services.md](../../research/services.md) — Service protocols
- [research/storage.md](../../research/storage.md) — alknet-storage contents
- [research/flow.md](../../research/flow.md) — alknet-flowgraph contents
- [ADR-028](028-auth-irpc-service.md) — Auth as irpc service (service protocol enabled by decomposition)
- [ADR-029](029-identity-core-type.md) — Identity as core type (narrow interface point)