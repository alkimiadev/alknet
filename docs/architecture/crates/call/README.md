---
status: draft
last_updated: 2026-07-17
review: call/review-call passed 2026-06-23 — registry, protocol, ADR (005/012/014/015/016/017/022/023/024), security, and pattern-consistency checks all conformant; 159 unit/integration tests green; `cargo build`, `cargo clippy -- -D warnings`, `cargo fmt --check`, `cargo test` clean. Call-completion gap (ADR-017 client/adapter surface) addressed 2026-06-26; ADR-029 migration landed. Transport generalization sweep (ADR-064 supersedes ADR-005; ADR-065 `from_stream`) synced 2026-07-09. Crate-extraction sweep (phases 0–5) landed 2026-07-17: `ConnectionCredentials`/`RemoteIdentity` in `alknet-core` (ADR-091); TLS helpers in `alknet-tls` (ADR-089 §5); dial in `alknet-client` (ADR-089); `alknet-call` is a pure protocol crate with no TLS/transport deps.
---

# alknet-call

Structured RPC: operations, request/response, streaming subscriptions, and service discovery. Implements `ProtocolHandler` on ALPN `alknet/call`. Runs over QUIC (quinn/iroh) and, via `Connection::from_stream` (ADR-065), over any `AsyncRead + AsyncWrite` transport. A pure protocol crate — no TLS or transport deps (the dial is in `alknet-client`, the TLS config is in `alknet-tls`).

## Documents

| Document | Status | Description |
|----------|--------|-------------|
| [call-protocol.md](call-protocol.md) | draft | CallAdapter, hand-rolled EventEnvelope framing (no irpc — ADR-064), stream model, PendingRequestMap, bidirectional calls |
| [operation-registry.md](operation-registry.md) | draft | OperationSpec, Handler, OperationRegistry, AccessControl, service discovery, hand-rolled framing (no irpc — ADR-064) |
| [client-and-adapters.md](client-and-adapters.md) | draft | CallClient (transport-agnostic `spawn_dispatch` primary; dial lives in `AlknetClient` per ADR-089), from_call, OperationAdapter trait, adapter location map, no-env-vars invariant, exchange-of-operations pattern (`from_jsonschema` in alknet-http per ADR-066) |

## Applicable ADRs

