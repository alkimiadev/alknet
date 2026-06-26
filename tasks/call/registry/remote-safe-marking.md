---
id: call/registry/remote-safe-marking
name: Add remote_safe field to HandlerRegistration for CallClient peer-scoped filtering (ADR-028)
status: pending
depends_on: []
scope: narrow
risk: medium
impact: isolated
level: implementation
---

## Description

Add the `remote_safe: bool` field to `HandlerRegistration` (and its builder) so
that a `CallClient` can default-deny which operations it exposes to a remote
peer. This is the v1 shape of the peer-scoped filtering mechanism locked by
ADR-028. It is the prerequisite for `call/client/call-client` (the
`CallClient`'s dispatch path reads this field) and is the only one-way-door
piece of the call-completion work, so it goes first.

### Field

```rust
pub struct HandlerRegistration {
    pub spec: OperationSpec,
    pub handler: Handler,
    pub provenance: OperationProvenance,
    pub composition_authority: Option<CompositionAuthority>, // None for leaves
    pub scoped_env: Option<ScopedOperationEnv>,               // None for leaves
    pub capabilities: Capabilities,
    pub remote_safe: bool,   // default false; ADR-028 — exposes this op to
                             // CallClient peers (trusted-peer mode bypasses)
}
```

`remote_safe` defaults to `false` across **all** provenance (Local, Session,
and the leaf provenances — see ADR-028 §4). The operator flips specific
operations to `true` when they want a peer to reach them. This mirrors the
default-deny posture of ADR-015 (visibility `Internal` by default) and
ADR-022 (composition authority `None` for leaves by default).

### Builder

`OperationRegistryBuilder` needs a way to set `remote_safe`:

- `.with_local(...)` / `.with_leaf(...)` / `.with(...)` should default
  `remote_safe: false` (current call sites stay valid unchanged).
- Add a chainable setter, e.g. `.remote_safe(true)` on the builder or a
  `with_local_remote(...)` / explicit-arg variant. The exact builder API shape
  is a two-way door — pick the least invasive (an optional trailing arg or a
  builder setter method); do not over-engineer per-peer allowlists (that's
  OQ-25's two-way-door remainder, explicitly out of scope here).

### services/list interaction (ADR-028 Assumption 2)

`services/list` already filters by `Visibility::External` (ADR-015). Per
ADR-028 Assumption 2, when served to a `CallClient` peer, `services/list` must
**additionally hide** non-remote-safe ops — a peer should not see ops it
cannot call, so discovery and dispatch filters agree. The
`services_list_handler` in `registry/discovery.rs` currently filters only on
`visibility`. 

**Scoping note**: the `services/list` handler doesn't know whether the caller
is a `CallClient` peer or a local process. The v1 implementation: the filter
applied by `services/list` is the *registry's* filter, and the peer-scoped
*view* a `CallClient` exposes is built atop this. The cleanest v1 split is:

- `services/list` keeps filtering by `Visibility::External` (unchanged).
- The `CallClient`'s peer-scoped view (task `call/client/call-client`) is a
  dispatch-time read that additionally filters by `remote_safe`, and the
  `CallClient`'s *own* `services/list` serving (when it receives
  `services/list` from the remote peer) hides non-remote-safe ops.

So this task adds the **field + builder setter + provenance defaults**, and
the *filtering behavior* that consumes the field is wired in
`call/client/call-client` (the dispatch path) and — for the
`services/list`-hides-non-remote-safe behavior — in the `CallClient`'s
serving path. **This task only adds the data and defaults**, plus a unit
test that the field defaults to `false` and that the setter flips it. Keep
this task tightly scoped: adding the field must not change any existing
dispatch behavior (the field is read-only by the CallClient layer added
later).

## Acceptance Criteria

- [ ] `HandlerRegistration` has `pub remote_safe: bool` field
- [ ] All existing `with_local` / `with_leaf` / `with` builder call sites
      compile unchanged (default `remote_safe: false`)
- [ ] A builder setter exists to set `remote_safe: true` (e.g.
      `.remote_safe(true)` or an explicit-arg variant)
- [ ] Provenance-aware defaults: `Local`, `Session`, `FromOpenAPI`, `FromMCP`,
      `FromCall`, `FromJsonSchema` all default to `false` (ADR-028 §4)
- [ ] No existing dispatch path behavior changes (field is data-only here;
      the CallClient filter that reads it is a later task)
- [ ] `services/list` handler is unchanged in this task (filtering wired later)
- [ ] Unit test: `HandlerRegistration` default has `remote_safe == false`
- [ ] Unit test: builder setter produces `remote_safe == true`
- [ ] Unit test: all six provenance variants default `remote_safe == false`
- [ ] `cargo test -p alknet-call` succeeds
- [ ] `cargo clippy -p alknet-call --all-targets` succeeds with no warnings

## References

- docs/architecture/decisions/028-callclient-peer-scoped-registry-filtering.md — ADR-028 (the one-way door; §2 field location, §4 provenance defaults, Assumption 2 services/list hide)
- docs/architecture/crates/call/operation-registry.md — HandlerRegistration struct sketch (now shows `remote_safe`)
- docs/architecture/crates/call/client-and-adapters.md — CallClient § (consumes the field; trusted-peer bypass)
- docs/research/alknet-call-completion/gap-analysis.md — DC-1

## Notes

> This is the one-way-door piece of the call-completion work: the *existence*
> of default-deny filtering is locked by ADR-028; this task adds only the v1
> data shape (`remote_safe: bool`) and defaults. The richer per-peer shape
> (allowlist, capability-class tag) is explicitly out of scope — it's the
> two-way-door remainder tracked as OQ-25. Do not implement per-peer logic
> here. The filtering *behavior* (dispatch path + services/list hide) is wired
> in `call/client/call-client`, not here — this task is data + defaults only
> so it can land first and unblock the CallClient task.