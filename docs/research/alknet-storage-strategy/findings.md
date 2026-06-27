---
status: draft
last_updated: 2026-06-27
---

# Storage and Auth Strategy

**Status**: Draft for iteration
**Date**: 2026-06-27
**Scope**: Cross-cutting — storage decomposition, auth/ACL model, repo pattern,
SQLite+honker as foundation, metagraph as tool. Synthesizes the discussion
that surfaced during the peer-graph routing research (ADR-029) and OQ-33/34
resolution.

This document consolidates a multi-thread discussion into an architectural
strategy for storage and auth in the alknet crate graph. It is not an ADR —
it's the research that will inform ADRs and spec amendments. The
implementation-relevant pieces (the `forwarded_for` field, the
`IdentityProvider`-as-repo framing) get folded into specs after review.

---

## 1. The Problem

Three separate threads converged on the same question: where does persistent
state live in the alknet crate graph, and what's the shared infrastructure
for it?

1. **Peer identity (OQ-33/OQ-34)** — a head node needs to persist the mapping
   from a stable logical peer identity to its current cryptographic material,
   surviving key rotation and restarts. The UUID workaround is ephemeral; the
   real solution is a store.
2. **Filesystem (POC-validated)** — SQLite + honker + iroh-blobs as the
   three-layer stack for path-tree metadata, content-addressed blobs, and
   transactional notify-on-commit. 24 tests across two POC crates.
3. **The old `alknet-storage` spec (alknet-main)** — a single crate doing
   metagraph, identity, ACL, secrets, and honker integration. Designed before
   the vault existed, before ADR-029, before the filesystem POC. Has residual
   issues: multi-tenant complexity, secrets module that's now the vault,
   metagraph-as-foundation rather than metagraph-as-tool.

The common thread: **SQLite via honker is the right local persistence layer
for all three**, and the metagraph model is the right shape for *some* of the
data. The question is how to decompose this so the core crates stay lean
while the storage-dependent crates get what they need — without forcing
everything through the same abstraction.

---

## 2. The Principle: Right Tool for the Right Shape

The metagraph (GraphType → NodeType → EdgeType → Graph → Node → Edge) is a
generalized graph store. It's the right tool for genuinely graph-shaped
problems: ACL delegation chains, workflows, task dependency DAGs, call
composition trees. It is the *wrong* tool for things that aren't graph-shaped:

| Data | Shape | Right tool |
|------|-------|------------|
| Peer identity → crypto material + scopes | Key-value (flat table) | `peers` table with typed columns |
| Filesystem path tree | Tree (degenerate graph) | Specialized path-tree tables (recursive CTE, proven by POC) |
| Provider credentials (encrypted blobs) | Key-value | `credentials` table |
| ACL delegation chains | Graph (traversal, narrowing) | Metagraph |
| Workflows / flowgraph | Graph (DAG, type compatibility) | Metagraph |
| Taskgraph | Graph (dependency DAG) | Metagraph |
| Operation specs | Flat records with typed fields | Table (or in-memory registry, as today) |

Forcing table-shaped data through the metagraph adds overhead (JSON Schema
validation on every node, graph traversal for what should be an indexed
lookup) without benefit. The filesystem POC proved this empirically: the
path tree uses specialized tables with a recursive CTE, and it's sub-
millisecond. The same data in a metagraph would be a graph traversal per
resolve — slower, more complex, no upside.

**The principle: SQLite + honker is the foundation. The metagraph is one
tool built on it, for graph-shaped problems. Direct tables are another tool,
for table-shaped problems. Each consumer picks the right tool.**

---

## 3. SQLite + Honker as Foundation (Pattern, Not Crate)

The filesystem POC established the integration pattern:

```rust
honker_core::apply_default_pragmas(conn)?;      // WAL, synchronous=NORMAL
honker_core::attach_notify(conn)?;              // notify() SQL function
honker_core::attach_honker_functions(conn)?;    // enqueue, claim, lock, stream, cron
honker_core::bootstrap_honker_schema(conn)?;   // queue/stream/scheduler tables
```

