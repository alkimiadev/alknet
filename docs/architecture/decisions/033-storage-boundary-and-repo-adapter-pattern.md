# ADR-033: Storage Boundary and Repo/Adapter Pattern

## Status

Accepted (resolves the storage-boundary dimension of OQ-34; establishes the
pattern that ADR-030 and ADR-031 follow). **The concrete adapter shapes
deferred by §"What this does NOT do" are now committed by
[ADR-035](035-concrete-persistence-adapter-shapes.md)** (read/write split,
honker+SQLite, `alknet-store-sqlite` crate).

## Context

OQ-34 tracked the storage-boundary question: do the core crates (alknet-core,
alknet-call, alknet-vault) know about persistence at all, or does persistence
live entirely outside the crate graph? The question was left open because the
project deliberately kept the core crates DB-free — smaller, fewer
dependencies, simpler testing. That posture served the local-only crates
(vault, registry) well: vault key rotation is version-indexed derivation
paths (ADR-021), no DB needed.

Then peer identity surfaced as the first cross-node state that wants
persistence: a stable logical peer identity mapped to its current
cryptographic material, surviving restarts and key rotations. OQ-33's v1
UUID workaround was a no-storage stand-in. The research at
`docs/research/alknet-storage-strategy/findings.md` identified the answer:
core defines repo traits (the abstraction), adapters implement them against
specific backends (the implementation), the assembly layer wires the
adapter. This is the same pattern `IdentityProvider` already establishes —
we're making it explicit and extending it to every storage-shaped concern.

