---
status: draft
last_updated: 2026-07-18
---

# Alknet Architecture

## Current State

**Client-dial SOCKS5 proxy seam added (ADR-090, 2026-07-16).**
`AlknetClient` (ADR-089) gains an optional SOCKS5 proxy
(`with_socks5_proxy`) so a native client can hide its real IP from the
hub. `dial_quic` routes QUIC through SOCKS5 UDP ASSOCIATE (validated by
the `/workspace/quinn-proxy-poc` PoC and
[`docs/research/quinn-quic-proxy/findings.md`](../research/quinn-quic-proxy/findings.md)
— quinn's `AsyncUdpSocket` + `new_with_abstract_socket` is the
extension point; 5/5 runs clean); `dial_tcp_tls` routes through SOCKS5
CONNECT. The proxy is invisible above the dial (`Connection`,
dispatch, credentials, TLS config all proxy-unaware) and the no-proxy
path is the zero-cost default (the `socks5` feature and `fast-socks5`
dep are opt-in). SOCKS5 is the sole proxy protocol (it covers both TCP
and UDP, so no HTTP CONNECT variant needed). The two distinct SOCKS5
concepts — the client-dial proxy (ADR-090, transport-layer privacy)
and the planned `alknet-socks5` channels data-channel handler (ADR-085
scope table, a service one side offers the other) — compose without
coupling (a client using the hub's `alknet/socks5` service tunnels it
locally and points its `Socks5ProxyConfig` at the local tunnel end).
iroh was the exception: `dial_iroh` did not consume
`Socks5ProxyConfig` — iroh's `proxy_url` covers the relay-exposure
surface, but the direct-connection peer-exposure case was
[OQ-67](open-questions.md) (deferred(unclear) — the pieces existed but
the iroh socket-stack composition wasn't clear; did not block the
first hub deployment, which uses QUIC/TCP+TLS). See
[ADR-090](decisions/090-client-dial-socks5-proxy-seam.md).

**iroh proxy resolved — force relay-only + HTTP-to-SOCKS5 bridge (ADR-090
§5 amended, OQ-67 resolved, 2026-07-16).** The iroh-proxy POC
(`/workspace/iroh-proxy-poc`,
[`docs/research/iroh-proxy-poc/findings.md`](../research/iroh-proxy-poc/findings.md),
5/5 runs clean) settled OQ-67: iroh does **not** expose a
socket-injection hook for the IP/direct transport (the quinn POC's
`Socks5UdpSocket` does not transfer — `noq_endpoint()` is `pub(crate)`,
the IP transport binds its own `netwatch::UdpSocket`, and
`CustomTransport` operates on a separate `CustomAddr` address space
iroh's hole-punching doesn't route through). The decision is **force
relay-only** when a proxy is configured: three stable public iroh
Builder knobs (`clear_ip_transports()` + `addr_filter(relay_only)` +
`proxy_url`) eliminate the direct path and tunnel the relay WebSocket
through the proxy. The peer sees the relay's IP; the relay sees the
proxy's IP; the client's real IP is hidden on both surfaces. No iroh
fork required. Because iroh's `proxy_url` expects an HTTP CONNECT proxy
(not SOCKS5), the integration runs a tiny local **HTTP-to-SOCKS5
bridge** (~80 lines) so a single `Socks5ProxyConfig` covers all three
dials uniformly. The POC also corrected a factual error: iroh's
`proxy_url` proxies the relay WebSocket only, not pkarr/DoH (those use
`pkarr`/`hickory-resolver` directly) — acceptable for the
force-relay-only config (QAD disabled), but the spec text in ADR-090
§5 is corrected. Force relay-only forgoes iroh's direct-path latency
advantage (negligible for the hub deployment, which runs its own
relay) and makes relay availability a hard dependency — the intended
privacy/availability tradeoff; a caller that prefers availability over
privacy for the iroh path simply does not set the proxy. See
[ADR-090](decisions/090-client-dial-socks5-proxy-seam.md) §5.

**Workspace scope corrected (ADR-085, 2026-07-15).** The overview's
crate graph had been describing the wrong scope since ADR-003 — a flat
~12-crate workspace including DNS, messaging, and NAPI, while omitting
channels, hub, worker, and tls. [ADR-085](decisions/085-workspace-scope-core-vs-consumer-repos.md)
records the actual scope: the mono-repo is the **core networking
toolkit** (substrate: core, tls, call, channels; deployment shapes: hub,
worker; foundational handlers: tty, http, ssh, tunnel, socks5, fs, sftp;
vault). Crates that build on top of a hub or worker (docker, agent) are
**consumer repos** — separate repos depending on the published core
crates. This corrects the root cause of the "assembly layer" hedging
pattern: the overview now reflects the real boundary, so the
"assembly layer" has a bounded home (hub/worker), not an escape hatch.
The [overview.md](overview.md) crate graph and ALPN registry are
rewritten to match.

