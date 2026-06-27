# ADR-031: CredentialStore Repo Trait

## Status

Accepted (establishes the second repo-trait in core, alongside
`IdentityProvider`; resolves the credential-persistence dimension of
OQ-34)

## Context

`alknet-http`'s `from_openapi` / `from_mcp` handlers need provider
credentials (API keys, OAuth tokens) to call outbound services. ADR-014
established the no-env-vars invariant: credentials come from
`Capabilities`, populated by the assembly layer from the vault at startup.
The vault (ADR-018/019/020/025/026) handles encryption/decryption; the
master seed and derived private keys never cross the network.

What's missing is the **persistence layer** for the encrypted credential
blobs. Today the in-memory `Capabilities` path works for the
vault-at-startup deployment (the assembly layer decrypts everything the
handlers need at boot, injects into `Capabilities`), but there is no
shared, trait-bound abstraction for *where the encrypted blobs live* before
the assembly layer decrypts them, and no way for a runtime process to
`put`/`get`/`delete` encrypted credentials without re-implementing the
storage shape in every consumer.

The research at `docs/research/alknet-storage-strategy/findings.md` §4
identified this as the second application of the repo/adapter pattern (the
first being `IdentityProvider` for peer identity). The vault encrypts; a
`CredentialStore` persists the `EncryptedData` blob; the assembly layer
loads them into `Capabilities` at registration time. The trait boundary
that matters for cross-crate sharing is the store trait, not the storage
backend — exactly mirroring `IdentityProvider`.

The kepal reference (`/workspace/keypal`) demonstrates the same pattern in
TypeScript: a `Storage` interface with adapters for Redis, Drizzle, Prisma,
Kysely, Convex, and in-memory. The core logic is backend-agnostic; storage
is a trait; the consumer picks the adapter at wiring time. The alknet
equivalent: core defines the repo trait, the default in-memory adapter
lives alongside it, and a future persistence adapter is a separate crate
(ADR-033).

## Decision

### 1. Add `CredentialStore` trait to alknet-core

```rust
pub trait CredentialStore: Send + Sync {
    fn get(&self, provider: &str) -> Option<EncryptedData>;
    fn put(&self, provider: &str, data: &EncryptedData) -> Result<(), CredentialStoreError>;
    fn delete(&self, provider: &str) -> Result<(), CredentialStoreError>;
}
```

- `provider: &str` — the provider identifier (`"openai"`, `"anthropic"`,
  `"github"`, etc.). The key the assembly layer uses to look up a
  credential when populating `Capabilities`.
- `EncryptedData` — the vault's encrypted-blob type (ADR-020, defined in
  `alknet-vault`). The store persists the blob as-is; it does not decrypt.
  Decryption is the vault's job (ADR-025, local-only by construction).
- `CredentialStoreError` — a crate-level error enum for store failures
  (backend unreachable, serialization, etc.). `#[non_exhaustive]` so
  adapter crates can extend without breaking match arms.

The trait returns `Option<EncryptedData>` from `get` (not `Result`): a
missing credential is the common case (the provider isn't configured),
not an error. `put` and `delete` are mutations and return `Result` since
the backend may be unwritable (a read-only deployment, a corrupted store,
etc.).

### 2. Add `InMemoryCredentialStore` default adapter to alknet-core

```rust
pub struct InMemoryCredentialStore {
    entries: RwLock<HashMap<String, EncryptedData>>,
}

impl InMemoryCredentialStore {
    pub fn new() -> Self;
    pub fn with_entries(entries: HashMap<String, EncryptedData>) -> Self;
}

impl CredentialStore for InMemoryCredentialStore { ... }
```

The default adapter covers tests and config-loaded deployments where
credentials are decrypted from the vault at startup and held in memory for
the process lifetime. This is the same posture as
`ConfigIdentityProvider` — no persistence, no backend dependency, no env
vars. The assembly layer constructs it from vault-decrypted entries at
boot.

### 3. `EncryptedData` re-export shape

The store trait references `EncryptedData`, which is defined in
`alknet-vault`. To keep alknet-core lean (no vault dependency — ADR-003
keeps the vault standalone with zero alknet-crate dependencies), the
trait's `EncryptedData` parameter is a **core-owned serializable type**:
the vault produces it; the store persists it as a serializable blob; the
vault consumes it back. The core trait carries the wire shape without a
vault dependency.

The exact shape of `EncryptedData` in core is a thin serializable struct
mirroring the vault's type: `{ key_version, salt, iv, data }` (the fields
the vault's `EncryptedData` carries, per ADR-020 and
`crates/alknet-vault/src/encryption.rs`). The `salt` field is kept for
wire-format compatibility with the TS predecessor (OQ-20) — a core mirror
that omitted it could not round-trip the vault's `EncryptedData`. This is a
one-way door — it pins the credential-blob wire shape — and it's
intentionally minimal (the vault's HD-derivation path is the vault's
concern, ADR-020). ADR-020 already defines this shape; this ADR's
commitment is that the store trait carries it as a serializable value type,
not a vault-bound reference.

