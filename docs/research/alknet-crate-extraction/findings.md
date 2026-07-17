# alknet-crate-extraction — Migration from the welded core to the extracted crates

**Status:** Findings in progress — mapping the existing code to the
target shape, phase by phase, so the migration can be ordered to keep
the tree compilable at each step.
**Date:** 2026-07-16
**Scope:** The three new crates (`alknet-tls`, `alknet-endpoint`,
`alknet-client`) + the prune of `alknet-core` and `alknet-call` + the
`alknet-http` residual fix. The specs are confirmed tight (reviewed +
amended); this doc is the *how* — what code moves where, in what order,
with what intermediate states.

---

## TL;DR

The migration is **additive-then-subtractive**: build all three new
crates first (no breakage), then prune the old code from core and call
(breakage confined to a single phase), then fix the http residual. Seven
phases (0-6), each leaving the workspace compilable. The heaviest single file
(`endpoint.rs`, 1606 lines) is not a monolith — it's three concerns
welded together, each going to a different destination.

---

## What exists now (the source map)

### `crates/alknet-core/src/endpoint.rs` — 1606 lines, three concerns

The file is not 1600 lines of one thing. It's three concerns that the
extraction separates:

| Lines | Concern | Destination | LOC |
|-------|---------|-------------|-----|
| 1-492 | `AlknetEndpoint` struct, `HandlerRegistry`, `dispatch_quinn`/`dispatch_iroh`, accept loops, `build_auth_context`, `has_iroh_identity` | `alknet-endpoint` | ~492 |
| 493-934 | `TlsSetup`, `build_rustls_server_config`, `build_quinn_server_config_from_rustls`, `RawKeyCertResolver`, `Ed25519SigningKey`, `AcceptAnyCertVerifier`, `SelfSignedCert`, `generate_self_signed_cert`, `load_cert_chain`, `load_private_key`, `build_iroh_endpoint` | `alknet-tls` (TLS setup) / assembly layer (iroh builder) | ~442 |
| 935-1606 | 42 test functions | split by subject | ~671 |

The endpoint struct + dispatch + accept loops (the part that goes to
`alknet-endpoint`) is ~492 lines of implementation — not 1600. The TLS
setup code (~442 lines) goes to `alknet-tls`. The tests (~671 lines)
split by what they test. The file *feels* monolithic because the
`#[cfg(feature = "quinn")]` gates weave the three concerns together,
but the extraction unwinds that.

### `crates/alknet-call/src/client/call_client.rs` — 930 lines

| Lines | Concern | Destination | LOC |
|-------|---------|-------------|-----|
| 40-88 | `RemoteIdentity`, `CallCredentials` (struct + builder) | `alknet-core` (new `credentials.rs` or `auth.rs`) | ~48 |
| 90-100 | `ClientError` enum | **removed** (only produced by `connect`) | ~10 |
| 102-187 | `CallClient` struct + `new` + `spawn_dispatch` | **stays** (the pure protocol take-over) | ~85 |
| 189-320 | `build_quinn_client_config`, `build_client_auth`, `select_server_verifier`, `load_platform_root_cert_store`, `load_cert_chain`, `load_private_key`, `Ed25519SigningKey`, `RawKeyClientCertResolver`, `NoClientCertResolver`, `FingerprintPinVerifier` | `alknet-tls` | ~130 |
| 321-640 | `CallConnection`, `Dispatcher` wiring, wire-protocol helpers | **stays** (protocol) | ~320 |
| 640-930 | Tests (use `connect`, `CallCredentials`, TLS helpers) | rewrite to use `spawn_dispatch` directly or `AlknetClient` | ~290 |

The call crate's prune is ~140 lines of implementation + ~290 lines of
tests that need rewriting. What remains is the pure protocol: `CallClient`
+ `CallConnection` + `Dispatcher` + the wire protocol.

### `crates/alknet-http/src/server/adapter.rs` — the residual

