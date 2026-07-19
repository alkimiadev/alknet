---
status: draft
last_updated: 2026-07-19
---

# call-channels-unification — Findings: openable ALPNs are operations

**Status:** Draft findings, iterating. Per the research-then-sync
pattern (see `docs/research/stream-unification/findings.md` for the
precedent), this doc iterates in `docs/research/`; we fix
inter-document drift here, then sync to `docs/architecture/` and the
ADRs only after it settles.

**Scope:** The control plane for channels-served ALPNs — how
`channel/open` is authorized, how per-ALPN ACLs are expressed, how the
quota lifecycle is accounted, and how the ALPN categorization from
ADR-086 reframes under "an openable ALPN is an operation." This is
*above* the channels wire format (ADR-071/093, settled) and *below*
the ALPN handler's data-plane protocol (each handler's own wire
format). The transport leaf (`BiStream` as the handler-facing duplex
type) is settled in ADR-092 and is not re-litigated here. The
channels wire format (8-byte header, no `stream_type`) is settled in
ADR-093 and is not re-litigated here.

**Date:** 2026-07-19

**Origin:** An outside review surfaced three high-value gaps in the channels control plane
after ADR-094 (per-identity channel cap) landed. Working through the
first gap — `channel/open` ACL granularity — surfaced a larger
unification: channels is "call + data channels" ("call++"), and the
ALPN crates served under channels are call-consuming apps in the same
shape alknet-docker is a call-consuming app. This doc records both the
gaps and the unification.

---

## TL;DR

The previous framing — "channel lifecycle goes through one generic
`channel/open` operation, and `AccessControl::check` on that op is
the ACL" — was a symptom. The actual question is the separation of
concerns between the call protocol (which already has per-op ACL,
identity, composition, ownership) and the channels layer (which routes
bytes by `channel_id`), and the resolution is **an openable ALPN is an
operation**: each openable ALPN registers its own open op
(`channels/tty/open`, `channels/tunnel/open`, etc.) on the call
`OperationRegistry`, with its own `access_control`, `input_schema`,
`resource_id_path`, and a `channel_open` marker that tells
channels-call "this op produces a data channel."

This dissolves the `channel/open` ACL granularity gap (each ALPN has
its own ACL — checked by the existing `OperationRegistry::invoke`
before the handler runs, like every other op), makes the
`resource_id_path` from ADR-050 work for channel-open ops (the path
is per-op, not per-ALPN-branch-of-a-single-op), and lets the two
`direction` values (initiator-to-responder vs responder-to-initiator)
become two verbs (`channels/<alpn>/open` and
`channels/<alpn>/expose`) with separate ACLs. `open` is specced;
`expose` is reserved in the enum and deferred until a concrete push
use case forces the design. The verb split's real win is enabling
`open`-only shipping: the old `direction` field forced both semantics
into the v1 wire contract.

The ALPN crates served under channels (tty, tunnel, socks5, fs, sftp)
stop being "just ALPNs" and become **call-consuming apps with a
data-channel data plane** — the same shape as alknet-docker (a
call-consuming app with a JSON data plane), but where some ops
produce a data channel alongside the JSON response. We call this
"call++" — channels is call + data channels. The hub/worker
composition story unifies: a hub composes call apps, full stop; some
ops return JSON, some ops carry the `channel_open` marker and
produce a data channel.

Three further gaps fall out of this framing:

1. **Quota lifecycle leaks (ADR-094 amendment).** The per-identity
   cap's `on_close` is called from the `channel/close` handler with
   `op_ctx.identity` (the closer, not the opener) and is not called
   at all on transport drop (REQ-CH-02 clears the channel map without
   touching the policy). A peer whose connection dies at cap is
   permanently at cap — a self-DoS. Fix: decouple the decrement from
   `channel/close` and tie it to channel-state deallocation on the
   side that counted, via a per-connection opener ledger in
   `channels-call` (keeping `channels-core` auth-blind) decremented
   on every teardown path.

2. **Connection-count DoS is an unowned layer (new OQ against
   `alknet-endpoint`).** The per-identity cap bounds channels, but
   each transport connection still costs a channel-0 buffer, a
   `CallAdapter`, and demux tasks before any `channel/open` — and
   ADR-094 explicitly says connections are unbounded. This is not a
   channels-ACL problem; it belongs at the endpoint/accept layer
   (per-identity connection cap, analogous shape to
   `ChannelLifecyclePolicy`). No doc owns it today. Naming it as a
   separate layer stops it from re-tangling every channels-ACL
   conversation.

3. **ALPN category reframe (ADR-086 amendment).** ADR-086 §4 split
   the foundational handlers into "channels data-channel ALPNs" and
   "SSH (endpoint ALPN wrapping channels)." Under "call++", the first
   category is reframed: they're not "ALPNs gated by channels" —
   they're **call apps that produce data channels**. They inherit
   call's auth/composition/identity by construction (they *are* call
   apps); the data-channel part is the `channel_open` marker on the
   op spec. The "flat ALPN list" re-implementation problem dissolves
   because they're call apps, not a separate category that each
   re-implements auth. SSH stays distinct (endpoint ALPN wrapping
   channels).

**No production/backward-compat constraint.** The develop branch is a
rewrite of main (pre-alpha). The decision is purely "what's
cleanest," not "what's least disruptive." ADR-073's four op names are
declared one-way doors, but there's no code and no deployments;
ADR-093/094 just demonstrated that amendment is the normal mode here.
This is the cheapest moment to amend.

---

## Terminology (three axes, not one)

