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
(breakage confined to a single phase), then fix the http residual. Six
phases, four compilable intermediate states. The heaviest single file
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

## The six phases

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
consuming `TlsClientConfig` from `alknet-tls` + `CallCredentials` from
`alknet-core`. The SOCKS5 proxy path (ADR-090) is feature-gated.

**Types:** `AlknetClient`, `ClientDialError`, `Socks5ProxyConfig`,
`Socks5Credentials` (behind `socks5` feature).

**Deps:** `alknet-core` (Connection, CallCredentials, RemoteIdentity,
Ed25519SecretKey), `alknet-tls` (TlsClientConfig), optional
`quinn`/`tokio-rustls`/`iroh`/`fast-socks5`.

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
`from_iroh` in `types.rs`). Add `CallCredentials` + `RemoteIdentity`
to core (from `alknet-call`).

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
pub mod credentials;  // ← new (CallCredentials, RemoteIdentity)
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
`call_client.rs`. Move `CallCredentials`/`RemoteIdentity` to core
(already done in Phase 4 — here we just remove the old definitions +
update imports). Update `Cargo.toml` to drop `quinn`/`rustls`/
`rustls-native-certs`/`rustls-pemfile`. Rewrite the tests that used
`connect` to use `spawn_dispatch` directly (with `Connection::from_stream`
mocks) or `AlknetClient::dial_quic` + `spawn_dispatch`.

**The `call_client.rs` after prune:**
- `CallClient` struct + `new` + `registry` + `identity_provider` +
  `spawn_dispatch` (~85 lines — unchanged)
- `CallConnection` + `Dispatcher` wiring (stays — protocol)
- `RemoteIdentity`/`CallCredentials` — removed (now in `alknet-core`,
  re-imported from there)
- `ClientError` — removed
- `connect` + all `build_*`/`select_*`/`load_*`/`Ed25519SigningKey`/
  `RawKeyClientCertResolver`/`NoClientCertResolver`/
  `FingerprintPinVerifier` — removed (now in `alknet-tls`)

**The `Cargo.toml` after prune:** `quinn`, `rustls`,
`rustls-native-certs`, `rustls-pemfile` all leave. The `quinn` feature
either disappears or becomes a no-op (it only gated `connect` + the
TLS helpers, both removed). `alknet-call` becomes a pure protocol crate.

**Test rewrite:** the ~290 lines of tests in `call_client.rs` that use
`connect` need to switch to either:
- `spawn_dispatch` directly with a `Connection::from_stream` mock (for
  protocol-level tests — the dispatch loop, the wire protocol, the
  pending-request map), or
- `AlknetClient::dial_quic` + `spawn_dispatch` (for integration tests
  that need a real TLS handshake — these move to `alknet-client`'s
  integration tests or a dev-dependency on `alknet-client`).

This is the most fiddly phase — the test rewrite is the bulk of the
work. The implementation prune is mechanical (~140 lines deleted); the
test rewrite is ~290 lines of restructuring.

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

**Verification needed:** confirm that `accept_bi()` on a `from_bidi`
connection yields the single bidi stream per ADR-070's yield-once
contract, and that the yielded streams are directly usable as
`AsyncRead+AsyncWrite` (no wrapper needed). If the yield-once semantics
require a different path for single-stream connections, the fix is
slightly larger — but the `serve_io` signature already accepts any
`AsyncRead+AsyncWrite`, so the adapter is ready.

**Compilable state:** `cargo test -p alknet-http` passes. The crate no
longer has the hand-rolled `QuicStream` wrapper.

**Done when:** `cargo test -p alknet-http` passes, the `QuicStream`
wrapper is gone.

---

## Intermediate states (compilable after each phase)

| After phase | State |
|-------------|-------|
| 1 (tls) | `alknet-tls` builds standalone; core/call/http unchanged (old code duplicated) |
| 2 (endpoint) | `alknet-endpoint` builds standalone; core still has old `endpoint.rs` (duplicate) |
| 3 (client) | `alknet-client` builds standalone; call still has old `connect` (duplicate) |
| 4 (core prune) | core is lightweight; `endpoint.rs` gone; `CallCredentials` in core |
| 5 (call prune) | call is pure protocol; `connect` + TLS helpers gone; tests rewritten |
| 6 (http fix) | http has no `QuicStream` wrapper; clean `accept_bi` path |

Phases 1-3 are purely additive — no existing code changes, no
breakage. Phases 4-5 are subtractive — the pruned code's callers don't
exist yet (no assembly layer), so the breakage is confined to the
crate's own tests. Phase 6 is a small fix.

## Ordering rationale

The ordering is **deps before dependents, additive before subtractive**:

- `alknet-tls` first because both `alknet-endpoint` (indirectly — the
  assembly layer builds `TlsServerConfig`) and `alknet-client`
  (directly — `TlsClientConfig`) depend on it. It has no dep on the
  other new crates.
- `alknet-endpoint` second because it depends only on `alknet-core`
  (already exists) — it doesn't need `alknet-tls` (the endpoint takes
  pre-built transports). It could go before `alknet-tls`, but putting
  tls first means the assembly layer's transport-building code has a
  home from the start.
- `alknet-client` third because it depends on `alknet-tls`
  (`TlsClientConfig`) + `alknet-core` (`CallCredentials` — which is
  still in `alknet-call` until Phase 4). So `alknet-client` Phase 3
  uses `CallCredentials` from `alknet-call` temporarily, then Phase 4
  moves it to core and Phase 5 updates the import. Alternatively,
  Phase 4 (the `CallCredentials` move) could go before Phase 3 — but
  that would make Phase 3 depend on a subtractive phase, which we want
  to avoid. The temporary `alknet-call` dep in Phase 3 is acceptable
  (it's the existing location; the crate already depends on it).

  **Alternative:** move `CallCredentials`/`RemoteIdentity` to core as
  a standalone step (Phase 0.5) before Phase 3, so `alknet-client`
  never depends on `alknet-call`. This is a small additive change to
  core (add the types, re-export from call) + a small subtractive
  change to call (remove the definitions, re-import from core). It
  keeps Phase 3 clean. Worth considering if the temporary dep feels
  wrong.

- Phases 4-5 (the prunes) go last because they're subtractive. The
  new crates (1-3) must exist first so the pruned code's
  functionality has a home.
- Phase 6 (http fix) goes last because it's independent of the
  extraction — it's a residual fix that could happen at any point
  after ADR-065 landed (which it did). Putting it last keeps the
  extraction phases clean.

## Open questions for the migration plan

- **Phase 3 `CallCredentials` location:** temporary `alknet-call` dep
  in Phase 3, or a Phase 0.5 move-to-core first? (See "Alternative"
  above.)
- **Phase 5 test rewrite scope:** which `call_client.rs` tests are
  protocol-level (rewrite to `spawn_dispatch` + mock) vs. integration
  (rewrite to `AlknetClient::dial_quic` + `spawn_dispatch`, or move to
  `alknet-client`'s integration tests)? Needs a test-by-test audit.
- **Phase 6 `accept_bi` semantics:** does `accept_bi()` on a
  `from_bidi` connection yield the single bidi stream directly usable
  as `AsyncRead+AsyncWrite`, or does it need a wrapper? Needs
  verification against the `BidiStreamSource` impl for `from_bidi`
  in `types.rs`.
- **Workspace `Cargo.toml`:** the three new crates need to be added to
  the workspace member list. Trivial but worth noting.