The `HttpAdapter::handle` method does `connection.accept_bi()` → wraps
the send/recv pair in a hand-rolled `QuicStream` (lines 271-300, an
`AsyncRead+AsyncWrite` adapter) → feeds it to `serve_io()`. But
`serve_io` already accepts *any* `AsyncRead+AsyncWrite` (line 244). The
`QuicStream` wrapper is a hand-rolled version of what
`Connection::from_bidi`/`from_stream` (ADR-065, already landed) provides
natively.

The residual: `handle` assumes a quinn-style multi-stream connection
(`accept_bi` returns a fresh bidi stream). For a `from_bidi` connection
(TCP+TLS), `accept_bi` yields the single bidi stream once (ADR-070's
yield-once contract). The `QuicStream` wrapper works but is unnecessary
— the streams from `accept_bi` are already `AsyncRead+AsyncWrite`. The
fix is to drop `QuicStream` and use the streams directly (or via
`TokioIo::new`). This is a small change (~30 lines removed) but needs
verification against ADR-070's `accept_bi` semantics for `from_bidi`.

The `QuicStreamDuplex` test helper (lines 456-471) is the test-side
equivalent — it can be replaced with `Connection::from_stream` test
helpers (the `MockConnection`/`ConnectionKind::Mock` variants were
already removed per ADR-065; tests use `from_stream` with
`tokio::io::sink`/`empty`).

---

## The seven phases

### Phase 0: Move `ConnectionCredentials`/`RemoteIdentity` to `alknet-core` (additive, no breakage)

**What:** Add `ConnectionCredentials` + `RemoteIdentity` to a new
`crates/alknet-core/src/credentials.rs` (~40 lines).
`ConnectionCredentials` is the transport-level credential bundle
(ADR-091) — it carries `local_identity` + `remote_identity` (the two
dimensions the dial consumes). `CallCredentials` stays in
`alknet-call` (it is the call-protocol bundle; its `auth_token` field
is a per-request call-protocol concept, not a transport credential).
Update `alknet-core/src/lib.rs` to `pub mod credentials` + re-export.
Update `alknet-call` to import `ConnectionCredentials` + `RemoteIdentity`
from core and re-export them; `CallCredentials` retains `auth_token` and
references the core types for the transport dimensions. No other
changes.

**Why first:** It's independent of the three new crates, purely
additive (core gains types, nothing breaks), and means `alknet-client`
(Phase 3) never has a temporary dep on `alknet-call`. The dep graph is
clean from the start. `ConnectionCredentials` (not `CallCredentials`)
is what moves — the dial consumes transport-level dimensions, not
call-protocol dimensions (ADR-091).

**Compilable state:** `cargo test` passes across the workspace.
`alknet-call` imports the types from core; its own code + tests
continue to work via the re-export.

**Done when:** `cargo test` passes, `ConnectionCredentials` +
`RemoteIdentity` are defined in `alknet-core`, `alknet-call` imports
them from core, `CallCredentials` stays in `alknet-call`.

### Phase 1: Create `alknet-tls` (greenfield, additive)

**What:** New crate `crates/alknet-tls/`. Extract the TLS setup code
from `alknet-core/endpoint.rs` (lines 493-934) + the client-side TLS
helpers from `alknet-call/client/call_client.rs` (lines 189-320).

**Types:** `TlsServerConfig`, `TlsClientConfig`, `TlsError`,
`FingerprintPinVerifier`, `RawKeyCertResolver`,
`RawKeyClientCertResolver`, `NoClientCertResolver`, `Ed25519SigningKey`
(consolidated — one copy, used by both server + client),
`AcceptAnyCertVerifier`, `SelfSignedCert`, `generate_self_signed_cert`,
`load_cert_chain`, `load_private_key`, `load_platform_root_cert_store`
(+ the `webpki-roots` fallback — new, not extracted).