The doc uses three independent role axes. They overlap in common cases
but are not the same thing — a hub can be a responder on one leg and
an initiator on another, a producer on one channel and a consumer on
another.

| Axis | Roles | What it means |
|------|-------|---------------|
| **Deployment** | hub, worker (spoke), browser | Where the code runs. Hub relays; worker/spoke hosts resources; browser is the end-user client. |
| **Call protocol** | initiator, responder | Who sends the call op. The initiator calls `channels/tty/open`; the responder receives it. |
| **Data plane** | producer, consumer | Who runs the `ProtocolHandler` (produces the data stream) vs who receives it. The producer is the side that spawns the handler on the `BiStream`. |

The old "ALPN-server"/"ALPN-client" vocabulary is retired. Call++ apps
are not TLS-layer ALPNs; "producer"/"consumer" describes the data-plane
role without implying a TLS ALPN. The `ChannelDirection` enum uses
`Open` (initiator is consumer, responder is producer — the common case)
and `Expose` (initiator is producer, responder is consumer — deferred).

**Assembly layer** is the CLI binary that wires crates together at
startup (ADR-019, ADR-024). It constructs backends, injects
capabilities, registers ops, and builds the ALPN lists. It is the trust
boundary — handlers never hold vault references or construct their own
transports. In this doc, "assembly time" means "at startup, in the CLI
binary, before any connections exist."

---

## Layering (to keep the questions separate)

The outsider's layer map, plus the ALPN-category layer this doc
adds:

| Layer | Question it answers | Owner | Status |
|-------|-------------------|-------|--------|
| Endpoint accept | May this identity hold N connections? | `alknet-endpoint` | **Unowned (Gap 3).** ADR-094 explicitly says connections are unbounded. New OQ against `alknet-endpoint`. |
| `channels-core` | Byte routing; per-connection memory bounds | ADR-075/076 | Decided, coherent. Auth-blind by design (ADR-075). |
| `channels-call`, op-level | May you call `channel/open` at all? | `AccessControl::check` | Decided. (Under "call++": per-ALPN ops, each with its own ACL — Gap 1 resolved.) |
| `channels-call`, quota | How many slots may you hold? | ADR-094 | Decided, lifecycle buggy (Gap 2). Trait shape survives; the change is where `on_close` is called from and where the opener identity comes from. |
| `channels-call`, per-ALPN/direction | May you open this ALPN, this direction? | Per-ALPN op's `access_control` (this doc) | **Resolved by "call++".** Each openable ALPN is its own op; the op's `access_control` is the per-ALPN/direction ACL. |
| ALPN handler (data plane) | May you touch container abc123? | ADR-050 ownership, handler-internal | Decided (call/docker land). Unchanged under "call++." |