| ADR | Title | Relevance |
|-----|-------|-----------|
| [001](../../decisions/001-alpn-protocol-dispatch.md) | ALPN-Based Protocol Dispatch | CallAdapter registers on ALPN `alknet/call` |
| [002](../../decisions/002-protocol-handler-trait.md) | ProtocolHandler Trait | CallAdapter implements ProtocolHandler |
| [003](../../decisions/003-crate-decomposition.md) | Crate Decomposition | alknet-call depends on alknet-core (no irpc — ADR-064) |
| [013](../../decisions/013-rust-canonical-implementation.md) | Rust as Canonical Implementation Language | Adapter traits defined in Rust; TS is reference/browser adaptation |
| [004](../../decisions/004-auth-as-shared-core.md) | Auth as Shared Core | AuthContext passed to call handlers |
| [005](../../decisions/005-irpc-as-call-protocol-foundation.md) | ~~irpc as Call Protocol Foundation~~ | ~~Accepted~~ → **Superseded** by [ADR-064](../../decisions/064-irpc-never-integrated-hand-rolled-framing.md) (irpc was never integrated; framing is hand-rolled) |
| [064](../../decisions/064-irpc-never-integrated-hand-rolled-framing.md) | Hand-Rolled EventEnvelope Framing | Wire format, registry, dispatch are hand-rolled in alknet-call; supersedes ADR-005 |
| [065](../../decisions/065-connection-from-stream-generic-single-stream.md) | `Connection::from_stream` | Generic single-stream connections; unblocks TCP+TLS/SSH/WT/wasm dispatch |
| [006](../../decisions/006-alpn-convention-and-connection-model.md) | ALPN String Convention | `alknet/call` ALPN, one ALPN per connection |
| [007](../../decisions/007-bistream-type-definition.md) | BiStream Type Definition | CallAdapter receives Connection, not BiStream |
| [008](../../decisions/008-secret-service-integration.md) | Vault Integration Point | Vault accessed at assembly layer, not on the wire |
| [010](../../decisions/010-alpn-router-and-endpoint.md) | ALPN Router and Endpoint | Static handler registration |
| [012](../../decisions/012-call-protocol-stream-model.md) | Call Protocol Stream Model | Bidirectional streams, EventEnvelope, ID-based correlation |
| [014](../../decisions/014-secret-material-flow-and-capability-injection.md) | Secret Material Flow and Capability Injection | Call protocol carries no secret material; capabilities injected at assembly layer |
| [015](../../decisions/015-privilege-model-and-authority-context.md) | Privilege Model and Authority Context | `internal` = authority switch not ACL skip; External/Internal visibility; handler identity + scoped env |
| [016](../../decisions/016-abort-cascade-for-nested-calls.md) | Abort Cascade for Nested Calls | `call.aborted` cascades to descendants; default `abort-dependents`, `continue-running` opt-in |
| [017](../../decisions/017-call-protocol-client-and-adapter-contract.md) | Call Protocol Client and Adapter Contract | `CallClient` opens connections; `from_call` imports remote ops; connection direction independent of call direction |
| [066](../../decisions/066-from-jsonschema-as-http-adapter.md) | `from_jsonschema` as HTTP-Backed Single-Endpoint Adapter in alknet-http | Moved `from_jsonschema` from `alknet-call` (broken schema-only placeholder) to `alknet-http` as a real reqwest-backed single-endpoint adapter; `FromJsonSchema` provenance stays in `alknet-call` as a leaf |
| [022](../../decisions/022-handler-registration-provenance-and-composition-authority.md) | Handler Registration, Provenance, and Composition Authority | Registration bundle carries provenance, composition authority, scoped env, capabilities |
| [023](../../decisions/023-operation-error-schemas.md) | Operation Error Schemas | Operations declare domain errors; `call.error` carries typed `details`; adapter fidelity |
| [024](../../decisions/024-operation-registry-layering.md) | Operation Registry Layering | Curated (static) + session/connection overlays (dynamic); `OperationEnv` as trait-object integration point; `OperationContext.env` split into `scoped_env` (data) and `env` (dispatch trait) |
| [028](../../decisions/028-callclient-peer-scoped-registry-filtering.md) | ~~Peer-Scoped Registry Filtering~~ | ~~Accepted~~ → **Superseded** by ADR-029 (flat-namespace single-peer model couldn't express head→N-workers; parallel auth system duplicated `AccessControl`) |
| [029](../../decisions/029-peer-graph-routing-model.md) | Peer-Graph Routing Model | Peer-keyed overlays + `PeerRef` routing; `AccessControl`-based peer authorization; retires `remote_safe`/`trusted_peer` |
| [030](../../decisions/030-peerentry-and-identity-id-decoupling.md) | PeerEntry and Identity.id Decoupling | `PeerId` source = `Identity.id` = `PeerEntry.peer_id` (stable); supersedes ADR-029's UUID source |
| [032](../../decisions/032-forwarded-for-identity.md) | Forwarded-For Identity | `forwarded_for` on `OperationContext` and `call.requested`; metadata only, never used by `AccessControl::check` |
| [033](../../decisions/033-storage-boundary-and-repo-adapter-pattern.md) | Storage Boundary and Repo/Adapter Pattern | Core defines repo traits + in-memory defaults; persistence adapters are separate crates |
| [089](../../decisions/089-alknetclient-native-dial-seam.md) | AlknetClient — Native Client Dial Seam | The dial is in `alknet-client`; `CallClient` is `spawn_dispatch` only; `alknet-call` is a pure protocol crate with no TLS/transport deps |
| [091](../../decisions/091-connectioncredentials-decouple-dial-from-call.md) | `ConnectionCredentials` — Decouple Dial from Call Protocol | `ConnectionCredentials`/`RemoteIdentity` in `alknet-core` (not `alknet-call`); `auth_token` is a per-request payload field |

## Relevant Open Questions

| OQ | Title | Status | Relevance |
|----|-------|--------|-----------|
| OQ-07 | Call protocol scope within a connection | resolved (ADR-012) | Stream model, multiplexing, scope |
| OQ-13 | Operation path format and routing scope | resolved | `/{service}/{op}` is the correct design; remote dispatch is a separate layer |
| OQ-14 | Batch operation semantics | resolved | Correlated `call.requested` events is the correct protocol design |
| OQ-16 | Safe vault operations for call protocol exposure | resolved (ADR-014) | None exposed for now |
| OQ-19 | Session-scoped operation registries | resolved | Agent-written operations overlaid on curated registry via `OperationEnv` trait layering. Protocol doesn't need changes; `OperationEnv` must remain a trait. Generalized by ADR-024 to cover connection-scoped overlays. |
| OQ-25 | ~~Remote-safe marking shape~~ | **dissolved** (ADR-029) | `remote_safe`/`trusted_peer` retired; peer authorization is `AccessControl::check(peer_identity)` |
| OQ-26 | OperationAdapter error type (AdapterError variants) | **resolved** | `DiscoveryFailed`, `SchemaParse`, `Transport`, `Unauthorized`, `SamePeerCollision`; `#[non_exhaustive]` |
| OQ-27 | from_call re-import trigger | **resolved** | `from_call` is a manual free function; the assembly layer calls it after the dial (in `AlknetClient`). `refresh()` is a genuine feature addition. See ADR-069. |
| OQ-28 | from_call namespace collision | **resolved** | Same-peer collision = error; cross-peer dissolved by ADR-029 (separate sub-overlays) |
| OQ-29 | CallClient TLS client-auth | **resolved** | Wire quinn client-auth; key-type-aware server cert verification; fingerprint normalization |
| OQ-30 | `PeerRef::Any` routing policy | **resolved** | Insertion-order first-match; richer routing is a feature extension |
| OQ-31 | `services/list-peers` re-export semantics | **resolved** | Opt-in `services/list-peers`; `services/list` is "own ops only" |
| OQ-32 | Multi-hop federation | open (feature extension) | One-hop model is the commitment; multi-hop is a feature extension, not a deferral |
| OQ-33 | PeerId — crypto identity vs stable logical id | **resolved** (ADR-030) | `PeerId = Identity.id = PeerEntry.peer_id` (stable across key rotation) |
| OQ-34 | Persistent peer registry | **resolved** (ADR-030+033) | Core trait + in-memory default; persistence adapters are separate crates |
| OQ-35 | ~~API key asymmetry~~ | **dissolved** | `PeerEntry` supports multiple credential paths; `ApiKeyEntry` is for tokens that ARE the identity |
| OQ-37 | X.509 outgoing-only case | **resolved** (ADR-034) | Three remote roles (public X.509 endpoint, transport relay, hub); `PeerEntry` asymmetry correct; verifier by `PeerEntry` presence |

## Key Design Principles

1. **One connection, full access**: An `alknet/call` connection gives access to the entire operation registry — calls, subscriptions, batch, schema.
2. **Protocol is symmetric**: Both sides can initiate calls. The server calling a client uses the same EventEnvelope format and correlation.
3. **Stream-agnostic correlation**: PendingRequestMap correlates by request ID, not by stream. The protocol works with any stream arrangement.
4. **Operation registry is layered**: The curated layer (`Local` provenance) is static — registered at startup by the CLI binary, immutable for the process lifetime. Session (`Session`) and imported (`FromCall` etc.) ops are dynamic overlays at their respective scopes (per-session, per-connection). The registry supports JSON Schema discovery. See ADR-024.
5. **Hand-rolled dispatch (no irpc)**: Operations dispatch through the hand-rolled `OperationRegistry` (ADR-064). The call protocol is the external interface; internal handler dispatch uses `Handler`/`StreamingHandler` trait objects (ADR-049), not an irpc service.
6. **Local dispatch only**: The operation registry dispatches to local handlers. Remote dispatch (federation, head/worker routing) would be a separate mechanism at a different layer, not a modification to alknet-call's path format.
7. **No secret material on the wire**: The call protocol carries no private keys, API keys, mnemonics, or decrypted credentials. Handlers receive outbound credentials through `OperationContext.capabilities`, injected at the assembly layer. See ADR-014.
8. **Abort cascades to descendants**: `call.aborted` for a parent request cascades to all non-terminal descendants. Default `abort-dependents`; `continue-running` opt-in. See ADR-016.
9. **Internal calls switch authority context, not skip ACL**: The `internal` flag marks composition-originated calls. ACL runs against the handler's composition authority, not the caller's and not as a blanket skip. Operations have External/Internal visibility. Scoped composition env bounds reachability. See ADR-015, ADR-022.
10. **Provenance determines composition capability**: Only `Local` and `Session` ops can compose. Leaves (`FromOpenAPI`, `FromMCP`, `FromCall`, `FromJsonSchema`) are forwarding stubs — they don't get composition authority or a scoped env. The assembly layer is the sole grantor of composition authority. See ADR-022. (`FromJsonSchema` is now a real HTTP-forwarding leaf per ADR-066, not a schema-only placeholder.)
11. **Connection direction is independent of call direction**: Who opens the connection is a connection-layer concern, not a protocol-layer concern. Both sides can call each other once connected. The `CallAdapter` accepts connections; the `CallClient` takes them over (`spawn_dispatch` primary; dial in `AlknetClient` per ADR-089); both produce the same `CallConnection` and dispatch through the same loop. See ADR-017, [client-and-adapters.md](client-and-adapters.md).
12. **Peer authorization via `AccessControl`**: A remote peer's call is authorized by `AccessControl::check(peer_identity)` against the op's `AccessControl` — the same mechanism that gates every other call. No `remote_safe` flag, no `trusted_peer` bypass. An op with `AccessControl::default()` is callable by any peer; an op with `required_scopes` is callable only by peers whose `Identity.scopes` satisfy them; an op with `Visibility::Internal` is never callable from the wire. See ADR-029.
13. **Adapter trait lives with the types; implementations live with their transport**: `OperationAdapter` is in `alknet-call`; `from_call` is in `alknet-call` (QUIC); `from_jsonschema`/`from_openapi`/`from_mcp`/`to_openapi`/`to_mcp` are in `alknet-http` (reqwest / axum). `alknet-call` stays lean — no HTTP client, no HTTP server. (`from_jsonschema` was originally in `alknet-call` as a schema-only placeholder; ADR-066 moved it to `alknet-http` as a real HTTP-backed adapter.) See [client-and-adapters.md](client-and-adapters.md).
14. **No handler reads outbound credentials from any source other than `OperationContext.capabilities`** (no-env-vars invariant): the credential injection path is vault → assembly layer → `Capabilities` → `HandlerRegistration.capabilities` → `OperationContext.capabilities` → handler. Downstream consumers' `std::env::var` reads are unreachable because the assembly layer never calls `Default::default()`. See ADR-014, [client-and-adapters.md](client-and-adapters.md).