The research also established that `IdentityProvider` is the right shape
*for the trait boundary*, not for the implementation: the trait is in core;
the implementations are adapters. The pre-ADR-030 framing ("core is
storage-free, persistence is entirely outside the crate graph") was too
narrow — it conflated "core has no DB dependency" (true and preserved) with
"core has no storage abstraction" (the question). The answer is: **core has
the trait and the in-memory default; persistence adapters are separate
crates; the assembly layer wires the adapter.**

This is a one-way door. If core gains a repo trait, downstream crates depend
on the trait shape and it becomes a contract. If core stays storage-free,
the registry lives in a service crate and core never knows about
persistence. Reversing either direction breaks downstream consumers. The
research has made the decision; this ADR records it.

## Decision

### 1. Core defines repo traits; the in-memory default adapter lives alongside the trait

The core crates own the **trait boundary** for storage-shaped concerns and
the **in-memory default adapter**. They do NOT own the persistence backends.

```rust
// alknet-core — the pattern, applied to two concerns:

pub trait IdentityProvider: Send + Sync + 'static {           // ADR-004
    fn resolve_from_fingerprint(&self, fingerprint: &str) -> Option<Identity>;
    fn resolve_from_token(&self, token: &AuthToken) -> Option<Identity>;
}
pub struct ConfigIdentityProvider { ... }                        // in-memory default (ADR-030)

pub trait CredentialStore: Send + Sync {                        // ADR-031, refined by ADR-035
    fn get(&self, provider: &str) -> Option<EncryptedData>;
    async fn put(&self, provider: &str, data: &EncryptedData) -> Result<(), StoreError>;
    async fn delete(&self, provider: &str) -> Result<(), StoreError>;
}
pub struct InMemoryCredentialStore { ... }                      // in-memory default (ADR-031)
```

The trait is the one-way door — once downstream crates depend on it, the
shape is a contract. The in-memory default adapter is a reference
implementation that covers tests and config-loaded deployments; it carries
no persistence backend dependency.

### 2. Persistence adapters are separate crates, built when a concrete use case forces them

A persistence adapter (e.g., `alknet-peer-store-sqlite`,
`alknet-credential-store-sqlite`) is a **separate crate** that implements a
core repo trait against a specific backend. The adapter:
(**Update:** [ADR-035](035-concrete-persistence-adapter-shapes.md)
collapses these two into a single `alknet-store-sqlite` crate
implementing both `IdentityStore` and `CredentialStore` with shared
SQLite connection + honker LISTEN infra; splitting later is a two-way
door.)

- Depends on alknet-core (for the trait and the types it implements
  against).
- Owns its backend dependency (rusqlite + honker, a key-value store, a
  remote service — the backend choice is the adapter's concern).
- Is wired by the assembly layer at deployment time, replacing the
  in-memory default when persistence is needed.

The pattern:

```
alknet-core (lean — no SQLite, no honker, no backend deps)
├── IdentityProvider trait          (the auth repo trait — ADR-004)
├── ConfigIdentityProvider          (in-memory default — ADR-030)
├── CredentialStore trait           (the credential repo trait — ADR-031)
└── InMemoryCredentialStore         (in-memory default — ADR-031)

Persistence adapters (separate crates, built when needed)
├── peer-store adapter              (implements IdentityProvider against a backend)
└── credential-store adapter        (implements CredentialStore against a backend)

alknet-call (lean — no SQLite, no honker, no storage traits)
├── Uses IdentityProvider (the trait, not the adapter)
└── AccessControl::check(identity) for per-node ACL
```

The decomposition principle: **the trait lives where the types live
(alknet-core); the adapter implementation lives where its backend
dependency lives (a separate crate).** This mirrors the adapter location
principle in `client-and-adapters.md`: `OperationAdapter` lives in
`alknet-call` (where the types live); `from_openapi`/`from_mcp` live in
`alknet-http` (where the HTTP dependency lives).

### 3. The assembly layer wires the adapter

The CLI binary (the only crate that depends on all handler crates and the
vault, ADR-003) constructs the adapter at startup. For a deployment that
needs persistence, the assembly layer constructs the SQLite adapter instead
of the in-memory default and passes it where the trait is consumed.

This is the same wiring pattern as `IdentityProvider` today: the CLI
constructs `ConfigIdentityProvider` (or, with this ADR, the SQLite adapter)
and passes `Arc<dyn IdentityProvider>` to every handler that needs it.

### 4. What this does NOT do

- **Does not add a SQLite dependency to alknet-core.** Core carries the
  trait and the in-memory default. The SQLite dependency lives in the
  adapter crate.
- **Does not specify concrete adapter shapes.** The trait shape is the
  one-way door. The concrete adapter shapes (table schemas, backend
  choice, indexing, caching) are deferred for exploration — the project's
  note is that the repo pattern is a tool to reach for when a storage
  concern is concrete, not a one-size-fits-all mold to apply
  speculatively. The pattern is committed; the adapters are not.
  **Update**: [ADR-035](035-concrete-persistence-adapter-shapes.md) now
  commits the concrete adapter shape for the SQLite+honker backend
  (read-sync / write-async split, `IdentityStore` write trait, honker
  NOTIFY cache invalidation, `alknet-store-sqlite` crate). The deferral
  above applied to *which* backend and *what* shape; ADR-035 resolves
  both for the SQLite case. Other backends (Redis, Postgres, on-chain)
  remain "not needed for current scope."
- **Does not change the no-DB posture of the core crates.** Core remains
  DB-free in the sense that it has no backend dependency — only a trait
  boundary. The in-memory adapter carries no persistence. The persistence
  adapters are additive crates.
- **Does not introduce a generic "Storage" trait.** Each storage-shaped
  concern gets its own trait (`IdentityProvider`, `CredentialStore`). A
  generic `Storage<T>` trait would be over-abstraction — the concerns are
  different enough (identity resolution vs. encrypted-blob persistence)
  that a single trait would force a least-common-denominator shape.

## Consequences

**Positive:**
- OQ-34 is resolved. The storage boundary is: core defines the repo trait
  + the in-memory default; persistence adapters are separate crates; the
  assembly layer wires. The no-DB posture is preserved in the sense that
  matters (core has no backend dependency) while the abstraction is in
  place for the cross-node state that wants persistence.
- The pattern is reusable. When a future storage-shaped concern surfaces
  (e.g., ACL delegation graph, filesystem path tree), it follows the same
  shape: trait in core, in-memory default, persistence adapter additive.
  The research identified this as the right tool to reach for, and this
  ADR commits the pattern.
- Downstream crates that don't use the call protocol (alknet-http, a
  service with no protocol at all) still resolve identities and check ACL
  via the same trait. The auth layer is not owned by alknet-call — it's
  owned by core, consumed everywhere.
- The door to distributed auth adapters (automerge sync, Redis, a remote
  identity service) is open without being designed. The trait doesn't care
  which backend is wired.

**Negative:**
- alknet-core gains repo traits. Each trait is a contract downstream
  crates depend on. Getting the trait shape right matters — a wrong shape
  breaks every consumer when it's fixed. ADR-030 and ADR-031 commit the
  first two trait shapes; future traits follow the same review bar.
- The in-memory default adapter is a reference implementation, not a
  production persistence layer. Deployments that need persistence must
  wire a persistence adapter — the in-memory default loses state on
  restart. This is documented, not hidden.
- Concrete adapter shapes are not specified. This is deliberate (the
  project is iterating on adapter simplification), but it means the
  persistence-adapter build order is open. The trait shape is the
  commitment; the adapter build is the two-way door.

## Assumptions

1. **The trait shape is the one-way door; the adapter shape is the
   two-way door.** Getting the trait right is the review bar; getting the
   adapter right is an implementation detail that can iterate.

2. **Each storage-shaped concern gets its own trait.** No generic
   `Storage<T>`. The concerns are different enough that a single trait
   would over-abstract.

3. **The in-memory default adapter is the reference implementation.** It
   covers tests and config-loaded deployments. It is not a production
   persistence layer.

4. **Persistence adapters are additive crates, built when a concrete use
   case forces them.** Not built speculatively. The pattern is committed;
   the adapters are not.

5. **Concrete adapter shapes are deferred for exploration.** The project
   is iterating on adapter simplification; the trait shapes in this ADR
   and ADR-030/031 are the commitment, not the adapter table schemas or
   backend choices.

## References

- ADR-003: Crate Decomposition (the dependency rules this ADR follows —
  core is lean, adapters are separate crates, the assembly layer wires)
- ADR-004: Auth as Shared Core (`IdentityProvider` — the first instance of
  the pattern this ADR makes explicit)
- ADR-018: Vault as Standalone Crate (the vault has zero alknet-crate
  dependencies; the repo pattern doesn't change that)
- ADR-025: Vault Local-Only Dispatch (the vault is the sole decryption
  boundary; `CredentialStore` persists encrypted blobs, never decrypts)
- ADR-030: PeerEntry and Identity.id Decoupling (the first application of
  this pattern to peer identity — `PeerEntry` config model +
  `ConfigIdentityProvider` in-memory default)
- ADR-031: CredentialStore Repo Trait (the second application —
  `CredentialStore` trait + `InMemoryCredentialStore` default)
- ADR-035: Concrete Persistence Adapter Shapes (commits the concrete
  adapter shape this ADR's §"What this does NOT do" deferred —
  read/write split, honker+SQLite, `alknet-store-sqlite` crate)
- OQ-34: Persistent Peer Registry (resolved by this ADR — the storage
  boundary is `core trait + in-memory default`, persistence adapters
  additive)
- OQ-36: Concrete Adapter Shapes (resolved by ADR-035 — the concrete
  SQLite+honker adapter shape is committed; the trait shapes committed
  by this ADR and ADR-030/031 are the one-way doors ADR-035 builds on)
- `docs/research/alknet-storage-strategy/findings.md` §3-4 (the
  SQLite+honker foundation and the repo/adapter pattern)
- `/workspace/keypal` — TypeScript repo-pattern reference (the Storage
  interface + adapters pattern alknet follows)