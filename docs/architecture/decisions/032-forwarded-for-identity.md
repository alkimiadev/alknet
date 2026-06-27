# ADR-032: Forwarded-For Identity (Metadata, Not Authority)

## Status

Accepted (adds a wire-format field and an `OperationContext` field;
included with the ADR-029 migration or a companion task immediately after,
since `OperationContext` and the `from_call` handler are being rewritten)

## Context

When a hub forwards a call to a spoke (via `from_call`, ADR-017), the spoke
authenticates the hub (resolves the hub's identity from the connection)
and checks its ACL: "is the hub authorized to call this operation?" The
spoke's ACL answers yes/no based on the hub's identity. This is per-node
ACL (ADR-029 §3) — the correct authorization model, no "trusted" bypass.

But the spoke is **blind to the originator**. It knows "the hub called me"
but not "alice asked the hub to call me." The hub's `OperationContext.identity`
holds alice's identity (the hub authenticated alice), but the `from_call`
forwarding handler authenticates as the hub (its own `auth_token`), so the
spoke sees the hub's identity, not alice's. The originator information is
at the hub, not at the spoke.

This matters for three use cases the research at
`docs/research/alknet-storage-strategy/findings.md` §6 identified:

1. **Audit trail.** A cross-node call chain is untraceable at the leaf
   without the originator. The spoke logs "the hub called `/docker/start`"
   but can't log "alice asked the hub to call `/docker/start`." For
   debugging, billing, and abuse investigation, the originator matters.

2. **Per-user rate limiting at the leaf.** If the spoke wants to rate-limit
   per-user (not per-hub), or apply per-user quotas, it can't — it only
   sees the hub. The hub would have to proxy and track everything, which
   defeats the point of direct service composition.

3. **Handler context.** A handler may want the originator's identity for
   application logic (per-user views, per-user data isolation, attribution
   in logs).

The question is whether to include the originator's identity in the
forwarded call. The wire format is the constraint: a field is either in the
`call.requested` payload or it isn't — it can't be bolted on later without
a protocol change. This is a wire-format one-way door.

## Decision

### 1. Add `forwarded_for` to the `call.requested` payload

```json
{
  "operationId": "/docker/start",
  "input": { ... },
  "auth_token": "alk_...",           // the direct caller's token (the hub's)
  "forwarded_for": {                 // the original caller (the end user's)
    "id": "alice",
    "scopes": ["fs:read", "docker:start"],
    "resources": {}
  }
}
```

`forwarded_for` is optional (`None` when the call is not forwarded, or
when the forwarder chooses not to propagate it). It carries a serialized
`Identity` (id, scopes, resources) — the originator's resolved identity at
the forwarding node.

### 2. Add `forwarded_for` to `OperationContext`

```rust
pub struct OperationContext {
    // ... existing fields ...

    /// The original caller when this call was forwarded (ADR-032).
    /// Metadata only — NOT used by `AccessControl::check`. The dispatch
    /// path populates it from the `call.requested.forwarded_for` field;
    /// the `from_call` handler sets it when constructing the forwarded
    /// payload (see §3). Handlers may read it for logging, auditing,
    /// per-user rate limiting, or application context. The ACL check
    /// always runs against `identity` (the direct caller), never against
    /// `forwarded_for`.
    pub forwarded_for: Option<Identity>,
}
```

`identity` is the direct caller (authorized by ACL). `forwarded_for` is
the original caller (metadata only). The ACL check signature is
`AccessControl::check(identity.as_ref())` — unchanged. The
`forwarded_for` field is a **separate** field, not a parameter to `check`.

### 3. The `from_call` handler populates `forwarded_for`

The hub's `from_call` forwarding handler constructs the `call.requested`
payload to send to the spoke. It populates `forwarded_for` with the end
user's identity — read from the hub's `OperationContext.identity` (the
caller the hub authenticated) when the hub forwards the call. The hub
authenticates as itself (its own `auth_token`); the `forwarded_for` field
carries the originator's identity as context.

This is the hub's responsibility, not the protocol's. The protocol carries
the field; the `from_call` handler chooses to populate it. A forwarder that
doesn't want to disclose the originator can set `forwarded_for: None` (the
spoke sees only the hub). A forwarder that wants to propagate it sets it.

### 4. `AccessControl::check` never reads `forwarded_for`

The security property: `forwarded_for` is metadata, not authority. The
spoke's dispatch path makes it available on `OperationContext` for handlers,
but `AccessControl::check(identity.as_ref())` — the ACL check — always
authorizes the **direct caller's** identity, never the forwarded-for
identity. There is no path through which `forwarded_for` becomes an
authorization input.