### 4. No `list` method

The trait is `get` / `put` / `delete` — no `list`. The research (§11 OQ-3)
flagged `list` as a two-way-door remainder: a management UI or a startup-
enumeration use case might want to list all stored providers, but no
current consumer needs it. Adding `list` is non-breaking (a new method
with a default-impl, or a `list_providers(&self) -> Vec<String>` that
returns `vec![]` from the in-memory adapter until overridden).

## Consequences

**Positive:**
- A second repo trait in core establishes the pattern concretely:
  `IdentityProvider` for identity resolution, `CredentialStore` for
  encrypted-credential persistence. Both follow the same shape (core trait
  + in-memory default; persistence adapters additive in separate crates,
  ADR-033).
- The vault stays local-only by construction (ADR-025). The store
  persists `EncryptedData` blobs; the vault decrypts them. The store
  never sees plaintext credentials, never sees the master seed, never
  holds derived keys. The encryption boundary is preserved.
- The no-env-vars invariant (ADR-014) gets a persistence-layer
  counterpart: encrypted credentials persist in a `CredentialStore`, the
  assembly layer loads them into `Capabilities` at registration time, the
  handlers read from `Capabilities` per-request. No `std::env::var` path
  exists at any layer.
- `alknet-http`'s `from_openapi` / `from_mcp` handlers consume the trait
  via `Capabilities` (the assembly layer wires the
  `CredentialStore` → `Capabilities` mapping at registration). The
  handlers don't know whether the credential came from an in-memory map
  or a SQLite file.

**Negative:**
- alknet-core gains a second trait and a default adapter. The dependency
  surface grows by one trait + one struct + one error enum — small, but
  non-zero. The trade is that downstream crates (alknet-http, future
  credential-management UIs) get a shared abstraction instead of each
  rolling their own store shape.
- The `EncryptedData` type is re-stated in core (a thin serializable
  shape mirroring the vault's type). If the vault's `EncryptedData` shape
  changes (a new key version, an additional field), the core shape must
  be kept in sync. The shape is small and stable (ADR-020 locked it), so
  the sync cost is low.
- A future persistence adapter (`alknet-credential-store-sqlite` or
  similar) is additive and not specified here. The trait shape is the
  one-way door; the adapter is a two-way door (ADR-033). Concrete adapter
  shapes are deferred for exploration per the project's note that the
  repo pattern is a tool to reach for, not a one-size-fits-all mold.

## Assumptions

1. **The vault remains the sole encryption boundary.** `CredentialStore`
   persists `EncryptedData` blobs and never decrypts. Decryption is the
   vault's job, local-only (ADR-025). This ADR does not introduce a remote
   decryption path.

2. **`provider: &str` is the key.** Credentials are keyed by provider
   name (`"openai"`, `"anthropic"`, etc.). Multi-credential-per-provider
   (e.g., separate keys for org-A vs org-B under the same provider) is
   not in the trait shape; if needed, an additive `get_scoped(provider,
   scope)` method is the extension path — not a signature change to the
   existing `get` (which is a one-way-door break on the trait).

3. **No `list` method.** The trait is `get` / `put` / `delete`. Adding
   `list` is non-breaking (a default-impl method). See "No `list` method"
   above.

4. **Adapter crates that persist credentials are additive and not
   specified here.** ADR-033 establishes the pattern; the concrete adapter
   shapes are deferred for exploration. This ADR's commitment is to the
   trait shape + the in-memory default, not to any specific backend.

5. **`EncryptedData` in core is a thin serializable mirror of the vault's
   type.** The vault owns the encryption logic and the HD-derivation path
   (ADR-020); core carries only the wire shape. This keeps the vault
   standalone (ADR-018) while letting the store trait reference a concrete
   type.

## References

- ADR-014: Secret Material Flow and Capability Injection (the no-env-vars
  invariant this trait supports)
- ADR-018: Vault as Standalone Crate (the vault has zero alknet-crate
  dependencies; core's `EncryptedData` is a thin mirror, not a vault
  reference)
- ADR-019: Vault Assembly-Layer-Only Access (the assembly layer bridges
  vault → `CredentialStore` → `Capabilities`)
- ADR-020: HD Derivation for Encryption Keys (the `EncryptedData` shape)
- ADR-025: Vault Local-Only Dispatch (the store never decrypts; the vault
  is the sole decryption boundary)
- ADR-033: Storage Boundary and Repo/Adapter Pattern (the overarching
  pattern this ADR follows)
- OQ-34: Persistent Peer Registry (resolved by this ADR + ADR-030 + ADR-033
  — the storage boundary is `config + in-memory adapter` now, persistence
  adapters additive)
- `docs/research/alknet-storage-strategy/findings.md` §4 (the
  `CredentialStore` trait and adapter pattern)
- `/workspace/keypal` — TypeScript repo-pattern reference (Storage
  interface + adapters; the pattern alknet's `CredentialStore` follows)