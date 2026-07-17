---
id: endpoint/tests
name: Move and adapt endpoint tests from alknet-core/endpoint.rs into alknet-endpoint
status: completed
depends_on: [endpoint/accept-quinn, endpoint/accept-iroh, endpoint/accept-tcp-tls]
scope: moderate
risk: medium
impact: component
level: implementation
---

## Description

Phase 2, Task 8 of the crate extraction. Move the endpoint-related tests from
`crates/alknet-core/src/endpoint.rs` into `crates/alknet-endpoint/`. Adapt them to
test the new ADR-083 API shape (`new(handlers, dynamic, identity_provider, drain_timeout)`
+ `with_quinn`/`with_iroh`/`with_tcp_tls` + infallible `shutdown()`).

The old tests **stay** in `endpoint.rs` (duplicated) — no breakage. The new crate's
tests are self-contained and pass standalone.

### Tests to move and adapt

#### Category A — Registry tests (5 tests, minimal adaptation)

Move to `crates/alknet-endpoint/src/registry.rs` `#[cfg(test)] mod tests`:

| Test | Line | Adaptation |
|------|------|------------|
| `handler_registry_new_is_empty` | 967 | Import `HandlerRegistry` from `crate::registry`; no other changes |
| `handler_registry_register_then_get` | 974 | Same |
| `handler_registry_multiple_alpns` | 983 | Same |
| `handler_registry_register_panics_on_duplicate` | 1000 | Same |
| `handler_registry_debug_lists_alpns` | 1007 | Same |
| `handler_registry_default_is_empty` | 1311 | Same |
| `handler_registry_debug_lists_alpns_via_default` | 1319 | Same |

These tests use `DummyHandler` + `make_handler()` helpers — move those helpers too.

#### Category B — `build_auth_context` tests (3 tests, minimal adaptation)

Move to `crates/alknet-endpoint/src/dispatch.rs` `#[cfg(test)] mod tests`:

| Test | Line | Adaptation |
|------|------|------------|
| `build_auth_context_resolves_identity_from_fingerprint` | 1026 | Import `build_auth_context` from `crate::dispatch`; feature-gate on `quinn` or `iroh` |
| `build_auth_context_no_fingerprint_no_identity` | 1058 | Same |
| `build_auth_context_fingerprint_unknown_identity_none` | 1076 | Same |

These tests use `IdentityProvider` + `AuthToken` + `Identity` from `alknet_core::auth`.
Feature-gate on `#[cfg(any(feature = "quinn", feature = "iroh"))]` (same as old code).

#### Category C — `dispatch_decision_logic_lookup_and_auth` (1 test, moderate adaptation)

Move to `crates/alknet-endpoint/src/dispatch.rs` `#[cfg(test)] mod tests`:

| Test | Line | Adaptation |
|------|------|------------|
| `dispatch_decision_logic_lookup_and_auth` | 1212 | Tests handler lookup + `build_auth_context` together. Adapt to use `crate::registry::HandlerRegistry` + `crate::dispatch::build_auth_context`. Feature-gate on `#[cfg(any(feature = "quinn", feature = "iroh"))]`. |

#### Category D — `has_iroh_identity` tests (3 tests, significant adaptation)

| Test | Line | Adaptation |
|------|------|------------|
| `has_iroh_identity_true_for_raw_key` | 1327 | **`has_iroh_identity` does not exist in the new crate** — the transport-building decision moves to the assembly layer. Replace with a test that verifies `AlknetEndpoint::new(...).with_iroh(endpoint)` sets the iroh field correctly. |
| `has_iroh_identity_false_for_x509` | 1341 | Same — replace with a test that verifies `with_iroh` is not called (iroh field stays `None`). |
| `has_iroh_identity_false_when_no_identity` | 1356 | Same — replace with a test that verifies the endpoint works without iroh. |

These tests need to be rewritten for the new API shape. The old tests verified a
transport-building decision (`has_iroh_identity`); the new tests verify the builder
pattern (`with_iroh` sets the field, omitting it leaves it `None`).

#### Category E — Endpoint construction + run + shutdown tests (3 tests, significant adaptation)