This is enforced structurally, not by convention: `AccessControl::check`
takes `Option<&Identity>` (the direct caller's identity). The
`forwarded_for` field is `Option<Identity>` on `OperationContext`, but
the check signature doesn't accept it. If someone wants to ACL on the
forwarded-for identity, they would have to change the
`AccessControl::check` signature — a visible, reviewable change, not a
quiet flag flip. The type system prevents accidental misuse.

## Why include it now

The window is the ADR-029 migration. The `from_call` handler is being
rewritten (peer-keyed overlays, `AccessControl`-based peer authorization,
removal of `remote_safe`/`trusted_peer`), and `OperationContext` is being
touched (the `PeerCompositeEnv` aggregation changes how the context is
built). Adding a field to the `call.requested` payload and to
`OperationContext` now is the cheapest point — the structures are already
under edit. After the protocol ships without it, adding it is a breaking
wire-format change (every client and server must learn the new field) and
an `OperationContext` break (every handler that pattern-matches the struct
must update).

## Consequences

**Positive:**
- The spoke can audit cross-node call chains. The leaf knows who actually
  initiated the call, not just who forwarded it.
- Per-user rate limiting at the leaf becomes possible. The spoke can key
  rate-limit state on `forwarded_for.id` instead of only on the hub's
  identity.
- Handler application logic can use the originator's identity for per-user
  views, per-user data isolation, or attribution.
- The security model is unchanged: the spoke authorizes the hub (its
  direct caller), not the end user. The `forwarded_for` field is metadata,
  not authority. The type-system separation (`check` takes `identity`, not
  `forwarded_for`) prevents misuse.
- The forwarder decides. A hub that doesn't want to disclose the
  originator (e.g., for privacy, or because the originator's identity is
  not meaningful to the spoke) sets `forwarded_for: None`. The field is
  opt-in by the forwarder, not mandatory.

**Negative:**
- The `call.requested` payload gains a field. Wire-format addition — old
  servers that don't recognize `forwarded_for` ignore it (JSON
  deserialization into a struct without the field drops it); old clients
  that don't send it produce `forwarded_for: None` on the server. This is
  forward-compatible, but a server that wants to *use* `forwarded_for`
  must be new enough to deserialize it.
- `OperationContext` gains a field. Handlers that construct
  `OperationContext` literals (tests, custom dispatch paths) must add the
  field. The composition path (`OperationEnv::invoke`) sets it to `None`
  for composed children — `forwarded_for` is a wire-ingress field, not a
  composition-ingress field.
- The `Identity` in `forwarded_for` is a serialized value on the wire,
  not a server-resolved identity. The spoke receives the hub's *claim*
  about the originator's identity. A malicious hub could lie — set
  `forwarded_for` to a fake identity. The spoke must not treat
  `forwarded_for` as authoritative for anything security-relevant; it's
  the hub's assertion, useful for audit/attribution when the hub is
  trusted, but not a verified identity. This is the inherent property of
  forwarded-for metadata (same as HTTP `X-Forwarded-For` — it's a claim by
  the forwarder, not a verified value).
- One more field for the `from_call` handler to populate correctly. The
  handler must read the hub's `OperationContext.identity` and decide
  whether to propagate it. This is a small implementation cost, but it's a
  handler-responsibility increase.

## Assumptions

1. **`forwarded_for` is a claim by the forwarder, not a verified
   identity.** The spoke receives the hub's assertion about the
   originator. A malicious hub can lie. The spoke must not use
   `forwarded_for` as authoritative for security decisions — only for
   audit, logging, and application-context purposes when the hub is
   trusted. This is the same property as HTTP `X-Forwarded-For`.

2. **`AccessControl::check` never reads `forwarded_for`.** The security
   property is structural (the check signature doesn't accept it), not
   conventional. Adding `forwarded_for` to the ACL path would require a
   signature change to `AccessControl::check` — a visible, reviewable
   change.

3. **`forwarded_for` is wire-ingress only.** Composed children (calls via
   `OperationEnv::invoke`) do not inherit `forwarded_for` — they get
   `None`. The field is populated from `call.requested.forwarded_for` by
   the dispatch path, and the `from_call` forwarding handler sets it when
   constructing the forwarded payload. Composition-propagation of
   `forwarded_for` would be a separate decision (not in this ADR).

4. **The `Identity` shape in `forwarded_for` is the same as `Identity`
   on `OperationContext`.** Both carry `id`, `scopes`, `resources`. The
   `forwarded_for` value is a serialized `Identity` from the forwarding
   node's resolution — the same `Identity` the hub resolved for the end
   user, just passed along as metadata.

## References

- ADR-014: Secret Material Flow and Capability Injection (`forwarded_for`
  carries an `Identity` with scopes/resources, not secret material — the
  no-secret-material-on-the-wire invariant is preserved)
- ADR-015: Privilege Model and Authority Context (the authority-switch
  model — `forwarded_for` does not participate; the direct caller's
  identity is the authority)
- ADR-017: Call Protocol Client and Adapter Contract (the `from_call`
  forwarding handler that populates `forwarded_for`)
- ADR-029: Peer-Graph Routing Model (the migration window —
  `OperationContext` and the `from_call` handler are being rewritten)
- `docs/research/alknet-storage-strategy/findings.md` §6 (the
  forwarded-for identity decision and rationale)