**Deps:** `alknet-core` (TlsIdentity, Ed25519SecretKey, fingerprint),
`rustls`, `rustls-pemfile`, `rustls-native-certs`, `webpki-roots`,
`rcgen`, `tokio`, optional `quinn`/`tokio-rustls`/`rustls-acme`.
`rustls-native-certs` and `webpki-roots` are **always-present (not
feature-gated)** — the unknown-X.509-remote CA-verification path in
`TlsClientConfig::new` is transport-agnostic; the `webpki-roots`
fallback merges built-in roots when the platform store is empty so
`NoRootAnchors` is unreachable in practice (ADR-088 §5). Do not gate
them under `quinn`/`tcp`.

**Compilable state:** `alknet-tls` builds and tests standalone. Core and
call are unchanged — the old code still exists (duplicated). No
breakage. The new crate's tests are the moved tests from
`endpoint.rs` (the TLS-setup tests) + the moved tests from
`call_client.rs` (the verifier/client-config tests).

**Tests that move here (from `endpoint.rs`):**
- `raw_key_cert_resolver_only_raw_public_keys`
- `self_signed_cert_generation_produces_cert_and_key`
- `acme_directory_production_url` / `staging_url` / `custom_url`
- `tls_setup_x509_returns_no_acme_state`
- `build_rustls_server_config_raw_key_succeeds`
- `build_rustls_server_config_self_signed_succeeds`
- `build_quinn_server_config_from_rustls_succeeds`
- `load_private_key_returns_error_when_no_key_present` / `_file_missing`
- `load_cert_chain_returns_error_when_file_missing`
- `accept_any_cert_verifier_*` (4 tests)
- `ed25519_signing_key_*` (6 tests)
- `raw_key_cert_resolver_debug_is_implemented`

**Tests that move here (from `call_client.rs`):** the
`FingerprintPinVerifier` + `build_quinn_client_config` tests (need
adaptation — they currently test via `connect`, should test via
`TlsClientConfig::new` directly).

**Done when:** `cargo test -p alknet-tls` passes, the crate is
self-contained, no other crate changed.

### Phase 2: Create `alknet-endpoint` (greenfield, additive)

**What:** New crate `crates/alknet-endpoint/`. Extract the endpoint
struct + dispatch + accept loops from `alknet-core/endpoint.rs` (lines
1-492), built fresh against the ADR-083 shape (`new(handlers, dynamic,
identity_provider, drain_timeout)` + `with_quinn`/`with_iroh`/`with_tcp_tls`
+ public `dispatch` + `run`/`shutdown`). No `EndpointError` (removed per
the spec review). `shutdown()` is infallible.

**Types:** `AlknetEndpoint`, `HandlerRegistry`, `TcpTlsListener`,
private `dispatch_quinn`/`dispatch_iroh`/`dispatch_tcp_tls`,
`build_auth_context`, the `extract_*` helpers.

**Deps:** `alknet-core` (Connection, ProtocolHandler, AuthContext,
IdentityProvider, DynamicConfig), optional `quinn`/`iroh`/`tokio-rustls`,
`tokio`, `arc-swap`, `tracing`.

**Module breakdown (the 492 lines split):**
- `registry.rs` — `HandlerRegistry` (~40 lines + tests)
- `endpoint.rs` — `AlknetEndpoint` struct, `new`, builder methods,
  `run`, `shutdown` (~120 lines)
- `dispatch.rs` — `dispatch` (public), `build_auth_context`, the
  `acme-tls/1` guard (~80 lines)
- `accept/quinn.rs` — `dispatch_quinn`, `run_quinn_accept_loop`,
  `extract_quinn_alpn`, `extract_quinn_client_fingerprint` (~80 lines)
- `accept/iroh.rs` — `dispatch_iroh`, `run_iroh_accept_loop`,
  `extract_iroh_client_fingerprint` (~50 lines)
- `accept/tcp_tls.rs` — `dispatch_tcp_tls`, `run_tcp_tls_accept_loop`,
  `extract_tcp_tls_alpn`, `extract_tcp_tls_client_fingerprint` (new,
  ~60 lines — not in the current code; written fresh per ADR-083)

