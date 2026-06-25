---
status: open
last_updated: 2026-06-25
reviewed_artifacts:
  - crates/alknet-vault/src/{lib,cache,derivation,encryption,ethereum,mnemonic,protocol,service}.rs
  - crates/alknet-core/src/{lib,auth,config,endpoint,types}.rs
  - crates/alknet-call/src/{lib,registry/*,protocol/*}.rs
tool: cargo-llvm-cov 0.8.4
invocation: cargo llvm-cov --workspace --all-features
reviewer: coverage pass (post-review-#004)
---

# Coverage Analysis #005

## Purpose

First dedicated code-coverage pass after the implementation round and the
review-#004 fix round. Goal: locate the weakly-covered areas that should be
tightened up while the surface is still small, and identify anything that
needs research (ACME was flagged upfront as a likely hard spot).

Headline result: **coverage is in good shape for a single implementation +
single review pass.** Workspace line coverage is **87.1%** (5759/6615), all
224 tests pass, and the vault + registry layers are essentially fully
covered. The remaining gaps concentrate in three files (endpoint.rs, types.rs,
connection.rs) and almost all of them share a single root cause: tests build
`Connection`/`CallConnection` over a `MockConnection` whose `open_bi`/`accept_bi`
return `Err(StreamClosed)`, so the real stream and accept-loop machinery is
never exercised. That's a testing-infrastructure gap, not a logic gap.

## Methodology

- `cargo llvm-cov --workspace --all-features --lcov --output-path cov.lcov`
  (matches the project convention of building/testing with `--all-features`;
  `--all-features` matters here because `endpoint.rs` has large `#[cfg(feature
  = "acme")]` and `#[cfg(feature = "iroh")]` blocks that are invisible under
  default features).
- Per-file line + region breakdown from the lcov export.
- Uncovered line ranges mapped back to source for the three weak files and
  the mid-tier files (adapter.rs, config.rs, protocol.rs).
- Each gap classified by testability: pure-function, needs-real-stream, or
  needs-network (ACME).

## Summary Statistics

| Severity | Count |
|----------|-------|
| Critical | 0 |
| Warning  | 0 |
| Suggestion | 8 (S1–S8) |

No correctness findings — this is a coverage pass, not a logic review (that
was #004). Everything below is a suggestion ordered by leverage, not
severity.

---

## Per-File Coverage

| File | Lines | Covered | % |
|------|------:|--------:|----:|
| alknet-vault/src/derivation.rs | 141 | 141 | 100.0% |
| alknet-vault/src/encryption.rs | 123 | 123 | 100.0% |
| alknet-vault/src/ethereum.rs | 126 | 126 | 100.0% |
| alknet-core/src/auth.rs | 192 | 192 | 100.0% |
| alknet-call/src/protocol/abort.rs | 250 | 250 | 100.0% |
| alknet-call/src/registry/registration.rs | 543 | 538 | 99.1% |
| alknet-call/src/registry/env.rs | 374 | 369 | 98.7% |
| alknet-call/src/registry/discovery.rs | 418 | 412 | 98.6% |
| alknet-vault/src/cache.rs | 289 | 286 | 99.0% |
| alknet-call/src/protocol/wire.rs | 355 | 347 | 97.7% |
| alknet-call/src/protocol/pending.rs | 409 | 398 | 97.3% |
| alknet-vault/src/service.rs | 408 | 389 | 95.3% |
| alknet-call/src/registry/spec.rs | 197 | 187 | 94.9% |
| alknet-call/src/registry/context.rs | 89 | 86 | 96.6% |
| alknet-vault/src/mnemonic.rs | 78 | 74 | 94.9% |
| alknet-core/src/config.rs | 232 | 218 | 94.0% |
| alknet-call/src/protocol/adapter.rs | 726 | 669 | 92.1% |
| alknet-vault/src/protocol.rs | 90 | 78 | 86.7% |
| **alknet-core/src/types.rs** | **374** | **212** | **56.7%** |
| **alknet-core/src/endpoint.rs** | **837** | **468** | **55.9%** |
| **alknet-call/src/protocol/connection.rs** | **364** | **196** | **53.8%** |
| **TOTAL** | **6615** | **5759** | **87.1%** |

Vault and registry layers: solid. Three weak files: the focus of this pass.

---

## Suggestions

### S1. `dispatch_envelope` and `SubscriptionStream` are untested pure logic (connection.rs)

**File**: `crates/alknet-call/src/protocol/connection.rs:206–230`, `299–345`

**Problem**: `dispatch_envelope` is a pure function over
`&Arc<Mutex<PendingRequestMap>>` + an `EventEnvelope` — it matches on
`EVENT_RESPONDED`, `EVENT_COMPLETED`, `EVENT_ABORTED`, `EVENT_ERROR`, and `_`,
calling `handle_*` on the pending map. None of these arms is exercised
today (only `register_imported*` / `overlay_env` tests exist in this file).
Likewise `SubscriptionStream::{new, closed}` and the three `Stream::poll_next`
branches (`Ok` value, `Err` error, channel-closed) are pure async plumbing
with zero coverage.

**Fix**: ~45 lines of unit tests. Drive `dispatch_envelope` with a freshly
registered pending entry and each of the five event types; assert the
pending map transitions. For `SubscriptionStream`, wire an `mpsc::channel`,
push `Ok`/`Err`/`None`, poll, and assert the three envelopes/termination.

**Lift**: ~45 uncovered lines in connection.rs, no new infrastructure.

### S2. `map_*_connection_error` and error `Display`/`Debug`/`source` for all variants (types.rs)

**File**: `crates/alknet-core/src/types.rs:134–217`, `491–513`

**Problem**: `map_quinn_connection_error` and `map_iroh_connection_error`
are pure mappings from `ConnectionError` variants to `StreamError`. None of
their arms (`TimedOut`, `ConnectionClosed`, `ApplicationClosed`, `Reset`,
`other`) is hit — the only tests use `MockConnection`. Separately, the
`HandlerError` and `StreamError` `Debug`/`Display`/`error::Error::source`
impls are only exercised for `HandlerError::AuthRequired`'s Display; the
other 11 arms are uncovered.

**Fix**: ~50 lines of unit tests. Construct each `ConnectionError` variant
(quinn exposes them publicly; iroh's are `iroh::endpoint::ConnectionError`)
and assert the `StreamError` mapping. Format each `HandlerError`/`StreamError`
variant via `format!("{:?}", e)` / `format!("{e}")` and assert the strings;
call `.source()` and assert `Some`/`None` per variant.

**Lift**: ~80 uncovered lines in types.rs, no new infrastructure. This is the
single highest-leverage pure-function addition in the pass.

### S3. `Capabilities` zeroize/default and `Ed25519SecretKey` from_bytes/sign/Debug (config.rs, types.rs)

**File**: `crates/alknet-core/src/types.rs:62–67`, `107–109`;
`crates/alknet-core/src/config.rs:41–62`

**Problem**: `Capabilities::zeroize`, `Capabilities::default`, and
`Ed25519SecretKey::{from_bytes, sign, Debug}` are trivial but untested.

**Fix**: ~15 lines. Round-trip a key through `from_bytes`/`as_bytes`, sign a
message and verify with `ed25519-dalek`'s `VerifyingKey`, assert the Debug
output is `Ed25519SecretKey { .. }`. Call `Capabilities::default()` and
`Capabilities::zeroize()` on a populated instance, assert cleared.

**Lift**: ~15 lines across two files.

### S4. Tier A of endpoint.rs: directly-callable TLS/rustls helpers

**File**: `crates/alknet-core/src/endpoint.rs`

A cluster of endpoint.rs functions are synchronous and directly callable but
only reachable today via a live QUIC handshake. They can be unit-tested in
isolation:

- `build_rustls_server_config` `RawKey` and `SelfSigned` arms
  (endpoint.rs:635–657) — only `X509` is tested (via
  `tls_setup_x509_returns_no_acme_state`). Call the function directly with
  each `TlsIdentity` variant, assert the returned config has the supplied
  ALPNs and `max_early_data_size == u32::MAX`.
- `build_quinn_server_config_from_rustls` (endpoint.rs:603–612) — feed a
  rustls `ServerConfig`, assert `Ok`.
- `load_private_key` empty-file error path (endpoint.rs:713–717) — write an
  empty file with `tempfile`, assert `Err(TlsConfig)`.
- `has_iroh_identity` (endpoint.rs:256–261) — pure bool over
  `StaticConfig`; assert both `Some(RawKey)` and `None`/`X509` branches.
- `HandlerRegistry::default` and `Debug for AlknetEndpoint`
  (endpoint.rs:73–75, 112–117) — trivial.
- `AcceptAnyCertVerifier` trait methods (endpoint.rs:758–809) —
  `offer_client_auth`, `client_auth_mandatory`, `root_hint_subjects`,
  `verify_client_cert`, `verify_tls12_signature`, `verify_tls13_signature`,
  `supported_verify_schemes` are all pure/synchronous. Call each directly and
  assert (`assertion()` results, the scheme vector contents, etc.). These
  are currently lit only by a live TLS handshake that never runs in tests.
- `RawKeyCertResolver::resolve` (endpoint.rs:832–837) and
  `Ed25519SigningKey::{choose_scheme (both branches), algorithm, public_key,
  sign}` (endpoint.rs:880–908) — directly callable. `only_raw_public_keys`
  is already tested; the rest isn't. Assert `choose_scheme` returns `Some`
  when `ED25519` is offered and `None` when it isn't; assert `sign` produces
  a 64-byte signature that verifies against the public key.

**Fix**: ~110 lines of unit tests, all `#[cfg(feature = "quinn")]`-gated
where appropriate, no networking.

**Lift**: endpoint.rs from ~56% to roughly **80%**. This is the bulk of the
endpoint.rs gap and the largest single win in the pass.

### S5. Tier B: one loopback quinn integration test (the real unlock)

**Files**: `crates/alknet-core/src/endpoint.rs:135–252, 264–364, 376–457`;
`crates/alknet-core/src/types.rs:254–360, 385–478`;
`crates/alknet-call/src/protocol/connection.rs:75–204`;
`crates/alknet-call/src/protocol/adapter.rs:205–290, 442–447`

**Problem**: The accept loops (`run_quinn_accept_loop`,
`run_iroh_accept_loop`), the dispatch functions (`dispatch_quinn`,
`dispatch_iroh`), the fingerprint extractors, `AlknetEndpoint::{new, run,
shutdown}` quinn/iroh paths, `Connection::{accept_bi, open_bi, close}` for
quinn/iroh, `SendStream`/`RecvStream` `AsyncRead`/`AsyncWrite` for quinn/iroh,
`CallConnection::{call, subscribe, abort, write_request, write_envelope,
read_stream_until_closed}`, and `CallAdapter::handle` / `handle_stream` are
all uncovered. They share one root cause: no test ever drives a real
bi-directional stream.

**Fix**: One integration test module (e.g. `crates/alknet-core/tests/
loopback_quinn.rs`) that:

1. Builds a self-signed cert (reuse `generate_self_signed_cert`'s approach,
   exposed via a test helper).
2. Binds a quinn server endpoint on `127.0.0.1:0` with a `DummyHandler`
   registered for `b"alknet/test"` that echoes the inbound frame.
3. Connects a quinn client, opens a bi-stream, writes a frame, reads the
   echo.
4. Shuts down via `AlknetEndpoint::shutdown`.

That single test lights up `run_quinn_accept_loop`, `dispatch_quinn`,
`extract_quinn_alpn`, `extract_quinn_client_fingerprint`, `build_auth_context`
on a live conn, the `AcceptAnyCertVerifier` callbacks during the handshake,
`Connection::{accept_bi, open_bi, close}`, and the `SendStream`/`RecvStream`
quinn arms. An iroh variant does the same for the iroh side.

The same loopback then carries a `CallConnection::call` round-trip and a
`CallAdapter::handle` session for connection.rs and adapter.rs. One harness,
four files benefit.

**Lift**: lights up ~300 lines across the four weak files. After S1–S4 +
S5, workspace coverage should land around **93–94%**.

### S6. ACME event-loop extraction (the flagged research item)

**File**: `crates/alknet-core/src/endpoint.rs:521–599`

**Problem**: `TlsSetup::new_acme` and its spawned event loop
(endpoint.rs:552–593) handle 11 `EventOk`/`EventError` arms. None is
covered. The `Acme` arm dispatch in `AlknetEndpoint::new`
(endpoint.rs:147–150) likewise never runs. ACME was flagged upfront as a
likely hard spot — confirmed: the `rustls_acme::AcmeConfig` builder owns the
network, so it can't be driven directly in a unit test.

**Options, in increasing fidelity**:

1. **Extract the event handler** (endpoint.rs:552–593) from the
   `tokio::spawn` closure into a named `async fn run_acme_event_loop(state,
   domains: Vec<String>)` that takes the `AcmeState` stream. Then unit-test
   it by feeding a synthetic `futures::stream::iter([...])` of `EventOk` and
   `EventError` values and asserting it consumes them without panicking.
   Covers all 11 match arms and the loop termination. **Pure refactor, no
   behavior change.** Recommended primary path — also makes the event-loop
   logic inspectable and debuggable.
2. **Pebble** (Let's Encrypt's ACME test server) in a Docker dev-container,
   pointed at via `AcmeDirectory::Custom(pebble_url)`. Highest fidelity
   (real issuance against a local ACME server), but adds a CI/dev
   dependency and is slower. Treat as a future CI-hardening step, not a
   unit test.
3. **Mock `AcmeConfig`** — not viable; the builder is not mockable.

Recommend option 1 now; option 2 as a follow-up CI task if/when ACME
becomes load-bearing in production.

**Lift**: ~50 lines (the 11 match arms + the loop body), no network.

### S7. `adapter.rs` non-request event arms and `handle` accept loop

**File**: `crates/alknet-call/src/protocol/adapter.rs:205–290, 442–447`

**Problem**: `handle_stream`'s `EVENT_ABORTED` arm (which invokes
`AbortCascade::cascade_abort` — the W1 fix from review #004) and the
unknown-event arm are untested, as is `CallAdapter::handle`'s accept loop
and the `fail_all`-on-close path. `identity_provider()` accessor (76–78)
is also untested.

**Fix**: The `EVENT_ABORTED` arm needs a crafted `EventEnvelope` fed into
`handle_stream` with a mock `SendStream`/`RecvStream` pair (the loopback
from S5 covers `handle`; the abort arm specifically wants a direct
`handle_stream` test with a prefilled `PendingRequestMap`). The
`identity_provider()` accessor is a one-line test.

**Lift**: ~25 lines; depends on S5's loopback for the `handle` path.

### S8. `DerivedKey` redacted-payload deserialize rejection (vault protocol.rs)

**File**: `crates/alknet-vault/src/protocol.rs:78–89`

**Problem**: The deserialize guard that rejects `private_key ==
"[REDACTED]"` is untested. This is a security-adjacent path (prevents a
redacted `DerivedKey` from being silently re-imported with a bogus key).

**Fix**: One test — serialize a `DerivedKey`, mangle the JSON to set
`private_key` to `"[REDACTED]"`, deserialize, assert the `Err` message
contains "redacted". ~10 lines.

**Lift**: 12 uncovered lines; closes a security-adjacent gap.

---

## Recommended Order

1. **S1, S2, S3** — pure-function tests across connection.rs / types.rs /
   config.rs. Lowest effort, ~140 uncovered lines, no new infrastructure.
2. **S4** — Tier A of endpoint.rs (TLS/rustls helpers, verifier callbacks,
   signing-key trait impls). ~110 lines of tests, lifts endpoint.rs to ~80%.
3. **S5** — the loopback quinn integration test. One harness unlocks the
   accept/dispatch/stream paths across endpoint.rs, types.rs, connection.rs,
   and adapter.rs. After this, workspace coverage should be ~93–94%.
4. **S6** — ACME event-loop extraction + synthetic-stream unit test. Covers
   the 11 match arms without network; sets up Pebble as a future CI step.
5. **S7, S8** — the remaining small gaps; S7 partly rides on S5's loopback.

## Notes

- All line numbers are against the tree at the time of this pass and use
  `--all-features`; coverage under default features understates endpoint.rs
  because the `acme` and `iroh` cfg blocks are invisible to the instrumenter.
- No critical or warning findings: this pass confirms that the gaps are
  testing-only. The underlying logic was reviewed in #004 and the fix round
  landed cleanly.
- The concentration of gaps in three files, all stemming from the same
  mock-connection limitation, is a good sign — one loopback harness (S5)
  closes most of them, rather than requiring per-path test scaffolding.