**alknet-channels specs drafted.** The alknet-channels crate (multiplexing
proxy — `ProtocolHandler` on `alknet/channels`, 8-byte chunk format, N
channels over transport stream(s), channel 0 pre-negotiated as
`alknet/call`) now has architecture specs:
[crates/channels/](crates/channels/) (overview, channels-wire,
channels-connection, channels-adapter, channel-operations, channel-client)
and twelve ADRs — [ADR-071](decisions/071-channels-wire-format.md) (8-byte
chunk header; amended by ADR-093 — the channels layer has no `stream_type`
concept; the handler owns its sub-stream multiplexing on the `BiStream`),
[ADR-072](decisions/072-channel-0-pre-negotiated-call.md)
(channel 0 = `alknet/call` pre-negotiated; the call protocol's
`EventEnvelope` framing is the channels payload, carried transparently),
[ADR-073](decisions/073-channel-lifecycle-operations.md) (channel
lifecycle operations on the call protocol — `channel/open`/`close`/
`control`/`resources/subscribe`; `channel/resources/subscribe` is a
`Subscription` operation using the already-implemented `StreamingHandler`
machinery, not a polled `Query`; the `direction` field pins who is the
ALPN-server; amended by ADR-093 — `stream_types` field removed from
`channel/open`, `stream_type` field removed from `channel/control`),
[ADR-074](decisions/074-channelconnection-bidistreamsource.md)
(`ChannelBidiStreamSource` implements `BidiStreamSource` — ADR-070's
extension point; amended by ADR-093 — `into_sub_streams()` removed;
`accept_bi()` is the only accessor, yields one `BiStream` per channel),
[ADR-075](decisions/075-channelsadapter-and-channelmanager.md)
(`ChannelsAdapter` substrate-agnostic demux loop (reads 8-byte headers off
every bidi stream, regardless of substrate) + `ChannelManager`
reassemble/allocate split; REQ-CH-01..04 wire-level invariants pinned:
shutdown emits zero-length sentinel, transport close drops all senders, mux
dynamic registration, lenient unknown-`channel_id`),
[ADR-076](decisions/076-backpressure-channel-limits-id-reuse.md)
(bounded-buffer backpressure 1 MiB default per channel, 256-channel cap,
monotonic IDs with wrap-around; amended by ADR-093 — per-`channel_id`,
not per-`(channel_id, stream_type)`),
[ADR-077](decisions/077-tty-inside-channels.md) (TTY inside channels —
**reversed by ADR-093**: TTY always uses its 5-byte format, carried
transparently in the channels payload; the two-mode design is preserved
but differs only in `BiStream` source, not in parsing; the control channel
split is TTY-internal, not channels-layer),
[ADR-078](decisions/078-two-pump-shutdown-on-completion.md) (two-pump
handlers MUST shut down the opposite sink on pump completion — the
deadlock contract the POC surfaced; handler-level, not channels-layer;
core helper extraction deferred per OQ-57),
[ADR-079](decisions/079-hub-relay-translate-not-forward.md) (hub relay
translates `channel/open` on channel 0 with `forwarded_for` — ADR-032;
data channels byte-forwarded with `channel_id` rewrite (4-byte field
rewrite within the 8-byte header); the hub never runs protocol-specific
handlers),
[ADR-080](decisions/080-channelclient.md) (`ChannelClient`,
transport-agnostic `from_connection` primary; dial lives in
`AlknetClient` (`alknet-client`, ADR-089, resolving OQ-55);
bidirectionality preserved; no `stream_types` on `open_channel`/`Channel`
per ADR-093),
[ADR-081](decisions/081-channels-subcrate-decomposition.md) (sub-crate
decomposition — `channels-core` (pure multiplexer, depends on alknet-core
only, no call dependency) / `channels-call` (channel 0 pre-negotiation +
lifecycle op registrations, depends on channels-core + alknet-call) /
`channels-hub` (relay) / `channels-worker` (ChannelClient); isolates the
call-protocol coupling from the pure multiplexer; amended by ADR-093 —
8-byte wire format, `ChannelSubStreams`/`SubStreamHandle` removed),
[ADR-093](decisions/093-channels-pure-channel-multiplexing.md) (the
umbrella decision: channels layer is pure channel multiplexing — 8-byte
header, no `stream_type`, `into_sub_streams` removed, `BiStream`-only,
TTY always 5-byte; amends ADR-071/074/077 and the channels-facing clauses
of ADR-072/073/075/076/080/081). The specs are grounded in the completed
de-risk POC (`docs/research/alknet-channels/poc-summary.md`, 28 tests
passing, three validated targets: chunk format + demux/mux, per-channel
`Connection` presentation, tunnel handler) and the stream-unification
research (`docs/research/stream-unification/findings.md`, which surfaced
the pure-multiplexing resolution). The core prerequisite — ADR-070
(`BidiStreamSource` trait + `Connection::from_source`) + ADR-092
(`BiStream` as the handler leaf) — is landed and implemented. The spec
work converted three research hedges into decisions:
`channel/resources` is subscribe from day one (not poll-for-v1), channel
ID allocation is server-assigned (not "if zero-RTT needed"), and
backpressure is bounded-buffer (not "if HOL blocking becomes a problem").
Two genuine deferrals: OQ-56 (full windowing — blocked on a real HOL-
blocking observation) and OQ-57 (two-pump helper extraction — blocked on
a second two-pump handler). ADR-093 is the channels-layer consequence of
ADR-092's `BiStream` handler-leaf decision — every channel is a
`BiStream`, the handler owns its sub-stream multiplexing, the channels
layer has no `stream_type` concept.

**Pre-implementation of the storage/repo pattern.** The project has completed a pivot from a three-layer model to an ALPN-as-service model. The greenfield workspace contains `alknet-vault` (stable — implementation complete and verified, local-only by construction per ADR-025, HD-derivation key model per ADR-026) and research/reference material. Foundational ADRs (001–035) are in place, with the call crate implemented and reviewed.

The storage and auth strategy research (`docs/research/alknet-storage-strategy/findings.md`) surfaced the repo/adapter pattern as the answer to cross-node state (peer identity, credentials). This has now landed as four ADRs:

- **ADR-030** (PeerEntry and Identity.id decoupling): `authorized_fingerprints: HashSet<String>` → `peers: Vec<PeerEntry>`; `Identity.id` becomes the stable `peer_id` (not the fingerprint); key rotation changes the fingerprint, not the identity. Supersedes ADR-029's v1 UUID source (the one-way door — `PeerId` is logical, not crypto — is preserved; the source changes from UUID to `Identity.id` from `PeerEntry`). Resolves OQ-33 and the storage-boundary half of OQ-34.
- **ADR-031** (CredentialStore repo trait): the second repo trait in core (alongside `IdentityProvider`), with `InMemoryCredentialStore` default adapter. Establishes the credential-persistence abstraction.
- **ADR-032** (Forwarded-for identity): `forwarded_for` field on `call.requested` and `OperationContext`; metadata only — `AccessControl::check` never reads it; the `from_call` handler populates it. Wire-format one-way door, included with the ADR-029 migration window.
- **ADR-033** (Storage boundary and repo/adapter pattern): core defines repo traits + in-memory defaults; persistence adapters are separate crates; the assembly layer wires the adapter. Resolves OQ-34's storage-boundary question. Concrete adapter shapes now committed by ADR-035 (was OQ-36).

The alknet-call crate is **implemented and reviewed** — both the server-side core and the client/adapter surface (207 lib + 2 integration tests passing). The alknet-core and alknet-call crate specs are in draft; the alknet-vault crate specs are stable.