This is a fresh build, not a move — the old `endpoint.rs` stays in core
until Phase 4. The new crate is written against the target shape, not
the old shape.

**Compilable state:** `alknet-endpoint` builds and tests standalone.
Core's `endpoint.rs` still exists (duplicate). No breakage.

**Tests that move here (from `endpoint.rs`):**
- `handler_registry_*` (5 tests)
- `build_auth_context_*` (3 tests)
- `dispatch_decision_logic_lookup_and_auth`
- `has_iroh_identity_*` (3 tests)
- `endpoint_constructs_with_iroh_raw_key_identity`
- `iroh_endpoint_runs_accept_loop_and_shutdown`
- `debug_for_alknet_endpoint_is_implemented_without_panicking`

**Done when:** `cargo test -p alknet-endpoint` passes, the crate is
self-contained, no other crate changed.

### Phase 3: Create `alknet-client` (greenfield, additive)

**What:** New crate `crates/alknet-client/`. The `AlknetClient` dial
seam — three dial methods (`dial_quic`/`dial_tcp_tls`/`dial_iroh`),
all unified on `&ConnectionCredentials` (ADR-091), consuming
`TlsClientConfig` from `alknet-tls` + `ConnectionCredentials` from
`alknet-core`. The SOCKS5 proxy path (ADR-090) is feature-gated.

**Types:** `AlknetClient`, `ClientDialError`, `Socks5ProxyConfig`,
`Socks5Credentials` (behind `socks5` feature).

**Deps:** `alknet-core` (Connection, ConnectionCredentials,
RemoteIdentity, Ed25519SecretKey), `alknet-tls` (TlsClientConfig),
optional `quinn`/`tokio-rustls`/`iroh`/`fast-socks5`.

**Feature gates:** `quinn = ["dep:quinn", "alknet-tls/quinn",
"alknet-core/quinn"]`, `tcp = ["dep:tokio-rustls", "alknet-tls/tcp"]`,
`iroh = ["dep:iroh", "alknet-core/iroh"]`, `socks5 = ["dep:fast-socks5"]`.

**Compilable state:** `alknet-client` builds and tests standalone. No
breakage — the old `CallClient::connect` still exists in
`alknet-call` (duplicate dial). The new crate's tests use
`AlknetClient::dial_*` + `spawn_dispatch`/`from_connection` or test
the dial in isolation with mock transports.

**Done when:** `cargo test -p alknet-client` passes, the crate is
self-contained, no other crate changed.

### Phase 4: Prune `alknet-core` (subtractive, breakage confined)

**What:** Delete `endpoint.rs` from core. Remove `pub mod endpoint`
from `lib.rs`. Remove the heavy deps (`quinn`, `iroh`, `rcgen`,
`rustls-pemfile`, `rustls-acme`) from `Cargo.toml` — but keep the
`quinn`/`iroh` *features* (they gate `Connection::from_quinn`/
`from_iroh` in `types.rs`). Add `ConnectionCredentials` +
`RemoteIdentity` to core (from `alknet-call` — done in Phase 0;
`CallCredentials` stays in `alknet-call` per ADR-091).

**The `lib.rs` change:**
```rust
// Before:
pub mod endpoint;  // ← removed
pub mod auth;
pub mod config;
// ... rest unchanged

// After:
pub mod auth;
pub mod config;
pub mod credentials;  // ← new (ConnectionCredentials, RemoteIdentity)
// ... rest unchanged
```

**The `Cargo.toml` change:** `quinn`/`iroh`/`rcgen`/`rustls-pemfile`/
`rustls-acme` leave `[dependencies]`. The `quinn`/`iroh` features stay
(gating `types.rs` constructors). The `acme` feature is removed
(vestigial — the ACME state machine is on `TlsServerConfig` in
`alknet-tls` now).

**The `fingerprint.rs` doc comment** (line 13) references
`alknet_core::endpoint` — update to reference `alknet-endpoint` /
`alknet-tls`.