| Test | Line | Adaptation |
|------|------|------------|
| `endpoint_constructs_with_iroh_raw_key_identity` | 1108 | Old: `AlknetEndpoint::new(&static_config, ...)`. New: `AlknetEndpoint::new(registry, dynamic, provider, timeout).with_iroh(endpoint)`. Need to build an iroh endpoint first. Feature-gate on `iroh`. |
| `iroh_endpoint_runs_accept_loop_and_shutdown` | 1139 | Old: uses `StaticConfig` to build iroh internally. New: build iroh endpoint externally, pass via `with_iroh`. The `CountingHandler` pattern stays. Feature-gate on `iroh`. |
| `debug_for_alknet_endpoint_is_implemented_without_panicking` | 1578 | Old: `AlknetEndpoint::new(&static_config, ...)`. New: `AlknetEndpoint::new(registry, dynamic, provider, timeout)`. No transport needed for Debug test. Feature-gate on `quinn` (or no feature gate — Debug is always available). |

### Test helpers to move

From `endpoint.rs` lines 944-965:

```rust
struct DummyHandler { alpn: &'static [u8] }
impl ProtocolHandler for DummyHandler { ... }
fn make_handler(alpn: &'static [u8]) -> Arc<dyn ProtocolHandler> { ... }
```

Move to a shared `#[cfg(test)]` module or duplicate in each test module that needs them.
The `CountingHandler` (lines 1162-1179) is only used by the iroh accept-loop test — move
it there.

### Test adaptations summary

1. **Imports**: Update `crate::config::*` → `alknet_core::config::*`, `crate::auth::*` →
   `alknet_core::auth::*`, `crate::types::*` → `alknet_core::types::*`. Use
   `crate::registry::HandlerRegistry`, `crate::dispatch::build_auth_context`,
   `crate::endpoint::AlknetEndpoint`.

2. **API shape**: All endpoint construction tests must use the new API:
   `AlknetEndpoint::new(registry, dynamic, provider, drain_timeout)` instead of
   `AlknetEndpoint::new(&static_config, registry, dynamic, provider)`.

3. **`shutdown()` is infallible**: Old tests call `.expect("shutdown ok")` on
   `shutdown().await`. New tests call `shutdown().await` directly (no `Result`).

4. **`has_iroh_identity` tests**: Rewritten to test the builder pattern instead.

5. **Feature gates**: Match the old feature gates where possible. Registry tests need no
   feature gates. `build_auth_context` tests need `#[cfg(any(feature = "quinn", feature = "iroh"))]`.
   Iroh tests need `#[cfg(feature = "iroh")]`. Debug test needs `#[cfg(feature = "quinn")]`
   (or no gate — Debug is always available; the old code gated it on `quinn` because
   `AlknetEndpoint` itself was gated).

### What stays in the original file

The old tests in `endpoint.rs` are **not deleted** — they stay as duplicates. The prune
happens in Phase 4. This task only adds tests to `alknet-endpoint`.

## Acceptance Criteria

- [ ] All 7 registry tests moved to `registry.rs` and pass
- [ ] All 3 `build_auth_context` tests moved to `dispatch.rs` and pass
- [ ] `dispatch_decision_logic_lookup_and_auth` moved to `dispatch.rs` and passes
- [ ] 3 `has_iroh_identity` tests rewritten for builder pattern and pass
- [ ] `endpoint_constructs_with_iroh_raw_key_identity` adapted to new API and passes
- [ ] `iroh_endpoint_runs_accept_loop_and_shutdown` adapted to new API and passes
- [ ] `debug_for_alknet_endpoint_is_implemented_without_panicking` adapted to new API and passes
- [ ] Test helpers (`DummyHandler`, `make_handler`, `CountingHandler`) moved with their tests
- [ ] `shutdown()` calls are infallible (no `.expect()` on shutdown)
- [ ] Feature gates correct on all moved tests
- [ ] `cargo test -p alknet-endpoint` passes (all feature combos)
- [ ] `cargo test -p alknet-core` still passes (old tests untouched)
- [ ] `cargo clippy -p alknet-endpoint --all-targets` succeeds with no warnings

## References

- docs/research/alknet-crate-extraction/findings.md — Phase 2, test list
- docs/architecture/crates/endpoint/README.md — ADR-083 API shape
- crates/alknet-core/src/endpoint.rs — lines 935-1606 (source tests)

## Notes

> This is the test migration task — 17 tests total (7 registry + 3 auth_context + 1
> dispatch + 3 has_iroh_identity + 3 endpoint construction). The `has_iroh_identity`
> tests need the most adaptation because `has_iroh_identity` doesn't exist in the new
> crate (the transport-building decision moves to the assembly layer). They're rewritten
> to test the builder pattern instead. The endpoint construction tests need adaptation
> for the new `new()` API (no `StaticConfig`). The old tests stay in `endpoint.rs` —
> the prune happens in Phase 4. Risk is medium because of the test rewrites.

## Summary

> To be filled on completion