**alknet-http specs drafted and consistency-reviewed.** The alknet-http crate (HTTP interface — `h2`/`http/1.1` server + WebSocket browser path + `from_openapi`/`to_openapi`/`from_mcp`/`to_mcp` adapters) now has architecture specs: [crates/http/](crates/http/) (overview, http-server, websocket, http-adapters, http-mcp, webtransport) and thirteen ADRs — [ADR-036](decisions/036-http-to-call-operation-mapping.md) (HTTP-to-call mapping; direct-call surface — **routing superseded by ADR-047**, non-routing clauses survive), [ADR-037](decisions/037-mcp-stdio-transport-exclusion.md) (MCP stdio exclusion), [ADR-038](decisions/038-http3-and-webtransport-as-first-class.md) (HTTP/3 + WebTransport as first-class — **superseded by ADR-044**; its correction of the two-way-door-as-deferral anti-pattern stands, its specific decision is reversed by the scope deferral), [ADR-039](decisions/039-http-server-and-client-host-colocated.md) (HTTP server + client host colocated in one crate), [ADR-040](decisions/040-webtransport-alpn-stream-proxy.md) (WebTransport ALPN-stream-proxy — **parked** per ADR-044; revives unchanged when WebTransport revives), [ADR-041](decisions/041-mcp-tool-gateway-pattern.md) (`to_mcp` tool-gateway pattern — 4 fixed gateway tools instead of one tool per operation, addressing LLM context tool-bloat), [ADR-042](decisions/042-openapi-gateway-pattern.md) (`to_openapi` gateway pattern — 5 fixed gateway endpoints instead of one path per operation; per-caller AccessControl-filtered API surface; supersedes ADR-036's original `to_openapi` clause), [ADR-043](decisions/043-webtransport-bidirectional-alpn-substrate.md) (WebTransport as a bidirectional ALPN transport substrate — **parked** per ADR-044; §2/§3 transfer to WebSocket for v1), [ADR-044](decisions/044-defer-webtransport-browsers-use-websocket.md) (defer `h3`/WebTransport; browsers use WebSocket for the bidirectional call-protocol path; a scope decision per ADR-009 §"What this framework is NOT"; reversal trigger = a concrete ALPN-stream-proxy use case; states the "browser is not a peer" rationale — addressability vs. bidirectionality — that amends ADR-034 §4), and [ADR-045](decisions/045-to-openapi-gateway-spec-versioning.md) (`to_openapi` published-spec versioning — `info.version` semver tracks the gateway endpoint contract, not the operation set; resolves OQ-39), and [ADR-046](decisions/046-assembly-layer-custom-http-routes.md) (assembly-layer custom HTTP routes on HttpAdapter — `extra_routes: Option<Router>` for deployment-specific endpoints like an OAI-compatible proxy; default surface unchanged, takes precedence on collision), and [ADR-047](decisions/047-remove-direct-call-http-surface.md) (remove the direct-call `POST /{service}/{op}` surface — the gateway `/call` is the sole invoke path; the simplified contract is the few-fixed-endpoints model, not a per-operation REST tree; ADR-036's non-routing clauses survive), and [ADR-048](decisions/048-websocket-native-session-not-gateway.md) (WebSocket carries the native `EventEnvelope` call-protocol session, not the HTTP gateway shape — the gateway endpoints are HTTP-only; discovery via `services/list`/`services/schema` as call-protocol ops; clarifies the WS-path shape ADR-044 committed). ADR-003 Amendment 1 clarifies that `alknet-call` is a protocol-foundation crate (the `alknet-http` → `alknet-call` dependency edge). A consistency review pass corrected drift from the mid-spec pivot (the `to_openapi` gateway pattern landed in the prose but not in cross-references; the WebTransport specs inherited the OpenAPI/MCP direction assumption that doesn't hold for the call protocol) — ADR-036's `to_openapi` clause is now amended as superseded by ADR-042, ADR-034 §5's "deferral bucket" wording is corrected (the decision stands), and the http specs now name the one-directional HTTP projection vs. the bidirectional WebSocket (and, when revived, WebTransport) substrate. The WebSocket path is promoted to its own spec ([websocket.md](crates/http/websocket.md)) with the native-session-vs-gateway distinction made explicit (ADR-048). The specs are in draft; implementation has not started. Two open questions carried: OQ-38 (WebTransport standalone relay service scope — distinct from the in-process ALPN-stream-proxy resolved by ADR-040) and OQ-40 (reqwest client config — since resolved by the `ClientWithMiddleware` + middleware stack design). OQ-39 (`to_openapi` published-spec versioning) is resolved by ADR-045.

**Next step**: The storage/repo-pattern ADRs (030–033) are accepted and amend the core and call specs. The next implementation phase is the ADR-029 migration (peer-keyed overlays, `PeerRef` routing, retire `remote_safe`/`trusted_peer`) with the ADR-030 `PeerEntry` change and the ADR-032 `forwarded_for` field folded in — the `OperationContext`, `from_call` handler, and `AuthPolicy` are all under edit, making this the cheapest window. After that: alknet-http implementation (specs drafted; `h3`/WebTransport deferred per ADR-044, browser bidirectional path uses WebSocket), which consumes the `CredentialStore` trait and the `OperationAdapter` contract. The alknet-ssh crate (the other post-core crate, specced in parallel) proceeds independently — it depends on `alknet-core`, not `alknet-call`.

**alknet-tty specs drafted.** The alknet-tty crate (terminal session protocol handler — `ProtocolHandler` on `alknet/tty`, two-carriage wire format with a raw chunk codec + JSON control channel, backend-agnostic via a `TtyBackend` trait) now has architecture specs: [crates/tty/](crates/tty/) (overview, tty-wire, tty-backend, tty-adapter, tty-local) and six ADRs — [ADR-052](decisions/052-alknet-tty-wire-format-and-two-carriage.md) (wire format: `alknet/tty` ALPN, JSON negotiation frame then raw chunks, fixed channel set 0-3, control as JSON), [ADR-053](decisions/053-ttybackend-trait-and-ttyhandle.md) (`TtyBackend` trait + `TtyHandle`; `exit_code` as a `Future`; backends need not be natively async — REQ-TTY-01 from the local-PTY POC; `TtyControlHandle` newtype for `Clone`-ability — `Clone` is not object-safe), [ADR-054](decisions/054-local-tty-backend-sibling-crate.md) (`alknet-tty-local` sibling crate behind a `local` feature re-export; PTY vs pipe per-session; the runner pattern preserved), [ADR-055](decisions/055-exit-code-on-control-chunk.md) (exit code on a stream_type 3 control chunk; "exit chunk is last" invariant; adapter owns the ordering), [ADR-056](decisions/056-backend-cleanup-on-session-cancel.md) (backend cleanup contract: dropping the `exit_code` future on session cancel MUST kill the session target — closes the orphaned-process gap the local-PTY POC surfaced for the waiter thread), [ADR-057](decisions/057-alknet-tty-no-alknet-call-dep.md) (alknet-tty does not depend on alknet-call — the negotiation framing is self-contained; the earlier "reuse `FrameFramedReader`" claim was unsound because the utility is welded to `EventEnvelope` deserialization). The specs are grounded in the alknet-docker POC (`docs/research/alknet-docker/poc-summary.md`) and the alknet-tty POC (`/workspace/alknet-tty-poc/`, built 2026-07-05), which validated the wire format, the control channel, the local-PTY bridge, and the signal-delivery contract (REQ-TTY-02). The docker and SSH backends are future crates that implement the `TtyBackend` trait — out of scope for this spec set, but the trait shape is committed so they can be built against it.

**alknet-docker specs drafted.** The alknet-docker crate (docker operations on the shared `alknet/call` ALPN + `DockerTtyBackend` behind a `tty` feature) now has architecture specs: [crates/docker/](crates/docker/) (overview, docker-operations, docker-tty-backend) and six ADRs — [ADR-058](decisions/058-alknet-docker-on-alknet-call.md) (docker ops register on `alknet/call`, not a separate `alknet/docker` ALPN; the raw-carriage handoff the POC struggled with is dissolved by the alknet-tty extraction — interactive attach moved to `alknet/tty` via `DockerTtyBackend`, no `carriage` field on `call.requested`), [ADR-059](decisions/059-bollard-021-dependency-and-features.md) (bollard 0.21, verified current on crates.io; features `http`+`pipe`+`time`, no `ssl`/`ssh`/`websocket`/`buildkit` — single-host by construction, fleet is a call-protocol concern), [ADR-060](decisions/060-container-resource-model-and-label-namespace.md) (ADR-050 application to bollard: `alknet.managed`/`alknet.owner` labels; `list` `owned_only` flag; hosted-services operator role via the static-resource fallback; handler-driven `revoke` on `remove` with autonomous-death tolerance), [ADR-061](decisions/061-docker-tty-backend-in-alknet-docker.md) (`DockerTtyBackend` in alknet-docker behind a `tty` feature, not a sibling crate; attach vs exec mode; the POC's `drive_attach_raw` as the reference), [ADR-062](decisions/062-docker-client-injection-via-closure-capture.md) (the `Docker` client + `OwnershipStore` are closure-captured at registration time, not read from `OperationContext` and not smuggled through `Capabilities` — `Capabilities` is for secret material only per ADR-014; matches the `from_openapi` pattern), [ADR-063](decisions/063-exit-code-on-terminal-call-responded.md) (non-interactive exec puts `{ "exitCode": N, "terminal": true }` on a final `call.responded` before `call.completed` — `call.completed` stays empty, ADR-012 unchanged). The specs are grounded in the alknet-docker POC (`docs/research/alknet-docker/poc-summary.md`, `/workspace/alknet-docker-poc/`), which validated the hard parts (interactive attach, logs subscription, exec with exit code); the remaining lifecycle operations are mechanical bollard wrapping. The two use cases — disposable dev containers (coordinator-spawned, ownership-recorded) and long-running hosted services (operator-managed, static-resource fallback, per `/workspace/system/dev1/docker.md`) — both work through one `AccessControl` model (ADR-050/060). The `DockerTtyBackend` fills the `TtyBackend` row the alknet-tty spec left open. Four OQs (048–051) track deferred scope: network/volume ops, buildkit, system events subscription, and the full `CreateContainerOptions` surface (deferred to v1 implementation).

**Transport generalization sweep (2026-07-09).** Three commits landed a
clean sweep discovered when building an external app against the crates:
(1) the dead `irpc` / `irpc-derive` workspace deps were removed (no `.rs`
file ever imported irpc — the wire protocol is hand-rolled), recorded by
[ADR-064](decisions/064-irpc-never-integrated-hand-rolled-framing.md)
(supersedes ADR-005, which had accepted "irpc as the call protocol
foundation" based on the previous architecture but was never implemented
as stated); (2) the iroh dep migrated `0.35 → 1.0.2` (6 API surface edits,
no architectural change — unblocks `alknet-blobs`); (3)
[ADR-065](decisions/065-connection-from-stream-generic-single-stream.md)
adds `Connection::from_stream` / `from_bidi` — `Connection` now accepts any
`AsyncRead + AsyncWrite` pair, unblocking TCP+TLS, SSH channel dispatch,
WebTransport streams, and wasm streams through the same `HandlerRegistry`
as QUIC connections, with zero handler code changes. The
`MockConnection` / `ConnectionKind::Mock` test variants are removed (tests
use `from_stream` with `tokio::io::sink`/`empty`). See
[`docs/research/transport-generalization/findings.md`](../research/transport-generalization/findings.md)
for the full trace.

**`from_jsonschema` relocation (ADR-066).** The `from_jsonschema`
adapter was originally placed in `alknet-call` (ADR-017 §5) as a
schema-only adapter with a `NOT_FOUND`-returning placeholder handler —
broken, because an op in the registry needs a real handler.
[ADR-066](decisions/066-from-jsonschema-as-http-adapter.md) moves it to
`alknet-http` as a real reqwest-backed single-endpoint adapter
(functionally similar to `from_openapi`, but one endpoint at a time),
for non-standard / non-OpenAPI / basic REST endpoints that don't have a
full OpenAPI document. The `FromJsonSchema` provenance variant stays in
`alknet-call` (now a handler-bearing leaf, not a "no handler" entry).
The "schema-only, no handler" concept is removed — schema validation
without a handler is served by consuming `OperationSpec` directly. The
adapter location map is now consistent: all HTTP-backed adapters
(`from_openapi`, `from_mcp`, `from_jsonschema`) live in `alknet-http`.

## Architecture Documents

| Document | Status | Description |
|----------|--------|-------------|
| [overview.md](overview.md) | draft | Workspace-level overview, crate graph (core mono-repo scope per ADR-085), hub/worker model, shared types, design principles |
| [open-questions.md](open-questions.md) | draft | OQ index — theme-grouped tables + Deferred/Blocked section; per-OQ files in [`questions/`](questions/) |
| [crates/core/README.md](crates/core/README.md) | draft | alknet-core crate index — shared types + auth + config (endpoint in `alknet-endpoint` per ADR-083 Am. 2026-07-15; `ConnectionCredentials`/`RemoteIdentity` here per ADR-091) |
| [crates/core/core-types.md](crates/core/core-types.md) | draft | ProtocolHandler, HandlerError, Connection (`Box<dyn BidiStreamSource>` — ADR-070), BidiStreamSource trait, BiStream, StreamError |
| [crates/core/endpoint.md](crates/core/endpoint.md) | deprecated | Endpoint spec — **moved to `alknet-endpoint`** (ADR-083 Am. 2026-07-15); see [`crates/endpoint/README.md`](crates/endpoint/README.md) |
| [crates/core/auth.md](crates/core/auth.md) | draft | AuthContext (incl. `anonymous` constructor), Identity, IdentityProvider, AuthToken, resolution flow |
| [crates/core/config.md](crates/core/config.md) | draft | StaticConfig, DynamicConfig, ArcSwap, ConfigReloadHandle |
| [crates/call/README.md](crates/call/README.md) | draft | alknet-call crate index |
| [crates/call/call-protocol.md](crates/call/call-protocol.md) | draft | CallAdapter, hand-rolled EventEnvelope framing (no irpc — ADR-064), stream model, PendingRequestMap, bidirectional calls, streaming subscribe example |
| [crates/call/operation-registry.md](crates/call/operation-registry.md) | draft | OperationSpec, Handler, OperationRegistry, AccessControl, capability injection, service discovery (hand-rolled, no irpc) |
| [crates/call/client-and-adapters.md](crates/call/client-and-adapters.md) | draft | CallClient (transport-agnostic `spawn_dispatch` primary; dial in `AlknetClient` per ADR-089), from_call, OperationAdapter trait, adapter location map, no-env-vars invariant, exchange-of-operations pattern (`from_jsonschema` in alknet-http per ADR-066) |
| [crates/http/README.md](crates/http/README.md) | draft | alknet-http crate index |
| [crates/http/overview.md](crates/http/overview.md) | draft | Crate purpose, two roles (server + client host), dependencies, adapter location map |
| [crates/http/http-server.md](crates/http/http-server.md) | draft | HttpAdapter for h2/http1.1 + WebSocket upgrade route, axum over QUIC, Bearer auth, stealth, /healthz |
| [crates/http/websocket.md](crates/http/websocket.md) | draft | WebSocket browser bidirectional path — native `EventEnvelope` call-protocol session (not the gateway shape); framing, dispatch, bidirectionality, connection-local overlay, browsers-are-not-peers, deferred `from_wss` |
| [crates/http/http-adapters.md](crates/http/http-adapters.md) | draft | from_openapi (reqwest; JSON + YAML input per ADR-051), from_jsonschema (single-endpoint reqwest forwarding handler per ADR-066), and to_openapi (projection); no-env-vars injection point |
| [crates/http/http-mcp.md](crates/http/http-mcp.md) | draft | from_mcp / to_mcp (feature-gated), streamable-HTTP-only, stdio exclusion |
| [crates/http/webtransport.md](crates/http/webtransport.md) | deferred | h3/WebTransport handler — deferred per ADR-044; browser bidirectional path uses WebSocket (see http-server.md). Spec kept intact for revival. |
| [crates/tty/README.md](crates/tty/README.md) | draft | alknet-tty crate index |
| [crates/tty/overview.md](crates/tty/overview.md) | draft | Crate purpose, two-carriage model, dependencies, ALPN, backend location map, feature gates |
| [crates/tty/tty-wire.md](crates/tty/tty-wire.md) | draft | Wire format: negotiation frame (JSON carriage), raw chunk codec (`[stream_type: u8][length: u32 be][payload]`), control channel (stream_type 3, JSON control messages), sentinels |
| [crates/tty/tty-backend.md](crates/tty/tty-backend.md) | draft | `TtyBackend` trait, `TtyParams`, `TtyHandle`, `TtyControl` — the backend inversion point. Carries REQ-TTY-01 (backends need not be natively async) |
| [crates/tty/tty-adapter.md](crates/tty/tty-adapter.md) | draft | `TtyAdapter` (`ProtocolHandler` on `alknet/tty`): session lifecycle, three-pump bidirectional driver, negotiation errors, exit-chunk ordering (ADR-055), access control |
| [crates/tty/tty-local.md](crates/tty/tty-local.md) | draft | `alknet-tty-local` sibling crate: `LocalTtyBackend` via `portable_pty` (PTY) and `std::process::Command` (pipe/runner). Carries REQ-TTY-02 (signal forwarding to the process group) |
| [crates/docker/README.md](crates/docker/README.md) | draft | alknet-docker crate index |
| [crates/docker/overview.md](crates/docker/overview.md) | draft | Crate purpose, two-role design (call ops + DockerTtyBackend), dependencies, ALPN, label namespace, feature gates, assembly-layer wiring |
| [crates/docker/docker-operations.md](crates/docker/docker-operations.md) | draft | Operation surface: lifecycle (Query/Mutation), logs/exec/pull (Subscription via StreamingHandler), access control (ADR-050/060), label namespace, teardown coupling |
| [crates/docker/docker-tty-backend.md](crates/docker/docker-tty-backend.md) | draft | `DockerTtyBackend` (impl `TtyBackend`): attach vs exec mode, `TtyHandle` field mapping, `TtyControl` → bollard resize/signal, `exit_code` Drop-kill (ADR-056) |
| [crates/vault/README.md](crates/vault/README.md) | stable | alknet-vault crate index |
| [crates/vault/mnemonic-derivation.md](crates/vault/mnemonic-derivation.md) | stable | BIP39, SLIP-0010, BIP-0032, derivation paths, key types |
| [crates/vault/encryption.md](crates/vault/encryption.md) | stable | AES-256-GCM, EncryptedData, key versioning, salt (Phase B reserved) |
| [crates/vault/service.md](crates/vault/service.md) | stable | VaultServiceHandle lifecycle, direct dispatch, cache, error model |
| [crates/vault/protocol.md](crates/vault/protocol.md) | stable | DerivedKey redaction, KeyType, serialization behavior |
| [crates/hub/README.md](crates/hub/README.md) | draft | alknet-hub crate — composes a subset of three endpoint types (web/native/iroh — ADR-086), channels substrate (ADR-079 relay), worker registration flow (OQ-58), identity over transports, aggregated peer env, connection lifecycle, service discovery |
| [crates/tls/README.md](crates/tls/README.md) | reviewed | alknet-tls crate — shared TLS config (`TlsServerConfig` + `TlsClientConfig`) shared across quinn + TCP+TLS + iroh; one cert, one ACME state machine, N transports; split ALPN lists per endpoint type (ADR-086, resolves OQ-62); `FingerprintPinVerifier` in `alknet-tls` (ADR-089 §5); `webpki-roots` fallback for empty platform stores (ADR-088 §5); isolates cert-reuse from transport wrappers (ADR-082) |
| [crates/client/README.md](crates/client/README.md) | draft | alknet-client crate — the native client dial seam (`AlknetClient`), client-side analogue of `AlknetEndpoint`; three dials (QUIC + TCP+TLS via `TlsClientConfig`, iroh via key) unified on `&ConnectionCredentials` (ADR-091); optional SOCKS5 proxy (ADR-090 — UDP ASSOCIATE for QUIC, CONNECT for TCP+TLS, force-relay-only + HTTP-to-SOCKS5 bridge for iroh; OQ-67 resolved); produces `Connection` for `CallClient`/`ChannelClient` take-over; dial centralized here; `alknet/register` named (wire protocol deferred, OQ-66) |
| [crates/endpoint/README.md](crates/endpoint/README.md) | draft | alknet-endpoint crate — the server-side accept-loop runner (`AlknetEndpoint`), extracted from `alknet-core` (ADR-083 Am. 2026-07-15); takes pre-built transports via `with_quinn`/`with_iroh`/`with_tcp_tls`; public `dispatch` for SSH/WT; handler crates no longer transitively link quinn/iroh |
| [crates/channels/README.md](crates/channels/README.md) | draft | alknet-channels crate — multiplexing proxy, 8-byte chunk format, N channels over one transport stream |
| [crates/channels/overview.md](crates/channels/overview.md) | draft | Crate purpose, the multiplexing collapse, dependencies, transport agnosticism, WASM, relationship to existing crates |
| [crates/channels/channels-wire.md](crates/channels/channels-wire.md) | draft | 8-byte chunk format, the add/strip composition, sentinels, framing disambiguation, wire-level invariants (REQ-CH-01..05) |
| [crates/channels/channels-connection.md](crates/channels/channels-connection.md) | draft | `ChannelBidiStreamSource` (implements `BidiStreamSource`), `accept_bi` yields `BiStream`, recursive composition |
| [crates/channels/channels-adapter.md](crates/channels/channels-adapter.md) | draft | `ChannelsAdapter`, `ChannelManager`, demux/mux contracts (REQ-CH-01..04), two-pump pattern (ADR-078) |
| [crates/channels/channel-operations.md](crates/channels/channel-operations.md) | draft | `channel/open`/`close`/`control`/`resources/subscribe`, ACL flow, `direction` semantics, hub relay contract (ADR-079) |
| [crates/channels/channel-client.md](crates/channels/channel-client.md) | draft | `ChannelClient` — client side of a channels connection, transport-agnostic `from_connection` primary; dial lives in `AlknetClient` (ADR-089); bidirectionality preserved |

## ADR Table

| ADR | Title | Status |
|-----|-------|--------|
| [001](decisions/001-alpn-protocol-dispatch.md) | ALPN-Based Protocol Dispatch | Accepted |
| [002](decisions/002-protocol-handler-trait.md) | ProtocolHandler Trait | Accepted |
| [003](decisions/003-crate-decomposition.md) | Crate Decomposition | Accepted |
| [004](decisions/004-auth-as-shared-core.md) | Auth as Shared Core (IdentityProvider) | Accepted |
| [005](decisions/005-irpc-as-call-protocol-foundation.md) | irpc as Call Protocol Foundation | ~~Accepted~~ → **Superseded** by ADR-064 (irpc was never integrated) |
| [006](decisions/006-alpn-convention-and-connection-model.md) | ALPN String Convention and Connection Model | Accepted |
| [007](decisions/007-bistream-type-definition.md) | BiStream Type Definition | Accepted |
| [008](decisions/008-secret-service-integration.md) | Vault Integration Point | Accepted |
| [009](decisions/009-one-way-door-decision-framework.md) | One-Way Door Decision Framework | Accepted |
| [010](decisions/010-alpn-router-and-endpoint.md) | ALPN Router and Endpoint | Accepted (Amendment 1 superseded by ADR-083 — TCP+TLS is a first-class owned transport via `with_tcp_tls`) |
| [011](decisions/011-authcontext-structure.md) | AuthContext Structure and Resolution Flow | Accepted |
| [012](decisions/012-call-protocol-stream-model.md) | Call Protocol Stream Model | Accepted |
| [013](decisions/013-rust-canonical-implementation.md) | Rust as Canonical Implementation Language | Accepted |
| [014](decisions/014-secret-material-flow-and-capability-injection.md) | Secret Material Flow and Capability Injection | Accepted |
| [015](decisions/015-privilege-model-and-authority-context.md) | Privilege Model and Authority Context | Accepted |
| [016](decisions/016-abort-cascade-for-nested-calls.md) | Abort Cascade for Nested Calls | Accepted |
| [017](decisions/017-call-protocol-client-and-adapter-contract.md) | Call Protocol Client and Adapter Contract | Accepted (`from_jsonschema` clause superseded by ADR-066) |
| [018](decisions/018-vault-standalone-crate.md) | Vault as Standalone Crate | Accepted |
| [019](decisions/019-vault-assembly-layer-only.md) | Vault Assembly-Layer-Only Access | Accepted |
| [020](decisions/020-hd-derivation-for-encryption-keys.md) | HD Derivation for Encryption Keys | Accepted |
| [021](decisions/021-key-rotation-via-version-indexed-paths.md) | Key Rotation via Version-Indexed Paths | Accepted |
| [022](decisions/022-handler-registration-provenance-and-composition-authority.md) | Handler Registration, Provenance, and Composition Authority | Accepted (`FromJsonSchema` row superseded by ADR-066) |
| [023](decisions/023-operation-error-schemas.md) | Operation Error Schemas | Accepted |
| [024](decisions/024-operation-registry-layering.md) | Operation Registry Layering | Accepted |
| [025](decisions/025-vault-local-only-dispatch.md) | Vault Local-Only Dispatch | Accepted |
| [026](decisions/026-vault-key-model-hd-derivation.md) | Vault Key Model — HD Derivation | Accepted |
| [027](decisions/027-tls-identity-redesign-acme-rawkey-decoupling.md) | TLS Identity Redesign — ACME + RawKey Decoupling | Accepted (§5 amended by ADR-083 — guard moves to shared `dispatch`) |
| [028](decisions/028-callclient-peer-scoped-registry-filtering.md) | Peer-Scoped Registry Filtering for CallClient Inbound Dispatch | ~~Accepted~~ → **Superseded** by ADR-029 |
| [029](decisions/029-peer-graph-routing-model.md) | Peer-Graph Routing Model for alknet-call Composition | Accepted (Assumption 1's `PeerId` source superseded by ADR-030) |
| [030](decisions/030-peerentry-and-identity-id-decoupling.md) | PeerEntry and Identity.id Decoupling | Accepted (supersedes ADR-029 Assumption 1's UUID source) |
| [031](decisions/031-credentialstore-repo-trait.md) | CredentialStore Repo Trait | Accepted |
| [032](decisions/032-forwarded-for-identity.md) | Forwarded-For Identity (Metadata, Not Authority) | Accepted |
| [033](decisions/033-storage-boundary-and-repo-adapter-pattern.md) | Storage Boundary and Repo/Adapter Pattern | Accepted |
| [034](decisions/034-outgoing-only-x509-and-three-peer-roles.md) | Outgoing-Only X.509 and the Three Peer Roles | Accepted |
| [035](decisions/035-concrete-persistence-adapter-shapes.md) | Concrete Persistence Adapter Shapes — Read/Write Split, honker+SQLite | Accepted |
| [036](decisions/036-http-to-call-operation-mapping.md) | HTTP-to-Call Operation Mapping | Proposed — **routing decision superseded by ADR-047** (non-routing clauses survive: SSE, auth, `/healthz`, stealth, error mapping) |
| [037](decisions/037-mcp-stdio-transport-exclusion.md) | MCP Stdio Transport Exclusion | Proposed |
| [038](decisions/038-http3-and-webtransport-as-first-class.md) | HTTP/3 and WebTransport as First-Class HTTP Transports | ~~Proposed~~ → **Superseded** by ADR-044 |
| [039](decisions/039-http-server-and-client-host-colocated.md) | HTTP Server and Client Host Colocated in alknet-http | Proposed |
| [040](decisions/040-webtransport-alpn-stream-proxy.md) | WebTransport ALPN-Stream-Proxy | Proposed — **parked** (implementation deferred per ADR-044) |
| [041](decisions/041-mcp-tool-gateway-pattern.md) | MCP Tool-Gateway Pattern for to_mcp | Proposed |
| [042](decisions/042-openapi-gateway-pattern.md) | OpenAPI Gateway Pattern for to_openapi | Proposed |
| [043](decisions/043-webtransport-bidirectional-alpn-substrate.md) | WebTransport as a Bidirectional ALPN Transport Substrate | Proposed — **parked** (implementation deferred per ADR-044; §2/§3 transfer to WebSocket) |
| [044](decisions/044-defer-webtransport-browsers-use-websocket.md) | Defer h3/WebTransport; Browsers Use WebSocket | Accepted |
| [045](decisions/045-to-openapi-gateway-spec-versioning.md) | to_openapi Gateway-Spec Versioning | Proposed |
| [046](decisions/046-assembly-layer-custom-http-routes.md) | Assembly-Layer Custom HTTP Routes on HttpAdapter | Proposed |
| [047](decisions/047-remove-direct-call-http-surface.md) | Remove the Direct-Call HTTP Surface; Gateway Is the Sole Invoke Path | Proposed |
| [048](decisions/048-websocket-native-session-not-gateway.md) | WebSocket Carries the Native Call-Protocol Session, Not the Gateway Shape | Accepted |
| [049](decisions/049-streaming-handler-for-subscriptions.md) | Streaming Handler for Subscription Operations | Accepted |
| [050](decisions/050-dynamic-resource-ownership-for-runtime-spawned-resources.md) | Dynamic Resource Ownership for Runtime-Spawned Resources | Accepted |
| [051](decisions/051-yaml-input-for-from-openapi.md) | YAML Input Format for from_openapi | Accepted |
| [052](decisions/052-alknet-tty-wire-format-and-two-carriage.md) | alknet-tty Wire Format and Two-Carriage Model | Accepted |
| [053](decisions/053-ttybackend-trait-and-ttyhandle.md) | TtyBackend Trait and TtyHandle — the Backend Inversion Point | Accepted |
| [054](decisions/054-local-tty-backend-sibling-crate.md) | Local TTY Backend as a Sibling Crate (`alknet-tty-local`) | Accepted |
| [055](decisions/055-exit-code-on-control-chunk.md) | Exit Code on a Control Chunk (the Last Chunk Before Stream Close) | Accepted |
| [056](decisions/056-backend-cleanup-on-session-cancel.md) | Backend Cleanup on Session Cancel (Drop of `exit_code` Kills) | Accepted |
| [057](decisions/057-alknet-tty-no-alknet-call-dep.md) | alknet-tty Does Not Depend on alknet-call (Self-Contained Negotiation Framing) | Accepted |
| [058](decisions/058-alknet-docker-on-alknet-call.md) | alknet-docker Registers on `alknet/call` (No Separate ALPN) | Accepted |
| [059](decisions/059-bollard-021-dependency-and-features.md) | bollard 0.21 Dependency and Feature Selection | Accepted |
| [060](decisions/060-container-resource-model-and-label-namespace.md) | Container Resource Model and Label Namespace | Accepted |
| [061](decisions/061-docker-tty-backend-in-alknet-docker.md) | DockerTtyBackend in alknet-docker | Accepted |
| [062](decisions/062-docker-client-injection-via-closure-capture.md) | Docker Client and OwnershipStore Injection via Closure Capture | Accepted |
| [063](decisions/063-exit-code-on-terminal-call-responded.md) | Exit Code on a Terminal `call.responded` for Non-Interactive Exec | Accepted |
| [064](decisions/064-irpc-never-integrated-hand-rolled-framing.md) | irpc Was Never Integrated — Hand-Rolled EventEnvelope Framing | Accepted (supersedes ADR-005) |
| [065](decisions/065-connection-from-stream-generic-single-stream.md) | `Connection::from_stream` — Generic Single-Stream Connections | Accepted |
| [066](decisions/066-from-jsonschema-as-http-adapter.md) | `from_jsonschema` as HTTP-Backed Single-Endpoint Adapter in alknet-http | Accepted (supersedes the `from_jsonschema` clause of ADR-017 §5 and the `FromJsonSchema` provenance row of ADR-022) |
| [067](decisions/067-aggregated-peer-env-wiring.md) | Aggregated Peer-Environment Wiring for Hub Deployments | Proposed |
| [068](decisions/068-peer-composite-env-peer-operations.md) | PeerCompositeEnv::peer_operations Override | Proposed |
| [069](decisions/069-from-call-manual-free-function.md) | from_call Is a Manual Free Function, Not Auto-Wired | Proposed |
| [070](decisions/070-bidistreamsource-trait.md) | BidiStreamSource Trait — Open Connection for Extension | Accepted |
| [071](decisions/071-channels-wire-format.md) | alknet-channels Wire Format — 8-Byte Chunk Header | Accepted (amended by ADR-093 — 8-byte header, no `stream_type`) |
| [072](decisions/072-channel-0-pre-negotiated-call.md) | Channel 0 Is Pre-Negotiated `alknet/call` | Accepted (amended by ADR-093 — channel 0's `stream_types` field removed; the call protocol's framing is the channels payload) |
| [073](decisions/073-channel-lifecycle-operations.md) | Channel Lifecycle Operations on the Call Protocol | Accepted (amended by ADR-093 — `stream_types` field removed from `channel/open`; `stream_type` field removed from `channel/control`) |
| [074](decisions/074-channelconnection-bidistreamsource.md) | ChannelConnection — BidiStreamSource over Chunk Reassembly | Accepted (amended by ADR-093 — `into_sub_streams()` removed; `accept_bi` yields `BiStream`) |
| [075](decisions/075-channelsadapter-and-channelmanager.md) | ChannelsAdapter and ChannelManager | Accepted (amended by ADR-093 — 8-byte headers, one reassembly buffer per channel) |
| [076](decisions/076-backpressure-channel-limits-id-reuse.md) | Backpressure, Channel Limits, and ID Reuse | Accepted (amended by ADR-093 — per-`channel_id`, not per-`(channel_id, stream_type)`) |
| [077](decisions/077-tty-inside-channels.md) | TTY Inside Channels — Sub-Streams, Not Wire Format | Accepted (reversed by ADR-093 — TTY always uses its 5-byte format, carried transparently) |
| [078](decisions/078-two-pump-shutdown-on-completion.md) | Two-Pump Shutdown-on-Completion Pattern | Accepted |
| [079](decisions/079-hub-relay-translate-not-forward.md) | Hub Relay — Translate, Not Transparently Forward | Accepted |
| [080](decisions/080-channelclient.md) | ChannelClient — the Client Side of a Channels Connection | Accepted (amended by ADR-093 — `stream_types` field removed from `open_channel` and `Channel`) |
| [081](decisions/081-channels-subcrate-decomposition.md) | channels Sub-Crate Decomposition | Accepted (amended by ADR-093 — 8-byte wire format; `ChannelSubStreams`/`SubStreamHandle` removed) |
| [082](decisions/082-alknet-tls-extraction.md) | alknet-tls Crate Extraction | Accepted (amended — endpoint signature superseded by ADR-083) |
| [083](decisions/083-endpoint-as-accept-loop-runner.md) | Endpoint as Multi-Transport Accept-Loop Runner with Public Dispatch | Accepted (revised — TCP+TLS is an owned transport, not external; amended 2026-07-15 — endpoint extracted from `alknet-core` into `alknet-endpoint`; `EndpointError` removed — both variants vestigial, `shutdown()` infallible) |
| [084](decisions/084-aws-lc-rs-crypto-provider.md) | aws-lc-rs as the TLS Crypto Provider | Accepted |
| [085](decisions/085-workspace-scope-core-vs-consumer-repos.md) | Workspace Scope — Core vs. Consumer Repos | Accepted |
| [086](decisions/086-endpoint-types-and-entry-points.md) | Endpoint Types and Entry Points | Accepted |
| [087](decisions/087-tlsclientconfig-not-blocked-on-dial.md) | `TlsClientConfig` Not Blocked on Dial Seam | Accepted (§5 amended by ADR-089 — `FingerprintPinVerifier` in `alknet-tls`; `alknet-call` sheds TLS deps; input framing amended by ADR-091 — `TlsClientConfig::new` takes `ConnectionCredentials`, not `CallCredentials`) |
| [088](decisions/088-tlserror-shape.md) | `TlsError` Shape — Single Enum, Owned by `alknet-tls` | Accepted (§5 added — `webpki-roots` fallback when platform store is empty; §7 references ADR-089 for handshake-error surfacing) |
| [089](decisions/089-alknetclient-native-dial-seam.md) | AlknetClient — Native Client Dial Seam | Accepted (resolves OQ-55; `CallClient::connect` / `ChannelClient::connect_quic` removed; §3/§5 amended by ADR-091 — dial takes `ConnectionCredentials`, not `CallCredentials`; `CallCredentials` removed per ADR-091 Am. 2026-07-17; `FingerprintPinVerifier` moved to `alknet-tls`; `ClientError` removed; `alknet-call` sheds TLS deps) |
| [090](decisions/090-client-dial-socks5-proxy-seam.md) | Client-Dial SOCKS5 Proxy Seam | Accepted (§5 amended 2026-07-16 — OQ-67 resolved: iroh force-relay-only + HTTP-to-SOCKS5 bridge) |
| [091](decisions/091-connectioncredentials-decouple-dial-from-call.md) | `ConnectionCredentials` — Decouple Dial Credentials from Call Protocol | Accepted (amends ADR-089 §3/§5 and ADR-087 input framing; dial takes `ConnectionCredentials` not `CallCredentials`; all three dial signatures unified; `dial_iroh`'s `node_id` derived from `remote_identity`; `auth_token` is a per-request payload field; `CallCredentials` removed per Am. 2026-07-17) |
| [092](decisions/092-bistream-as-the-handler-leaf.md) | `BiStream` as the Handler Leaf — Unify the Split-Pair `accept_bi` | Accepted (amends ADR-070's `accept_bi` return type; amends ADR-065's `from_stream`/`from_bidi` constructors; amends ADR-074's `ChannelBidiStreamSource::accept_bi` return type; `Connection::from_stream` removed; `from_bidi` is the only public stream constructor) |
| [093](decisions/093-channels-pure-channel-multiplexing.md) | alknet-channels — Pure Channel Multiplexing (8-Byte Header, No `stream_type`) | Accepted (amends ADR-071 — 8-byte header; ADR-074 — `into_sub_streams` removed; reverses ADR-077 — TTY always uses its 5-byte format; amends the channels-facing clauses of ADR-072/073/075/076/080/081) |

## Open Questions

Open questions are tracked in [open-questions.md](open-questions.md) — an index of theme-grouped tables (68 OQs across 20 themes) with a cross-theme [Deferred / Blocked](open-questions.md#deferred--blocked) section surfacing the safe-exit deferrals. Each OQ lives in its own file under [`questions/`](questions/) (`NNN-slug.md`, mirroring the ADR convention).

## Document Lifecycle

| Status | Meaning | Transitions |
|--------|---------|-------------|
| `draft` | Under active development. May change significantly. | → `reviewed` when open questions are resolved |
| `reviewed` | Architecture is final. Implementation may begin. Changes require review. | → `stable` when implementation is complete and verified |
| `stable` | Locked. Changes require review and may warrant an ADR. | → `deprecated` when superseded |
| `deprecated` | Superseded. Kept for reference. | Removed when no longer referenced |

## References

- Pivot proposal: `docs/research/pivot/alpn-service-architecture.md`
- Cleanup plan: `docs/research/pivot/cleanup-plan.md`
- SDD process: `docs/sdd_process.md`
- Reference implementation: `/workspace/@alkdev/alknet-main/`