**Breakage:** anything that imported `alknet_core::endpoint` breaks.
But per the source map, *nothing does* — no handler crate or call
imports the endpoint module. The only consumer is the future assembly
layer (hub/worker), which doesn't exist yet. So the prune is clean:
delete the file, update `lib.rs`, update `Cargo.toml`, fix the
`fingerprint.rs` comment. Core's own tests for `endpoint.rs` are gone
(moved to `alknet-tls` in Phase 1 and `alknet-endpoint` in Phase 2).

**Compilable state:** `cargo test -p alknet-core` passes (minus the
endpoint tests, which moved). The `quinn`/`iroh` features still work
(`Connection::from_quinn`/`from_iroh` in `types.rs`).

**Done when:** `cargo test -p alknet-core` passes, the crate is
lightweight (~3200 LOC, no heavy transport deps).

### Phase 5: Prune `alknet-call` (subtractive, breakage confined)

**What:** Delete `connect()` + all TLS helpers + `ClientError` from
`call_client.rs`. The transport dimensions (`ConnectionCredentials`/
`RemoteIdentity`) already moved to core in Phase 0; here we remove the
old definitions from `call_client.rs` and update imports.
`CallCredentials` stays in `alknet-call` (retaining `auth_token`,
referencing the core types — ADR-091). Update `Cargo.toml` to drop
`quinn`/`rustls`/`rustls-native-certs`/`rustls-pemfile`. Rewrite the
tests that used `connect` to use `spawn_dispatch` directly (with
`Connection::from_stream` mocks) or `AlknetClient::dial_quic` +
`spawn_dispatch`.

**The `call_client.rs` after prune:**
- `CallClient` struct + `new` + `registry` + `identity_provider` +
  `spawn_dispatch` (~85 lines — unchanged)
- `CallConnection` + `Dispatcher` wiring (stays — protocol)
- `RemoteIdentity`/`ConnectionCredentials` — removed from
  `call_client.rs` (now in `alknet-core`, re-imported from there).
  `CallCredentials` stays in `alknet-call` (retaining `auth_token` —
  ADR-091).
- `ClientError` — removed
- `connect` + all `build_*`/`select_*`/`load_*`/`Ed25519SigningKey`/
  `RawKeyClientCertResolver`/`NoClientCertResolver`/
  `FingerprintPinVerifier` — removed (now in `alknet-tls`)

**The `Cargo.toml` after prune:** `quinn`, `rustls`,
`rustls-native-certs`, `rustls-pemfile` all leave. The `quinn` feature
is **removed** (it only gated `connect` + the TLS helpers, both
removed — keeping it as a no-op would mislead a user enabling
`quinn` on `alknet-call` expecting QUIC support; removing it surfaces
any stray `#[cfg(feature = "quinn")]` the prune missed). `alknet-call`
becomes a pure protocol crate.

**Test impact (per the test audit below):** the lib tests in
`call_client.rs` (16 tests) split into 6 that stay unchanged
(protocol-level, use `spawn_dispatch(stub_connection())`) and 10 that
move to `alknet-tls` in Phase 1 (TLS/verifier tests). **Zero lib tests
need rewriting** — no test in `call_client.rs` calls `connect()`. The
`from_call.rs` tests (27 tests) stay unchanged — they use
`CallConnection` directly. The one integration test file
(`tests/two_node_call.rs`, 2 tests) calls `connect()` twice — it moves
to `alknet-client/tests/` (Phase 3) or is updated to use
`AlknetClient::dial_quic` + `spawn_dispatch` (Phase 5).

The implementation prune is mechanical: delete the error enum, delete
`connect`, delete the TLS helpers (~140 lines). The test work is: the
10 TLS tests already moved in Phase 1, the 6 protocol tests stay, the
integration test is handled separately.

**Compilable state:** `cargo test -p alknet-call` passes with the
rewritten tests. The crate has no TLS/transport deps.

**Done when:** `cargo test -p alknet-call` passes, the crate is a pure
protocol crate.

### Phase 6: Fix `alknet-http` (small, additive)