This is ~20 lines of setup per consumer. Each consumer that wants its own
tables does this on its own rusqlite connection. The critical property: the
honker functions live on *the same connection* as the data tables, so writes
and notifications are atomic in one transaction (the transactional-outbox
pattern, built in). This is `honker-core` (attach to your connection), not
`honker` (manages its own connection) — the POC documented this distinction.

**This is a pattern, not a crate.** Packaging ~20 lines of setup as a shared
crate adds a dependency boundary for no gain. Each consumer opens its own
SQLite file, attaches honker, defines its schema. A `setup_honker(conn)`
helper function (in a shared utility, or just copy-pasted) is enough.

### Why SQLite, not a "real database"

SQLite is an [application file format](https://sqlite.org/appfileformat.html),
not just a database. The filesystem POC's insight: BLOBs < 100KB are faster
inline in SQLite than as filesystem files; atomic transactions over metadata
independent of content; the schema is the documentation. Each consumer gets
a local, crash-safe, queryable file — not a database server to operate.

The core crates (alknet-core, alknet-call) stay DB-free. The storage-
consuming crates (filesystem, peer registry, graphs) each own their SQLite
file. The assembly layer wires them together.

### What honker adds

| Feature | Use case |
|---------|---------|
| `notify` / `listen` | Ephemeral pub/sub — "ACL entry changed, invalidate cache" |
| `stream_publish` / `subscribe` | Durable pub/sub — "peer identity updated, propagate" |
| `queue` / `claim` / `ack` | Task queue — "orphaned write session cleanup" |
| `lock_acquire` / `lock_release` | Named locks — "writer coordination on a path" |
| `scheduler` | Periodic tasks — "session cleanup, audit log pruning" |

The key integration: every mutation is atomic with its notification. A
`peers` table update + `notify("peers:changed", peer_id)` commit together.
A downstream consumer (e.g., the call protocol's `IdentityProvider` cache)
wakes on commit, not on poll.

---

## 4. The Repo Pattern for Auth

### The existing pattern (make it explicit)

`alknet-core` already has the repo pattern: `IdentityProvider` is a trait
with two methods (`resolve_from_fingerprint`, `resolve_from_token`), one
adapter (`ConfigIdentityProvider`, backed by `ArcSwap<DynamicConfig>`), and
one consumer (the call protocol's `Dispatcher`). This is a repo trait — it
abstracts the *what* (resolve an identity from a credential) from the *how*
(in-memory config, SQLite, Redis, remote service).

**Make this explicit.** `IdentityProvider` is the auth repo trait in core.
Adapters implement it. The assembly layer wires the adapter. Downstream
crates consume the trait, not the adapter.

### Why this matters beyond the call crate

Downstream crates that don't use the call protocol still need auth. A crate
that exposes operations over HTTP (alknet-http) or a service with no protocol
at all still needs to resolve identities and check ACL. If the auth layer is
a repo trait in core, those crates use the same trait, the same adapters, and
potentially the same backing store — without depending on alknet-call. The
call crate is one consumer of auth, not the owner of it.

### The distributed-auth door

If the repo trait is clean, someone can wire an adapter that syncs via
automerge (like the filesystem POC's path-tree CRDT), a Redis adapter, or a
remote-service adapter. The trait doesn't care. Auth data that isn't storing
sensitive details (unless encrypted) could be distributed via the same
patterns the filesystem uses for its path tree. This isn't designed here —
it's a door the repo pattern opens by not foreclosing it.

### Reference: kepal

The TypeScript project [kepal](/workspace/keypal) is a clean example of this
pattern. It abstracts API key management (hashing, validation, scopes,
expiration, caching) with a `Storage` interface and adapters for Redis,
Drizzle, Prisma, Kysely, Convex, and in-memory. The core logic
(`Manager`) is backend-agnostic; the storage is a trait; the consumer picks
the adapter at wiring time. An `AdapterFactory` provides column-mapping /
schema-config so the same adapter works against different table schemas.

The alknet equivalent: `IdentityProvider` is the trait (like kepal's
`Storage`), `ConfigIdentityProvider` is the in-memory adapter (like kepal's
`MemoryStore`), the SQLite peer registry is the real adapter (like kepal's
`RedisStore`/`DrizzleStore`), and the assembly layer wires the adapter (like
kepal's `Manager` constructor). The shapes map cleanly.

### PeerStore: adapter-internal, not core

A `PeerStore` trait (save/find/update/delete peer records) is an
*adapter-internal* detail, not a core trait. The core trait is
`IdentityProvider`. The SQLite adapter implements `IdentityProvider` by
delegating to a `PeerStore` internally. The trait boundary that matters for
cross-crate sharing is `IdentityProvider`, not `PeerStore`.

This keeps core lean: one auth trait (`IdentityProvider`), not two. The
store trait lives in the adapter crate (or the assembly layer), where it's
an implementation detail. If a future adapter (Redis, remote service) needs
a different internal store shape, it's free to define one — the core contract
is `IdentityProvider`, not the store.

---

## 5. Per-Node ACL, No "Trusted" Flag

### The model

Each node has its own ACL. A node's ACL answers one question: **is this
caller authorized to call this operation?** The caller is whoever
authenticated to the connection — resolved by `IdentityProvider` from the
TLS fingerprint or `auth_token`, checked by `AccessControl::check(identity)`.
No "trusted" flag, no bypass, no special mode.

This is the existing mechanism, restated for the cross-node case. The call
protocol's dispatch path (`registration.rs:128-140`) already runs
`AccessControl::check` against the caller's `Identity`. For a remote peer's
call, the caller's `Identity` is the peer's resolved identity. Same check,
same mechanism, no new concept.

### Why no "trusted=true"

A generic "trusted" flag is a blanket authorization bypass — the exact
anti-pattern that ADR-015 was written to kill (it replaced `trusted: true`
with the authority-switch model). There is no circumstance where a generic
"skip the security check" flag is the right answer in a reasonably secure
system. If a caller is authorized, the ACL says so. If the ACL doesn't say
so, the caller isn't authorized. There's no third state.

### The cross-node case

When a hub forwards to a spoke (via `from_call`), the spoke authenticates
the hub (resolves the hub's identity from the connection), and checks its
ACL: "is this identity authorized to call this operation?" The answer is
yes or no, based on the hub's identity and the op's `AccessControl`. Same
mechanism, same check, no special-casing.

```
End user ──calls──> Hub ──forwards as hub──> Spoke (docker service)
           │                    │
     hub's ACL             spoke's ACL
     (user → hub ops)       (hub → spoke ops)
```

The hub's ACL checked the end user. The spoke's ACL checked the hub. Two
independent authorization decisions, same mechanism, no replication. The hub
isn't "trusted" by the spoke — the hub is *authorized* by the spoke's ACL,
the same way any caller is authorized.

### The service-to-service pattern

This is the same principle as: a database server authorizes the application
server; it doesn't need to know about every end user the app server
authenticated. The application server is the authorization boundary. In
alknet, each node is an authorization boundary for its direct callers.

The docker service example: the service exposes `/docker/start`. It's
reachable directly (end users connect and call it) or through a hub (the
hub imports via `from_call`, re-exposes, forwards). The docker service's
ACL lists the principals that call it directly — either end users (direct
topology) or the hub (proxied topology). It doesn't need to know about the
hub's end users. The hub's ACL handles end-user authorization.

### No global ACL, no replication

Each node's ACL is local — in its own SQLite file (when storage arrives), in
its own `peers` table, checked by its own `AccessControl`. There is no
global ACL, no cross-service ACL replication. When a user's key rotates, the
hub's `peers` table updates her fingerprint. The spoke's `peers` table is
unchanged — it only knows about the hub. When the hub's key rotates, the
spoke's `peers` table updates the hub's fingerprint — a single entry update,
not a full ACL replication.

### The "many DBs" concern

Having many SQLite files (one per node, one per concern) looks like the
microservices ACL-replication mess. It isn't, because the trust model is
per-node: each node only authorizes its direct callers. The DBs don't
overlap. The mess only happens if you try end-to-end identity propagation
(the spoke needs to know about every end user) — that's the anti-pattern,
and the repo pattern + per-node ACL avoids it.

---

## 6. Forwarded-For Identity (Metadata, Not Authority)

### The question

When a hub forwards a call to a spoke, should the spoke know *who initiated
the call* (the end user), or just *who called it* (the hub)?

**Without forwarded-for** (what the implementation does today): the spoke
sees the hub as the caller. It authorizes the hub. It logs "the hub called
`/docker/start`." If the spoke needs to audit "who actually initiated this,"
it can't — that information is at the hub.

**With forwarded-for**: the hub includes the original caller's identity in
the `call.requested` payload. The spoke can log it, use it for per-user
quotas, or pass it to the operation handler for context. But the spoke's ACL
still authorizes the *hub*, not the end user — the forwarded-for identity is
informational, not authoritative.

### The recommendation: add it, as metadata

The forwarded-for identity should be added as a protocol-level field, not
as an afterthought. Reasoning:

1. **Audit trail.** Without it, a cross-node call chain is untraceable at
   the leaf. The spoke knows "the hub called me" but not "alice asked the
   hub to call me." For debugging, billing, and abuse investigation, the
   originator matters.

2. **It's metadata, not authority.** The forwarded-for identity goes in the
   call's metadata (or a dedicated `forwarded_for` field), not as the
   `auth_token`. The spoke's dispatch path makes it available on
   `OperationContext` but `AccessControl::check` *never* uses it — it
   always authorizes the direct caller's identity. This keeps it from
   becoming an authorization bypass.

3. **The ACL check signature prevents misuse.** `AccessControl::check` takes
   `Option<&Identity>` (the direct caller's identity). `forwarded_for` is a
   *separate* field on `OperationContext` (`Option<Identity>`). The ACL
   check signature doesn't accept it. If someone wants to ACL on the
   forwarded-for identity, they'd have to change the `AccessControl::check`
   signature — a visible, reviewable change, not a quiet flag flip.

4. **Without it, the leaf service is blind to the originator.** If the spoke
   needs to rate-limit per-user (not per-hub), or log who triggered a
   container start, it can't. The hub would have to proxy and track
   everything, which defeats the point of direct service composition.

### Protocol shape

The `call.requested` payload gains an optional `forwarded_for` field:

```json
{
  "operationId": "/docker/start",
  "input": { ... },
  "auth_token": "alk_...",           // the direct caller's token (the hub's)
  "forwarded_for": {                 // the original caller (the end user's)
    "id": "alice-fingerprint",
    "scopes": ["fs:read", "docker:start"]
  }
}
```

The dispatch path populates `OperationContext`:
```rust
pub struct OperationContext {
    // ... existing fields ...
    pub identity: Option<Identity>,              // the direct caller (authorized by ACL)
    pub forwarded_for: Option<Identity>,         // the original caller (metadata only)
}
```

`AccessControl::check(identity.as_ref())` — unchanged. The `forwarded_for`
field is available to handlers for logging, auditing, rate-limiting, but
never to the ACL.

### The `from_call` handler's responsibility

The hub's `from_call` forwarding handler populates `forwarded_for` with the
end user's identity (from the hub's `OperationContext.identity`) when it
constructs the `call.requested` payload to send to the spoke. The hub
authenticates as itself (its own `auth_token`); the `forwarded_for` field
carries the originator's identity as context.

This is a protocol addition — a field on the `call.requested` payload and
on `OperationContext`. It's in or it's out; it can't be bolted on later
without a protocol change. The recommendation is to include it from the
start.

---

## 7. The Decomposition

### Crate boundaries

```
alknet-core (lean — no SQLite, no honker)
├── IdentityProvider trait          (the auth repo trait — already exists)
├── Identity, AuthToken, AuthContext (the auth types — already exist)
├── AccessControl, AccessResult      (the ACL check — already exists)
└── (no PeerStore trait — adapter-internal, not core)

Storage-consuming crates (each owns its SQLite + honker):
├── alknet-filesystem     — path-tree tables (tree, not graph; POC-proven)
├── peer registry         — peers table (KV; implements IdentityProvider)
├── provider credentials  — credentials table (KV; encrypted by vault)
└── alknet-graphs (future) — metagraph tables (graph-shaped problems)

alknet-call (lean — no SQLite, no honker, no storage traits)
├── Uses IdentityProvider (the trait, not the adapter)
├── PeerCompositeEnv keyed by PeerId (= Identity.id from IdentityProvider)
├── AccessControl::check(identity) for per-node ACL
└── from_call handler authenticates as the hub, forwards-for as metadata
```

### What goes where

| Concern | Where it lives | Shape |
|---------|---------------|-------|
| Auth repo trait (`IdentityProvider`) | alknet-core | Trait (already exists) |
| Auth adapters (Config, SQLite, future Redis/remote) | Adapter crates or assembly layer | Implements `IdentityProvider` |
| Per-node ACL check (`AccessControl::check`) | alknet-core (already exists) | Table-shaped: scope/resource match |
| Peer identity storage (PeerStore) | Adapter crate (adapter-internal) | `peers` table |
| Filesystem path tree + bucket ACL | alknet-filesystem | Specialized tables (POC-proven) |
| Provider credentials (encrypted) | Adapter crate or assembly layer | `credentials` table (vault encrypts) |
| ACL delegation graph (future) | alknet-graphs (metagraph) | Graph (traversal, scope narrowing) |
| Workflows / flowgraph (future) | alknet-graphs (metagraph) | Graph (DAG) |
| Taskgraph (future) | alknet-graphs (metagraph) | Graph (dependency DAG) |
| Forwarded-for identity | alknet-call (protocol field) | Metadata on `call.requested` + `OperationContext` |

### What the old spec had that we're dropping

| Old spec | Status | Why |
|----------|--------|-----|
| Multi-tenant (system.db + tenant.db) | Dropped | Each tenant gets its own complete setup (own ACL, ops, DB). Simpler, no cross-tenant complexity. |
| `secrets/` module (HD derivation, secret service) | Replaced by alknet-vault | The vault already handles encryption/decryption (ADR-018/019/020/025/026). Storage just stores the `EncryptedData` blob. |
| Metagraph as the foundation | Demoted to tool | SQLite+honker is the foundation. Metagraph is one tool on it, for graph-shaped problems. Tables are another tool, for table-shaped problems. |
| `alknet-storage` as one crate | Split | The storage-consuming concerns are separate (filesystem, peer registry, graphs). No single "storage" crate. |
| Accounts/organizations/multi-tenant identity | Deferred | The v1 need is a `peers` table (PeerId → fingerprint + scopes). The full account/org model is a future adapter. |
| `alknet-flowgraph` as a separate crate | Folded into alknet-graphs | The metagraph + petgraph interop are one crate for graph-shaped problems. |

---

## 8. The ACL Split: Check Stays Table, Delegation Is Graph

### The current ACL is table-shaped

`AccessControl` on `OperationSpec` is `required_scopes` (AND-gate),
`required_scopes_any` (OR-gate), `resource_type`/`resource_action`. `Identity`
has `scopes: Vec<String>` and `resources: HashMap<String, Vec<String>>`. The
check is `AccessControl::check(identity)` — a flat scope-match, not a graph
traversal. This is fast, indexable, and correct for the current model (no
delegation).

### Delegation is graph-shaped (future)

When delegation is needed ("A delegates to B with narrowed scopes, B
delegates to C with further narrowing"), the delegation chain is a graph
traversal — you walk the chain computing the effective scope set. This is
where the metagraph pays off (PrincipalNode, DelegatesEdge, scope narrowing).

But the *check* stays table-shaped even with delegation: the delegation
graph produces the effective `Identity.scopes` (the graph's output); the ACL
check is still "does the effective scope set satisfy the op's requirements?"
(a flat join). The graph and the table compose — the graph produces the
scopes, the table checks them.

### Don't force the check through the graph

The temptation is to make `AccessControl::check` traverse the delegation
graph. Don't. The check is a flat scope-match — keep it that way. The
delegation graph is a separate concern (producing effective scopes), and it
lives in `alknet-graphs` (metagraph). The check lives in core (table). They
compose at the `IdentityProvider` boundary: the adapter resolves the identity
(possibly by traversing the delegation graph to compute effective scopes),
returns an `Identity` with the effective scopes, and the check is a flat
match against that `Identity`.

This matches the "don't use a screwdriver to hammer a nail" principle: the
check is table-shaped, the delegation is graph-shaped, and forcing either
through the other's shape is worse.

---

## 9. The Hub Proxy Tangle (Resolved)

### The tangle

A hub can "have a filesystem" two ways:
1. **In-process** — the hub's binary loads `alknet-filesystem`. The
   filesystem's SQLite is local. The hub's call protocol dispatches
   `/fs/readFile` directly to the filesystem handler. No network.
2. **Proxied** — the filesystem runs on a spoke. The hub imports the spoke's
   ops via `from_call`. The hub's `from_call` handler forwards over QUIC.
   The spoke's call protocol dispatches to its own filesystem handler.

These are different deployment topologies for the same libraries. The
libraries don't change; the assembly does.

### The three concerns that got conflated

1. **ACL** — who can call the operation? The hub's ACL authorizes the user.
   The spoke's ACL authorizes the hub. (Per-node ACL, same mechanism.)
2. **Bucket routing** — which bucket is the operation targeting? The bucket
   is a *parameter* in the operation input (`{ "bucket": "alice-files",
   "path": "hello.txt" }`). It's not an ACL concern — it's operation input.
3. **Peer routing** — which spoke *hosts* the operation? This is
   `PeerRef::Specific` (ADR-029) — the hub's composition env routes to the
   right peer.

These are three separate decisions at three separate layers:

```
User calls hub's /fs/readFile with { bucket: "alice-files", path: "hello.txt" }
  → hub's ACL: is this user authorized to call /fs/readFile? (AccessControl::check)
  → hub's composition env: which peer serves /fs/readFile? (PeerRef routing)
  → hub's from_call handler: forward { bucket, path } to that peer
  → spoke's ACL: is the hub authorized to call /fs/readFile? (AccessControl::check)
  → spoke's filesystem handler: read path from bucket (operation logic + bucket ACL)
```

### Bucket-level authorization

The call protocol's ACL is coarse: "can this identity call `/fs/readFile`?"
It doesn't know about buckets. The bucket is in the operation input. The
**handler** checks bucket-level authorization — the filesystem handler reads
`ctx.identity`, reads the input's `bucket` field, and checks its own bucket
ACL (a `bucket_acl` table in the filesystem's SQLite: "is this identity
authorized for this bucket?"). This is application logic — the filesystem
owns its bucket authorization. The call protocol's ACL is the coarse gate;
the handler is the fine gate.

This keeps the call protocol's ACL simple and fast (a scope/resource check),
and lets each service define its own fine-grained authorization against its
own storage. The ACL doesn't inspect operation input; the handler does.

---

## 10. What This Means for the Immediate Path

### ADR-029 migration (now)

The peer-graph routing migration uses the UUID workaround (no storage). This
document doesn't change that. But it establishes the pattern for when
storage arrives:

1. **ADR-029 migration** (now) — UUID PeerId, no storage, in-memory peer
   overlays. `IdentityProvider` is `ConfigIdentityProvider` (in-memory).
2. **Peer registry** (when key rotation / durable peer attribution is
   needed) — `peers` table + honker, implements `IdentityProvider`, replaces
   `ConfigIdentityProvider`. The call protocol's `Dispatcher` uses
   `IdentityProvider` as today — no change. The `PeerCompositeEnv` uses
   `PeerId` (= `Identity.id` from the adapter) — no change to routing.
3. **alknet-graphs** (when ACL delegation / workflows / taskgraph are
   needed) — metagraph crate, built on the same SQLite+honker pattern. For
   graph-shaped problems only.

Each step is independent. The migration doesn't wait for storage. Storage
doesn't wait for the metagraph. The metagraph doesn't wait for the filesystem
(which already has its own tables).

### What goes into specs next (after this doc is reviewed)

1. **`IdentityProvider` as the auth repo trait** — make the repo framing
   explicit in `auth.md` and the `IdentityProvider` doc. No trait change;
   just documenting the pattern.
2. **`forwarded_for` field** — add to `call-protocol.md` (the
   `call.requested` payload schema) and `operation-registry.md`
   (`OperationContext`). `AccessControl::check` signature unchanged.
3. **Per-node ACL framing** — add to `client-and-adapters.md` and
   `operation-registry.md` as the cross-node extension of the existing
   `AccessControl` model. No "trusted" flag.
4. **OQ-34 update** — record the repo-pattern framing and the decomposition
   (SQLite+honker as pattern, metagraph as tool, `IdentityProvider` as the
   core trait).

### What does NOT go into specs (stays in this research doc)

- The metagraph schema (GraphType/NodeType/EdgeType) — that's a future
  `alknet-graphs` spec, not relevant to the current crates
- The filesystem's path-tree schema — that's the filesystem crate's spec
- The full account/org identity model — deferred; the v1 need is a `peers`
  table
- The distributed-auth adapter (automerge/Redis) — a door the repo pattern
  opens; not designed

---

## 11. Open Questions

1. **When does the `forwarded_for` field get added?** It's a protocol
   addition (a field on `call.requested` and `OperationContext`). It's in
   the ADR-029 migration or it's a separate protocol-change task. The
   recommendation is to include it in the migration — the `from_call`
   handler is being rewritten anyway, and the `OperationContext` struct is
   being touched. Adding the field now is cheaper than a separate protocol
   change later.

2. **Does the peer registry adapter live in its own crate or in the assembly
   layer?** The `ConfigIdentityProvider` lives in alknet-core (a simple
   impl). The SQLite adapter could live in a `alknet-peer-store-sqlite`
   crate, or it could be in the assembly layer's binary (like a wiring
   detail). The kepal pattern suggests a separate crate (the adapter is
   reusable across deployments). This is a two-way door — the trait is in
   core either way; the adapter's location is a packaging choice.

3. **Does the ACL delegation graph (future) produce `Identity.scopes` at
   resolution time or at check time?** The recommendation in §8 is at
   resolution time (the `IdentityProvider` adapter traverses the delegation
   graph to compute effective scopes, returns an `Identity` with them, and
   the check is flat). But an alternative is lazy computation (the check
   triggers the traversal). This is a future question, not a v1 decision —
   the current model has no delegation.

---

## References

- ADR-014: Secret Material Flow and Capability Injection (the no-env-vars
  invariant)
- ADR-015: Privilege Model and Authority Context (the authority-switch model
  that replaced `trusted: true`)
- ADR-017: Call Protocol Client and Adapter Contract (the `from_call`
  forwarding handler)
- ADR-018/019/020/025/026: The vault crate (handles encryption/decryption;
  storage stores the `EncryptedData` blob)
- ADR-029: Peer-Graph Routing Model (peer-keyed overlays, `PeerRef` routing,
  `AccessControl`-based peer authorization)
- OQ-33: PeerId — logical id, not crypto identity
- OQ-34: Persistent peer registry (the storage dimension)
- `docs/research/alknet-call-peer-routing/findings.md` — the peer-graph
  routing research that surfaced the storage question
- `docs/research/alknet-filesystem/poc-summary.md` — the filesystem POC that
  validated SQLite + honker + iroh-blobs
- `/workspace/@alkdev/alknet-main/docs/architecture/storage.md` — the old
  storage spec (residual issues documented in §7)
- `/workspace/@alkdev/alknet-main/docs/research/storage.md` — the old storage
  research (metagraph, identity, ACL, honker integration)
- `/workspace/keypal` — TypeScript repo-pattern reference for API key
  management (Storage interface + adapters, the pattern alknet's
  `IdentityProvider` follows)
- `/workspace/honker` — SQLite extension with pub/sub, streams, queues,
  locks, scheduler (`honker-core` for the attach-to-your-connection pattern)
- https://sqlite.org/appfileformat.html — SQLite as an application file format