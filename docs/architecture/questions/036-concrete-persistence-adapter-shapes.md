# OQ-36: Concrete Persistence Adapter Shapes

- **Origin**: ADR-033 §"What this does NOT do" (concrete adapter shapes not
  specified), the project's note that the repo pattern is a tool to reach
  for, not a one-size-fits-all mold
- **Status**: **resolved** (2026-06-28 by ADR-035)
- **Door type**: Two-way (adapter shapes are implementation details;
  the trait shapes are the one-way doors, already committed by ADR-030/031/033)
- **Priority**: medium → resolved
- **Resolution**: **[ADR-035](decisions/035-concrete-persistence-adapter-shapes.md)
  commits the concrete adapter shape.** The design is driven by two
  constraints: the hot-path read trait (`IdentityProvider::resolve_from_
  fingerprint`, `CredentialStore::get`) is **sync** (called in the
  accept loop, no `.await`), and auth changes must take effect **without
  a restart** (an early issue the project already fixed for
  `ConfigIdentityProvider` via `ArcSwap` config reload).

  The resolution:
  - **Read trait stays sync; persistence adapters cache in memory.** A
    SQLite-backed adapter serves sync reads from an in-memory index
    (`HashMap<fingerprint, PeerEntry>` / `HashMap<String, EncryptedData>`),
    loaded from SQLite at construction and refreshed on honker `NOTIFY`.
    Same `ArcSwap`-backed full-reload pattern as `ConfigIdentityProvider`,
    generalized from "config file is source of truth" to "SQLite is
    source of truth, honker signals when it changed."
  - **New async `IdentityStore` write trait** (`put_peer` / `update_peer`
    / `remove_peer`) extends `IdentityProvider` for peer mutations.
    `ConfigIdentityProvider` does NOT implement it (config reload is its
    write path); the SQLite adapter does. The read trait stays lean;
    the write surface is opt-in.
  - **`CredentialStore::put`/`delete` become async** (refines ADR-031's
    sync sketch — within the one-way door ADR-031 committed; `get` stays
    sync/cached). `InMemoryCredentialStore`'s write methods are
    async-with-no-awaits (signature change only).
  - **honker is the cache-invalidation mechanism** — a hard dependency of
    `alknet-store-sqlite`, NOT of `alknet-core`. honker's SQLite
    `NOTIFY`/`LISTEN` (single-digit-ms wake, no polling) is what makes
    the sync-read + cached-index + no-restart combination work. Without
    it, the adapter either polls (stale window) or requires restart
    (the bug already fixed). Not optional for the SQLite adapter.
  - **`alknet-store-sqlite`** — one crate, both adapters
    (`SqliteIdentityProvider: IdentityProvider + IdentityStore`,
    `SqliteCredentialStore: CredentialStore`), shared SQLite connection
    pool + honker LISTEN loop + bootstrap migrations. Splitting into
    two crates later is a two-way door (additive).
  - **Schema shape committed** (one row per `PeerEntry` with JSON
    columns for `fingerprints`/`scopes`/`resources`; one row per
    `EncryptedData` blob keyed by `provider`); exact DDL is an
    implementation-detail two-way door in the adapter crate.
  - **Shared `StoreError`** (`#[non_exhaustive]`, `thiserror::Error`)
    in alknet-core for both adapters.

  The keypal adapter-factory pattern is **intentionally not ported** to
  Rust (runtime column-mapping/type-coercion is a TS affordance; in
  Rust each adapter is a concrete type, cross-cutting concerns are a
  shared helper module). Two trait families (not one generic
  `Storage<T>`) preserved per ADR-033 §4. Redis / Postgres / on-chain
  adapters are **not needed for current scope** — the trait shapes
  make them possible; the adapter crates get built when a use case
  forces them.
- **Cross-references**: ADR-004, ADR-011, ADR-014, ADR-020, ADR-025,
  ADR-030, ADR-031, ADR-033, ADR-035, OQ-33, OQ-34,
  [auth.md](crates/core/auth.md), [config.md](crates/core/config.md)