**What:** Remove the `QuicStream` wrapper from `server/adapter.rs`.
The `HttpAdapter::handle` method does `connection.accept_bi()` →
wraps in `QuicStream` → feeds to `serve_io`. After the fix, it does
`connection.accept_bi()` → uses the streams directly (they're already
`AsyncRead+AsyncWrite`) → feeds to `serve_io` via `TokioIo::new`.

**The `QuicStream` struct** (lines 271-300) is deleted. The
`QuicStreamDuplex` test helper (lines 456-471) is replaced with
`Connection::from_stream` test helpers.

**Verification needed:** confirmed — `StreamBidiStreamSource::accept_bi`
(`types.rs:482`) yields the underlying `(SendStream, RecvStream)` pair
once (then `ConnectionClosed`). The `SendStream`/`RecvStream` are
`from_stream`-wrapped adapters (lines 267/289) — already
`AsyncRead`/`AsyncWrite`. So `accept_bi()` on a `from_bidi` connection
returns streams directly usable as `AsyncRead+AsyncWrite` — no
`QuicStream` wrapper needed. The `BidiStreamSource` trait has a unified
intent (`accept_bi`/`open_bi`/`remote_addr`/`close`); all three impls
(quinn, iroh, stream) collapse into adapters with the same shape. The
`QuicStream` wrapper is redundant — delete it, use the streams from
`accept_bi()` directly via `TokioIo::new`. The `QuicStreamDuplex` test
helper is replaced with `Connection::from_stream` test helpers (the
`stub_connection()` pattern already used in `call_client.rs` tests).

**Compilable state:** `cargo test -p alknet-http` passes. The crate no
longer has the hand-rolled `QuicStream` wrapper.

**Done when:** `cargo test -p alknet-http` passes, the `QuicStream`
wrapper is gone.

---

## Intermediate states (compilable after each phase)

| After phase | State |
|-------------|-------|
| 0 (credentials) | `ConnectionCredentials`/`RemoteIdentity` in core; call imports from core; `CallCredentials` stays in call; no breakage |
| 1 (tls) | `alknet-tls` builds standalone; core/call/http unchanged (old code duplicated) |
| 2 (endpoint) | `alknet-endpoint` builds standalone; core still has old `endpoint.rs` (duplicate) |
| 3 (client) | `alknet-client` builds standalone; call still has old `connect` (duplicate) |
| 4 (core prune) | core is lightweight; `endpoint.rs` gone; `ConnectionCredentials` in core |
| 5 (call prune) | call is pure protocol; `connect` + TLS helpers gone; Category B tests already moved |
| 6 (http fix) | http has no `QuicStream` wrapper; clean `accept_bi` path |

Phases 0-3 are purely additive — no existing code breaks, no tests
break. Phases 4-5 are subtractive — the pruned code's callers don't
exist yet (no assembly layer), so the breakage is confined to the
crate's own tests (and per the test audit, the call prune breaks zero
tests — the TLS tests moved in Phase 1, the protocol tests use
`spawn_dispatch` directly). Phase 6 is a small fix.

## Ordering rationale

The ordering is **deps before dependents, additive before subtractive**:

- **Phase 0** (`ConnectionCredentials` to core) first because it's
  independent, additive, and makes `alknet-client` (Phase 3) never
  depend on `alknet-call`. ~40 lines moved, zero breakage.
- `alknet-tls` (Phase 1) because both `alknet-endpoint` (indirectly —
  the assembly layer builds `TlsServerConfig`) and `alknet-client`
  (directly — `TlsClientConfig`) depend on it. It has no dep on the
  other new crates.
- `alknet-endpoint` (Phase 2) depends only on `alknet-core` (already
  exists) — it doesn't need `alknet-tls` (the endpoint takes
  pre-built transports). It could go before `alknet-tls`, but putting
  tls first means the assembly layer's transport-building code has a
  home from the start.
- `alknet-client` (Phase 3) depends on `alknet-tls`
  (`TlsClientConfig`) + `alknet-core` (`ConnectionCredentials` — moved
  in Phase 0, so the dep is clean from the start).