The tangle was that layers 5 and 6 got blurred ("ACL is checked on
`channel/open`" is true but only for layer 3, because the single
generic op couldn't see `alpn`/`direction`/`params`), and layer 1 got
silently assumed away.

---

## The structural tangle (the actual gaps)

### Gap 1 — `channel/open` ACL granularity is underspecified

The call crate's ACL is per-operation: `OperationSpec.access_control`,
checked before the handler runs by `OperationRegistry::invoke`. But
`channel/open` is one operation whose input decides what's really
being requested (`alpn`, `params`, `direction`). One op-level
`AccessControl` cannot express "peer X may open `alknet/tty` but not
`alknet/tunnel`."

The specs gesture at per-ALPN ACLs —
`channel/resources/subscribe` returns `access: { required_scopes:
["tty:open"] }` per ALPN, "advisory... the real check happens on
`channel/open`" (`channel-operations.md:150`) — but no mechanism is
specified for that real check. `HandlerRegistry` carries no
`AccessControl`. Nobody registers `alpn → required_scopes` anywhere.

Three candidate shapes were considered (the outsider's analysis):

(a) A per-ALPN ACL map held by `ChannelOperations`, checked inside the
`channel/open` handler after the op-level check. Parallel to how the
quota policy slots in.

(b) A `ChannelOpenSpec` registry (`alpn → AccessControl + params
schema + resource_id_path + allowed directions`) consulted inside the
handler after the op-level check.

(c) Delegate to the ALPN handler — it gets `AuthContext` and can
refuse.

(c) is the worst: allocation and handler spawn happen *before*
authorization, refusal semantics diverge per ALPN, and every handler
crate reimplements scope checking.

(b) looks like minimal churn, but notice what the registry actually
is: a parallel structure holding ACLs, input schemas, resource
pointers, and discovery data, with its own check invocation — a shadow
`OperationRegistry`. **The repo has been here before:** ADR-028 was
superseded by ADR-029 exactly because "a parallel authorization system
duplicated the existing `AccessControl`," and ADR-029's whole thesis
is that peer authorization is just `AccessControl::check` on the
existing path. (b) re-commits that structural miss one layer down.

### Gap 2 — The quota's accounting lifecycle leaks

ADR-094 increments in the `channel/open` handler and decrements in
the `channel/close` handler via `policy.on_close(&op_ctx.identity)`.
Two problems:

1. **Responder-initiated close decrements the wrong ledger.** If A
   opens (counted against A on B's policy) and B later sends
   `channel/close` (e.g., TTY exit happens on B's side — REQ-CH-06
   makes B the closer), the close handler runs on A with
   `op_ctx.identity = B`. B's count for A is never decremented; A's
   policy decrements a channel it never counted.

2. **Transport drop leaks the quota.** REQ-CH-02 clears the channel
   map on transport EOF, but nothing calls `on_close`. Since the
   policy is shared across connections (that's the whole point), a
   peer whose connection dies at 256 open channels is permanently at
   cap — a self-DoS.

The conceptual fix: decouple the decrement from the `channel/close`
operation and tie it to channel-state deallocation on the side that
counted. `channels-call` keeps its own `channel_id → opener PeerId`
ledger per connection (keeping `channels-core` auth-blind) and
decrements on any teardown path — close received, close sent locally,
handler exit, or connection drop. The ledger entry must be removed
atomically with its decrement: handler-exit, close-received,
close-sent-locally, and connection-drop can race, and a
double-decrement under-counts and weakens the cap.

### Gap 3 — Connection-count DoS is an unowned layer

The per-identity cap bounds channels, but each transport connection
still costs a channel-0 buffer, a `CallAdapter`, and demux tasks
before any `channel/open` — and ADR-094 explicitly says connections
are unbounded. This is not a channels-ACL problem and shouldn't be
solved there; it belongs at the endpoint/accept layer (per-identity
connection cap, analogous shape to `ChannelLifecyclePolicy`). Right
now no doc owns it. Naming it as a separate layer stops it from
re-tangling every channels-ACL conversation.

### Gap 4 — ALPN category blur (the user's insight)

ADR-086 §4 split the foundational handlers into "channels data-channel
ALPNs" (gated by channels, opened via `channel/open`, inherit ACL +
bidirectionality) and "SSH" (endpoint ALPN wrapping channels). The
"channels data-channel ALPNs" framing implies they're a separate kind
of thing that channels happens to gate — and each would re-implement
auth in a flat ALPN list. Under "call++," they're not a separate
category: they're call apps that produce data channels. They inherit
call's auth by construction (they *are* call apps). The gating is a
consequence, not the definition.

This also touches the HTTP/ WebSocket path: ADR-048 says "WebSocket
carries the native call-protocol session." Under "call++," a browser
that wants to open a TTY channel needs to call `channels/tty/open`,
which lives on channel 0 inside a *channels* connection. So WebSocket
may carry either `alknet/call` (bare, for call-only clients) or
`alknet/channels` (8-byte chunk framing, with call on channel 0
inside). Channels framing is required for data-channel clients; a
call-only client (e.g. a dashboard doing docker JSON ops) can use
bare `alknet/call` over WebSocket. OQ-65 ("WebSocket carrying
channels, not just call?") is resolved by "call++": WebSocket may
carry either; channels required for data channels.

---

## The resolution: openable ALPNs are operations ("call++")

The core observation: ADR-073's claim is "no new auth machinery —
channel lifecycle goes through the existing `AccessControl::check`."
But the single generic `channel/open` operation is precisely what
*breaks* that promise: it hides the authorization-relevant facts
(`alpn`, `direction`, `params`) inside one op's input, where the call
ACL machinery can't see them. Consequences:

- Per-ALPN scopes (`tty:open` vs `tunnel:open`) have no enforcement
  home.
- ADR-050's `resource_id_path` (ownership via JSON pointer into input)
  is declared per `OperationSpec` — a single `channel/open` can't use
  it because the pointer differs per ALPN (`/params/container` for
  tty-docker, `/params/target` for tunnel).
- The two `direction` values are wildly different grants ("may you
  consume my TTY" vs "may you register a service I will consume as a
  client") squeezed under one ACL.

The fix that requires *new* machinery is keeping the generic op; the
fix that requires *none* is the opposite of what's specced. **An
openable ALPN is an operation.**

### The marker on `OperationSpec`

`OperationSpec` gains a `channel_open` field:

```rust
pub struct OperationSpec {
    // ... existing fields ...
    pub access_control: AccessControl,
    pub resource_id_path: Option<String>,
    /// Marker consumed by layers that manage data channels. When set,
    /// the op produces a data channel alongside (or instead of) the JSON
    /// response. The marker is registry metadata, not auth machinery —
    /// it's how layers like channels-call know an op is a channel-open
    /// op, parallel to how `resource_id_path` is how ADR-050 knows where
    /// to find the resource id. The op's `access_control` is the ACL
    /// (unchanged); the marker is the dispatch hint.
    pub channel_open: Option<ChannelOpenSpec>,
}

pub struct ChannelOpenSpec {
    pub alpn: &'static str,         // e.g., "alknet/tty"
    pub direction: ChannelDirection, // Open or Expose
}

pub enum ChannelDirection {
    /// Initiator wants to consume a resource the responder will produce.
    /// Initiator is the consumer; responder is the producer (runs the
    /// data-plane handler). Common case: "open me a TTY on your docker
    /// container."
    Open,
    /// Initiator is the producer (runs the data-plane handler);
    /// responder is the consumer. The worker-expose case: "I'm exposing
    /// a TTY for you to consume."
    Expose,
}
```

The marker is registry metadata, not auth machinery — it doesn't
violate ADR-073's "no new auth machinery" promise. The op's
`access_control` is the ACL (unchanged, checked by
`OperationRegistry::invoke` before the handler runs); the marker is
the dispatch hint that tells channels-call to wrap the handler with
the channel machinery.

**The marker is wire-visible.** `channel_open` must survive discovery
serialization — it is part of the `services/schema` payload, not just
the in-process struct. Otherwise the hub (and any `from_call` importer)
can't see it. The `services/schema` handler already serializes the full
`OperationSpec` to JSON; `channel_open` is included in that
serialization.

**`from_call` relay wrapper.** A `FromCall`-imported marked op cannot
be the standard forwarding stub. The hub's version must do *forward +
allocate local-leg channel + record id mapping + start the byte-forward
pumps*. When `from_call` imports a marked op on a channels-backed
connection, it wraps it with relay machinery instead of the plain
forwarding stub. ADR-022's provenance table (leaves are forwarding
stubs, no composition authority) gets a note for this case: a
`FromCall`-imported marked op is a leaf for composition purposes but
carries relay machinery that allocates channels and spawns byte-forward
tasks. This is the load-bearing piece of the relay under call++.

**Marked ops invoked outside a channels session.** `channels/tty/open`
is registered on the call registry — which means it's also
visible/invocable on a bare top-level `alknet/call` connection, where
there is no `ChannelManager` and no data plane. The wrapper resolves
`OperationEnv::channel_manager()` at invocation time; if it returns
`None`, the wrapper returns `channel:no_channels_session`. Relatedly,
the HTTP-side adapters (`to_openapi`, `to_mcp`) must exclude marked
ops — "produces a data channel" is not expressible over a
request/response export.

### Two verbs: `open` and `expose`

`direction` becomes two verbs, not a field:

- `channels/<alpn>/open` — the initiator wants to consume a resource
  the responder will produce. Responder is the producer. The common
  case: "open me a TTY on your docker container." Browser → hub →
  spoke: both legs are `channels/tty/open`. **Specced.**
- `channels/<alpn>/expose` — the initiator wants to produce a resource
  for the responder to consume. Initiator is the producer. The
  worker-expose case: worker → hub, worker is making a TTY available
  for the hub's clients to consume. **Reserved in the
  `ChannelDirection` enum; deferred until a concrete push use case
  forces the design.**

Separate verbs → separate ACLs. The verb split's real win is enabling
`open`-only shipping: the old `direction` field forced both semantics
into the v1 wire contract. `Expose` is reserved so the shape is
available when needed, but the op + hold semantics are deferred
(`deferred(scope)`, blocked on a concrete consumer — the repo's own
pattern from OQ-56, OQ-57). Half-specifying "hold until consumer
connects" now would produce exactly the kind of hedge this doc
criticizes ADR-073 for.

### The `ChannelCore` seam (wrapper shape — flag for POC)

The ALPN crate's open-op handler does ALPN-specific work (validate
params, consult ownership, prepare the backend) and returns a
"channel plan." `channels-call` wraps it: the wrapper allocates the
`channel_id`, gets the `BiStream` from `ChannelManager`, records the
opener in the ledger (Gap 2), consults `ChannelLifecyclePolicy`,
spawns the `ProtocolHandler` on the `BiStream` with the plan's
backend, returns `{channel_id}`.

The ALPN crate provides:
- The `OperationSpec` (with `channel_open` marker, `access_control`,
  `input_schema`, `resource_id_path`).
- The open handler (the ALPN-specific work — validate params, consult
  ownership, prepare the backend, return a plan).
- The `ProtocolHandler` for the data plane (unchanged — used by both
  direct connections and channels-opened sessions; both paths
  converge at the `BiStream`).

`channels-call` provides:
- The `ChannelCore` (channel-id allocation, `ChannelManager`
  integration, per-connection opener ledger, `ChannelLifecyclePolicy`
  consultation, teardown hooks).
- A `register_openable(spec, open_handler, channel_core)` helper that
  wraps the ALPN's open handler with the channel machinery and
  registers the op on the call `OperationRegistry`.

This keeps the ALPN crate's handler focused on ALPN concerns (params,
backend) and lets channels-call own the channel machinery. The
alternative (invoke shape — the handler calls
`context.channel_core.open(...)` itself) requires either
`OperationContext` to carry a `channel_core` reference (inverting
the call → channels-call dependency) or a generic extension mechanism
on `OperationContext` (more complexity than the wrapper). The wrapper
shape is preferred; **the exact API shape (plan vs callback) is
POC-worthy** — flag this as the thing to pressure-test in a small POC
(one ALPN crate, one open op, one channels connection, prove the
wrapper allocates the channel and spawns the handler).

**Per-connection state plumbing (POC-critical).** `register_openable`
registers ops "at assembly time" (Layer 0, curated, static per
ADR-024). But the wrapper needs the **per-connection** `ChannelManager`
— the op arrives on channel 0 of one specific channels connection, and
the channel must be allocated on *that* connection's manager. A
globally-registered handler closing over a static `ChannelCore` has no
way to know which channels connection invoked it.

The resolution: the `OperationEnv` trait (already on
`OperationContext.env`) gains an optional
`fn channel_manager(&self) -> Option<&ChannelManager>`. The wrapper
handler resolves it at invocation time — static registration, dynamic
resolution. This also handles the "no channels session" case (Issue 3):
if `channel_manager()` returns `None`, the wrapper returns
`channel:no_channels_session`. The `OperationEnv` is already the
integration point for per-connection state (ADR-024); adding a
`ChannelManager` accessor is the natural extension.

This also affects recursive channels (inner connection needs its own
binding): each channels connection's `OperationEnv` overlay carries its
own `ChannelManager` reference, so nested connections resolve
correctly.

### The discovery split

Under "call++", the discovery question splits cleanly:

- **"What may I open"** (static, per-op): `services/list` (visibility-
  filtered + `AccessControl::check(calling_peer_identity)` server-side,
  per ADR-029 §6) + `services/schema` (per-op `access_control`). The
  existing server-side ACL-filtered discovery is preserved; the spec is
  the authority. `channel:forbidden` on the open op is the real check;
  the preview is "here's what the spec says, you can fail fast."
- **"What is currently there"** (dynamic, ALPN-level):
  `channel/resources/subscribe`. Each ALPN crate that registers open
  ops also provides a resource enumerator (which containers are
  running, which TTY sessions are active). `channel/resources/subscribe`
  aggregates across all registered openable ALPNs. This is the data
  source the outsider's Gap 1(c) was missing — and it naturally lives
  in the ALPN crate, not in channels-call.

The `access` preview in `resources/subscribe` (ADR-073) becomes
redundant — it's on the op spec, available via `services/schema`. We
drop it from the `resources/subscribe` payload; the spec is the
authority, and carrying a preview in a different shape invites
staleness.

The exact aggregation shape (per-ALPN `channels/<alpn>/resources/
subscribe` ops merged by the generic `channel/resources/subscribe`,
vs. callbacks registered with channels-call at registration time) is a
POC-worthy detail. The architectural point is that the data source
lives in the ALPN crate.

---

## The ALPN three-category reframe (ADR-086 amendment)

ADR-086 §4 split the foundational handlers into two categories. Under
"call++", the first category is reframed. The three categories become:

| Category | TLS-layer? | Identity at TLS? | How they're reached | Examples |
|----------|-----------|------------------|---------------------|----------|
| **Entry points** | yes | no | TLS ALPN negotiation | `h2`, `http/1.1`, `alknet/register` |
| **Endpoints** | yes | yes | TLS ALPN negotiation | `alknet/channels`, `alknet/call`, `alknet/ssh` |
| **Call++ apps** (was "channels data-channel ALPNs") | no | n/a (inside channels) | `channels/<alpn>/open` / `expose` op on channel 0 | `alknet/tty`, `alknet/tunnel`, `alknet/socks5`, `alknet/fs`, `alknet/sftp` |

The third category changes description. Previously "ALPNs gated by
channels" (implying they're a separate kind of thing that channels
happens to gate, and each re-implements auth in a flat ALPN list).
Now "call apps that produce data channels" (they *are* call apps;
they inherit call's auth/composition/identity by construction; the
data-channel part is the `channel_open` marker). The gating is a
consequence, not the definition.

**Direct registration remains possible.** The category table says
call++ apps are not TLS-layer ALPNs, but the `ProtocolHandler` is
still usable by both direct connections (`HandlerRegistry` →
`ProtocolHandler` → `BiStream`) and channels-opened sessions. ADR-077's
two-mode survives at the mechanism level; the canonical composition is
call++. The table describes the canonical path, not a prohibition on
direct use.

**Naming.** "Call++ apps" is the mental model. "Channels-served
ALPNs" is descriptive. The final naming is a separate (cosmetic)
decision tracked as an OQ; this doc uses "call++ apps" as the working
name.

### What this means for the ALPN crates (the lineage)

The lineage makes the unification obvious in retrospect:
**call → docker → tty → channels**. Docker was the first
call-consuming app (wraps bollard in JSON ops — ADR-058). Working
docker surfaced TTY (exec needs a terminal). Working TTY surfaced
channels (terminal output isn't JSON). The loop closes: the ALPN
crates that channels serves become channels-consuming apps in the
same shape docker is a call-consuming app. The only difference is
that some ops produce a data channel instead of (or alongside) a
JSON response.

The dependency split parallels `channels-core` / `channels-call`
(ADR-081):

- **Data-plane core** (the `ProtocolHandler`, wire format, backend
  trait) — depends on `alknet-core` only. No call dep. ADR-057's
  "tty does not depend on call" property survives here.
- **Control-plane layer** (the open/expose ops, registration helper)
  — depends on the data-plane core + `alknet-call` +
  `alknet-channels-call` (for the channel machinery). ADR-057 is
  amended: the *data plane* stays call-free; the *control plane* is
  call by construction.

Whether that's a sub-crate split (`alknet-tty-core` + `alknet-tty`)
or a feature flag (`alknet-tty` with a `channels` feature) is a
packaging choice — two-way-door, not architecture. The architectural
point is the dependency boundary.

### SSH stays distinct

SSH is an endpoint ALPN that wraps channels. Under "call++", the
channels inside SSH have call on channel 0, and the call registry
has the open ops. An SSH client opens an SSH channel; the SSH server
translates that to `channels/<alpn>/open` on channel 0 internally
(SSH server as translator, same shape as the hub relay — ADR-079).
The SSH client doesn't know about call; it just opens SSH channels.
SSH's category (endpoint ALPN wrapping channels) is unchanged. This
is SSH-implementation detail and SSH is deferred, but the shape
holds.

---

## The hub-relay + worker-expose flow (walked end-to-end)

This is the flow the outsider flagged as "where ADR-073's single-op
design was doing the most implicit work." Walking it under "call++"
to verify it holds.

### Browser → hub → spoke, "open me a TTY" (the common case)

1. Browser sends `channels/tty/open` with
   `{params: {backend: docker, cmd: ["bash"], container: "abc123"}}`
   on its channel 0 (call op on the browser→hub leg).
2. Hub's `CallAdapter` receives `channels/tty/open`. The
   `OperationRegistry` checks the op's `access_control` against the
   browser's identity. The op's spec has
   `channel_open: Some(ChannelOpenSpec { alpn: "alknet/tty", direction: Open })`.
   Browser is the initiator / consumer.
3. Hub's `CallAdapter` recognizes the `channel_open` marker. The hub
   does NOT run a local `TtyAdapter` — the hub never runs
   protocol-specific handlers (ADR-079). It forwards to the spoke via
   `from_call`: hub re-issues `channels/tty/open` on the spoke leg
   with `forwarded_for = browser`.
4. Spoke's `CallAdapter` receives `channels/tty/open`.
   `OperationRegistry` checks the op's `access_control` against the
   hub's identity (the direct caller per ADR-032). The spoke's
   ownership store verifies the hub owns `container:abc123`
   (per ADR-050). The spoke consults
   `ChannelLifecyclePolicy::check_open(hub)` (Gap 2 — keyed by
   direct caller, opener recorded in the per-connection ledger).
5. Spoke's `ChannelCore` allocates `channel_id`, spawns `TtyAdapter`
   on the channel's `BiStream` with the docker backend, records
   opener (hub) in the ledger, returns `{channel_id}`.
6. Hub receives the spoke's `{channel_id}`, opens a matching channel
   on the browser's side (hub is the responder for the browser leg),
   records the `channel_id` mapping `browser_id ↔ spoke_id`, returns
   `{channel_id: browser_id}` to the browser.
7. Hub byte-forwards between `browser_id` and `spoke_id` with 4-byte
   `channel_id` rewrite (ADR-079 unchanged).

The hub ran **zero** protocol-specific auth and zero protocol-specific
data-plane work. It ran `channels/tty/open`'s `access_control`
(call-protocol machinery) and forwarded. The relay contract from
ADR-079 holds unchanged in shape; only the op name changed (from
generic `channel/open` to per-ALPN `channels/tty/open`).

### Worker → hub → browser, "worker exposes a TTY" (the push case — deferred)

The `expose` verb is reserved in the `ChannelDirection` enum but
deferred. The walk-through below is a sketch of the shape, not a spec.
The open case (consumer initiates on demand) is the common case and is
specced; the expose case is for push scenarios (a worker pushes a log
stream to a monitoring browser that's already connected) and will be
specced when a concrete consumer forces the design.

Sketch: worker sends `channels/tty/expose` on its channel 0. Hub
checks `access_control` against the worker's identity (separate ACL
from `channels/tty/open`). Hub recognizes the `channel_open` marker
and must relay to a connected browser that wants to consume. The hub
initiates `channels/tty/expose` on the browser leg (hub is
producer transparently — it forwards the data plane to the worker).
Hub allocates `channel_id` on both legs, records the mapping, and
byte-forwards with `channel_id` rewrite.

The hard question — "hold until consumer connects" vs "reject if no
consumer" — is deferred with the verb. The `direction` field is
per-leg, not end-to-end; the relay passes the verb through
(`expose` → `expose`), same as in the open case (`open` → `open`).

### The `channel_id` allocation symmetry

In both cases, `channel_id` allocation is by the responder (DP-1,
unchanged). In the open case, the responder is the spoke (spoke
allocates). In the expose case, the responder is the hub on the
worker→hub leg (hub allocates) and the browser on the hub→browser
leg (browser allocates). The hub relay records the mapping across
legs. This preserves ADR-073's "channel_id allocation is always by
the responder" invariant.

---

## What goes where (ADR plan)

| ADR | Scope | Status |
|-----|-------|--------|
| **ADR-095 (new)** | "Openable ALPNs are operations" — the call++ design. The mental model, the `channel_open` marker on `OperationSpec`, the `ChannelCore` seam (wrapper shape, POC-flagged), the two-verb split, the discovery split, the three-category ALPN reframe. The unifying ADR. | **Ready to draft.** |
| **ADR-073 amendment** | `channel/open` dissolves into per-ALPN ops in `channels/<alpn>/open` and `channels/<alpn>/expose`. `channel/close`, `channel/control`, `channel/resources/subscribe` stay generic (keyed by `channel_id`). The `direction` field is removed (becomes the verb). Error codes: `channel:unknown_alpn` becomes "operation not found"; `channel:invalid_params` becomes ordinary schema rejection. | **Ready to draft.** |
| **ADR-094 amendment (Gap 2)** | The per-connection opener ledger in `channels-call`. The decrement is keyed by the opener (from the ledger), not the closer. The decrement is called from every teardown path (close received, close sent, handler exit, connection drop), not just `channel/close`. The trait shape (`check_open`, `on_close`) survives. The teardown hooks (connection-drop, handler-exit) are new structural requirements on `channels-call`. | **Ready to draft.** |
| **ADR-086 §4 amendment** | "Channels data-channel ALPNs" → "call++ apps" (or whatever the naming OQ settles). The category distinction holds (them vs SSH); the description changes from "gated by channels" to "call apps that produce data channels." | **Ready to draft.** |
| **ADR-048 amendment + OQ-65 resolution** | WebSocket may carry either `alknet/call` (bare, for call-only clients) or `alknet/channels` (8-byte chunk framing, call on channel 0 inside). Channels framing required for data-channel clients. OQ-65 resolved. "Native session, not gateway" survives (the decision). | **Ready to draft.** |
| **ADR-057 amendment** | TTY data plane stays call-free; control plane (open/expose ops) depends on call. "Self-contained negotiation framing" becomes the data-plane negotiation (the 5-byte format's negotiation frame); the control-plane negotiation is the call op. | **Ready to draft.** |
| **ADR-058 clarification** | The boundary criterion (EventEnvelope-compatible → call op; incompatible → data channel with call control plane) is preserved and sharpened by "call++". Probably a note, not a full amendment. | **Ready to draft.** |
| **New OQ (Gap 3)** | Per-identity connection cap against `alknet-endpoint`. Named, `deferred(scope)` — the deployment shape that needs it isn't concrete yet. Owned by `alknet-endpoint`, not channels. | **Ready to open.** |

---

## What changes in each crate

### `alknet-call`

- `OperationSpec` gains `channel_open: Option<ChannelOpenSpec>` (the
  marker). This is a one-way-door API change (every spec-constructing
  code adds the field, defaulting to `None`).
- No other change. The `OperationRegistry` is unchanged — it still
  invokes ops by name, checks `access_control`, runs the handler. The
  marker is opaque to the registry; it's channels-call that reads it.

### `alknet-channels-call`

- Gains the `ChannelCore` (channel-id allocation, `ChannelManager`
  integration, per-connection opener ledger, `ChannelLifecyclePolicy`
  consultation, teardown hooks).
- Gains the `register_openable(spec, open_handler, channel_core)`
  helper that wraps the ALPN's open handler with the channel
  machinery and registers the op on the call `OperationRegistry`.
- The `channel/close` handler no longer calls
  `policy.on_close(&op_ctx.identity)` directly; instead, the
  per-connection ledger is walked on every teardown path and
  `on_close` is called per opener. (Gap 2.)
- The generic ops (`channel/close`, `channel/control`,
  `channel/resources/subscribe`) stay in channels-call, keyed by
  `channel_id`.
- The teardown hooks (connection-drop, handler-exit) are new. The
  connection-drop hook needs to interpose before `channels-core`'s
  REQ-CH-02 clears the channel map — channels-call walks its ledger
  and decrements every opener before the map is cleared.

### `alknet-channels-core`

- Unchanged. The pure multiplexer (ADR-075/093) is auth-blind by
  design and stays that way. The per-connection opener ledger lives
  in `channels-call`, not `channels-core`.

### `alknet-tty` (the first call++ app)

- Data plane (the `TtyAdapter`, the 5-byte wire format, the
  `TtyBackend` trait) is unchanged. Used by both direct connections
  (`HandlerRegistry` → `ProtocolHandler` → `BiStream`) and
  channels-opened sessions (`channels/tty/open` → allocate channel →
  spawn `TtyAdapter` on the channel's `BiStream`). Both paths
  converge at the `BiStream`.
- Control plane (new): registers `channels/tty/open` on the call
  `OperationRegistry` at assembly time, alongside its
  `ProtocolHandler` on the `HandlerRegistry` for direct connections.
  The op spec carries the `channel_open` marker, the `access_control`
  (e.g., `required_scopes: ["tty:open"]`), the `input_schema` (the
  `NegotiateRequest`), and the `resource_id_path` (e.g.,
  `/params/container` for docker-backed TTY). `channels/tty/expose`
  is reserved in the enum but not registered until a concrete push
  use case forces it.
- The open handler validates params, consults ownership (ADR-050),
  prepares the `TtyBackend`, returns a "channel plan" to the
  `ChannelCore` wrapper, which spawns the `TtyAdapter`.
- Dependency: data-plane core depends on `alknet-core` only
  (ADR-057's property survives); control plane depends on
  `alknet-core` + `alknet-call` + `alknet-channels-call`.
- Optional: registers `channels/tty/list-sessions` etc. if session
  management is wanted. The ALPN gets a full management plane over
  the call protocol — which it didn't have before.

### `alknet-tunnel`, `alknet-socks5`, `alknet-fs`, `alknet-sftp`

- Same shape as TTY: data plane (ProtocolHandler) unchanged; control
  plane (open/expose ops) new. Each registers its open ops on the
  call registry at assembly time.
- These crates are not yet specced (per ADR-085). When specced, they
  follow the call++ pattern from the start.

### `alknet-hub`

- Unchanged in shape. The hub composes call apps — docker (JSON ops),
  tty (open op + data channel), tunnel (open op + data channel),
  agent, etc. All register ops on the call registry. The hub's
  assembly layer wires them uniformly. The hub doesn't distinguish
  "call app" from "channels app" — both are just apps with ops
  registered. Some ops return JSON; some ops carry the `channel_open`
  marker and produce a data channel. The composition model is
  uniform.
- The relay contract (ADR-079) is unchanged in shape. The op name
  changes (from generic `channel/open` to per-ALPN
  `channels/<alpn>/open`); the marker (not prefix-matching) is how
  the hub recognizes and translates channel-open ops.
- **`from_call` relay wrapper.** When `from_call` imports a marked op
  on a channels-backed connection, it wraps it with relay machinery
  (forward + allocate local-leg channel + record id mapping + start
  byte-forward pumps) instead of the plain forwarding stub. ADR-022's
  provenance table gets a note: a `FromCall`-imported marked op is a
  leaf for composition purposes but carries relay machinery.

### `alknet-worker`

- Same as hub. A worker registers its ops on its call registry. The
  worker's assembly layer wires them uniformly.

### `alknet-endpoint`

- Unchanged (for now). Gap 3 (per-identity connection cap) is a new
  OQ against `alknet-endpoint`, deferred until a deployment forces
  it. Named now to stop the re-tangle.

### `alknet-http`

- WebSocket may carry either `alknet/call` (bare, for call-only
  clients) or `alknet/channels` (8-byte chunk framing, call on
  channel 0 inside). Channels framing required for data-channel
  clients. ADR-048 amended; OQ-65 resolved.
- The HTTP adapter's call-protocol surface (registration, browser
  API routes) is unchanged — it's call ops, not channels ops.
- The MCP/OpenAPI adapters (`to_openapi`, `to_mcp`) must exclude
  marked ops — "produces a data channel" is not expressible over a
  request/response export.

---

## Open questions

- **Per-connection state plumbing (POC-critical).** The
  `OperationEnv::channel_manager()` accessor is the proposed
  resolution for per-connection `ChannelManager` resolution. The
  exact trait shape (return type, whether it's on `OperationEnv` or a
  separate extension trait) is POC-worthy. The architectural point is
  that the `OperationEnv` is the integration point for per-connection
  state (ADR-024), and the wrapper resolves the `ChannelManager` at
  invocation time — static registration, dynamic resolution.

- **ChannelCore seam: wrapper vs invoke.** This doc specs the wrapper
  shape (the ALPN's open handler returns a "channel plan"; channels-
  call's wrapper does the allocation/ledger/policy/spawn). The
  alternative (invoke — the handler calls
  `context.channel_core.open(...)` itself) inverts the call →
  channels-call dependency or requires a generic extension mechanism
  on `OperationContext`. The wrapper is preferred. The exact API
  shape (plan vs callback) is **POC-worthy** — flag for a small POC
  during implementation. Not a blocker for the architecture decision.

- **Resource enumeration aggregation shape.** Each ALPN crate
  provides a resource enumerator. The generic
  `channel/resources/subscribe` aggregates. Exact shape (per-ALPN
  `channels/<alpn>/resources/subscribe` ops merged by the generic op,
  vs. callbacks registered with channels-call at registration time)
  is a POC-worthy detail. The architectural point is that the data
  source lives in the ALPN crate.

- **Naming the third category.** "Call++ apps" (the mental model) vs
  "channels-served ALPNs" (descriptive) vs "data-channel call apps."
  Cosmetic but in a lot of tables. Tracked as an OQ.

- **Op naming vs OQ-13.** `channels/tty/open` implies the ops belong
  to the channels service, but the *TTY crate* registers them — the
  docker precedent (`docker/container/list`) suggests `tty/open`.
  Since relay recognition is via the marker (not name prefix), nothing
  constrains the name; but the ALPN→path-segment mapping
  (`alknet/tty` → `tty`) needs pinning either way, including what
  non-`alknet/*` ALPNs do.

- **Expose verb.** Deferred. The `Expose` variant is reserved in the
  `ChannelDirection` enum; the op + hold semantics are deferred until
  a concrete push use case forces the design (`deferred(scope)`,
  blocked on a concrete consumer — the repo's own pattern from OQ-56,
  OQ-57).

- **Does `ProtocolHandler` want channel-context?** Probably not for
  TTY/tunnel/ssh (they just want a `BiStream`), but maybe for ALPNs
  that want the opener's identity for per-session logging/ACL. An
  optional channel-context passed alongside the `BiStream`. Not a
  blocker; defer. Two-way-door implementation detail.

- **Per-identity connection cap (Gap 3).** A per-identity connection
  cap at the endpoint/accept layer, analogous to
  `ChannelLifecyclePolicy` but before any `ChannelsAdapter` /
  `CallAdapter` / channel-0 buffer exists. Lives in
  `alknet-endpoint`. `deferred(scope)` — the deployment shape that
  needs it isn't concrete yet. Named now to stop the re-tangle.

- **`channel-operations.md` §ACL-flow `forwarded_for` inconsistency.**
  Step 4 says "the spoke's ownership store verifies the hub (or the
  `forwarded_for` browser, per policy) owns `container:abc123`."
  This contradicts ADR-032 (`forwarded_for` is metadata, not
  authority — `AccessControl::check` never reads it) and ADR-050 §4c
  ("the spoke sees the hub as the owner"). The "(or the
  `forwarded_for` browser, per policy)" clause is a spec
  inconsistency. Fix while editing. The spoke authorizes the hub,
  full stop; the hub's per-browser ACL is the hub's own layer.

---

## References

- ADR-073: channel lifecycle operations (amended by this resolution
  — `channel/open` dissolves into per-ALPN ops; generic ops stay)
- ADR-094: per-identity channel cap (amended by this resolution —
  the ledger + teardown hooks; the trait shape survives)
- ADR-079: hub relay — translate, not forward (unchanged in shape;
  the op name changes, the marker replaces prefix-matching)
- ADR-050: dynamic resource ownership (the `resource_id_path`
  mechanism that works again under per-ALPN ops)
- ADR-028: the "parallel authorization system" precedent (superseded
  by ADR-029 for exactly the structural miss this resolution avoids
  re-committing one layer down)
- ADR-029: peer-graph routing (the existing `AccessControl::check`
  path this resolution preserves)
- ADR-086: endpoint types and entry points (§4 amended — the
  "channels data-channel ALPNs" category reframed to "call++ apps")
- ADR-048: WebSocket native session (amended — WebSocket may carry
  either `alknet/call` or `alknet/channels`; channels framing required
  for data-channel clients; OQ-65 resolved)
- ADR-057: alknet-tty does not depend on call (amended — the data
  plane stays call-free; the control plane depends on call)
- ADR-058: alknet-docker on alknet/call (the boundary criterion
  preserved and sharpened — EventEnvelope-compatible → call;
  incompatible → data channel with call control plane)
- ADR-075: ChannelsAdapter and ChannelManager (the auth-blindness
  this resolution preserves — the ledger lives in channels-call, not
  channels-core)
- ADR-092: BiStream as the handler leaf (the transport-leaf layer,
  settled; this doc is the layer above it)
- ADR-093: channels pure channel multiplexing (the wire format, the
  handler-owns-sub-multiplexing property — both preserved)
- `docs/research/stream-unification/findings.md` — the precedent for
  this research-then-sync pattern
- `docs/architecture/crates/channels/channel-operations.md` — the
  spec this resolution rewrites (per-ALPN open ops; generic
  close/control/resources; the ACL-flow `forwarded_for` fix)
- `docs/architecture/crates/call/operation-registry.md` —
  `OperationSpec`, `AccessControl`, `OperationRegistry::invoke` (the
  machinery this resolution reuses verbatim, with the `channel_open`
  marker added)