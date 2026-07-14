---
status: draft
last_updated: 2026-07-14
---

# Alknet Architecture

## Current State

**alknet-channels specs drafted.** The alknet-channels crate (multiplexing
proxy — `ProtocolHandler` on `alknet/channels`, 9-byte chunk format, N
channels over transport stream(s), channel 0 pre-negotiated as
`alknet/call`) now has architecture specs:
[crates/channels/](crates/channels/) (overview, channels-wire,
channels-connection, channels-adapter, channel-operations, channel-client)
and eleven ADRs — [ADR-071](decisions/071-channels-wire-format.md) (9-byte
chunk header; revised for substrate simplification — the header is used in
all substrates including QUIC native, not just in-line; and stream_type
decomposition — every stream_type is unidirectional, grouped in threes:
0/1/2 = data write/read/err, 3/4/5 = control write/read/err, `% 3` formula;
resolves the TTY control channel's "not actually bidirectional" flaw),
[ADR-072](decisions/072-channel-0-pre-negotiated-call.md)
(channel 0 = `alknet/call` pre-negotiated, stream_types [0,1] — call frames
bidirectional via 0=in, 1=out),
[ADR-073](decisions/073-channel-lifecycle-operations.md) (channel
lifecycle operations on the call protocol — `channel/open`/`close`/
`control`/`resources/subscribe`; `channel/resources/subscribe` is a
`Subscription` operation using the already-implemented `StreamingHandler`
machinery, not a polled `Query`; the `direction` field pins who is the
ALPN-server; the control-message division is call-ops for orchestration,
`stream_type 3`/`4` for data-ordered control),
[ADR-074](decisions/074-channelconnection-bidistreamsource.md)
(`ChannelBidiStreamSource` implements `BidiStreamSource` — ADR-070's
extension point; `into_sub_streams()` with `SubStreamHandle` enum (Send/Recv
per unidirectional stream_type); `accept_bi()` generic path for tunnel/SSH),
[ADR-075](decisions/075-channelsadapter-and-channelmanager.md)
(`ChannelsAdapter` substrate-agnostic demux loop (reads 9-byte headers off
every bidi stream, regardless of substrate) + `ChannelManager`
reassemble/allocate split; REQ-CH-01..04 wire-level invariants pinned:
shutdown emits zero-length sentinel, transport close drops all senders, mux
dynamic registration, lenient unknown-`channel_id`),
[ADR-076](decisions/076-backpressure-channel-limits-id-reuse.md)
(bounded-buffer backpressure 1 MiB default, 256-channel cap, monotonic IDs
with wrap-around),
[ADR-077](decisions/077-tty-inside-channels.md) (TTY inside channels uses
sub-streams, not its own 5-byte wire format; 5 sub-streams [0,1,2,3,4]
with control properly bidirectional via 3 (write) + 4 (read); ADR-052's
scope amended to direct-connect TTY only; `channels` feature on alknet-tty),
[ADR-078](decisions/078-two-pump-shutdown-on-completion.md) (two-pump
handlers MUST shut down the opposite sink on pump completion — the
deadlock contract the POC surfaced; handler-level, not channels-layer;
core helper extraction deferred per OQ-57),
[ADR-079](decisions/079-hub-relay-translate-not-forward.md) (hub relay
translates `channel/open` on channel 0 with `forwarded_for` — ADR-032;
data channels byte-forwarded with `channel_id` rewrite; the hub never runs
protocol-specific handlers),
[ADR-080](decisions/080-channelclient.md) (`ChannelClient`,
transport-agnostic `from_connection` primary + `connect_quic` convenience,
bidirectionality preserved; `AlknetClient` dial-seam extraction stays
deferred per OQ-55 — blocked on a second *transport's* dial, not a second
client),
[ADR-081](decisions/081-channels-subcrate-decomposition.md) (sub-crate
decomposition — `channels-core` (pure multiplexer, depends on alknet-core
only, no call dependency) / `channels-call` (channel 0 pre-negotiation +
lifecycle op registrations, depends on channels-core + alknet-call) /
`channels-hub` (relay) / `channels-worker` (ChannelClient); isolates the
call-protocol coupling from the pure multiplexer). The specs are grounded
in the completed de-risk POC
(`docs/research/alknet-channels/poc-summary.md`, 28 tests passing, three
validated targets: chunk format + demux/mux, per-channel `Connection`
presentation, tunnel handler). The core prerequisite — ADR-070
(`BidiStreamSource` trait + `Connection::from_source`) — is landed and
implemented. The spec work converted three research hedges into decisions:
`channel/resources` is subscribe from day one (not poll-for-v1), channel
ID allocation is server-assigned (not "if zero-RTT needed"), and
backpressure is bounded-buffer (not "if HOL blocking becomes a problem").
Two genuine deferrals: OQ-56 (full windowing — blocked on a real HOL-
blocking observation) and OQ-57 (two-pump helper extraction — blocked on a
second two-pump handler). The TTY integration (ADR-077) amends ADR-052's
scope — the 5-byte format is unchanged for direct `alknet/tty` connections;
inside channels, TTY uses `into_sub_streams()` and the channels layer's
de-chunking, with control properly bidirectional via stream_types 3/4.

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
| [overview.md](overview.md) | draft | Workspace-level overview, crate graph, shared types, design principles |
| [open-questions.md](open-questions.md) | draft | OQ index — theme-grouped tables + Deferred/Blocked section; per-OQ files in [`questions/`](questions/) |
| [crates/core/README.md](crates/core/README.md) | draft | alknet-core crate index |
| [crates/core/core-types.md](crates/core/core-types.md) | draft | ProtocolHandler, HandlerError, Connection (`Box<dyn BidiStreamSource>` — ADR-070), BidiStreamSource trait, BiStream, StreamError |
| [crates/core/endpoint.md](crates/core/endpoint.md) | draft | ALPN router, HandlerRegistry, accept loop, shutdown |
| [crates/core/auth.md](crates/core/auth.md) | draft | AuthContext (incl. `anonymous` constructor), Identity, IdentityProvider, AuthToken, resolution flow |
| [crates/core/config.md](crates/core/config.md) | draft | StaticConfig, DynamicConfig, ArcSwap, ConfigReloadHandle |
| [crates/call/README.md](crates/call/README.md) | draft | alknet-call crate index |
| [crates/call/call-protocol.md](crates/call/call-protocol.md) | draft | CallAdapter, hand-rolled EventEnvelope framing (no irpc — ADR-064), stream model, PendingRequestMap, bidirectional calls, streaming subscribe example |
| [crates/call/operation-registry.md](crates/call/operation-registry.md) | draft | OperationSpec, Handler, OperationRegistry, AccessControl, capability injection, service discovery (hand-rolled, no irpc) |
| [crates/call/client-and-adapters.md](crates/call/client-and-adapters.md) | draft | CallClient (transport-agnostic `spawn_dispatch` primary, `connect` QUIC convenience — ADR-017 Am. 2026-07-13), from_call, OperationAdapter trait, adapter location map, no-env-vars invariant, exchange-of-operations pattern (from_jsonschema moved to alknet-http per ADR-066) |
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
| [crates/hub/README.md](crates/hub/README.md) | draft | alknet-hub crate — multi-transport endpoint (TCP+TLS + QUIC), channels substrate (ADR-079 relay), worker registration flow (OQ-58), identity over transports, aggregated peer env, connection lifecycle, service discovery |
| [crates/tls/README.md](crates/tls/README.md) | draft | alknet-tls crate — shared TLS config (`TlsServerConfig`) extractable across quinn + TCP+TLS + iroh; one cert, one ACME state machine, N transports; fixes cert-reuse welding in `alknet-core/endpoint.rs` (ADR-082) |
| [crates/channels/README.md](crates/channels/README.md) | draft | alknet-channels crate — multiplexing proxy, 9-byte chunk format, N channels over one transport stream |
| [crates/channels/overview.md](crates/channels/overview.md) | draft | Crate purpose, the multiplexing collapse, dependencies, transport agnosticism, WASM, relationship to existing crates |
| [crates/channels/channels-wire.md](crates/channels/channels-wire.md) | draft | 9-byte chunk format, stream types, sentinels, framing disambiguation, wire-level invariants (REQ-CH-01..05) |
| [crates/channels/channels-connection.md](crates/channels/channels-connection.md) | draft | `ChannelBidiStreamSource` (implements `BidiStreamSource`), `into_sub_streams()` typed accessor, recursive composition |
| [crates/channels/channels-adapter.md](crates/channels/channels-adapter.md) | draft | `ChannelsAdapter`, `ChannelManager`, demux/mux contracts (REQ-CH-01..04), two-pump pattern (ADR-078) |
| [crates/channels/channel-operations.md](crates/channels/channel-operations.md) | draft | `channel/open`/`close`/`control`/`resources/subscribe`, ACL flow, `direction` semantics, hub relay contract (ADR-079) |
| [crates/channels/channel-client.md](crates/channels/channel-client.md) | draft | `ChannelClient` — client side of a channels connection, transport-agnostic `from_connection` primary + `connect_quic` convenience, bidirectionality preserved |

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
| [071](decisions/071-channels-wire-format.md) | alknet-channels Wire Format — 9-Byte Chunk Header | Accepted |
| [072](decisions/072-channel-0-pre-negotiated-call.md) | Channel 0 Is Pre-Negotiated `alknet/call` | Accepted |
| [073](decisions/073-channel-lifecycle-operations.md) | Channel Lifecycle Operations on the Call Protocol | Accepted |
| [074](decisions/074-channelconnection-bidistreamsource.md) | ChannelConnection — BidiStreamSource over Chunk Reassembly | Accepted |
| [075](decisions/075-channelsadapter-and-channelmanager.md) | ChannelsAdapter and ChannelManager | Accepted |
| [076](decisions/076-backpressure-channel-limits-id-reuse.md) | Backpressure, Channel Limits, and ID Reuse | Accepted |
| [077](decisions/077-tty-inside-channels.md) | TTY Inside Channels — Sub-Streams, Not Wire Format | Accepted (amends ADR-052 scope — 5-byte format scoped to direct TTY) |
| [078](decisions/078-two-pump-shutdown-on-completion.md) | Two-Pump Shutdown-on-Completion Pattern | Accepted |
| [079](decisions/079-hub-relay-translate-not-forward.md) | Hub Relay — Translate, Not Transparently Forward | Accepted |
| [080](decisions/080-channelclient.md) | ChannelClient — the Client Side of a Channels Connection | Accepted |
| [081](decisions/081-channels-subcrate-decomposition.md) | channels Sub-Crate Decomposition | Accepted |
| [082](decisions/082-alknet-tls-extraction.md) | alknet-tls Crate Extraction | Proposed (amended — endpoint signature superseded by ADR-083) |
| [083](decisions/083-endpoint-as-accept-loop-runner.md) | Endpoint as Multi-Transport Accept-Loop Runner with Public Dispatch | Proposed (revised — TCP+TLS is an owned transport, not external) |

## Open Questions

Open questions are tracked in [open-questions.md](open-questions.md) — an index of theme-grouped tables (55 OQs across 17 themes) with a cross-theme [Deferred / Blocked](open-questions.md#deferred--blocked) section surfacing the safe-exit deferrals. Each OQ lives in its own file under [`questions/`](questions/) (`NNN-slug.md`, mirroring the ADR convention).

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