- Phases 4-5 (the prunes) go last because they're subtractive. The
  new crates (0-3) must exist first so the pruned code's
  functionality has a home.
- Phase 6 (http fix) goes last because it's independent of the
  extraction — it's a residual fix that could happen at any point
  after ADR-065 landed (which it did). Putting it last keeps the
  extraction phases clean.

## Resolved decisions

### `ConnectionCredentials`/`RemoteIdentity` move — Phase 0 (before Phase 1)

**Decision:** Move `ConnectionCredentials`/`RemoteIdentity` to
`alknet-core` as a standalone additive step *before any new crate is
created* (ADR-091). It's independent of everything else, purely
additive (core gains two small types, nothing breaks), and
`alknet-call` imports them from core so its own code + tests don't
change yet. This means `alknet-client` (Phase 3) never depends on
`alknet-call` — the dep graph is clean from the start, no temporary dep
to clean up later.

`ConnectionCredentials` (not `CallCredentials`) is what moves — it is
the transport-level credential bundle (`local_identity` +
`remote_identity`), carrying only the dimensions the dial consumes.
`CallCredentials` stays in `alknet-call` because its `auth_token` field
is a call-protocol / hub-layer concept (bearer-token identity
correlation for browsers and `alknet/register`), not a transport
credential. See ADR-091 for the full rationale.

The move is ~40 lines (struct definitions + builder impls) into a new
`crates/alknet-core/src/credentials.rs` (or `auth.rs` — `auth.rs`
already holds `AuthToken`, so `credentials.rs` is cleaner to keep the
auth module from growing). `alknet-call`'s `client/mod.rs` imports
`ConnectionCredentials` + `RemoteIdentity` from core and re-exports
them; `CallCredentials` stays defined in `alknet-call` (retaining
`auth_token`, referencing the core types for the transport dimensions).
Test changes are minimal — the Category A tests that reference
`CallCredentials` directly may need import updates depending on how
`CallCredentials` is restructured.

### Phase 5 test audit — `call_client.rs` (16 tests)

The 16 tests in `call_client.rs` split into three categories:

**Category A — protocol-level, stay in `alknet-call`, no rewrite
needed (6 tests):**

These tests use `spawn_dispatch(stub_connection())` or
`CallCredentials` directly. They don't touch `connect` or any TLS
helper. `stub_connection()` (line 582) uses
`Connection::from_stream(tokio::io::channel(...))` — already
transport-agnostic. These survive the prune unchanged.

| Test | Line | What it tests |
|------|------|---------------|
| `call_credentials_builder_methods` | 652 | `CallCredentials` builder |
| `external_op_dispatches_and_populates_capabilities` | 665 | dispatch + capabilities |
| `unknown_op_returns_not_found` | 679 | dispatch error path |
| `spawn_dispatch_returns_live_call_connection` | 691 | `spawn_dispatch` + ALPN |
| `call_client_is_send_sync` | 705 | trait bounds |
| `remote_identity_none_is_load_bearing_not_defaulted` | 921 | `CallCredentials::new()` |

**Category B — TLS/verifier tests, move to `alknet-tls` (10 tests):**

These test `FingerprintPinVerifier`, `build_client_auth`,
`select_server_verifier`, and `build_quinn_client_config` directly.
They're `#[cfg(feature = "quinn")]`-gated and test the TLS helpers,
not the call protocol. They move to `alknet-tls` in Phase 1 (adapted
to test `TlsClientConfig::new` instead of the free functions). The
two `build_quinn_client_config` tests test the full config build
(verifier + client-auth + provider wired together) and are adapted
to test `TlsClientConfig::new` + `for_quinn()` instead of the free
function.

| Test | Line | What it tests | Move target |
|------|------|---------------|-------------|
| `fingerprint_pin_verifier_matches_correct_ed25519_fingerprint` | 750 | verifier accept | `alknet-tls` |
| `fingerprint_pin_verifier_rejects_wrong_ed25519_fingerprint` | 769 | verifier reject | `alknet-tls` |
| `fingerprint_pin_verifier_matches_correct_sha256_fingerprint` | 789 | verifier X.509 accept | `alknet-tls` |
| `fingerprint_pin_verifier_rejects_wrong_sha256_fingerprint` | 806 | verifier X.509 reject | `alknet-tls` |
| `select_server_verifier_returns_ca_verifier_for_none` | 822 | CA path | `alknet-tls` |
| `select_server_verifier_returns_fingerprint_pin_for_some` | 839 | pin path | `alknet-tls` |
| `build_client_auth_presents_ed25519_raw_key_without_error` | 857 | client cert resolver | `alknet-tls` |
| `build_client_auth_none_resolves_to_no_client_cert` | 879 | no-cert resolver | `alknet-tls` |
| `build_quinn_client_config_with_raw_key_identity_builds_without_error` | 893 | full config build | `alknet-tls` |
| `build_quinn_client_config_with_no_remote_identity_builds_without_error` | 909 | CA-verify config | `alknet-tls` |

**Category C — `connect` integration test, remove (0 tests):**

No test in `call_client.rs` actually calls `connect()`. The tests that
exercise the full QUIC dial path are in `from_call.rs` (which has 27
tests) and in the integration tests. `call_client.rs`'s tests are all
either protocol-level (Category A) or TLS-helper-level (Category B).
This means the `connect` removal doesn't break any test in
`call_client.rs` itself — the tests that need a real connection already
use `spawn_dispatch(stub_connection())`.

**The `from_call.rs` tests (27 tests):** these use `CallConnection`
directly (constructed from `stub_connection()` or a mock), not
`connect`. They're protocol-level and stay in `alknet-call` unchanged.
The one reference to `connect()` is in a doc comment (line 76:
"the assembly layer calls `from_call` immediately after `connect()`")
— update the comment to say "after `AlknetClient::dial_*` +
`spawn_dispatch`".

**Net Phase 5 test impact:** 6 tests stay unchanged (Category A), 10
tests move to `alknet-tls` in Phase 1 (Category B), 0 tests need
rewriting. The `from_call.rs` tests (27) stay unchanged. The prune
of `call_client.rs` is mechanical: delete `ClientError`, `connect`,
and all the TLS helpers (`build_*`, `select_*`, `load_*`,
`Ed25519SigningKey`, `RawKeyClientCertResolver`, `NoClientCertResolver`,
`FingerprintPinVerifier`); keep `CallClient` + `new` + `spawn_dispatch`
unchanged; update imports. The test suite keeps the Category A tests,
removes the Category B tests (moved in Phase 1), and updates the one
doc comment.

This is much simpler than the initial estimate of "~290 lines of test
restructuring." The actual test work is: move 10 tests to `alknet-tls`
in Phase 1 (adapted to the new API), keep 6 tests unchanged, update one
doc comment. The `connect` removal breaks zero tests because no test
calls `connect`.

## Resolved questions

- **Phase 6 `accept_bi` semantics:** **Resolved.**
  `StreamBidiStreamSource::accept_bi` yields the `(SendStream,
  RecvStream)` pair once; the streams are already
  `AsyncRead`/`AsyncWrite` adapters. The `QuicStream` wrapper is
  redundant — delete it. The `BidiStreamSource` trait has a unified
  intent; all three impls (quinn, iroh, stream) collapse into adapters
  with the same shape. See Phase 6 above.
- **Integration test home:** **Resolved.** The dial + take-over
  composition test moves to `alknet-client/tests/` with a minimal echo
  `ProtocolHandler` on a test ALPN — no `alknet-call` dependency, no
  circular path. The call-protocol-specific tests stay in
  `alknet-call` (rewritten to use `spawn_dispatch` + loopback
  `Connection`). See Phase 5 / open questions above.
- **Workspace `Cargo.toml`:** the three new crates need to be added to
  the workspace member list. Trivial but worth noting.